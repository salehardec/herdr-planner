//! Backlog freshness: a new commit in the project's main (or a new task)
//! makes the cheap agent compare the open tasks with the code once. Each
//! verdict is cached by (task text, main commit) in the saved state; «Оставить»
//! hides a badge until main moves. Nothing is ever removed here and TASKS.md is
//! never touched: «Убрать» only asks the coordinator, its only writer.
//!
//! Ported from the Tern plugin (0.17.1) with two differences: the agent gets
//! the notes under each task, and a verdict «сделано» stands only when it names
//! a file of the repository that does the task (`proof`, checked on disk).

use crate::agent::{self, Kind};
use crate::app::{now_ms, Shared};
use crate::cards;
use crate::jsonscan;
use crate::model::Repo;
use crate::sources::runner;
use crate::sources::tasks::{self, Task};
use crate::store::{FreshCache, Verdict};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Duration;

/// A new commit waits this long after its last change before the whole
/// backlog is checked again (new or rewritten tasks are checked at once).
pub const DEBOUNCE_MS: u64 = 60_000;
/// After a failed check the next one waits this long.
pub const RETRY_MS: u64 = 300_000;

pub const WORD_PARTIAL: &str = "◐ частично";
pub const WORD_DONE: &str = "⊘ сделано";

pub fn verdict_word(v: &str) -> &'static str {
    if v == "done" {
        WORD_DONE
    } else {
        WORD_PARTIAL
    }
}

/// What the page needs per project while it is open: the check in progress,
/// the commit of main seen, the last error.
#[derive(Debug, Default)]
pub struct FreshRun {
    pub busy: bool,
    /// The commit of main is being asked for (or the profiles are).
    pub probing: bool,
    /// Tasks the running check is about, by text.
    pub pending: HashSet<String>,
    pub head_seen: Option<String>,
    /// When `head_seen` last changed (ms since the epoch, 0 = at the first look).
    pub head_changed_at: u64,
    pub error: Option<String>,
    pub retry_after: Option<u64>,
}

// ---------------------------------------------------------------------------
// Who a TASKS.md line belongs to

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A thread already runs it.
    Thread,
    /// The user's or a person's.
    Human,
    /// Nobody yet, or an agent profile: the check is for these.
    Agent,
}

/// The owner is the last `(...)` of the line (before an optional ` · t-0007`;
/// `herdr-projects` reads it the same way), else the one in front: `(me)`,
/// `(profile)`, `(@machine)`, `(profile@machine)` or a person's name. A name
/// that is no profile is a person.
pub fn task_class(task: &Task, is_profile: impl Fn(&str) -> bool) -> Class {
    if task.thread_led {
        return Class::Thread;
    }
    let full = task.full.trim_end();
    let trailing = full
        .strip_suffix(')')
        .and_then(|inner| inner.rsplit_once('('))
        .map(|(_, o)| o)
        .filter(|o| !o.contains('(') && !o.contains(')'));
    let owner = trailing
        .map(str::to_string)
        .or_else(|| task.owner.clone())
        .unwrap_or_default();
    let owner = owner.trim();
    if owner == "me" || full.contains("(me,") || full.contains("(me;") {
        return Class::Human;
    }
    if owner.is_empty() || owner == "agent" {
        return Class::Agent;
    }
    let word = |s: &str| {
        s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    if let Some((profile, machine)) = owner.split_once('@') {
        return if word(profile) && !machine.is_empty() && word(machine) {
            Class::Agent
        } else {
            Class::Human
        };
    }
    if owner.chars().next().is_some_and(char::is_alphanumeric) && word(owner) && is_profile(owner) {
        return Class::Agent;
    }
    Class::Human
}

// ---------------------------------------------------------------------------
// The agent's answer

/// `actual`, `partial` or `done` from whatever the agent wrote for it.
pub fn verdict_of(v: &Value) -> Option<&'static str> {
    let s = v.as_str()?.trim().to_lowercase().replace([' ', '-'], "_");
    match s.as_str() {
        "actual" | "relevant" | "open" | "todo" | "актуально" => Some("actual"),
        "partial" | "partially" | "частично" => Some("partial"),
        "done" | "completed" | "finished" | "already_done" | "сделано" => Some("done"),
        _ if s.starts_with("part") || s.starts_with("част") => Some("partial"),
        _ if s.starts_with("done") || s.starts_with("сдел") || s.starts_with("уже") => {
            Some("done")
        }
        _ if s.starts_with("actual") || s.starts_with("актуал") => Some("actual"),
        _ => None,
    }
}

