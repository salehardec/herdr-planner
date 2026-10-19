//! What the board keeps in its data folder (`$HERDR_PROJECTS_DATA_DIR`): the
//! repos it knows (`repos.json`, name → folder), the short key of each
//! (`repo-keys.json`), the herdr space of each repo (`spaces.json`) and, for
//! when a task came to its stage, `tasks/<repo>/<n>/task.json`. All read only,
//! and none of it is needed for the tasks themselves: a missing file is an
//! empty answer.

use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `repos.json`: repo name → its folder, by name.
pub fn read_repos(dir: &Path) -> BTreeMap<String, String> {
    match read_json(&dir.join("repos.json")) {
        Some(Value::Object(m)) => m
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|p| (k, p.to_string())))
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// `repo-keys.json`: repo name → short key. The file records every key a repo
/// ever had (`had`: key → repo); when a repo had several, the last by name stands for it.
/// Keys the board prints (`list`) win over this where the planner can see them.
pub fn read_keys(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(Value::Object(had)) = read_json(&dir.join("repo-keys.json")).and_then(|v| v.get("had").cloned()) {
        for (key, repo) in had {
            if let Some(repo) = repo.as_str() {
                out.insert(repo.to_string(), key);
            }
        }
    }
    out
}

/// `spaces.json`: the herdr workspace id of each repo's folder.
pub fn read_spaces(dir: &Path) -> Vec<(String, String)> {
    match read_json(&dir.join("spaces.json")) {
        Some(Value::Object(m)) => m
            .into_iter()
            .filter_map(|(k, v)| {
                // `socket|folder`
                let path = k.split_once('|').map_or(k.as_str(), |(_, p)| p).to_string();
                Some((path, v.as_str()?.to_string()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The file with the task's own record.
pub fn task_file(dir: &Path, id: &str) -> Option<PathBuf> {
    let (repo, n) = id.rsplit_once('/')?;
    if repo.is_empty() || repo.contains("..") || n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(dir.join("tasks").join(repo).join(n).join("task.json"))
}

/// When tasks came to their stage (`at` of `task.json`), cached by the file's modification time.
#[derive(Default)]
pub struct TimeCache {
    seen: HashMap<String, (SystemTime, u64)>,
}

impl TimeCache {
    /// The time of each id that has a record; forgets tasks that are gone.
    pub fn times(&mut self, dir: &Path, ids: &[&str]) -> HashMap<String, u64> {
        let mut out = HashMap::new();
        let mut keep = HashMap::new();
        for id in ids {
            let Some(file) = task_file(dir, id) else { continue };
            let Ok(mtime) = std::fs::metadata(&file).and_then(|m| m.modified()) else {
                continue;
            };
            let at = match self.seen.get(*id) {
                Some((t, at)) if *t == mtime => *at,
                _ => read_json(&file)
                    .and_then(|v| v.get("at").and_then(Value::as_f64))
                    .map_or(0, |n| n.max(0.0) as u64),
            };
            keep.insert(id.to_string(), (mtime, at));
            if at > 0 {
                out.insert(id.to_string(), at);
            }
        }
        self.seen = keep;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("herdr-planner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_the_boards_files() {
        let d = temp("repos");
        std::fs::write(d.join("repos.json"), r#"{"zeta":"/z","alpha":"/a","bad":3}"#).unwrap();
        std::fs::write(d.join("repo-keys.json"), r#"{"upgraded":true,"had":{"ALP":"alpha","ZET":"zeta"}}"#).unwrap();
        std::fs::write(d.join("spaces.json"), r#"{"/s/herdr.sock|/a":"wDA"}"#).unwrap();
        let repos = read_repos(&d);
        assert_eq!(repos.keys().collect::<Vec<_>>(), ["alpha", "zeta"]);
        assert_eq!(repos["alpha"], "/a");
        assert_eq!(read_keys(&d)["zeta"], "ZET");
        assert_eq!(read_spaces(&d), vec![("/a".to_string(), "wDA".to_string())]);
        // Nothing there: empty answers.
        let none = temp("none");
        assert!(read_repos(&none).is_empty() && read_keys(&none).is_empty() && read_spaces(&none).is_empty());
        std::fs::remove_dir_all(d).unwrap();
        std::fs::remove_dir_all(none).unwrap();
    }

    #[test]
    fn task_times_follow_the_file() {
        let d = temp("times");
        std::fs::create_dir_all(d.join("tasks/alpha/3")).unwrap();
        std::fs::write(d.join("tasks/alpha/3/task.json"), r#"{"id":"alpha/3","at":1792390116230}"#).unwrap();
        let mut cache = TimeCache::default();
        let t = cache.times(&d, &["alpha/3", "alpha/4", "../x/1", "alpha/x"]);
        assert_eq!(t.len(), 1);
        assert_eq!(t["alpha/3"], 1792390116230);
        assert!(task_file(&d, "../x/1").is_none());
        assert!(task_file(&d, "alpha/x").is_none());
        std::fs::remove_dir_all(d).unwrap();
    }
}
