//! `herdr-projects thread list <slug> --json`: one object per thread.
//! Threads are grouped by `group` in order of first appearance.

// Reference logic: parts are used only by tests and by stages 2–4 (the page mirrors the list logic).
#![allow(dead_code)]

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
    fn bad_json_is_an_error() {
        assert!(parse_threads("not json").is_err());
    }
}