fn number_of(v: Option<&Value>) -> Option<usize> {
    let v = v?;
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (n >= 1.0 && n.fract() == 0.0).then_some(n as usize)
}

fn text_of(row: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| row.get(*k))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

/// The file a `proof` names, if it is a file inside `repo`: the first word that
/// looks like a path (`src/a.rs`, `src/a.rs:42`, `` `src/a.rs` ``), never one
/// that climbs out of the repository.
pub fn proof_file(proof: &str, repo: &Path) -> Option<String> {
    proof
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | ',' | ';' | '(' | ')')))
        .find_map(|w| {
            let w = w.strip_prefix("./").unwrap_or(w);
            // `path:12` and `path:12-20`.
            let path = match w.rsplit_once(':') {
                Some((p, n)) if n.chars().all(|c| c.is_ascii_digit() || c == '-') => p,
                _ => w,
            };
            let ok = !path.is_empty()
                && !path.starts_with('/')
                && !path.split('/').any(|part| part == "..")
                && repo.join(path).is_file();
            ok.then(|| path.to_string())
        })
}

/// What was read from an answer: task number → (verdict, reason, proof file).
pub type Rows = BTreeMap<usize, (String, String, Option<String>)>;

/// The verdicts one decoded JSON value holds: an array of rows, or an object
/// wrapping one. `done` stands only with a proof file; otherwise the task is
/// treated as still actual.
fn verdict_rows(data: &Value, count: usize, repo: &Path) -> Rows {
    let rows: Option<&Vec<Value>> = match data {
        Value::Array(a) => Some(a),
        Value::Object(m) => m.values().find_map(|x| {
            x.as_array()
                .filter(|a| a.first().is_some_and(Value::is_object))
        }),
        _ => None,
    };
    let mut out = BTreeMap::new();
    for row in rows.into_iter().flatten() {
        if !row.is_object() {
            continue;
        }
        let n = number_of(
            row.get("n")
                .or_else(|| row.get("id"))
                .or_else(|| row.get("index"))
                .or_else(|| row.get("task")),
        );
        let v = row
            .get("v")
            .or_else(|| row.get("verdict"))
            .or_else(|| row.get("status"))
            .and_then(verdict_of);
        let (Some(n), Some(v)) = (n, v) else { continue };
        if n > count || out.contains_key(&n) {
            continue;
        }
        let why = tasks::clip(
            text_of(row, &["why", "reason", "r"])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .trim(),
            240,
        );
        let proof_text = text_of(row, &["proof", "evidence", "file", "where"]);
        let proof = proof_file(&proof_text, repo);
        let (v, proof) = match (v, proof) {
            ("done", None) => ("actual", None),
            (v, proof) => (v, proof),
        };
        out.insert(n, (v.to_string(), why, proof));
    }
    out
}

/// Verdicts per task number from the agent's text. Anything else it said is
/// ignored; an error text when no verdict could be read.
pub fn parse_verdicts(text: &str, count: usize, repo: &Path) -> Result<Rows, String> {
    let found = jsonscan::candidates(text);
    if found.is_empty() {
        return Err("агент ответил не JSON".into());
    }
    for data in &found {
        let rows = verdict_rows(data, count, repo);
        if !rows.is_empty() {
            return Ok(rows);
        }
    }
    Err("в ответе агента нет вердиктов".into())
}

pub const SYSTEM: &str = "Ты проверяешь актуальность задач бэклога проекта по коду его репозитория. \
Ты НЕ координатор и ничего не меняешь: только читай файлы (Read, Grep, Glob). \
К каждой задаче могут быть заметки из TASKS.md — это уточнения пользователя, учитывай их. \
Ответь СТРОГО одним JSON-массивом, без текста до и после, без ``` и без пояснений: \
[{\"n\":1,\"v\":\"actual\",\"why\":\"…\",\"proof\":\"путь/к/файлу\"}, …] — по одному объекту на каждую задачу списка. \
n — номер задачи; v — одно из: actual (задача нужна, не сделана), partial (часть уже сделана в коде), \
done (уже полностью сделано); why — причина по-русски, одна фраза до 120 символов. \
«done» ТОЛЬКО с доказательством в коде: в proof — путь файла репозитория (от его корня), где задача реально сделана, \
и ты этот файл прочитал; без такого файла «done» не принимается. Для actual и partial proof не нужен. \
Сомневаешься — actual.";

