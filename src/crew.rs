//! The crew screen's brain (PLN-63): what each program is now (signed in, the account, out of allowance until when,
//! the providers inside it, who uses it) and the crew presets: a named set that gives every role its agent, effort and
//! fallback, applied through the board's own `set` and `unset` after a «было → стало» list.
//!
//! The readings and the plan of a preset are pure functions (unit-tested below); `station.rs` calls them with what the
//! board and Omarchy said. Presets live in the planner's state folder, in `crew-presets.json`; the board never sees
//! them, only the commands an applied preset runs.

use crate::board::{Status, Task};
use crate::config::{now_ms, AppConfig};
use crate::models;
use crate::station::{agent_program, check_agent, check_effort, find, Setting, PROGRAMS};
use crate::usage::{AgentUsage, CRIT_AT, WARN_AT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A role of the crew, as the board's settings name it. `light` is the planner's own preset (no board setting).
pub struct Role {
    pub id: &'static str,
    pub agent: &'static str,
    pub effort: &'static str,
    pub fallback: &'static str,
}

/// The roles in the order a preset is shown and applied: for each, the agent first (the effort is checked against
/// it), then the effort, then the fallback.
pub const ROLES: &[Role] = &[
    Role { id: "coordinator", agent: "coordinator.agent", effort: "coordinator.effort", fallback: "" },
    Role { id: "plan", agent: "plan.agent", effort: "", fallback: "" },
    Role { id: "reviewer", agent: "reviewer.agent", effort: "reviewer.effort", fallback: "reviewer.fallback" },
    Role { id: "worker", agent: "worker.agent", effort: "worker.effort", fallback: "worker.fallback" },
    Role { id: "light", agent: "", effort: "", fallback: "" },
];

/// The programs that carry several providers inside (a model is `program/provider/model`).
const MULTI: &[&str] = &["omp", "pi", "opencode"];
/// A window this far used counts as spent.
const SPENT_AT: f64 = 0.995;

fn text_after<'a>(hay: &'a str, key: &str) -> Option<&'a str> {
    hay.find(key).map(|i| &hay[i + key.len()..])
}

/* ───────── what the board says about a program: out of allowance, logged out ───────── */

/// A live task that says its program is out of allowance (or logged out): the program, and the board's words for when.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub program: String,
    /// `17:00`, `in 2 hours`; empty when the board does not say.
    pub until: String,
    pub logged_out: bool,
    pub task: String,
}

fn hit_of(t: &Task) -> Option<Hit> {
    let text = format!("{} {}", t.msg, t.board_label);
    let lower = text.to_lowercase();
    let allowance = lower.contains("out of allowance");
    let logged_out = !allowance && lower.contains("logged out");
    if !allowance && !logged_out {
        return None;
    }
    let first_word = |s: &str| -> String {
        s.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
    };
    let program = text_after(&lower, "took over from ")
        .map(first_word)
        .filter(|p| models::valid_program(p))
        .or_else(|| Some(t.app.clone()).filter(|p| models::valid_program(p)))
        .or_else(|| {
            Some(t.worker_agent.split('/').next().unwrap_or("").to_string())
                .filter(|p| models::valid_program(p))
        })?;
    let until = text_after(&lower, "out of allowance until ")
        .map(|rest| {
            let end = rest.find(['.', ';', '\n', '(']).unwrap_or(rest.len());
            rest[..end].trim().to_string()
        })
        .unwrap_or_default();
    Some(Hit { program, until, logged_out, task: t.tag.clone() })
}

/// The hits among the live tasks (not done, not archived).
pub fn hits_of(tasks: &[Task]) -> Vec<Hit> {
    tasks
        .iter()
        .filter(|t| !t.archived && t.status != Status::Done)
        .filter_map(hit_of)
        .collect()
}

/* ───────── the state of one program ───────── */

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Window {
    pub label: String,
    pub percent: f64,
    pub resets_at_ms: u64,
    pub severity: String,
}

/// Whether the program can be given work now, and how that is known.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Avail {
    /// `missing` (not installed), `signed_out`, `limited`, `ok`, `unknown`.
    pub state: &'static str,
    /// When the allowance comes back (ms) when Omarchy's windows say it; 0 otherwise.
    pub until_ms: u64,
    /// The board's own words for when (`17:00`); empty when it does not say.
    pub until: String,
    /// Why: the task that said so, the window that is spent, what `doctor` printed, or why nothing is known.
    pub reason: String,
    /// Where it comes from: `board`, `omarchy`, `doctor`, or empty.
    pub source: &'static str,
}

/// Everything known about one program, as the page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct State {
    pub signed_in: Option<bool>,
    /// The account or plan (`Max 5x`, `ChatGPT sign-in (go plan)`); empty when unknown.
    pub account: String,
    /// Why the account (or the sign-in) is unknown, when it is.
    pub unknown_why: String,
    pub avail: Avail,
    pub windows: Vec<Window>,
    /// Why the limits are not known, when no window is.
    pub limit_why: String,
    pub stale: bool,
}

pub struct Facts<'a> {
    pub program: &'a str,
    pub installed: bool,
    /// What `doctor` says about signing in: `None` when it says nothing.
    pub doctor_signed_in: Option<bool>,
    pub doctor_line: &'a str,
    /// The sign-in `herdr-projects models <program>` names (`your ChatGPT sign-in (go plan)`), or empty.
    pub models_account: &'a str,
    pub usage: &'a AgentUsage,
    pub hits: &'a [Hit],
    pub now: u64,
}

/// `codex takes the names codex lists for the account it is signed in with (here: your ChatGPT sign-in (go plan)).
/// Write one …` → `ChatGPT sign-in (go plan)`.
pub fn account_of_note(note: &str) -> String {
    let Some(rest) = text_after(note, "(here: ") else { return String::new() };
    let end = rest.find("). ").or_else(|| rest.rfind(')')).unwrap_or(rest.len());
    let acc = rest[..end].trim();
    acc.strip_prefix("your ").unwrap_or(acc).trim().to_string()
}

