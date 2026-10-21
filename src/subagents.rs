//! Sub-agents of an agent session: the moons of its planet (PLN-9 plan §4.6).
//!
//! Read-only. Each program keeps its sub-agents differently:
//! - claude: `<claude>/projects/<dashed cwd>/<session>/subagents/agent-<id>.jsonl`
//!   with `agent-<id>.meta.json` beside it;
//! - omp: children `<parent without .jsonl>/<TaskName>.jsonl` (+ `<TaskName>.md`
//!   with the final result) next to the parent session file;
//! - codex: `<codex>/sessions/yyyy/mm/dd/rollout-*.jsonl`; a child names its
//!   parent in `session_meta`, and the parent logs `SubAgentActivity`;
//! - pi: no sub-agents at all.
//!
//! The state is decided by content and freshness, not by the flags the programs
//! write (`finished` is set on few records): `done` when the sub-agent ended
//! itself or its parent session ended; otherwise `running` while the file changed
//! less than [`RUNNING_MS`] ago, `quiet` after that, and not returned at all once
//! quiet for [`QUIET_DROP_MS`].

use crate::config::{home, AppConfig};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A sub-agent whose file changed less than this long ago is `running`.
pub const RUNNING_MS: u64 = 10 * 60 * 1000;
/// A `quiet` sub-agent older than this is not returned at all.
pub const QUIET_DROP_MS: u64 = 60 * 60 * 1000;
/// The list of a task or the Coordinator is capped at this many, newest first.
pub const MAX_LIST: usize = 12;
const RESULT_LIST: usize = 4000;
const RESULT_CARD: usize = 16000;
const TITLE_MAX: usize = 120;
/// How much of a session file is read from its start and its end.
const HEAD: u64 = 64 * 1024;
const TAIL: u64 = 64 * 1024;
const CODEX_TAIL: u64 = 256 * 1024;
/// Files larger than this are never parsed whole (only their tails are read).
const WHOLE_FILE_MAX: u64 = 32 * 1024 * 1024;
const WEEK_MS: u64 = 7 * 24 * 3600 * 1000;
const HERDR_TTL: Duration = Duration::from_secs(20);
const PI_NOTE: &str = "нет данных: у pi нет субагентов";

/// One sub-agent as the snapshot carries it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Subagent {
    /// Stable within the parent: the file stem or thread id.
    pub id: String,
    /// `claude`, `omp` or `codex`.
    pub kind: String,
    /// The role or agent type: `Explore`, `general-purpose`, a task name.
    pub role: String,
    /// What it was asked to do (short).
    pub title: String,
    pub started_at_ms: u64,
    pub last_at_ms: u64,
    /// `running`, `quiet` or `done`.
    pub state: String,
    /// What it is doing now (last tool call or text), short.
    pub doing: String,
    /// Its result so far / final text (≤ 4000 chars). Empty while running.
    pub result: String,
    /// An honest «нет данных: …» when the source is missing.
    pub note: String,
}

/// The session whose sub-agents are wanted.
pub struct Query<'a> {
    /// `claude`, `omp`, `pi` or `codex`.
    pub app: &'a str,
    /// Claude: the session id. omp and codex: the parent session file path
    /// (task.json `session`), or a thread id.
    pub session: &'a str,
    /// The task's worktree (or root): Claude names its project folder after it.
    pub cwd: &'a str,
    /// `sw-<repo>-<n>`, the pane name, used to find an omp session when `session` is not a path.
    pub agent_name: &'a str,
    /// The task is working, in review or in plan, and its agent has not exited.
    pub parent_active: bool,
    /// `exitedAt` of the task in ms; 0 while it has not exited.
    pub parent_exited_at_ms: u64,
}

/// The sub-agents found, and why there may be none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub list: Vec<Subagent>,
    /// Empty when the list is the whole truth; otherwise «нет данных: …».
    pub note: String,
}

/// Where the programs keep their files. Production uses [`Roots::live`]; tests
/// point every field at a temporary folder.
#[derive(Debug, Clone)]
struct Roots {
    /// Claude's config home (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    claude: PathBuf,
    /// `~/.codex/sessions`.
    codex: PathBuf,
    /// The herdr program (`HERDR_BIN` or `herdr`).
    herdr: String,
    herdr_wait: Duration,
}

impl Roots {
    fn live() -> Self {
        let codex_home = std::env::var_os("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".codex"));
        Roots {
            claude: claude_home(),
            codex: codex_home.join("sessions"),
            herdr: std::env::var("HERDR_BIN")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "herdr".to_string()),
            herdr_wait: Duration::from_secs(8),
        }
    }
}

/// The sub-agents of a task's agent, newest first (at most [`MAX_LIST`]).
pub fn for_task(q: &Query, now_ms: u64) -> Found {
    found(&Roots::live(), q, now_ms)
}

/// The Coordinator's sub-agents: the moons of Quasar. `busy` is whether the
/// Coordinator is working now (when it is not, its sub-agents are done).
pub fn for_coordinator(cfg: &AppConfig, busy: bool, now_ms: u64) -> Found {
    coordinator_in(&Roots::live(), &cfg.data_dir, busy, now_ms)
}

/// One sub-agent with its full result (up to 16000 chars). `None` when it is not
/// in the list (unknown id, or quiet for more than [`QUIET_DROP_MS`]).
pub fn card(q: &Query, id: &str, now_ms: u64) -> Option<Subagent> {
    card_in(&Roots::live(), q, id, now_ms)
}

fn card_in(r: &Roots, q: &Query, id: &str, now: u64) -> Option<Subagent> {
    collect(r, q, now, true)
        .ok()?
        .into_iter()
        .find(|s| s.id == id)
}

fn found(r: &Roots, q: &Query, now: u64) -> Found {
    match collect(r, q, now, false) {
        Ok(list) => Found {
            list,
            note: String::new(),
        },
        Err(note) => Found {
            list: Vec::new(),
            note,
        },
    }
}

fn coordinator_in(r: &Roots, data_dir: &Path, busy: bool, now: u64) -> Found {
    let coordinator = data_dir.join("coordinator").join("default");
    let kind = std::fs::read_to_string(coordinator.join("state.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| {
            v.pointer("/run/kind")
                .and_then(Value::as_str)
                .map(String::from)
        });
    if let Some(kind) = kind.filter(|k| k != "claude") {
        return Found {
            list: Vec::new(),
            note: format!("нет данных: Координатор работает на {kind}, его субагенты не читаются"),
        };
    }
    let chat = coordinator.join("chat");
    let chat_str = chat.to_string_lossy().into_owned();
    let folder = claude_projects(r).join(project_folder(&canonical(&chat_str)));
    let newest = std::fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(m, _)| *m)
        .map(|(_, p)| p);
    let Some(stem) = newest.and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
    else {
        return Found {
            list: Vec::new(),
            note: "нет данных: нет стенограммы Координатора".to_string(),
        };
    };
    let q = Query {
        app: "claude",
        session: &stem,
        cwd: &chat_str,
        agent_name: "sw-coordinator",
        parent_active: busy,
        parent_exited_at_ms: 0,
    };
    found(r, &q, now)
}

