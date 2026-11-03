//! The Coordinator's conversation, read from its own transcript.
//!
//! The board has no "chat history" command, but the Coordinator's program saves its conversation in its
//! own folder, and `state.json` says which program it is and which conversation is its own. The planner
//! finds that file (see [`places`]) and cuts it into turns: what the human wrote and the words the
//! Coordinator answered with. Tool calls and thinking are left out.
//!
//! Two record shapes are read: the one where a record's type is its role (`{"type":"user", …}`), and
//! the one omp and pi write, where every record is a message and the role is inside it
//! (`{"type":"message","message":{"role":"user", …}}`). Images are either blocks with the bytes in
//! them or blocks naming a file of the program's blob store (`blob:sha256:…`).
//!
//! `HERDR_PLANNER_TRANSCRIPTS` points the fixture at its transcript root. Without transcript files the
//! page keeps showing the window's tail.

use crate::config::{home, AppConfig};
use crate::db::{chat_row_hash, ChatHit, ChatTurnRow, Db};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// A human message may be 100 000 characters (a spec, a log): the history keeps all of it, so the
/// chat shows and reuses the words as they were sent. Answers are still bounded by `MAX_ANSWER`.
const MAX_QUESTION: usize = 100_000;
const MAX_ANSWER: usize = 8000;
/// Board notices the Coordinator is told by itself, not typed by the human.
const BOARD_PREFIX: &str = "[Уведомление доски]";
/// What the planner tells the Coordinator about the human's own actions (approving a plan, allowing an agent).
const ACTION_PREFIX: &str = "[Действие в планировщике]";

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
    /// The media types of the images in the human's message (the transcript keeps them as blocks of the
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
    let mut seen = HashSet::new();
    for (line_no, line) in text.lines().enumerate() {
        // A line without a type is not a record of a conversation (a torn tail, a line of a log):
        // the big snapshots and requests are parsed and then passed over by the match below.
        let wanted = line.contains("\"type\"") || line.contains("\"queued_command\"");
        if !wanted {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // Native logs can repeat a record when a session resumes. GPUI also deduplicates by UUID.
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if let Some(id) = record_id(&v) {
            if !seen.insert(id.to_string()) {
                continue;
            }
        }
        let at = record_time(&v);
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
            Some("assistant") => answer(&v, at, &mut turns, &mut running),
            // omp and pi put the role inside the record; a tool result is no turn of its own.
            Some("message") => match v.pointer("/message/role").and_then(Value::as_str) {
                Some("user") => {
                    if let Some((q, images)) = human_text(v.pointer("/message/content")) {
                        // The record does not say whether it was typed during a run: a notice that
                        // arrives while the Coordinator works belongs to that run, a human's words
                        // keep their own place.
                        open_turn(&mut turns, q, images, line_no, at, running, &mut running);
                    }
                }
                Some("assistant") => answer(&v, at, &mut turns, &mut running),
                _ => {}
            },
            _ => {}
        }
    }
    turns
}

/// A record's own id: `uuid` in the older shape, `id` in omp's and pi's.
fn record_id(v: &Value) -> Option<&str> {
    ["uuid", "id"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(Value::as_str))
        .find(|s| !s.is_empty())
}

/// A record's time: the ISO stamp at its top, else the milliseconds omp puts beside the message.
fn record_time(v: &Value) -> u64 {
    if let Some(s) = v.get("timestamp").and_then(Value::as_str) {
        return parse_time(s);
    }
    v.pointer("/message/timestamp").and_then(Value::as_u64).unwrap_or(0)
}

/// The words of an assistant record, and what it is doing: both shapes of the record are read here
/// (`tool_use` with `input` and `end_turn`, `toolCall` with `arguments` and `stop`/`toolUse`).
fn answer(v: &Value, at: u64, turns: &mut [Turn], running: &mut bool) {
    let Some(turn) = turns.last_mut() else {
        return;
    };
    let stop = v.pointer("/message/stop_reason").or_else(|| v.pointer("/message/stopReason")).and_then(Value::as_str);
    let Some(Value::Array(blocks)) = v.pointer("/message/content") else {
        return;
    };
    let calls = blocks.iter().any(|b| matches!(b.get("type").and_then(Value::as_str), Some("tool_use" | "toolCall")));
    let ended = match stop {
        Some("end_turn" | "stop" | "error") => true,
        Some("tool_use" | "toolUse") => false,
        _ => !calls,
    };
    if ended {
        *running = false;
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
            Some("tool_use" | "toolCall") => turn.doing = doing_of(b),
            _ => {}
        }
    }
}

