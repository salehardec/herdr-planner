//! Other machines of the herdr-projects board: the "distant galaxies" of PLN-9.
//!
//! The board asks each machine it knows for its tasks with one SSH call (`herdr-projects remote list`), and so does
//! this module, in the board's own form (`ssh_args`). Nothing here reads, stores or asks for SSH keys: the ssh agent
//! and the user's ssh config on the planner's machine decide who may connect.

use crate::board::{self, Link, Task};
use crate::config::{now_ms, AppConfig};
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// A reply from a machine younger than this is reused by a poll (unless the poll is forced).
const LIST_FRESH_MS: u64 = 20_000;
const MACHINES_WAIT: Duration = Duration::from_secs(25);
const LIST_WAIT: Duration = Duration::from_secs(25);
const ACTION_WAIT: Duration = Duration::from_secs(60);
/// The longest free text sent to another machine (a message to its Coordinator, a reply to a task).
pub const MAX_TEXT: usize = 20_000;
pub const OLD_PLUGIN_LINE: &str = "там старый herdr-projects: обновите";
/// The actions `herdr-projects remote do` answers (plan §4.7; `approve` with `anyway` covers the forced yes).
const DO_ACTIONS: &[&str] = &[
    "tell", "approve", "pick", "fix", "retry", "again", "review", "finish", "stop", "archive",
    "restore", "pin", "delegate",
];

pub use crate::ssh::{short_label, ssh_args, valid_label, valid_target, MachineRef, Why};
pub(crate) use crate::ssh::{sh_quote, ssh_ended, test_line, why_not, Ended};
#[cfg(test)]
pub(crate) use crate::ssh::test_cfg;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TalkLine {
    pub you: bool,
    pub text: String,
}

/// The other machine's Coordinator as it sent it: `status` is its state (`off` when it has none).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RemoteCoordinator {
    pub status: String,
    pub reply: String,
    pub talk: Vec<TalkLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RemoteProject {
    pub name: String,
    pub key: String,
    pub folder: String,
    pub origin: Option<String>,
}

/// One machine's board as the planner shows it. A machine that dropped keeps its last good tasks with `ok = false`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Galaxy {
    pub label: String,
    pub target: String,
    pub ok: bool,
    /// Set by the server while a fresh `ask` is on its way; never by a poll.
    pub connecting: bool,
    pub why: Option<Why>,
    pub version: String,
    pub host: String,
    pub coordinator: RemoteCoordinator,
    pub projects: Vec<RemoteProject>,
    /// Ids stay `repo/n` as on the home board; `tag` is `KEY-n`.
    pub tasks: Vec<Task>,
    pub links: Vec<Link>,
    /// When the machine's list was read (ms since the epoch); for a dropped machine, its last good read.
    pub fetched_at_ms: u64,
}

/// A machine as `herdr-projects machines --json` names it.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardMachine {
    pub label: String,
    pub target: String,
    pub ok: bool,
    pub why: Option<Why>,
    pub version: String,
}

/// The planner's own herdr binary (`HERDR_BIN`, as the server reads it).
pub(crate) fn herdr_bin() -> String {
    std::env::var("HERDR_BIN")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "herdr".to_string())
}

/// The board's request line: `herdr-projects remote <what> [<base64 of the JSON payload>]`.
pub fn remote_line(what: &str, payload: Option<&Value>) -> String {
    match payload {
        None => format!("herdr-projects remote {what}"),
        Some(v) => {
            let b64 = base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(v).unwrap_or_default());
            format!("herdr-projects remote {what} {b64}")
        }
    }
}



/// The board's answer: the last JSON object of stdout, where a shell's start-up files may print before it. An
/// object that is the board's own (a task list, or one with `ok`) wins over any other.
pub fn last_json(stdout: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) {
        if v.is_object() {
            return Some(v);
        }
    }
    let mut other = None;
    for line in stdout.lines().rev().map(str::trim) {
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if !v.is_object() {
                continue;
            }
            if v.get("tasks").is_some_and(Value::is_array)
                || v.get("ok").is_some_and(Value::is_boolean)
            {
                return Some(v);
            }
            other.get_or_insert(v);
        }
    }
    other
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The answer of a machine as a JSON object, or why there is none.
pub(crate) fn reply(end: &Ended, m: &MachineRef) -> Result<Value, Why> {
    last_json(&end.stdout).ok_or_else(|| why_not(end, m))
}

/// The task list of a reply. A reply without `tasks` comes from an older plugin (it does not know `remote list`).
fn list_reply(v: Value, m: &MachineRef) -> Result<Value, Why> {
    if v.get("tasks").is_some_and(Value::is_array) {
        return Ok(v);
    }
    let name = if m.label.is_empty() {
        "Машина"
    } else {
        m.label.as_str()
    };
    let msg: String = str_of(&v, "msg").chars().take(300).collect();
    Err(Why {
        line: OLD_PLUGIN_LINE.to_string(),
        detail: vec![
            if msg.is_empty() {
                format!("{name} ответила без списка задач: команды `remote` там нет.")
            } else {
                format!("{name} ответила: {msg}")
            },
            "Обновите herdr-projects на той машине: нужна версия с `remote`.".into(),
            test_line(m),
        ],
    })
}

