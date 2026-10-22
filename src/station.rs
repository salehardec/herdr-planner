//! The Station of the Galaxy («Станция»): its crew (the program that runs the Coordinator, the planner, the reviewer,
//! the workers and the spares), the settings of this machine's board, and the «light tasks» preset the planner keeps
//! for it. The board is read and written through `app::run_board`. The ship's own settings live in `ship.rs`.

use crate::app::{self, run_board, Shared};
use crate::board::{Status, Task};
use crate::config::AppConfig;
use crate::models;
use crate::server::{clip, error, same_origin, valid_agent};
use crate::taskmeta;
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

/// `program/model` as a `*.agent` value takes it: returns the program, or says what shape is wrong.
pub fn agent_program(value: &str) -> Result<String, String> {
    match value.split_once('/') {
        Some((program, model)) if models::valid_program(program) && !model.is_empty() => {
            Ok(program.to_string())
        }
        _ => Err(format!(
            "«{value}»: нужно «программа/модель», например claude/claude-opus-5-5"
        )),
    }
}

/// A `*.agent` value against the models its program lists. An empty list (not read yet, or a program with no
/// list) checks nothing: the board decides on apply.
pub fn check_agent(value: &str, names: &[String]) -> Result<(), String> {
    if names.is_empty() || names.iter().any(|n| n == value) {
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
    if levels.is_empty() || levels.iter().any(|l| l == value) {
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

fn read_light(cfg: &AppConfig) -> String {
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
fn available() -> Vec<String> {
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

async fn station_read(app: &Shared, q: StationQuery) -> Result<Value, Fail> {
    let repo = q.repo.unwrap_or_default();
    if !repo.is_empty() && !taskmeta::valid_repo(&repo) {
        return Err(Fail::new(400, "bad universe name"));
    }
    let (settings, doctor) = tokio::join!(read_settings(&app.cfg, &repo), read_doctor(&app.cfg));
    let settings = settings?;
    let doctor = doctor?;
    Ok(json!({
        "ok": true,
        "level": if repo.is_empty() { "you" } else { "repo" },
        "repo": repo,
        "repos": repos_of(app),
        "version": doctor_version(&doctor),
        "settings": settings,
        "programs": programs_of(&doctor, &available()),
        "light": read_light(&app.cfg),
    }))
}

/// `GET /api/station?repo=R`: the crew, the board's settings and the light preset.
pub async fn station_get(State(app): State<Shared>, Query(q): Query<StationQuery>) -> Response {
    match station_read(&app, q).await {
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
    Ok(json!({
        "ok": true,
        "program": q.program,
        "names": listed.names,
        "effort": listed.effort,
        "note": listed.note,
        "loading": listed.loading,
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
        let Some((program, _)) = agent.split_once('/') else {
            return Ok(());
        };
        let program = program.to_string();
        return check_effort(value, &models_of(app, &program).await.effort, &program);
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
    if !body.agent.is_empty() && !valid_agent(&body.agent) {
        return error(
            StatusCode::BAD_REQUEST,
            "нужно «программа/модель», например codex/gpt-5.5",
        );
    }
    match write_light(&app.cfg, &body.agent) {
        Ok(()) => Json(json!({"ok": true, "light": body.agent})).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
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
        assert!(agent_program("claude").is_err());
        assert!(agent_program("claude/").is_err());
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
    }

    #[test]
    fn an_effort_must_be_one_the_program_takes() {
        let levels = vec!["low".to_string(), "high".to_string()];
        assert!(check_effort("high", &levels, "codex").is_ok());
        let err = check_effort("ultra", &levels, "codex").unwrap_err();
        assert!(err.contains("codex") && err.contains("low, high"), "{err}");
        assert!(check_effort("ultra", &[], "omp").is_ok());
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
}
