//! The project card's data: the project, its TASKS.md tasks and its threads,
//! each with its git line and model badges.
//!
//! The git reads (a script per thread), the profile lookups and the trailer
//! scans never run inside a request: the card answers at once with what the
//! cache holds, starts what is missing or stale in the background, and sends
//! a `card` event when something it learned changes the card, so the page
//! asks again. The previous answer stays on screen until a new one arrives.

use crate::app::{now_ms, Shared};
use crate::model::Project;
use crate::poller;
use crate::sources::git::{self, GitInfo};
use crate::sources::models::{self, Resolved, ThreadModels};
use crate::sources::profiles::{self, Assignable};
use crate::sources::{report, runner, tasks, threads};
use crate::store::{mem_key, State};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// How long a thread's git line is trusted before it is read again.
const GIT_TTL: Duration = Duration::from_secs(10);
/// How long a resolved profile or an assignable list is trusted.
const SLOW_TTL: Duration = Duration::from_secs(600);
/// Finished threads are read for a git line only for the newest few …
const MAX_RESOLVED_PROBES: usize = 8;
/// … and for trailers only for this many newest per project.
const MAX_TRAILER_THREADS: usize = 60;
const MAX_TRIES: u32 = 3;

pub struct Slot<T> {
    pub data: Option<T>,
    pub at: Option<Instant>,
    pub busy: bool,
    pub tries: u32,
    pub done: bool,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot {
            data: None,
            at: None,
            busy: false,
            tries: 0,
            done: false,
        }
    }
}

impl<T> Slot<T> {
    fn fresh(&self, ttl: Duration) -> bool {
        self.at.is_some_and(|a| a.elapsed() < ttl)
    }
}

#[derive(Default)]
pub struct Cache {
    /// `slug/id` → the thread's git summary.
    git: HashMap<String, Slot<GitInfo>>,
    /// `slug/id` → the trailers of a finished thread, from its repository.
    trailers: HashMap<String, Slot<GitInfo>>,
    /// Profile name → what `profile resolve` said (none: it did not resolve).
    profiles: HashMap<String, Slot<Option<Resolved>>>,
    /// Project → the profiles a task can be handed to, or why not.
    assign: HashMap<String, Slot<Result<Assignable, String>>>,
    /// Project → its threads as last listed (for the actions on one thread).
    threads: HashMap<String, Vec<threads::Thread>>,
    /// `slug\ntask` of the actions that are running.
    pub running: HashSet<String>,
}

fn emit_card(app: &Shared, slug: &str) {
    app.emit("card", &json!({ "slug": slug }));
}

pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && !slug.contains('/')
        && !slug.contains('\\')
        && !slug.starts_with('.')
        && !slug.starts_with('-')
}

pub fn argv_projects(app: &Shared, rest: &[&str]) -> Vec<String> {
    let mut argv = runner::argv([
        app.config.herdr_projects.as_str(),
        "--root",
        &app.config.root.display().to_string(),
    ]);
    argv.extend(rest.iter().map(|s| s.to_string()));
    argv
}

// ---------------------------------------------------------------------------
// Background reads

fn ensure_git(app: &Shared, slug: &str, t: &threads::Thread, path: &str) {
    let key = format!("{slug}/{}", t.id);
    {
        let mut c = app.cache.lock().unwrap();
        let slot = c.git.entry(key.clone()).or_default();
        if slot.busy || slot.fresh(GIT_TTL) {
            return;
        }
        slot.busy = true;
    }
    let (app, slug, path, base) = (
        app.clone(),
        slug.to_string(),
        path.to_string(),
        t.base.clone(),
    );
    tokio::spawn(async move {
        let _permit = app.git_gate.acquire().await;
        let info = git::summary(&path, &base).await;
        let changed = {
            let mut c = app.cache.lock().unwrap();
            let slot = c.git.entry(key).or_default();
            let changed = slot.data.as_ref() != Some(&info);
            slot.data = Some(info);
            slot.at = Some(Instant::now());
            slot.busy = false;
            changed
        };
        if changed {
            emit_card(&app, &slug);
        }
    });
}