/// The saved machines of herdr (`herdr machine list --json`): tolerant of `[]`, of an object with `machines`, and
/// of the field names the board reads (`label`/`name`, `ssh_target`/`target`, `remote_session`/`session`).
pub fn parse_saved(v: &Value) -> Vec<MachineRef> {
    let rows: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => v
            .get("machines")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    rows.into_iter()
        .filter(|m| m.is_object() && m.get("enabled") != Some(&json!(false)))
        .filter_map(|m| {
            let pick = |keys: &[&str]| {
                keys.iter()
                    .filter_map(|k| m.get(*k).and_then(Value::as_str))
                    .map(str::trim)
                    .find(|s| !s.is_empty())
                    .unwrap_or("")
                    .to_string()
            };
            let label = short_label(&pick(&["label", "name", "ssh_target", "target"]));
            let target = pick(&["ssh_target", "target", "ssh", "host"]);
            let session = pick(&["remote_session", "session"]);
            (valid_label(&label) && valid_target(&target)).then_some(MachineRef {
                label,
                target,
                session,
            })
        })
        .collect()
}


/// Reads `herdr machine list --json` through the given herdr binary; no herdr or no answer means no saved machines.
pub(crate) async fn saved_machines_with(herdr: &str) -> Vec<MachineRef> {
    let mut cmd = Command::new(herdr);
    cmd.args(["machine", "list", "--json"])
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match timeout(Duration::from_secs(10), cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let v = serde_json::from_str::<Value>(text.trim()).unwrap_or(Value::Null);
            parse_saved(&v)
        }
        _ => Vec::new(),
    }
}

/// The saved machine with this label, validated.
pub(crate) async fn resolve_with(herdr: &str, label: &str) -> Result<MachineRef, String> {
    if !valid_label(label) {
        return Err(format!("недопустимая метка галактики: {label:?}"));
    }
    saved_machines_with(herdr)
        .await
        .into_iter()
        .find(|m| m.label == label)
        .ok_or_else(|| format!("нет галактики «{label}»: её нет среди машин herdr"))
}

/// `herdr-projects machines --json`: the board's view of its machines. `off` is the board's switch for them.
pub fn parse_board_machines(stdout: &str) -> Result<(bool, Vec<BoardMachine>), String> {
    let v = last_json(stdout)
        .ok_or_else(|| "herdr-projects machines --json: нет ответа".to_string())?;
    let off = v.get("off").and_then(Value::as_bool).unwrap_or(false);
    let machines = v
        .get("machines")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| valid_label(&str_of(m, "label")))
        .map(|m| {
            let why = m.get("why").filter(|w| w.is_object()).map(|w| Why {
                line: str_of(w, "line"),
                detail: w
                    .get("detail")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect(),
            });
            BoardMachine {
                label: str_of(m, "label"),
                target: str_of(m, "target"),
                ok: m.get("ok").and_then(Value::as_bool).unwrap_or(false),
                why,
                version: str_of(m, "version"),
            }
        })
        .collect();
    Ok((off, machines))
}

fn coordinator_from(v: &Value) -> RemoteCoordinator {
    let Some(c) = v.get("coordinator").filter(|c| c.is_object()) else {
        return RemoteCoordinator::default();
    };
    if c.get("off").and_then(Value::as_bool) == Some(true) {
        return RemoteCoordinator {
            status: "off".into(),
            ..RemoteCoordinator::default()
        };
    }
    let state = str_of(c, "state");
    let status = if state.is_empty() {
        str_of(c, "error")
    } else {
        state
    };
    let talk = c
        .get("talk")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let text = str_of(m, "text");
            (!text.is_empty()).then(|| TalkLine {
                you: m.get("you").and_then(Value::as_bool).unwrap_or(false),
                text,
            })
        })
        .collect();
    RemoteCoordinator {
        status,
        reply: str_of(c, "said"),
        talk,
    }
}

/// One task record of `remote list`: the board's own parse (so a remote task reads like a local one), its tag from
/// the project key, its record fields, the tags it waits for, and its project.
fn task_from_record(rec: &Value) -> Option<(Task, Vec<String>, RemoteProject)> {
    if !rec.is_object() {
        return None;
    }
    let mut rec = rec.clone();
    if str_of(&rec, "id").is_empty() {
        let repo = str_of(&rec, "repo");
        let n = rec.get("n").and_then(Value::as_u64)?;
        if repo.is_empty() {
            return None;
        }
        rec["id"] = json!(format!("{repo}/{n}"));
    }
    let mut t = board::parse_task(&rec)?;
    let key = str_of(&rec, "key");
    if !key.is_empty() {
        t.tag = format!("{key}-{}", t.n);
    }
    t.key = key.clone();
    t.created = rec.get("created").and_then(Value::as_u64).unwrap_or(0);
    t.description = str_of(&rec, "description");
    t.model = str_of(&rec, "model");
    t.session = str_of(&rec, "session");
    t.worktree = str_of(&rec, "worktree");
    t.exited_at = rec.get("exitedAt").and_then(Value::as_u64).unwrap_or(0);
    t.worker_agent = rec
        .get("settings")
        .and_then(|s| s.get("worker.agent"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| str_of(&rec, "agent"));
    t.subagents_note = "далёкая галактика: субагенты здесь не читаются".into();
    let names = match rec.get("afterNames").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        None => board::parse_after(&str_of(&rec, "afterLine")),
    };
    let snap = rec.get("snap").filter(|s| s.is_object());
    let folder = snap.map(|s| str_of(s, "folder")).unwrap_or_default();
    let repo = snap
        .map(|s| str_of(s, "repo"))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| t.repo.clone());
    let project = RemoteProject {
        name: repo,
        key,
        folder,
        origin: None,
    };
    Some((t, names, project))
}