/// Keep each human record at its own transcript position, including mid-turn input.
/// Board notices inside a run stay folded into that run, as before.
fn open_turn(turns: &mut Vec<Turn>, q: String, images: Vec<String>, line: usize, at: u64, queued: bool, running: &mut bool) {
    let from = if q.starts_with(BOARD_PREFIX) || q.starts_with(ACTION_PREFIX) {
        "board"
    } else {
        "human"
    };
    if queued && *running && from == "board" {
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
    // The older shape keeps the arguments in `input`, omp's in `arguments`.
    let input = block.get("input").or_else(|| block.get("arguments"));
    let field = |k: &str| {
        input
            .and_then(|i| i.get(k))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.lines().next().unwrap_or("").to_string())
    };
    // omp says in a few words what a call is for (`intent`, or `i` inside the arguments).
    let intent = block
        .get("intent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.lines().next().unwrap_or("").to_string())
        .or_else(|| field("intent"))
        .or_else(|| field("i"));
    let text = if name.eq_ignore_ascii_case("bash") {
        intent.or_else(|| field("description")).or_else(|| field("command")).unwrap_or_default()
    } else {
        let what = intent
            .or_else(|| field("file_path"))
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
                    // The older shape names the type inside `source`, omp beside the block.
                    let mime = b
                        .pointer("/source/media_type")
                        .and_then(Value::as_str)
                        .or_else(|| b.get("mimeType").and_then(Value::as_str))
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
    if (text.is_empty() && images.is_empty()) || text.starts_with('<') {
        return None;
    }
    Some((text.to_string(), images))
}

/// The `i`-th image block of a transcript record (a message's content or a queued command's prompt):
/// its media type and bytes. The bytes are inside the block, or in the program's blob store under the
/// hash the block names (`blob:sha256:…`).
fn image_block(record: &str, i: usize, blobs: &Path) -> Option<(String, Vec<u8>)> {
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
        .or_else(|| block.get("mimeType").and_then(Value::as_str))
        .unwrap_or("image/png")
        .to_string();
    if let Some(data) = block.pointer("/source/data").and_then(Value::as_str) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .ok()?;
        return Some((mime, bytes));
    }
    let hash = block.get("data").and_then(Value::as_str)?.strip_prefix("blob:sha256:")?;
    // The hash names a file of the store, never a path of its own.
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = std::fs::read(blobs.join(hash))
        .or_else(|_| std::fs::read(blobs.join(format!("{hash}.png"))))
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
    image_block(&record, i, &Homes::live().blobs())
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

/// A transcript names a project folder after the directory: every character that is
/// not a letter or a digit becomes `-`.
pub(crate) fn project_folder(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Where the agent programs keep their saved conversations. Tests point these at a temporary folder.
#[derive(Debug, Clone)]
struct Homes {
    /// `HOME`: omp names a session folder after the path as it sees it, relative to this.
    home: PathBuf,
    /// `PI_CODING_AGENT_DIR` or `~/.pi/agent`: pi's conversations are `<agent>/sessions/--<cwd>--`.
    pi: PathBuf,
    /// `PI_CODING_AGENT_DIR` or `~/.omp/agent`: omp's conversations are `<agent>/sessions/<folder>`,
    /// its images `<agent>/blobs/<hash>`.
    omp: PathBuf,
    /// `CODEX_HOME` or `~/.codex`: codex's conversations are below `<home>/sessions/yyyy/mm/dd`.
    codex: PathBuf,
    /// `CLAUDE_CONFIG_DIR` or `~/.claude`: `<home>/projects/<folder>/<session>.jsonl`.
    claude: PathBuf,
    /// `QWEN_HOME` or `~/.qwen`: `<home>/projects/<folder>/chats`.
    qwen: PathBuf,
}

impl Homes {
    fn live() -> Homes {
        let home = home();
        let env_path = |var: &str, fallback: PathBuf| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or(fallback)
        };
        Homes {
            pi: env_path("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
            omp: env_path("PI_CODING_AGENT_DIR", home.join(".omp/agent")),
            codex: env_path("CODEX_HOME", home.join(".codex")),
            claude: env_path("CLAUDE_CONFIG_DIR", home.join(".claude")),
            qwen: env_path("QWEN_HOME", home.join(".qwen")),
            home,
        }
    }

    /// omp's blob store: an image block names a file here by its hash.
    fn blobs(&self) -> PathBuf {
        self.omp.join("blobs")
    }
}

/// A path as omp names a session folder: the leading separator becomes the leading dash, the others too.
fn path_dashes(p: &Path) -> String {
    format!("-{}", p.to_string_lossy().trim_start_matches(['/', '\\']).replace(['/', '\\'], "-"))
}

/// The folder omp keeps the conversations of `cwd` in. It dashes the separators of the path as it sees
/// it — under `HOME` relative to it. Both spellings are looked for, wrapped in `--…--` too.
fn omp_folders(cwd: &Path, home: &Path) -> Vec<String> {
    let mut names = vec![path_dashes(cwd)];
    if let Ok(rel) = cwd.strip_prefix(home) {
        names.push(path_dashes(rel));
    }
    let wrapped: Vec<String> = names.iter().map(|n| format!("--{}--", n.trim_start_matches('-'))).collect();
    names.extend(wrapped);
    names
}

/// pi keeps each working folder's conversations in `--<path with / and : dashed>--`.
fn pi_folder(agent: &Path, cwd: &Path) -> PathBuf {
    let p = cwd.to_string_lossy();
    let p = p.strip_prefix(['/', '\\']).unwrap_or(&p);
    agent.join("sessions").join(format!("--{}--", p.replace(['/', '\\', ':'], "-")))
}

/// The folders a program keeps the conversations of this working folder in. An unknown or absent
/// program tries them all: whatever is there is the Coordinator's, since only it runs in its chat.
/// The folders of the other programs are not read when this one is known: each does not keep the
/// conversation of the other, and a program changed mid-day would otherwise show two of them at once.
fn session_dirs(r: &Homes, cwd: &Path, kind: &str) -> Vec<PathBuf> {
    let all = kind.is_empty();
    let mut dirs = Vec::new();
    if all || kind == "omp" {
        let root = r.omp.join("sessions");
        dirs.extend(omp_folders(cwd, &r.home).into_iter().map(|n| root.join(n)));
    }
    if all || kind == "pi" {
        dirs.push(pi_folder(&r.pi, cwd));
    }
    if all || kind == "claude" {
        dirs.push(r.claude.join("projects").join(project_folder(cwd)));
    }
    if all || kind == "qwen" {
        dirs.push(r.qwen.join("projects").join(project_folder(cwd)).join("chats"));
    }
    dirs
}

/// The folders with the Coordinator's conversations (what the watcher watches) and the files it was
/// told about (pi's hook writes the path, omp's `session` is the file itself).
#[derive(Debug, Default, Clone, PartialEq)]
struct Places {
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
}

impl Places {
    fn push_file(&mut self, f: &Path) {
        if let Some(d) = f.parent() {
            self.dirs.push(d.to_path_buf());
        }
        self.files.push(f.to_path_buf());
    }
}

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect()
}

/// Where the Coordinator's conversation files are. `state.json` decides: `transcript` is the exact file
/// the board remembered (pi's turn-end hook writes it), `session` is that file's path for omp and an id
/// for pi, codex and claude, and `run.kind` is the program. A folder of the program's own is read whole,
/// so the turns of the sessions before this one are there too.
fn places(r: &Homes, cwd: &Path, state: &Value) -> Places {
    let kind = state.pointer("/run/kind").and_then(Value::as_str).unwrap_or("");
    let session = state.get("session").and_then(Value::as_str).unwrap_or("").trim();
    let mut out = Places::default();
    if let Some(f) = state.get("transcript").and_then(Value::as_str).filter(|s| !s.is_empty()).map(PathBuf::from) {
        if f.is_file() {
            out.push_file(&f);
        }
    }
    let direct = PathBuf::from(session);
    if direct.is_absolute() && direct.is_file() {
        out.push_file(&direct);
    }
    out.dirs.extend(session_dirs(r, cwd, kind));
    // Codex saves every conversation in one date tree; the id names the file.
    if kind == "codex" && !session.is_empty() {
        if let Some(f) = codex_file(&r.codex.join("sessions"), session) {
            out.push_file(&f);
        }
    }
    out.dirs.sort();
    out.dirs.dedup();
    out
}

/// The file of a codex conversation by its id, at most four folders deep (`sessions/yyyy/mm/dd`).
fn codex_file(root: &Path, session: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, want: &str, depth: usize) -> Option<PathBuf> {
        let mut deeper = Vec::new();
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth > 0 {
                    deeper.push(p);
                }
                continue;
            }
            if p.file_name().map(|n| n.to_string_lossy().ends_with(want)).unwrap_or(false) {
                return Some(p);
            }
        }
        deeper.into_iter().find_map(|d| walk(&d, want, depth - 1))
    }
    walk(root, &format!("{session}.jsonl"), 3)
}