fn collect(r: &Roots, q: &Query, now: u64, full: bool) -> Result<Vec<Subagent>, String> {
    let mut list = match q.app {
        "claude" => claude_list(r, q, now, full)?,
        "omp" => omp_list(r, q, now, full)?,
        "codex" => codex_list(r, q, now, full)?,
        "pi" => return Err(PI_NOTE.to_string()),
        "" => return Err("нет данных: не указана программа".to_string()),
        other => return Err(format!("нет данных: программа «{other}» не читается")),
    };
    list.sort_by(|a, b| {
        b.last_at_ms
            .cmp(&a.last_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    if !full {
        list.truncate(MAX_LIST);
    }
    Ok(list)
}

// ---------------------------------------------------------------- claude

fn claude_list(r: &Roots, q: &Query, now: u64, full: bool) -> Result<Vec<Subagent>, String> {
    if !safe_segment(q.session) {
        return Err("нет данных: нет сессии".to_string());
    }
    let session_dir = claude_projects(r)
        .join(project_folder(&canonical(q.cwd)))
        .join(q.session);
    if !session_dir.is_dir() {
        return Err("нет данных: нет папки сессии Claude".to_string());
    }
    let dir = session_dir.join("subagents");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // A session without sub-agents has no folder for them.
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name
            .strip_prefix("agent-")
            .and_then(|s| s.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let path = entry.path();
        let meta = dir.join(format!("agent-{stem}.meta.json"));
        let Some(st) = stamp(&path) else { continue };
        let facts = cached(&path, "claude", st, || claude_facts(&path, &meta));
        let last = st.0;
        let parent_ended = ended_by_parent(q, last);
        let Some(state) = judge(facts.ended || parent_ended, last, now) else {
            continue;
        };
        let role = facts.role.clone();
        let title = facts.title.clone();
        out.push(build(
            "claude",
            format!("agent-{stem}"),
            &facts,
            role,
            title,
            last,
            state,
            full,
        ));
    }
    Ok(out)
}

/// The Claude project folder name: every character that is not an ASCII
/// letter or digit becomes `-` (the rule of history.rs).
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

fn claude_projects(r: &Roots) -> PathBuf {
    r.claude.join("projects")
}

fn claude_facts(path: &Path, meta_path: &Path) -> Facts {
    let mut f = Facts::default();
    let head = read_prefix(path, HEAD);
    f.started_ms = first_timestamp(&head);
    f.title = head_lines(&head)
        .filter(|l| l.contains("\"type\":\"user\"") && !l.contains("\"isMeta\":true"))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| user_text(v.pointer("/message/content")))
        .unwrap_or_default();
    if let Some(meta) = std::fs::read_to_string(meta_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        f.role = text_of(&meta, "agentType");
        let description = text_of(&meta, "description");
        if !description.is_empty() {
            f.title = description;
        }
        f.ended = meta.get("finished").and_then(Value::as_bool) == Some(true);
    }
    let values = tail_values(
        path,
        |l| l.contains("\"type\":\"assistant\"") || l.contains("\"type\":\"user\""),
        |vs| {
            vs.iter()
                .any(|v| v.get("type").and_then(Value::as_str) == Some("assistant"))
        },
    );
    let (ended, doing, result) = claude_end_doing_result(&values);
    f.ended |= ended;
    f.doing = doing;
    f.result = result;
    f
}

/// The end, what it does now, and its result, from the assistant and user records.
fn claude_end_doing_result(values: &[Value]) -> (bool, String, String) {
    let is = |v: &Value, t: &str| v.get("type").and_then(Value::as_str) == Some(t);
    let last_asst = values.iter().rposition(|v| is(v, "assistant"));
    // Ended: its last answer ended the turn and nothing but harness notes followed,
    // or the handback tool ended the run (a user record with `toolEndsTurn`).
    let ended = match last_asst {
        Some(i) => {
            let stop_end = values[i]
                .pointer("/message/stop_reason")
                .and_then(Value::as_str)
                == Some("end_turn");
            let after = &values[i + 1..];
            let user_after = after
                .iter()
                .any(|v| is(v, "user") && v.get("isMeta").and_then(Value::as_bool) != Some(true));
            let handed_back = after.iter().any(|v| {
                is(v, "user") && v.get("toolEndsTurn").and_then(Value::as_bool) == Some(true)
            });
            (stop_end && !user_after) || handed_back
        }
        None => false,
    };
    let mut doing = String::new();
    let mut result = String::new();
    let mut found_doing = false;
    let mut found_result = false;
    for v in values.iter().rev().filter(|v| is(v, "assistant")) {
        let blocks = v.pointer("/message/content").and_then(Value::as_array);
        let Some(blocks) = blocks else { continue };
        if !found_doing {
            if let Some(tool) = blocks.iter().rev().find(|b| block_is(b, "tool_use")) {
                doing = tool_summary(
                    &text_of(tool, "name"),
                    tool.get("input").unwrap_or(&Value::Null),
                );
                found_doing = true;
            } else if let Some(text) = last_text(blocks) {
                doing = clip(&one_line(&text), TITLE_MAX);
                found_doing = true;
            }
        }
        if !found_result {
            // The sub-agent's report: the text of its last answer, or the message it
            // handed back with the SubagentHandback tool.
            let handback = blocks
                .iter()
                .rev()
                .find(|b| block_is(b, "tool_use") && text_of(b, "name").contains("Handback"));
            let text = blocks
                .iter()
                .filter(|b| block_is(b, "text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            if !text.is_empty() {
                result = clip(&text, RESULT_CARD);
                found_result = true;
            } else if let Some(msg) = handback
                .and_then(|b| b.pointer("/input/message"))
                .and_then(Value::as_str)
            {
                result = clip(msg.trim(), RESULT_CARD);
                found_result = true;
            }
        }
        if found_doing && found_result {
            break;
        }
    }
    (ended, doing, result)
}

// ---------------------------------------------------------------- omp

fn omp_list(r: &Roots, q: &Query, now: u64, full: bool) -> Result<Vec<Subagent>, String> {
    let parent = omp_parent(r, q).ok_or_else(|| "нет данных: не найдена сессия omp".to_string())?;
    let Some(dir) = parent
        .to_string_lossy()
        .strip_suffix(".jsonl")
        .map(PathBuf::from)
    else {
        return Err("нет данных: не найдена сессия omp".to_string());
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let path = entry.path();
        let md = dir.join(format!("{stem}.md"));
        let Some(st) = stamp(&path) else { continue };
        let md_mtime = stamp(&md).map(|s| s.0).unwrap_or(0);
        let last = st.0.max(md_mtime);
        let facts = cached(&path, "omp", st, || omp_facts(&path, &md));
        let parent_ended = ended_by_parent(q, last);
        let Some(state) = judge(facts.ended || parent_ended, last, now) else {
            continue;
        };
        let title = facts.title.clone();
        out.push(build(
            "omp",
            stem.to_string(),
            &facts,
            stem.to_string(),
            title,
            last,
            state,
            full,
        ));
    }
    Ok(out)
}

/// The parent session file of an omp task. The task's `session` is the path
/// itself; otherwise herdr's pane list tells it (`agent_session{kind:"path"}`).
fn omp_parent(r: &Roots, q: &Query) -> Option<PathBuf> {
    let direct = Path::new(q.session);
    if direct.is_absolute() && direct.extension().is_some_and(|x| x == "jsonl") && direct.is_file()
    {
        return Some(direct.to_path_buf());
    }
    if q.agent_name.is_empty() {
        return None;
    }
    herdr_omp_parent(r, q.agent_name)
}

/// The omp session file of a herdr pane `agent_name`, asked from `herdr agent list`
/// (JSON on stdout). Cached for [`HERDR_TTL`]; `None` when herdr does not answer.
fn herdr_omp_parent(r: &Roots, agent_name: &str) -> Option<PathBuf> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Option<PathBuf>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{}\n{agent_name}", r.herdr);
    if let Ok(c) = cache.lock() {
        if let Some((at, found)) = c.get(&key) {
            if at.elapsed() < HERDR_TTL {
                return found.clone();
            }
        }
    }
    let found = run_herdr_agent_list(&r.herdr, r.herdr_wait).and_then(|text| {
        let v: Value = serde_json::from_str(&text).ok()?;
        let agents = v
            .pointer("/result/agents")
            .or_else(|| v.get("agents"))
            .and_then(Value::as_array)?;
        agents
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some(agent_name))
            .and_then(|a| a.get("agent_session"))
            .filter(|s| s.get("kind").and_then(Value::as_str) == Some("path"))
            .and_then(|s| s.get("value").and_then(Value::as_str))
            .map(PathBuf::from)
            .filter(|p| p.is_file())
    });
    if let Ok(mut c) = cache.lock() {
        c.insert(key, (Instant::now(), found.clone()));
    }
    found
}

/// Runs `<herdr> agent list` and returns its stdout, or `None` after `wait`.
fn run_herdr_agent_list(bin: &str, wait: Duration) -> Option<String> {
    let mut child = Command::new(bin)
        .args(["agent", "list"])
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read in a thread so a full pipe never blocks the child.
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader.join().ok()?;
                return status.success().then_some(text);
            }
            Ok(None) if started.elapsed() < wait => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn omp_facts(path: &Path, md: &Path) -> Facts {
    let mut f = Facts::default();
    let head = read_prefix(path, HEAD);
    f.started_ms = first_timestamp(&head);
    f.title = head_lines(&head)
        .filter(|l| l.contains("\"role\":\"user\""))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| user_text(v.pointer("/message/content")))
        .unwrap_or_default();
    let values = tail_values(
        path,
        |l| l.contains("\"type\":\"message\"") || l.contains("session_exit"),
        |vs| {
            vs.iter()
                .any(|v| v.get("type").and_then(Value::as_str) == Some("message"))
        },
    );
    // Finished: a session_exit record with no message after it (a resumed child has messages after its exit).
    let mut last_exit = None;
    let mut last_message = None;
    for (i, v) in values.iter().enumerate() {
        match v.get("type").and_then(Value::as_str) {
            Some("custom")
                if v.get("customType").and_then(Value::as_str) == Some("session_exit") =>
            {
                last_exit = Some(i)
            }
            Some("message") => last_message = Some(i),
            _ => {}
        }
    }
    f.ended = match last_exit {
        Some(e) => last_message.is_none_or(|m| m < e),
        None => false,
    };
    let mut doing_found = false;
    let mut result_text = String::new();
    for v in values.iter().rev() {
        if v.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        if v.pointer("/message/role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        if !doing_found {
            if let Some(call) = blocks.iter().rev().find(|b| block_is(b, "toolCall")) {
                f.doing = tool_summary(
                    &text_of(call, "name"),
                    call.get("arguments").unwrap_or(&Value::Null),
                );
                doing_found = true;
            } else if let Some(text) = last_text(blocks) {
                f.doing = clip(&one_line(&text), TITLE_MAX);
                doing_found = true;
            }
        }
        if result_text.is_empty() {
            result_text = blocks
                .iter()
                .filter(|b| block_is(b, "text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        if doing_found && !result_text.is_empty() {
            break;
        }
    }
    // The child's own final report, written when it finishes, is the better result.
    f.result = match read_prefix(md, RESULT_CARD as u64 * 4) {
        bytes if !bytes.is_empty() => {
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            if text.is_empty() {
                clip(&result_text, RESULT_CARD)
            } else {
                clip(&text, RESULT_CARD)
            }
        }
        _ => clip(&result_text, RESULT_CARD),
    };
    f
}

// ---------------------------------------------------------------- codex

fn codex_list(r: &Roots, q: &Query, now: u64, full: bool) -> Result<Vec<Subagent>, String> {
    if !r.codex.is_dir() {
        return Err("нет данных: нет сессий codex".to_string());
    }
    let files = codex_files(&r.codex);
    let session = q.session;
    let parent = {
        let direct = Path::new(session);
        if direct.is_absolute() && direct.is_file() {
            Some(direct.to_path_buf())
        } else if session.is_empty() {
            None
        } else {
            files
                .iter()
                .filter(|(p, _)| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().contains(session))
                })
                .max_by_key(|(_, m)| *m)
                .map(|(p, _)| p.clone())
        }
    };
    let Some(parent) = parent else {
        return Err("нет данных: не найдена сессия codex".to_string());
    };
    let parent_id = match stamp(&parent) {
        Some(st) => cached(&parent, "codex", st, || codex_facts(&parent)).id,
        None => String::new(),
    };
    let parent_id = if parent_id.is_empty() {
        session.to_string()
    } else {
        parent_id
    };

    // Children: the ids the parent logged as started, and recent rollouts whose
    // session_meta names this parent.
    let mut children: Vec<PathBuf> = Vec::new();
    if let Some(st) = stamp(&parent) {
        let ids = cached(&parent, "codex-ids", st, || Facts {
            ids: started_ids(&parent),
            ..Facts::default()
        })
        .ids;
        for id in ids {
            if let Some((p, _)) = files.iter().find(|(p, _)| p != &parent && file_has(p, &id)) {
                children.push(p.clone());
            }
        }
    }
    for (p, mtime) in &files {
        if p == &parent || children.contains(p) || mtime + WEEK_MS < now {
            continue;
        }
        if let Some(st) = stamp(p) {
            if cached(p, "codex", st, || codex_facts(p)).parent == parent_id {
                children.push(p.clone());
            }
        }
    }

    let mut out = Vec::new();
    for child in children {
        let Some(st) = stamp(&child) else { continue };
        let facts = cached(&child, "codex", st, || codex_facts(&child));
        let last = st.0;
        let parent_ended = ended_by_parent(q, last);
        let Some(state) = judge(facts.ended || parent_ended, last, now) else {
            continue;
        };
        let id = if facts.id.is_empty() {
            child
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            facts.id.clone()
        };
        let role = facts.role.clone();
        let title = facts.title.clone();
        out.push(build("codex", id, &facts, role, title, last, state, full));
    }
    Ok(out)
}