/// A machine's `remote list` reply as its galaxy. Pure: the tests feed it a list.
pub fn galaxy_from_list(m: &MachineRef, v: &Value, now: u64) -> Galaxy {
    let mut tasks: Vec<Task> = Vec::new();
    let mut afters: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut projects: BTreeMap<String, RemoteProject> = BTreeMap::new();
    for rec in v
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some((task, names, project)) = task_from_record(rec) else {
            continue;
        };
        projects.entry(project.name.clone()).or_insert(project);
        afters.insert(task.id.clone(), names);
        tasks.push(task);
    }
    // A name that is one of this machine's task ids is that task's tag, as the local board names them.
    let tags: BTreeMap<String, String> = tasks
        .iter()
        .map(|t| (t.id.clone(), t.tag.clone()))
        .collect();
    let afters: BTreeMap<String, Vec<String>> = afters
        .into_iter()
        .map(|(to, names)| {
            let names = names
                .into_iter()
                .map(|n| tags.get(&n).cloned().unwrap_or(n))
                .collect();
            (to, names)
        })
        .collect();
    let links = board::links_from(&afters, &tasks);
    Galaxy {
        label: m.label.clone(),
        target: m.target.clone(),
        ok: true,
        connecting: false,
        why: None,
        version: str_of(v, "version"),
        host: str_of(v, "host"),
        coordinator: coordinator_from(v),
        projects: projects.into_values().collect(),
        tasks,
        links,
        fetched_at_ms: now,
    }
}

/// One fresh `remote list` of a machine.
pub(crate) async fn list_remote(cfg: &AppConfig, m: &MachineRef, now: u64) -> Result<Galaxy, Why> {
    let end = ssh_ended(cfg, m, &remote_line("list", None), LIST_WAIT)
        .await
        .map_err(|e| Why {
            line: "настройки машины не годятся".into(),
            detail: vec![e],
        })?;
    let v = reply(&end, m)?;
    let v = list_reply(v, m)?;
    Ok(galaxy_from_list(m, &v, now))
}

/// A galaxy that did not answer now: the last good one, flagged, or an empty one that says why.
fn flagged(prev: Option<&Galaxy>, m: &MachineRef, why: Why) -> Galaxy {
    match prev {
        Some(p) => Galaxy {
            ok: false,
            why: Some(why),
            connecting: false,
            ..p.clone()
        },
        None => Galaxy {
            label: m.label.clone(),
            target: m.target.clone(),
            ok: false,
            why: Some(why),
            ..Galaxy::default()
        },
    }
}

async fn galaxy_for(
    cfg: &AppConfig,
    bm: &BoardMachine,
    m: MachineRef,
    prev: Option<&Galaxy>,
    force: bool,
    now: u64,
) -> Galaxy {
    if !bm.ok {
        let why = bm.why.clone().unwrap_or_else(|| Why {
            line: "не отвечает".into(),
            detail: vec![format!(
                "Доска не может прочитать задачи на машине {}.",
                bm.label
            )],
        });
        return flagged(prev, &m, why);
    }
    if !force {
        if let Some(p) =
            prev.filter(|p| p.ok && now.saturating_sub(p.fetched_at_ms) < LIST_FRESH_MS)
        {
            return p.clone();
        }
    }
    match list_remote(cfg, &m, now).await {
        Ok(g) => g,
        Err(why) => flagged(prev, &m, why),
    }
}

/// The poll, with the herdr binary given: `machines --json` from the board, then `remote list` from each machine it
/// calls ok (in parallel). A machine that drops keeps its last good snapshot, flagged `ok = false`.
pub(crate) async fn poll_with(
    cfg: &AppConfig,
    herdr: &str,
    prev: &[Galaxy],
    force: bool,
) -> Vec<Galaxy> {
    let now = now_ms();
    let parsed = match crate::app::run_board(cfg, &["machines", "--json"], MACHINES_WAIT).await {
        Ok(out) => parse_board_machines(&out),
        Err(e) => Err(e),
    };
    let (off, machines) = match parsed {
        Ok(p) => p,
        Err(e) => {
            return prev
                .iter()
                .map(|g| {
                    let m = MachineRef {
                        label: g.label.clone(),
                        target: g.target.clone(),
                        session: String::new(),
                    };
                    flagged(
                        Some(g),
                        &m,
                        Why {
                            line: "доска не ответила".into(),
                            detail: vec![e.clone()],
                        },
                    )
                })
                .collect();
        }
    };
    if off {
        return Vec::new();
    }
    let saved = saved_machines_with(herdr).await;
    let calls = machines.iter().map(|bm| {
        let prev_g = prev.iter().find(|g| g.label == bm.label);
        let saved_m = saved.iter().find(|s| s.label == bm.label);
        let m = MachineRef {
            label: bm.label.clone(),
            target: if valid_target(&bm.target) {
                bm.target.clone()
            } else {
                saved_m.map(|s| s.target.clone()).unwrap_or_default()
            },
            session: saved_m.map(|s| s.session.clone()).unwrap_or_default(),
        };
        async move { galaxy_for(cfg, bm, m, prev_g, force, now).await }
    });
    futures_util::future::join_all(calls).await
}

/// Polls the machines of the board. `prev` is the last result; `force` skips the 20 s reuse of a fresh list.
pub async fn poll(cfg: &AppConfig, prev: &[Galaxy], force: bool) -> Vec<Galaxy> {
    poll_with(cfg, &herdr_bin(), prev, force).await
}

/// One machine asked now, for a flight to it.
pub(crate) async fn ask_with(cfg: &AppConfig, herdr: &str, label: &str) -> Result<Galaxy, Why> {
    let m = resolve_with(herdr, label).await.map_err(|e| Why {
        line: "нет такой галактики".into(),
        detail: vec![
            e,
            "Добавьте её пунктом «Добавить галактику» в навигаторе.".into(),
        ],
    })?;
    list_remote(cfg, &m, now_ms()).await
}

/// Asks one machine for its board now (a flight to it).
pub async fn ask(cfg: &AppConfig, label: &str) -> Result<Galaxy, Why> {
    ask_with(cfg, &herdr_bin(), label).await
}