pub fn judge(f: &Facts) -> State {
    let u = f.usage;
    let account_note = account_of_note(f.models_account);
    let windows: Vec<Window> = u
        .limits
        .iter()
        .map(|w| Window {
            label: w.label.clone(),
            percent: w.percent,
            resets_at_ms: w.resets_at_ms,
            severity: w.severity.clone(),
        })
        .collect();
    let api_keys = f.doctor_line.to_lowercase().contains("api keys")
        || u.status.to_lowercase().contains("api keys");

    let signed_in = if let Some(s) = f.doctor_signed_in {
        Some(s)
    } else if !account_note.is_empty() {
        Some(true)
    } else if u.present && u.ready && (!u.limits.is_empty() || !u.tier.is_empty()) {
        Some(true)
    } else if u.present && !u.ready && !u.auth_help.is_empty() {
        Some(false)
    } else {
        None
    };
    let account = if !account_note.is_empty() {
        account_note
    } else {
        u.tier.clone()
    };
    let unknown_why = if signed_in.is_none() {
        if api_keys {
            "программа работает по ключам API: входа в аккаунт нет, доска его не проверяет".to_string()
        } else if u.present {
            format!("доска (doctor) молчит о входе, а запись Omarchy о {} его не содержит", f.program)
        } else {
            format!("доска (doctor) молчит о входе в {}, а Omarchy не пишет её запись", f.program)
        }
    } else if account.is_empty() {
        if api_keys {
            "ключи API: аккаунта и тарифа нет".to_string()
        } else {
            format!("{} не сообщает ни аккаунт, ни тариф ни доске, ни Omarchy", f.program)
        }
    } else {
        String::new()
    };
    let limit_why = if windows.is_empty() {
        if api_keys {
            "у ключей API нет окон лимита; об исчерпании скажет доска («out of allowance»)".to_string()
        } else if u.present {
            u.note.clone()
        } else {
            format!("Omarchy не пишет запись для {}; об исчерпании скажет доска («out of allowance»)", f.program)
        }
    } else {
        String::new()
    };

    let mine: Vec<&Hit> = f.hits.iter().filter(|h| h.program == f.program).collect();
    let spent: Vec<&Window> = windows
        .iter()
        .filter(|w| w.percent >= SPENT_AT && (w.resets_at_ms == 0 || w.resets_at_ms > f.now))
        .collect();
    let avail = if !f.installed {
        Avail { state: "missing", until_ms: 0, until: String::new(), reason: "программы нет на этой машине".into(), source: "" }
    } else if signed_in == Some(false) || mine.iter().any(|h| h.logged_out) {
        let reason = mine
            .iter()
            .find(|h| h.logged_out)
            .map(|h| format!("задача {}: вышла из аккаунта (logged out)", h.task))
            .or_else(|| Some(f.doctor_line.to_string()).filter(|l| !l.is_empty()))
            .or_else(|| Some(u.auth_help.clone()).filter(|l| !l.is_empty()))
            .unwrap_or_else(|| "вход не выполнен".into());
        let source = if mine.iter().any(|h| h.logged_out) { "board" } else if f.doctor_signed_in == Some(false) { "doctor" } else { "omarchy" };
        Avail { state: "signed_out", until_ms: 0, until: String::new(), reason, source }
    } else if let Some(h) = mine.iter().find(|h| !h.logged_out) {
        Avail {
            state: "limited",
            until_ms: 0,
            until: h.until.clone(),
            reason: format!("задача {}: out of allowance{}", h.task, if h.until.is_empty() { String::new() } else { format!(" until {}", h.until) }),
            source: "board",
        }
    } else if !spent.is_empty() {
        let until_ms = spent.iter().map(|w| w.resets_at_ms).max().unwrap_or(0);
        Avail {
            state: "limited",
            until_ms,
            until: String::new(),
            reason: format!("израсходовано: {}", spent.iter().map(|w| w.label.as_str()).collect::<Vec<_>>().join(", ")),
            source: "omarchy",
        }
    } else if signed_in == Some(true) {
        let worst = windows.iter().filter(|w| w.resets_at_ms == 0 || w.resets_at_ms > f.now).map(|w| w.percent).fold(0.0, f64::max);
        let reason = if worst >= CRIT_AT || worst >= WARN_AT {
            format!("лимит почти исчерпан: занято {} %", (worst * 100.0).round() as u32)
        } else {
            String::new()
        };
        Avail { state: "ok", until_ms: 0, until: String::new(), reason, source: if windows.is_empty() { "" } else { "omarchy" } }
    } else {
        Avail { state: "unknown", until_ms: 0, until: String::new(), reason: unknown_why.clone(), source: "" }
    };
    State { signed_in, account, unknown_why, avail, windows, limit_why, stale: u.stale }
}

/* ───────── providers inside omp, pi and opencode ───────── */

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provider {
    pub name: String,
    /// `list` (the program's own model list names it) or `usage` (a model it was used with; the provider is guessed).
    pub source: &'static str,
}

/// The provider a bare model name belongs to, by its family, or empty when it is not a known one.
pub fn family_of(model: &str) -> &'static str {
    let m = model.to_lowercase();
    if m.starts_with("deepseek") {
        "deepseek"
    } else if m.starts_with("kimi") || m.starts_with("moonshot") || (m.starts_with('k') && m[1..].starts_with(|c: char| c.is_ascii_digit())) {
        "kimi"
    } else if m.starts_with("claude") {
        "anthropic"
    } else if m.starts_with("gpt") || m.starts_with("codex") || (m.starts_with('o') && m[1..].starts_with(|c: char| c.is_ascii_digit())) {
        "openai"
    } else if m.starts_with("gemini") {
        "google"
    } else if m.starts_with("qwen") {
        "qwen"
    } else if m.starts_with("glm") {
        "zai"
    } else if m.starts_with("grok") {
        "xai"
    } else {
        ""
    }
}

/// `program/provider/model` → `provider`; nothing for a name that has no provider part.
fn provider_of_listed(program: &str, name: &str) -> Option<String> {
    let rest = name.strip_prefix(&format!("{program}/"))?;
    let (provider, model) = rest.split_once('/')?;
    (!provider.is_empty() && !model.is_empty()).then(|| provider.to_string())
}

/// The providers inside a program: from its model list, then from the models it was used with. Only for the programs
/// that carry several (omp, pi, opencode).
pub fn providers_of(program: &str, names: &[String], seen: &[String]) -> Vec<Provider> {
    if !MULTI.contains(&program) {
        return Vec::new();
    }
    let mut out: Vec<Provider> = Vec::new();
    let mut add = |name: String, source: &'static str| {
        if !name.is_empty() && !out.iter().any(|p| p.name == name) {
            out.push(Provider { name, source });
        }
    };
    for n in names {
        if let Some(p) = provider_of_listed(program, n) {
            add(p, "list");
        }
    }
    for s in seen {
        let provider = match s.split_once('/') {
            Some((p, _)) => p.to_string(),
            None => family_of(s).to_string(),
        };
        add(provider, "usage");
    }
    out
}