fn ensure_trailers(app: &Shared, slug: &str, t: &threads::Thread) {
    let key = format!("{slug}/{}", t.id);
    {
        let mut c = app.cache.lock().unwrap();
        let slot = c.trailers.entry(key.clone()).or_default();
        if slot.done || slot.busy || slot.tries >= MAX_TRIES {
            return;
        }
        slot.busy = true;
    }
    let (app, slug) = (app.clone(), slug.to_string());
    let (repo, branch, base, pr) = (
        t.repo.clone(),
        t.branch.clone(),
        t.base.clone(),
        t.pr_number(),
    );
    tokio::spawn(async move {
        let _permit = app.trailer_gate.acquire().await;
        let result = git::trailers(&repo, &branch, &base, pr).await;
        let changed = {
            let mut c = app.cache.lock().unwrap();
            let slot = c.trailers.entry(key).or_default();
            slot.busy = false;
            match result {
                Ok(info) => {
                    slot.done = true;
                    let changed = slot.data.as_ref() != Some(&info);
                    slot.data = Some(info);
                    changed
                }
                Err(_) => {
                    slot.tries += 1;
                    false
                }
            }
        };
        if changed {
            emit_card(&app, &slug);
        }
    });
}

fn ensure_profile(app: &Shared, slug: &str, name: &str) {
    {
        let mut c = app.cache.lock().unwrap();
        let slot = c.profiles.entry(name.to_string()).or_default();
        if slot.busy || slot.fresh(SLOW_TTL) {
            return;
        }
        slot.busy = true;
    }
    let (app, slug, name) = (app.clone(), slug.to_string(), name.to_string());
    tokio::spawn(async move {
        let argv = argv_projects(&app, &["profile", "resolve", &name]);
        let resolved = match runner::run(&argv, None, Duration::from_secs(10)).await {
            Ok(out) if out.ok() => models::parse_resolve(&out.stdout),
            _ => None,
        };
        let changed = {
            let mut c = app.cache.lock().unwrap();
            let slot = c.profiles.entry(name).or_default();
            let changed = slot.data.as_ref() != Some(&resolved);
            slot.data = Some(resolved);
            slot.at = Some(Instant::now());
            slot.busy = false;
            changed
        };
        if changed {
            emit_card(&app, &slug);
        }
    });
}

/// The profiles a task of `slug` can be handed to (cached for ten minutes).
pub async fn assignable(app: &Shared, slug: &str) -> Result<Assignable, String> {
    {
        let c = app.cache.lock().unwrap();
        if let Some(slot) = c.assign.get(slug) {
            if slot.fresh(SLOW_TTL) {
                if let Some(d) = &slot.data {
                    return d.clone();
                }
            }
        }
    }
    let argv = argv_projects(app, &["assignable", slug]);
    let result = match runner::run(&argv, None, Duration::from_secs(20)).await {
        Ok(out) if out.ok() => Ok(profiles::parse_assignable(&out.stdout)),
        Ok(out) => Err(tasks::clip(
            &out.error().split_whitespace().collect::<Vec<_>>().join(" "),
            160,
        )),
        Err(e) => Err(e),
    };
    let mut c = app.cache.lock().unwrap();
    let slot = c.assign.entry(slug.to_string()).or_default();
    slot.data = Some(result.clone());
    // A failure is asked again at the next request.
    slot.at = result.is_ok().then(Instant::now);
    result
}

// ---------------------------------------------------------------------------
// The card

#[derive(Serialize)]
struct ThreadView {
    id: String,
    title: String,
    group: String,
    /// The group in words («в работе», «ревью»).
    state: String,
    /// herdr's note in words («PR слит», «закрыт вручную»), if it adds anything.
    note: Option<String>,
    closed: bool,
    failed: bool,
    error: String,
    rank: u32,
    branch: String,
    /// The branch without `hp/<project>/`.
    worktree: Option<String>,
    base: String,
    kind: String,
    path: Option<String>,
    wt_exists: bool,
    pr: String,
    pr_state: String,
    pr_number: Option<u32>,
    profile: String,
    machine: String,
    percent: Option<u32>,
    activity: String,
    /// What came of a finished thread, from its report.
    summary: Option<String>,
    git: Option<GitInfo>,
    models: ThreadModels,
    /// Everything the card's filter finds the thread by.
    search: String,
}

fn id_number(id: &str) -> u32 {
    id.trim_start_matches("t-")
        .split('-')
        .next()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0)
}

