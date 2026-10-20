//! The Coordinator's conversation, read from its own transcript.
//!
//! The board has no "chat history" command, but the Coordinator is a Claude
//! session started in `<data>/coordinator/default/chat`, and Claude keeps every
//! turn of it in `<claude>/projects/<folder>/<session>.jsonl`. The planner reads
//! that file (read-only) and cuts it into turns: what the human wrote and the
//! words the Coordinator answered with. Tool calls and thinking are left out.
//!
//! Other programs (omp, pi, …) keep no such file here; then there is no history
//! and the page keeps showing the tail of the Coordinator's window.

use crate::config::{home, AppConfig};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

const MAX_QUESTION: usize = 4000;
const MAX_ANSWER: usize = 8000;
/// Board notices the Coordinator is told by itself, not typed by the human.
const BOARD_PREFIX: &str = "[Уведомление доски]";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Turn {
    /// Position in the whole conversation, oldest first; the page pages by it.
    pub n: usize,
    /// Milliseconds since the epoch; 0 when the record has no time.
    pub at: u64,
    /// `human` or `board` (a notice the board sent to the Coordinator).
    pub from: &'static str,
    pub q: String,
    /// The Coordinator's last words of the turn; empty while it works.
    pub a: String,
}

#[derive(Debug, Serialize)]
pub struct Page {
    pub available: bool,
    pub turns: Vec<Turn>,
    /// There are older turns than the ones returned.
    pub more: bool,
    pub total: usize,
}

