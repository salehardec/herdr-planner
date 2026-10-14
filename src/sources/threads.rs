//! `herdr-projects thread list <slug> --json`: one object per thread.
//! Threads are grouped by `group` in order of first appearance.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thread {
    pub id: String,
    pub title: String,
    pub note: String,
    pub group: String,
    pub status: String,
    pub branch: String,
    pub base: String,
    pub worktree_path: String,
    pub cwd: String,
    pub kind: String,
    pub pr: String,
    pub pr_state: String,
    pub profile: String,
    pub profile_args: Vec<String>,
    pub agent: String,
    pub machine: String,
    pub remote_profile: bool,
    pub repo: String,
    pub error: String,
    pub launched_at: String,
    pub percent: Option<u32>,
    pub activity: String,
    pub next: Vec<String>,
    pub created: String,
    pub updated: String,
    pub rank: Option<u32>,
}

impl Thread {
    /// A start that failed: herdr says so, or a launch error with no launch behind it.
    pub fn failed(&self) -> bool {
        self.status == "failed" || (!self.error.is_empty() && self.launched_at.is_empty())
    }

    pub fn closed(&self) -> bool {
        self.group == "Resolved" || self.status == "resolved"
    }

    /// The checkout the thread works in: `worktree_path`, which is empty for a
    /// resolved thread, whose path then lives in `cwd`.
    pub fn path(&self) -> Option<&str> {
        [&self.worktree_path, &self.cwd]
            .into_iter()
            .find(|p| !p.is_empty())
            .map(String::as_str)
    }

    pub fn remote(&self) -> bool {
        self.remote_profile || !self.machine.is_empty()
    }

    /// Only a worktree thread has a directory of its own; a tab or checkout
    /// thread works in the project's.
    pub fn own_path(&self) -> Option<&str> {
        if self.kind == "worktree" {
            self.path()
        } else {
            None
        }
    }