/// Reads the report summaries of finished threads and which worktrees are alive.
fn read_disk(dir: &Path, list: &[threads::Thread]) -> Vec<(bool, Option<String>)> {
    list.iter()
        .map(|t| {
            let alive = t
                .own_path()
                .is_some_and(|p| Path::new(p).join(".git").exists());
            let summary = t
                .closed()
                .then(|| {
                    std::fs::read(dir.join("threads").join(format!("{}.md", t.id)))
                        .ok()
                        .and_then(|b| {
                            let b = &b[..b.len().min(131_072)];
                            report::summary(&String::from_utf8_lossy(b))
                        })
                })
                .flatten();
            (alive, summary)
        })
        .collect()
}

/// The card of `slug`: `{project, tasks, qa, sent, threads, error, …}`.
pub async fn build(app: &Shared, slug: &str) -> Value {
    let dir = app.config.project_dir(slug);
    let known = app
        .snapshot
        .read()
        .unwrap()
        .projects
        .iter()
        .find(|p| p.slug == slug)
        .cloned();
    let project = known.unwrap_or_else(|| {
        let mut p = Project {
            slug: slug.to_string(),
            name: slug.to_string(),
            status: "active".into(),
            ..Default::default()
        };
        poller::enrich(&mut p, &dir);
        p
    });

    let task_text = tokio::fs::read_to_string(dir.join("TASKS.md")).await.ok();
    let task_list = task_text
        .as_deref()
        .map(tasks::parse_tasks)
        .unwrap_or_default();

    let argv = argv_projects(app, &["thread", "list", slug, "--json"]);
    let (mut list, error): (Vec<threads::Thread>, Option<String>) =
        match runner::run(&argv, None, Duration::from_secs(20)).await {
            Ok(out) if out.ok() => match threads::parse_threads(&out.stdout) {
                Ok(groups) => (groups.into_iter().flat_map(|g| g.threads).collect(), None),
                Err(e) => (vec![], Some(e)),
            },
            Ok(out) => (vec![], Some(out.error())),
            Err(e) => (vec![], Some(e)),
        };

    // The same reading as the project list: a finished thread is «ревью», not «ждёт тебя».
    {
        let mr = crate::poller::mr_states(app, slug, &list).await;
        let env = crate::attention::Env {
            now: now_ms(),
            stale_ms: app.state.lock().unwrap().settings.review_stale_ms(),
            mr: &mr,
        };
        for t in &mut list {
            crate::attention::normalize(t, &env);
        }
    }

    let disk = {
        let (dir, list) = (dir.clone(), list.clone());
        tokio::task::spawn_blocking(move || read_disk(&dir, &list))
            .await
            .unwrap_or_default()
    };

    // What the background reads must still fetch.
    let mut need_git: Vec<(usize, String)> = Vec::new();
    let mut need_trailers: Vec<usize>;
    let mut need_profiles: HashSet<String> = HashSet::new();
    {
        let mut resolved_live: Vec<usize> = Vec::new();
        for (i, t) in list.iter().enumerate() {
            let (alive, _) = &disk[i];
            if t.failed() {
                continue;
            }
            if !t.profile.is_empty() && !t.remote() {
                need_profiles.insert(t.profile.clone());
            }
            if *alive {
                if let Some(p) = t.own_path() {
                    if t.closed() {
                        resolved_live.push(i);
                    } else {
                        need_git.push((i, p.to_string()));
                    }
                }
            }
        }
        resolved_live.sort_by_key(|&i| std::cmp::Reverse(id_number(&list[i].id)));
        for &i in resolved_live.iter().take(MAX_RESOLVED_PROBES) {
            if let Some(p) = list[i].own_path() {
                need_git.push((i, p.to_string()));
            }
        }
        let mut finished: Vec<usize> = (0..list.len())
            .filter(|&i| {
                let t = &list[i];
                t.group == "Resolved"
                    && !t.failed()
                    && t.kind == "worktree"
                    && t.machine.is_empty()
                    && !t.repo.is_empty()
                    && (!t.branch.is_empty() || t.pr_number().is_some())
            })
            .collect();
        finished.sort_by_key(|&i| std::cmp::Reverse(id_number(&list[i].id)));
        need_trailers = finished.into_iter().take(MAX_TRAILER_THREADS).collect();
    }

    // Views from the cache as it is now.
    let mut views: Vec<ThreadView> = Vec::with_capacity(list.len());
    {
        let mut c = app.cache.lock().unwrap();
        c.threads.insert(slug.to_string(), list.clone());
        for (i, t) in list.iter().enumerate() {
            let (alive, summary) = &disk[i];
            let key = format!("{slug}/{}", t.id);
            let g = c.git.get(&key).and_then(|s| s.data.clone());
            let x = c.trailers.get(&key).and_then(|s| s.data.clone());
            let resolved = c
                .profiles
                .get(&t.profile)
                .and_then(|s| s.data.clone())
                .flatten();
            let m = models::of(
                t,
                resolved.as_ref(),
                g.as_ref().map(|g| g.co.as_slice()).unwrap_or(&[]),
                None,
                x.as_ref().map(|x| x.co.as_slice()).unwrap_or(&[]),
                x.as_ref().and_then(|x| x.src.as_deref()),
            );
            let note = threads::note_word(&t.group, &t.note);
            let state = threads::group_word(&t.group);
            let rank = if t.failed() && !t.closed() {
                0
            } else {
                threads::group_rank(&t.group)
            };
            let mut hay: Vec<String> = vec![
                t.id.clone(),
                t.title.clone(),
                t.note.clone(),
                t.group.clone(),
                state.clone(),
                note.clone().unwrap_or_default(),
                t.worktree_name().unwrap_or_default(),
                summary.clone().unwrap_or_default(),
                t.pr_state.clone(),
                t.profile.clone(),
                t.agent.clone(),
                m.launch.model.clone().unwrap_or_default(),
                m.launch.label.clone().unwrap_or_default(),
            ];
            if t.failed() {
                hay.push("сбой запуска не запустился".into());
            }
            hay.extend(m.badges.iter().map(|b| b.label.clone()));
            hay.extend(g.iter().flat_map(|g| g.co.clone()));
            hay.extend(x.iter().flat_map(|x| x.co.clone()));
            views.push(ThreadView {
                id: t.id.clone(),
                title: t.title.clone(),
                group: t.group.clone(),
                state,
                note,
                closed: t.closed(),
                failed: t.failed(),
                error: t.error.clone(),
                rank,
                branch: t.branch.clone(),
                worktree: t.worktree_name().map(|n| threads::short_worktree(&n, slug)),
                base: t.base.clone(),
                kind: t.kind.clone(),
                path: t.own_path().map(String::from),
                wt_exists: *alive,
                pr: t.pr.clone(),
                pr_state: t.pr_state.clone(),
                pr_number: t.pr_number(),
                profile: t.profile.clone(),
                machine: t.machine.clone(),
                percent: t.percent,
                activity: t.activity.clone(),
                summary: summary.clone(),
                git: if *alive && !t.failed() { g } else { None },
                models: m,
                search: hay
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" "),
            });
        }
        // A finished thread with a live worktree is read by the git summary first.
        need_trailers.retain(|&i| {
            let key = format!("{slug}/{}", list[i].id);
            if !disk[i].0 {
                return true;
            }
            match c.git.get(&key).and_then(|s| s.data.as_ref()) {
                None => false,
                Some(g) => g.co.is_empty(),
            }
        });
    }
    // Attention first, then the newest; finished threads after the open ones.
    views.sort_by(|a, b| {
        (a.closed, a.rank, std::cmp::Reverse(id_number(&a.id))).cmp(&(
            b.closed,
            b.rank,
            std::cmp::Reverse(id_number(&b.id)),
        ))
    });

    for (i, path) in &need_git {
        ensure_git(app, slug, &list[*i], path);
    }
    for i in &need_trailers {
        ensure_trailers(app, slug, &list[*i]);
    }
    for name in &need_profiles {
        ensure_profile(app, slug, name);
    }

    // Answers and «sent» marks, cleaned against a TASKS.md that was read.
    let texts: HashSet<String> = task_list.iter().map(|t| t.full.clone()).collect();
    let (qa, sent, agent) = {
        let all: Vec<String> = app
            .snapshot
            .read()
            .unwrap()
            .projects
            .iter()
            .map(|p| p.slug.clone())
            .collect();
        let mut read = app.tasks_read.lock().unwrap().clone();
        let mut st = app.state.lock().unwrap();
        let mut changed = false;
        if State::trusted(task_list.len(), task_text.is_some()) {
            let before = (st.qa.clone(), st.sent.clone());
            st.settle(slug, &texts, &mut read, &all);
            changed = before != (st.qa.clone(), st.sent.clone());
            *app.tasks_read.lock().unwrap() = read;
        }
        let mut qa = serde_json::Map::new();
        let mut sent = serde_json::Map::new();
        for t in &task_list {
            let key = mem_key(slug, &t.full);
            if let Some(list) = st.qa.get(&key).filter(|l| !l.is_empty()) {
                qa.insert(
                    t.full.clone(),
                    serde_json::to_value(list).unwrap_or(Value::Null),
                );
            }
            if let Some(v) = st.sent.get(&key) {
                sent.insert(t.full.clone(), v.clone());
            }
        }
        let agent = crate::agent::Kind::from_setting(&st.settings.agent).short();
        drop(st);
        if changed {
            app.save_state();
        }
        (qa, sent, agent)
    };

    // The check of the backlog runs in the background for the open project.
    crate::fresh::tick(app, slug, &project.repos, &task_list);
    let fresh = crate::fresh::card_part(app, slug, &task_list);

    json!({
        "project": project,
        "tasks": task_list,
        "fresh": fresh,
        "tasks_read": task_text.is_some(),
        "threads": views,
        "qa": qa,
        "sent": sent,
        "agent": agent,
        "error": error,
    })
}