/// Every `rollout-*.jsonl` under `<root>/yyyy/mm/dd/`, with its modification time.
fn codex_files(root: &Path) -> Vec<(PathBuf, u64)> {
    fn walk(dir: &Path, depth: u8, out: &mut Vec<(PathBuf, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if depth < 3 {
                    walk(&path, depth + 1, out);
                }
            } else if meta.is_file() {
                let rollout = path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("rollout-"));
                if rollout && path.extension().is_some_and(|x| x == "jsonl") {
                    out.push((path, ms_of(meta.modified().unwrap_or(UNIX_EPOCH))));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

fn file_has(path: &Path, needle: &str) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().contains(needle))
}

fn codex_facts(path: &Path) -> Facts {
    let mut f = Facts::default();
    let head = read_prefix(path, HEAD);
    f.started_ms = first_timestamp(&head);
    for line in head_lines(&head).filter(|l| l.contains("session_meta")) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = v.get("payload").unwrap_or(&Value::Null);
        f.id = text_of(payload, "id");
        f.parent = text_of(payload, "parent_thread_id");
        f.role = {
            let role = text_of(payload, "agent_role");
            if role.is_empty() {
                text_of(payload, "agent_nickname")
            } else {
                role
            }
        };
        if let Some(t) = v
            .get("timestamp")
            .and_then(Value::as_str)
            .map(parse_time)
            .filter(|t| *t > 0)
        {
            f.started_ms = t;
        }
        break;
    }
    f.title = head_lines(&head)
        .filter(|l| l.contains("user_message") || l.contains("\"role\":\"user\""))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| {
            let p = v.get("payload")?;
            if p.get("type").and_then(Value::as_str) == Some("user_message") {
                return p
                    .get("message")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().to_string());
            }
            user_text(p.get("content"))
        })
        .map(|t| clip(&one_line(&t), TITLE_MAX))
        .unwrap_or_default();

    let values = tail_values(path, |_| true, |vs| !vs.is_empty());
    let kind_of = |v: &Value| -> String {
        v.pointer("/payload/type")
            .or_else(|| v.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    // The last turn event decides: a turn that was started and not closed is still working.
    f.ended = values
        .iter()
        .rev()
        .map(kind_of)
        .find(|k| {
            matches!(
                k.as_str(),
                "task_started"
                    | "turn_started"
                    | "task_complete"
                    | "turn_complete"
                    | "turn_aborted"
                    | "task_aborted"
            )
        })
        .is_some_and(|k| k.ends_with("complete") || k.ends_with("aborted"));
    let mut doing_found = false;
    let mut result_found = false;
    for v in values.iter().rev() {
        let kind = kind_of(v);
        let payload = v.get("payload").unwrap_or(&Value::Null);
        if !doing_found && kind == "function_call" {
            let args = payload.get("arguments").map(|a| match a {
                Value::String(s) => {
                    serde_json::from_str::<Value>(s).unwrap_or(Value::String(s.clone()))
                }
                other => other.clone(),
            });
            f.doing = tool_summary(&text_of(payload, "name"), &args.unwrap_or(Value::Null));
            doing_found = true;
        }
        if !result_found {
            let message = if kind == "agent_message" {
                payload
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else if kind == "message"
                && payload.get("role").and_then(Value::as_str) == Some("assistant")
            {
                user_text(payload.get("content")).or_else(|| assistant_text(payload.get("content")))
            } else {
                None
            };
            if let Some(text) = message
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
            {
                f.result = clip(&text, RESULT_CARD);
                result_found = true;
                if !doing_found {
                    f.doing = clip(&one_line(&text), TITLE_MAX);
                    doing_found = true;
                }
            }
        }
        if doing_found && result_found {
            break;
        }
    }
    f
}

/// The ids of the sub-agents a codex parent logged as started (`SubAgentActivity`,
/// `kind:"started"`, `agent_thread_id`), read from the whole file.
fn started_ids(path: &Path) -> Vec<String> {
    fn find(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                if map.get("kind").and_then(Value::as_str) == Some("started") {
                    if let Some(id) = map.get("agent_thread_id").and_then(Value::as_str) {
                        if !out.iter().any(|x| x == id) {
                            out.push(id.to_string());
                        }
                    }
                }
                map.values().for_each(|x| find(x, out));
            }
            Value::Array(items) => items.iter().for_each(|x| find(x, out)),
            _ => {}
        }
    }
    let mut ids = Vec::new();
    let Some(text) = read_whole(path) else {
        return ids;
    };
    for line in text.lines().filter(|l| l.contains("SubAgentActivity")) {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            find(&v, &mut ids);
        }
    }
    ids
}