/// The result of an action on a machine: its `msg` when `ok`, its words otherwise.
fn outcome(v: &Value) -> Result<String, String> {
    let msg = str_of(v, "msg");
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(msg)
    } else if msg.is_empty() {
        Err("машина ответила без подтверждения".into())
    } else {
        Err(msg)
    }
}

async fn act(
    cfg: &AppConfig,
    herdr: &str,
    label: &str,
    what: &str,
    payload: &Value,
    wait: Duration,
) -> Result<Value, String> {
    let m = resolve_with(herdr, label).await?;
    let end = ssh_ended(cfg, &m, &remote_line(what, Some(payload)), wait).await?;
    reply(&end, &m).map_err(|w| format!("{} не ответила: {}", m.label, w.line))
}

/// The payload of `remote do`: the task id and the action, with the few keys the board reads.
pub(crate) fn do_payload(action: &Value) -> Result<Value, String> {
    let id = action
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| board::valid_id(s))
        .ok_or("нет задачи или её id недопустим")?;
    let name = action
        .get("action")
        .and_then(Value::as_str)
        .filter(|a| DO_ACTIONS.contains(a))
        .ok_or("такого действия над задачей на другой машине нет")?;
    if let Some(text) = action.get("text").and_then(Value::as_str) {
        if text.chars().count() > MAX_TEXT {
            return Err("текст слишком длинный".into());
        }
    }
    if let Some(letter) = action.get("letter").and_then(Value::as_str) {
        if !board::valid_letter(letter) {
            return Err("буква попытки недопустима".into());
        }
    }
    let mut p = json!({ "id": id, "action": name });
    for key in ["text", "letter", "seen", "anyway", "now"] {
        if let Some(v) = action.get(key) {
            p[key] = v.clone();
        }
    }
    Ok(p)
}

pub(crate) async fn remote_do_with(
    cfg: &AppConfig,
    herdr: &str,
    label: &str,
    action: &Value,
) -> Result<String, String> {
    let payload = do_payload(action)?;
    let v = act(cfg, herdr, label, "do", &payload, ACTION_WAIT).await?;
    outcome(&v)
}

/// Runs an action on a task of another machine (`remote do`). `action` is `{id, action, text?, letter?, seen?,
/// anyway?, now?}`; the answer's `msg` is returned, or the machine's reason it was refused.
pub async fn remote_do(cfg: &AppConfig, label: &str, action: &Value) -> Result<String, String> {
    remote_do_with(cfg, &herdr_bin(), label, action).await
}

pub(crate) async fn remote_dispatch_with(
    cfg: &AppConfig,
    herdr: &str,
    label: &str,
    to: &str,
    text: &str,
    delegate: bool,
    about: Option<&str>,
) -> Result<String, String> {
    if !to.is_empty()
        && !(to.len() <= 64
            && to
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)))
    {
        return Err(format!("недопустимое имя адресата: {to:?}"));
    }
    if text.trim().is_empty() || text.chars().count() > MAX_TEXT {
        return Err("текст пуст или слишком длинный".into());
    }
    let mut payload = json!({ "to": to, "text": text, "delegate": delegate });
    if let Some(a) = about {
        if !board::valid_id(a) {
            return Err(format!("недопустимый id задачи: {a:?}"));
        }
        payload["about"] = json!(a);
    }
    let v = act(cfg, herdr, label, "dispatch", &payload, ACTION_WAIT).await?;
    outcome(&v)
}

/// Sends text to the Coordinator of another machine (`remote dispatch`); `to` is `""` for its machine level, or a
/// repo name. `about` is a task id of that machine, for the Coordinator's folder.
pub async fn remote_dispatch(
    cfg: &AppConfig,
    label: &str,
    to: &str,
    text: &str,
    delegate: bool,
    about: Option<&str>,
) -> Result<String, String> {
    remote_dispatch_with(cfg, &herdr_bin(), label, to, text, delegate, about).await
}

pub(crate) async fn remote_after_with(
    cfg: &AppConfig,
    herdr: &str,
    label: &str,
    task_id: &str,
    refs: &[String],
) -> Result<Vec<String>, String> {
    if !board::valid_id(task_id) {
        return Err(format!("недопустимый id задачи: {task_id:?}"));
    }
    if refs.len() > 50 || refs.iter().any(|r| !board::valid_ref(r)) {
        return Err("недопустимая ссылка на задачу".into());
    }
    let m = resolve_with(herdr, label).await?;
    let last = if refs.is_empty() {
        "--clear".to_string()
    } else {
        refs.join(",")
    };
    let cmd = format!(
        "herdr-projects after {} {}",
        sh_quote(task_id),
        sh_quote(&last)
    );
    let end = ssh_ended(cfg, &m, &cmd, ACTION_WAIT).await?;
    let last_line = |s: &str| {
        s.lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string()
    };
    if end.code != Some(0) && end.stdout.trim().is_empty() {
        let detail = if end.stderr.trim().is_empty() {
            last_line(&end.stdout)
        } else {
            last_line(&end.stderr)
        };
        return Err(format!("{}: {detail}", m.label));
    }
    let line = end
        .stdout
        .lines()
        .rev()
        .find(|l| l.contains("waits for") || l.contains("is to wait for"));
    match line {
        Some(l) => Ok(board::parse_after(l)),
        None if refs.is_empty() => Ok(Vec::new()),
        None => Err(format!("ответ не разобран: {}", last_line(&end.stdout))),
    }
}

/// Sets the tasks a task of another machine waits for: `herdr-projects after <id> <refs|--clear>` on that machine,
/// in its login shell. Returns the tags it now names, as `board::parse_after` reads them.
pub async fn remote_after(
    cfg: &AppConfig,
    label: &str,
    task_id: &str,
    refs: &[String],
) -> Result<Vec<String>, String> {
    remote_after_with(cfg, &herdr_bin(), label, task_id, refs).await
}