/// Names a program with no list of its own can be offered with, from the models it was used with:
/// `deepseek-v4-flash` → `omp/deepseek/deepseek-v4-flash`. Only for the multi-provider programs; others have none.
pub fn seen_names(program: &str, seen: &[String]) -> Vec<String> {
    if !MULTI.contains(&program) {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for s in seen {
        let name = if s.contains('/') {
            format!("{program}/{s}")
        } else {
            match family_of(s) {
                "" => continue,
                fam => format!("{program}/{fam}/{s}"),
            }
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/* ───────── who uses a program ───────── */

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoleUse {
    pub role: &'static str,
    /// `agent` or `fallback`.
    pub field: &'static str,
    pub value: String,
}

pub fn program_of(value: &str) -> &str {
    value.split('/').next().unwrap_or("").trim()
}

/// The programs a fallback list names, in order (`codex, claude/claude-opus-5-5` → codex, claude).
pub fn chain_programs(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|i| program_of(i).to_string())
        .filter(|p| !p.is_empty() && p != "none")
        .collect()
}

pub fn roles_using(program: &str, settings: &[Setting], light: &str) -> Vec<RoleUse> {
    let mut out = Vec::new();
    for r in ROLES {
        if r.id == "light" {
            if !light.is_empty() && program_of(light) == program {
                out.push(RoleUse { role: "light", field: "agent", value: light.to_string() });
            }
            continue;
        }
        if let Some(s) = find(settings, r.agent) {
            if program_of(&s.value) == program {
                out.push(RoleUse { role: r.id, field: "agent", value: s.value.clone() });
            }
        }
        if !r.fallback.is_empty() {
            if let Some(s) = find(settings, r.fallback) {
                if chain_programs(&s.value).iter().any(|p| p == program) {
                    out.push(RoleUse { role: r.id, field: "fallback", value: s.value.clone() });
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskUse {
    pub id: String,
    pub title: String,
    pub status: &'static str,
}

/// The live tasks whose agent is `program` (what the board's row says it runs now), newest first as the snapshot has them.
pub fn tasks_using(program: &str, tasks: &[Task]) -> Vec<TaskUse> {
    tasks
        .iter()
        .filter(|t| !t.archived && matches!(t.status, Status::Working | Status::Review))
        .filter(|t| t.app == program || (t.app.is_empty() && program_of(&t.worker_agent) == program))
        .map(|t| TaskUse { id: t.tag.clone(), title: t.title.clone(), status: t.status.key() })
        .collect()
}

/* ───────── presets ───────── */

/// What a preset says for one role. An empty field means «not set»: applying it takes the board's own entry away,
/// so the value below it (the repository's, the built-in) shows again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSet {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub fallback: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub roles: BTreeMap<String, RoleSet>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub presets: Vec<Preset>,
    /// The preset last applied, by level: `""` for «для меня», or a repository's name.
    #[serde(default)]
    pub active: BTreeMap<String, String>,
}

pub const MAX_PRESETS: usize = 40;
const MAX_NAME: usize = 40;

pub fn valid_preset_name(name: &str) -> Result<String, String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        return Err("у набора должно быть имя".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("имя набора — не длиннее {MAX_NAME} знаков"));
    }
    if n.chars().any(char::is_control) {
        return Err("в имени набора не может быть управляющих знаков".into());
    }
    Ok(n)
}

/// One fallback list: `none`, or agents separated by commas — each a program, or `program/model`.
pub fn check_fallback(value: &str, listed: &dyn Fn(&str) -> Vec<String>) -> Result<(), String> {
    let v = value.trim();
    if v == "none" {
        return Ok(());
    }
    let mut seen: Vec<&str> = Vec::new();
    for item in v.split(',').map(str::trim) {
        if item.is_empty() || item == "none" {
            return Err(format!("«{value}»: в списке запасных пустое место или none. Нужно «программа» или «программа/модель» через запятую"));
        }
        let program = agent_program(item).map_err(|_| format!("«{item}»: нужно «программа» или «программа/модель»"))?;
        if !PROGRAMS.contains(&program.as_str()) {
            return Err(format!("«{item}»: такой программы в экипаже нет"));
        }
        if seen.contains(&item) {
            return Err(format!("«{item}» назван в списке запасных дважды"));
        }
        seen.push(item);
        if item.contains('/') {
            check_agent(item, &listed(&program))?;
        }
    }
    Ok(())
}

/// A preset's field values by setting name, for the role.
fn field_of<'a>(set: &'a RoleSet, which: &str) -> &'a str {
    match which {
        "agent" => &set.agent,
        "effort" => &set.effort,
        _ => &set.fallback,
    }
}

/// One thing applying a preset does.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    pub role: &'static str,
    /// `agent`, `effort` or `fallback`.
    pub field: &'static str,
    /// The board's setting (`worker.agent`), or `light` (the planner's own preset).
    pub setting: String,
    pub was: String,
    pub was_from: String,
    /// `None`: the board's `unset`.
    pub now: Option<String>,
    /// Why this step cannot run; empty when it can.
    pub error: String,
}

impl Step {
    pub fn light(&self) -> bool {
        self.setting == "light"
    }
}

/// The steps that bring the board to the preset, in the order they run: the roles in order, the agent before the
/// effort and the fallback. A field that already is what the preset says is not a step. `repo` empty is «для меня»;
/// the light preset is the planner's own and only changes at that level.
pub fn plan(preset: &Preset, settings: &[Setting], light: &str, repo: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for r in ROLES {
        let Some(set) = preset.roles.get(r.id) else { continue };
        if r.id == "light" {
            if repo.is_empty() && set.agent != light {
                steps.push(Step {
                    role: "light",
                    field: "agent",
                    setting: "light".into(),
                    was: light.to_string(),
                    was_from: String::new(),
                    now: Some(set.agent.clone()).filter(|v| !v.is_empty()),
                    error: String::new(),
                });
            }
            continue;
        }
        for (field, name) in [("agent", r.agent), ("effort", r.effort), ("fallback", r.fallback)] {
            if name.is_empty() {
                continue;
            }
            let want = field_of(set, field);
            let cur = find(settings, name);
            let was = cur.map(|s| s.value.clone()).unwrap_or_default();
            let was_from = cur.map(|s| s.from.clone()).unwrap_or_default();
            let own = cur.is_some_and(|s| s.own);
            let step = if want.is_empty() {
                own.then(|| None)
            } else if was == want {
                None
            } else {
                Some(Some(want.to_string()))
            };
            let Some(now) = step else { continue };
            steps.push(Step {
                role: r.id,
                field,
                setting: name.to_string(),
                was,
                was_from,
                now,
                error: String::new(),
            });
        }
    }
    steps
}

/// Checks every step against what the planner can know (shapes, the models and effort levels the programs list) and
/// writes the problem on the step. `models_of(program)` gives the program's model names and effort levels.
pub fn check_steps(
    steps: &mut [Step],
    preset: &Preset,
    settings: &[Setting],
    models_of: &dyn Fn(&str) -> (Vec<String>, Vec<String>),
) {
    for i in 0..steps.len() {
        let Some(value) = steps[i].now.clone() else { continue };
        let role = steps[i].role;
        let err = match steps[i].field {
            "agent" => agent_program(&value).and_then(|p| {
                if value.contains('/') { check_agent(&value, &models_of(&p).0) } else { Ok(()) }
            }),
            "effort" => {
                // The effort belongs to the agent the role will have, not the one it has now.
                let agent = ROLES
                    .iter()
                    .find(|r| r.id == role)
                    .and_then(|r| {
                        let want = preset.roles.get(role).map(|s| s.agent.clone()).unwrap_or_default();
                        if want.is_empty() { find(settings, r.agent).map(|s| s.value.clone()) } else { Some(want) }
                    })
                    .unwrap_or_default();
                let program = program_of(&agent).to_string();
                if value == "default" || program.is_empty() { Ok(()) } else { check_effort(&value, &models_of(&program).1, &program) }
            }
            "fallback" => check_fallback(&value, &|p: &str| models_of(p).0),
            _ => Ok(()),
        };
        if let Err(e) = err {
            steps[i].error = e;
        }
    }
}

/// The board's command for a step: the arguments after `herdr-projects`.
pub fn command_of(step: &Step, repo: &str) -> Option<Vec<String>> {
    if step.light() {
        return None;
    }
    let mut args = match &step.now {
        Some(v) => vec!["set".to_string(), step.setting.clone(), v.clone()],
        None => vec!["unset".to_string(), step.setting.clone()],
    };
    if !repo.is_empty() {
        args.push("--repo".into());
        args.push(repo.to_string());
    }
    Some(args)
}

/// What a preset's apply does to the tasks that are live: the running ones carry on with the agent they have and the
/// waiting ones start with the new settings; the lists name them.
pub fn affected(tasks: &[Task], repo: &str) -> Value {
    let live = |t: &&Task| !t.archived && t.status != Status::Done && (repo.is_empty() || t.repo == repo);
    let row = |t: &Task| json!({"id": t.id, "tag": t.tag, "title": t.title, "status": t.status.key(), "app": t.app});
    let running: Vec<Value> = tasks.iter().filter(live).filter(|t| matches!(t.status, Status::Working | Status::Review)).map(row).collect();
    let waiting: Vec<Value> = tasks.iter().filter(live).filter(|t| !matches!(t.status, Status::Working | Status::Review)).map(row).collect();
    json!({"running": running, "waiting": waiting})
}

/// The current crew as a preset: what the settings say now. A value that is only the board's built-in is left empty,
/// so applying the preset back changes nothing and does not pin the built-in.
pub fn capture(settings: &[Setting], light: &str) -> BTreeMap<String, RoleSet> {
    let pick = |name: &str| -> String {
        if name.is_empty() {
            return String::new();
        }
        match find(settings, name) {
            Some(s) if s.from != "built in" && s.place != "builtin" => s.value.clone(),
            _ => String::new(),
        }
    };
    let mut roles = BTreeMap::new();
    for r in ROLES {
        let set = if r.id == "light" {
            RoleSet { agent: light.to_string(), ..RoleSet::default() }
        } else {
            RoleSet { agent: pick(r.agent), effort: pick(r.effort), fallback: pick(r.fallback) }
        };
        roles.insert(r.id.to_string(), set);
    }
    roles
}

/// Where the active preset stands at a level: the recorded one (or, with none recorded, the first that matches) and
/// how many steps still separate the crew from it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Active {
    pub id: String,
    pub name: String,
    /// `matches` or `differs`.
    pub state: &'static str,
    pub diff: usize,
}

pub fn active_of(store: &Store, diffs: &[(String, usize)], repo: &str) -> Option<Active> {
    let pick = |id: &str| {
        let p = store.presets.iter().find(|p| p.id == id)?;
        let diff = diffs.iter().find(|(i, _)| i == id).map_or(0, |(_, d)| *d);
        Some(Active { id: p.id.clone(), name: p.name.clone(), state: if diff == 0 { "matches" } else { "differs" }, diff })
    };
    if let Some(a) = store.active.get(repo).and_then(|id| pick(id)) {
        return Some(a);
    }
    diffs.iter().find(|(_, d)| *d == 0).and_then(|(id, _)| pick(id))
}

/* ───────── templates: the examples of the task, built from what this machine has ───────── */

/// What the machine offers for one program: the template builder's input.
#[derive(Debug, Clone, Default)]
pub struct Cat {
    pub program: String,
    /// Installed, and neither signed out nor out of allowance.
    pub usable: bool,
    pub names: Vec<String>,
    pub effort: Vec<String>,
}

fn first_with(cat: &Cat, words: &[&str]) -> Option<String> {
    for w in words {
        if let Some(n) = cat.names.iter().find(|n| n.to_lowercase().contains(w)) {
            return Some(n.clone());
        }
    }
    cat.names.first().cloned()
}

fn top_effort(cat: &Cat) -> String {
    for want in ["max", "xhigh", "high"] {
        if cat.effort.iter().any(|e| e == want) {
            return want.to_string();
        }
    }
    String::new()
}

fn effort_for(cat: &Cat, want: &str) -> String {
    if cat.effort.iter().any(|e| e == want) { want.to_string() } else { String::new() }
}

/// The preset for a kind (`max`, `claude`, `economy`, `noclaude`) from the programs at hand. An error says what is missing.
pub fn template(kind: &str, cats: &[Cat]) -> Result<(String, BTreeMap<String, RoleSet>), String> {
    let get = |p: &str| cats.iter().find(|c| c.program == p && c.usable);
    let claude = get("claude");
    let need_claude = || claude.ok_or_else(|| "Claude не установлен или не вошёл".to_string());
    let mut roles: BTreeMap<String, RoleSet> = BTreeMap::new();
    let mut put = |role: &str, agent: Option<String>, effort: String, fallback: String| {
        roles.insert(role.to_string(), RoleSet { agent: agent.unwrap_or_default(), effort, fallback });
    };
    let name = match kind {
        "max" => {
            let c = need_claude()?;
            let opus = first_with(c, &["opus"]).ok_or("у Claude нет моделей в списке")?;
            let e = top_effort(c);
            for r in ["coordinator", "plan", "reviewer", "worker"] {
                put(r, Some(opus.clone()), if r == "plan" { String::new() } else { e.clone() }, if matches!(r, "reviewer" | "worker") { "none".into() } else { String::new() });
            }
            put("light", Some(opus), String::new(), String::new());
            "Максимум"
        }
        "claude" => {
            let c = need_claude()?;
            let opus = first_with(c, &["opus"]).ok_or("у Claude нет моделей в списке")?;
            let sonnet = first_with(c, &["sonnet", "opus"]).unwrap_or_else(|| opus.clone());
            let haiku = first_with(c, &["haiku", "sonnet"]).unwrap_or_else(|| sonnet.clone());
            put("coordinator", Some(opus.clone()), effort_for(c, "medium"), String::new());
            put("plan", Some(opus.clone()), String::new(), String::new());
            put("reviewer", Some(opus.clone()), effort_for(c, "high"), sonnet.clone());
            put("worker", Some(sonnet.clone()), effort_for(c, "high"), haiku.clone());
            put("light", Some(haiku), String::new(), String::new());
            "Только Claude"
        }
        "economy" => {
            let cheap = cats
                .iter()
                .filter(|c| c.usable && c.program != "claude")
                .find_map(|c| {
                    let flash = c.names.iter().find(|n| n.contains("deepseek") && n.contains("flash")).cloned();
                    flash
                        .or_else(|| c.names.iter().find(|n| n.contains("kimi") || n.contains("deepseek")).cloned())
                        .or_else(|| (c.program == "kimi").then(|| c.names.first().cloned().unwrap_or_else(|| "kimi".into())))
                })
                .ok_or("нет дешёвой подписки: нужен omp с deepseek или kimi")?;
            let cheap_effort = effort_for(cats.iter().find(|c| c.program == program_of(&cheap)).unwrap_or(&Cat::default()), "high");
            let (big, mid, fb) = match claude {
                Some(c) => (
                    first_with(c, &["opus"]),
                    first_with(c, &["sonnet", "opus"]),
                    first_with(c, &["sonnet", "opus"]),
                ),
                None => (Some(cheap.clone()), Some(cheap.clone()), None),
            };
            let ce = claude.map(|c| effort_for(c, "medium")).unwrap_or_default();
            put("coordinator", mid.clone(), ce.clone(), String::new());
            put("plan", big.clone(), String::new(), String::new());
            put("reviewer", mid, claude.map(|c| effort_for(c, "high")).unwrap_or_default(), cheap.clone());
            put("worker", Some(cheap.clone()), cheap_effort, fb.unwrap_or_else(|| "none".into()));
            put("light", Some(cheap), String::new(), String::new());
            "Экономный"
        }
        "noclaude" => {
            let others: Vec<&Cat> = cats.iter().filter(|c| c.usable && c.program != "claude" && !c.names.is_empty()).collect();
            let strong = others
                .iter()
                .find(|c| c.program == "codex")
                .or_else(|| others.first())
                .ok_or("нет подписки, кроме Claude")?;
            let big = first_with(strong, &["gpt-6", "pro", "opus"]).ok_or("нет моделей")?;
            let se = top_effort(strong);
            let cheap = others
                .iter()
                .find_map(|c| c.names.iter().find(|n| n.contains("deepseek") && n.contains("flash")).cloned())
                .unwrap_or_else(|| big.clone());
            let chain = |skip: &str| -> String {
                let items: Vec<String> = others
                    .iter()
                    .filter(|c| c.program != skip)
                    .filter_map(|c| first_with(c, &["flash", "mini", "gpt-5"]))
                    .collect();
                if items.is_empty() { "none".into() } else { items.join(", ") }
            };
            put("coordinator", Some(big.clone()), se.clone(), String::new());
            put("plan", Some(big.clone()), String::new(), String::new());
            put("reviewer", Some(big.clone()), se, chain(strong.program.as_str()));
            put("worker", Some(cheap.clone()), String::new(), chain(program_of(&cheap)));
            put("light", Some(cheap), String::new(), String::new());
            "Без Claude"
        }
        _ => return Err("такого шаблона нет".into()),
    };
    Ok((name.to_string(), roles))
}

/* ───────── the presets file ───────── */

static FILE_LOCK: Mutex<()> = Mutex::new(());

fn presets_file(cfg: &AppConfig) -> PathBuf {
    cfg.state_dir.join("crew-presets.json")
}

fn read_store_at(path: &Path) -> Store {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Store>(&t).ok())
        .unwrap_or_default()
}

