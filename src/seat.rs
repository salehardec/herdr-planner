//! Who is on a task right now: the reviewer while it is under review, the working agent while it works.
//!
//! The board decides it the same way (`on_now`, `reviewer_now`): the agent that took the task over (`on`,
//! `reviewerOn` in the task's record), else the task's own setting, else the repository's or the Galaxy's, and
//! the program that is actually on the task wins over a setting that names another program. The seat is derived
//! from the task's status at every read, so it is gone the moment the review ends or the agent leaves.

use crate::board::{Status, Task};
use crate::taskmeta::TaskMeta;
use serde::Serialize;

/// The settings a task inherits (what `herdr-projects show --repo NAME` gives for the repository).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Crew {
    pub reviewer: String,
    pub worker: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Seat {
    /// `reviewer` or `worker`.
    pub role: String,
    pub program: String,
    /// `kimi-coding/k3.1-pro`; empty when only the program is known.
    pub model: String,
    /// The reviewer or worker that took over from the first one (the board's hand-over).
    pub took_over: bool,
}

/// `pi/kimi-coding/k3.1-pro` → (`pi`, `kimi-coding/k3.1-pro`); `none` and an empty value name nobody.
fn split(spec: &str) -> (String, String) {
    let spec = spec.trim();
    if spec.is_empty() || spec == "none" {
        return Default::default();
    }
    match spec.split_once('/') {
        Some((p, m)) => (p.to_string(), m.to_string()),
        None => (spec.to_string(), String::new()),
    }
}

fn first<'a>(of: &[&'a str]) -> &'a str {
    of.iter().copied().find(|s| !s.trim().is_empty() && s.trim() != "none").unwrap_or("")
}

fn seat(role: &str, spec: &str, took_over: bool) -> Seat {
    let (program, model) = split(spec);
    Seat { role: role.into(), program, model, took_over }
}

