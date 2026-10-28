//! The board's own records of a task, read-only.
//!
//! herdr-projects keeps everything it knows about a task in
//! `<data>/tasks/<repo>/<n>/task.json` (rewritten at every change) and the final
//! word of its agent in `closing.md`. The repos map is `<data>/repos.json`
//! (name → folder) and `<data>/repo-keys.json` (`had`: short key → repo name).
//! Nothing here writes, and `tokens.json` / `hook-*.json` are never opened.

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The fields of `task.json` that the planner shows. Missing or odd fields are
/// empty (`""`, `0`, `[]`) rather than errors.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskMeta {
    /// `created`, milliseconds since the epoch.
    pub created_ms: u64,
    pub description: String,
    /// `claude`, `omp`, `pi` or `codex` (falls back to `agent`).
    pub app: String,
    /// `settings["worker.agent"]`, e.g. `claude/claude-sonnet-5-5`; `""` when unset.
    pub worker_agent: String,
    pub model: String,
    pub session: String,
    pub worktree: String,
    pub branch: String,
    pub root: String,
    /// `exitedAt` in milliseconds; 0 while the agent has not exited.
    pub exited_at_ms: u64,
    /// `at`: the last change of the record (ms).
    pub at_ms: u64,
    /// `archivedAt` in milliseconds; 0 for a task that is not archived.
    pub archived_at_ms: u64,
    /// `last.gone`: the board saw the agent's pane gone.
    pub gone: bool,
    /// `last.status` is `working`: the agent is busy right now.
    pub working: bool,
    /// Task refs this task waits for, e.g. `herdr-planner/2`.
    pub after: Vec<String>,
    /// `agent`: the program on the task (`claude`).
    pub agent: String,
    /// `on`: the agent that took the task over from the first one (`codex/gpt-6-luna`); `""` while the first is on it.
    pub on: String,
    /// `reviewerOn`: the reviewer that took the review over, the same way.
    pub reviewer_on: String,
    /// `settings["reviewer.agent"]`: the task's own reviewer; `""` when it keeps the repo's or the Galaxy's.
    pub own_reviewer: String,
}

/// The closing note of a task, cut to a size limit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Closing {
    pub text: String,
    /// The file was longer than the limit; `text` ends with `…`.
    pub truncated: bool,
}

/// One repository known to the board.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepoInfo {
    pub name: String,
    /// Short key used in task ids (`APC` for `apicache`); `""` when unknown.
    pub key: String,
    /// Every key the board remembers for this repository (the current one and older `had` ones).
    pub keys: Vec<String>,
    /// Folder of the repository; `""` when unknown.
    pub root: String,
}