// ---------------------------------------------------------------- shared

/// What is known about one sub-agent file, cached by (path, mtime, len).
#[derive(Debug, Clone, Default)]
struct Facts {
    /// Codex: the thread id; Claude and omp: unused.
    id: String,
    /// Codex child: the parent thread id.
    parent: String,
    /// Codex parent: the started children's thread ids.
    ids: Vec<String>,
    started_ms: u64,
    role: String,
    title: String,
    /// The program wrote its own end.
    ended: bool,
    doing: String,
    /// The result so far, clipped to [`RESULT_CARD`].
    result: String,
}

type CacheKey = (PathBuf, &'static str);
type CacheVal = ((u64, u64), Facts);

fn cache() -> &'static Mutex<HashMap<CacheKey, CacheVal>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, CacheVal>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The facts of `path`, computed again only when its (mtime, len) changed.
fn cached(
    path: &Path,
    tag: &'static str,
    st: (u64, u64),
    compute: impl FnOnce() -> Facts,
) -> Facts {
    let key = (path.to_path_buf(), tag);
    if let Ok(c) = cache().lock() {
        if let Some((stamp, facts)) = c.get(&key) {
            if *stamp == st {
                return facts.clone();
            }
        }
    }
    let facts = compute();
    if let Ok(mut c) = cache().lock() {
        c.insert(key, (st, facts.clone()));
    }
    facts
}

/// (modification time in ms, length) of a regular file.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some((ms_of(meta.modified().unwrap_or(UNIX_EPOCH)), meta.len()))
}

fn ms_of(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The parent task has ended: it is not active any more, or it exited after the
/// sub-agent last wrote.
fn ended_by_parent(q: &Query, last_ms: u64) -> bool {
    !q.parent_active || (q.parent_exited_at_ms > 0 && q.parent_exited_at_ms > last_ms)
}

/// `done` when ended; otherwise by age; `None` when quiet for too long.
fn judge(ended: bool, last_ms: u64, now: u64) -> Option<&'static str> {
    if ended {
        return Some("done");
    }
    let age = now.saturating_sub(last_ms);
    if age < RUNNING_MS {
        Some("running")
    } else if age < QUIET_DROP_MS {
        Some("quiet")
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn build(
    kind: &str,
    id: String,
    facts: &Facts,
    role: String,
    title: String,
    last_ms: u64,
    state: &str,
    full: bool,
) -> Subagent {
    let limit = if full { RESULT_CARD } else { RESULT_LIST };
    Subagent {
        id,
        kind: kind.to_string(),
        role,
        title: clip(&one_line(&title), TITLE_MAX),
        started_at_ms: if facts.started_ms > 0 {
            facts.started_ms
        } else {
            last_ms
        },
        last_at_ms: last_ms,
        state: state.to_string(),
        doing: facts.doing.clone(),
        // A running sub-agent has no result yet.
        result: if state == "running" {
            String::new()
        } else {
            clip(&facts.result, limit)
        },
        note: String::new(),
    }
}

/// A path segment from an id: no separators, no `..`, not empty.
fn safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

fn canonical(cwd: &str) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd))
}

/// The first `max` bytes of a file (empty when unreadable).
fn read_prefix(path: &Path, max: u64) -> Vec<u8> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = file.take(max).read_to_end(&mut buf);
    buf
}

/// The whole file, or `None` when it is unreadable or larger than [`WHOLE_FILE_MAX`].
fn read_whole(path: &Path) -> Option<String> {
    let len = std::fs::metadata(path).ok()?.len();
    if len > WHOLE_FILE_MAX {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// The last `max` bytes of a file, from the first complete line in them.
fn read_suffix(path: &Path, max: u64) -> Vec<u8> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let start = len.saturating_sub(max);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let _ = file.read_to_end(&mut buf);
    if start > 0 {
        // The first line of the read may be cut: drop it, or everything when there is no line end.
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                buf.drain(..=i);
            }
            None => buf.clear(),
        }
    }
    buf
}

/// The complete lines of a head buffer: a last line cut by the read is dropped
/// only when the buffer is full (so the read was not the whole file).
fn head_lines(buf: &[u8]) -> impl Iterator<Item = &str> {
    let end = if buf.len() as u64 >= HEAD {
        buf.iter().rposition(|&b| b == b'\n').unwrap_or(0)
    } else {
        buf.len()
    };
    std::str::from_utf8(&buf[..end])
        .unwrap_or_else(|e| std::str::from_utf8(&buf[..e.valid_up_to()]).unwrap_or(""))
        .lines()
}

/// The parsed records of a file's end that pass `keep`. When the tail holds nothing
/// that `enough` accepts, the whole file (if small enough) is read instead.
fn tail_values(
    path: &Path,
    keep: impl Fn(&str) -> bool,
    enough: impl Fn(&[Value]) -> bool,
) -> Vec<Value> {
    let parse = |bytes: &[u8]| -> Vec<Value> {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|l| keep(l))
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    };
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let is_codex = path.to_string_lossy().contains("rollout-");
    let max = if is_codex { CODEX_TAIL } else { TAIL };
    let values = parse(&read_suffix(path, max));
    if enough(&values) || len <= max || len > WHOLE_FILE_MAX {
        return values;
    }
    parse(&read_prefix(path, WHOLE_FILE_MAX))
}

/// `"timestamp":"2026-10-08T13:51:52.958Z"` → ms; the first one in the buffer.
fn first_timestamp(buf: &[u8]) -> u64 {
    let key = b"\"timestamp\":\"";
    let Some(pos) = buf.windows(key.len()).position(|w| w == key) else {
        return 0;
    };
    let start = pos + key.len();
    let end = (start + 24).min(buf.len());
    std::str::from_utf8(&buf[start..end])
        .map(parse_time)
        .unwrap_or(0)
}

/// `2026-10-19T05:56:05.628Z` → milliseconds since the epoch (0 when not a time).
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
    // Days from civil (Howard Hinnant), as in history.rs.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + se;
    (secs * 1000 + ms).max(0) as u64
}