    /// The pull request number in `…/pull/N`.
    pub fn pr_number(&self) -> Option<u32> {
        let (_, rest) = self.pr.split_once("/pull/")?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    /// The worktree as herdr names it: the branch, else the folder once the branch is gone.
    pub fn worktree_name(&self) -> Option<String> {
        if !self.branch.is_empty() {
            return Some(self.branch.clone());
        }
        let path = self.worktree_path.trim_end_matches('/');
        path.rsplit('/')
            .next()
            .filter(|n| !n.is_empty())
            .map(String::from)
    }
}

/// `hp/<project>/t-0115-18-prisma` reads as `t-0115-18-prisma` inside its project.
pub fn short_worktree(name: &str, slug: &str) -> String {
    let prefix = format!("hp/{slug}/");
    name.strip_prefix(&prefix).unwrap_or(name).to_string()
}

/// A herdr thread group in words, and the order groups ask for attention.
pub fn group_word(group: &str) -> String {
    match group {
        "Needs you" | "Waiting on you" => "ждёт тебя",
        "Ready for review" => "ревью",
        "Blocked" => "блок",
        "Landing" => "вливается",
        "Working" => "в работе",
        "Idle" => "простаивает",
        "Resolved" => "сделан",
        other => other,
    }
    .to_string()
}

pub fn group_rank(group: &str) -> u32 {
    match group {
        "Needs you" | "Waiting on you" => 1,
        "Ready for review" => 2,
        "Blocked" => 3,
        "Landing" => 3,
        "Working" => 4,
        "Idle" => 5,
        _ => 6,
    }
}

/// herdr's `note` of a thread in words, or none when it only repeats the
/// group: the agent states (`working`, `idle`), the reasons a thread was
/// resolved (`manual`, `merged`) and what the ticker saw (`pane closed`).
pub fn note_word(group: &str, note: &str) -> Option<String> {
    let note = note.trim();
    let word = match note {
        "" => return None,
        "working" | "idle" => {
            // The group already says so.
            if matches!(group, "Working" | "Idle") {
                return None;
            }
            if note == "working" {
                "агент работает"
            } else {
                "агент простаивает"
            }
        }
        "blocked" => "агент ждёт ответа",
        "done" => "агент закончил",
        "unknown" => "состояние неизвестно",
        "manual" => "закрыт вручную",
        "merged" => "PR слит",
        "auto" => "закрыт автоматически",
        "pane closed" => "пейн закрыт",
        "no agent" => "агента нет",
        "session unreachable" => "сессия herdr недоступна",
        "not polled yet" => "ещё не опрошен",
        other => {
            if other.starts_with("failed:") {
                // The launch error has its own line.
                return None;
            }
            if let Some((state, machine)) = other.split_once(", on ") {
                let state = match state {
                    "working" => "работает",
                    "idle" => "простаивает",
                    "blocked" => "ждёт ответа",
                    "done" => "закончил",
                    other => other,
                };
                return Some(format!("{state} на машине {machine}"));
            }
            return Some(other.to_string());
        }
    };
    Some(word.to_string())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ThreadGroup {
    pub name: String,
    pub threads: Vec<Thread>,
}

pub fn parse_threads(stdout: &str) -> Result<Vec<ThreadGroup>, String> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(stdout)
        .map_err(|_| "ответ thread list не разобран как JSON".to_string())?;
    let mut groups: Vec<ThreadGroup> = Vec::new();
    for row in rows {
        let Ok(mut t) = serde_json::from_value::<Thread>(row) else {
            continue;
        };
        if t.id.is_empty() {
            continue;
        }
        if t.group.is_empty() {
            t.group = "Working".into();
        }
        let name = t.group.clone();
        match groups.iter_mut().find(|g| g.name == name) {
            Some(g) => g.threads.push(t),
            None => groups.push(ThreadGroup {
                name,
                threads: vec![t],
            }),
        }
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[
      {"id":"t-0001","title":"Строка","group":"Resolved","status":"resolved","worktree_path":"","cwd":"/wt/t-0001","launched_at":"2026-10-12T04:25:30Z","pr":"https://x/pull/1","pr_state":"MERGED","profile":"claude-sonnet","profile_args":[]},
      {"id":"t-0002","title":"Сбой","group":"Resolved","status":"resolved","error":"herdr: timed out","launched_at":""},
      {"id":"t-0016","title":"Этап 1","group":"Working","status":"working","worktree_path":"/wt/t-0016","cwd":"/wt/t-0016","launched_at":"2026-10-14T05:00:00Z","machine":"mac","percent":15},
      {"notid":true}
    ]"#;

    #[test]
    fn groups_in_order_of_appearance() {
        let g = parse_threads(JSON).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "Resolved");
        assert_eq!(g[0].threads.len(), 2);
        assert_eq!(g[1].threads[0].id, "t-0016");
        assert_eq!(g[1].threads[0].percent, Some(15));
        assert!(g[1].threads[0].remote());
    }

    #[test]
    fn failure_and_paths() {
        let g = parse_threads(JSON).unwrap();
        let resolved = &g[0].threads;
        assert_eq!(resolved[0].path(), Some("/wt/t-0001"));
        assert!(!resolved[0].failed());
        assert!(resolved[1].failed());
        assert_eq!(resolved[1].path(), None);
    }

    #[test]
    fn pr_and_worktree_names() {
        let t = Thread {
            pr: "https://github.com/o/r/pull/12".into(),
            branch: "hp/proj/t-0016-1".into(),
            kind: "worktree".into(),
            worktree_path: "/wt/t-0016".into(),
            ..Default::default()
        };
        assert_eq!(t.pr_number(), Some(12));
        assert_eq!(t.worktree_name().as_deref(), Some("hp/proj/t-0016-1"));
        assert_eq!(short_worktree("hp/proj/t-0016-1", "proj"), "t-0016-1");
        assert_eq!(short_worktree("hp/other/t-1", "proj"), "hp/other/t-1");
        assert_eq!(t.own_path(), Some("/wt/t-0016"));
        let tab = Thread {
            kind: "tab".into(),
            cwd: "/repo".into(),
            ..Default::default()
        };
        assert_eq!(tab.own_path(), None);
        let gone = Thread {
            worktree_path: "/wt/old/".into(),
            ..Default::default()
        };
        assert_eq!(gone.worktree_name().as_deref(), Some("old"));
    }

    #[test]
    fn notes_in_words() {
        assert_eq!(note_word("Working", "working"), None);
        assert_eq!(note_word("Idle", "idle"), None);
        assert_eq!(note_word("Resolved", "merged").as_deref(), Some("PR слит"));
        assert_eq!(
            note_word("Resolved", "manual").as_deref(),
            Some("закрыт вручную")
        );
        assert_eq!(
            note_word("Ready for review", "pane closed").as_deref(),
            Some("пейн закрыт")
        );
        assert_eq!(
            note_word("Ready for review", "idle").as_deref(),
            Some("агент простаивает")
        );
        assert_eq!(
            note_word("Working", "working, on mac").as_deref(),
            Some("работает на машине mac")
        );
        assert_eq!(note_word("Working", "failed: boom"), None);
        assert_eq!(
            note_word("Idle", "something new").as_deref(),
            Some("something new")
        );
        assert_eq!(group_word("Ready for review"), "ревью");
        assert!(group_rank("Waiting on you") < group_rank("Working"));
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(parse_threads("not json").is_err());
    }
}