/// The seat of one task, or none when nobody sits on it now.
pub fn of(t: &Task, m: &TaskMeta, crew: &Crew) -> Option<Seat> {
    if t.archived {
        return None;
    }
    // «Ревью» and «ревьюеру нужен ты»: the reviewer's terminal is where the human is needed.
    if t.status == Status::Review || t.stage == "reviewneeds" {
        let spec = first(&[&m.reviewer_on, &m.own_reviewer, &crew.reviewer]);
        // The program that really reviews is written on the task when its pane is made: the board puts the working
        // agent's program there when the configured one is not installed here, and without a hand-over on record.
        let kind = m.reviewer_kind.trim();
        let spec = if kind.is_empty() || split(spec).0 == kind { spec } else { kind };
        return Some(seat("reviewer", spec, !m.reviewer_on.trim().is_empty()));
    }
    if t.status == Status::Working && t.exited_at == 0 && t.finished_ms == 0 {
        let program = first(&[&m.agent, &t.app]);
        let said = first(&[&m.worker_agent, &crew.worker]);
        let spec = if !m.on.trim().is_empty() {
            m.on.as_str()
        } else if program.is_empty() || split(said).0 == program {
            said
        } else {
            program
        };
        return Some(seat("worker", spec, !m.on.trim().is_empty()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: Status, stage: &str) -> Task {
        Task { id: "alpha/7".into(), stage: stage.into(), status, ..Task::default() }
    }

    fn crew() -> Crew {
        Crew { reviewer: "claude/claude-opus-5-5".into(), worker: "claude/claude-sonnet-5-5".into() }
    }

    #[test]
    fn the_reviewer_is_the_settings_when_nobody_took_over() {
        let t = task(Status::Review, "reviewing");
        let s = of(&t, &TaskMeta::default(), &crew()).unwrap();
        assert_eq!((s.role.as_str(), s.program.as_str(), s.model.as_str(), s.took_over), ("reviewer", "claude", "claude-opus-5-5", false));
    }

    #[test]
    fn the_tasks_own_reviewer_beats_the_inherited_one() {
        let m = TaskMeta { own_reviewer: "pi/kimi-coding/k3.1-pro".into(), ..TaskMeta::default() };
        let s = of(&task(Status::Review, "reviewing"), &m, &crew()).unwrap();
        assert_eq!((s.program.as_str(), s.model.as_str()), ("pi", "kimi-coding/k3.1-pro"));
    }

    #[test]
    fn the_fallback_that_took_the_review_over_is_the_reviewer() {
        let m = TaskMeta { own_reviewer: "pi/kimi-coding/k3.1-pro".into(), reviewer_on: "codex".into(), ..TaskMeta::default() };
        let s = of(&task(Status::Review, "reviewing"), &m, &crew()).unwrap();
        assert_eq!((s.program.as_str(), s.model.as_str(), s.took_over), ("codex", "", true));
        let m = TaskMeta { reviewer_on: "codex/gpt-6-luna".into(), ..TaskMeta::default() };
        let s = of(&task(Status::Review, "reviewing"), &m, &crew()).unwrap();
        assert_eq!((s.program.as_str(), s.model.as_str()), ("codex", "gpt-6-luna"));
    }

    #[test]
    fn the_program_that_really_reviews_wins_over_a_setting_that_is_not_installed() {
        // codex is set and not installed here: the board reviews with the working agent's program, on its default model
        let m = TaskMeta { own_reviewer: "codex/gpt-6-luna".into(), reviewer_kind: "claude".into(), ..TaskMeta::default() };
        let s = of(&task(Status::Review, "reviewing"), &m, &crew()).unwrap();
        assert_eq!((s.program.as_str(), s.model.as_str()), ("claude", ""));
        // the program is the configured one: its model stays
        let m = TaskMeta { reviewer_kind: "claude".into(), ..TaskMeta::default() };
        let s = of(&task(Status::Review, "reviewing"), &m, &crew()).unwrap();
        assert_eq!((s.program.as_str(), s.model.as_str()), ("claude", "claude-opus-5-5"));
    }

    #[test]
    fn a_reviewer_the_human_is_needed_by_keeps_its_seat() {
        let s = of(&task(Status::Waiting, "reviewneeds"), &TaskMeta::default(), &crew()).unwrap();
        assert_eq!(s.role, "reviewer");
    }

    #[test]
    fn nobody_is_reviewing_once_the_review_is_over() {
        for status in [Status::Ready, Status::Waiting, Status::Done, Status::Failed, Status::Queued] {
            assert_eq!(of(&task(status, "approval"), &TaskMeta::default(), &crew()), None, "{status:?}");
        }
        let archived = Task { archived: true, ..task(Status::Review, "reviewing") };
        assert_eq!(of(&archived, &TaskMeta::default(), &crew()), None);
    }

    #[test]
    fn a_reviewer_set_to_none_is_a_seat_with_no_name() {
        let crew = Crew { reviewer: "none".into(), worker: String::new() };
        let s = of(&task(Status::Review, "reviewing"), &TaskMeta::default(), &crew).unwrap();
        assert_eq!((s.program.as_str(), s.model.as_str()), ("", ""));
    }

    #[test]
    fn the_worker_is_the_program_on_the_task_with_the_model_the_setting_gives() {
        let m = TaskMeta { agent: "omp".into(), worker_agent: "omp/deepseek/deepseek-v4-flash".into(), ..TaskMeta::default() };
        let t = Task { app: "omp".into(), ..task(Status::Working, "working") };
        let s = of(&t, &m, &crew()).unwrap();
        assert_eq!((s.role.as_str(), s.program.as_str(), s.model.as_str(), s.took_over), ("worker", "omp", "deepseek/deepseek-v4-flash", false));
        // the repo's worker.agent names another program than the one on the task: the program wins, no model is guessed
        let m = TaskMeta { agent: "codex".into(), ..TaskMeta::default() };
        let s = of(&task(Status::Working, "working"), &m, &crew()).unwrap();
        assert_eq!((s.program.as_str(), s.model.as_str()), ("codex", ""));
    }

    #[test]
    fn a_worker_that_took_over_is_named_by_its_own_record() {
        let m = TaskMeta { agent: "codex".into(), on: "codex/gpt-6-luna".into(), ..TaskMeta::default() };
        let s = of(&task(Status::Working, "working"), &m, &crew()).unwrap();
        assert_eq!((s.program.as_str(), s.model.as_str(), s.took_over), ("codex", "gpt-6-luna", true));
    }

    #[test]
    fn a_worker_that_has_left_has_no_seat() {
        let t = Task { exited_at: 5, ..task(Status::Working, "working") };
        assert_eq!(of(&t, &TaskMeta::default(), &crew()), None);
        let t = Task { finished_ms: 5, ..task(Status::Working, "working") };
        assert_eq!(of(&t, &TaskMeta::default(), &crew()), None);
    }
}
