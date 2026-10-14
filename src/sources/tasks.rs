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
    /// `full` cut to one line of the card.
    pub text: String,
    /// The indented lines under the task (its notes), joined with line breaks.
    pub notes: String,
    /// A thread already works on it: it cannot be given up from the card.
    pub thread_led: bool,
}

/// `s` cut to `n` characters, with an ellipsis.
pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A thread leads the task when the line ends with ` · t-NNNN` or `→ t-NNNN)`.
fn led_by_thread(full: &str) -> bool {
    let tail = full.trim_end();
    let ends_id = |s: &str| {
        let id = s
            .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .next()
            .unwrap_or("");
        id.strip_prefix("t-")
            .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
    };
    if let Some((_, t)) = tail.rsplit_once('·') {
        if ends_id(t.trim()) {
            return true;
        }
    }
    if let Some(inner) = tail.strip_suffix(')') {
        if let Some((_, t)) = inner.rsplit_once('→') {
            return ends_id(t.trim());
        }
    }
    false
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

/// `- [ ] text` / `- [x] text`: whether it is done, and the text after the box.
fn checkbox(t: &str) -> Option<(bool, &str)> {
    let rest = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))?
        .trim_start();
    if let Some(x) = rest.strip_prefix("[ ]") {
        Some((false, x))
    } else {
        rest.strip_prefix("[x]")
            .or_else(|| rest.strip_prefix("[X]"))
            .map(|x| (true, x))
    }
}

pub fn parse_tasks(content: &str) -> Vec<Task> {
    let mut out: Vec<Task> = Vec::new();
    // Indented lines belong to the task above until a blank line or a new task.
    let mut open_notes = false;
    for line in content.lines() {
        let t = line.trim_start();
        let indented = line.len() > t.len();
        let Some((done, text)) = checkbox(t) else {
            if indented && open_notes && !t.is_empty() {
                if let Some(last) = out.last_mut() {
                    if !last.notes.is_empty() {
                        last.notes.push('\n');
                    }
                    last.notes.push_str(t.trim_end());
                }
            } else {
                open_notes = false;
            }
            continue;
        };
        open_notes = false;
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        open_notes = true;
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
        let thread_led = thread.is_some() || led_by_thread(&full);
        out.push(Task {
            idx: out.len() + 1,
            done,
            text: clip(&full, 260),
            full,
            owner,
            thread,
            notes: String::new(),
            thread_led,
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
    fn notes_under_a_task() {
        let text = "## Backlog\n- [ ] Этап 2 (claude-sonnet) · t-0001\n  Треды: git-строки.\n  Задачи: решить.\n- [ ] Второй\n\n  Не заметка после пустой строки\n- [x] Третий → t-0007)\n";
        let t = parse_tasks(text);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].notes, "Треды: git-строки.\nЗадачи: решить.");
        assert!(t[0].thread_led);
        assert_eq!(t[1].notes, "");
        assert!(!t[1].thread_led);
        assert!(t[2].thread_led);
        assert_eq!(clip("абвгд", 3), "аб…");
        assert!(led_by_thread("Сделать (x → t-0042)"));
        assert!(!led_by_thread("Сделать · не тред"));
    }

    #[test]
    fn strips_links() {
        assert_eq!(strip_md("see [docs](http://x) now"), "see docs now");
    }
}
