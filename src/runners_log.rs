//! The journal of one shipyard (PLN-47): who did what and when — a container stopped, restarted or removed, a
//! cleaning (manual, disk, automatic), the agents' rule block connected or taken out, the yard itself taken out of the
//! list, and every period it did not answer.
//!
//! The rows live in the planner's database, newest first, and are kept [`KEEP_DAYS`] days; an open downtime row is not
//! pruned while the yard is silent (it is the current silence, not history). Nothing here is a sentence: `kind` and the
//! numbers in `data` are read by the page, so both languages can say the same thing.
//!
//! Who acts is never guessed from a container's name. An HTTP action comes from the human at the page; the auto-clean
//! says so itself. A container action also keeps the task the container served (`task`, `tag`), which is what an audit
//! asks about; the scope the human pressed from is not a fact about the yard and is not stored.

use crate::db::Db;
use crate::runners::{Container, RunnerState};
use crate::ssh::Why;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The journal is kept this long, then forgotten.
pub const KEEP_DAYS: u64 = 90;
pub const KEEP_MS: u64 = KEEP_DAYS * 24 * 3600 * 1000;
/// The page shows this many rows unless it asks for more.
pub const LIMIT_DEFAULT: usize = 100;
pub const LIMIT_MAX: usize = 500;
/// How much of a failure's words the journal keeps.
const WHY_CHARS: usize = 400;

/* the kinds the page filters by; a new one must be added here, in the page's list and in both dictionaries */
pub const STOP: &str = "stop";
pub const RESTART: &str = "restart";
pub const DELETE: &str = "delete";
pub const CLEAN: &str = "clean";
pub const DISK: &str = "disk";
pub const AUTO: &str = "auto";
pub const RULES: &str = "rules";
pub const DOWN: &str = "down";
pub const REMOVE: &str = "remove";
pub const KINDS: &[&str] = &[STOP, RESTART, DELETE, CLEAN, DISK, AUTO, RULES, DOWN, REMOVE];

/// The actor of a row: the human at the page, the auto-clean working by itself, or the planner noticing a silence.
pub const HUMAN: &str = "human";
pub const AUTO_ACTOR: &str = "auto";
pub const PLANNER: &str = "planner";

pub fn valid_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

pub fn valid_actor(who: &str) -> bool {
    who.is_empty() || who == HUMAN || who == AUTO_ACTOR || who == PLANNER
}

/// The searchable text of a row: the lower-cased name, tag, task, reason and the scalar values of `data` (arrays and
/// nested objects too), never the JSON keys. `to_lowercase` is Unicode, so «Верфь» and «верфь» are one word here.
pub fn hay_of(name: &str, tag: &str, task: &str, why: &str, data: &Value) -> String {
    let mut parts: Vec<String> = vec![name.to_string(), tag.to_string(), task.to_string(), why.to_string()];
    collect_strings(data, &mut parts);
    parts.join("\n").to_lowercase()
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Number(n) => out.push(n.to_string()),
        Value::Bool(b) => out.push(b.to_string()),
        Value::Array(items) => {
            for item in items {
                collect_strings(item, out);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_strings(item, out);
            }
        }
        Value::Null => {}
    }
}

/// One row as the page reads it. `n` is the stable order (newest first); `until` is set for a downtime row only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub n: u64,
    pub at: u64,
    pub kind: String,
    pub who: String,
    pub task: String,
    pub tag: String,
    pub name: String,
    pub ok: bool,
    pub why: String,
    pub until: u64,
    /// The numbers and names of this kind: counts of a cleaning, `action` of the auto-clean switch, nothing for a
    /// single container action.
    pub data: Value,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            n: 0,
            at: 0,
            kind: String::new(),
            who: String::new(),
            task: String::new(),
            tag: String::new(),
            name: String::new(),
            ok: false,
            why: String::new(),
            until: 0,
            data: json!({}),
        }
    }
}

