//! The Station of the Galaxy («Станция»): its crew (the program that runs the Coordinator, the planner, the reviewer,
//! the workers and the spares), the settings of this machine's board, and the «light tasks» preset the planner keeps
//! for it. The board is read and written through `app::run_board`. The ship's own settings live in `ship.rs`.

use crate::app::{self, run_board, Shared};
use crate::board::{Status, Task};
use crate::config::{now_ms, AppConfig};
use crate::crew;
use crate::failover;
use crate::models;
use crate::server::{clip, error, same_origin};
use crate::taskmeta;
use crate::usage;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The programs the crew may name, in the order the page offers them.
pub const PROGRAMS: &[&str] = &[
    "claude",
    "codex",
    "omp",
    "pi",
    "opencode",
    "agy",
    "copilot",
    "cursor-agent",
    "droid",
    "grok",
    "devin",
    "gemini",
    "kimi",
    "qwen",
    "kilo",
    "hermes",
    "letta",
    "mastracode",
    "qodercli",
];
pub(crate) const BOARD_WAIT: Duration = Duration::from_secs(30);
const DOCTOR_WAIT: Duration = Duration::from_secs(60);
const MAX_VALUE: usize = 4000;
const SHOWN_NAMES: usize = 12;

/// One setting as `herdr-projects show --json` gives it. The value is always text.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Setting {
    pub name: String,
    pub kind: String,
    pub value: String,
    pub from: String,
    pub place: String,
    pub own: bool,
    pub note: String,
}

/// One program of the crew: whether its command is on the PATH, and what `doctor` says about it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Program {
    pub name: String,
    pub installed: bool,
    pub version: String,
    /// `None` when `doctor` says nothing about signing in (API keys, or no line at all).
    pub signed_in: Option<bool>,
    pub line: String,
}

/// A failed call: the status and the words for the page.
#[derive(Debug)]
pub struct Fail {
    status: StatusCode,
    message: String,
}

impl Fail {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            message: message.into(),
        }
    }

    /// A board that answered with an error is a refusal (422), not a board that did not answer (502).
    pub fn refused(mut self) -> Self {
        if self.status == StatusCode::BAD_GATEWAY {
            self.status = StatusCode::UNPROCESSABLE_ENTITY;
        }
        self
    }

    pub fn message(&self) -> String {
        self.message.clone()
    }

    pub fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

/// What a board command printed and how it ended. A failure has its words in both fields.
#[derive(Debug, Default)]
pub(crate) struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    pub(crate) fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// The board's own words for a failure (its error text, else what it printed).
    pub(crate) fn words(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        let text = if text.is_empty() {
            "доска не сказала, что случилось"
        } else {
            text
        };
        clip(text.to_string())
    }
}

/* ───────── pure parsers and validators (unit-tested) ───────── */

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn value_text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(t)) => t.clone(),
        Some(other) => other.to_string(),
    }
}

fn setting_of(s: &Value) -> Option<Setting> {
    let name = text_of(s, "name");
    if name.is_empty() {
        return None;
    }
    Some(Setting {
        name,
        kind: text_of(s, "kind"),
        value: value_text(s.get("value")),
        from: text_of(s, "from"),
        place: text_of(s, "place"),
        own: s.get("own").and_then(Value::as_bool).unwrap_or(false),
        note: text_of(s, "note"),
    })
}

/// The JSON object of a board answer, after whatever the shell printed before it. `show --json` is
/// pretty-printed over several lines, so the first line that starts an object and parses as one wins.
pub fn object_in(stdout: &str) -> Option<Value> {
    let mut at = 0;
    for line in stdout.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('{') {
            let rest = &stdout[at + (line.len() - trimmed.len())..];
            if let Some(Ok(v)) = serde_json::Deserializer::from_str(rest)
                .into_iter::<Value>()
                .next()
            {
                if v.is_object() {
                    return Some(v);
                }
            }
        }
        at += line.len();
    }
    None
}

/// The settings of `herdr-projects show [--repo R] --json`, in the board's order.
pub fn parse_settings(stdout: &str) -> Result<Vec<Setting>, String> {
    let v =
        object_in(stdout).ok_or_else(|| "herdr-projects show --json: нет ответа".to_string())?;
    let list = v
        .get("settings")
        .and_then(Value::as_array)
        .ok_or_else(|| "herdr-projects show --json: нет списка настроек".to_string())?;
    Ok(list.iter().filter_map(setting_of).collect())
}

pub(crate) fn find<'a>(settings: &'a [Setting], name: &str) -> Option<&'a Setting> {
    settings.iter().find(|s| s.name == name)
}

/// The first word of a `doctor` line once the mark (✓, ✗, !, →) is gone.
fn first_word(line: &str) -> Option<&str> {
    line.trim_start_matches(|c: char| !c.is_alphanumeric())
        .split_whitespace()
        .next()
}

/// The line `herdr-projects doctor` prints about one program, e.g. `✓ claude 2.1.303, signed in (...)`.
pub fn doctor_line(doctor: &str, program: &str) -> Option<String> {
    doctor
        .lines()
        .map(str::trim)
        .find(|l| first_word(l) == Some(program))
        .map(str::to_string)
}