fn block_is(b: &Value, kind: &str) -> bool {
    b.get("type").and_then(Value::as_str) == Some(kind)
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The last non-empty text block of a content list.
fn last_text(blocks: &[Value]) -> Option<String> {
    blocks
        .iter()
        .rev()
        .filter(|b| block_is(b, "text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .map(str::trim)
        .find(|t| !t.is_empty())
        .map(str::to_string)
}

/// The text of an assistant content value (a list of text blocks or a string).
fn assistant_text(content: Option<&Value>) -> Option<String> {
    let text = match content? {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The words a user typed: a plain string or the text blocks of a list; `None`
/// for tool results and the harness's own wrappers (text starting with `<`).
fn user_text(content: Option<&Value>) -> Option<String> {
    let text = match content? {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            if blocks.iter().any(|b| block_is(b, "tool_result")) {
                return None;
            }
            blocks
                .iter()
                .filter(|b| block_is(b, "text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| text.to_string())
}

/// What a tool call is doing, in a few words: the tool name and its most telling argument.
fn tool_summary(name: &str, input: &Value) -> String {
    // omp's `intent` (also `i`) says what the call is for, so it comes first.
    const KEYS: [&str; 13] = [
        "intent",
        "i",
        "command",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "description",
        "prompt",
        "subagent_type",
        "message",
        "name",
    ];
    let arg = KEYS.iter().find_map(|k| {
        let v = input.get(k)?;
        let text = match v {
            Value::String(s) => s.trim().to_string(),
            // A command given as an argument list, e.g. ["ls", "-la"].
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    });
    let name = if name.is_empty() { "tool" } else { name };
    match arg {
        Some(a) => clip(&format!("{name}: {}", one_line(&a)), 160),
        None => name.to_string(),
    }
}

/// Whitespace collapsed to single spaces.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters; `…` marks a cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_792_400_000_000;
    /// 2026-10-20T00:00:00Z in ms, and one hour.
    const DAY_1020: u64 = 1_792_454_400_000;
    const HOUR: u64 = 3_600_000;

    /// Writes `text` at an absolute path (its folder is created).
    fn put(path: &Path, text: &str) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        path.to_path_buf()
    }

    /// A throw-away `~` with the four program folders.
    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Fixture {
                dir: tempfile::tempdir().unwrap(),
            }
        }
        fn roots(&self) -> Roots {
            Roots {
                claude: self.dir.path().join("claude"),
                codex: self.dir.path().join("codex"),
                herdr: "/definitely/not/herdr".to_string(),
                herdr_wait: Duration::from_millis(300),
            }
        }
        fn write(&self, rel: &str, text: impl AsRef<str>) -> PathBuf {
            let path = self.dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text.as_ref()).unwrap();
            path
        }
        fn at(&self, path: &Path, ms: u64) {
            let file = std::fs::File::options().write(true).open(path).unwrap();
            file.set_modified(UNIX_EPOCH + Duration::from_millis(ms))
                .unwrap();
        }
    }

    fn query<'a>(app: &'a str, session: &'a str, cwd: &'a str) -> Query<'a> {
        Query {
            app,
            session,
            cwd,
            agent_name: "sw-test-1",
            parent_active: true,
            parent_exited_at_ms: 0,
        }
    }

    const CWD: &str = "/work/proj-1";
    /// The Claude project folder of [`CWD`] (it does not exist on disk, so it is not canonicalised).
    const FOLDER: &str = "-work-proj-1";
    const SESSION: &str = "11111111-2222-3333-4444-555555555555";

    fn claude_dir(fx: &Fixture) -> PathBuf {
        let dir = fx
            .dir
            .path()
            .join("claude/projects")
            .join(FOLDER)
            .join(SESSION)
            .join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A claude sub-agent file: a start, a tool call and an answer.
    fn claude_jsonl(stop: Option<&str>) -> String {
        let end = match stop {
            Some(s) => format!(r#","stop_reason":"{s}""#),
            None => String::new(),
        };
        [
            r#"{"parentUuid":null,"isSidechain":true,"type":"user","timestamp":"2026-10-20T10:00:00.000Z","message":{"role":"user","content":"Изучи модуль подписок"}}"#.to_string(),
            r#"{"type":"assistant","timestamp":"2026-10-20T10:00:05.000Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test -p subs"}}]}}"#.to_string(),
            r#"{"type":"user","timestamp":"2026-10-20T10:00:09.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#.to_string(),
            format!(r#"{{"type":"assistant","timestamp":"2026-10-20T10:00:20.000Z","message":{{"content":[{{"type":"text","text":"Готово: 3 файла проверены."}}]{end}}}}}"#),
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn claude_finished_by_meta_flag() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let f = dir.join("agent-aaa1.jsonl");
        std::fs::write(&f, claude_jsonl(None)).unwrap();
        std::fs::write(
            dir.join("agent-aaa1.meta.json"),
            r#"{"agentType":"Explore","description":"Разобрать подписки","finished":true}"#,
        )
        .unwrap();
        fx.at(&f, NOW - 60_000);
        let found = found(&fx.roots(), &query("claude", SESSION, CWD), NOW);
        assert_eq!(found.note, "");
        let s = &found.list[0];
        assert_eq!(s.id, "agent-aaa1");
        assert_eq!(
            (s.kind.as_str(), s.role.as_str(), s.state.as_str()),
            ("claude", "Explore", "done")
        );
        assert_eq!(s.title, "Разобрать подписки");
        assert_eq!(s.result, "Готово: 3 файла проверены.");
        assert_eq!(
            s.started_at_ms,
            DAY_1020 + 10 * HOUR,
            "2026-10-20T10:00:00Z"
        );
    }

    #[test]
    fn claude_finished_by_end_turn_without_flag() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let f = dir.join("agent-bbb2.jsonl");
        std::fs::write(&f, claude_jsonl(Some("end_turn"))).unwrap();
        fx.at(&f, NOW - 30 * 60_000);
        let s = &found(&fx.roots(), &query("claude", SESSION, CWD), NOW).list[0];
        assert_eq!(s.state, "done", "end_turn with no user record after it");
        assert_eq!(
            s.title, "Изучи модуль подписок",
            "no meta: the first user words are the title"
        );
        assert_eq!(s.doing, "Готово: 3 файла проверены.");
    }

    #[test]
    fn claude_handback_tool_ends_the_run() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let lines = [
            r#"{"type":"user","timestamp":"2026-10-20T10:00:00.000Z","message":{"role":"user","content":"задача"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-20T10:00:10.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Посмотрел."}]}}"#,
            r#"{"type":"user","isMeta":true,"timestamp":"2026-10-20T10:00:11.000Z","message":{"role":"user","content":"[handback-send-enforce] call SubagentHandback"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-20T10:00:20.000Z","message":{"content":[{"type":"tool_use","name":"SubagentHandback","input":{"message":"Отчёт: всё на месте."}}]}}"#,
            r#"{"type":"user","toolEndsTurn":true,"timestamp":"2026-10-20T10:00:21.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#,
        ]
        .join("\n");
        let f = dir.join("agent-hhh.jsonl");
        std::fs::write(&f, lines).unwrap();
        fx.at(&f, NOW - 1_000);
        let s = &found(&fx.roots(), &query("claude", SESSION, CWD), NOW).list[0];
        assert_eq!(s.state, "done");
        assert_eq!(
            s.result, "Отчёт: всё на месте.",
            "the handed-back report is the result"
        );
    }

    #[test]
    fn claude_running_quiet_and_dropped_by_age() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let fresh = dir.join("agent-run.jsonl");
        std::fs::write(&fresh, claude_jsonl(None)).unwrap();
        fx.at(&fresh, NOW - 2 * 60_000);
        let quiet = dir.join("agent-qui.jsonl");
        std::fs::write(&quiet, claude_jsonl(None)).unwrap();
        fx.at(&quiet, NOW - 20 * 60_000);
        let old = dir.join("agent-old.jsonl");
        std::fs::write(&old, claude_jsonl(None)).unwrap();
        fx.at(&old, NOW - 61 * 60_000);
        let found = found(&fx.roots(), &query("claude", SESSION, CWD), NOW);
        let by_id = |id: &str| found.list.iter().find(|s| s.id == id).cloned();
        let run = by_id("agent-run").unwrap();
        assert_eq!(run.state, "running");
        assert_eq!(run.result, "", "a running sub-agent has no result yet");
        assert_eq!(run.doing, "Готово: 3 файла проверены.", "its last words");
        assert_eq!(by_id("agent-qui").unwrap().state, "quiet");
        assert!(
            by_id("agent-old").is_none(),
            "quiet for more than an hour is dropped"
        );
        assert_eq!(found.list[0].id, "agent-run", "newest first");
        assert!(card_in(
            &fx.roots(),
            &query("claude", SESSION, CWD),
            "agent-old",
            NOW
        )
        .is_none());
        assert!(card_in(
            &fx.roots(),
            &query("claude", SESSION, CWD),
            "agent-qui",
            NOW
        )
        .is_some());
    }

    #[test]
    fn parent_exit_makes_every_sub_agent_done() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let f = dir.join("agent-pex.jsonl");
        std::fs::write(&f, claude_jsonl(None)).unwrap();
        fx.at(&f, NOW - 1_000);
        let mut q = query("claude", SESSION, CWD);
        q.parent_active = false;
        assert_eq!(
            found(&fx.roots(), &q, NOW).list[0].state,
            "done",
            "parent not active"
        );
        let mut q = query("claude", SESSION, CWD);
        q.parent_exited_at_ms = NOW - 2_000;
        assert_eq!(
            found(&fx.roots(), &q, NOW).list[0].state,
            "running",
            "exited before the last write (NOW-1000)"
        );
        q.parent_exited_at_ms = NOW - 500;
        assert_eq!(
            found(&fx.roots(), &q, NOW).list[0].state,
            "done",
            "exited after the last write"
        );
    }

    #[test]
    fn claude_missing_folders_give_a_note_and_no_panic() {
        let fx = Fixture::new();
        let f = found(&fx.roots(), &query("claude", SESSION, CWD), NOW);
        assert!(f.list.is_empty());
        assert_eq!(f.note, "нет данных: нет папки сессии Claude");
        let f = found(&fx.roots(), &query("claude", "../etc", CWD), NOW);
        assert_eq!(f.note, "нет данных: нет сессии");
        // The session folder exists, the sub-agents folder does not: no sub-agents, no note.
        std::fs::create_dir_all(
            fx.dir
                .path()
                .join("claude/projects")
                .join(FOLDER)
                .join(SESSION),
        )
        .unwrap();
        let f = found(&fx.roots(), &query("claude", SESSION, CWD), NOW);
        assert!(f.list.is_empty());
        assert_eq!(f.note, "");
    }

    #[test]
    fn garbage_lines_are_skipped() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let f = dir.join("agent-gar.jsonl");
        let text = format!(
            "not json at all\n{{\"type\":\"assistant\",\"timestamp\":\"2026-1\n{}\n\u{0}\u{1}{{{{\n[1,2\n",
            claude_jsonl(Some("end_turn")).trim_end()
        );
        std::fs::write(&f, text).unwrap();
        fx.at(&f, NOW - 1_000);
        let found = found(&fx.roots(), &query("claude", SESSION, CWD), NOW);
        assert_eq!(found.list.len(), 1);
        assert_eq!(found.list[0].state, "done");
        assert_eq!(found.list[0].result, "Готово: 3 файла проверены.");
    }

    #[test]
    fn list_is_capped_and_newest_first() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        for i in 0..15 {
            let f = dir.join(format!("agent-n{i:02}.jsonl"));
            std::fs::write(&f, claude_jsonl(Some("end_turn"))).unwrap();
            fx.at(&f, NOW - (i as u64 + 1) * 1_000);
        }
        let found = found(&fx.roots(), &query("claude", SESSION, CWD), NOW);
        assert_eq!(found.list.len(), MAX_LIST);
        assert_eq!(found.list[0].id, "agent-n00");
        let card = card_in(
            &fx.roots(),
            &query("claude", SESSION, CWD),
            "agent-n14",
            NOW,
        );
        assert!(card.is_some(), "the card looks beyond the list cap");
    }

    #[test]
    fn card_has_the_long_result() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let long = "я".repeat(20_000);
        let text = format!(
            "{}\n{}\n",
            r#"{"type":"user","timestamp":"2026-10-20T10:00:00.000Z","message":{"role":"user","content":"x"}}"#,
            format!(
                r#"{{"type":"assistant","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"{long}"}}]}}}}"#
            )
        );
        let f = dir.join("agent-long.jsonl");
        std::fs::write(&f, text).unwrap();
        fx.at(&f, NOW - 1_000);
        let facts = claude_facts(&f, &dir.join("agent-long.meta.json"));
        assert_eq!(
            facts.result.chars().count(),
            RESULT_CARD + 1,
            "16000 characters and the cut mark"
        );
        let q = query("claude", SESSION, CWD);
        let listed = found(&fx.roots(), &q, NOW)
            .list
            .into_iter()
            .find(|s| s.id == "agent-long")
            .unwrap();
        assert_eq!(
            listed.result.chars().count(),
            RESULT_LIST + 1,
            "the list gets 4000 characters"
        );
        let card = card_in(&fx.roots(), &q, "agent-long", NOW).unwrap();
        assert_eq!(
            card.result.chars().count(),
            RESULT_CARD + 1,
            "the card gets 16000"
        );
    }

    #[test]
    fn big_file_is_read_from_its_tail() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let mut text = String::from(
            "{\"type\":\"user\",\"timestamp\":\"2026-10-20T09:00:00.000Z\",\"message\":{\"role\":\"user\",\"content\":\"начало\"}}\n",
        );
        let filler = r#"{"type":"attachment","attachment":{"type":"x","pad":"#.to_string()
            + &"z".repeat(900)
            + "}}\n";
        while text.len() < 2 * 1024 * 1024 {
            text.push_str(&filler);
        }
        text.push_str(&claude_jsonl(Some("end_turn")));
        let f = dir.join("agent-big.jsonl");
        std::fs::write(&f, text).unwrap();
        fx.at(&f, NOW - 1_000);
        let s = &found(&fx.roots(), &query("claude", SESSION, CWD), NOW).list[0];
        assert_eq!(s.state, "done");
        assert_eq!(
            s.started_at_ms,
            DAY_1020 + 9 * HOUR,
            "the start is the first record"
        );
    }

    #[test]
    fn cache_is_refreshed_when_the_file_changes() {
        let fx = Fixture::new();
        let dir = claude_dir(&fx);
        let f = dir.join("agent-cch.jsonl");
        std::fs::write(&f, claude_jsonl(None)).unwrap();
        fx.at(&f, NOW - 1_000);
        let q = query("claude", SESSION, CWD);
        assert_eq!(found(&fx.roots(), &q, NOW).list[0].state, "running");
        std::fs::write(&f, claude_jsonl(Some("end_turn"))).unwrap();
        fx.at(&f, NOW - 500);
        assert_eq!(found(&fx.roots(), &q, NOW).list[0].state, "done");
    }

    #[test]
    fn pi_has_no_sub_agents() {
        let fx = Fixture::new();
        let f = found(&fx.roots(), &query("pi", SESSION, CWD), NOW);
        assert!(f.list.is_empty());
        assert_eq!(f.note, "нет данных: у pi нет субагентов");
    }

    #[test]
    fn omp_child_with_session_exit_and_md_result() {
        let fx = Fixture::new();
        let parent = fx.write(
            "omp/-work-proj-1/2026-10-20T09-00-00-000Z_01a1.jsonl",
            "{\"type\":\"session\"}\n",
        );
        let child_dir = parent.with_extension("");
        let head = concat!(
            "{\"type\":\"title\",\"v\":1,\"title\":\"\"}\n",
            "{\"type\":\"session\",\"version\":3,\"id\":\"c1\",\"timestamp\":\"2026-10-20T09:30:00.000Z\",\"cwd\":\"/tmp\",\"parentSession\":\"/x.jsonl\"}\n",
            "{\"type\":\"message\",\"id\":\"u1\",\"timestamp\":\"2026-10-20T09:30:01.000Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Проверь миграции\"}]}}\n",
            "{\"type\":\"message\",\"id\":\"a1\",\"timestamp\":\"2026-10-20T09:30:05.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"c\",\"name\":\"read\",\"arguments\":{\"i\":\"Читаю схему\",\"path\":\"/a.sql\"}}]}}\n",
            "{\"type\":\"message\",\"id\":\"t1\",\"timestamp\":\"2026-10-20T09:30:06.000Z\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"c\",\"toolName\":\"read\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n",
            "{\"type\":\"message\",\"id\":\"a2\",\"timestamp\":\"2026-10-20T09:31:00.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Миграции в порядке.\"}]}}\n",
            "{\"type\":\"custom\",\"customType\":\"session_exit\",\"data\":{\"reason\":\"dispose\"},\"id\":\"e\",\"timestamp\":\"2026-10-20T09:31:02.000Z\"}\n",
        );
        let child = put(&child_dir.join("Migrations.jsonl"), head);
        let md = put(&child_dir.join("Migrations.md"), "## Итог\nВсё чисто.");
        fx.at(&child, NOW - 90 * 60_000);
        fx.at(&md, NOW - 89 * 60_000);
        let parent_path = parent.to_string_lossy().into_owned();
        let q = query("omp", &parent_path, CWD);
        let s = &found(&fx.roots(), &q, NOW).list[0];
        assert_eq!(s.id, "Migrations");
        assert_eq!(
            (s.kind.as_str(), s.role.as_str(), s.state.as_str()),
            ("omp", "Migrations", "done")
        );
        assert_eq!(s.title, "Проверь миграции");
        assert_eq!(
            s.result, "## Итог\nВсё чисто.",
            "the .md file is the result"
        );
        assert_eq!(
            s.started_at_ms,
            DAY_1020 + 9 * HOUR + 30 * 60_000,
            "2026-10-20T09:30:00Z"
        );
        assert_eq!(
            s.doing, "Миграции в порядке.",
            "the last words of the child"
        );
        let c = card_in(&fx.roots(), &q, "Migrations", NOW).unwrap();
        assert_eq!(c.result, s.result);
    }

    #[test]
    fn omp_tool_call_says_its_intent() {
        let fx = Fixture::new();
        let parent = fx.write("omp/-p/2026-10-20T09-00-00-000Z_i.jsonl", "{}\n");
        let dir = parent.with_extension("");
        let child = put(
            &dir.join("Read.jsonl"),
            "{\"type\":\"session\",\"timestamp\":\"2026-10-20T09:30:00.000Z\"}\n{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"read\",\"arguments\":{\"i\":\"Читаю схему\",\"path\":\"/a.sql\"}}]}}\n",
        );
        fx.at(&child, NOW - 1_000);
        let parent_path = parent.to_string_lossy().into_owned();
        let s = &found(&fx.roots(), &query("omp", &parent_path, CWD), NOW).list[0];
        assert_eq!(s.doing, "read: Читаю схему");
    }

    #[test]
    fn omp_running_child_without_exit() {
        let fx = Fixture::new();
        let parent = fx.write("omp/-p/2026-10-20T09-00-00-000Z_b.jsonl", "{}\n");
        let dir = parent.with_extension("");
        let child = put(
            &dir.join("Scan.jsonl"),
            "{\"type\":\"session\",\"timestamp\":\"2026-10-20T09:30:00.000Z\"}\n{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"bash\",\"arguments\":{\"command\":\"make\"}}]}}\n",
        );
        fx.at(&child, NOW - 60_000);
        let parent_path = parent.to_string_lossy().into_owned();
        let q2 = query("omp", &parent_path, CWD);
        let s = &found(&fx.roots(), &q2, NOW).list[0];
        assert_eq!(s.state, "running");
        assert_eq!(s.doing, "bash: make");
        assert_eq!(s.result, "", "no .md and no text yet");
        // The parent ended: the same child is done.
        let mut q3 = query("omp", &parent_path, CWD);
        q3.parent_active = false;
        assert_eq!(found(&fx.roots(), &q3, NOW).list[0].state, "done");
        // The task's `session` is the parent path, so herdr is never asked for it.
        assert_eq!(omp_parent(&fx.roots(), &q2), Some(parent.clone()));
    }

    #[test]
    fn omp_parent_found_through_herdr_agent_list() {
        let fx = Fixture::new();
        let parent = fx.write("omp/-p/2026-10-20T09-00-00-000Z_h.jsonl", "{}\n");
        let name = format!("sw-herdr-test-{}", std::process::id());
        let fake = fx.dir.path().join("fake-herdr");
        let json = format!(
            r#"{{"id":"x","result":{{"agents":[{{"name":"{name}","agent":"omp","agent_session":{{"kind":"path","value":"{}"}}}}]}}}}"#,
            parent.display()
        );
        crate::testutil::write_exec(&fake, format!("#!/bin/sh\ncat <<'EOF'\n{json}\nEOF\n"));
        let mut r = fx.roots();
        r.herdr = fake.to_string_lossy().into_owned();
        assert_eq!(herdr_omp_parent(&r, &name), Some(parent.clone()));
        let q = Query {
            app: "omp",
            session: "",
            cwd: CWD,
            agent_name: &name,
            parent_active: true,
            parent_exited_at_ms: 0,
        };
        // No child folder yet: no sub-agents and no note.
        let f = found(&r, &q, NOW);
        assert!(f.list.is_empty());
        assert_eq!(f.note, "");
    }

    #[test]
    fn herdr_that_never_answers_is_given_up() {
        let fx = Fixture::new();
        let fake = fx.dir.path().join("slow-herdr");
        crate::testutil::write_exec(&fake, "#!/bin/sh\nsleep 5\n");
        let mut r = fx.roots();
        r.herdr = fake.to_string_lossy().into_owned();
        r.herdr_wait = Duration::from_millis(200);
        let started = Instant::now();
        assert!(run_herdr_agent_list(&r.herdr, r.herdr_wait).is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn omp_without_a_parent_gives_a_note() {
        let fx = Fixture::new();
        let f = found(&fx.roots(), &query("omp", "", CWD), NOW);
        assert_eq!(f.note, "нет данных: не найдена сессия omp");
    }

    /// A codex parent and its child, shaped like rollouts (no real codex data here).
    fn codex_fixture(fx: &Fixture, child_done: bool) -> (PathBuf, PathBuf) {
        let parent_id = "019f0000-aaaa-7000-8000-000000000001";
        let child_id = "019f0000-bbbb-7000-8000-000000000002";
        let parent = fx.write(
            &format!("codex/2026/10/20/rollout-2026-10-20T09-00-00-{parent_id}.jsonl"),
            [
                format!(r#"{{"timestamp":"2026-10-20T09:00:00.000Z","type":"session_meta","payload":{{"id":"{parent_id}","timestamp":"2026-10-20T09:00:00.000Z","cwd":"{CWD}"}}}}"#),
                r#"{"timestamp":"2026-10-20T09:00:05.000Z","type":"event_msg","payload":{"type":"task_started"}}"#.to_string(),
                format!(r#"{{"timestamp":"2026-10-20T09:00:09.000Z","type":"event_msg","payload":{{"type":"SubAgentActivity","kind":"started","agent_thread_id":"{child_id}"}}}}"#),
                r#"{"timestamp":"2026-10-20T09:00:10.000Z","type":"event_msg","payload":{"type":"task_started"}}"#.to_string(),
            ]
            .join("\n")
                + "\n",
        );
        let end = if child_done {
            r#"{"timestamp":"2026-10-20T09:20:00.000Z","type":"event_msg","payload":{"type":"agent_message","message":"Разбор готов: 4 риска."}}
{"timestamp":"2026-10-20T09:20:01.000Z","type":"event_msg","payload":{"type":"task_complete"}}"#
        } else {
            r#"{"timestamp":"2026-10-20T09:20:01.000Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#
        };
        let child = fx.write(
            &format!("codex/2026/10/20/rollout-2026-10-20T09-00-20-{child_id}.jsonl"),
            [
                format!(r#"{{"timestamp":"2026-10-20T09:00:20.000Z","type":"session_meta","payload":{{"id":"{child_id}","timestamp":"2026-10-20T09:00:20.000Z","cwd":"{CWD}","parent_thread_id":"{parent_id}","agent_role":"explorer"}}}}"#),
                r#"{"timestamp":"2026-10-20T09:00:21.000Z","type":"event_msg","payload":{"type":"user_message","message":"Разбери риски миграции"}}"#.to_string(),
                r#"{"timestamp":"2026-10-20T09:00:22.000Z","type":"event_msg","payload":{"type":"task_started"}}"#.to_string(),
                end.to_string(),
            ]
            .join("\n")
                + "\n",
        );
        (parent, child)
    }

    #[test]
    fn codex_parent_lists_its_children() {
        let fx = Fixture::new();
        let (parent, child) = codex_fixture(&fx, false);
        fx.at(&child, NOW - 60_000);
        fx.at(&parent, NOW - 60_000);
        let parent_path = parent.to_string_lossy().into_owned();
        let f = found(&fx.roots(), &query("codex", &parent_path, CWD), NOW);
        assert_eq!(f.note, "");
        let s = &f.list[0];
        assert_eq!((s.kind.as_str(), s.role.as_str()), ("codex", "explorer"));
        assert_eq!(s.id, "019f0000-bbbb-7000-8000-000000000002");
        assert_eq!(s.title, "Разбери риски миграции");
        assert_eq!(s.state, "running", "its turn is open and it is fresh");
        assert_eq!(s.doing, "shell: ls");
        assert_eq!(s.result, "");
    }

    #[test]
    fn codex_child_that_completed_is_done_with_its_last_message() {
        let fx = Fixture::new();
        let (parent, child) = codex_fixture(&fx, true);
        fx.at(&child, NOW - 60_000);
        let parent_path = parent.to_string_lossy().into_owned();
        let s = &found(&fx.roots(), &query("codex", &parent_path, CWD), NOW).list[0];
        assert_eq!(s.state, "done");
        assert_eq!(s.result, "Разбор готов: 4 риска.");
    }

    #[test]
    fn codex_without_sessions_or_parent_gives_a_note() {
        let fx = Fixture::new();
        let f = found(&fx.roots(), &query("codex", SESSION, CWD), NOW);
        assert_eq!(f.note, "нет данных: нет сессий codex");
        fx.write("codex/2026/10/20/rollout-x.jsonl", "{}\n");
        let f = found(&fx.roots(), &query("codex", SESSION, CWD), NOW);
        assert_eq!(f.note, "нет данных: не найдена сессия codex");
    }

    #[test]
    fn coordinator_reads_the_newest_chat_transcript() {
        let fx = Fixture::new();
        let data = fx.dir.path().join("data");
        let chat = data.join("coordinator/default/chat");
        std::fs::create_dir_all(&chat).unwrap();
        std::fs::write(
            data.join("coordinator/default/state.json"),
            r#"{"run":{"kind":"claude"}}"#,
        )
        .unwrap();
        // The chat folder does not exist on disk, so the dashed name is that of the path as given.
        let folder = fx
            .dir
            .path()
            .join("claude/projects")
            .join(project_folder(&canonical(&chat.to_string_lossy())));
        let session = "22222222-3333-4444-5555-666666666666";
        std::fs::create_dir_all(folder.join(session).join("subagents")).unwrap();
        std::fs::write(folder.join(format!("{session}.jsonl")), "{}\n").unwrap();
        let sub = folder.join(session).join("subagents");
        std::fs::write(sub.join("agent-c1.jsonl"), claude_jsonl(None)).unwrap();
        std::fs::write(
            sub.join("agent-c1.meta.json"),
            r#"{"agentType":"general-purpose","description":"Сверить доску"}"#,
        )
        .unwrap();
        fx.at(&sub.join("agent-c1.jsonl"), NOW - 1_000);
        let f = coordinator_in(&fx.roots(), &data, true, NOW);
        assert_eq!(f.note, "");
        assert_eq!(f.list[0].title, "Сверить доску");
        assert_eq!(f.list[0].state, "running");
        // Not busy: the Coordinator's sub-agents are done.
        let f = coordinator_in(&fx.roots(), &data, false, NOW);
        assert_eq!(f.list[0].state, "done");
    }

    #[test]
    fn coordinator_without_claude_or_transcript_gives_a_note() {
        let fx = Fixture::new();
        let data = fx.dir.path().join("data");
        std::fs::create_dir_all(data.join("coordinator/default/chat")).unwrap();
        std::fs::write(
            data.join("coordinator/default/state.json"),
            r#"{"run":{"kind":"omp"}}"#,
        )
        .unwrap();
        assert!(coordinator_in(&fx.roots(), &data, true, NOW)
            .note
            .contains("omp"));
        std::fs::write(
            data.join("coordinator/default/state.json"),
            r#"{"run":{"kind":"claude"}}"#,
        )
        .unwrap();
        assert_eq!(
            coordinator_in(&fx.roots(), &data, true, NOW).note,
            "нет данных: нет стенограммы Координатора"
        );
    }

    #[test]
    fn helpers_clip_and_read_safely() {
        assert_eq!(clip("абвгд", 3), "абв…");
        assert_eq!(clip("ab", 3), "ab");
        assert_eq!(one_line("  a \n\n b\t"), "a b");
        assert_eq!(parse_time("1970-01-01T00:00:01.500Z"), 1500);
        assert_eq!(parse_time("junk"), 0);
        assert_eq!(first_timestamp(b"{\"a\":1}"), 0);
        assert!(!safe_segment("a/b") && !safe_segment("..") && safe_segment("11-22"));
        assert_eq!(tool_summary("", &Value::Null), "tool");
    }

    #[test]
    #[ignore = "reads the real ~/.claude; run by hand"]
    fn real_machine_sub_agents_parse_without_panic() {
        // Run by hand: `cargo test real_machine -- --ignored --nocapture`.
        let r = Roots::live();
        let mut total = 0;
        let mut states = HashMap::new();
        let folders = std::fs::read_dir(claude_projects(&r))
            .into_iter()
            .flatten()
            .flatten();
        for folder in folders {
            for session in std::fs::read_dir(folder.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let dir = session.path().join("subagents");
                for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|x| x == "jsonl") {
                        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
                        let meta = dir.join(format!("{stem}.meta.json"));
                        let st = stamp(&path).unwrap();
                        let facts = claude_facts(&path, &meta);
                        let now = ms_of(SystemTime::now());
                        let state = judge(facts.ended, st.0, now).unwrap_or("dropped");
                        *states.entry(state).or_insert(0) += 1;
                        let age_min = now.saturating_sub(st.0) / 60_000;
                        *states
                            .entry(if age_min < 60 { "age<60m" } else { "age>=60m" })
                            .or_insert(0) += 1;
                        *states
                            .entry(if facts.ended {
                                "flag:ended"
                            } else {
                                "flag:open"
                            })
                            .or_insert(0) += 1;
                        total += 1;
                    }
                }
            }
        }
        println!("claude sub-agent files {total}, by state {states:?}");
    }

    #[test]
    #[ignore = "reads the real ~/.claude; run by hand"]
    fn real_machine_for_task_summary() {
        // Two real sessions: this worktree's and a finished APICache one.
        let cases = [
            (
                "9b7d7b22-c360-4e7e-b599-d4561bc34921",
                "/home/me/.herdr/worktrees/herdr-planner/work-9-herdr-projects",
            ),
            (
                "070a2f94-a6c6-45aa-b663-cc3e3ba065fe",
                "/home/me/.herdr/worktrees/APICache/hp-apicache-t-0161-dokploy",
            ),
        ];
        for (session, cwd) in cases {
            let q = Query {
                app: "claude",
                session,
                cwd,
                agent_name: "",
                parent_active: true,
                parent_exited_at_ms: 0,
            };
            let found = for_task(&q, ms_of(SystemTime::now()));
            println!(
                "session {session}: {} listed, note {:?}",
                found.list.len(),
                found.note
            );
            for s in &found.list {
                println!(
                    "  {} {} {} role={} title={} doing={} result={}",
                    s.id,
                    s.kind,
                    s.state,
                    s.role,
                    s.title.chars().count(),
                    s.doing.chars().count(),
                    s.result.chars().count()
                );
            }
        }
    }
}
