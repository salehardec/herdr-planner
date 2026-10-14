//! PROJECT.md: a TOML front matter between `+++` lines (`name`, `goal`,
//! `[[repos]]` with `path` and an optional `machine`), then instructions.
//! `path` outside the front matter (in the instructions) is not a repository.

// Reference logic: parts are used only by tests and by stages 2–4 (the page mirrors the list logic).
#![allow(dead_code)]

use crate::model::Repo;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ProjectMd {
    pub name: Option<String>,
    pub goal: Option<String>,
    pub repos: Vec<Repo>,
}

impl ProjectMd {
    /// The repository the planner works in itself: the first one on this machine.
    pub fn local_repo(&self) -> Option<&str> {
        self.repos
            .iter()
            .find(|r| r.machine.is_empty())
            .map(|r| r.path.as_str())
    }
}

/// The text between the first `+++` line and the next one, or none.
pub fn front_matter(content: &str) -> Option<&str> {
    let text = content.trim_start_matches('\u{feff}');
    let mut lines = text.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim() != "+++" {
        return None;
    }
    let start = first.len();
    let mut pos = start;
    for line in lines {
        if line.trim() == "+++" {
            return Some(&text[start..pos]);
        }
        pos += line.len();
    }
    None
}

pub fn parse(content: &str) -> ProjectMd {
    let content = content.replace('\r', "");
    let Some(front) = front_matter(&content) else {
        return ProjectMd::default();
    };
    let mut out = ProjectMd::default();
    if let Ok(doc) = toml::from_str::<toml::Value>(front) {
        out.name = doc
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        out.goal = doc
            .get("goal")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        if let Some(list) = doc.get("repos").and_then(|v| v.as_array()) {
            for r in list {
                let path = r
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if path.is_empty() {
                    continue;
                }
                let machine = r
                    .get("machine")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                out.repos.push(Repo { path, machine });
            }
        }
        return out;
    }
    // Not valid TOML (a hand-edited header): a line-wise fallback like the plugin's.
    let mut cur: Option<Repo> = None;
    for line in front.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if let Some(r) = cur.take() {
                if !r.path.is_empty() {
                    out.repos.push(r);
                }
            }
            if t.replace(' ', "") == "[[repos]]" {
                cur = Some(Repo::default());
            }
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').to_string();
        match (k.trim(), cur.as_mut()) {
            ("path", Some(r)) => r.path = v,
            ("machine", Some(r)) => r.machine = v,
            ("name", None) if !v.is_empty() => out.name = Some(v),
            ("goal", None) if !v.is_empty() => out.goal = Some(v),
            _ => {}
        }
    }
    if let Some(r) = cur.take() {
        if !r.path.is_empty() {
            out.repos.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "+++\nname = \"Tern Herdr Plugin\"\ngoal = \"Плагин Tern\"\ncoordinator_profile = \"claude\"\n\n[[repos]]\npath = \"/mnt/a\"\n\n[[repos]]\npath = \"/mnt/b\"\nmachine = \"mac\"\n+++\n\n# Instructions\npath = \"/not/a/repo\"\n";

    #[test]
    fn parses_front_matter() {
        let md = parse(DOC);
        assert_eq!(md.name.as_deref(), Some("Tern Herdr Plugin"));
        assert_eq!(md.goal.as_deref(), Some("Плагин Tern"));
        assert_eq!(md.repos.len(), 2);
        assert_eq!(md.repos[1].machine, "mac");
        assert_eq!(md.local_repo(), Some("/mnt/a"));
    }

    #[test]
    fn no_front_matter_is_empty() {
        assert_eq!(parse("# Just text\nname = \"x\"\n"), ProjectMd::default());
        assert_eq!(parse("+++\nname = \"x\"\n"), ProjectMd::default());
    }

    #[test]
    fn broken_toml_falls_back() {
        let md = parse("+++\nname = \"X\"\nweird line\n[[repos]]\npath = \"/r\"\n+++\n");
        assert_eq!(md.name.as_deref(), Some("X"));
        assert_eq!(md.repos.len(), 1);
    }

    #[test]
    fn remote_only_has_no_local_repo() {
        let md = parse("+++\n[[repos]]\npath = \"/r\"\nmachine = \"m\"\n+++\n");
        assert_eq!(md.local_repo(), None);
    }
}