pub fn read_store(cfg: &AppConfig) -> Store {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read_store_at(&presets_file(cfg))
}

/// Reads the file, lets `f` change it, writes it back (a temporary file, then a rename) — all under one lock.
pub fn edit_store<T>(cfg: &AppConfig, f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = presets_file(cfg);
    let mut store = read_store_at(&path);
    let out = f(&mut store)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, text).map_err(|e| format!("не записать {}: {e}", path.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(out)
}

fn fresh_id(store: &Store) -> String {
    let mut n = now_ms();
    loop {
        let id = format!("p{n:x}");
        if !store.presets.iter().any(|p| p.id == id) {
            return id;
        }
        n += 1;
    }
}

fn name_taken(store: &Store, name: &str, except: &str) -> bool {
    store.presets.iter().any(|p| p.id != except && p.name.to_lowercase() == name.to_lowercase())
}

/// A role map with only the roles and fields a preset can hold, each value shaped (the agents `program` or
/// `program/model`, efforts one word, fallbacks `none` or a list).
pub fn clean_roles(roles: &BTreeMap<String, RoleSet>) -> Result<BTreeMap<String, RoleSet>, String> {
    let mut out = BTreeMap::new();
    for (role, set) in roles {
        if !ROLES.iter().any(|r| r.id == role) {
            return Err(format!("«{role}»: такой роли нет"));
        }
        let agent = set.agent.trim().to_string();
        if !agent.is_empty() {
            agent_program(&agent)?;
        }
        let effort = set.effort.trim().to_string();
        if effort.len() > 24 || !effort.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("«{effort}»: так усилие не называется"));
        }
        let fallback = set.fallback.trim().to_string();
        check_fallback(if fallback.is_empty() { "none" } else { &fallback }, &|_| Vec::new())?;
        out.insert(role.clone(), RoleSet { agent, effort, fallback });
    }
    Ok(out)
}

