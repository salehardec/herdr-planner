//! What `herdr-projects coordinator` says, and the piece of it the dashboard
//! shows as the Coordinator's last answer.
//!
//! The board has no "read the last reply" command. `herdr-projects coordinator`
//! prints the Coordinator's standing line and then the last lines of its pane
//! (through herdr's sidebar). The planner shows exactly that tail, labelled as
//! such; when herdr is not running the board returns only the standing line,
//! and the page says that the answer is unavailable.

use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CoordinatorView {
    /// `idle · claude · claude-opus-5-5 · medium · default: …`
    pub status_line: String,
    /// The first word of the standing line: `idle`, `working`, `stopped`, ….
    pub status: String,
    /// The lines after the standing line, trimmed.
    pub reply: String,
    /// The Coordinator shows a dialog on its own screen (`state == blocked`):
    /// it waits for a person, not for a turn.
    pub dialog: bool,
    pub error: String,
    /// A message was just sent and its answer has not arrived yet.
    pub busy: bool,
    /// The last text the human sent from this planner.
    pub last_sent: String,
    pub sent_at: u64,
}

pub fn parse(stdout: &str) -> CoordinatorView {
    let text = stdout.trim_end_matches(['\r', '\n']);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("").trim();
    let status_line = first
        .strip_prefix("Coordinator:")
        .unwrap_or(first)
        .trim()
        .to_string();
    let status = status_line
        .split(['·', ' '])
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let mut rest: Vec<&str> = lines.collect();
    while rest.first().is_some_and(|l| l.trim().is_empty()) {
        rest.remove(0);
    }
    while rest.last().is_some_and(|l| l.trim().is_empty()) {
        rest.pop();
    }
    let dialog = status == "blocked";
    CoordinatorView {
        status_line,
        status,
        reply: rest.join("\n"),
        dialog,
        ..CoordinatorView::default()
    }
}

/// Whether the standing line says the Coordinator is on something. `blocked`
/// is not work: it shows a dialog and waits for the human.
pub fn is_busy(status: &str) -> bool {
    !(status.is_empty()
        || status == "idle"
        || status == "stopped"
        || status == "off"
        || status == "blocked")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_the_last_lines() {
        let out = "Coordinator: idle · claude · claude-opus-5-5 · medium · default: claude · claude-opus-5-5 · medium\n\nPLN-2 слита.\n\nДописал требования.\n\nUpdated PLN-3. Approved PLN-2\n";
        let c = parse(out);
        assert_eq!(c.status, "idle");
        assert_eq!(
            c.status_line,
            "idle · claude · claude-opus-5-5 · medium · default: claude · claude-opus-5-5 · medium"
        );
        assert_eq!(
            c.reply,
            "PLN-2 слита.\n\nДописал требования.\n\nUpdated PLN-3. Approved PLN-2"
        );
        assert!(!c.busy);
    }

    #[test]
    fn only_the_standing_line() {
        let c = parse("Coordinator: working · omp · deepseek-v4-flash · high\n");
        assert_eq!(c.status, "working");
        assert_eq!(c.reply, "");
        assert!(is_busy(&c.status));
    }

    #[test]
    fn empty_and_unexpected_output() {
        let c = parse("");
        assert_eq!(c.status, "");
        assert_eq!(c.reply, "");
        assert!(!is_busy(&c.status));
        let c = parse("something else\nline two\n");
        assert_eq!(c.status_line, "something else");
        assert_eq!(c.reply, "line two");
    }

    #[test]
    fn busy_words() {
        assert!(is_busy("working"));
        assert!(!is_busy("idle"));
        assert!(!is_busy("stopped"));
        assert!(!is_busy(""));
        assert!(
            !is_busy("blocked"),
            "a dialog waits for the human, not for a turn"
        );
    }

    #[test]
    fn blocked_means_a_dialog_on_its_screen() {
        let c = parse("Coordinator: blocked · claude · claude-opus-5-5 · medium\n\nMay I remove the old branches?");
        assert_eq!(c.status, "blocked");
        assert!(c.dialog);
        assert!(!c.busy);
        assert_eq!(c.reply, "May I remove the old branches?");
        assert!(!parse("Coordinator: idle · claude\n").dialog);
        assert!(!parse("").dialog);
    }
}
