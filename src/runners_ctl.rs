//! One container of a task on a shipyard: its log tail and «Остановить» / «Перезапустить» / «Удалить».
//!
//! Only the containers of the board's tasks are touched, and only by their full id. A foreign container (no
//! `herdr.task` label, or under a protection mask) is refused for everything; one whose task is unclear is refused for
//! the three actions; one of a task that is still on the board answers only to the Focus of that very task (the page
//! sends the task it acts from), so a neighbour's task in work is never stopped from another task's screen. Every
//! action is shown as «было → стало» first and runs only after the second press.

use crate::runners::{Class, Container};
use serde::Serialize;
use std::time::Instant;

/// The log tail the page may ask for, in lines.
pub const TAIL_MIN: u32 = 20;
pub const TAIL_MAX: u32 = 2000;
pub const TAIL_DEFAULT: u32 = 300;
/// The text handed to the page is cut to its last part when a tail is longer than this.
pub const LOG_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Act {
    Stop,
    Restart,
    Delete,
}

impl Act {
    pub fn parse(s: &str) -> Option<Act> {
        match s {
            "stop" => Some(Act::Stop),
            "restart" => Some(Act::Restart),
            "delete" => Some(Act::Delete),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Act::Stop => "stop",
            Act::Restart => "restart",
            Act::Delete => "delete",
        }
    }

    /// The command for one full id (the caller has checked it with `safe_id`). `rm` has no `-f`: a running
    /// container is stopped first, by its own button.
    pub fn command(self, id: &str) -> String {
        match self {
            Act::Stop => format!("docker stop {id}"),
            Act::Restart => format!("docker restart {id}"),
            Act::Delete => format!("docker rm -v {id}"),
        }
    }
}

pub fn tail_of(asked: Option<u32>) -> u32 {
    asked.unwrap_or(TAIL_DEFAULT).clamp(TAIL_MIN, TAIL_MAX)
}

/// `docker logs` for one full id; stderr of the container is merged into the text.
pub fn logs_command(id: &str, tail: u32) -> String {
    format!("docker logs --tail {tail} {id} 2>&1")
}

fn running(state: &str) -> bool {
    matches!(state, "running" | "restarting" | "paused")
}

/// Who may be read: every container of a board task (also a forgotten one), never a foreign one or a shared cache.
pub fn logs_gate(c: &Container) -> Result<(), String> {
    if c.class == Class::Foreign {
        return Err("чужой контейнер (не задачи доски или под маской защиты): его логи не читаются".into());
    }
    if c.class == Class::Shared {
        return Err("общий кэш сборок верфи (herdr.shared): им не управляют с доски".into());
    }
    Ok(())
}

/// Who may be stopped, restarted or removed from the screen of `scope` (the task id, `""` from the shipyard's own).
pub fn gate(c: &Container, act: Act, scope: &str) -> Result<(), String> {
    match c.class {
        Class::Foreign => {
            return Err("чужой контейнер (не задачи доски или под маской защиты): его не трогаю".into());
        }
        Class::Unsure => {
            return Err("неясно, какой задаче принадлежит контейнер: его не трогаю".into());
        }
        Class::Shared => {
            return Err("общий кэш сборок верфи (herdr.shared): его не трогаю".into());
        }
        Class::Active => {
            let own = c.task.as_ref().filter(|t| !scope.is_empty() && t.id == scope);
            if own.is_none() {
                let tag = c.task.as_ref().map(|t| t.tag.as_str()).unwrap_or("?");
                return Err(format!(
                    "контейнер задачи {tag}, которая в работе: им управляют только из Фокуса этой задачи"
                ));
            }
        }
        Class::Forgotten => {}
    }
    match act {
        Act::Stop if !running(&c.state) => Err("контейнер уже остановлен".into()),
        Act::Delete if running(&c.state) => Err("контейнер запущен: сначала остановите его".into()),
        _ => Ok(()),
    }
}

/// What the card shows for one container: its state now and after the action.
#[derive(Debug, Clone, Serialize)]
pub struct Side {
    pub state: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub ok: bool,
    pub runner: String,
    pub act: Act,
    pub id: String,
    pub name: String,
    pub image: String,
    pub task: String,
    pub ports: String,
    pub mounts: String,
    pub was: Side,
    pub now: Side,
    pub token: String,
    pub expires_ms: u64,
}

pub fn preview_of(runner: &str, c: &Container, act: Act, token: &str, expires_ms: u64) -> Preview {
    let now = match act {
        Act::Stop => Side { state: "exited".into(), status: String::new() },
        Act::Restart => Side { state: "running".into(), status: String::new() },
        Act::Delete => Side { state: "gone".into(), status: String::new() },
    };
    Preview {
        ok: true,
        runner: runner.to_string(),
        act,
        id: c.id.clone(),
        name: c.name.clone(),
        image: c.image.clone(),
        task: c.task.as_ref().map(|t| t.tag.clone()).unwrap_or_default(),
        ports: c.ports.clone(),
        mounts: c.mounts.clone(),
        was: Side { state: c.state.clone(), status: c.status.clone() },
        now,
        token: token.to_string(),
        expires_ms,
    }
}