/// The request: repository, commit of main, what changed, the tasks with their notes.
pub fn prompt(repo: &str, head: &str, log: &str, missing: &[Task]) -> String {
    let mut lines = vec![
        format!("Репозиторий проекта: {repo}"),
        format!("Коммит main: {}", head.chars().take(10).collect::<String>()),
        "Последние коммиты main (git log):".to_string(),
        if log.is_empty() {
            "(пусто)".into()
        } else {
            log.to_string()
        },
        String::new(),
        "Задачи бэклога:".to_string(),
    ];
    for (i, t) in missing.iter().enumerate() {
        lines.push(format!("{}. {}", i + 1, t.full));
        for note in t.notes.lines() {
            lines.push(format!("   заметка: {note}"));
        }
    }
    lines.join("\n")
}

/// `$1` repo; prints the commit of main (else master), exits 1 when there is none.
const HEAD_SCRIPT: &str = r#"for c in main master; do
	h=$(git -C "$1" --no-optional-locks rev-parse --verify -q "$c^{commit}" 2>/dev/null) && { echo "$h"; exit 0; }
done
exit 1"#;

/// `$1` repo, `$2` the commit checked last time (maybe empty), `$3` main now: what changed since.
const LOG_SCRIPT: &str = r#"P="$1"
g() { git -C "$P" --no-optional-locks "$@"; }
if [ -n "$2" ] && g merge-base --is-ancestor "$2" "$3" 2>/dev/null; then
	echo "С прошлой проверки ($2):"
	g log -n 60 --format='%h %ad %s' --date=short "$2..$3"
else
	echo "Последние коммиты:"
	g log -n 40 --format='%h %ad %s' --date=short "$3"
fi"#;

// ---------------------------------------------------------------------------
// Deciding

/// The tasks that have no verdict for `head`, and how many of them have none
/// at all (a new or rewritten task).
pub fn stale<'a>(cache: &FreshCache, head: &str, eligible: &'a [Task]) -> (Vec<&'a Task>, usize) {
    let mut missing = Vec::new();
    let mut brand_new = 0;
    for t in eligible {
        match cache.verdicts.get(&t.full) {
            Some(e) if e.head == head => {}
            Some(_) => missing.push(t),
            None => {
                missing.push(t);
                brand_new += 1;
            }
        }
    }
    (missing, brand_new)
}

/// New or rewritten tasks are checked at once; tasks that only went stale
/// because main moved wait for the debounce after its last change.
pub fn due(brand_new: usize, head_changed_at: u64, now: u64, debounce_ms: u64) -> bool {
    brand_new > 0 || now.saturating_sub(head_changed_at) >= debounce_ms
}

/// The saved verdicts of tasks that are not open in TASKS.md any more go.
pub fn prune(cache: &mut FreshCache, tasks: &[Task]) -> bool {
    if tasks.is_empty() {
        return false;
    }
    let open: HashSet<&str> = tasks
        .iter()
        .filter(|t| !t.done)
        .map(|t| t.full.as_str())
        .collect();
    let before = (cache.verdicts.len(), cache.kept.len());
    cache.verdicts.retain(|k, _| open.contains(k.as_str()));
    cache.kept.retain(|k, _| open.contains(k.as_str()));
    before != (cache.verdicts.len(), cache.kept.len())
}

// ---------------------------------------------------------------------------
// What the card shows

/// The verdict worth a badge for the task `full` (none for actual, kept or
/// stale ones) and whether the task is being checked right now.
pub fn view(app: &Shared, slug: &str, full: &str) -> (Option<Verdict>, bool) {
    let (head, pending) = {
        let runs = app.fresh_run.lock().unwrap();
        match runs.get(slug) {
            Some(r) => (r.head_seen.clone(), r.pending.contains(full)),
            None => (None, false),
        }
    };
    if pending {
        return (None, true);
    }
    let Some(head) = head else {
        return (None, false);
    };
    let st = app.state.lock().unwrap();
    let Some(cache) = st.fresh.get(slug) else {
        return (None, false);
    };
    match cache.verdicts.get(full) {
        Some(e)
            if e.head == head
                && e.v != "actual"
                && cache.kept.get(full).map(String::as_str) != Some(head.as_str()) =>
        {
            (Some(e.clone()), false)
        }
        _ => (None, false),
    }
}

