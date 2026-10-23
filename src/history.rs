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
use crate::db::{chat_row_hash, ChatHit, ChatTurnRow, Db};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// A human message may be 100 000 characters (a spec, a log): the history keeps all of it, so the
/// chat shows and reuses the words as they were sent. Answers are still bounded by `MAX_ANSWER`.
const MAX_QUESTION: usize = 100_000;
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
    /// What it is doing now (its last tool call, in a few words): only on the turn it is still in.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub doing: String,
    /// When the answer was written (the assistant record's time); 0 while there is none.
    #[serde(skip_serializing_if = "is_zero")]
    pub at_a: u64,
    /// The media types of the images in the human's message (Claude Code keeps them as blocks of the
    /// record, base64 inside, and drops the file paths the planner wrote): `image(cfg, n, i)` serves them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Where the record that opened the turn is: the transcript file and the line in it.
    #[serde(skip)]
    pub file: PathBuf,
    #[serde(skip)]
    pub line: usize,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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
    for (line_no, line) in text.lines().enumerate() {
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
                if let Some((q, images)) = human_text(v.pointer("/message/content")) {
                    open_turn(&mut turns, q, images, line_no, at, false, &mut running);
                }
            }
            // A message sent while the Coordinator was busy waits in a queue and
            // reaches the session as this attachment, with no `user` record.
            Some("attachment") => {
                let a = v.get("attachment");
                if a.and_then(|a| a.get("type")).and_then(Value::as_str) == Some("queued_command") {
                    if let Some((q, images)) = human_text(a.and_then(|a| a.get("prompt"))) {
                        open_turn(&mut turns, q, images, line_no, at, true, &mut running);
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
                        turn.doing.clear();
                    }
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                let t = b.get("text").and_then(Value::as_str).unwrap_or("").trim();
                                if !t.is_empty() {
                                    turn.a = clip(t, MAX_ANSWER);
                                    turn.at_a = at;
                                }
                            }
                            Some("tool_use") => turn.doing = doing_of(b),
                            _ => {}
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
fn open_turn(turns: &mut Vec<Turn>, q: String, images: Vec<String>, line: usize, at: u64, queued: bool, running: &mut bool) {
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
    // the turn before is over: what it was doing is no longer news
    if let Some(prev) = turns.last_mut() {
        prev.doing.clear();
    }
    turns.push(Turn {
        n: 0,
        at,
        from,
        q: clip(&q, MAX_QUESTION),
        a: String::new(),
        doing: String::new(),
        at_a: 0,
        images,
        file: PathBuf::new(),
        line,
    });
}

/// A tool call in a few words, for the «working» line: Bash by its description or the first line of its
/// command, another tool by its name and the file, pattern or prompt it was given.
fn doing_of(block: &Value) -> String {
    const MAX_DOING: usize = 80;
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    let input = block.get("input");
    let field = |k: &str| {
        input
            .and_then(|i| i.get(k))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.lines().next().unwrap_or("").to_string())
    };
    let text = if name == "Bash" {
        field("description").or_else(|| field("command")).unwrap_or_default()
    } else {
        let what = field("file_path")
            .or_else(|| field("pattern"))
            .or_else(|| field("prompt"))
            .or_else(|| field("description"))
            .or_else(|| field("command"))
            .or_else(|| field("query"))
            .or_else(|| field("url"));
        match what {
            Some(w) => format!("{name} {w}"),
            None => name.to_string(),
        }
    };
    clip(text.trim(), MAX_DOING)
}