/// The fixture's transcript root: the front of the folder the planner keeps for the chat.
fn transcript_home() -> Option<PathBuf> {
    std::env::var_os("HERDR_PLANNER_TRANSCRIPTS")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The Coordinator's chat folder: its program was started there and names its session after it.
fn chat_cwd(cfg: &AppConfig) -> PathBuf {
    let cwd = cfg.data_dir.join("coordinator").join("default").join("chat");
    std::fs::canonicalize(&cwd).unwrap_or(cwd)
}

/// What the board's record of the Coordinator says about its run and its conversation.
fn coordinator_state(cfg: &AppConfig) -> Value {
    std::fs::read_to_string(cfg.data_dir.join("coordinator/default/state.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or(Value::Null)
}

fn places_of(cfg: &AppConfig) -> Places {
    let cwd = chat_cwd(cfg);
    if let Some(root) = transcript_home() {
        return Places { dirs: vec![root.join(project_folder(&cwd))], files: Vec::new() };
    }
    places(&Homes::live(), &cwd, &coordinator_state(cfg))
}

/// A folder the watcher should watch: `deep` for a whole tree (codex's date folders), `chat` when the
/// Coordinator's own conversation files live in it — a change there is a new turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub path: PathBuf,
    pub deep: bool,
    pub chat: bool,
}

/// The closest folder above `path` that is there: a watcher cannot watch a folder that is not made yet.
fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut at = path.parent();
    while let Some(dir) = at {
        if dir.is_dir() {
            return (dir != Path::new("/")).then(|| dir.to_path_buf());
        }
        at = dir.parent();
    }
    None
}

/// Where the watcher should look. The folders of every program are named, not only those of the one in
/// use: the Coordinator can be started for the first time or switched to another program while the
/// planner runs, and its folder then appears out of nothing. For a folder that is not made yet, the
/// closest folder above it is watched instead, so its making is seen; the watcher builds the plan
/// again after each event and adds the folders that have appeared.
pub fn watch_plan(cfg: &AppConfig) -> Vec<Watch> {
    watch_plan_at(&Homes::live(), &chat_cwd(cfg), &coordinator_state(cfg), transcript_home().as_deref())
}

fn watch_plan_at(r: &Homes, cwd: &Path, state: &Value, fixture: Option<&Path>) -> Vec<Watch> {
    let mut plan: Vec<Watch> = Vec::new();
    let candidates = match fixture {
        Some(root) => vec![root.join(project_folder(cwd))],
        None => {
            let mut dirs = places(r, cwd, state).dirs;
            dirs.extend(session_dirs(r, cwd, ""));
            dirs.sort();
            dirs.dedup();
            dirs
        }
    };
    for dir in candidates {
        let chat = dir.is_dir();
        let path = if chat { Some(dir) } else { nearest_existing(&dir) };
        let Some(path) = path else { continue };
        match plan.iter_mut().find(|w| w.path == path) {
            Some(w) => w.chat |= chat,
            None => plan.push(Watch { path, deep: false, chat }),
        }
    }
    // Codex names its conversations by date, so the folder of the one in use cannot be named before
    // it starts: until the file is found, its whole tree is watched.
    let kind = state.pointer("/run/kind").and_then(Value::as_str).unwrap_or("");
    if fixture.is_none() && kind == "codex" {
        let root = r.codex.join("sessions");
        let path = if root.is_dir() { Some(root) } else { nearest_existing(&root) };
        if let Some(path) = path {
            match plan.iter_mut().find(|w| w.path == path) {
                Some(w) => w.deep = true,
                None => plan.push(Watch { path, deep: true, chat: false }),
            }
        }
    }
    plan
}

/// The Coordinator's transcripts, oldest first (a new session starts a new file); empty when there are none.
fn transcripts(cfg: &AppConfig) -> Vec<PathBuf> {
    let places = places_of(cfg);
    let mut files: Vec<PathBuf> = places.dirs.iter().flat_map(|d| jsonl_files(d)).collect();
    files.extend(places.files);
    files.sort();
    files.dedup();
    let mut with_time: Vec<(SystemTime, PathBuf)> = files
        .into_iter()
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH), p))
        .collect();
    with_time.sort();
    with_time.into_iter().map(|(_, p)| p).collect()
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