/// A repo name is one path segment: no separators, no `..`, no leading `-`.
pub fn valid_repo(repo: &str) -> bool {
    !repo.is_empty()
        && repo.len() <= 200
        && repo != "."
        && !repo.starts_with('-')
        && !repo.contains("..")
        && repo
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `<data>/tasks/<repo>/<n>`; `None` when the repo name is not a single safe segment.
pub fn task_dir(data_dir: &Path, repo: &str, n: u64) -> Option<PathBuf> {
    valid_repo(repo).then(|| data_dir.join("tasks").join(repo).join(n.to_string()))
}

/// The board's record of task `repo/n`; `None` when there is no readable `task.json`.
pub fn read_task(data_dir: &Path, repo: &str, n: u64) -> Option<TaskMeta> {
    let dir = task_dir(data_dir, repo, n)?;
    let text = std::fs::read_to_string(dir.join("task.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    if !v.is_object() {
        return None;
    }
    Some(parse_task(&v))
}

/// The hand-overs the board kept on task `repo/n` (`handovers`, oldest first: `{at, role, from, to, why}`): the lasting
/// record of «codex took over from claude: out of allowance until 17:00», which the row's own line loses at the
/// next report of the agent. Empty when there is no record or none was kept.
pub fn read_handovers(data_dir: &Path, repo: &str, n: u64) -> Vec<Value> {
    let Some(dir) = task_dir(data_dir, repo, n) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(dir.join("task.json")) else { return Vec::new() };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("handovers").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

fn parse_task(v: &Value) -> TaskMeta {
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let millis = |key: &str| {
        v.get(key)
            .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
            .unwrap_or(0)
    };
    let app = {
        let app = text("app");
        if app.is_empty() {
            text("agent")
        } else {
            app
        }
    };
    let worker_agent = v
        .pointer("/settings/worker.agent")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    // Plain strings, or objects with an `id` (the board's `afterDone` shape).
    let after = v
        .get("after")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    Value::Object(_) => x.get("id").and_then(Value::as_str).map(String::from),
                    _ => None,
                })
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    TaskMeta {
        created_ms: millis("created"),
        description: text("description"),
        app,
        worker_agent,
        model: text("model"),
        session: text("session"),
        worktree: text("worktree"),
        branch: text("branch"),
        root: text("root"),
        exited_at_ms: millis("exitedAt"),
        at_ms: millis("at"),
        agent: text("agent"),
        on: text("on"),
        reviewer_on: text("reviewerOn"),
        own_reviewer: v
            .pointer("/settings/reviewer.agent")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        archived_at_ms: millis("archivedAt"),
        gone: v.pointer("/last/gone").and_then(Value::as_bool).unwrap_or(false),
        working: v.pointer("/last/status").and_then(Value::as_str) == Some("working"),
        after,
    }
}

/// The ceiling for one closing note: high enough to show a real note whole (the longest on the board is
/// about 16 KiB), low enough that a runaway file cannot be read without bound.
pub const CLOSING_MAX_BYTES: usize = 1024 * 1024;

/// `<data>/tasks/<repo>/<n>/closing.md`, at most `max_bytes` bytes (cut at a
/// character boundary; `…` is appended when cut). `None` when the file is missing.
pub fn read_closing(data_dir: &Path, repo: &str, n: u64, max_bytes: usize) -> Option<Closing> {
    let path = task_dir(data_dir, repo, n)?.join("closing.md");
    let file = std::fs::File::open(path).ok()?;
    // One byte more than the limit tells whether the file was cut.
    let mut buf = Vec::new();
    file.take(max_bytes as u64 + 1).read_to_end(&mut buf).ok()?;
    if buf.len() <= max_bytes {
        return Some(Closing {
            text: String::from_utf8_lossy(&buf).into_owned(),
            truncated: false,
        });
    }
    // Keep only whole characters: the valid UTF-8 prefix of the cut.
    let valid = match std::str::from_utf8(&buf[..max_bytes]) {
        Ok(_) => max_bytes,
        Err(e) => e.valid_up_to(),
    };
    let mut text = String::from_utf8_lossy(&buf[..valid]).into_owned();
    text.push('…');
    Some(Closing {
        text,
        truncated: true,
    })
}

/// The repos the board knows: every name in `repos.json` or `repo-keys.json`,
/// sorted by name. Missing or broken files give fewer entries, never an error.
pub fn repos(data_dir: &Path) -> Vec<RepoInfo> {
    let folders = read_json(&data_dir.join("repos.json"))
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let keys_file = read_json(&data_dir.join("repo-keys.json"));
    // Current files keep the map under `had`; older ones are the map itself.
    let had = keys_file
        .as_ref()
        .and_then(|v| {
            v.get("had")
                .and_then(Value::as_object)
                .cloned()
                .or_else(|| v.as_object().cloned())
        })
        .unwrap_or_default();

    let mut names: Vec<String> = folders.keys().cloned().collect();
    for repo in had.values().filter_map(Value::as_str) {
        if !names.iter().any(|n| n == repo) {
            names.push(repo.to_string());
        }
    }
    names.sort();
    names
        .into_iter()
        .filter(|name| valid_repo(name))
        .map(|name| {
            // The first key in the file is the repository's current one; the rest are older names.
            let mut keys: Vec<String> = had
                .iter()
                .filter(|(_, repo)| repo.as_str() == Some(name.as_str()))
                .map(|(key, _)| key.clone())
                .collect();
            let key = keys.first().cloned().unwrap_or_default();
            keys.sort();
            keys.dedup();
            let root = folders
                .get(&name)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            RepoInfo { name, key, keys, root }
        })
        .collect()
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a real `task.json` of the board (1.2.4), trimmed.
    const TASK: &str = r#"{
        "id": "herdr-planner/2",
        "repo": "herdr-planner",
        "n": 2,
        "root": "/srv/projects/herdr-planner",
        "title": "Дизайн галактики",
        "description": "Сделать макеты трёх экранов; см. ветку work/2.",
        "owner": "me",
        "app": "omp",
        "stage": "done",
        "created": 1792389651900,
        "at": 1792390697151,
        "settings": {"worker.agent": "omp/deepseek/deepseek-v4-flash", "merge.by": "itself"},
        "model": null,
        "session": "/home/me/.omp/agent/sessions/-x/2026-10-19T07-26-47-619Z_01a1.jsonl",
        "worktree": "/home/me/.herdr/worktrees/herdr-planner/work-2",
        "branch": "work/2",
        "after": ["herdr-planner/1", "herdr-planner/7"],
        "afterDone": [{"id": "herdr-planner/1", "title": "Старое"}],
        "exitedAt": 1792390700000,
        "agent": "omp",
        "on": "codex/gpt-6-luna",
        "reviewerOn": "codex",
        "tokens": "not read"
    }"#;

    fn data_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    #[test]
    fn reads_the_fields_of_a_real_shaped_task() {
        let dir = data_with(&[("tasks/herdr-planner/2/task.json", TASK)]);
        let t = read_task(dir.path(), "herdr-planner", 2).unwrap();
        assert_eq!(t.created_ms, 1_792_389_651_900);
        assert_eq!(
            t.description,
            "Сделать макеты трёх экранов; см. ветку work/2."
        );
        assert_eq!(t.app, "omp");
        assert_eq!(t.worker_agent, "omp/deepseek/deepseek-v4-flash");
        assert_eq!(t.model, "", "a null model is empty");
        assert!(t.session.ends_with(".jsonl"));
        assert_eq!(t.branch, "work/2");
        assert_eq!(t.root, "/srv/projects/herdr-planner");
        assert_eq!(t.exited_at_ms, 1_792_390_700_000);
        assert_eq!(t.after, vec!["herdr-planner/1", "herdr-planner/7"]);
        assert_eq!((t.agent.as_str(), t.on.as_str(), t.reviewer_on.as_str()), ("omp", "codex/gpt-6-luna", "codex"));
        assert_eq!(t.own_reviewer, "", "no reviewer of its own");
    }

    #[test]
    fn tolerates_missing_and_odd_fields() {
        let dir = data_with(&[(
            "tasks/apicache/5/task.json",
            r#"{"agent":"claude","settings":null,"exitedAt":null,"after":[{"id":"a/1"},3,""]}"#,
        )]);
        let t = read_task(dir.path(), "apicache", 5).unwrap();
        assert_eq!(t.app, "claude", "app falls back to agent");
        assert_eq!(t.worker_agent, "");
        assert_eq!(t.exited_at_ms, 0);
        assert_eq!(t.after, vec!["a/1"]);
        assert_eq!(t.created_ms, 0);
        assert_eq!((t.on.as_str(), t.reviewer_on.as_str()), ("", ""), "no hand-over");
    }

    #[test]
    fn missing_or_broken_files_give_none() {
        let dir = data_with(&[("tasks/apicache/5/task.json", "{ broken")]);
        assert!(read_task(dir.path(), "apicache", 5).is_none());
        assert!(read_task(dir.path(), "apicache", 6).is_none());
        assert!(read_task(dir.path(), "nope", 1).is_none());
        assert!(read_closing(dir.path(), "apicache", 5, 100).is_none());
    }

    #[test]
    fn unsafe_repo_names_never_build_a_path() {
        let dir = data_with(&[("tasks/x/1/task.json", "{}")]);
        for bad in ["", ".", "..", "../x", "a/b", "a\\b", "-rf", "a..b", "x\0y"] {
            assert!(!valid_repo(bad), "{bad:?} must be refused");
            assert!(task_dir(dir.path(), bad, 1).is_none());
            assert!(read_task(dir.path(), bad, 1).is_none());
            assert!(read_closing(dir.path(), bad, 1, 10).is_none());
        }
        assert!(valid_repo("herdr-planner") && valid_repo("data") && valid_repo("my.repo_2"));
        assert_eq!(
            task_dir(Path::new("/d"), "herdr-planner", 12).unwrap(),
            PathBuf::from("/d/tasks/herdr-planner/12")
        );
    }

    #[test]
    fn closing_is_cut_at_a_character_boundary() {
        let text = "Итог: ".repeat(100); // Cyrillic: 2 bytes per letter
        let dir = data_with(&[("tasks/r/3/closing.md", &text)]);
        let short = read_closing(dir.path(), "r", 3, 50).unwrap();
        assert!(short.truncated);
        assert!(short.text.ends_with('…'));
        let body = short.text.trim_end_matches('…');
        assert!(body.len() <= 50 && text.starts_with(body));
        let whole = read_closing(dir.path(), "r", 3, 100_000).unwrap();
        assert!(!whole.truncated);
        assert_eq!(whole.text, text);
    }

    #[test]
    fn a_note_over_the_old_limit_is_read_whole() {
        // The old endpoint cap was 16 KiB: an ordinary long note must not be cut by it.
        let text = format!("{}\nКОНЕЦ-ИТОГА\n", "- строка итога\n".repeat(2000));
        assert!(text.len() > 16 * 1024);
        let dir = data_with(&[("tasks/r/7/closing.md", &text)]);
        let whole = read_closing(dir.path(), "r", 7, CLOSING_MAX_BYTES).unwrap();
        assert!(!whole.truncated);
        assert_eq!(whole.text, text);
        assert!(whole.text.contains("КОНЕЦ-ИТОГА"));
    }

    #[test]
    fn repos_join_names_folders_and_keys() {
        let dir = data_with(&[
            (
                "repos.json",
                r#"{"apicache":"/src/APICache","herdr-planner":"/src/hp"}"#,
            ),
            (
                "repo-keys.json",
                r#"{"upgraded":true,"had":{"APC":"apicache","PLN":"herdr-planner","CHT":"chat"}}"#,
            ),
        ]);
        let r = repos(dir.path());
        let names: Vec<&str> = r.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["apicache", "chat", "herdr-planner"]);
        assert_eq!(r[0].key, "APC");
        assert_eq!(r[0].root, "/src/APICache");
        assert_eq!(r[1].root, "", "a repo known only by key has no folder");
        assert_eq!(r[2].key, "PLN");
    }

    #[test]
    fn repos_tolerate_missing_files() {
        let dir = data_with(&[("repo-keys.json", "not json")]);
        assert!(repos(dir.path()).is_empty());
        assert!(repos(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    #[ignore = "reads the real board data; run by hand"]
    fn real_machine_board_reads_without_panic() {
        // Run by hand: `cargo test real_machine -- --ignored --nocapture`.
        let data = crate::config::home().join(".herdr-projects/data");
        let repos = repos(&data);
        let mut tasks = 0;
        for r in &repos {
            for n in 1..=50 {
                if read_task(&data, &r.name, n).is_some() {
                    tasks += 1;
                }
            }
        }
        println!("repos {} tasks {}", repos.len(), tasks);
    }
}