/// Turns of one transcript text. `n` is filled in by the caller.
pub fn parse(text: &str) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    // The Coordinator is in the middle of a run: it began with a message and has not
    // ended its turn yet. What is queued meanwhile reaches the same run.
    let mut running = false;
    for line in text.lines() {
        // Skip the big records (requests, snapshots) without parsing them.
        let wanted = line.contains("\"type\":\"user\"")
            || line.contains("\"type\":\"assistant\"")
            || line.contains("\"queued_command\"");
        if !wanted {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let at = v
            .get("timestamp")
            .and_then(Value::as_str)
            .map(parse_time)
            .unwrap_or(0);
        match v.get("type").and_then(Value::as_str) {
            Some("user") => {
                if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                if let Some(q) = human_text(v.pointer("/message/content")) {
                    open_turn(&mut turns, q, at, false, &mut running);
                }
            }
            // A message sent while the Coordinator was busy waits in a queue and
            // reaches the session as this attachment, with no `user` record.
            Some("attachment") => {
                let a = v.get("attachment");
                if a.and_then(|a| a.get("type")).and_then(Value::as_str) == Some("queued_command") {
                    if let Some(q) = human_text(a.and_then(|a| a.get("prompt"))) {
                        open_turn(&mut turns, q, at, true, &mut running);
                    }
                }
            }
            Some("assistant") => {
                let Some(turn) = turns.last_mut() else {
                    continue;
                };
                let stop = v.pointer("/message/stop_reason").and_then(Value::as_str);
                if let Some(Value::Array(blocks)) = v.pointer("/message/content") {
                    let calls = blocks
                        .iter()
                        .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"));
                    if stop == Some("end_turn") || (stop.is_none() && !calls) {
                        running = false;
                    }
                    for b in blocks {
                        if b.get("type").and_then(Value::as_str) == Some("text") {
                            let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                            if !t.is_empty() {
                                turn.a = clip(t, MAX_ANSWER);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    turns
}

/// A message that reaches a Coordinator in the middle of a run is queued and read
/// together with the one that started it; one final answer covers them all. So
/// a notice is dropped and a human's words are added to the turn of the run.
fn open_turn(turns: &mut Vec<Turn>, q: String, at: u64, queued: bool, running: &mut bool) {
    let from = if q.starts_with(BOARD_PREFIX) {
        "board"
    } else {
        "human"
    };
    if queued && *running {
        if from == "human" {
            if let Some(turn) = turns.last_mut() {
                turn.from = "human";
                turn.q = clip(&format!("{}\n{}", turn.q, q), MAX_QUESTION);
            }
        }
        return;
    }
    *running = true;
    turns.push(Turn {
        n: 0,
        at,
        from,
        q: clip(&q, MAX_QUESTION),
        a: String::new(),
    });
}

/// The words of a human message: a plain string, or the text parts of a list.
/// `None` for tool results and for the harness's own wrappers.
fn human_text(content: Option<&Value>) -> Option<String> {
    let text = match content? {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') {
        return None;
    }
    Some(text.to_string())
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// `2026-10-19T05:56:05.628Z` → milliseconds since the epoch.
fn parse_time(s: &str) -> u64 {
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        num(0, 4),
        num(5, 7),
        num(8, 10),
        num(11, 13),
        num(14, 16),
        num(17, 19),
    ) else {
        return 0;
    };
    let ms = s
        .get(20..23)
        .and_then(|x| x.parse::<i64>().ok())
        .unwrap_or(0);
    // Days from civil (Howard Hinnant).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + se;
    (secs * 1000 + ms).max(0) as u64
}

/// Claude names a project folder after the directory: every character that is
/// not a letter or a digit becomes `-`.
fn project_folder(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn claude_home() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude"))
}

/// The Coordinator's transcripts, oldest first (a new session starts a new
/// file); empty when it is not a Claude session or has none.
fn transcripts(cfg: &AppConfig) -> Vec<PathBuf> {
    let coordinator = cfg.data_dir.join("coordinator").join("default");
    if let Some(kind) = std::fs::read_to_string(coordinator.join("state.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.pointer("/run/kind").and_then(Value::as_str).map(String::from))
    {
        if kind != "claude" {
            return Vec::new();
        }
    }
    let cwd = coordinator.join("chat");
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let dir = claude_home().join("projects").join(project_folder(&cwd));
    let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort();
    files.into_iter().map(|(_, p)| p).collect()
}

type Cached = (SystemTime, u64, Vec<Turn>);

fn cache() -> &'static Mutex<HashMap<PathBuf, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One transcript's turns, parsed again only when the file changed.
fn turns_of(path: &Path) -> Vec<Turn> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
    if let Some((m, l, turns)) = cache().lock().ok().and_then(|c| c.get(path).cloned()) {
        if (m, l) == stamp {
            return turns;
        }
    }
    let turns = std::fs::read_to_string(path)
        .map(|t| parse(&t))
        .unwrap_or_default();
    if let Ok(mut c) = cache().lock() {
        c.insert(path.to_path_buf(), (stamp.0, stamp.1, turns.clone()));
    }
    turns
}

/// A page of the conversation: the `limit` turns before turn `before` (the end
/// when `None`), oldest first.
pub fn page(cfg: &AppConfig, before: Option<usize>, limit: usize) -> Page {
    let files = transcripts(cfg);
    let mut all: Vec<Turn> = files.iter().flat_map(|p| turns_of(p)).collect();
    for (i, t) in all.iter_mut().enumerate() {
        t.n = i;
    }
    let total = all.len();
    let end = before.unwrap_or(total).min(total);
    let start = end.saturating_sub(limit.clamp(1, 50));
    Page {
        available: !files.is_empty(),
        turns: all[start..end].to_vec(),
        more: start > 0,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        r#"{"type":"user","timestamp":"2026-10-19T05:56:05.628Z","message":{"role":"user","content":"удали все задачи"}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-10-19T05:56:09.000Z","message":{"content":[{"type":"thinking","thinking":""},{"type":"tool_use","name":"Bash","input":{}}]}}"#,
        "\n",
        r#"{"type":"user","timestamp":"2026-10-19T05:56:09.800Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-10-19T05:56:16.998Z","message":{"content":[{"type":"text","text":"Работаю."},{"type":"text","text":"Удалил шесть задач."}]}}"#,
        "\n",
        r#"{"type":"api-request","timestamp":"2026-10-19T05:56:17.000Z","user":"ignored \"type\":\"user\""}"#,
        "\n",
        r#"{"type":"attachment","timestamp":"2026-10-19T06:07:29.959Z","attachment":{"type":"queued_command","prompt":[{"type":"text","text":"[Image #2] переделай"},{"type":"image","source":{}}]}}"#,
        "\n",
        r#"{"type":"attachment","timestamp":"2026-10-19T06:07:39.914Z","attachment":{"type":"queued_command","prompt":"[Уведомление доски] apicache/2 ждёт тебя"}}"#,
        "\n",
        r#"{"type":"user","isMeta":true,"timestamp":"2026-10-19T06:08:00.000Z","message":{"content":"<system-reminder>x</system-reminder>"}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-10-19T06:08:36.000Z","message":{"content":[{"type":"text","text":"Принято."}]}}"#,
        "\n",
        r#"{"type":"user","timestamp":"2026-10-19T06:09:00.000Z","message":{"content":"а теперь?"}}"#,
        "\n",
    );

    #[test]
    fn a_notice_in_the_middle_of_a_run_keeps_the_answer_with_the_question() {
        let text = [
            r#"{"type":"user","timestamp":"2026-10-19T09:07:17.000Z","message":{"content":"сливай"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:07:23.000Z","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            r#"{"type":"attachment","timestamp":"2026-10-19T09:07:52.000Z","attachment":{"type":"queued_command","prompt":"[Уведомление доски] apicache/4 ждёт"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:08:20.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Слил APC-4."}]}}"#,
            r#"{"type":"attachment","timestamp":"2026-10-19T09:09:00.000Z","attachment":{"type":"queued_command","prompt":"[Уведомление доски] apicache/5 ждёт"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:09:10.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"APC-5 готова."}]}}"#,
        ]
        .join("\n");
        let t = parse(&text);
        assert_eq!(t.len(), 2, "the notice inside the run is not a turn of its own");
        assert_eq!((t[0].from, t[0].q.as_str(), t[0].a.as_str()), ("human", "сливай", "Слил APC-4."));
        assert_eq!((t[1].from, t[1].a.as_str()), ("board", "APC-5 готова."), "a notice at rest is a turn");
    }

    #[test]
    fn cuts_the_transcript_into_turns() {
        let t = parse(SAMPLE);
        assert_eq!(t.len(), 3);
        assert_eq!((t[0].from, t[0].q.as_str()), ("human", "удали все задачи"));
        assert_eq!(t[0].a, "Удалил шесть задач.", "the last words are the answer");
        assert_eq!(t[1].q, "[Image #2] переделай");
        assert_eq!(
            t[1].a, "Принято.",
            "the notice queued behind it joined the run, the answer is the human's"
        );
        assert_eq!((t[2].q.as_str(), t[2].a.as_str()), ("а теперь?", ""));
    }

    #[test]
    fn a_human_word_queued_in_a_run_joins_its_turn() {
        let text = [
            r#"{"type":"user","timestamp":"2026-10-19T09:00:00.000Z","message":{"content":"давай 1"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:00:05.000Z","message":{"stop_reason":"tool_use","content":[{"type":"text","text":"Now the next one."},{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            r#"{"type":"attachment","timestamp":"2026-10-19T09:00:10.000Z","attachment":{"type":"queued_command","prompt":"ещё и таймаут 600"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:00:30.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Готово обе."}]}}"#,
        ]
        .join("\n");
        let t = parse(&text);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].q, "давай 1\nещё и таймаут 600");
        assert_eq!(t[0].a, "Готово обе.", "the final answer, not the line between the calls");
    }

    #[test]
    fn reads_the_time() {
        assert_eq!(parse_time("1970-01-01T00:00:01.500Z"), 1500);
        assert_eq!(parse_time("2026-10-19T05:56:05.628Z"), 1_792_389_365_628);
        assert_eq!(parse_time("junk"), 0);
    }

    #[test]
    fn names_the_project_folder_like_claude() {
        assert_eq!(
            project_folder(Path::new(
                "/home/me/.herdr-projects/data/coordinator/default/chat"
            )),
            "-home-me--herdr-projects-data-coordinator-default-chat"
        );
    }
}
