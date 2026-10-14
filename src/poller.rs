//! Background work: the project list every 20 s (or on demand), the focused
//! herdr workspace every 3 s, the theme files every 2 s. Each loop publishes
//! on the event bus; nothing here blocks a request.

use crate::app::{now_ms, Shared, Snapshot, Workspace, REFRESH_SECS, THEME_SECS, WORKSPACE_SECS};
use crate::attention;
use crate::model::{project_for_label, Project};
use crate::mrstate::{self, MrState};
use crate::sources::threads::{self, Thread};
use crate::sources::{list, project_md, runner, workspace};
use crate::theme;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Items in `inbox/` other than `done`.
pub fn inbox_count(dir: &Path) -> u32 {
    std::fs::read_dir(dir.join("inbox"))
        .map(|rd| rd.flatten().filter(|e| e.file_name() != "done").count() as u32)
        .unwrap_or(0)
}

/// The highest thread number a project holds: its recency signal.
pub fn newest_thread(dir: &Path) -> u32 {
    std::fs::read_dir(dir.join("threads"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let digits: String = name
                        .strip_prefix("t-")?
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    digits.parse::<u32>().ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// Fills a project from its folder: PROJECT.md, inbox, threads.
pub fn enrich(p: &mut Project, dir: &Path) {
    if let Ok(text) = std::fs::read_to_string(dir.join("PROJECT.md")) {
        let md = project_md::parse(&text);
        if let Some(n) = md.name {
            p.name = n;
        }
        p.goal = md.goal;
        p.repos = md.repos;
    }
    p.inbox = inbox_count(dir);
    p.activity = newest_thread(dir);
}

pub async fn read_projects(app: &Shared) -> Result<Vec<Project>, String> {
    let argv = runner::argv([
        app.config.herdr_projects.as_str(),
        "--root",
        &app.config.root.display().to_string(),
        "list",
        "--all",
    ]);
    let out = runner::run(&argv, None, Duration::from_secs(20)).await?;
    if !out.ok() {
        return Err(out.error());
    }
    let mut projects = list::parse_list(&out.stdout);
    let root = app.config.root.clone();
    // Folder reads are quick but synchronous: off the runtime's workers.
    projects = tokio::task::spawn_blocking(move || {
        for p in &mut projects {
            enrich(p, &root.join(&p.slug));
        }
        projects
    })
    .await
    .map_err(|e| e.to_string())?;
    rebuild_attention(app, &mut projects).await;
    Ok(projects)
}

/// herdr-projects' «Waiting on you» and «Ready for review» counts are not signals
/// by themselves: this rebuilds `needs`, `review` and the waits of every project
/// (see `attention`, `coordask`, `mrstate`).
async fn rebuild_attention(app: &Shared, projects: &mut [Project]) {
    let root = app.config.root.clone();
    let (records, alive) = tokio::task::spawn_blocking(move || {
        (
            attention::read_progress(&root),
            attention::ticker_alive(&root),
        )
    })
    .await
    .unwrap_or_default();
    let stale_ms = app.state.lock().unwrap().settings.review_stale_ms();
    for p in projects.iter_mut() {
        if p.status == "archived" {
            continue;
        }
        let dir = app.config.root.join(&p.slug);
        // A dead ticker leaves the coordinators' state as it was: not a signal.
        let candidate = if alive {
            attention::coordinator_candidate(&dir, &records)
        } else {
            None
        };
        let coordinator = match candidate {
            Some(c) => coordinator_state(app, &p.slug, &c),
            None => attention::Coordinator::No,
        };
        let listed = if p.counts.needs > 0 || p.counts.review > 0 {
            thread_list(app, &p.slug).await
        } else {
            Some(Vec::new())
        };
        let mr = match &listed {
            Some(list) => mr_states(app, &p.slug, list).await,
            None => HashMap::new(),
        };
        let env = attention::Env {
            now: now_ms(),
            stale_ms,
            mr: &mr,
        };
        attention::apply(p, coordinator, listed.as_deref(), &env);
    }
}

/// Whether the coordinator that reported `Waiting for you` really asks something.
fn coordinator_state(app: &Shared, slug: &str, c: &attention::Candidate) -> attention::Coordinator {
    use crate::coordask::{self, Verdict};
    let unclear =
        |why: &str| attention::Coordinator::Unclear(format!("координатор: статус ждёт, {why}"));
    if c.agent != "claude" || c.session.is_empty() {
        return unclear("вопрос не распознан");
    }
    let Some(claude) = coordask::claude_dir() else {
        return unclear("вопрос не распознан");
    };
    let path = coordask::transcript_path(&claude, &c.cwd, &c.session);
    match coordask::verdict(app, slug, &path) {
        Some(Verdict::No) => attention::Coordinator::No,
        Some(Verdict::Asks(what)) => attention::Coordinator::Asks(crate::model::Wait {
            kind: "coordinator".into(),
            thread: String::new(),
            text: what,
            since: c.since,
        }),
        Some(Verdict::Unclear(_)) => unclear("вопрос не распознан"),
        None => unclear("проверяю, есть ли вопрос"),
    }
}

/// The state of the merge request each finished thread's report names (GitLab),
/// for the threads herdr-projects shows in review or waiting. See `mrstate`.
pub async fn mr_states(app: &Shared, slug: &str, list: &[Thread]) -> HashMap<String, MrState> {
    let dir = app.config.root.join(slug).join("threads");
    let ids: Vec<String> = list
        .iter()
        .filter(|t| {
            !t.closed()
                && matches!(
                    t.group.as_str(),
                    "Ready for review" | "Waiting on you" | "Needs you"
                )
        })
        .filter(|t| t.pr_number().is_none())
        .map(|t| t.id.clone())
        .collect();
    let refs = tokio::task::spawn_blocking(move || {
        ids.into_iter()
            .filter_map(|id| {
                let text = std::fs::read_to_string(dir.join(format!("{id}.md"))).ok()?;
                Some((id, mrstate::find_mr(&text)?))
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    refs.into_iter()
        .map(|(id, mr)| (id, mrstate::state(app, &mr)))
        .collect()
}

async fn thread_list(app: &Shared, slug: &str) -> Option<Vec<Thread>> {
    let argv = runner::argv([
        app.config.herdr_projects.as_str(),
        "--root",
        &app.config.root.display().to_string(),
        "thread",
        "list",
        slug,
        "--json",
    ]);
    let out = runner::run(&argv, None, Duration::from_secs(20))
        .await
        .ok()?;
    if !out.ok() {
        return None;
    }
    let groups = threads::parse_threads(&out.stdout).ok()?;
    Some(groups.into_iter().flat_map(|g| g.threads).collect())
}

pub async fn refresh_once(app: &Shared) {
    {
        let mut s = app.snapshot.write().unwrap();
        s.loading = true;
    }
    let result = read_projects(app).await;
    {
        let mut s = app.snapshot.write().unwrap();
        s.loading = false;
        match result {
            Ok(projects) => {
                *s = Snapshot {
                    projects,
                    updated: now_ms(),
                    loading: false,
                    error: None,
                };
            }
            Err(e) => s.error = Some(e),
        }
    }
    sync_workspace(app);
    app.emit_snapshot();
    // Projects that appeared since the last read get their group guess.
    crate::suggest::tick(app);
}

pub async fn refresh_loop(app: Shared) {
    loop {
        refresh_once(&app).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(REFRESH_SECS),
            app.refresh_now.notified(),
        )
        .await;
    }
}

/// Re-matches the known label against the (new) project list.
fn sync_workspace(app: &Shared) {
    let projects = app.snapshot.read().unwrap().projects.clone();
    let mut ws = app.workspace.write().unwrap();
    let slug = ws
        .label
        .as_deref()
        .and_then(|l| project_for_label(&projects, l));
    ws.slug = slug;
}

pub async fn workspace_loop(app: Shared) {
    let argv = runner::argv([app.config.herdr.as_str(), "workspace", "list"]);
    loop {
        if let Ok(out) = runner::run(&argv, None, Duration::from_secs(5)).await {
            if out.ok() {
                let label = workspace::focused_label(&out.stdout);
                let changed = {
                    let projects = app.snapshot.read().unwrap().projects.clone();
                    let mut ws = app.workspace.write().unwrap();
                    let same = ws.label.as_deref().map(str::to_lowercase)
                        == label.as_deref().map(str::to_lowercase);
                    if same {
                        false
                    } else {
                        let slug = label
                            .as_deref()
                            .and_then(|l| project_for_label(&projects, l));
                        *ws = Workspace {
                            label,
                            slug,
                            seq: ws.seq + 1,
                        };
                        true
                    }
                };
                if changed {
                    let ws = app.workspace.read().unwrap().clone();
                    app.emit("workspace", &ws);
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(WORKSPACE_SECS)).await;
    }
}

pub async fn theme_loop(app: Shared) {
    let files = theme::watched_files();
    let stamp = || {
        files
            .iter()
            .map(|f| crate::config::file_mtime(f))
            .collect::<Vec<_>>()
    };
    let mut last = stamp();
    loop {
        tokio::time::sleep(Duration::from_secs(THEME_SECS)).await;
        let now = stamp();
        if now != last {
            last = now;
            let report = tokio::task::spawn_blocking(theme::load)
                .await
                .unwrap_or_else(|_| theme::load());
            let changed = {
                let mut t = app.theme.write().unwrap();
                let changed = *t != report;
                *t = report;
                changed
            };
            if changed {
                let t = app.theme.read().unwrap().clone();
                app.emit("theme", &t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_project_folder() {
        let dir = std::env::temp_dir().join(format!("herdr-planner-proj-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("inbox/done")).unwrap();
        std::fs::create_dir_all(dir.join("threads")).unwrap();
        std::fs::write(dir.join("inbox/one.md"), "").unwrap();
        std::fs::write(dir.join("threads/t-0003.md"), "").unwrap();
        std::fs::write(dir.join("threads/t-0012.md"), "").unwrap();
        std::fs::write(dir.join("threads/notes"), "").unwrap();
        std::fs::write(
            dir.join("PROJECT.md"),
            "+++\nname = \"Имя\"\ngoal = \"Цель\"\n[[repos]]\npath = \"/r\"\n+++\n",
        )
        .unwrap();
        let mut p = Project {
            slug: "x".into(),
            name: "x".into(),
            ..Default::default()
        };
        enrich(&mut p, &dir);
        assert_eq!(p.name, "Имя");
        assert_eq!(p.goal.as_deref(), Some("Цель"));
        assert_eq!(p.inbox, 1);
        assert_eq!(p.activity, 12);
        assert_eq!(p.repos.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