/// A thread of `slug` as the last card listed it.
pub fn find_thread(app: &Shared, slug: &str, id: &str) -> Option<threads::Thread> {
    let c = app.cache.lock().unwrap();
    c.threads.get(slug)?.iter().find(|t| t.id == id).cloned()
}

/// A thread's block, opened: its git in full.
pub async fn thread_detail(app: &Shared, slug: &str, id: &str) -> Result<Value, String> {
    let t = find_thread(app, slug, id).ok_or("тред не найден, обновите карточку")?;
    let Some(path) = t.own_path().filter(|p| Path::new(p).join(".git").exists()) else {
        return Ok(json!({ "git": Value::Null }));
    };
    if t.failed() {
        return Ok(json!({ "git": Value::Null }));
    }
    let info = git::detail(path, &t.base).await;
    let mut c = app.cache.lock().unwrap();
    let slot = c.git.entry(format!("{slug}/{id}")).or_default();
    slot.data = Some(info.clone());
    slot.at = Some(Instant::now());
    Ok(json!({ "git": info }))
}

/// `w`: the thread's worktree, opened in herdr.
pub async fn open_worktree(app: &Shared, slug: &str, id: &str) -> Result<(), String> {
    let t = find_thread(app, slug, id).ok_or("тред не найден, обновите карточку")?;
    let path = t
        .own_path()
        .filter(|p| Path::new(p).join(".git").exists())
        .ok_or("ворктри удалён или у треда его нет")?
        .to_string();
    let argv = runner::argv([
        app.config.herdr.as_str(),
        "worktree",
        "open",
        "--path",
        path.as_str(),
        "--focus",
    ]);
    match runner::run(&argv, None, Duration::from_secs(15)).await {
        Ok(out) if out.ok() => Ok(()),
        Ok(out) => Err(tasks::clip(
            &out.error().split_whitespace().collect::<Vec<_>>().join(" "),
            200,
        )),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_slugs() {
        assert_eq!(id_number("t-0016"), 16);
        assert_eq!(id_number("t-0115-18"), 115);
        assert_eq!(id_number("x"), 0);
        assert!(valid_slug("herdr-planner"));
        assert!(!valid_slug("../x"));
        assert!(!valid_slug(".hidden"));
        assert!(!valid_slug("-x"));
        assert!(!valid_slug(""));
    }

    #[test]
    fn disk_reads_reports_and_worktrees() {
        let root = std::env::temp_dir().join(format!("hp-cards-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("threads")).unwrap();
        std::fs::create_dir_all(root.join("wt/.git")).unwrap();
        std::fs::write(
            root.join("threads/t-0001.md"),
            "## Report\n\nСделано: всё работает и проверено тестами.\n\n## Next\n",
        )
        .unwrap();
        let wt = root.join("wt");
        let list = vec![
            threads::Thread {
                id: "t-0001".into(),
                group: "Resolved".into(),
                kind: "worktree".into(),
                worktree_path: wt.display().to_string(),
                ..Default::default()
            },
            threads::Thread {
                id: "t-0002".into(),
                group: "Working".into(),
                kind: "tab".into(),
                cwd: wt.display().to_string(),
                ..Default::default()
            },
        ];
        let got = read_disk(&root, &list);
        assert!(got[0].0);
        assert_eq!(
            got[0].1.as_deref(),
            Some("Сделано: всё работает и проверено тестами.")
        );
        assert!(!got[1].0, "a tab thread has no worktree of its own");
        assert_eq!(got[1].1, None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