/// The Coordinator's program (`codex`, `omp`, …) from its state; `""` when unknown.
pub fn coordinator_program(cfg: &AppConfig) -> String {
    coordinator_state(cfg).pointer("/run/kind").and_then(Value::as_str).map(String::from).unwrap_or_default()
}

/// The last assistant turn's token count and model, read from the tail of the
/// newest transcript. `None` when there is no transcript or no usage in it.
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
        // The older shape names the role as the record's type; omp and pi put it inside.
        let assistant = v.get("type").and_then(Value::as_str) == Some("assistant")
            || (v.get("type").and_then(Value::as_str) == Some("message")
                && v.pointer("/message/role").and_then(Value::as_str) == Some("assistant"));
        if !assistant {
            continue;
        }
        let Some(msg) = v.get("message") else { continue };
        let Some(u) = msg.get("usage") else { continue };
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        // Old records count in `*_tokens`, omp's in `input`/`cacheRead`/`cacheWrite`.
        let used = n("input_tokens")
            + n("cache_read_input_tokens")
            + n("cache_creation_input_tokens")
            + n("input")
            + n("cacheRead")
            + n("cacheWrite");
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
        assert_eq!(context_window(50_000, "gpt-6-luna"), 200_000);
        assert_eq!(context_window(260_000, "gpt-6-luna"), 1_000_000);
        assert_eq!(context_window(10, "gpt-6-luna[1m]"), 1_000_000);
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
    fn an_action_in_the_planner_is_a_board_notice() {
        let text = [
            r#"{"type":"user","timestamp":"2026-10-19T09:00:00.000Z","message":{"content":"продолжай"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:00:05.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Жду."}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-19T09:01:00.000Z","message":{"content":"[Действие в планировщике] PLN-60: план одобрен"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:01:05.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Принято, продолжаю."}]}}"#,
        ]
        .join("\n");
        let t = parse(&text);
        assert_eq!(t.len(), 2);
        assert_eq!((t[1].from, t[1].q.as_str()), ("board", "[Действие в планировщике] PLN-60: план одобрен"));
        assert_eq!((t[0].from, t[0].q.as_str()), ("human", "продолжай"));
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
    fn a_human_word_queued_in_a_run_keeps_its_position() {
        let text = [
            r#"{"type":"user","timestamp":"2026-10-19T09:00:00.000Z","message":{"content":"давай 1"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:00:05.000Z","message":{"stop_reason":"tool_use","content":[{"type":"text","text":"Now the next one."},{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            r#"{"type":"attachment","timestamp":"2026-10-19T09:00:10.000Z","attachment":{"type":"queued_command","prompt":"ещё и таймаут 600"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-19T09:00:30.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Готово обе."}]}}"#,
        ]
        .join("\n");
        let t = parse(&text);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].q, "давай 1");
        assert_eq!(t[0].a, "Now the next one.");
        assert_eq!(t[1].q, "ещё и таймаут 600");
        assert_eq!(t[1].a, "Готово обе.");
        assert_eq!(t[1].line, 2);
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
        // The transcript writes a message sent with pictures as a text block (ending at the planner's header,
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
        let (mime, bytes) = image_block(&rec, 0, Path::new("/nonexistent-blobs")).expect("the image block");
        assert_eq!(mime, "image/png");
        assert_eq!(&bytes[..8], &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
        assert!(image_block(&rec, 1, Path::new("/nonexistent-blobs")).is_none());
        let json = serde_json::to_value(&t[1]).unwrap();
        assert_eq!(json["images"], serde_json::json!(["image/png"]));
        assert!(json.get("file").is_none() && json.get("line").is_none(), "where the record is stays inside");
        assert!(serde_json::to_value(&t[0]).unwrap().get("images").is_none());
    }

    #[test]
    fn mid_turn_images_keep_their_own_record_and_all_blocks() {
        let source = r#"{"type":"base64","media_type":"image/png","data":"AQID"}"#;
        let queued = format!(r#"{{"type":"attachment","uuid":"queued","attachment":{{"type":"queued_command","prompt":[{{"type":"text","text":"[Image #1] [Image #2]ещё мысль"}},{{"type":"image","source":{source}}},{{"type":"image","source":{source}}}]}}}}"#);
        let text = [
            r#"{"type":"user","uuid":"first","message":{"content":"первый"}}"#,
            r#"{"type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"text","text":"Работаю"},{"type":"tool_use","name":"Bash"}]}}"#,
            &queued,
            &queued, // resumed logs can repeat a UUID
            r#"{"type":"user","isSidechain":true,"message":{"content":"чужой поток"}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"третья мысль"}}"#,
            r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Сделал все три"}]}}"#,
            r#"{"type":"user","message":{"content":"Запускай"}}"#,
        ].join("\n");
        let turns = parse(&text);
        assert_eq!(turns.len(), 4);
        assert_eq!(turns[0].a, "Работаю");
        assert_eq!(turns[1].q, "[Image #1] [Image #2]ещё мысль");
        assert_eq!(turns[1].line, 2);
        assert_eq!(turns[1].images, vec!["image/png", "image/png"]);
        assert!(turns[1].a.is_empty());
        assert_eq!(turns[2].q, "третья мысль");
        assert_eq!(turns[2].a, "Сделал все три");
        assert_eq!(turns[3].q, "Запускай");
        assert!(turns[3].a.is_empty());
        for i in 0..2 {
            assert_eq!(image_block(text.lines().nth(turns[1].line).unwrap(), i, Path::new("/nonexistent-blobs")), Some(("image/png".into(), vec![1, 2, 3])));
        }
        assert!(image_block(&queued, 2, Path::new("/nonexistent-blobs")).is_none());
    }

    #[test]
    fn image_only_messages_are_not_lost() {
        let text = r#"{"type":"user","message":{"content":[{"type":"image","source":{"media_type":"image/png"}}]}}"#;
        let turns = parse(text);
        assert_eq!(turns.len(), 1);
        assert!(turns[0].q.is_empty());
        assert_eq!(turns[0].images, vec!["image/png"]);
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
    fn names_the_project_folder_from_the_working_directory() {
        assert_eq!(
            project_folder(Path::new(
                "/home/me/.herdr-projects/data/coordinator/default/chat"
            )),
            "-home-me--herdr-projects-data-coordinator-default-chat"
        );
    }

    /// The shape omp 18.16 writes (and pi too): every record is a message and the role is inside it.
    const OMP: &str = include_str!("../tests/fixtures/transcripts/omp-chat.jsonl");

    #[test]
    fn the_new_omp_format_is_cut_into_the_same_turns() {
        let t = parse(OMP);
        assert_eq!(t.len(), 2, "a notice inside the run is not a turn, nor is a tool result or a repeat");
        assert_eq!(t[0].from, "human");
        assert_eq!(t[0].q, "PLN-103 доделай: первая строка\nвторая строка", "a multi-line message keeps its lines");
        assert_eq!(t[0].a, "Готово: PLN-103 доделана.\nPLN-104 тоже посмотрел.");
        assert_eq!(t[0].at, 1_793_623_835_692);
        assert_eq!(t[0].doing, "", "the finished run says nothing more");
        assert_eq!(t[1].q, "[Image #1, 1009x285] почему авто-компакт запускается");
        assert_eq!(t[1].images, vec!["image/png".to_string()], "an image block is kept, the bytes live in the blob store");
        assert_eq!(t[1].a, "Авто-компакт включается на 300k токенах.");
        assert_eq!(t[1].at_a, 1_793_682_902_000, "the time inside the message, when the record has no stamp");
    }

    #[test]
    fn a_running_omp_turn_says_what_it_does() {
        let prefix = OMP.lines().take(6).collect::<Vec<_>>().join("\n");
        let t = parse(&prefix);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].doing, "Смотрю доску", "the tool's own intent, as omp writes it");
        assert!(t[0].a.is_empty(), "nothing answered yet");
    }

    #[test]
    fn an_omp_image_is_read_from_the_blob_store() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = dir.path().join("blobs");
        std::fs::create_dir_all(&blobs).unwrap();
        let hash = "bba27d3d6a67b883bc0bc6449874283d3b8614c2a4e38fc58312d1ca0fccc587";
        std::fs::write(blobs.join(hash), [1u8, 2, 3]).unwrap();
        let rec = OMP.lines().find(|l| l.contains("\"mimeType\"")).unwrap();
        assert_eq!(image_block(rec, 0, &blobs), Some(("image/png".to_string(), vec![1, 2, 3])));
        assert!(image_block(rec, 1, &blobs).is_none(), "there is only one image in the message");
        assert_eq!(image_block(rec, 0, dir.path()), None, "an empty store has no image");
        let traversal = rec.replace(hash, "../etc/passwd");
        assert!(image_block(&traversal, 0, &blobs).is_none(), "a hash is hex and nothing else");
    }

    /// A home with one working folder: every program names its own folder inside it.
    fn homes(dir: &Path) -> (Homes, PathBuf) {
        let home = dir.join("home");
        let cwd = home.join(".herdr-projects/data/coordinator/default/chat");
        std::fs::create_dir_all(&cwd).unwrap();
        let home = std::fs::canonicalize(&home).unwrap_or(home);
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
        let r = Homes {
            home: home.clone(),
            pi: home.join(".pi/agent"),
            omp: home.join(".omp/agent"),
            codex: home.join(".codex"),
            claude: home.join(".claude"),
            qwen: home.join(".qwen"),
        };
        (r, cwd)
    }

    fn write_session(path: &Path) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, OMP).unwrap();
        path.to_path_buf()
    }

    #[test]
    fn the_programs_name_the_folder_of_a_conversation_their_own_way() {
        let cwd = Path::new("/home/me/.herdr-projects/data/coordinator/default/chat");
        let home = Path::new("/home/me");
        assert_eq!(
            omp_folders(cwd, home),
            vec![
                "-home-me-.herdr-projects-data-coordinator-default-chat",
                "-.herdr-projects-data-coordinator-default-chat",
                "--home-me-.herdr-projects-data-coordinator-default-chat--",
                "--.herdr-projects-data-coordinator-default-chat--",
            ]
        );
        assert_eq!(
            pi_folder(Path::new("/home/me/.pi/agent"), cwd),
            Path::new("/home/me/.pi/agent/sessions/--home-me-.herdr-projects-data-coordinator-default-chat--")
        );
        // Without a known program (and for the watcher) every folder of the chat is taken.
        let dir = tempfile::tempdir().unwrap();
        let (r, cwd) = homes(dir.path());
        let all = session_dirs(&r, &cwd, "");
        assert!(all.contains(&pi_folder(&r.pi, &cwd)));
        assert!(all.contains(&r.claude.join("projects").join(project_folder(&cwd))));
        assert!(all.contains(&r.qwen.join("projects").join(project_folder(&cwd)).join("chats")));
        assert_eq!(all.len(), 7, "four spellings of an omp folder and one folder for each other program");
        assert_eq!(session_dirs(&r, &cwd, "omp").len(), 4, "a known program keeps its own folder only");
    }

    #[test]
    fn an_omp_session_is_found_by_its_path_and_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (r, cwd) = homes(dir.path());
        // The live spelling of omp 18.15/18.16: the path under HOME with its separators dashed.
        let folder = omp_folders(&cwd, &r.home)[1].clone();
        let file = write_session(&r.omp.join("sessions").join(&folder).join("2026-11-02.jsonl"));
        let state = serde_json::json!({ "run": { "kind": "omp" }, "session": file.display().to_string() });
        let found = places(&r, &cwd, &state);
        assert!(found.dirs.contains(&file.parent().unwrap().to_path_buf()));
        assert_eq!(found.files, vec![file.clone()], "omp says the conversation's path itself");
        assert_eq!(transcript_files(&found), vec![file], "and the folder holds the older sessions too");
    }

    #[test]
    fn pi_and_codex_are_found_by_the_session_the_board_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let (r, cwd) = homes(dir.path());
        // pi: a folder named --<path with its separators dashed>--, a file that ends with _<id>.jsonl
        let id = "sw-01a19c4d-7ad4";
        let pi_file = write_session(&pi_folder(&r.pi, &cwd).join(format!("2026-11-02T12-48-05-844Z_{id}.jsonl")));
        let state = serde_json::json!({ "run": { "kind": "pi" }, "session": id });
        let found = places(&r, &cwd, &state);
        assert!(found.dirs.contains(&pi_file.parent().unwrap().to_path_buf()));
        assert_eq!(transcript_files(&found), vec![pi_file], "the history comes from the folder, not from the id");

        // codex: one date tree for every conversation, the id names the file.
        let codex_file = write_session(&r.codex.join(format!("sessions/2026/11/03/rollout-2026-11-03T05-00-00-{id}.jsonl")));
        let state = serde_json::json!({ "run": { "kind": "codex" }, "session": id });
        let found = places(&r, &cwd, &state);
        assert!(found.files.contains(&codex_file));
        assert!(found.dirs.contains(&codex_file.parent().unwrap().to_path_buf()), "the file's folder is watched");
    }

    #[test]
    fn the_file_the_board_remembered_is_used_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let (r, cwd) = homes(dir.path());
        // pi's turn-end hook writes the exact file: it is read even when the folder name is not the expected one.
        let file = write_session(&dir.path().join("elsewhere/session.jsonl"));
        let state = serde_json::json!({ "run": { "kind": "pi" }, "transcript": file.display().to_string() });
        let found = places(&r, &cwd, &state);
        assert!(found.files.contains(&file));
        assert!(found.dirs.contains(&file.parent().unwrap().to_path_buf()));
    }

    #[test]
    fn a_missing_program_leaves_the_folders_it_can_find() {
        let dir = tempfile::tempdir().unwrap();
        let (r, cwd) = homes(dir.path());
        let file = write_session(&pi_folder(&r.pi, &cwd).join("a.jsonl"));
        let found = places(&r, &cwd, &serde_json::json!({}));
        assert_eq!(transcript_files(&found), vec![file], "without a program, every folder of the chat is tried");
    }

    #[test]
    fn the_watch_covers_the_folders_that_are_not_made_yet() {
        let dir = tempfile::tempdir().unwrap();
        let (r, cwd) = homes(dir.path());
        let state = serde_json::json!({ "run": { "kind": "omp" }, "session": "/x/none.jsonl" });
        // Nothing of the programs exists: the closest folder above each candidate is watched instead.
        let plan = watch_plan_at(&r, &cwd, &state, None);
        assert!(plan.iter().all(|w| !w.chat), "there is no conversation folder to watch yet");
        assert!(plan.iter().any(|w| w.path == r.home), "the home folder tells when a program's own appears");

        // The program's sessions folder appears: it is watched, so the chat's own folder cannot slip by.
        let sessions = r.omp.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let plan = watch_plan_at(&r, &cwd, &state, None);
        assert!(plan.iter().any(|w| w.path == sessions && !w.chat), "the root is watched until the folder exists");

        // The chat's folder appears after the planner started: it is watched as a conversation folder.
        let folder = sessions.join(omp_folders(&cwd, &r.home)[1].clone());
        write_session(&folder.join("s.jsonl"));
        let plan = watch_plan_at(&r, &cwd, &state, None);
        assert!(plan.iter().any(|w| w.path == folder && w.chat), "a folder made after start counts");

        // A folder of another program is in the plan too, even while omp is the program in use.
        let pi = write_session(&pi_folder(&r.pi, &cwd).join("a.jsonl"));
        let plan = watch_plan_at(&r, &cwd, &state, None);
        assert!(plan.iter().any(|w| w.path == pi.parent().unwrap() && w.chat), "a program change is seen without a restart");
    }

    #[test]
    fn codex_watches_its_whole_tree_until_its_conversation_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let (r, cwd) = homes(dir.path());
        let root = r.codex.join("sessions");
        std::fs::create_dir_all(&root).unwrap();
        let state = serde_json::json!({ "run": { "kind": "codex" }, "session": "abc" });
        let plan = watch_plan_at(&r, &cwd, &state, None);
        assert!(plan.iter().any(|w| w.path == root && w.deep && !w.chat), "the date folders are not known before a session");
        // Once it is found, its own folder is a conversation folder (and it sits under the deep watch).
        let file = write_session(&root.join("2026/11/03/rollout-2026-11-03T05-00-00-abc.jsonl"));
        let plan = watch_plan_at(&r, &cwd, &state, None);
        assert!(plan.iter().any(|w| w.path == file.parent().unwrap() && w.chat));
    }

    #[test]
    fn the_fixture_watches_its_folder_and_the_folder_above_it() {
        let dir = tempfile::tempdir().unwrap();
        let (r, cwd) = homes(dir.path());
        let root = dir.path().join("transcripts");
        let plan = watch_plan_at(&r, &cwd, &serde_json::json!({}), Some(&root));
        assert_eq!(plan, vec![Watch { path: dir.path().to_path_buf(), deep: false, chat: false }], "only the fixture root matters");
        let folder = root.join(project_folder(&cwd));
        std::fs::create_dir_all(&folder).unwrap();
        let plan = watch_plan_at(&r, &cwd, &serde_json::json!({}), Some(&root));
        assert_eq!(plan, vec![Watch { path: folder, deep: false, chat: true }]);
    }

    /// Files of a [`Places`] that are really there, oldest first (the read side of [`transcripts`]).
    fn transcript_files(p: &Places) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = p.dirs.iter().flat_map(|d| jsonl_files(d)).collect();
        files.extend(p.files.clone());
        files.sort();
        files.dedup();
        let mut with_time: Vec<(SystemTime, PathBuf)> = files
            .into_iter()
            .map(|f| (std::fs::metadata(&f).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH), f))
            .collect();
        with_time.sort();
        with_time.into_iter().map(|(_, f)| f).collect()
    }

    fn line(kind: &str, at: &str, body: &str) -> String {
        format!(r#"{{"type":"{kind}","timestamp":"{at}",{body}}}"#)
    }

    #[test]
    fn the_index_follows_the_transcript_numbering() {
        let dir = tempfile::tempdir().unwrap();
        let transcripts = dir.path().join("transcripts");
        // Only this test reads HERDR_PLANNER_TRANSCRIPTS.
        std::env::set_var("HERDR_PLANNER_TRANSCRIPTS", &transcripts);
        let mut cfg = AppConfig::load(&[]);
        cfg.data_dir = dir.path().join("data");
        let chat = cfg.data_dir.join("coordinator").join("default").join("chat");
        std::fs::create_dir_all(&chat).unwrap();
        let chat = std::fs::canonicalize(&chat).unwrap();
        let folder = transcripts.join(project_folder(&chat));
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

        // A session in the shape omp 18.16 writes: its turns are indexed like the others.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(folder.join("s2.jsonl"), OMP).unwrap();
        assert_eq!(sync_index(&db, &cfg), 5, "two more turns from the omp transcript");
        assert_eq!(search(&db, "компакт", 10)[0].n, 4, "the question of the newer session");
        assert_eq!(search(&db, "PLN-104", 10)[0].n, 3, "the answer about PLN-104");
        assert_eq!(page(&cfg, None, 50).total, 5);

        // The transcripts go away: the index is emptied too.
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(sync_index(&db, &cfg), 0);
        assert!(search(&db, "пиццу", 10).is_empty());
    }
}