/// The words of a human message (a plain string, or the text parts of a list) and the media types of
/// its image blocks. `None` for tool results and for the harness's own wrappers.
fn human_text(content: Option<&Value>) -> Option<(String, Vec<String>)> {
    let mut images = Vec::new();
    let text = match content? {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            for b in blocks {
                if b.get("type").and_then(Value::as_str) == Some("image") {
                    let mime = b
                        .pointer("/source/media_type")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    images.push(mime.to_string());
                }
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
    Some((text.to_string(), images))
}

/// The `i`-th image block of a transcript record (a `user` record's content or a queued command's prompt):
/// its media type and bytes.
fn image_block(record: &str, i: usize) -> Option<(String, Vec<u8>)> {
    use base64::Engine;
    let v: Value = serde_json::from_str(record).ok()?;
    let blocks = v
        .pointer("/message/content")
        .or_else(|| v.pointer("/attachment/prompt"))?
        .as_array()?;
    let block = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("image"))
        .nth(i)?;
    let mime = block
        .pointer("/source/media_type")
        .and_then(Value::as_str)
        .unwrap_or("image/png")
        .to_string();
    let data = block.pointer("/source/data").and_then(Value::as_str)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .ok()?;
    Some((mime, bytes))
}

/// The `i`-th image of turn `n` (the numbering of [`page`]), read from the transcript on demand.
pub fn image(cfg: &AppConfig, n: usize, i: usize) -> Option<(String, Vec<u8>)> {
    use std::io::BufRead;
    let files = transcripts(cfg);
    let all: Vec<Turn> = files.iter().flat_map(|p| turns_of(p)).collect();
    let turn = all.get(n)?;
    if i >= turn.images.len() {
        return None;
    }
    let f = std::fs::File::open(&turn.file).ok()?;
    let record = std::io::BufReader::new(f).lines().nth(turn.line)?.ok()?;
    image_block(&record, i)
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
pub(crate) fn project_folder(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub(crate) fn claude_home() -> PathBuf {
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
    let mut turns = std::fs::read_to_string(path)
        .map(|t| parse(&t))
        .unwrap_or_default();
    for t in &mut turns {
        t.file = path.to_path_buf();
    }
    if let Ok(mut c) = cache().lock() {
        c.insert(path.to_path_buf(), (stamp.0, stamp.1, turns.clone()));
    }
    turns
}

/// How full the Coordinator's context is: the tokens of its last turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextUse {
    pub used: u64,
    pub model: String,
}

/// The Coordinator's program (`claude`, `omp`, …) from its state; `""` when unknown.
pub fn coordinator_program(cfg: &AppConfig) -> String {
    std::fs::read_to_string(cfg.data_dir.join("coordinator/default/state.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.pointer("/run/kind").and_then(Value::as_str).map(String::from))
        .unwrap_or_default()
}

/// The folder with the Coordinator's transcripts (for the file watcher).
pub fn transcript_dir(cfg: &AppConfig) -> PathBuf {
    let cwd = cfg.data_dir.join("coordinator/default/chat");
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    claude_home().join("projects").join(project_folder(&cwd))
}

/// The last assistant turn's token count and model, read from the tail of the
/// newest transcript. `None` when there is no Claude transcript or no usage in it.
pub fn coordinator_context(cfg: &AppConfig) -> Option<ContextUse> {
    use std::io::{Read, Seek, SeekFrom};
    let path = transcripts(cfg).pop()?;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let take = len.min(512 * 1024);
    f.seek(SeekFrom::Start(len - take)).ok()?;
    let mut buf = Vec::with_capacity(take as usize);
    f.take(take).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    for line in text.lines().rev() {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message") else { continue };
        let Some(u) = msg.get("usage") else { continue };
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let used = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
        if used == 0 {
            continue;
        }
        let model = msg.get("model").and_then(Value::as_str).unwrap_or("").to_string();
        return Some(ContextUse { used, model });
    }
    None
}

/// The model's context window: 1M when the turn already used more than 200k or the
/// name says `1m`, otherwise 200k. The transcript does not carry the window itself.
pub fn context_window(used: u64, model: &str) -> u64 {
    if used > 200_000 || model.to_lowercase().contains("1m") {
        1_000_000
    } else {
        200_000
    }
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

/// Brings the index of the Coordinator's turns (in the database) up to date with the
/// transcripts. Turn `n` of the index is turn `n` of [`page`]. Turns are compared with
/// the stored ones from the start; the first that differs and all after it are
/// rewritten, so a new session, a changed last answer or a shifted numbering all
/// come out right. Returns the number of turns now indexed.
pub fn sync_index(db: &Db, cfg: &AppConfig) -> usize {
    let files = transcripts(cfg);
    let all: Vec<Turn> = files.iter().flat_map(|p| turns_of(p)).collect();
    let stored = db.chat_turn_hashes();
    let mut first = 0;
    while let Some(t) = all.get(first) {
        match stored.get(first) {
            Some((n, h)) if *n == first && *h == chat_row_hash(t.at, t.from, &t.q, &t.a) => {
                first += 1
            }
            _ => break,
        }
    }
    if first < all.len() || stored.len() != all.len() {
        let tail: Vec<ChatTurnRow> = all[first..]
            .iter()
            .enumerate()
            .map(|(i, t)| ChatTurnRow {
                n: first + i,
                at: t.at,
                from: t.from.to_string(),
                q: t.q.clone(),
                a: t.a.clone(),
            })
            .collect();
        db.chat_turns_replace_from(first, &tail);
    }
    all.len()
}

/// The Coordinator's turns matching `q`, best first, from the index. Call
/// [`sync_index`] first for the index to show the latest turns.
pub fn search(db: &Db, q: &str, limit: usize) -> Vec<ChatHit> {
    db.chat_search(q, limit)
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn the_window_follows_the_use() {
        assert_eq!(context_window(50_000, "claude-opus-5-5"), 200_000);
        assert_eq!(context_window(260_000, "claude-opus-5-5"), 1_000_000);
        assert_eq!(context_window(10, "claude-opus-5-5[1m]"), 1_000_000);
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
    fn the_running_turn_says_what_it_does() {
        let text = [
            r#"{"type":"user","timestamp":"2026-10-19T09:00:00.000Z","message":{"content":"проверь очередь"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:00:05.000Z","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","input":{"command":"herdr-projects list --json","description":"List the board's tasks"}}]}}"#,
        ]
        .join("\n");
        let t = parse(&text);
        assert_eq!(t[0].doing, "List the board's tasks", "Bash by its description");
        let more = format!(
            "{text}\n{}\n{}",
            r#"{"type":"user","timestamp":"2026-10-19T09:00:06.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:00:10.000Z","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Read","input":{"file_path":"/x/task.json"}}]}}"#,
        );
        let t = parse(&more);
        assert_eq!(t[0].doing, "Read /x/task.json", "the latest call wins; another tool by its name and file");
        let bash_cmd = text.replace(r#","description":"List the board's tasks""#, "");
        assert_eq!(parse(&bash_cmd)[0].doing, "herdr-projects list --json", "without a description, the command's first line");
        let done = format!(
            "{more}\n{}",
            r#"{"type":"assistant","timestamp":"2026-10-19T09:00:20.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Очередь пуста."}]}}"#,
        );
        let t = parse(&done);
        assert_eq!((t[0].doing.as_str(), t[0].a.as_str()), ("", "Очередь пуста."), "end_turn clears it");
        let long = format!(
            r#"{{"type":"user","timestamp":"2026-10-19T09:00:00.000Z","message":{{"content":"x"}}}}
{{"type":"assistant","timestamp":"2026-10-19T09:00:05.000Z","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Bash","input":{{"command":"{}"}}}}]}}}}"#,
            "y".repeat(200)
        );
        assert_eq!(parse(&long)[0].doing.chars().count(), 81, "cut to 80 and an ellipsis");
        let json = serde_json::to_value(&parse(&done)[0]).unwrap();
        assert!(json.get("doing").is_none(), "an empty `doing` is not sent to the page");
    }

    #[test]
    fn a_message_with_images_keeps_them_as_blocks() {
        // Claude Code writes a message sent with pictures as a text block (ending at the planner's header,
        // the path lines dropped) and an image block with the bytes inside.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lK3Q6wAAAABJRU5ErkJggg==";
        let rec = format!(
            r#"{{"type":"user","timestamp":"2026-10-21T04:16:03.468Z","message":{{"role":"user","content":[{{"type":"text","text":"вот снимок\nКартинки (файлы на машине с доской):"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{png}"}}}}]}}}}"#
        );
        let text = format!("{}\n{rec}\n", r#"{"type":"user","timestamp":"2026-10-21T04:00:00.000Z","message":{"content":"первый"}}"#);
        let t = parse(&text);
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].q, "вот снимок\nКартинки (файлы на машине с доской):");
        assert_eq!(t[1].images, vec!["image/png".to_string()]);
        assert_eq!(t[1].line, 1, "the record's line, for the image to be read back");
        assert!(t[0].images.is_empty());
        let (mime, bytes) = image_block(&rec, 0).expect("the image block");
        assert_eq!(mime, "image/png");
        assert_eq!(&bytes[..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
        assert!(image_block(&rec, 1).is_none());
        let json = serde_json::to_value(&t[1]).unwrap();
        assert_eq!(json["images"], serde_json::json!(["image/png"]));
        assert!(json.get("file").is_none() && json.get("line").is_none(), "where the record is stays inside");
        assert!(serde_json::to_value(&t[0]).unwrap().get("images").is_none());
    }

    #[test]
    fn the_answer_has_its_own_time() {
        let t = parse(SAMPLE);
        assert_eq!(t[0].at, 1_792_389_365_628);
        assert_eq!(t[0].at_a, parse_time("2026-10-19T05:56:16.998Z"), "the time of the assistant record");
        assert_eq!(t[2].at_a, 0, "no answer yet");
        assert!(serde_json::to_value(&t[2]).unwrap().get("at_a").is_none());
    }

    #[test]
    fn a_long_question_keeps_its_words() {
        let q = "ж".repeat(30_000);
        let text = format!(
            r#"{{"type":"user","timestamp":"2026-10-19T05:56:05.000Z","message":{{"content":"{q}"}}}}"#
        );
        let t = parse(&text);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].q, q, "a message the board accepts is not cut in the history");
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

    fn line(kind: &str, at: &str, body: &str) -> String {
        format!(r#"{{"type":"{kind}","timestamp":"{at}",{body}}}"#)
    }

    #[test]
    fn the_index_follows_the_transcript_numbering() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude");
        // Only this test reads CLAUDE_CONFIG_DIR.
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude);
        let mut cfg = AppConfig::load(&[]);
        cfg.data_dir = dir.path().join("data");
        let chat = cfg.data_dir.join("coordinator").join("default").join("chat");
        std::fs::create_dir_all(&chat).unwrap();
        let chat = std::fs::canonicalize(&chat).unwrap();
        let folder = claude.join("projects").join(project_folder(&chat));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("s1.jsonl");
        let q1 = line("user", "2026-10-19T05:56:05.000Z", r#""message":{"content":"первый вопрос"}"#);
        let a1 = line(
            "assistant",
            "2026-10-19T05:56:09.000Z",
            r#""message":{"stop_reason":"end_turn","content":[{"type":"text","text":"ответ про пиццу"}]}"#,
        );
        std::fs::write(&file, format!("{q1}\n{a1}\n")).unwrap();

        let db = Db::memory().unwrap();
        assert_eq!(sync_index(&db, &cfg), 1);
        assert_eq!(search(&db, "пиццу", 10).len(), 1);
        assert_eq!(search(&db, "пиццу", 10)[0].n, 0);

        // A new question and its answer: appended, nothing before them is rewritten.
        let q2 = line("user", "2026-10-19T06:00:00.000Z", r#""message":{"content":"второй вопрос"}"#);
        std::fs::write(&file, format!("{q1}\n{a1}\n{q2}\n")).unwrap();
        assert_eq!(sync_index(&db, &cfg), 2);
        assert_eq!(search(&db, "второй", 10)[0].n, 1);

        // The last answer changes while the Coordinator works.
        let a2 = line(
            "assistant",
            "2026-10-19T06:00:30.000Z",
            r#""message":{"stop_reason":"end_turn","content":[{"type":"text","text":"ответ про бюджет"}]}"#,
        );
        std::fs::write(&file, format!("{q1}\n{a1}\n{q2}\n{a2}\n")).unwrap();
        assert_eq!(sync_index(&db, &cfg), 2);
        assert!(search(&db, "пиццу", 10).len() == 1, "the first turn is kept");
        assert_eq!(search(&db, "бюджет", 10)[0].n, 1);

        // A new session file is older than the current one: the numbering shifts.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let older = folder.join("s0.jsonl");
        std::fs::write(&older, format!("{}\n", line("user", "2026-10-18T10:00:00.000Z", r#""message":{"content":"старый"}"#))).unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&older).unwrap().set_modified(t).unwrap();
        assert_eq!(sync_index(&db, &cfg), 3);
        assert_eq!(search(&db, "старый", 10)[0].n, 0, "the older session comes first");
        assert_eq!(search(&db, "пиццу", 10)[0].n, 1, "numbers follow the page()");
        assert_eq!(page(&cfg, None, 50).total, 3);

        // The transcripts go away: the index is emptied too.
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(sync_index(&db, &cfg), 0);
        assert!(search(&db, "пиццу", 10).is_empty());
    }
}