/// The validated argv for `herdr machine add <target> --label <label>` (the server runs it in a terminal).
pub fn machine_add_command(target: &str, label: &str) -> Result<Vec<String>, String> {
    if !valid_target(target) {
        return Err(format!("недопустимый адрес машины: {target:?}"));
    }
    if !valid_label(label) {
        return Err(format!("недопустимая метка машины: {label:?}"));
    }
    Ok(vec![
        "herdr".into(),
        "machine".into(),
        "add".into(),
        target.into(),
        "--label".into(),
        label.into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::path::{Path, PathBuf};

    /// A stand-in for ssh. It logs its arguments (one call, one line group) and answers by the target (the
    /// argument before the remote command): `good` lists and acts, `down` fails as ssh does, `old` is a plugin
    /// without `remote`, `garbage` prints text, `off` is not asked at all.
    const SSH: &str = r#"#!/bin/sh
printf '%s\n' "$@" >> '@DIR@/args.txt'
last=""; prev=""
for a; do prev=$last; last=$a; done
case "$prev" in
  down) echo "ssh: connect to host down port 22: Connection refused" >&2; exit 255 ;;
esac
case "$last" in
  *"remote list"*)
    case "$prev" in
      old) echo '{"ok":false,"msg":"Unknown request list"}'; exit 0 ;;
      garbage) echo "hello there"; exit 0 ;;
    esac
    echo "Welcome to the login shell"
    cat '@DIR@/list.json'
    echo ;;
  *"remote do "*)
    b=${last#*remote do }; b=${b%?}
    printf '%s' "$b" | base64 -d > '@DIR@/payload.json'
    echo '{"ok":true,"msg":"принято"}' ;;
  *"remote dispatch "*)
    b=${last#*remote dispatch }; b=${b%?}
    printf '%s' "$b" | base64 -d > '@DIR@/payload.json'
    echo '{"ok":true,"msg":"sent to vm-a"}' ;;
  *"herdr-projects after"*)
    printf '%s\n' "$last" > '@DIR@/after-cmd.txt'
    echo "ela/4: waits for ELA-3 (done), vm-x:PLN-9" ;;