/// A container stopped, restarted or removed (`act` is the action's own word, the same the API takes).
pub fn action(at: u64, act: &str, c: &Container, ok: bool, why: &str) -> Entry {
    let (task, tag) = match &c.task {
        Some(t) => (t.id.clone(), t.tag.clone()),
        None => (String::new(), String::new()),
    };
    Entry {
        at,
        kind: act.to_string(),
        who: HUMAN.to_string(),
        task,
        tag,
        name: c.name.clone(),
        ok,
        why: why.to_string(),
        ..Entry::default()
    }
}

/// The manual «убрать забытое»: how many units went, what they were, and what could not be removed.
pub fn cleaned(
    at: u64,
    containers: usize,
    networks: usize,
    volumes: usize,
    bytes: u64,
    names: &[String],
    tags: &[String],
    failed: usize,
    why: &str,
) -> Entry {
    Entry {
        at,
        kind: CLEAN.to_string(),
        who: HUMAN.to_string(),
        ok: failed == 0,
        why: why.to_string(),
        data: json!({
            "containers": containers,
            "networks": networks,
            "volumes": volumes,
            "bytes": bytes,
            "names": names,
            "tags": tags,
            "failed": failed,
        }),
        ..Entry::default()
    }
}

/// The disk card's cleaning; `data` carries the parts the human ticked (counts and flags) and the bytes that went.
pub fn disk(at: u64, data: Value, ok: bool, why: &str) -> Entry {
    Entry {
        at,
        kind: DISK.to_string(),
        who: HUMAN.to_string(),
        ok,
        why: why.to_string(),
        data,
        ..Entry::default()
    }
}

/// One pass of the auto-clean: what it removed and which tasks the units belonged to.
pub fn auto_run(at: u64, containers: usize, networks: usize, volumes: usize, tags: &[String], failed: usize, why: &str) -> Entry {
    Entry {
        at,
        kind: AUTO.to_string(),
        who: AUTO_ACTOR.to_string(),
        ok: failed == 0,
        why: why.to_string(),
        data: json!({
            "action": "run",
            "containers": containers,
            "networks": networks,
            "volumes": volumes,
            "tags": tags,
            "failed": failed,
        }),
        ..Entry::default()
    }
}

/// The auto-clean switch and its time, changed by the human.
pub fn auto_switch(at: u64, on: bool, hours: u64) -> Entry {
    Entry {
        at,
        kind: AUTO.to_string(),
        who: HUMAN.to_string(),
        ok: true,
        data: json!({"action": "switch", "on": on, "hours": hours}),
        ..Entry::default()
    }
}

/// The agents' rule block connected to (or taken out of) this machine's instruction settings. A write that failed
/// after the first setting is a row too (`ok` false, `settings` what was written): the page never says «connected»
/// when the two instruction settings do not both hold the block.
pub fn rules(at: u64, on: bool, drop_old: bool, settings: usize, ok: bool, why: &str) -> Entry {
    Entry {
        at,
        kind: RULES.to_string(),
        who: HUMAN.to_string(),
        ok,
        why: why.to_string(),
        data: json!({"on": on, "drop_old": drop_old, "settings": settings}),
        ..Entry::default()
    }
}

/// The yard taken out of `runners.json`; its containers stay on the machine.
pub fn removed(at: u64, force: bool, warned: &[String]) -> Entry {
    Entry {
        at,
        kind: REMOVE.to_string(),
        who: HUMAN.to_string(),
        ok: true,
        data: json!({"force": force, "warned": warned}),
        ..Entry::default()
    }
}

/// The words of a failed read, short enough for a row.
fn reason(why: Option<&Why>) -> String {
    let Some(w) = why else { return String::new() };
    let mut parts = vec![w.line.clone()];
    for d in &w.detail {
        let d = d.trim();
        if !d.is_empty() {
            parts.push(d.to_string());
        }
    }
    let text = parts.join("; ");
    if text.chars().count() > WHY_CHARS {
        let head: String = text.chars().take(WHY_CHARS).collect();
        format!("{head}…")
    } else {
        text
    }
}

