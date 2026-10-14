//! TASKS.md: `- [ ] text` / `- [x] text` lines. A task may name its owner in
//! parentheses at the start (`(me)`, `(claude-sonnet)`, `(Priya Rao)`) and the
//! thread that works on it at the end (` · t-0016`).

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Task {
    pub idx: usize,
    pub done: bool,
    /// The full text as written (markdown stripped), the key for saved answers.
    pub full: String,
    /// The owner in the leading parentheses, if any.
    pub owner: Option<String>,
    /// `t-NNNN` after a trailing ` · `, if any.
    pub thread: Option<String>,
}

/// Drops the markdown that would only clutter a one-line view: bold/italic
/// markers, inline code backticks and link brackets.
pub fn strip_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '_' | '`' => {}
            '[' => {}
            ']' => {
                // `[text](url)` keeps the text.
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for d in chars.by_ref() {
                        if d == ')' {
                            break;
                        }
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_tasks(content: &str) -> Vec<Task> {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) else {
            continue;
        };
        let rest = rest.trim_start();
        let (done, text) = if let Some(x) = rest.strip_prefix("[ ]") {
            (false, x)
        } else if let Some(x) = rest
            .strip_prefix("[x]")
            .or_else(|| rest.strip_prefix("[X]"))
        {
            (true, x)
        } else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let full = strip_md(text);
        let owner = full
            .strip_prefix('(')
            .and_then(|r| r.split_once(')'))
            .map(|(o, _)| o.trim().to_string())
            .filter(|o| !o.is_empty() && !o.contains('('));
        let thread = full
            .rsplit_once(" · ")
            .map(|(_, t)| t.trim())
            .filter(|t| {
                t.starts_with("t-") && t[2..].chars().all(|c| c.is_ascii_digit()) && t.len() > 2
            })
            .map(String::from);
        out.push(Task {
            idx: out.len() + 1,
            done,
            full,
            owner,
            thread,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owners_and_threads() {
        let text = "# Tasks\n\n## Backlog\n\n- [ ] Herdr Planner, этап 1: каркас (claude-fable) · t-0016\n- [ ] (me) Выпустить ключ API\n- [x] **0.7.0**: треды через `thread list` (2026-10-12)\n* [ ] (Priya Rao) звонок\n- not a task\n";
        let t = parse_tasks(text);
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].thread.as_deref(), Some("t-0016"));
        assert_eq!(t[0].owner, None);
        assert!(!t[0].done);
        assert_eq!(t[1].owner.as_deref(), Some("me"));
        assert_eq!(t[1].thread, None);
        assert!(t[2].done);
        assert_eq!(t[2].full, "0.7.0: треды через thread list (2026-10-12)");
        assert_eq!(t[3].owner.as_deref(), Some("Priya Rao"));
        assert_eq!(t[3].idx, 4);
    }

    #[test]
    fn strips_links() {
        assert_eq!(strip_md("see [docs](http://x) now"), "see docs now");
    }
}