pub fn add_preset(store: &mut Store, name: &str, roles: &BTreeMap<String, RoleSet>) -> Result<String, String> {
    let name = valid_preset_name(name)?;
    if store.presets.len() >= MAX_PRESETS {
        return Err(format!("наборов уже {MAX_PRESETS}: удалите ненужные"));
    }
    if name_taken(store, &name, "") {
        return Err(format!("набор «{name}» уже есть"));
    }
    let id = fresh_id(store);
    store.presets.push(Preset { id: id.clone(), name, roles: clean_roles(roles)? });
    Ok(id)
}

pub fn rename_preset(store: &mut Store, id: &str, name: &str) -> Result<(), String> {
    let name = valid_preset_name(name)?;
    if name_taken(store, &name, id) {
        return Err(format!("набор «{name}» уже есть"));
    }
    let p = store.presets.iter_mut().find(|p| p.id == id).ok_or("такого набора нет")?;
    p.name = name;
    Ok(())
}

pub fn update_preset(store: &mut Store, id: &str, roles: &BTreeMap<String, RoleSet>) -> Result<(), String> {
    let roles = clean_roles(roles)?;
    let p = store.presets.iter_mut().find(|p| p.id == id).ok_or("такого набора нет")?;
    p.roles = roles;
    Ok(())
}