esac
"#;

    const LIST: &str = r#"{"host":"vm-a.lan","version":"1.2.4","automations":{},"coordinator":{"state":"idle","error":"","queue":0,"readable":true,"said":"Жду задач","sent":"","talk":[{"you":true,"text":"привет"},{"you":false,"text":"здравствуйте"}]},"coordinators":{},"sidebar":[],"tasks":[{"id":"ela/3","repo":"ela","n":3,"title":"Первая","state":"working","stage":"queued","kind":"","msg":"","app":"","key":"ELA","created":1700000000000,"description":"Описание","agent":"claude","model":"claude/claude-haiku-5-5","session":"s1","worktree":"/w/ela-3","exitedAt":0,"settings":{"worker.agent":"claude/claude-haiku-5-5"},"afterNames":[],"afterLine":"ela/3: waits for nothing","snap":{"folder":"~/Projects/ela","repo":"ela"}},{"id":"ela/4","repo":"ela","n":4,"title":"Вторая","state":"blocked","stage":"pick","kind":"","msg":"","app":"","key":"ELA","created":1700000100000,"afterNames":["ELA-3","vm-x:PLN-9"],"snap":{"folder":"~/Projects/ela","repo":"ela"}}]}"#;

    const BOARD_MACHINES: &str = r#"{"here":{"label":"orion","here":true,"ok":true,"version":"1.2.4","shell":"/usr/bin/zsh"},"machines":[
{"label":"vm-a","target":"good","ok":true,"version":"1.2.4","tasks":2},
{"label":"vm-down","target":"down","ok":true,"version":"1.2.4","tasks":0},
{"label":"vm-old","target":"old","ok":true,"version":"1.2.3","tasks":0},
{"label":"vm-off","target":"off","ok":false,"why":{"kind":"connect","line":"ssh не смог подключиться","detail":["Проверьте сеть."],"said":"","code":255}}
],"off":false}"#;

    const SAVED: &str = r#"[{"id":"m1","label":"vm-a","ssh_target":"good","remote_session":"work"},{"id":"m2","label":"vm-down","ssh_target":"down"},{"id":"m3","label":"vm-old","ssh_target":"old"},{"id":"m4","label":"vm-off","ssh_target":"off"}]"#;

    fn exec(p: &Path, body: &str) -> String {
        crate::testutil::write_exec(p, body);
        p.display().to_string()
    }

    /// The fake world: ssh, the board's `machines`, and herdr's saved machines, all under `dir`.
    struct World {
        dir: PathBuf,
        cfg: AppConfig,
        herdr: String,
    }

    impl World {
        fn new(dir: &Path) -> World {
            let d = dir.display().to_string();
            let fill = |s: &str| s.replace("@DIR@", &d);
            std::fs::write(dir.join("list.json"), LIST).unwrap();
            std::fs::write(dir.join("machines.json"), BOARD_MACHINES).unwrap();
            std::fs::write(dir.join("saved.json"), SAVED).unwrap();
            let ssh = exec(&dir.join("ssh"), &fill(SSH));
            let board = exec(
                &dir.join("board"),
                &fill("#!/bin/sh\ncase \"$1\" in machines) cat '@DIR@/machines.json' ;; *) exit 2 ;; esac\n"),
            );
            let herdr = exec(
                &dir.join("herdr"),
                &fill("#!/bin/sh\ncat '@DIR@/saved.json'\n"),
            );
            let cfg = AppConfig {
                ssh_bin: ssh,
                herdr_projects: board,
                ..test_cfg(dir)
            };
            World {
                dir: dir.to_path_buf(),
                cfg,
                herdr,
            }
        }

        fn args(&self) -> String {
            std::fs::read_to_string(self.dir.join("args.txt")).unwrap_or_default()
        }

        fn payload(&self, name: &str) -> serde_json::Value {
            let s = std::fs::read_to_string(self.dir.join(name)).unwrap();
            serde_json::from_str(&s).unwrap()
        }
    }

    fn mref(label: &str, target: &str, session: &str) -> MachineRef {
        MachineRef {
            label: label.into(),
            target: target.into(),
            session: session.into(),
        }
    }

    #[test]
    fn bad_labels_targets_and_sessions_are_refused_before_ssh() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        for m in [
            mref("-x", "vm-a", ""),
            mref("vm a", "vm-a", ""),
            mref("vm-a", "-oProxyCommand=x", ""),
            mref("vm-a", "a;rm -rf /", ""),
            mref("vm-a", "a b", ""),
            mref("vm-a", "vm-a", "x y"),
            mref("", "vm-a", ""),
        ] {
            assert!(
                ssh_args(&cfg, &m, "herdr-projects remote list").is_err(),
                "{m:?}"
            );
        }
        assert!(machine_add_command("u@h", "bad label").is_err());
        assert!(machine_add_command("-oX", "ok").is_err());
        assert!(machine_add_command("u@h.lan:2222", "vm-a").is_ok());
    }

    #[test]
    fn the_payload_is_base64_json_and_the_line_has_no_raw_text() {
        let v = json!({"id": "ela/3", "action": "tell", "text": "it's $(id) \"x\"\nнова"});
        let line = remote_line("do", Some(&v));
        let b64 = line.strip_prefix("herdr-projects remote do ").unwrap();
        assert!(!line.contains('\'') && !line.contains('$') && !line.contains('"'));
        let back = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&back).unwrap(), v);
        assert_eq!(remote_line("list", None), "herdr-projects remote list");
    }

    #[test]
    fn sh_quote_keeps_harmless_words_bare() {
        assert_eq!(sh_quote("ela/3"), "ela/3");
        assert_eq!(sh_quote("ela/3,vm-x:PLN-9"), "ela/3,vm-x:PLN-9");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn the_last_json_line_is_the_answer() {
        let banner = "motd: welcome {not json}\n{\"ok\":true,\"msg\":\"a\"}\n{\"tasks\":[]}\n";
        assert_eq!(last_json(banner).unwrap()["tasks"], json!([]));
        assert_eq!(last_json("hello\n{\"host\":\"h\"}").unwrap()["host"], "h");
        assert!(last_json("hello there").is_none());
    }

    #[test]
    fn saved_machines_are_read_tolerantly() {
        assert!(parse_saved(&json!([])).is_empty());
        assert!(parse_saved(&json!("text")).is_empty());
        let got = parse_saved(&json!({"machines": [
            {"label": "vm-a", "ssh_target": "u@vm-a.lan", "remote_session": "work"},
            {"label": "off", "enabled": false, "ssh_target": "x"},
            {"label": "-bad", "ssh_target": "x"},
            {"name": "u@box.lan", "target": "u@box.lan"},
            {"label": "vm-c"}
        ]}));
        assert_eq!(
            got,
            vec![
                mref("vm-a", "u@vm-a.lan", "work"),
                mref("box", "u@box.lan", ""),
            ]
        );
    }

    #[test]
    fn the_board_says_which_machines_answer() {
        let (off, ms) = parse_board_machines(BOARD_MACHINES).unwrap();
        assert!(!off);
        assert_eq!(ms.len(), 4);
        assert!(ms[0].ok && ms[0].why.is_none());
        assert_eq!(ms[3].why.as_ref().unwrap().line, "ssh не смог подключиться");
        assert!(parse_board_machines("нет json").is_err());
        assert!(
            parse_board_machines(r#"{"off":true,"machines":[]}"#)
                .unwrap()
                .0
        );
    }

    #[test]
    fn a_remote_list_becomes_a_galaxy_with_the_board_s_tasks() {
        let m = mref("vm-a", "vm-a", "");
        let g = galaxy_from_list(&m, &serde_json::from_str(LIST).unwrap(), 42);
        assert!(g.ok && !g.connecting && g.why.is_none());
        assert_eq!(
            (g.host.as_str(), g.version.as_str(), g.fetched_at_ms),
            ("vm-a.lan", "1.2.4", 42)
        );
        assert_eq!(g.tasks.len(), 2);
        let first = &g.tasks[0];
        assert_eq!(first.id, "ela/3");
        assert_eq!(first.tag, "ELA-3");
        assert_eq!(first.key, "ELA");
        assert_eq!(first.created, 1_700_000_000_000);
        assert_eq!(first.description, "Описание");
        assert_eq!(first.model, "claude/claude-haiku-5-5");
        assert_eq!(first.worker_agent, "claude/claude-haiku-5-5");
        assert_eq!(first.worktree, "/w/ela-3");
        assert_eq!(first.status, crate::board::Status::Queued);
        assert_eq!(g.tasks[1].status, crate::board::Status::Waiting);
        // `ELA-3` names a task here, so the link is from that task; `vm-x:PLN-9` is far away.
        let waiting = g.links.iter().find(|l| l.kind == "waiting").unwrap();
        assert_eq!(
            (waiting.from.as_str(), waiting.to.as_str()),
            ("ela/3", "ela/4")
        );
        let far = g.links.iter().find(|l| l.kind == "far").unwrap();
        assert_eq!(
            (far.from.as_str(), far.label.as_str()),
            ("vm-x:PLN-9", "vm-x:PLN-9")
        );
        assert_eq!(g.projects.len(), 1);
        assert_eq!(
            (g.projects[0].name.as_str(), g.projects[0].key.as_str()),
            ("ela", "ELA")
        );
        assert_eq!(g.coordinator.status, "idle");
        assert_eq!(g.coordinator.reply, "Жду задач");
        assert_eq!(g.coordinator.talk.len(), 2);
        assert!(g.coordinator.talk[0].you);
    }

    #[test]
    fn a_reply_from_an_older_plugin_says_to_update_it() {
        let m = mref("vm-old", "old", "");
        let why = list_reply(json!({"ok": false, "msg": "Unknown request list"}), &m).unwrap_err();
        assert_eq!(why.line, OLD_PLUGIN_LINE);
        assert!(why.detail[0].contains("Unknown request list"));
    }

    #[test]
    fn why_not_names_the_reason_the_way_the_board_does() {
        let m = mref("vm-a", "user@vm-a.lan", "");
        let end = |code: Option<i32>, out: &str, err: &str| Ended {
            code,
            stdout: out.into(),
            stderr: err.into(),
            ..Ended::default()
        };
        assert_eq!(
            why_not(
                &end(
                    Some(255),
                    "",
                    "ssh: connect to host x port 22: Connection refused"
                ),
                &m
            )
            .line,
            "не подключиться (она отказала в соединении)"
        );
        assert_eq!(
            why_not(
                &end(Some(255), "", "user@vm-a: Permission denied (publickey)."),
                &m
            )
            .line,
            "она не пускает доску"
        );
        assert_eq!(
            why_not(&end(Some(255), "", "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED @@@\nHost key verification failed."), &m).line,
            "у машины другой ключ хоста"
        );
        assert_eq!(
            why_not(
                &end(Some(127), "", "sh: herdr-projects: command not found"),
                &m
            )
            .line,
            "herdr-projects там не найден"
        );
        assert_eq!(
            why_not(&end(Some(2), "", "herdr-projects does not know remote"), &m).line,
            OLD_PLUGIN_LINE
        );
        assert_eq!(
            why_not(&end(Some(0), "hello there", ""), &m).line,
            "ответила не то, что ждёт доска"
        );
        assert_eq!(
            why_not(&end(Some(0), "", ""), &m).line,
            "ничего не ответила"
        );
        assert_eq!(
            why_not(&end(Some(3), "", "boom"), &m).line,
            "ответила ошибкой и без ответа"
        );
        let slow = Ended {
            timed_out: true,
            wait_ms: 25_000,
            ..Ended::default()
        };
        assert_eq!(why_not(&slow, &m).line, "не ответила вовремя");
        let no_ssh = Ended {
            error: "ssh: No such file".into(),
            ..Ended::default()
        };
        assert_eq!(why_not(&no_ssh, &m).line, "ssh здесь не запускается");
        // every reason ends with the test to run by hand
        assert!(why_not(&end(Some(255), "", "ssh: x"), &m)
            .detail
            .last()
            .unwrap()
            .contains("ssh user@vm-a.lan"));
    }

    #[tokio::test]
    async fn a_poll_reads_the_good_machines_and_flags_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::new(dir.path());
        let prev = vec![Galaxy {
            label: "vm-down".into(),
            target: "down".into(),
            ok: true,
            fetched_at_ms: 1,
            tasks: vec![Task {
                id: "old/1".into(),
                ..Task::default()
            }],
            ..Galaxy::default()
        }];
        let got = poll_with(&w.cfg, &w.herdr, &prev, false).await;
        let by = |l: &str| got.iter().find(|g| g.label == l).unwrap().clone();

        let a = by("vm-a");
        assert!(a.ok, "{a:?}");
        assert_eq!(a.tasks.len(), 2);
        assert_eq!(a.target, "good");

        let down = by("vm-down");
        assert!(!down.ok);
        assert_eq!(
            down.why.as_ref().unwrap().line,
            "не подключиться (она отказала в соединении)"
        );
        assert_eq!(down.tasks.len(), 1, "the last good snapshot stays");
        assert_eq!(down.tasks[0].id, "old/1");

        let old = by("vm-old");
        assert!(!old.ok);
        assert_eq!(old.why.as_ref().unwrap().line, OLD_PLUGIN_LINE);

        let off = by("vm-off");
        assert!(!off.ok);
        assert_eq!(off.why.as_ref().unwrap().line, "ssh не смог подключиться");
        assert!(off.tasks.is_empty());

        let args = w.args();
        assert!(
            args.contains("HERDR_SESSION=work herdr-projects remote list"),
            "{args}"
        );
        assert!(args.contains("BatchMode=yes"));
    }

    #[tokio::test]
    async fn a_fresh_list_is_reused_unless_forced() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::new(dir.path());
        let now = now_ms();
        let prev = vec![Galaxy {
            label: "vm-a".into(),
            target: "good".into(),
            ok: true,
            fetched_at_ms: now,
            ..Galaxy::default()
        }];
        // Each ssh call names its target as one argument line: count the calls to vm-a's target.
        let calls = |w: &World| w.args().lines().filter(|l| *l == "good").count();
        let before = calls(&w);
        let reused = poll_with(&w.cfg, &w.herdr, &prev, false).await;
        assert_eq!(
            reused
                .iter()
                .find(|g| g.label == "vm-a")
                .unwrap()
                .fetched_at_ms,
            now
        );
        assert_eq!(calls(&w), before, "no ssh call for vm-a");
        let forced = poll_with(&w.cfg, &w.herdr, &prev, true).await;
        assert!(
            forced
                .iter()
                .find(|g| g.label == "vm-a")
                .unwrap()
                .fetched_at_ms
                >= now
        );
        assert!(calls(&w) > before);
    }

    #[tokio::test]
    async fn a_board_that_does_not_answer_keeps_the_last_galaxies_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::new(dir.path());
        let cfg = AppConfig {
            herdr_projects: exec(
                &dir.path().join("dead"),
                "#!/bin/sh\necho 'no board' >&2\nexit 1\n",
            ),
            ..w.cfg.clone()
        };
        let prev = vec![Galaxy {
            label: "vm-a".into(),
            target: "good".into(),
            ok: true,
            ..Galaxy::default()
        }];
        let got = poll_with(&cfg, &w.herdr, &prev, false).await;
        assert_eq!(got.len(), 1);
        assert!(!got[0].ok);
        assert_eq!(got[0].why.as_ref().unwrap().line, "доска не ответила");
    }

    #[tokio::test]
    async fn ask_is_fresh_and_refuses_unknown_or_bad_labels() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::new(dir.path());
        let g = ask_with(&w.cfg, &w.herdr, "vm-a").await.unwrap();
        assert!(g.ok);
        assert_eq!(g.tasks.len(), 2);
        let none = ask_with(&w.cfg, &w.herdr, "vm-zz").await.unwrap_err();
        assert_eq!(none.line, "нет такой галактики");
        let bad = ask_with(&w.cfg, &w.herdr, "-x").await.unwrap_err();
        assert_eq!(bad.line, "нет такой галактики");
        let down = ask_with(&w.cfg, &w.herdr, "vm-down").await.unwrap_err();
        assert_eq!(down.line, "не подключиться (она отказала в соединении)");
    }

    #[tokio::test]
    async fn an_action_is_sent_as_a_payload_not_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::new(dir.path());
        let text = "ок, 'кавычки' $(rm -rf /)\nстрока";
        let action =
            json!({"id": "ela/3", "action": "approve", "anyway": true, "text": text, "junk": 1});
        let msg = remote_do_with(&w.cfg, &w.herdr, "vm-a", &action)
            .await
            .unwrap();
        assert_eq!(msg, "принято");
        let got = w.payload("payload.json");
        assert_eq!(
            got,
            json!({"id": "ela/3", "action": "approve", "anyway": true, "text": text})
        );
        assert!(
            !w.args().contains("кавычки"),
            "the text is never on the command line"
        );
    }

    #[tokio::test]
    async fn an_action_that_is_not_allowed_or_a_machine_that_is_down_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::new(dir.path());
        let rm = json!({"id": "ela/3", "action": "rm"});
        assert!(remote_do_with(&w.cfg, &w.herdr, "vm-a", &rm).await.is_err());
        let bad_id = json!({"id": "-x", "action": "tell"});
        assert!(remote_do_with(&w.cfg, &w.herdr, "vm-a", &bad_id)
            .await
            .is_err());
        let long = json!({"id": "ela/3", "action": "tell", "text": "x".repeat(MAX_TEXT + 1)});
        assert!(remote_do_with(&w.cfg, &w.herdr, "vm-a", &long)
            .await
            .is_err());
        let down = json!({"id": "ela/3", "action": "tell", "text": "hi"});
        let err = remote_do_with(&w.cfg, &w.herdr, "vm-down", &down)
            .await
            .unwrap_err();
        assert!(err.contains("не ответила"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_goes_to_the_machine_coordinator_with_the_task_it_is_about() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::new(dir.path());
        let msg = remote_dispatch_with(&w.cfg, &w.herdr, "vm-a", "", "Привет", true, Some("ela/3"))
            .await
            .unwrap();
        assert_eq!(msg, "sent to vm-a");
        let got = w.payload("payload.json");
        assert_eq!(got["to"], "");
        assert_eq!(got["text"], "Привет");
        assert_eq!(got["delegate"], true);
        assert_eq!(got["about"], "ela/3");
        assert!(
            remote_dispatch_with(&w.cfg, &w.herdr, "vm-a", "a b", "x", false, None)
                .await
                .is_err()
        );
        assert!(
            remote_dispatch_with(&w.cfg, &w.herdr, "vm-a", "", "  ", false, None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn after_runs_the_board_command_and_reads_its_line() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::new(dir.path());
        let refs = vec!["ela/3".to_string(), "vm-x:PLN-9".to_string()];
        let tags = remote_after_with(&w.cfg, &w.herdr, "vm-a", "ela/4", &refs)
            .await
            .unwrap();
        assert_eq!(tags, vec!["ELA-3".to_string(), "vm-x:PLN-9".to_string()]);
        let cmd = std::fs::read_to_string(w.dir.join("after-cmd.txt")).unwrap();
        // vm-a's saved session is `work`: its herdr session goes in front, as the board does it.
        assert_eq!(
            cmd.trim(),
            "exec \"${SHELL:-/bin/sh}\" -lc 'HERDR_SESSION=work herdr-projects after ela/4 ela/3,vm-x:PLN-9'"
        );
        assert!(remote_after_with(&w.cfg, &w.herdr, "vm-a", "ela/4", &[])
            .await
            .is_ok());
        assert!(std::fs::read_to_string(w.dir.join("after-cmd.txt"))
            .unwrap()
            .contains("--clear"));
        let bad = vec!["ela/3;rm".to_string()];
        assert!(remote_after_with(&w.cfg, &w.herdr, "vm-a", "ela/4", &bad)
            .await
            .is_err());
        assert!(remote_after_with(&w.cfg, &w.herdr, "vm-a", "x y", &refs)
            .await
            .is_err());
    }

    #[test]
    fn machine_add_is_the_validated_argv() {
        assert_eq!(
            machine_add_command("u@vm.lan", "vm-a").unwrap(),
            vec!["herdr", "machine", "add", "u@vm.lan", "--label", "vm-a"]
        );
        assert!(machine_add_command("u@h", "a b").is_err());
        assert!(machine_add_command("u@h", "a;b").is_err());
        assert!(machine_add_command("-x", "a").is_err());
    }
}