/// The card's part: `{verdicts, checking, error, on}`.
pub fn card_part(app: &Shared, slug: &str, tasks: &[Task]) -> Value {
    let mut verdicts = serde_json::Map::new();
    let mut checking = Vec::new();
    for t in tasks.iter().filter(|t| !t.done && !t.thread_led) {
        let (v, busy) = view(app, slug, &t.full);
        if let Some(v) = v {
            verdicts.insert(
                t.full.clone(),
                json!({"v": v.v, "why": v.why, "proof": v.proof, "word": verdict_word(&v.v)}),
            );
        }
        if busy {
            checking.push(t.full.clone());
        }
    }
    let on = app.state.lock().unwrap().settings.autocheck == "on";
    let error = app
        .fresh_run
        .lock()
        .unwrap()
        .get(slug)
        .and_then(|r| r.error.clone());
    json!({"verdicts": verdicts, "checking": checking, "error": on.then_some(error).flatten(), "on": on})
}

/// «Оставить»: the badge stays hidden until main gets a new commit.
pub fn keep(app: &Shared, slug: &str, full: &str) -> bool {
    let Some(head) = app
        .fresh_run
        .lock()
        .unwrap()
        .get(slug)
        .and_then(|r| r.head_seen.clone())
    else {
        return false;
    };
    app.state
        .lock()
        .unwrap()
        .fresh
        .entry(slug.to_string())
        .or_default()
        .kept
        .insert(full.to_string(), head);
    app.save_state();
    true
}

// ---------------------------------------------------------------------------
// Running

/// The repository the check works in: the first one of the project that is on
/// this machine and exists.
pub fn check_repo(repos: &[Repo]) -> Option<String> {
    repos
        .iter()
        .find(|r| r.machine.is_empty() && Path::new(&r.path).is_dir())
        .map(|r| r.path.clone())
}

fn emit(app: &Shared, slug: &str) {
    app.emit("card", &json!({ "slug": slug }));
}

fn set_probing(app: &Shared, slug: &str, on: bool) {
    app.fresh_run
        .lock()
        .unwrap()
        .entry(slug.to_string())
        .or_default()
        .probing = on;
}

/// Called after every read of the open project's card: checks the backlog
/// when it is due. Never blocks; the work runs in the background and the card
/// is told by a `card` event.
pub fn tick(app: &Shared, slug: &str, repos: &[Repo], task_list: &[Task]) {
    if app.state.lock().unwrap().settings.autocheck != "on" {
        return;
    }
    let Some(repo) = check_repo(repos) else {
        return;
    };
    {
        let mut runs = app.fresh_run.lock().unwrap();
        let run = runs.entry(slug.to_string()).or_default();
        let waiting = run.retry_after.is_some_and(|t| now_ms() < t);
        if run.busy || run.probing || waiting {
            return;
        }
        run.probing = true;
    }
    // The saved verdicts of tasks that are gone.
    {
        let changed = {
            let mut st = app.state.lock().unwrap();
            st.fresh.get_mut(slug).is_some_and(|c| prune(c, task_list))
        };
        if changed {
            app.save_state();
        }
    }
    let (app, slug, task_list) = (app.clone(), slug.to_string(), task_list.to_vec());
    tokio::spawn(async move {
        let plan = plan(&app, &slug, &repo, &task_list).await;
        set_probing(&app, &slug, false);
        if let Some((head, missing)) = plan {
            check(&app, &slug, &repo, &head, missing).await;
        }
    });
}