/// The previewed action, kept under a token until the second press.
#[derive(Debug, Clone)]
pub struct Plan {
    pub runner: String,
    pub id: String,
    pub act: Act,
    pub scope: String,
    pub at: Instant,
}

/// The last part of a log: whole lines, at most `max` bytes. The flag says that the beginning was cut off.
pub fn tail_text(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut from = text.len() - max;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let cut = &text[from..];
    // Start at a line start when the cut fell in the middle of one.
    let cut = match text[..from].ends_with('\n') {
        true => cut,
        false => cut.split_once('\n').map(|(_, rest)| rest).unwrap_or(cut),
    };
    (cut.to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::TaskRef;

    fn c(class: Class, state: &str, task: &str) -> Container {
        Container {
            id: "a".repeat(64),
            name: "pg-X-1".into(),
            state: state.into(),
            class,
            task: (!task.is_empty()).then(|| TaskRef { id: task.into(), tag: "X-1".into(), ..TaskRef::default() }),
            ..Container::default()
        }
    }

    #[test]
    fn commands_have_exact_ids_and_no_force() {
        let id = "b".repeat(64);
        assert_eq!(Act::Stop.command(&id), format!("docker stop {id}"));
        assert_eq!(Act::Restart.command(&id), format!("docker restart {id}"));
        assert_eq!(Act::Delete.command(&id), format!("docker rm -v {id}"));
        assert!(!Act::Delete.command(&id).contains(" -f"));
        assert_eq!(logs_command(&id, 300), format!("docker logs --tail 300 {id} 2>&1"));
    }

    #[test]
    fn the_tail_is_clamped() {
        assert_eq!(tail_of(None), TAIL_DEFAULT);
        assert_eq!(tail_of(Some(1)), TAIL_MIN);
        assert_eq!(tail_of(Some(10_000_000)), TAIL_MAX);
        assert_eq!(tail_of(Some(500)), 500);
    }

    #[test]
    fn a_foreign_container_gets_nothing() {
        let f = c(Class::Foreign, "running", "");
        assert!(logs_gate(&f).is_err());
        for a in [Act::Stop, Act::Restart, Act::Delete] {
            assert!(gate(&f, a, "t1").is_err());
        }
    }

    #[test]
    fn a_shared_build_cache_is_neither_read_nor_touched() {
        let s = c(Class::Shared, "running", "");
        assert!(logs_gate(&s).is_err(), "the cache is not a task: its logs are not read from here");
        for a in [Act::Stop, Act::Restart, Act::Delete] {
            assert!(gate(&s, a, "").is_err());
        }
    }

    #[test]
    fn an_unsure_one_is_read_but_not_touched() {
        let u = c(Class::Unsure, "exited", "");
        assert!(logs_gate(&u).is_ok());
        for a in [Act::Stop, Act::Restart, Act::Delete] {
            assert!(gate(&u, a, "t1").is_err());
        }
    }

    #[test]
    fn a_task_in_work_answers_only_to_its_own_screen() {
        let a = c(Class::Active, "running", "t1");
        assert!(logs_gate(&a).is_ok());
        assert!(gate(&a, Act::Stop, "t1").is_ok());
        assert!(gate(&a, Act::Restart, "t1").is_ok());
        assert!(gate(&a, Act::Stop, "t2").is_err());
        assert!(gate(&a, Act::Restart, "").is_err());
        assert!(gate(&a, Act::Delete, "t1").is_err(), "a running container is stopped first");
        let e = c(Class::Active, "exited", "t1");
        assert!(gate(&e, Act::Delete, "t1").is_ok());
        assert!(gate(&e, Act::Delete, "t2").is_err());
    }

    #[test]
    fn a_forgotten_one_is_free_from_anywhere_but_by_its_state() {
        let r = c(Class::Forgotten, "running", "t1");
        assert!(gate(&r, Act::Stop, "").is_ok());
        assert!(gate(&r, Act::Delete, "").is_err());
        let e = c(Class::Forgotten, "exited", "t1");
        assert!(gate(&e, Act::Stop, "").is_err(), "already stopped");
        assert!(gate(&e, Act::Restart, "").is_ok());
        assert!(gate(&e, Act::Delete, "").is_ok());
    }

    #[test]
    fn the_log_is_cut_at_a_line() {
        let (t, cut) = tail_text("one\ntwo\nthree\n", 100);
        assert_eq!((t.as_str(), cut), ("one\ntwo\nthree\n", false));
        let (t, cut) = tail_text("one\ntwo\nthree\n", 10);
        assert_eq!((t.as_str(), cut), ("two\nthree\n", true));
        let (t, cut) = tail_text("one\ntwo\nthree\n", 9);
        assert_eq!((t.as_str(), cut), ("three\n", true));
        let (t, cut) = tail_text("ёёёёё\nab\n", 5);
        assert_eq!((t.as_str(), cut), ("ab\n", true));
    }

    #[test]
    fn the_preview_says_where_it_goes() {
        let a = c(Class::Forgotten, "running", "t1");
        let p = preview_of("yard", &a, Act::Stop, "tok", 5);
        assert_eq!((p.was.state.as_str(), p.now.state.as_str(), p.task.as_str()), ("running", "exited", "X-1"));
        assert_eq!(preview_of("yard", &a, Act::Delete, "tok", 5).now.state, "gone");
    }
}