/// The version after the program's name in a doctor line (`2.1.303`), or nothing.
pub fn version_of(line: &str) -> String {
    let mut words = line
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .split_whitespace();
    words.next();
    words
        .next()
        .map(|w| w.trim_end_matches(','))
        .filter(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or("")
        .to_string()
}

/// Whether a doctor line says the program is signed in: `None` when it does not say.
pub fn signed_in_of(line: &str) -> Option<bool> {
    let l = line.to_lowercase();
    if l.contains("not signed in") || l.contains("not logged in") {
        Some(false)
    } else if l.contains("signed in") || l.contains("logged in") {
        Some(true)
    } else {
        None
    }
}

/// The planner's own version as the board's doctor reports it (`✓ herdr-projects 1.2.4, …`).
pub fn doctor_version(doctor: &str) -> String {
    doctor_line(doctor, "herdr-projects")
        .map(|l| version_of(&l))
        .unwrap_or_default()
}

/// The crew's programs: installed (from `available`) and what doctor says.
pub fn programs_of(doctor: &str, available: &[String]) -> Vec<Program> {
    PROGRAMS
        .iter()
        .map(|name| {
            let line = doctor_line(doctor, name).unwrap_or_default();
            Program {
                name: name.to_string(),
                installed: available.iter().any(|a| a == name),
                version: version_of(&line),
                signed_in: if line.is_empty() {
                    None
                } else {
                    signed_in_of(&line)
                },
                line,
            }
        })
        .collect()
}

/// Whether `program` is an executable file in one of the directories of `path_var` (a PATH).
pub fn on_path(path_var: &str, program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path_var.split(':').filter(|d| !d.is_empty()).any(|dir| {
        std::fs::metadata(Path::new(dir).join(program))
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

/// A setting's name: a letter, then letters, digits and dots (`worker.agent`, `review.mustFix`).
pub fn valid_name(name: &str) -> bool {
    name.len() <= 80
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// A value the board is given: not empty, at most 4000 characters, no NUL, and not a flag-looking word.
pub fn valid_value(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("пустое значение: чтобы убрать настройку, выберите «сбросить»".into());
    }
    if value.chars().count() > MAX_VALUE {
        return Err("значение длиннее 4000 знаков".into());
    }
    if value.contains('\0') {
        return Err("в значении не может быть нулевого байта".into());
    }
    if value.starts_with('-') {
        return Err("значение не может начинаться с «-»".into());
    }
    Ok(())
}

/// A `*.agent` value as the board takes it: `program/model`, or a bare `program` (the program's own default model).
/// Returns the program, or says what shape is wrong.
pub fn agent_program(value: &str) -> Result<String, String> {
    let (program, model) = match value.split_once('/') {
        Some((p, m)) => (p, Some(m)),
        None => (value, None),
    };
    if models::valid_program(program) && model.map_or(true, |m| !m.is_empty() && !m.starts_with('-')) {
        Ok(program.to_string())
    } else {
        Err(format!(
            "«{value}»: нужно «программа/модель», например claude/claude-opus-5-5, или одна программа"
        ))
    }
}

/// A `*.agent` value against the models its program lists. An empty list (not read yet, or a program with no
/// list) checks nothing: the board decides on apply.
pub fn check_agent(value: &str, names: &[String]) -> Result<(), String> {
    if names.is_empty() || !value.contains('/') || names.iter().any(|n| n == value) {
        return Ok(());
    }
    let shown: Vec<&str> = names.iter().take(SHOWN_NAMES).map(String::as_str).collect();
    let more = if names.len() > SHOWN_NAMES {
        " и другие"
    } else {
        ""
    };
    Err(format!(
        "«{value}» — такой модели нет. Доска принимает: {}{more}",
        shown.join(", ")
    ))
}

/// A `*.effort` value against the levels its program takes; an empty list checks nothing.
pub fn check_effort(value: &str, levels: &[String], program: &str) -> Result<(), String> {
    if levels.is_empty() || value == "default" || levels.iter().any(|l| l == value) {
        return Ok(());
    }
    Err(format!(
        "«{value}» — у {program} нет такого усилия. Принимает: {}",
        levels.join(", ")
    ))
}

/// The light-task preset is kept in the planner's crew file under the empty name, as it always was.
const LIGHT_ENTRY: &str = "";

/// The light-task preset in the planner's crew file.
pub fn light_of(doc: &Map<String, Value>) -> String {
    doc.get(LIGHT_ENTRY)
        .and_then(|e| e.get("light"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Sets (or, for `""`, clears) the light-task preset. Entries of other names (the machines an older planner
/// kept presets for) are dropped.
pub fn set_light_in(doc: &mut Map<String, Value>, agent: &str) {
    doc.retain(|name, _| name == LIGHT_ENTRY);
    if agent.is_empty() {
        doc.remove(LIGHT_ENTRY);
    } else {
        doc.insert(LIGHT_ENTRY.to_string(), json!({ "light": agent }));
    }
}

/* ───────── the crew file ───────── */

static CREW_LOCK: Mutex<()> = Mutex::new(());

fn crew_file(cfg: &AppConfig) -> PathBuf {
    cfg.state_dir.join("crew.json")
}

fn read_crew(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

pub(crate) fn read_light(cfg: &AppConfig) -> String {
    let _guard = CREW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    light_of(&read_crew(&crew_file(cfg)))
}

fn write_light(cfg: &AppConfig, agent: &str) -> Result<(), String> {
    let _guard = CREW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = crew_file(cfg);
    let mut doc = read_crew(&path);
    set_light_in(&mut doc, agent);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, text).map_err(|e| format!("не записать {}: {e}", path.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/* ───────── running the board ───────── */

/// `herdr-projects ARGS` on this machine. A board that answers with an error comes back as `Out` with that error's code.
pub(crate) async fn board_run(cfg: &AppConfig, args: &[String], wait: Duration) -> Out {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match run_board(cfg, &refs, wait).await {
        Ok(stdout) => Out {
            code: Some(0),
            stdout,
            stderr: String::new(),
        },
        Err(e) => Out {
            code: Some(1),
            stdout: e.clone(),
            stderr: e,
        },
    }
}

fn read_failed(out: &Out) -> Fail {
    Fail::new(502, out.words())
}

/// The board's settings at a level: the Galaxy's (`repo` empty) or one repository's (with the inherited ones).
pub(crate) async fn read_settings(cfg: &AppConfig, repo: &str) -> Result<Vec<Setting>, Fail> {
    let mut args = vec!["show".to_string()];
    if !repo.is_empty() {
        args.push("--repo".into());
        args.push(repo.into());
    }
    args.push("--json".into());
    let out = board_run(cfg, &args, BOARD_WAIT).await;
    if !out.ok() {
        return Err(read_failed(&out));
    }
    parse_settings(&out.stdout).map_err(|e| Fail::new(502, e))
}

/// `herdr-projects doctor`'s text. The board exits non-zero when a check fails, and its lines still matter.
async fn read_doctor(cfg: &AppConfig) -> Result<String, Fail> {
    let out = board_run(cfg, &["doctor".to_string()], DOCTOR_WAIT).await;
    if out.stdout.trim().is_empty() {
        return Err(read_failed(&out));
    }
    Ok(out.stdout)
}

/// The crew's programs that are installed here.
pub(crate) fn available() -> Vec<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    PROGRAMS
        .iter()
        .filter(|p| on_path(&path, p))
        .map(|p| p.to_string())
        .collect()
}

/// A program's models and effort levels, as its `models` answer gives them.
struct Listed {
    names: Vec<String>,
    effort: Vec<String>,
    note: String,
    loading: bool,
}

async fn models_of(app: &Shared, program: &str) -> Listed {
    let m = models::get(&app.cfg, program).await;
    Listed {
        names: m.names,
        effort: m.effort,
        note: m.note,
        loading: m.loading,
    }
}

/// The repositories of the board, by name.
fn repos_of(app: &Shared) -> Vec<String> {
    taskmeta::repos(&app.cfg.data_dir)
        .into_iter()
        .map(|r| r.name)
        .collect()
}

/// The live tasks (of the repository, when one is named): not done, not archived.
async fn affects_of(app: &Shared, repo: &str) -> Vec<Value> {
    let tasks: Vec<Task> = app.snapshot.read().await.tasks.clone();
    tasks
        .iter()
        .filter(|t| !t.archived && t.status != Status::Done)
        .filter(|t| repo.is_empty() || t.repo == repo)
        .map(|t| json!({"id": t.id, "title": t.title, "status": t.status.key()}))
        .collect()
}

/* ───────── the endpoints ───────── */

#[derive(Deserialize)]
pub struct StationQuery {
    repo: Option<String>,
}

/// What the page knows about one program of the crew: its install, the sign-in, the state, the models, the providers
/// inside, and who uses it.
struct Gathered {
    program: Program,
    state: crew::State,
    names: Vec<String>,
    effort: Vec<String>,
    loading: bool,
    /// The names offered when the program lists none: the models it was used with.
    suggested: Vec<String>,
    providers: Vec<crew::Provider>,
}

/// Every program of the crew with its state. Reads Omarchy's records and the cached models answers; asks the board
/// nothing that waits (a program's models are read in the background the first time).
async fn gather(app: &Shared, doctor: &str, installed: &[String]) -> Vec<Gathered> {
    let tasks: Vec<Task> = app.snapshot.read().await.tasks.clone();
    // The row's own line is gone at the next report of the agent; the hand-overs the board kept are the lasting record.
    let mut hits = crew::hits_of(&tasks);
    let (kept_tasks, data_dir) = (tasks.clone(), app.cfg.data_dir.clone());
    hits.extend(
        tokio::task::spawn_blocking(move || crew::handover_hits_of(&kept_tasks, &data_dir, now_ms()))
            .await
            .unwrap_or_default(),
    );
    let mut out = Vec::new();
    for program in programs_of(doctor, installed) {
        let usage = if program.installed {
            usage::agent_by_id(&program.name)
        } else {
            usage::AgentUsage::default()
        };
        let listed = if program.installed {
            models::get(&app.cfg, &program.name).await
        } else {
            models::Models::default()
        };
        let state = crew::judge(&crew::Facts {
            program: &program.name,
            installed: program.installed,
            doctor_signed_in: program.signed_in,
            doctor_line: &program.line,
            models_account: &listed.note,
            usage: &usage,
            hits: &hits,
            now: now_ms(),
        });
        let seen = if program.installed { usage::seen_models(&program.name) } else { Vec::new() };
        let providers = crew::providers_of(&program.name, &listed.names, &seen);
        let suggested = if listed.names.is_empty() { crew::seen_names(&program.name, &seen) } else { Vec::new() };
        out.push(Gathered {
            program,
            state,
            names: listed.names,
            effort: listed.effort,
            loading: listed.loading,
            suggested,
            providers,
        });
    }
    out
}

/// The programs that cannot be given work now (out of allowance, signed out, not installed), for the spares.
fn down_of(gathered: &[Gathered]) -> failover::Down {
    let now = now_ms();
    gathered
        .iter()
        .filter_map(|g| {
            let a = &g.state.avail;
            let kind = match a.state {
                "limited" | "signed_out" | "missing" => a.state,
                _ => return None,
            };
            let until = if kind != "limited" {
                String::new()
            } else if !a.until.is_empty() {
                a.until.clone()
            } else if a.until_ms > now {
                failover::clock(a.until_ms)
            } else {
                String::new()
            };
            Some((g.program.name.clone(), failover::Gone { kind: kind.to_string(), until }))
        })
        .collect()
}

/// The programs that are down now, read as the page reads them (the doctor, Omarchy's records, the board's lines).
pub(crate) async fn down_programs(app: &Shared, installed: &[String]) -> Result<failover::Down, Fail> {
    let doctor = read_doctor(&app.cfg).await?;
    Ok(down_of(&gather(app, &doctor, installed).await))
}

/// A chain of spares against the models the programs list (the shape alone when a list cannot be read).
pub(crate) async fn check_chain(app: &Shared, chain: &str) -> Result<(), String> {
    match read_settings(&app.cfg, "").await {
        Ok(settings) => check(app, &settings, "spare.fallback", chain).await,
        Err(_) => crew::check_fallback(chain, &|_: &str| Vec::new()),
    }
}

/// The programs for the page: the doctor's reading plus everything `gather` found.
fn programs_json(gathered: &[Gathered], settings: &[Setting], light: &str, tasks: &[Task]) -> Vec<Value> {
    gathered
        .iter()
        .map(|g| {
            let mut v = serde_json::to_value(&g.program).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("state".into(), json!(g.state));
                o.insert("providers".into(), json!(g.providers));
                o.insert("roles".into(), json!(crew::roles_using(&g.program.name, settings, light)));
                o.insert("tasks".into(), json!(crew::tasks_using(&g.program.name, tasks)));
                o.insert(
                    "models".into(),
                    json!({
                        "count": g.names.len(),
                        "loading": g.loading,
                        "listed": !g.names.is_empty(),
                        "suggested": g.suggested.len(),
                    }),
                );
            }
            v
        })
        .collect()
}

/// The presets with how far each is from the crew now, and which is active.
fn presets_json(cfg: &AppConfig, settings: &[Setting], light: &str, repo: &str) -> Value {
    let store = crew::read_store(cfg);
    let diffs: Vec<(String, usize)> = store
        .presets
        .iter()
        .map(|p| (p.id.clone(), crew::plan(p, settings, light, repo).len()))
        .collect();
    let list: Vec<Value> = store
        .presets
        .iter()
        .zip(&diffs)
        .map(|(p, (_, diff))| json!({"id": p.id, "name": p.name, "roles": p.roles, "diff": diff}))
        .collect();
    json!({"list": list, "active": crew::active_of(&store, &diffs, repo)})
}

async fn station_read(app: &Shared, q: StationQuery, installed: &[String]) -> Result<Value, Fail> {
    let repo = q.repo.unwrap_or_default();
    if !repo.is_empty() && !taskmeta::valid_repo(&repo) {
        return Err(Fail::new(400, "bad universe name"));
    }
    let (settings, doctor) = tokio::join!(read_settings(&app.cfg, &repo), read_doctor(&app.cfg));
    let settings = settings?;
    let doctor = doctor?;
    let light = read_light(&app.cfg);
    let gathered = gather(app, &doctor, installed).await;
    let tasks: Vec<Task> = app.snapshot.read().await.tasks.clone();
    Ok(json!({
        "ok": true,
        "level": if repo.is_empty() { "you" } else { "repo" },
        "repo": repo,
        "repos": repos_of(app),
        "version": doctor_version(&doctor),
        "settings": settings,
        "programs": programs_json(&gathered, &settings, &light, &tasks),
        "light": light,
        "failover": failover::page_json(&app.cfg, &light, &down_of(&gathered)),
        "presets": presets_json(&app.cfg, &settings, &light, &repo),
    }))
}

/// `GET /api/station?repo=R`: the crew, the board's settings and the light preset.
pub async fn station_get(State(app): State<Shared>, Query(q): Query<StationQuery>) -> Response {
    match station_read(&app, q, &available()).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[derive(Deserialize)]
pub struct ModelsQuery {
    program: String,
}

async fn models_read(app: &Shared, q: ModelsQuery) -> Result<Value, Fail> {
    if !models::valid_program(&q.program) {
        return Err(Fail::new(400, "такой программы нет"));
    }
    let listed = models_of(app, &q.program).await;
    let seen = usage::seen_models(&q.program);
    Ok(json!({
        "ok": true,
        "program": q.program,
        "names": listed.names,
        "effort": listed.effort,
        "note": listed.note,
        "loading": listed.loading,
        "listed": !listed.names.is_empty(),
        "suggested": if listed.names.is_empty() { crew::seen_names(&q.program, &seen) } else { Vec::new() },
        "providers": crew::providers_of(&q.program, &listed.names, &seen),
    }))
}

/// `GET /api/station/models?program=P`: the model names and effort levels a program takes.
pub async fn station_models(State(app): State<Shared>, Query(q): Query<ModelsQuery>) -> Response {
    match models_read(&app, q).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[derive(Deserialize)]
pub struct ChangeBody {
    #[serde(default)]
    name: String,
    value: Option<String>,
    #[serde(default)]
    unset: bool,
    repo: Option<String>,
    confirm: Option<bool>,
}

/// One change to a setting, checked for its shape before anything is asked of the board.
struct Change {
    name: String,
    repo: String,
    /// `None` for an unset.
    value: Option<String>,
}

fn prepare(body: &ChangeBody) -> Result<Change, Fail> {
    if !valid_name(&body.name) {
        return Err(Fail::new(400, "недопустимое имя настройки"));
    }
    let repo = body.repo.clone().unwrap_or_default().trim().to_string();
    if !repo.is_empty() && !taskmeta::valid_repo(&repo) {
        return Err(Fail::new(400, "недопустимое имя вселенной"));
    }
    let value = if body.unset {
        None
    } else {
        let v = body
            .value
            .as_deref()
            .ok_or_else(|| Fail::new(400, "нет значения: дайте value или unset"))?;
        valid_value(v).map_err(|e| Fail::new(400, e))?;
        Some(v.to_string())
    };
    Ok(Change {
        name: body.name.clone(),
        repo,
        value,
    })
}

/// The board's own check of a value the planner can check itself: `*.agent` against the program's models, and
/// `*.effort` against the levels of the program the role's agent runs. A models list that cannot be read checks
/// nothing; the board decides then.
async fn check(
    app: &Shared,
    settings: &[Setting],
    name: &str,
    value: &str,
) -> Result<(), String> {
    if name.ends_with(".agent") {
        let program = agent_program(value)?;
        return check_agent(value, &models_of(app, &program).await.names);
    }
    if let Some(role) = name.strip_suffix(".effort") {
        let Some(agent) = find(settings, &format!("{role}.agent")).map(|s| s.value.clone()) else {
            return Ok(());
        };
        let program = crew::program_of(&agent).to_string();
        if program.is_empty() || program == "none" {
            return Ok(());
        }
        return check_effort(value, &models_of(app, &program).await.effort, &program);
    }
    if name.ends_with(".fallback") {
        let mut cache: Map<String, Value> = Map::new();
        let mut programs: Vec<String> = Vec::new();
        for item in value.split(',') {
            if let Ok(p) = agent_program(item.trim()) {
                if !programs.contains(&p) {
                    programs.push(p);
                }
            }
        }
        for p in programs {
            cache.insert(p.clone(), json!(models_of(app, &p).await.names));
        }
        return crew::check_fallback(value, &|p: &str| {
            cache
                .get(p)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default()
        });
    }
    Ok(())
}

/// `POST /api/station/preview`: what a change would do, without changing anything.
pub async fn station_preview(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ChangeBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        let change = prepare(&body)?;
        let settings = read_settings(&app.cfg, &change.repo)
            .await
            .map_err(Fail::refused)?;
        let problem = match &change.value {
            Some(v) => check(&app, &settings, &change.name, v)
                .await
                .err()
                .unwrap_or_default(),
            None => String::new(),
        };
        let entry = find(&settings, &change.name);
        Ok::<Value, Fail>(json!({
            "ok": true,
            "name": change.name,
            "repo": change.repo,
            "was": entry.map(|s| s.value.clone()).unwrap_or_default(),
            "was_from": entry.map(|s| s.from.clone()).unwrap_or_default(),
            "now": change.value,
            "valid": problem.is_empty(),
            "error": problem,
            "affects": affects_of(&app, &change.repo).await,
            "note": entry.map(|s| s.note.clone()).unwrap_or_default(),
        }))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

/// `POST /api/station/set`: `herdr-projects set|unset` on this board, after the same checks as the preview.
pub async fn station_set(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ChangeBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        if body.confirm != Some(true) {
            return Err(Fail::new(400, "нужно подтверждение: было → стало"));
        }
        let change = prepare(&body)?;
        let settings = read_settings(&app.cfg, &change.repo)
            .await
            .map_err(Fail::refused)?;
        if let Some(v) = &change.value {
            if let Err(problem) = check(&app, &settings, &change.name, v).await {
                return Err(Fail::new(422, problem));
            }
        }
        let was = find(&settings, &change.name)
            .map(|s| s.value.clone())
            .unwrap_or_default();
        let mut args: Vec<String> = match &change.value {
            Some(_) => vec!["set".into(), change.name.clone()],
            None => vec!["unset".into(), change.name.clone()],
        };
        if let Some(v) = &change.value {
            args.push(v.clone());
        }
        if !change.repo.is_empty() {
            args.push("--repo".into());
            args.push(change.repo.clone());
        }
        let out = board_run(&app.cfg, &args, BOARD_WAIT).await;
        if !out.ok() {
            return Err(Fail::new(422, out.words()));
        }
        app::refresh(&app).await;
        Ok::<Value, Fail>(json!({
            "ok": true,
            "name": change.name,
            "repo": change.repo,
            "was": was,
            "now": change.value,
            "out": clip(out.stdout.trim().to_string()),
        }))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[derive(Deserialize)]
pub struct LightBody {
    /// `program/model`; empty clears the preset.
    #[serde(default)]
    agent: String,
    confirm: Option<bool>,
}

/// `POST /api/station/light`: the «Лёгкие задачи» preset, kept in the planner's own crew file.
pub async fn station_light(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<LightBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return error(StatusCode::BAD_REQUEST, "нужно подтверждение: было → стало");
    }
    if !body.agent.is_empty() {
        if let Err(e) = agent_program(&body.agent) {
            return error(StatusCode::BAD_REQUEST, e);
        }
    }
    match write_light(&app.cfg, &body.agent) {
        Ok(()) => Json(json!({"ok": true, "light": body.agent})).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/* ───────── the crew presets ───────── */

/// One apply at a time: two sets of `set` commands would interleave.
pub(crate) static APPLY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn level_of(repo: Option<String>) -> Result<String, Fail> {
    let repo = repo.unwrap_or_default().trim().to_string();
    if !repo.is_empty() && !taskmeta::valid_repo(&repo) {
        return Err(Fail::new(400, "недопустимое имя вселенной"));
    }
    Ok(repo)
}

#[derive(Deserialize)]
pub struct PresetBody {
    /// `create`, `capture`, `template`, `update`, `rename` or `delete`.
    op: String,
    id: Option<String>,
    name: Option<String>,
    roles: Option<std::collections::BTreeMap<String, crew::RoleSet>>,
    /// `max`, `claude`, `economy` or `noclaude` for `template`.
    kind: Option<String>,
    repo: Option<String>,
}

fn store_fail(e: String) -> Fail {
    Fail::new(422, e)
}

async fn presets_change(app: &Shared, body: PresetBody) -> Result<Value, Fail> {
    let name = body.name.clone().unwrap_or_default();
    let id_of = || body.id.clone().ok_or_else(|| Fail::new(400, "нет набора: дайте id"));
    let id = match body.op.as_str() {
        "create" => {
            let roles = body.roles.clone().unwrap_or_default();
            crew::edit_store(&app.cfg, |s| crew::add_preset(s, &name, &roles)).map_err(store_fail)?
        }
        "capture" => {
            let repo = level_of(body.repo.clone())?;
            let settings = read_settings(&app.cfg, &repo).await.map_err(Fail::refused)?;
            let roles = crew::capture(&settings, &read_light(&app.cfg));
            // A preset made from the crew as it is, is the active one: later edits of the crew show as a difference from it.
            crew::edit_store(&app.cfg, |s| {
                let id = crew::add_preset(s, &name, &roles)?;
                s.active.insert(repo.clone(), id.clone());
                Ok(id)
            })
            .map_err(store_fail)?
        }
        "template" => {
            let kind = body.kind.clone().unwrap_or_default();
            let doctor = read_doctor(&app.cfg).await?;
            let gathered = gather(app, &doctor, &available()).await;
            let cats: Vec<crew::Cat> = gathered
                .iter()
                .map(|g| crew::Cat {
                    program: g.program.name.clone(),
                    usable: g.program.installed && matches!(g.state.avail.state, "ok" | "unknown"),
                    names: if g.names.is_empty() { g.suggested.clone() } else { g.names.clone() },
                    effort: g.effort.clone(),
                })
                .collect();
            let (default_name, roles) = crew::template(&kind, &cats).map_err(|e| {
                let wait = if gathered.iter().any(|g| g.loading) { " Модели ещё читаются — повторите через минуту." } else { "" };
                Fail::new(422, format!("{e}.{wait}"))
            })?;
            let name = if name.trim().is_empty() { default_name } else { name };
            crew::edit_store(&app.cfg, |s| crew::add_preset(s, &name, &roles)).map_err(store_fail)?
        }
        "update" => {
            let id = id_of()?;
            let roles = body.roles.clone().unwrap_or_default();
            crew::edit_store(&app.cfg, |s| crew::update_preset(s, &id, &roles)).map_err(store_fail)?;
            id
        }
        "rename" => {
            let id = id_of()?;
            crew::edit_store(&app.cfg, |s| crew::rename_preset(s, &id, &name)).map_err(store_fail)?;
            id
        }
        "delete" => {
            let id = id_of()?;
            crew::edit_store(&app.cfg, |s| crew::delete_preset(s, &id)).map_err(store_fail)?;
            id
        }
        _ => return Err(Fail::new(400, "неизвестное действие с набором")),
    };
    Ok(json!({"ok": true, "id": id}))
}

/// `POST /api/station/presets`: create, capture, rename, update and delete the planner's crew presets. The board is
/// not asked: a preset only holds values; applying it is `station_preset_apply`.
pub async fn station_presets(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<PresetBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    match presets_change(&app, body).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[derive(Deserialize)]
pub struct ApplyBody {
    id: String,
    repo: Option<String>,
    confirm: Option<bool>,
}

/// The preset, and the steps that bring the board to it, each checked against the models and efforts the programs list.
/// Programs whose models are still being read are named, since their steps are not checked yet.
async fn prepared(app: &Shared, id: &str, repo: &str) -> Result<(crew::Preset, Vec<crew::Step>, Vec<String>), Fail> {
    let preset = crew::read_store(&app.cfg)
        .presets
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| Fail::new(404, "такого набора нет"))?;
    let settings = read_settings(&app.cfg, repo).await.map_err(Fail::refused)?;
    let light = read_light(&app.cfg);
    let mut steps = crew::plan(&preset, &settings, &light, repo);
    let mut programs: Vec<String> = Vec::new();
    for set in preset.roles.values() {
        for p in std::iter::once(crew::program_of(&set.agent).to_string()).chain(crew::chain_programs(&set.fallback)) {
            if models::valid_program(&p) && !programs.contains(&p) {
                programs.push(p);
            }
        }
    }
    for st in &steps {
        if st.field == "effort" && st.now.is_some() {
            let current = find(&settings, &st.setting.replace("effort", "agent")).map(|s| crew::program_of(&s.value).to_string());
            if let Some(p) = current.filter(|p| models::valid_program(p) && !programs.contains(p)) {
                programs.push(p);
            }
        }
    }
    let mut lists: std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> = std::collections::BTreeMap::new();
    let mut loading: Vec<String> = Vec::new();
    for p in &programs {
        let m = models_of(app, p).await;
        if m.loading {
            loading.push(p.clone());
        }
        lists.insert(p.clone(), (m.names, m.effort));
    }
    crew::check_steps(&mut steps, &preset, &settings, &|p: &str| lists.get(p).cloned().unwrap_or_default());
    Ok((preset, steps, loading))
}

fn command_text(step: &crew::Step, repo: &str) -> String {
    match crew::command_of(step, repo) {
        Some(args) => format!("herdr-projects {}", args.join(" ")),
        None => String::new(),
    }
}

/// `POST /api/station/preset/preview`: the full «было → стало» of a preset over every role, and the tasks it touches.
pub async fn station_preset_preview(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ApplyBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        let repo = level_of(body.repo.clone())?;
        let (preset, steps, loading) = prepared(&app, &body.id, &repo).await?;
        let tasks: Vec<Task> = app.snapshot.read().await.tasks.clone();
        let light_skipped = !repo.is_empty() && preset.roles.get("light").is_some_and(|r| !r.agent.is_empty());
        let rows: Vec<Value> = steps
            .iter()
            .map(|s| {
                let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
                if let Some(o) = v.as_object_mut() {
                    o.insert("command".into(), json!(command_text(s, &repo)));
                }
                v
            })
            .collect();
        Ok::<Value, Fail>(json!({
            "ok": true,
            "preset": {"id": preset.id, "name": preset.name},
            "repo": repo,
            "steps": rows,
            "valid": steps.iter().all(|s| s.error.is_empty()),
            "loading": loading,
            "light_skipped": light_skipped,
            "affects": crew::affected(&tasks, &repo),
        }))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

/// `POST /api/station/preset/apply`: the steps of the preview, one board command after the other in the preview's
/// order, after the same checks. A refusal stops the run and says what was done and where it stopped.
pub async fn station_preset_apply(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ApplyBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        if body.confirm != Some(true) {
            return Err(Fail::new(400, "нужно подтверждение: было → стало"));
        }
        let repo = level_of(body.repo.clone())?;
        let _one_at_a_time = APPLY.lock().await;
        let (preset, steps, _) = prepared(&app, &body.id, &repo).await?;
        if let Some(bad) = steps.iter().find(|s| !s.error.is_empty()) {
            return Err(Fail::new(422, format!("{}: {}", bad.setting, bad.error)));
        }
        let mut ran: Vec<String> = Vec::new();
        for step in &steps {
            let done = if step.light() {
                write_light(&app.cfg, step.now.as_deref().unwrap_or(""))
            } else {
                let args = crew::command_of(step, &repo).unwrap_or_default();
                let out = board_run(&app.cfg, &args, BOARD_WAIT).await;
                if out.ok() { Ok(()) } else { Err(out.words()) }
            };
            if let Err(why) = done {
                app::refresh(&app).await;
                let before = if ran.is_empty() { "Ничего не изменилось.".to_string() } else { format!("Уже выполнено: {}.", ran.join(", ")) };
                return Err(Fail::new(422, format!("{} остановилась: {why}. {before} Остальное не тронуто.", step.setting)));
            }
            ran.push(step.setting.clone());
        }
        crew::edit_store(&app.cfg, |s| {
            s.active.insert(repo.clone(), preset.id.clone());
            Ok(())
        })
        .map_err(|e| Fail::new(500, e))?;
        app::refresh(&app).await;
        Ok::<Value, Fail>(json!({"ok": true, "preset": preset.name, "repo": repo, "ran": ran}))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `herdr-projects show --json` as the board printed it (trimmed to three settings, one of them inherited).
    const SHOW: &str = r#"{
  "settings": [
    {"name": "worker.agent", "kind": "work", "value": "claude/claude-sonnet-5-5", "from": "you", "place": "you", "own": true, "note": ""},
    {"name": "worker.fast", "kind": "work", "value": false, "from": "built in", "place": "builtin", "own": false, "note": ""},
    {"name": "reviewer.fast", "kind": "work", "value": true, "from": "built in", "place": "builtin", "own": false, "note": "does nothing: claude has no fast mode"}
  ],
  "problems": []
}"#;

    const DOCTOR: &str = "✓ herdr-projects 1.2.4, downloaded: ~/.config/herdr/plugins/x\n\
✓ claude 2.1.303, signed in (worker.agent, reviewer.agent)\n\
✓ codex 0.9.2, not signed in\n\
✓ omp 1.5.0, API keys: no sign-in needed\n\
✗ pi not found\n";

    #[test]
    fn settings_keep_the_boards_order_and_values_as_text() {
        let s = parse_settings(SHOW).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].name, "worker.agent");
        assert_eq!(s[0].value, "claude/claude-sonnet-5-5");
        assert!(s[0].own);
        assert_eq!(s[1].value, "false");
        assert!(!s[1].own);
        assert_eq!(s[2].note, "does nothing: claude has no fast mode");
        assert_eq!(s[2].place, "builtin");
    }

    #[test]
    fn a_settings_answer_after_a_login_banner_still_parses() {
        let out = format!("Welcome to the shell\n{SHOW}\n");
        assert_eq!(parse_settings(&out).unwrap().len(), 3);
        let one_line = format!("Welcome {{x}}\n{}\n", SHOW.replace('\n', " "));
        assert_eq!(parse_settings(&one_line).unwrap().len(), 3);
        assert!(parse_settings("no json here").is_err());
        assert!(parse_settings("{\"ok\": true}").is_err());
    }

    #[test]
    fn doctor_lines_give_version_and_sign_in() {
        assert_eq!(doctor_version(DOCTOR), "1.2.4");
        let claude = doctor_line(DOCTOR, "claude").unwrap();
        assert_eq!(version_of(&claude), "2.1.303");
        assert_eq!(signed_in_of(&claude), Some(true));
        let codex = doctor_line(DOCTOR, "codex").unwrap();
        assert_eq!(version_of(&codex), "0.9.2");
        assert_eq!(signed_in_of(&codex), Some(false));
        let omp = doctor_line(DOCTOR, "omp").unwrap();
        assert_eq!(signed_in_of(&omp), None);
        assert!(doctor_line(DOCTOR, "pi").is_some());
        assert!(doctor_line(DOCTOR, "cla").is_none());
        assert!(doctor_line(DOCTOR, "gemini").is_none());
    }

    #[test]
    fn programs_mark_installed_and_what_doctor_knows() {
        let avail = vec!["claude".to_string(), "omp".to_string()];
        let list = programs_of(DOCTOR, &avail);
        assert_eq!(list.len(), PROGRAMS.len());
        let claude = list.iter().find(|p| p.name == "claude").unwrap();
        assert!(claude.installed);
        assert_eq!(claude.version, "2.1.303");
        assert_eq!(claude.signed_in, Some(true));
        let codex = list.iter().find(|p| p.name == "codex").unwrap();
        assert!(!codex.installed);
        assert_eq!(codex.signed_in, Some(false));
        let gemini = list.iter().find(|p| p.name == "gemini").unwrap();
        assert_eq!(gemini.signed_in, None);
        assert_eq!(gemini.line, "");
    }

    #[test]
    fn a_program_is_on_the_path_when_it_is_an_executable_file() {
        let dir = std::env::temp_dir().join(format!("hp-station-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("codex");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.join("omp");
        std::fs::write(&plain, "text").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path = format!("/nonexistent-x:{}", dir.display());
        assert!(on_path(&path, "codex"));
        assert!(
            !on_path(&path, "omp"),
            "a file that is not executable is not installed"
        );
        assert!(!on_path(&path, "pi"));
        assert!(!on_path("", "codex"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_and_values_are_checked_before_the_board_sees_them() {
        assert!(valid_name("worker.agent"));
        assert!(valid_name("review.mustFix"));
        assert!(!valid_name("Worker.agent"));
        assert!(!valid_name("worker agent"));
        assert!(!valid_name("../x"));
        assert!(!valid_name(""));
        assert!(valid_value("high").is_ok());
        assert!(valid_value("  ").is_err());
        assert!(valid_value("-x").is_err());
        assert!(valid_value("a\0b").is_err());
        assert!(valid_value(&"x".repeat(4001)).is_err());
        assert!(valid_value(&"x".repeat(4000)).is_ok());
    }

    #[test]
    fn an_agent_needs_a_program_and_a_listed_model() {
        assert_eq!(agent_program("claude/claude-opus-5-5").unwrap(), "claude");
        assert_eq!(
            agent_program("omp/deepseek/deepseek-v4-flash").unwrap(),
            "omp"
        );
        assert_eq!(agent_program("kimi").unwrap(), "kimi", "a program alone takes its own default model");
        assert!(agent_program("claude/").is_err());
        assert!(agent_program("claude/-x").is_err());
        assert!(agent_program("Claude/x").is_err());
        let names = vec![
            "claude/claude-opus-5-5".to_string(),
            "claude/fable".to_string(),
        ];
        assert!(check_agent("claude/fable", &names).is_ok());
        let err = check_agent("claude/nope", &names).unwrap_err();
        assert!(err.contains("claude/fable"), "{err}");
        assert!(
            check_agent("claude/nope", &[]).is_ok(),
            "no list checks nothing"
        );
        assert!(check_agent("claude", &names).is_ok(), "a program alone is not a model");
    }

    #[test]
    fn an_effort_must_be_one_the_program_takes() {
        let levels = vec!["low".to_string(), "high".to_string()];
        assert!(check_effort("high", &levels, "codex").is_ok());
        let err = check_effort("ultra", &levels, "codex").unwrap_err();
        assert!(err.contains("codex") && err.contains("low, high"), "{err}");
        assert!(check_effort("ultra", &[], "omp").is_ok());
        assert!(check_effort("default", &levels, "codex").is_ok(), "default is the program's own");
    }

    #[test]
    fn the_light_preset_is_kept_and_cleared_by_an_empty_agent() {
        let mut doc = Map::new();
        set_light_in(&mut doc, "omp/deepseek/deepseek-v4-flash");
        assert_eq!(light_of(&doc), "omp/deepseek/deepseek-v4-flash");
        set_light_in(&mut doc, "codex/gpt-5.5");
        assert_eq!(light_of(&doc), "codex/gpt-5.5");
        set_light_in(&mut doc, "");
        assert_eq!(light_of(&doc), "");
        assert!(doc.is_empty());
    }

    #[test]
    fn the_presets_of_other_machines_in_an_old_crew_file_are_forgotten() {
        let mut doc = read_crew_text(r#"{"": {"light": "codex/gpt-5.5"}, "vm-a": {"light": "omp/x/y"}}"#);
        assert_eq!(light_of(&doc), "codex/gpt-5.5");
        set_light_in(&mut doc, "codex/gpt-5.5");
        assert_eq!(doc.len(), 1, "{doc:?}");
    }

    fn read_crew_text(text: &str) -> Map<String, Value> {
        serde_json::from_str::<Value>(text).unwrap().as_object().cloned().unwrap()
    }

    #[test]
    fn the_effort_of_a_models_answer_is_read_for_the_page() {
        let (names, _) = models::parse("codex", "codex takes ...\n\ncodex/gpt-5.5\n");
        assert_eq!(names, vec!["codex/gpt-5.5"]);
        assert!(models::parse_effort("nothing here").is_empty());
    }

    /* ───────── the endpoints, against a stand-in board ───────── */

    use axum::body::to_bytes;
    use std::path::Path as FsPath;

    fn show_json(extra: &[(&str, &str, &str)]) -> String {
        let mut rows: Vec<String> = Vec::new();
        let mut all = vec![
            ("coordinator.agent", "claude/claude-opus-5-5", "you"),
            ("coordinator.effort", "medium", "you"),
            ("plan.agent", "claude/claude-opus-5-5", "you"),
            ("reviewer.agent", "claude/claude-opus-5-5", "you"),
            ("reviewer.effort", "high", "you"),
            ("reviewer.fallback", "none", "built in"),
            ("worker.agent", "claude/claude-sonnet-5-5", "you"),
            ("worker.effort", "high", "you"),
            ("worker.fallback", "none", "built in"),
        ];
        all.extend_from_slice(extra);
        for (n, v, from) in all {
            let place = if from == "built in" { "builtin" } else { "you" };
            rows.push(format!(
                r#"{{"name":"{n}","kind":"work","value":"{v}","from":"{from}","place":"{place}","own":{},"note":""}}"#,
                from != "built in"
            ));
        }
        format!(r#"{{"settings":[{}],"problems":[]}}"#, rows.join(","))
    }

    /// A board that logs every call, answers `show --json` and `models`, and refuses the setting named in `refuse`.
    fn stand_in_board(dir: &FsPath, refuse: &str) -> (String, PathBuf) {
        let log = dir.join("board.log");
        let show = dir.join("show.json");
        std::fs::write(&show, show_json(&[])).unwrap();
        let body = format!(
            r#"echo "$@" >> {log}
case "$1" in
  show) cat {show};;
  doctor) echo "✓ claude 2.1.303, signed in (worker.agent)"; echo "✗ codex 0.9.2, not signed in: codex login"; echo "✓ omp 1.5.0, API keys: no sign-in needed";;
  models)
    case "$2" in
      claude) printf 'claude takes a short name.\n\nclaude/claude-opus-5-5\nclaude/claude-sonnet-5-5\nclaude/claude-haiku-5-5\n\nEffort (worker.effort and the like): low, medium, high, xhigh, max (from `claude --help`).\n';;
      codex) printf 'codex takes the names codex lists for the account it is signed in with (here: your ChatGPT sign-in (go plan)). Write one:\n\ncodex/gpt-6-luna\n\nEffort (worker.effort and the like): low, high, ultra (from `codex debug models`).\n';;
      omp) printf 'omp: any name omp takes (not checked: omp has no list of them).\n\nEffort (worker.effort and the like): off, low, high (from `omp --help`).\n';;
      *) echo "models: unknown program $2" >&2; exit 2;;
    esac;;
  set|unset) if [ "$2" = "{refuse}" ]; then echo "$2: refused by the stand-in" >&2; exit 3; fi;;
  list|coordinator) echo "[]";;
  *) exit 1;;
esac"#,
            log = log.display(),
            show = show.display(),
            refuse = refuse
        );
        let script = dir.join("board.sh");
        crate::testutil::write_exec(&script, format!("#!/bin/sh\n{body}\n"));
        (script.display().to_string(), log)
    }

    async fn json_of(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn app_with_board(dir: &FsPath, refuse: &str) -> (Shared, PathBuf) {
        let (board, log) = stand_in_board(dir, refuse);
        let cfg = AppConfig { herdr_projects: board, ..crate::testutil::test_cfg(dir) };
        (app::App::new(cfg), log)
    }

    fn board_commands(log: &FsPath) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("set ") || l.starts_with("unset "))
            .map(String::from)
            .collect()
    }

    fn economy(app: &Shared) -> String {
        let roles = crew_roles(&[
            ("coordinator", "claude/claude-opus-5-5", "medium", ""),
            ("worker", "codex/gpt-6-luna", "ultra", "claude/claude-sonnet-5-5, omp"),
            ("reviewer", "claude/claude-opus-5-5", "high", "codex/gpt-6-luna"),
            ("light", "omp/deepseek/deepseek-v4-flash", "", ""),
        ]);
        crew::edit_store(&app.cfg, |s| crew::add_preset(s, "Экономный", &roles)).unwrap()
    }

    fn crew_roles(rows: &[(&str, &str, &str, &str)]) -> std::collections::BTreeMap<String, crew::RoleSet> {
        rows.iter()
            .map(|(r, a, e, f)| (r.to_string(), crew::RoleSet { agent: a.to_string(), effort: e.to_string(), fallback: f.to_string() }))
            .collect()
    }

    /// Waits until the programs' models answers are read. The answers are cached for the whole test run: a test that
    /// ends (and removes its stand-in board) while one is being read would leave a failed answer for the others.
    async fn wait_models(app: &Shared, programs: &[&str]) {
        for p in programs {
            for _ in 0..200 {
                if !models::get(&app.cfg, p).await.loading {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    fn headers() -> HeaderMap {
        HeaderMap::new()
    }

    #[tokio::test]
    async fn applying_a_preset_runs_the_boards_set_commands_in_the_order_of_the_preview() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = app_with_board(dir.path(), "");
        let id = economy(&app);
        // warm the models answers so the checks have lists
        wait_models(&app, &["claude", "codex", "omp"]).await;
        let body = || ApplyBody { id: id.clone(), repo: None, confirm: Some(true) };
        let (code, prev) = json_of(station_preset_preview(State(app.clone()), headers(), Json(ApplyBody { confirm: None, ..body() })).await).await;
        assert_eq!(code, StatusCode::OK, "{prev}");
        assert_eq!(prev["valid"], true, "{prev}");
        let steps: Vec<String> = prev["steps"].as_array().unwrap().iter().map(|s| format!("{} {} → {}", s["setting"].as_str().unwrap(), s["was"].as_str().unwrap(), s["now"].as_str().unwrap_or("unset"))).collect();
        assert_eq!(
            steps,
            vec![
                "reviewer.fallback none → codex/gpt-6-luna",
                "worker.agent claude/claude-sonnet-5-5 → codex/gpt-6-luna",
                "worker.effort high → ultra",
                "worker.fallback none → claude/claude-sonnet-5-5, omp",
                "light  → omp/deepseek/deepseek-v4-flash",
            ],
            "the preview lists every field that changes, the roles in order, the agent before its effort and fallback"
        );
        assert!(prev["steps"][1]["command"].as_str().unwrap().ends_with("set worker.agent codex/gpt-6-luna"));
        assert_eq!(prev["steps"][4]["command"], "", "the light preset is the planner's, not a board command");
        assert!(board_commands(&log).is_empty(), "the preview changes nothing");

        // not without the confirmation
        let (code, _) = json_of(station_preset_apply(State(app.clone()), headers(), Json(ApplyBody { confirm: None, ..body() })).await).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(board_commands(&log).is_empty());

        let (code, done) = json_of(station_preset_apply(State(app.clone()), headers(), Json(body())).await).await;
        assert_eq!(code, StatusCode::OK, "{done}");
        // the roles in order: coordinator, plan, reviewer, worker; for each the agent, the effort, the fallback
        assert_eq!(
            board_commands(&log),
            vec![
                "set reviewer.fallback codex/gpt-6-luna",
                "set worker.agent codex/gpt-6-luna",
                "set worker.effort ultra",
                "set worker.fallback claude/claude-sonnet-5-5, omp",
            ]
        );
        assert_eq!(read_light(&app.cfg), "omp/deepseek/deepseek-v4-flash", "the light preset is kept by the planner");
        assert_eq!(crew::read_store(&app.cfg).active[""], id, "the planner remembers which preset it applied");
    }

    #[tokio::test]
    async fn a_refusal_stops_the_run_and_says_what_was_done() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = app_with_board(dir.path(), "worker.effort");
        let id = economy(&app);
        wait_models(&app, &["claude", "codex", "omp"]).await;
        let (code, err) = json_of(
            station_preset_apply(State(app.clone()), headers(), Json(ApplyBody { id: id.clone(), repo: None, confirm: Some(true) })).await,
        )
        .await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
        let msg = err["error"].as_str().unwrap();
        assert!(msg.contains("worker.effort") && msg.contains("refused by the stand-in"), "{msg}");
        assert!(msg.contains("Уже выполнено: reviewer.fallback, worker.agent"), "{msg}");
        let cmds = board_commands(&log);
        assert_eq!(cmds.len(), 3, "stops at the refused one: {cmds:?}");
        assert!(crew::read_store(&app.cfg).active.is_empty(), "a run that stopped makes nothing active");
        assert_eq!(read_light(&app.cfg), "", "and the planner's own part has not run yet");
    }

    #[tokio::test]
    async fn a_preset_with_a_bad_model_is_refused_before_anything_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = app_with_board(dir.path(), "");
        let roles = crew_roles(&[("reviewer", "claude/claude-nope", "", ""), ("worker", "codex/gpt-6-luna", "", "")]);
        let id = crew::edit_store(&app.cfg, |s| crew::add_preset(s, "Кривой", &roles)).unwrap();
        wait_models(&app, &["claude", "codex"]).await;
        let (code, err) = json_of(station_preset_apply(State(app.clone()), headers(), Json(ApplyBody { id, repo: None, confirm: Some(true) })).await).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err["error"].as_str().unwrap().contains("такой модели нет"), "{err}");
        assert!(board_commands(&log).is_empty(), "nothing is sent when one step is wrong");
    }

    #[tokio::test]
    async fn the_station_shows_a_subscription_the_board_says_is_out_of_allowance() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _log) = app_with_board(dir.path(), "");
        {
            let mut snap = app.snapshot.write().await;
            let mut t = crate::board::Task::default();
            t.id = "planner/7".into();
            t.repo = "planner".into();
            t.tag = "PLN-7".into();
            t.app = "codex".into();
            t.status = Status::Working;
            t.msg = "codex took over from claude: out of allowance until 17:00".into();
            snap.tasks = vec![t];
        }
        let installed: Vec<String> = ["claude", "codex", "omp"].map(String::from).to_vec();
        let q = StationQuery { repo: None };
        let v = station_read(&app, q, &installed).await.unwrap();
        wait_models(&app, &["claude", "codex", "omp"]).await;
        let prog = |name: &str| v["programs"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap().clone();
        let claude = prog("claude");
        assert_eq!(claude["state"]["avail"]["state"], "limited", "{claude}");
        assert_eq!(claude["state"]["avail"]["until"], "17:00");
        assert_eq!(claude["state"]["avail"]["source"], "board");
        let tags: Vec<&str> = claude["roles"].as_array().unwrap().iter().map(|r| r["role"].as_str().unwrap()).collect();
        assert_eq!(tags, vec!["coordinator", "plan", "reviewer", "worker"], "who uses it");
        let codex = prog("codex");
        assert_eq!(codex["state"]["avail"]["state"], "signed_out", "codex is not signed in: {codex}");
        assert_eq!(codex["tasks"][0]["id"], "PLN-7", "the task that runs on codex now");
        let omp = prog("omp");
        assert_eq!(omp["state"]["signed_in"], Value::Null);
        assert!(omp["state"]["unknown_why"].as_str().unwrap().contains("ключам API"), "{omp}");
        assert_eq!(prog("pi")["state"]["avail"]["state"], "missing");
        // the stand-in board lists three models for claude only after the first read: the program says it is loading or not
        assert!(claude["models"]["loading"].is_boolean());
    }

    #[tokio::test]
    async fn presets_are_saved_from_the_current_crew_and_the_station_says_which_is_active() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _log) = app_with_board(dir.path(), "");
        let make = |op: &str, name: &str| PresetBody {
            op: op.into(),
            id: None,
            name: Some(name.into()),
            roles: None,
            kind: None,
            repo: None,
        };
        let (code, saved) = json_of(station_presets(State(app.clone()), headers(), Json(make("capture", "Сейчас"))).await).await;
        assert_eq!(code, StatusCode::OK, "{saved}");
        let id = saved["id"].as_str().unwrap().to_string();
        let installed: Vec<String> = vec!["claude".into()];
        let v = station_read(&app, StationQuery { repo: None }, &installed).await.unwrap();
        wait_models(&app, &["claude"]).await;
        assert_eq!(v["presets"]["list"][0]["name"], "Сейчас");
        assert_eq!(v["presets"]["list"][0]["diff"], 0, "the captured crew is the crew");
        assert_eq!(v["presets"]["active"]["state"], "matches");
        // a second preset is not active until it is applied; renaming and deleting
        let (code, _) = json_of(station_presets(State(app.clone()), headers(), Json(PresetBody { id: Some(id.clone()), ..make("rename", "Рабочий") })).await).await;
        assert_eq!(code, StatusCode::OK);
        let (code, dup) = json_of(station_presets(State(app.clone()), headers(), Json(make("create", "рабочий"))).await).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{dup}");
        let (code, _) = json_of(station_presets(State(app.clone()), headers(), Json(PresetBody { id: Some(id), ..make("delete", "") })).await).await;
        assert_eq!(code, StatusCode::OK);
        let v = station_read(&app, StationQuery { repo: None }, &installed).await.unwrap();
        assert!(v["presets"]["list"].as_array().unwrap().is_empty());
        assert!(v["presets"]["active"].is_null());
    }

    #[tokio::test]
    async fn the_kept_hand_over_of_a_task_marks_omp_out_of_allowance_when_the_row_line_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _log) = app_with_board(dir.path(), "");
        let task_dir = dir.path().join("data/tasks/planner/7");
        std::fs::create_dir_all(&task_dir).unwrap();
        let rec = json!({"handovers": [{"at": now_ms() - 60_000, "role": "worker", "from": "omp", "to": "claude", "why": "out of allowance"}]});
        std::fs::write(task_dir.join("task.json"), rec.to_string()).unwrap();
        {
            let mut snap = app.snapshot.write().await;
            let mut t = crate::board::Task::default();
            t.id = "planner/7".into();
            t.repo = "planner".into();
            t.n = 7;
            t.tag = "PLN-7".into();
            t.app = "claude".into();
            t.status = Status::Working;
            t.msg = "Writing the tests".into();
            snap.tasks = vec![t];
        }
        let installed: Vec<String> = ["claude", "omp"].map(String::from).to_vec();
        let v = station_read(&app, StationQuery { repo: None }, &installed).await.unwrap();
        wait_models(&app, &["claude", "omp"]).await;
        let omp = v["programs"].as_array().unwrap().iter().find(|p| p["name"] == "omp").unwrap().clone();
        assert_eq!(omp["state"]["avail"]["state"], "limited", "{omp}");
        assert_eq!(omp["state"]["avail"]["source"], "board");
        assert!(omp["state"]["avail"]["until_ms"].as_u64().unwrap() > now_ms());
    }

    /* ───────── the spares of the Coordinator, the planner and the light tasks (PLN-89) ───────── */

    /// A task whose line says claude is out of allowance until 17:00 (or no task at all).
    async fn claude_limited(app: &Shared, on: bool) {
        let mut snap = app.snapshot.write().await;
        snap.tasks.clear();
        if on {
            let mut t = crate::board::Task::default();
            t.id = "planner/7".into();
            t.repo = "planner".into();
            t.tag = "PLN-7".into();
            t.app = "omp".into();
            t.status = Status::Working;
            t.msg = "omp took over from claude: out of allowance until 17:00".into();
            snap.tasks = vec![t];
        }
    }

    fn installed() -> Vec<String> {
        ["claude", "codex", "omp"].map(String::from).to_vec()
    }

    /// What the board was told to do (not what it was asked to read).
    fn all_calls(log: &FsPath) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("set ") || l.starts_with("unset ") || l.starts_with("coordinator "))
            .map(String::from)
            .collect()
    }

    /// The stand-in board's `show` now says the role runs on `agent` (it keeps no state of its own).
    fn board_says(dir: &FsPath, name: &str, agent: &str) {
        let show = dir.join("show.json");
        let text = std::fs::read_to_string(&show).unwrap();
        let from = format!(r#"{{"name":"{name}","kind":"work","value":"claude/claude-opus-5-5""#);
        assert!(text.contains(&from), "{name} is in the stand-in's settings");
        std::fs::write(&show, text.replace(&from, &format!(r#"{{"name":"{name}","kind":"work","value":"{agent}""#))).unwrap();
    }

    fn keep_chain(app: &Shared, role: &str, chain: &str, auto: bool) {
        failover::edit_store(&app.cfg, |s| {
            s.roles.insert(role.into(), failover::Spare { chain: chain.into(), auto });
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn a_coordinator_out_of_allowance_goes_to_the_next_usable_spare_with_the_boards_commands() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = app_with_board(dir.path(), "");
        // codex is signed out in the stand-in's doctor, so the chain skips it and lands on omp
        keep_chain(&app, "coordinator", "codex/gpt-6-luna, omp/deepseek/deepseek-v4-flash", true);
        claude_limited(&app, true).await;
        wait_models(&app, &["claude", "codex", "omp"]).await;
        failover::tick_with(&app, &installed()).await;
        assert_eq!(board_commands(&log), vec!["set coordinator.agent omp/deepseek/deepseek-v4-flash --main-coordinator"]);
        assert!(all_calls(&log).iter().any(|l| l == "coordinator new"), "and a new Coordinator is started: {:?}", all_calls(&log));
        let sw = failover::read_store(&app.cfg).switched["coordinator"].clone();
        assert_eq!((sw.home.as_str(), sw.to.as_str(), sw.until.as_str()), ("claude/claude-opus-5-5", "omp/deepseek/deepseek-v4-flash", "17:00"));
        let journal = app.snapshot.read().await.journal.iter().map(|e| e.text.clone()).collect::<Vec<_>>();
        assert!(
            journal.iter().any(|t| t == "Координатор: claude/claude-opus-5-5 → omp/deepseek/deepseek-v4-flash, лимит Claude до 17:00"),
            "{journal:?}"
        );
        // the board now says so: the next look changes nothing
        board_says(dir.path(), "coordinator.agent", "omp/deepseek/deepseek-v4-flash");
        let before = all_calls(&log).len();
        failover::tick_with(&app, &installed()).await;
        assert_eq!(all_calls(&log).len(), before, "a second look changes nothing: {:?}", all_calls(&log));
    }

    #[tokio::test]
    async fn the_return_is_only_offered_and_runs_the_commands_when_the_human_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = app_with_board(dir.path(), "");
        keep_chain(&app, "coordinator", "omp/deepseek/deepseek-v4-flash", true);
        claude_limited(&app, true).await;
        wait_models(&app, &["claude", "codex", "omp"]).await;
        failover::tick_with(&app, &installed()).await;
        board_says(dir.path(), "coordinator.agent", "omp/deepseek/deepseek-v4-flash");
        assert_eq!(board_commands(&log).len(), 1);
        // the subscription is back: the planner says so once and changes nothing
        claude_limited(&app, false).await;
        failover::tick_with(&app, &installed()).await;
        failover::tick_with(&app, &installed()).await;
        assert_eq!(board_commands(&log).len(), 1, "no silent return");
        let store = failover::read_store(&app.cfg);
        assert!(store.switched["coordinator"].offered);
        let told = app.snapshot.read().await.journal.iter().filter(|e| e.text.contains("снова доступен")).count();
        assert_eq!(told, 1, "the offer is made once");
        let v = station_read(&app, StationQuery { repo: None }, &installed()).await.unwrap();
        assert_eq!(v["failover"]["coordinator"]["switched"]["back_ready"], true, "{}", v["failover"]);
        // «Вернуть»
        let back = |op: &str| failover::BackBody::test(op, "coordinator");
        let (code, done) = json_of(failover::back_for_test(&app, back("back"), &installed()).await).await;
        assert_eq!(code, StatusCode::OK, "{done}");
        assert_eq!(board_commands(&log).last().unwrap(), "set coordinator.agent claude/claude-opus-5-5 --main-coordinator");
        assert_eq!(all_calls(&log).iter().filter(|l| *l == "coordinator new").count(), 2);
        assert!(failover::read_store(&app.cfg).switched.is_empty());
    }

    #[tokio::test]
    async fn the_planner_agent_is_switched_with_one_set_and_no_new_coordinator() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = app_with_board(dir.path(), "");
        keep_chain(&app, "plan", "omp/deepseek/deepseek-v4-flash", true);
        claude_limited(&app, true).await;
        wait_models(&app, &["claude", "codex", "omp"]).await;
        failover::tick_with(&app, &installed()).await;
        assert_eq!(board_commands(&log), vec!["set plan.agent omp/deepseek/deepseek-v4-flash"]);
        assert!(!all_calls(&log).iter().any(|l| l.starts_with("coordinator ")), "the Coordinator is not touched");
        assert!(failover::read_store(&app.cfg).switched.contains_key("plan"));
    }

    #[tokio::test]
    async fn nothing_is_done_without_a_chain_with_the_mark_off_or_when_no_spare_is_usable() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = app_with_board(dir.path(), "");
        claude_limited(&app, true).await;
        wait_models(&app, &["claude", "codex", "omp"]).await;
        failover::tick_with(&app, &installed()).await;
        assert!(all_calls(&log).is_empty(), "no chain: nothing is done: {:?}", all_calls(&log));
        keep_chain(&app, "coordinator", "omp/deepseek/deepseek-v4-flash", false);
        keep_chain(&app, "plan", "codex/gpt-6-luna", true);
        failover::tick_with(&app, &installed()).await;
        assert!(board_commands(&log).is_empty(), "the mark is off for the Coordinator; codex, the only spare of the plan, is signed out");
        assert!(failover::read_store(&app.cfg).switched.is_empty());
    }

    #[tokio::test]
    async fn a_light_task_starts_on_the_first_usable_agent_and_carries_the_rest_as_its_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _log) = app_with_board(dir.path(), "");
        write_light(&app.cfg, "claude/claude-haiku-5-5").unwrap();
        keep_chain(&app, "light", "codex/gpt-6-luna, omp/deepseek/deepseek-v4-flash", true);
        claude_limited(&app, true).await;
        wait_models(&app, &["claude", "codex", "omp"]).await;
        let down = down_programs(&app, &installed()).await.unwrap();
        let pick = failover::pick_light("claude/claude-haiku-5-5", &failover::read_store(&app.cfg).spare("light"), &down).unwrap();
        assert_eq!((pick.agent.as_str(), pick.fallback.as_str()), ("omp/deepseek/deepseek-v4-flash", ""));
        let v = station_read(&app, StationQuery { repo: None }, &installed()).await.unwrap();
        assert_eq!(v["failover"]["light"]["use"], "omp/deepseek/deepseek-v4-flash", "the page says where light tasks would go now");
        assert_eq!(v["failover"]["light"]["chain"], "codex/gpt-6-luna, omp/deepseek/deepseek-v4-flash");
    }

    #[tokio::test]
    async fn the_chain_of_a_role_is_kept_checked_and_cleared_by_the_planner() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = app_with_board(dir.path(), "");
        wait_models(&app, &["claude", "codex", "omp"]).await;
        let body = |role: &str, chain: Option<&str>, auto: Option<bool>| failover::SpareBody::test(role, chain, auto);
        let (code, _) = json_of(failover::failover_set(State(app.clone()), headers(), Json(failover::SpareBody::unconfirmed("coordinator", Some("omp")))).await).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "not without the confirmation");
        let (code, _) = json_of(failover::failover_set(State(app.clone()), headers(), Json(body("reviewer", Some("omp"), None))).await).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "the board keeps the Reviewer's");
        let (code, err) = json_of(failover::failover_set(State(app.clone()), headers(), Json(body("coordinator", Some("claude/claude-nope"), None))).await).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
        let (code, ok) = json_of(failover::failover_set(State(app.clone()), headers(), Json(body("plan", Some("omp, claude/claude-haiku-5-5"), None))).await).await;
        assert_eq!(code, StatusCode::OK, "{ok}");
        let (code, _) = json_of(failover::failover_set(State(app.clone()), headers(), Json(body("plan", None, Some(false)))).await).await;
        assert_eq!(code, StatusCode::OK);
        let s = failover::read_store(&app.cfg).spare("plan");
        assert_eq!((s.chain.as_str(), s.auto), ("omp, claude/claude-haiku-5-5", false));
        let v = station_read(&app, StationQuery { repo: None }, &installed()).await.unwrap();
        assert_eq!(v["failover"]["plan"]["auto"], false);
        let (code, _) = json_of(failover::failover_set(State(app.clone()), headers(), Json(body("plan", Some("none"), Some(true)))).await).await;
        assert_eq!(code, StatusCode::OK);
        assert!(failover::read_store(&app.cfg).roles.is_empty(), "an empty chain with the default mark is no entry");
        assert!(board_commands(&log).is_empty(), "the board is not asked: it has no such setting");
    }
}