/// The commit of main and the tasks to ask about, when a check is due.
async fn plan(
    app: &Shared,
    slug: &str,
    repo: &str,
    task_list: &[Task],
) -> Option<(String, Vec<Task>)> {
    // Profiles: a task owned by a name that is no profile is a person's.
    let locals: Vec<String> = match cards::assignable(app, slug).await {
        Ok(a) => a
            .profiles
            .into_iter()
            .filter(|p| !p.contains('@'))
            .collect(),
        Err(_) => Vec::new(),
    };
    let eligible: Vec<Task> = task_list
        .iter()
        .filter(|t| !t.done && task_class(t, |n| locals.iter().any(|l| l == n)) == Class::Agent)
        .cloned()
        .collect();
    if eligible.is_empty() {
        return None;
    }
    let argv = runner::argv(["sh", "-c", HEAD_SCRIPT, "sh", repo]);
    let head = match runner::run(&argv, None, Duration::from_secs(15)).await {
        Ok(out) if out.ok() && is_hex(out.stdout.trim()) => out.stdout.trim().to_string(),
        _ => {
            let mut runs = app.fresh_run.lock().unwrap();
            runs.entry(slug.to_string()).or_default().error =
                Some("в репозитории нет ветки main".into());
            drop(runs);
            emit(app, slug);
            return None;
        }
    };
    let cache = app
        .state
        .lock()
        .unwrap()
        .fresh
        .get(slug)
        .cloned()
        .unwrap_or_default();
    let (stale, brand_new) = stale(&cache, &head, &eligible);
    let missing: Vec<Task> = stale.into_iter().cloned().collect();
    let now = now_ms();
    let head_changed = {
        let mut runs = app.fresh_run.lock().unwrap();
        let run = runs.entry(slug.to_string()).or_default();
        run.error = None;
        if run.head_seen.as_deref() == Some(head.as_str()) {
            false
        } else {
            // The first look since the start counts as an old change.
            run.head_changed_at = if run.head_seen.is_none() { 0 } else { now };
            run.head_seen = Some(head.clone());
            true
        }
    };
    if head_changed {
        // A commit hides the badges of the old one: the card learns of it.
        emit(app, slug);
    }
    let mut runs = app.fresh_run.lock().unwrap();
    let run = runs.entry(slug.to_string()).or_default();
    if missing.is_empty() || run.busy {
        return None;
    }
    let debounce = app.config.fresh_debounce.as_millis() as u64;
    if !due(brand_new, run.head_changed_at, now, debounce) {
        return None;
    }
    run.busy = true;
    run.pending = missing.iter().map(|t| t.full.clone()).collect();
    Some((head, missing))
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// One agent call for `missing`, then the cache.
async fn check(app: &Shared, slug: &str, repo: &str, head: &str, missing: Vec<Task>) {
    emit(app, slug);
    let before = app
        .state
        .lock()
        .unwrap()
        .fresh
        .get(slug)
        .and_then(|c| c.head.clone())
        .unwrap_or_default();
    let argv = runner::argv(["sh", "-c", LOG_SCRIPT, "sh", repo, before.as_str(), head]);
    let log = match runner::run(&argv, None, Duration::from_secs(20)).await {
        Ok(out) if out.ok() => tasks::clip(out.stdout.trim(), 6000),
        _ => String::new(),
    };
    let dir = app.config.project_dir(slug).display().to_string();
    let kind = Kind::from_setting(&app.state.lock().unwrap().settings.agent);
    let mut text = prompt(repo, head, &log, &missing);
    if kind == Kind::Deepseek {
        text.push_str(&format!(
            "\nПапка проекта herdr (PROJECT.md, TASKS.md): {dir}"
        ));
    }
    let req = agent::Request {
        system: SYSTEM.into(),
        prompt: text,
        cwd: repo.to_string(),
        dirs: vec![dir],
    };
    let result = match agent::run(kind, &req).await {
        Ok(text) => parse_verdicts(&text, missing.len(), Path::new(repo)),
        Err(e) => Err(e),
    };
    let ok = result.is_ok();
    match result {
        Ok(rows) => {
            {
                let mut st = app.state.lock().unwrap();
                let cache = st.fresh.entry(slug.to_string()).or_default();
                // A task the agent skipped counts as up to date, so it is not
                // asked again until main moves.
                for (i, t) in missing.iter().enumerate() {
                    let (v, why, proof) = rows
                        .get(&(i + 1))
                        .cloned()
                        .unwrap_or_else(|| ("actual".into(), String::new(), None));
                    cache.verdicts.insert(
                        t.full.clone(),
                        Verdict {
                            head: head.to_string(),
                            v,
                            why,
                            proof,
                        },
                    );
                }
                cache.head = Some(head.to_string());
            }
            app.save_state();
        }
        Err(e) => {
            let mut runs = app.fresh_run.lock().unwrap();
            let run = runs.entry(slug.to_string()).or_default();
            run.error = Some(e);
            run.retry_after = Some(now_ms() + RETRY_MS);
        }
    }
    {
        let mut runs = app.fresh_run.lock().unwrap();
        let run = runs.entry(slug.to_string()).or_default();
        run.busy = false;
        run.pending.clear();
        if ok {
            run.error = None;
            run.retry_after = None;
        }
    }
    emit(app, slug);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(full: &str) -> Task {
        tasks::parse_tasks(&format!("- [ ] {full}\n")).remove(0)
    }

    fn class(full: &str, profiles: &[&str]) -> Class {
        task_class(&task(full), |n| profiles.contains(&n))
    }

    #[test]
    fn classes_of_tasks() {
        let profiles = ["claude-sonnet", "claude-opus"];
        assert_eq!(class("Починить экспорт", &profiles), Class::Agent);
        assert_eq!(
            class("Починить экспорт (claude-sonnet)", &profiles),
            Class::Agent
        );
        assert_eq!(class("Починить экспорт (agent)", &profiles), Class::Agent);
        assert_eq!(class("Выпустить ключ (me)", &profiles), Class::Human);
        assert_eq!(class("(me) Выпустить ключ", &profiles), Class::Human);
        assert_eq!(class("Позвонить (Priya Rao)", &profiles), Class::Human);
        assert_eq!(
            class("Позвонить (Bob)", &profiles),
            Class::Human,
            "a name that is no profile"
        );
        assert_eq!(
            class("Задача (claude-opus) · t-0005", &profiles),
            Class::Thread
        );
        assert_eq!(class("Задача (pi@mac)", &profiles), Class::Agent);
        assert_eq!(class("Задача (@mac)", &profiles), Class::Agent);
        assert_eq!(class("Задача (Bob Lee@mac)", &profiles), Class::Human);
        assert_eq!(
            class("Задача (me, потом claude-sonnet)", &profiles),
            Class::Human
        );
        assert_eq!(class("Сделать (x → t-0042)", &profiles), Class::Thread);
        assert_eq!(
            class("Задача (claude-sonnet)", &[]),
            Class::Human,
            "profiles unknown: only the ownerless"
        );
    }

    #[test]
    fn verdict_words_in_any_language() {
        for (s, want) in [
            ("actual", "actual"),
            ("Актуально", "actual"),
            ("partially", "partial"),
            ("Частично", "partial"),
            ("already-done", "done"),
            ("сделано", "done"),
            ("уже есть", "done"),
        ] {
            assert_eq!(verdict_of(&json!(s)), Some(want), "{s}");
        }
        assert_eq!(verdict_of(&json!("maybe")), None);
        assert_eq!(verdict_of(&json!(3)), None);
    }

    fn repo_with(files: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hp-fresh-{}-{}",
            std::process::id(),
            files.join("_").replace('/', "-")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for f in files {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        }
        dir
    }

    #[test]
    fn verdicts_parse_around_prose_and_need_proof_for_done() {
        let repo = repo_with(&["src/theme.rs", "README.md"]);
        let text = "Проверил.\n```json\n[{\"n\":1,\"v\":\"done\",\"why\":\"тема уже есть\",\"proof\":\"src/theme.rs:12\"},\
{\"n\":\"2\",\"v\":\"Сделано\",\"why\":\"кажется\"},{\"n\":3,\"v\":\"done\",\"proof\":\"../etc/passwd\"},\
{\"n\":4,\"v\":\"partial\",\"reason\":\"часть\"},{\"n\":9,\"v\":\"done\",\"proof\":\"README.md\"},{\"n\":1,\"v\":\"partial\"},{\"v\":\"done\"}]\n```";
        let rows = parse_verdicts(text, 4, &repo).unwrap();
        assert_eq!(
            rows[&1],
            (
                "done".into(),
                "тема уже есть".into(),
                Some("src/theme.rs".into())
            )
        );
        assert_eq!(
            rows[&2].0, "actual",
            "«сделано» without a file is not believed"
        );
        assert_eq!(
            rows[&3].0, "actual",
            "a path out of the repository is no proof"
        );
        assert_eq!(rows[&4], ("partial".into(), "часть".into(), None));
        assert!(!rows.contains_key(&9), "a task number out of range");
        assert_eq!(rows.len(), 4, "the first row of a number wins");
        // An object wrapping the rows is read too.
        let wrapped = parse_verdicts(
            r#"{"verdicts":[{"n":1,"v":"partial","why":"w"}]}"#,
            1,
            &repo,
        )
        .unwrap();
        assert_eq!(wrapped[&1].0, "partial");
        assert_eq!(
            parse_verdicts("нет json", 1, &repo).unwrap_err(),
            "агент ответил не JSON"
        );
        assert_eq!(
            parse_verdicts("[]", 1, &repo).unwrap_err(),
            "в ответе агента нет вердиктов"
        );
        assert_eq!(
            parse_verdicts("[{\"n\":1,\"v\":\"???\"}]", 1, &repo).unwrap_err(),
            "в ответе агента нет вердиктов"
        );
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn proof_names_a_file_of_the_repository() {
        let repo = repo_with(&["src/a.rs", "docs/x y.md"]);
        assert_eq!(proof_file("`src/a.rs`", &repo).as_deref(), Some("src/a.rs"));
        assert_eq!(
            proof_file("см. ./src/a.rs:3-9 (функция)", &repo).as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(proof_file("src/nope.rs", &repo), None);
        assert_eq!(proof_file("/etc/passwd", &repo), None);
        assert_eq!(proof_file("src/../src/a.rs", &repo), None);
        assert_eq!(proof_file("", &repo), None);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn the_prompt_carries_the_notes() {
        let t = tasks::parse_tasks("- [ ] Первая\n  Заметка один.\n  Заметка два.\n- [ ] Вторая\n");
        let p = prompt("/r", "0123456789abcdef", "abc123 2026-10-14 коммит", &t);
        assert!(p.contains("Репозиторий проекта: /r\nКоммит main: 0123456789\n"));
        assert!(p.contains("abc123 2026-10-14 коммит"));
        assert!(
            p.contains("1. Первая\n   заметка: Заметка один.\n   заметка: Заметка два.\n2. Вторая")
        );
        assert!(prompt("/r", "ab", "", &t).contains("(пусто)"));
    }

    fn verdict(head: &str, v: &str) -> Verdict {
        Verdict {
            head: head.into(),
            v: v.into(),
            why: String::new(),
            proof: None,
        }
    }

    #[test]
    fn what_is_missing_and_when_it_is_due() {
        let t = tasks::parse_tasks("- [ ] a\n- [ ] b\n- [ ] c\n");
        let mut cache = FreshCache::default();
        cache.verdicts.insert("a".into(), verdict("h1", "partial"));
        cache.verdicts.insert("b".into(), verdict("h0", "actual"));
        let (m, fresh) = stale(&cache, "h1", &t);
        assert_eq!(
            m.iter().map(|t| t.full.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert_eq!(fresh, 1, "only c has no verdict at all");
        // New tasks are checked at once, a moved main waits for the debounce.
        assert!(due(1, 1_000, 1_001, DEBOUNCE_MS));
        assert!(!due(0, 1_000, 1_000 + DEBOUNCE_MS - 1, DEBOUNCE_MS));
        assert!(due(0, 1_000, 1_000 + DEBOUNCE_MS, DEBOUNCE_MS));
        assert!(
            due(0, 0, 1_700_000_000_000, DEBOUNCE_MS),
            "the first look counts as an old change"
        );
        assert!(stale(&cache, "h1", &t[..1]).0.is_empty());
    }

    #[test]
    fn verdicts_of_vanished_tasks_are_pruned_only_after_a_real_read() {
        let mut cache = FreshCache::default();
        cache.verdicts.insert("a".into(), verdict("h", "done"));
        cache.verdicts.insert("gone".into(), verdict("h", "done"));
        cache.kept.insert("gone".into(), "h".into());
        assert!(!prune(&mut cache, &[]), "an empty TASKS.md removes nothing");
        assert_eq!(cache.verdicts.len(), 2);
        let t = tasks::parse_tasks("- [ ] a\n- [x] gone\n");
        assert!(prune(&mut cache, &t), "a done task is no open task either");
        assert!(
            cache.verdicts.contains_key("a") && cache.verdicts.len() == 1 && cache.kept.is_empty()
        );
    }

    #[test]
    fn the_check_repo_is_the_first_local_one() {
        let here = std::env::temp_dir();
        let repos = vec![
            Repo {
                path: "/srv/mac".into(),
                machine: "mac".into(),
            },
            Repo {
                path: "/нет/такой/папки".into(),
                machine: String::new(),
            },
            Repo {
                path: here.display().to_string(),
                machine: String::new(),
            },
        ];
        assert_eq!(check_repo(&repos), Some(here.display().to_string()));
        assert_eq!(check_repo(&repos[..2]), None);
    }
}