/// Keeps the downtime history from the reads: a yard that stopped answering opens one row, and the read that finds it
/// again closes that row. Called after every read that reaches the database (the poll, «Обновить», a re-read after an
/// action); repeated failures of one silence do not add rows.
pub fn note_read(db: &Db, runner: &str, prev: Option<&RunnerState>, state: &RunnerState, at_ms: u64) {
    if !state.ok {
        if state.down_since_ms > 0 {
            let e = Entry {
                at: state.down_since_ms,
                kind: DOWN.to_string(),
                who: PLANNER.to_string(),
                ok: false,
                why: reason(state.why.as_ref()),
                ..Entry::default()
            };
            db.log_down(runner, &e);
        }
        return;
    }
    // It answers now: close the silence that was open (nothing is inserted when there was none). `prev` is the read
    // before this one; when the app starts and the yard is already back, the open row in the database is closed too.
    let since = prev
        .filter(|p| p.down_since_ms > 0)
        .map(|p| p.down_since_ms)
        .unwrap_or(0);
    let why = prev
        .filter(|p| p.down_since_ms > 0)
        .map(|p| reason(p.why.as_ref()))
        .unwrap_or_default();
    db.log_up(runner, since, at_ms, &why);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::TaskRef;
    use crate::ssh::Why;

    fn at(ms: u64) -> Entry {
        Entry { at: ms, ..Entry::default() }
    }

    fn yard(ok: bool, down_since: u64, why: Option<Why>) -> RunnerState {
        RunnerState {
            id: "vm-r".into(),
            ok,
            down_since_ms: down_since,
            why,
            fetched_at_ms: down_since.max(1),
            ..RunnerState::default()
        }
    }

    fn container(name: &str, tag: &str) -> Container {
        Container {
            name: name.into(),
            task: Some(TaskRef { id: format!("alpha/{}", tag.trim_start_matches("ALP-")), tag: tag.into(), ..TaskRef::default() }),
            ..Container::default()
        }
    }

    #[test]
    fn a_container_action_keeps_its_task_and_name() {
        let e = action(10, STOP, &container("pg-ALP-3", "ALP-3"), true, "");
        assert_eq!(e.kind, "stop");
        assert_eq!(e.who, "human");
        assert_eq!(e.name, "pg-ALP-3");
        assert_eq!(e.task, "alpha/3");
        assert_eq!(e.tag, "ALP-3");
        assert!(e.ok);
        assert_eq!(e.data, json!({}));
    }

    #[test]
    fn a_failed_action_says_why() {
        let e = action(10, DELETE, &container("web-ALP-15", "ALP-15"), false, "container is running");
        assert!(!e.ok);
        assert_eq!(e.why, "container is running");
    }

    #[test]
    fn a_downtime_period_is_one_row_from_silence_to_the_next_answer() {
        let db = Db::memory().unwrap();
        let base = crate::config::now_ms();
        let (d0, d1, up_at, d2, up2_at) = (base - 3_000, base - 2_000, base - 1_000, base, base + 1_000);
        let why = Why { line: "верфь не отвечает".into(), detail: vec!["ssh: timed out".into()] };
        let down = yard(false, d0, Some(why.clone()));
        note_read(&db, "vm-r", None, &down, d0);
        let rows = db.log_page("vm-r", "", "", "", 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, DOWN);
        assert_eq!(rows[0].at, d0);
        assert_eq!(rows[0].until, 0);
        assert_eq!(rows[0].why, "верфь не отвечает; ssh: timed out");

        // The same silence, read again: no second row.
        note_read(&db, "vm-r", Some(&down), &down, d1);
        assert_eq!(db.log_page("vm-r", "", "", "", 0, 10).len(), 1);

        // It answers: the row closes where it ends.
        let up = yard(true, 0, None);
        note_read(&db, "vm-r", Some(&down), &up, up_at);
        let rows = db.log_page("vm-r", "", "", "", 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, DOWN);
        assert_eq!(rows[0].until, up_at);

        // A later silence opens a row of its own.
        let down2 = yard(false, d2, Some(Why { line: "нет ответа".into(), detail: vec![] }));
        note_read(&db, "vm-r", Some(&up), &down2, d2);
        note_read(&db, "vm-r", Some(&down2), &yard(true, 0, None), up2_at);
        let rows = db.log_page("vm-r", "", "", "", 0, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].at, d2);
        assert_eq!(rows[0].until, up2_at);
        assert_eq!(rows[1].until, up_at);
    }

    #[test]
    fn a_recovery_after_the_app_start_closes_the_open_row() {
        let db = Db::memory().unwrap();
        let base = crate::config::now_ms();
        let down = yard(false, base - 5_000, Some(Why { line: "молчит".into(), detail: vec![] }));
        note_read(&db, "vm-r", None, &down, base - 5_000);
        // No previous state in memory (a fresh start): the open row is still closed.
        note_read(&db, "vm-r", None, &yard(true, 0, None), base);
        let rows = db.log_page("vm-r", "", "", "", 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].until, base);
    }

    #[test]
    fn the_journal_is_filtered_by_kind_actor_and_words() {
        let db = Db::memory().unwrap();
        let base = crate::config::now_ms();
        db.log_add("vm-r", &action(base - 50, STOP, &container("pg-ALP-3", "ALP-3"), true, ""));
        db.log_add("vm-r", &action(base - 40, DELETE, &container("web-ALP-15", "ALP-15"), false, "already running"));
        db.log_add("vm-r", &auto_run(base - 30, 2, 1, 0, &["ALP-15".into()], 0, ""));
        db.log_add("vm-r", &rules(base - 20, true, false, 2, true, ""));
        db.log_add("vm-r", &cleaned(base - 10, 1, 0, 1, 1024, &["web-ALP-15".into()], &["ALP-15".into()], 0, ""));

        assert_eq!(db.log_page("vm-r", "", "", "", 0, 10).len(), 5);
        assert_eq!(db.log_page("vm-r", "delete", "", "", 0, 10).len(), 1);
        assert_eq!(db.log_page("vm-r", "stop,delete", "", "", 0, 10).len(), 2);
        assert_eq!(db.log_page("vm-r", "", "auto", "", 0, 10).len(), 1);
        assert_eq!(db.log_page("vm-r", "", "", "ALP-15", 0, 10).len(), 3);
        assert_eq!(db.log_page("vm-r", "", "", "already", 0, 10).len(), 1);
        assert_eq!(db.log_page("vm-r", "", "", "", 0, 2).len(), 2);
        // Newest first, and a page continues where the last one stopped.
        let page = db.log_page("vm-r", "", "", "", 0, 2);
        assert_eq!(page[0].at, base - 10);
        assert_eq!(page[1].at, base - 20);
        let next = db.log_page("vm-r", "", "", "", page[1].n, 2);
        assert_eq!(next[0].at, base - 30);
        // Another yard's rows never leak.
        assert!(db.log_page("vm-x", "", "", "", 0, 10).is_empty());
    }

    #[test]
    fn retention_forgets_old_rows_but_keeps_the_current_silence() {
        let db = Db::memory().unwrap();
        let base = crate::config::now_ms();
        let old = base - KEEP_MS - 1_000;
        let fresh = base;
        db.log_add("vm-r", &action(old, STOP, &container("pg-ALP-3", "ALP-3"), true, ""));
        db.log_down("vm-r", &at(old));
        db.log_prune(fresh.saturating_sub(KEEP_MS));
        let rows = db.log_page("vm-r", "", "", "", 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, DOWN);
        assert_eq!(rows[0].at, old);

        // Once closed (and its end is old too), the silence is history.
        db.log_up("vm-r", old, fresh, "почему-то");
        db.log_prune(fresh + 1);
        assert!(db.log_page("vm-r", "", "", "", 0, 10).is_empty());
    }

    #[test]
    fn the_counts_of_a_cleaning_are_numbers_not_words() {
        let e = cleaned(10, 3, 1, 2, 4_096, &["a".into()], &["ALP-15".into()], 1, "volume is busy");
        assert_eq!(e.kind, CLEAN);
        assert!(!e.ok);
        assert_eq!(e.data["containers"], 3);
        assert_eq!(e.data["networks"], 1);
        assert_eq!(e.data["volumes"], 2);
        assert_eq!(e.data["bytes"], 4_096);
        assert_eq!(e.data["names"][0], "a");
        assert_eq!(e.data["failed"], 1);
        assert_eq!(e.why, "volume is busy");
    }

    #[test]
    fn the_auto_switch_keeps_its_action_and_time() {
        let e = auto_switch(10, true, 6);
        assert_eq!(e.kind, AUTO);
        assert_eq!(e.who, HUMAN);
        assert_eq!(e.data["action"], "switch");
        assert_eq!(e.data["on"], true);
        assert_eq!(e.data["hours"], 6);
        let run = auto_run(20, 1, 0, 2, &["ALP-15".into()], 0, "");
        assert_eq!(run.data["action"], "run");
        assert_eq!(run.who, AUTO_ACTOR);
    }

    #[test]
    fn the_search_lower_cases_any_script_and_takes_wildcards_literally() {
        let db = Db::memory().unwrap();
        let base = crate::config::now_ms();
        db.log_add(
            "vm-r",
            &Entry {
                at: base - 30,
                kind: DOWN.into(),
                who: PLANNER.into(),
                why: "Верфь не отвечает".into(),
                ..Entry::default()
            },
        );
        db.log_add("vm-r", &auto_run(base - 20, 2, 1, 0, &["Том".into()], 0, ""));
        db.log_add("vm-r", &rules(base - 10, true, false, 2, true, ""));

        assert_eq!(db.log_page("vm-r", "", "", "верфь", 0, 10).len(), 1);
        assert_eq!(db.log_page("vm-r", "", "", "ВЕРФЬ", 0, 10).len(), 1);
        assert_eq!(db.log_page("vm-r", "", "", "том", 0, 10).len(), 1);
        assert_eq!(db.log_page("vm-r", "", "", "ТОМ", 0, 10).len(), 1);
        assert_eq!(db.log_page("vm-r", "", "", "alp-15", 0, 10).len(), 0);
        // Wildcards are words, not patterns; keys of `data` are not searchable text.
        assert!(db.log_page("vm-r", "", "", "_", 0, 10).is_empty());
        assert!(db.log_page("vm-r", "", "", "%", 0, 10).is_empty());
        assert!(db.log_page("vm-r", "", "", "settings", 0, 10).is_empty());
        assert!(db.log_page("vm-r", "", "", "containers", 0, 10).is_empty());
    }

    #[test]
    fn the_cursor_reaches_rows_past_the_page_limit() {
        let db = Db::memory().unwrap();
        let base = crate::config::now_ms();
        for i in 0..520 {
            db.log_add("vm-r", &action(base - i, STOP, &container(&format!("c-{i}"), "ALP-3"), true, ""));
        }
        let first = db.log_page("vm-r", "", "", "", 0, 500);
        assert_eq!(first.len(), 500);
        assert_eq!(first[499].name, "c-20");
        let next = db.log_page("vm-r", "", "", "", first[499].n, 500);
        assert_eq!(next.len(), 20);
        assert_eq!(next[0].name, "c-19");
        assert_eq!(next[19].name, "c-0");
    }

    #[test]
    fn a_rule_block_that_failed_after_one_setting_is_not_called_connected() {
        let e = rules(10, true, false, 1, false, "доска не приняла второй параметр");
        assert_eq!(e.kind, RULES);
        assert!(!e.ok);
        assert_eq!(e.data["settings"], 1);
        assert_eq!(e.why, "доска не приняла второй параметр");
    }

    #[test]
    fn the_kinds_and_actors_the_page_may_ask_for() {
        for k in KINDS {
            assert!(valid_kind(k), "{k}");
        }
        assert!(!valid_kind("kill"));
        assert!(valid_actor(""));
        assert!(valid_actor(HUMAN));
        assert!(valid_actor(AUTO_ACTOR));
        assert!(valid_actor(PLANNER));
        assert!(!valid_actor("robot"));
    }
}