pub fn delete_preset(store: &mut Store, id: &str) -> Result<(), String> {
    let before = store.presets.len();
    store.presets.retain(|p| p.id != id);
    if store.presets.len() == before {
        return Err("такого набора нет".into());
    }
    store.active.retain(|_, v| v != id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::LimitWindow;

    fn setting(name: &str, value: &str, from: &str) -> Setting {
        Setting {
            name: name.into(),
            kind: "work".into(),
            value: value.into(),
            from: from.into(),
            place: if from == "built in" { "builtin".into() } else { "you".into() },
            own: from != "built in",
            note: String::new(),
        }
    }

    fn crew() -> Vec<Setting> {
        vec![
            setting("coordinator.agent", "claude/claude-opus-5-5", "you"),
            setting("coordinator.effort", "medium", "you"),
            setting("plan.agent", "claude/claude-opus-5-5", "you"),
            setting("reviewer.agent", "claude/claude-opus-5-5", "you"),
            setting("reviewer.effort", "high", "you"),
            setting("reviewer.fallback", "none", "built in"),
            setting("worker.agent", "claude/claude-sonnet-5-5", "you"),
            setting("worker.effort", "high", "you"),
            setting("worker.fallback", "none", "built in"),
        ]
    }

    fn roles(pairs: &[(&str, &str, &str, &str)]) -> BTreeMap<String, RoleSet> {
        pairs
            .iter()
            .map(|(r, a, e, f)| (r.to_string(), RoleSet { agent: a.to_string(), effort: e.to_string(), fallback: f.to_string() }))
            .collect()
    }

    fn preset(pairs: &[(&str, &str, &str, &str)]) -> Preset {
        Preset { id: "p1".into(), name: "Тест".into(), roles: roles(pairs) }
    }

    fn task(tag: &str, status: Status, app: &str, msg: &str) -> Task {
        Task {
            id: format!("planner/{tag}"),
            repo: "planner".into(),
            tag: tag.into(),
            title: format!("Задача {tag}"),
            msg: msg.into(),
            app: app.into(),
            status,
            ..Task::default()
        }
    }

    #[test]
    fn a_task_line_that_says_out_of_allowance_marks_the_program_it_took_over_from() {
        let t = task("PLN-1", Status::Working, "codex", "codex took over from claude: out of allowance until 17:00");
        let h = hit_of(&t).unwrap();
        assert_eq!(h.program, "claude");
        assert_eq!(h.until, "17:00");
        assert!(!h.logged_out);
        let t = task("PLN-2", Status::Queued, "codex", "Out of allowance");
        let h = hit_of(&t).unwrap();
        assert_eq!((h.program.as_str(), h.until.as_str()), ("codex", ""));
        let t = task("PLN-3", Status::Failed, "omp", "omp took over from pi: logged out");
        assert!(hit_of(&t).unwrap().logged_out);
        assert!(hit_of(&task("PLN-4", Status::Working, "claude", "Writing the tests")).is_none());
    }

    #[test]
    fn done_and_archived_tasks_say_nothing_about_the_allowance() {
        let mut done = task("PLN-1", Status::Done, "claude", "out of allowance");
        let live = task("PLN-2", Status::Working, "codex", "out of allowance until 5pm");
        assert_eq!(hits_of(&[done.clone(), live]).len(), 1);
        done.status = Status::Failed;
        done.archived = true;
        assert!(hits_of(&[done]).is_empty());
    }

    fn usage(limits: Vec<LimitWindow>, ready: bool, tier: &str) -> AgentUsage {
        AgentUsage { present: true, ready, tier: tier.into(), limits, ..AgentUsage::default() }
    }

    fn win(label: &str, percent: f64, resets: u64) -> LimitWindow {
        LimitWindow { label: label.into(), percent, resets_at: String::new(), resets_at_ms: resets, severity: crate::usage::severity(percent).into() }
    }

    fn facts<'a>(program: &'a str, u: &'a AgentUsage, hits: &'a [Hit]) -> Facts<'a> {
        Facts { program, installed: true, doctor_signed_in: None, doctor_line: "", models_account: "", usage: u, hits, now: 1_000 }
    }

    #[test]
    fn the_account_comes_from_the_models_note_or_the_tier() {
        let note = "codex takes the names codex lists for the account it is signed in with (here: your ChatGPT sign-in (go plan)). Write one after the program, as in worker.agent codex/gpt-6-luna:";
        assert_eq!(account_of_note(note), "ChatGPT sign-in (go plan)");
        assert_eq!(account_of_note("claude takes a short name"), "");
        let none = AgentUsage::default();
        let mut f = facts("codex", &none, &[]);
        f.models_account = note;
        let s = judge(&f);
        assert_eq!(s.signed_in, Some(true));
        assert_eq!(s.account, "ChatGPT sign-in (go plan)");
        assert_eq!(s.avail.state, "ok");
        let u = usage(vec![win("Session (5-hour)", 0.02, 5_000)], true, "Max 5x");
        let mut f = facts("claude", &u, &[]);
        f.doctor_signed_in = Some(true);
        let s = judge(&f);
        assert_eq!(s.account, "Max 5x");
        assert_eq!(s.windows.len(), 1);
        assert!(s.unknown_why.is_empty());
    }

    #[test]
    fn what_cannot_be_known_is_said_with_the_reason() {
        let none = AgentUsage::default();
        let s = judge(&facts("devin", &none, &[]));
        assert_eq!(s.signed_in, None);
        assert_eq!(s.avail.state, "unknown");
        assert!(s.unknown_why.contains("doctor") && s.unknown_why.contains("devin"), "{}", s.unknown_why);
        assert!(s.limit_why.contains("devin"), "{}", s.limit_why);
        let mut f = facts("omp", &none, &[]);
        f.doctor_line = "✓ omp 1.5.0, API keys: no sign-in needed";
        let s = judge(&f);
        assert!(s.unknown_why.contains("ключам API"), "{}", s.unknown_why);
        let mut f = facts("agy", &none, &[]);
        f.installed = false;
        assert_eq!(judge(&f).avail.state, "missing");
    }

    #[test]
    fn a_program_the_board_says_is_out_of_allowance_is_limited_until_its_time() {
        let u = usage(vec![win("Session (5-hour)", 0.30, 5_000)], true, "Max 5x");
        let hits = vec![Hit { program: "claude".into(), until: "17:00".into(), logged_out: false, task: "PLN-9".into() }];
        let mut f = facts("claude", &u, &hits);
        f.doctor_signed_in = Some(true);
        let s = judge(&f);
        assert_eq!(s.avail.state, "limited");
        assert_eq!(s.avail.until, "17:00");
        assert_eq!(s.avail.source, "board");
        assert!(s.avail.reason.contains("PLN-9") && s.avail.reason.contains("17:00"), "{}", s.avail.reason);
        // another program's hit does not touch this one
        let other = vec![Hit { program: "codex".into(), until: String::new(), logged_out: false, task: "PLN-9".into() }];
        let mut f = facts("claude", &u, &other);
        f.doctor_signed_in = Some(true);
        assert_eq!(judge(&f).avail.state, "ok");
    }

    #[test]
    fn a_spent_window_limits_the_program_until_it_resets() {
        let u = usage(vec![win("Session (5-hour)", 1.0, 9_000), win("Weekly (7-day)", 0.4, 90_000)], true, "Max 5x");
        let mut f = facts("claude", &u, &[]);
        f.doctor_signed_in = Some(true);
        let s = judge(&f);
        assert_eq!(s.avail.state, "limited");
        assert_eq!(s.avail.until_ms, 9_000);
        assert_eq!(s.avail.source, "omarchy");
        assert!(s.avail.reason.contains("Session"), "{}", s.avail.reason);
        // a window that has reset since is not spent
        let u = usage(vec![win("Session (5-hour)", 1.0, 500)], true, "Max 5x");
        let mut f = facts("claude", &u, &[]);
        f.doctor_signed_in = Some(true);
        assert_eq!(judge(&f).avail.state, "ok");
    }

    #[test]
    fn signed_out_beats_everything_and_says_how_to_sign_in() {
        let u = AgentUsage { present: true, ready: false, auth_help: "Start Grok, or run `grok login`, to sign in.".into(), ..AgentUsage::default() };
        let s = judge(&facts("grok", &u, &[]));
        assert_eq!(s.signed_in, Some(false));
        assert_eq!(s.avail.state, "signed_out");
        assert!(s.avail.reason.contains("grok login"));
        let none = AgentUsage::default();
        let mut f = facts("codex", &none, &[]);
        f.doctor_signed_in = Some(false);
        f.doctor_line = "✓ codex 0.9.2, not signed in: codex login";
        let s = judge(&f);
        assert_eq!(s.avail.state, "signed_out");
        assert_eq!(s.avail.source, "doctor");
    }

    #[test]
    fn providers_come_from_the_list_then_from_the_models_used() {
        let names: Vec<String> = ["pi/anthropic/claude-fable-5", "pi/openai/gpt-5", "pi/anthropic/claude-opus-5"].map(String::from).to_vec();
        let seen: Vec<String> = ["deepseek-v4-flash", "k3.1-pro", "yanao/small", "anthropic/claude-opus-5"].map(String::from).to_vec();
        let p = providers_of("pi", &names, &seen);
        let list: Vec<(&str, &str)> = p.iter().map(|x| (x.name.as_str(), x.source)).collect();
        assert_eq!(
            list,
            vec![("anthropic", "list"), ("openai", "list"), ("deepseek", "usage"), ("kimi", "usage"), ("yanao", "usage")]
        );
        assert!(providers_of("claude", &names, &seen).is_empty(), "claude has no providers inside");
        assert_eq!(providers_of("omp", &[], &["deepseek-flash".to_string()])[0].name, "deepseek");
    }

    #[test]
    fn a_program_with_no_list_is_offered_the_models_it_was_used_with() {
        let seen: Vec<String> = ["deepseek-v4-flash", "gibberish", "yanao/small"].map(String::from).to_vec();
        assert_eq!(seen_names("omp", &seen), vec!["omp/deepseek/deepseek-v4-flash", "omp/yanao/small"]);
        assert!(seen_names("kimi", &seen).is_empty());
    }

    #[test]
    fn roles_and_tasks_using_a_program() {
        let mut s = crew();
        s.push(setting("worker.fallback", "codex, claude/claude-opus-5-5", "you"));
        s.retain(|x| !(x.name == "worker.fallback" && x.value == "none"));
        let uses = roles_using("claude", &s, "claude/claude-haiku-5-5");
        let tags: Vec<String> = uses.iter().map(|u| format!("{}:{}", u.role, u.field)).collect();
        assert_eq!(tags, vec!["coordinator:agent", "plan:agent", "reviewer:agent", "worker:agent", "worker:fallback", "light:agent"]);
        assert_eq!(roles_using("codex", &s, "").len(), 1);
        assert!(roles_using("pi", &s, "").is_empty());
        let tasks = vec![
            task("PLN-1", Status::Working, "claude", ""),
            task("PLN-2", Status::Review, "codex", ""),
            task("PLN-3", Status::Done, "claude", ""),
            task("PLN-4", Status::Queued, "claude", ""),
        ];
        let ids: Vec<String> = tasks_using("claude", &tasks).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["PLN-1"]);
    }

    #[test]
    fn a_preset_is_a_plan_of_the_fields_that_differ_in_the_order_of_the_roles() {
        let p = preset(&[
            ("worker", "omp/deepseek/deepseek-v4-flash", "medium", "claude/claude-sonnet-5-5"),
            ("coordinator", "claude/claude-opus-5-5", "medium", ""),
            ("reviewer", "claude/claude-opus-5-5", "high", ""),
            ("light", "omp/deepseek/deepseek-v4-flash", "", ""),
        ]);
        let steps = plan(&p, &crew(), "", "");
        let list: Vec<String> = steps
            .iter()
            .map(|s| format!("{} {} → {}", s.setting, s.was, s.now.clone().unwrap_or("unset".into())))
            .collect();
        assert_eq!(
            list,
            vec![
                "worker.agent claude/claude-sonnet-5-5 → omp/deepseek/deepseek-v4-flash",
                "worker.effort high → medium",
                "worker.fallback none → claude/claude-sonnet-5-5",
                "light  → omp/deepseek/deepseek-v4-flash",
            ],
            "coordinator and reviewer already are what the preset says"
        );
        // the commands, in this order
        let cmds: Vec<Vec<String>> = steps.iter().filter_map(|s| command_of(s, "")).collect();
        assert_eq!(cmds[0], vec!["set", "worker.agent", "omp/deepseek/deepseek-v4-flash"]);
        assert_eq!(cmds.len(), 3, "the light preset is the planner's, not a command of the board");
    }

    #[test]
    fn an_empty_field_takes_the_boards_own_entry_away_only_when_there_is_one() {
        let p = preset(&[("worker", "claude/claude-sonnet-5-5", "", ""), ("plan", "", "", "")]);
        let steps = plan(&p, &crew(), "", "alpha");
        let cmds: Vec<Vec<String>> = steps.iter().filter_map(|s| command_of(s, "alpha")).collect();
        assert_eq!(
            cmds,
            vec![
                vec!["unset", "plan.agent", "--repo", "alpha"],
                vec!["unset", "worker.effort", "--repo", "alpha"],
            ],
            "worker.fallback is the built-in one: nothing to take away"
        );
        assert!(steps.iter().all(|s| s.now.is_none()));
    }

    #[test]
    fn the_light_preset_changes_only_for_me() {
        let p = preset(&[("light", "codex/gpt-6-luna", "", "")]);
        assert_eq!(plan(&p, &crew(), "", "alpha").len(), 0);
        assert_eq!(plan(&p, &crew(), "", "").len(), 1);
        assert_eq!(plan(&p, &crew(), "codex/gpt-6-luna", "").len(), 0);
    }

    #[test]
    fn capturing_the_crew_and_planning_it_back_changes_nothing() {
        let s = crew();
        let roles = capture(&s, "claude/claude-haiku-5-5");
        assert_eq!(roles["worker"].fallback, "", "the built-in none is not pinned");
        assert_eq!(roles["worker"].agent, "claude/claude-sonnet-5-5");
        assert_eq!(roles["light"].agent, "claude/claude-haiku-5-5");
        let p = Preset { id: "p".into(), name: "Сейчас".into(), roles };
        assert!(plan(&p, &s, "claude/claude-haiku-5-5", "").is_empty());
    }

    fn lookup(program: &str) -> (Vec<String>, Vec<String>) {
        match program {
            "claude" => (
                ["claude/claude-opus-5-5", "claude/claude-sonnet-5-5", "claude/claude-haiku-5-5"].map(String::from).to_vec(),
                ["low", "medium", "high", "xhigh", "max"].map(String::from).to_vec(),
            ),
            "codex" => (["codex/gpt-6-luna"].map(String::from).to_vec(), ["low", "high", "ultra"].map(String::from).to_vec()),
            _ => (Vec::new(), Vec::new()),
        }
    }

    #[test]
    fn steps_are_checked_against_the_models_and_the_effort_of_the_new_agent() {
        let p = preset(&[
            ("worker", "codex/gpt-6-luna", "ultra", "claude/claude-nope"),
            ("reviewer", "claude/claude-opus-5-5", "ultra", ""),
            ("coordinator", "claude/claude-opus-5-6", "default", ""),
        ]);
        let mut steps = plan(&p, &crew(), "", "");
        check_steps(&mut steps, &p, &crew(), &lookup);
        let by = |name: &str| steps.iter().find(|s| s.setting == name).unwrap();
        assert!(by("worker.agent").error.is_empty());
        assert!(by("worker.effort").error.is_empty(), "ultra is codex's, the effort of the NEW agent counts: {}", by("worker.effort").error);
        assert!(by("worker.fallback").error.contains("такой модели нет"), "{}", by("worker.fallback").error);
        assert!(by("reviewer.effort").error.contains("нет такого усилия"), "{}", by("reviewer.effort").error);
        assert!(by("coordinator.agent").error.contains("такой модели нет"));
    }

    #[test]
    fn a_fallback_chain_is_programs_and_models_with_commas() {
        let none = |_: &str| Vec::<String>::new();
        assert!(check_fallback("none", &none).is_ok());
        assert!(check_fallback("codex, claude/claude-opus-5-5", &none).is_ok());
        assert!(check_fallback("codex,,claude", &none).is_err());
        assert!(check_fallback("codex, codex", &none).unwrap_err().contains("дважды"));
        assert!(check_fallback("nosuch", &none).unwrap_err().contains("такой программы"));
        assert!(check_fallback("claude, none", &none).is_err());
        assert_eq!(chain_programs("codex, claude/claude-opus-5-5"), vec!["codex", "claude"]);
        assert!(chain_programs("none").is_empty());
    }

    #[test]
    fn who_is_touched_is_split_into_running_and_waiting() {
        let tasks = vec![
            task("PLN-1", Status::Working, "claude", ""),
            task("PLN-2", Status::Review, "claude", ""),
            task("PLN-3", Status::Queued, "", ""),
            task("PLN-4", Status::Waiting, "claude", ""),
            task("PLN-5", Status::Done, "claude", ""),
        ];
        let a = affected(&tasks, "");
        assert_eq!(a["running"].as_array().unwrap().len(), 2);
        assert_eq!(a["waiting"].as_array().unwrap().len(), 2);
        assert_eq!(affected(&tasks, "other")["running"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn the_active_preset_is_the_recorded_one_or_the_first_that_matches() {
        let mut store = Store::default();
        store.presets = vec![
            Preset { id: "a".into(), name: "A".into(), roles: BTreeMap::new() },
            Preset { id: "b".into(), name: "B".into(), roles: BTreeMap::new() },
        ];
        let diffs = vec![("a".to_string(), 3), ("b".to_string(), 0)];
        let a = active_of(&store, &diffs, "").unwrap();
        assert_eq!((a.id.as_str(), a.state), ("b", "matches"));
        store.active.insert(String::new(), "a".into());
        let a = active_of(&store, &diffs, "").unwrap();
        assert_eq!((a.id.as_str(), a.state, a.diff), ("a", "differs", 3));
        assert!(active_of(&store, &[("a".to_string(), 3), ("b".to_string(), 2)], "alpha").is_none());
        store.active.insert("alpha".into(), "gone".into());
        assert!(active_of(&store, &[("a".to_string(), 3)], "alpha").is_none());
    }

    #[test]
    fn presets_are_created_renamed_and_deleted_with_unique_names() {
        let mut store = Store::default();
        let r = roles(&[("worker", "claude/claude-sonnet-5-5", "high", "none")]);
        let id = add_preset(&mut store, "  Экономный  ", &r).unwrap();
        assert_eq!(store.presets[0].name, "Экономный");
        assert!(add_preset(&mut store, "экономный", &r).unwrap_err().contains("уже есть"));
        assert!(add_preset(&mut store, "  ", &r).is_err());
        assert!(add_preset(&mut store, &"я".repeat(41), &r).is_err());
        assert!(add_preset(&mut store, "Плохой", &roles(&[("captain", "", "", "")])).is_err());
        assert!(add_preset(&mut store, "Плохой", &roles(&[("worker", "Claude", "", "")])).is_err());
        let id2 = add_preset(&mut store, "Максимум", &r).unwrap();
        assert_ne!(id, id2);
        rename_preset(&mut store, &id2, "Максимум!").unwrap();
        assert!(rename_preset(&mut store, &id2, "экономный").is_err());
        rename_preset(&mut store, &id, "Экономный").unwrap();
        store.active.insert(String::new(), id.clone());
        delete_preset(&mut store, &id).unwrap();
        assert_eq!(store.presets.len(), 1);
        assert!(store.active.is_empty(), "the active mark goes with the preset");
        assert!(delete_preset(&mut store, &id).is_err());
    }

    #[test]
    fn the_presets_file_is_kept_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::testutil::test_cfg(dir.path());
        assert!(read_store(&cfg).presets.is_empty());
        let r = roles(&[("worker", "claude/claude-sonnet-5-5", "high", "codex")]);
        let id = edit_store(&cfg, |s| add_preset(s, "Экономный", &r)).unwrap();
        edit_store(&cfg, |s| {
            s.active.insert(String::new(), id.clone());
            Ok(())
        })
        .unwrap();
        let back = read_store(&cfg);
        assert_eq!(back.presets[0].roles["worker"].fallback, "codex");
        assert_eq!(back.active[""], id);
        // a refused change writes nothing
        assert!(edit_store(&cfg, |s| add_preset(s, "Экономный", &r)).is_err());
        assert_eq!(read_store(&cfg).presets.len(), 1);
        assert!(dir.path().join("state/crew-presets.json").exists());
        assert!(!dir.path().join("state/crew-presets.json.new").exists());
    }

    fn cat(program: &str, names: &[&str], effort: &[&str]) -> Cat {
        Cat {
            program: program.into(),
            usable: true,
            names: names.iter().map(|s| s.to_string()).collect(),
            effort: effort.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn machine() -> Vec<Cat> {
        vec![
            cat("claude", &["claude/opus", "claude/sonnet", "claude/haiku"], &["low", "medium", "high", "xhigh", "max"]),
            cat("codex", &["codex/gpt-6-luna", "codex/gpt-5.6-terra"], &["low", "high", "max"]),
            cat("omp", &["omp/deepseek/deepseek-v4-pro", "omp/deepseek/deepseek-v4-flash"], &["low", "high"]),
        ]
    }

    #[test]
    fn the_templates_are_built_from_the_programs_that_are_there() {
        let (name, r) = template("max", &machine()).unwrap();
        assert_eq!(name, "Максимум");
        assert!(["coordinator", "plan", "reviewer", "worker", "light"].iter().all(|k| r[*k].agent == "claude/opus"));
        assert_eq!(r["worker"].effort, "max");
        let (name, r) = template("claude", &machine()).unwrap();
        assert_eq!(name, "Только Claude");
        assert_eq!(r["worker"].agent, "claude/sonnet");
        assert!(r.values().all(|s| s.agent.starts_with("claude/") && (s.fallback.is_empty() || s.fallback.starts_with("claude/") || s.fallback == "none")));
        let (name, r) = template("economy", &machine()).unwrap();
        assert_eq!(name, "Экономный");
        assert_eq!(r["worker"].agent, "omp/deepseek/deepseek-v4-flash");
        assert_eq!(r["worker"].fallback, "claude/sonnet", "when the cheap one runs out, Claude takes over");
        let (name, r) = template("noclaude", &machine()).unwrap();
        assert_eq!(name, "Без Claude");
        for s in r.values() {
            assert!(!s.agent.starts_with("claude") && !s.fallback.contains("claude"), "{s:?}");
        }
        assert_eq!(r["reviewer"].agent, "codex/gpt-6-luna");
        assert!(template("weird", &machine()).is_err());
    }

    #[test]
    fn a_template_says_what_is_missing() {
        let mut m = machine();
        m[0].usable = false;
        assert!(template("max", &m).unwrap_err().contains("Claude"));
        assert!(template("claude", &m).is_err());
        // economy still works without Claude, on the cheap one alone
        let (_, r) = template("economy", &m).unwrap();
        assert_eq!(r["worker"].agent, "omp/deepseek/deepseek-v4-flash");
        let only_claude = vec![cat("claude", &["claude/opus"], &[])];
        assert!(template("economy", &only_claude).is_err());
        assert!(template("noclaude", &only_claude).is_err());
    }

    #[test]
    fn every_role_of_the_crew_is_a_known_setting_name() {
        for r in ROLES {
            for n in [r.agent, r.effort, r.fallback] {
                assert!(n.is_empty() || crate::station::valid_name(n), "{n}");
            }
        }
    }
}
