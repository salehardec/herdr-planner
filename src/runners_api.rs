//! The shipyards' HTTP calls: the list, one probe (with the agents' own way in), adding and removing a yard, the
//! agents' block, «было → стало» cleaning, the disk count and its cleaning, the auto-clean switch and the terminal.

use crate::app::{self, Shared};
use crate::config::{now_ms, AppConfig};
use crate::ssh::{self, MachineRef};
use crate::runners::{self, Class, CleanPlan, Runner, RunnerState, RunnersFile};
use crate::{runners_auto, runners_disk};
use crate::server::{client_is_local, same_origin};
use crate::station;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::time::Duration;

const CLEAN_WAIT: Duration = Duration::from_secs(60);

fn fail(status: u16, message: impl Into<String>) -> Response {
    crate::server::error(
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        message,
    )
}

fn file_of(app: &Shared) -> Result<RunnersFile, Response> {
    runners::load(&app.cfg.runners_file()).map_err(|e| fail(500, e))
}

/// Fills the classes of a state with the tasks this board's snapshot holds (a probe knows nothing about tasks).
fn classify_now(app: &Shared, snap: &app::Snapshot, state: &mut RunnerState) {
    let repos = crate::taskmeta::repos(&app.cfg.data_dir);
    let idx = runners::TaskIndex::build(&snap.tasks, &snap.archived, &repos);
    runners::classify(&mut state.containers, &state.protected, &idx);
}

async fn state_now(app: &Shared, id: &str) -> Option<RunnerState> {
    app.snapshot
        .read()
        .await
        .runners
        .iter()
        .find(|r| r.id == id)
        .cloned()
}

fn index_of(app: &Shared, snap: &app::Snapshot) -> runners::TaskIndex {
    let repos = crate::taskmeta::repos(&app.cfg.data_dir);
    runners::TaskIndex::build(&snap.tasks, &snap.archived, &repos)
}

/// The state of the file and the runner reads, plus the agents' block state.
pub async fn runners_get(State(app): State<Shared>) -> Response {
    let (file, error) = match runners::load(&app.cfg.runners_file()) {
        Ok(f) => (f, String::new()),
        Err(e) => (RunnersFile::default(), e),
    };
    let snap = app.snapshot.read().await.clone();
    Json(json!({
        "ok": error.is_empty(),
        "error": error,
        "file": {
            "path": app.cfg.runners_file().display().to_string(),
            "default": file.default_runner().map(|r| r.id.clone()).unwrap_or_default(),
            "updated_at_ms": file.updated_at_ms,
            "runners": file.runners,
        },
        "runners": snap.runners,
        "block": snap.runner_block,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct AskBody {
    id: String,
}

/// `POST /api/runner/ask`: read one yard now.
pub async fn runner_ask(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<AskBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    let file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(r) = file.get(&body.id).cloned() else {
        return fail(404, "такой верфи нет в списке");
    };
    let fresh = runners::probe(&app.cfg, &r).await;
    let state = {
        let prev = state_now(&app, &body.id).await;
        let mut kept = runners::keep_last(prev.as_ref(), fresh);
        let snap = app.snapshot.read().await.clone();
        classify_now(&app, &snap, &mut kept);
        kept
    };
    if let Ok(json) = serde_json::to_string(&state) {
        app.db.runner_put(
            &state.id,
            &json,
            state.ok,
            state.why.as_ref().map(|w| w.line.as_str()).unwrap_or(""),
        );
    }
    {
        let mut list = app.runners.lock().await;
        list.retain(|s| s.id != state.id);
        list.push(state.clone());
        list.sort_by(|a, b| a.id.cmp(&b.id));
    }
    app::rebuild_runners(&app).await;
    app::publish(&app).await;
    app::index_snapshot(&app).await;
    Json(json!({"ok": state.ok, "runner": state, "why": state.why})).into_response()
}

#[derive(Deserialize)]
pub struct ProbeBody {
    target: String,
    label: Option<String>,
}

/// The masks a probe suggests: a compose project as it is, else the first two segments of a foreign name.
pub fn suggest_protected(containers: &[runners::Container]) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for c in containers {
        if c.task.is_some() || c.class == Class::Unsure {
            continue;
        }
        if let Some(p) = c.labels.get("com.docker.compose.project").filter(|p| !p.is_empty()) {
            set.insert(p.clone());
            continue;
        }
        let name = &c.name;
        if name.is_empty() || c.id.is_empty() {
            continue;
        }
        if let Some((first, _)) = name.split_once('_') {
            if !first.is_empty() {
                set.insert(format!("{first}_*"));
            }
            continue;
        }
        let parts: Vec<&str> = name.split('-').collect();
        match parts.as_slice() {
            [a, b, ..] if parts.len() >= 3 && !a.is_empty() && !b.is_empty() => {
                set.insert(format!("{a}-{b}-*"));
            }
            [a, ..] if !a.is_empty() => {
                set.insert(format!("{a}-*"));
            }
            _ => {}
        }
    }
    set.into_iter().collect()
}

pub fn suggested_tags(host: &runners::HostState) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    if !host.arch.is_empty() {
        tags.push(host.arch.clone());
    }
    if let Some(major) = host.docker_version.split('.').next().filter(|m| !m.is_empty()) {
        tags.push(format!("docker{major}"));
    }
    if !host.gpu.is_empty() || host.runtimes.iter().any(|r| r == "nvidia") {
        tags.push("gpu".into());
    }
    tags.sort();
    tags.dedup();
    tags
}

fn docker_client_here() -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':').any(|dir| {
        let p = std::path::Path::new(dir).join("docker");
        p.is_file()
    })
}

/// The way the agents will come in: the user's own ssh config and known hosts, then docker through it. Nothing on
/// the far side is changed; a fresh host key is accepted once, as `DOCKER_HOST=ssh://` will accept it.
async fn local_checks(cfg: &AppConfig, target: &str) -> Value {
    let client = docker_client_here();
    let (ssh_ok, mut hint) = match ssh::user_ssh_ended(cfg, target, "true", Duration::from_secs(15)).await {
        Ok(e) if !e.timed_out && e.error.is_empty() && e.code == Some(0) => (true, String::new()),
        Ok(e) if e.timed_out => (false, "ssh не дозвонилась за 15 с".to_string()),
        Ok(e) => {
            let m = MachineRef { label: "эта машина".into(), target: target.into(), session: String::new() };
            let w = ssh::why_not_with(
                &e,
                &m,
                &ssh::CheckHint { test: ssh::hand_check(target, &[], "true"), program: "ssh".into() },
            );
            (false, w.line)
        }
        Err(e) => (false, e),
    };
    let mut via = String::new();
    if ssh_ok {
        match ssh::user_ssh_ended(
            cfg,
            target,
            "docker version --format '{{.Server.Version}}'",
            Duration::from_secs(20),
        )
        .await
        {
            Ok(e) if !e.timed_out && e.error.is_empty() && e.code == Some(0) => {
                via = e.stdout.lines().last().unwrap_or("").trim().to_string();
            }
            Ok(e) if e.timed_out => hint = "docker не ответил за 20 с".to_string(),
            Ok(e) => {
                let m = MachineRef { label: "эта машина".into(), target: target.into(), session: String::new() };
                hint = ssh::why_not_with(
                    &e,
                    &m,
                    &ssh::CheckHint { test: ssh::hand_check(target, &[], "docker version"), program: "docker".into() },
                )
                .line;
            }
            Err(e) => hint = e,
        }
    }
    if !client && hint.is_empty() {
        hint = "на этой машине нет клиента docker".to_string();
    }
    json!({
        "docker_client": client,
        "ssh_ok": ssh_ok,
        "docker_via_ssh": via,
        "hint": hint,
    })
}

/// `POST /api/runner/probe`: one read, nothing changed; the wizard's checklist, the agents' way in and the
/// suggestions for the tags and the protected masks.
pub async fn runner_probe(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<ProbeBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    // `ssh://user@host:2222` and `user@host:2222` are the same yard: store one form, no scheme.
    let target = ssh::normalize_target(&body.target);
    if !ssh::valid_target(&target) {
        return fail(400, format!("адрес верфи не похож на ssh-адрес: {target:?}"));
    }
    let label = body
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| ssh::short_label(&target));
    if !ssh::valid_label(&label) {
        return fail(400, format!("метка верфи: буквы, цифры, «-», «_», «.»: {label:?}"));
    }
    let ports_host = ssh::config_host(&app.cfg, &target)
        .unwrap_or_else(|| runners::host_of_target(&target));
    let r = Runner {
        id: label.clone(),
        target: target.clone(),
        ports_host: ports_host.clone(),
        tags: Vec::new(),
        protected: Vec::new(),
        note: String::new(),
        added_at_ms: 0,
        capacity: None,
        auto: Default::default(),
    };
    let local = local_checks(&app.cfg, &target).await;
    let state = runners::probe(&app.cfg, &r).await;
    if !state.ok {
        let why = state.why.clone().unwrap_or_default();
        return Json(json!({
            "ok": false,
            "step": "probe",
            "error": if why.line.is_empty() { "верфь не ответила".to_string() } else { why.line.clone() },
            "why": why,
            "probe": state,
            "local": local,
        }))
        .into_response();
    }
    if state.host.docker_version.is_empty() {
        return Json(json!({
            "ok": false,
            "step": "probe",
            "error": "docker не найден в оболочке входа (или пользователь не в группе docker)",
            "probe": state,
            "local": local,
        }))
        .into_response();
    }
    // The machine answered, so the yard can be added even if this machine's own way in is red: the rows say plainly
    // what is broken, and the human decides (a yard is still useful to watch).
    let local_ok = local["docker_client"].as_bool().unwrap_or(false)
        && local["ssh_ok"].as_bool().unwrap_or(false)
        && !local["docker_via_ssh"].as_str().unwrap_or("").is_empty();
    let mut state2 = state.clone();
    // Suggest only masks for what no task of this board owns.
    let snap = app.snapshot.read().await.clone();
    let idx = index_of(&app, &snap);
    runners::classify(&mut state2.containers, &[], &idx);
    let protected = suggest_protected(&state2.containers);
    Json(json!({
        "ok": true,
        "label": label,
        "target": target,
        "ports_host": ports_host,
        "probe": state,
        "local": local,
        "local_ok": local_ok,
        "suggested": {
            "tags": suggested_tags(&state.host),
            "protected": protected,
            "ports_host": ports_host,
        },
    }))
    .into_response()
}

/* ───────── add, set, remove ───────── */

#[derive(Deserialize)]
pub struct AddBody {
    target: String,
    label: String,
    #[serde(default)]
    ports_host: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    protected: Vec<String>,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    note: String,
    confirm: Option<bool>,
}

/// After a change of the list: the page's snapshot at once, the agents' block, and a read of a new yard so it does
/// not sit «нет связи» until the next poll.
fn spawn_after_change(app: Shared, probe: bool) {
    tokio::spawn(async move {
        if probe {
            runners::poll_all(&app).await;
        }
        runners::sync_blocks(&app).await;
        app::rebuild_runners(&app).await;
        app::publish(&app).await;
        app::index_snapshot(&app).await;
    });
}

/// `POST /api/runner/add`: writes the list and refreshes the agents' block if it was connected.
pub async fn runner_add(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<AddBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return fail(400, "добавление верфи требует подтверждения");
    }
    let target = ssh::normalize_target(&body.target);
    let label = body.label.trim().to_string();
    if !ssh::valid_target(&target) {
        return fail(400, format!("адрес верфи не похож на ssh-адрес: {target:?}"));
    }
    if !ssh::valid_label(&label) {
        return fail(400, format!("метка верфи: буквы, цифры, «-», «_», «.»: {label:?}"));
    }
    let mut file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    if file.get(&label).is_some() {
        return fail(409, format!("верфь с меткой «{label}» уже есть"));
    }
    let ports_host = if body.ports_host.trim().is_empty() {
        ssh::config_host(&app.cfg, &target)
            .unwrap_or_else(|| runners::host_of_target(&target))
    } else {
        body.ports_host.trim().to_string()
    };
    let entry = Runner {
        id: label.clone(),
        target,
        ports_host,
        tags: body.tags,
        protected: body.protected,
        note: body.note,
        added_at_ms: now_ms(),
        capacity: None,
        auto: Default::default(),
    };
    if file.runners.is_empty() || body.default {
        file.default = label.clone();
    }
    file.runners.push(entry);
    file.updated_at_ms = now_ms();
    if let Err(e) = runners::save(&app.cfg.runners_file(), &file) {
        return fail(422, e);
    }
    {
        let mut list = app.runners.lock().await;
        if !list.iter().any(|s| s.id == label) {
            list.push(RunnerState::from_runner(file.get(&label).unwrap()));
        }
    }
    spawn_after_change(app.clone(), true);
    Json(json!({"ok": true, "id": label})).into_response()
}

#[derive(Deserialize)]
pub struct SetBody {
    id: String,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    protected: Option<Vec<String>>,
    #[serde(default)]
    ports_host: Option<String>,
    #[serde(default)]
    default: Option<bool>,
    #[serde(default)]
    note: Option<String>,
    confirm: Option<bool>,
}

/// `POST /api/runner/set`: tags, masks, the ports' address, the default and the note.
pub async fn runner_set(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<SetBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return fail(400, "изменение верфи требует подтверждения");
    }
    let mut file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    if file.get(&body.id).is_none() {
        return fail(404, "такой верфи нет в списке");
    }
    {
        let r = file.runners.iter_mut().find(|r| r.id == body.id).unwrap();
        if let Some(t) = body.tags {
            r.tags = t;
        }
        if let Some(p) = body.protected {
            r.protected = p;
        }
        if let Some(p) = body.ports_host {
            r.ports_host = p.trim().to_string();
        }
        if let Some(n) = body.note {
            r.note = n;
        }
    }
    if body.default == Some(true) {
        file.default = body.id.clone();
    }
    file.updated_at_ms = now_ms();
    if let Err(e) = runners::save(&app.cfg.runners_file(), &file) {
        return fail(422, e);
    }
    {
        let mut list = app.runners.lock().await;
        if let Some(s) = list.iter_mut().find(|s| s.id == body.id) {
            if let Some(r) = file.get(&body.id) {
                s.target = r.target.clone();
                s.ports_host = r.ports_host.clone();
                s.tags = r.tags.clone();
                s.protected = r.protected.clone();
                s.note = r.note.clone();
            }
        }
    }
    spawn_after_change(app.clone(), false);
    Json(json!({"ok": true, "id": body.id})).into_response()
}

#[derive(Deserialize)]
pub struct RemoveBody {
    id: String,
    #[serde(default)]
    force: bool,
    confirm: Option<bool>,
}

/// `POST /api/runner/remove`: takes the yard out of the list; its containers stay on the machine.
pub async fn runner_remove(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<RemoveBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return fail(400, "удаление верфи требует подтверждения");
    }
    let mut file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    if file.get(&body.id).is_none() {
        return fail(404, "такой верфи нет в списке");
    }
    let busy: Vec<String> = app
        .snapshot
        .read()
        .await
        .runners
        .iter()
        .find(|r| r.id == body.id)
        .map(|r| {
            r.containers
                .iter()
                .filter(|c| c.class == Class::Active)
                .map(|c| c.name.clone())
                .collect()
        })
        .unwrap_or_default();
    if !busy.is_empty() && !body.force {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "на верфи есть контейнеры задач в работе; они останутся на машине",
                "active": busy,
            })),
        )
            .into_response();
    }
    file.runners.retain(|r| r.id != body.id);
    if file.default == body.id {
        file.default = file.runners.first().map(|r| r.id.clone()).unwrap_or_default();
    }
    file.updated_at_ms = now_ms();
    if let Err(e) = runners::save(&app.cfg.runners_file(), &file) {
        return fail(422, e);
    }
    app.db.runner_remove(&body.id);
    app.runners.lock().await.retain(|s| s.id != body.id);
    spawn_after_change(app.clone(), false);
    Json(json!({"ok": true, "warned": busy})).into_response()
}

/* ───────── the agents' block ───────── */

/// `GET /api/runner/connect`: the two instruction settings of this machine, the block as it would be, and whether
/// it is in.
pub async fn runner_connect_get(State(app): State<Shared>) -> Response {
    let file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let block = runners::block_text(&file);
    let settings = match station::read_settings(&app.cfg, "").await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let mut out = Vec::new();
    for name in ["worker.instructions", "reviewer.instructions"] {
        let current = station::find(&settings, name)
            .filter(|s| s.from != "built in" && s.value.trim() != "none")
            .map(|s| s.value.clone())
            .unwrap_or_default();
        let connected = !block.is_empty() && runners::has_block(&current, &block);
        let stale = !block.is_empty() && runners::has_any_block(&current) && !connected;
        // The exact value each choice writes: the card shows it instead of the bare generated block.
        let next_on = if block.is_empty() {
            String::new()
        } else {
            runners::connect_value(&current, &block, true)
                .ok()
                .flatten()
                .unwrap_or_default()
        };
        let next_off = runners::connect_value(&current, &block, false)
            .ok()
            .flatten()
            .unwrap_or_default();
        let next_on_dropped = crate::blocks::drop_paragraphs(&next_on, runners::MARKERS, "DOCKER_HOST=");
        out.push(json!({
            "setting": name,
            "connected": connected,
            "stale": stale,
            "current": current,
            "block": block,
            "next_on": next_on,
            "next_on_dropped": next_on_dropped,
            "next_off": next_off,
            "has_old_docker_paragraph": runners::has_old_docker_paragraph(&current),
        }));
    }
    let state = app.runner_block.lock().await.clone();
    Json(json!({"ok": true, "block": state, "settings": out})).into_response()
}

#[derive(Deserialize)]
pub struct ConnectBody {
    on: bool,
    #[serde(default)]
    drop_old: bool,
    confirm: Option<bool>,
}

/// `POST /api/runner/connect`: puts the block into both instruction settings of this machine (or takes it out),
/// keeping the human's own text; `drop_old` removes the hand-written docker paragraph from before.
pub async fn runner_connect(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<ConnectBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return fail(400, "подключение к агентам требует подтверждения");
    }
    let file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let block = runners::block_text(&file);
    if block.is_empty() && body.on {
        return fail(409, "в списке нет ни одной верфи");
    }
    let settings = match station::read_settings(&app.cfg, "").await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let mut results = Vec::new();
    for name in ["worker.instructions", "reviewer.instructions"] {
        let was = station::find(&settings, name)
            .filter(|s| s.from != "built in" && s.value.trim() != "none")
            .map(|s| s.value.clone())
            .unwrap_or_default();
        let value = match runners::connect_value(&was, &block, body.on) {
            Ok(v) => v,
            Err(e) => return fail(422, e),
        };
        let value = match value {
            Some(v) if body.on && body.drop_old => {
                Some(crate::blocks::drop_paragraphs(&v, runners::MARKERS, "DOCKER_HOST="))
            }
            other => other,
        };
        let args: Vec<String> = match &value {
            Some(v) => vec!["set".into(), name.into(), v.clone()],
            None => vec!["unset".into(), name.into()],
        };
        let out = station::board_run(&app.cfg, &args, station::BOARD_WAIT).await;
        if out.ok() {
            results.push(json!({"setting": name, "was": was, "now": value}));
        } else {
            return fail(422, out.words());
        }
    }
    {
        let mut state = app.runner_block.lock().await;
        state.connected = body.on;
        state.stale = false;
        state.at = now_ms();
        state.why = String::new();
        if let Ok(json) = serde_json::to_string(&*state) {
            app.db.meta_set("runner_block", &json);
        }
    }
    app::refresh(&app).await;
    app::rebuild_runners(&app).await;
    app::publish(&app).await;
    Json(json!({"ok": true, "settings": results})).into_response()
}

/* ───────── cleaning ───────── */

#[derive(Deserialize)]
pub struct PreviewBody {
    runner: String,
    #[serde(default)]
    ids: Option<Vec<String>>,
}

/// `POST /api/runner/clean/preview`: «было → стало» and a token; nothing is removed.
pub async fn clean_preview(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<PreviewBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    let state = match state_now(&app, &body.runner).await {
        Some(s) => s,
        None => return fail(404, "такой верфи нет в списке"),
    };
    if !state.ok {
        return fail(409, "верфь не на связи: последнее чтение устарело");
    }
    let ids = body.ids.filter(|v| !v.is_empty());
    let plan = runners::plan_of(&state, ids.as_deref());
    let token = {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    {
        let mut plans = app.runner_plans.lock().await;
        plans.retain(|_, p| p.at.elapsed().as_millis() < runners::PLAN_TTL_MS as u128);
        plans.insert(token.clone(), plan.clone());
    }
    Json(runners::preview_of(&plan, &token)).into_response()
}

#[derive(Deserialize)]
pub struct CleanBody {
    runner: String,
    token: String,
    #[serde(default)]
    ids: Vec<String>,
    confirm: Option<bool>,
}

pub fn safe_id(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn safe_volume(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// `POST /api/runner/clean`: removes exactly the planned ids, by full id, then reads the yard again.
pub async fn runner_clean(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<CleanBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return fail(400, "уборка требует подтверждения");
    }
    let plan: CleanPlan = {
        let mut plans = app.runner_plans.lock().await;
        let Some(p) = plans.remove(&body.token) else {
            return fail(409, "план уборки не найден или истёк: откройте карточку заново");
        };
        if p.at.elapsed().as_millis() >= runners::PLAN_TTL_MS as u128 {
            return fail(409, "план уборки истёк: откройте карточку заново");
        }
        p
    };
    if plan.runner != body.runner {
        return fail(409, "план принадлежит другой верфи");
    }
    let state = match state_now(&app, &body.runner).await {
        Some(s) => s,
        None => return fail(404, "такой верфи нет в списке"),
    };
    // No ids means «the forgotten ones of the shown plan»: an unsure container goes only when the human ticked it
    // and the page sent its id.
    let wanted: BTreeSet<&str> = if body.ids.is_empty() {
        plan.containers
            .iter()
            .filter(|c| !plan.unsure.contains(&c.id))
            .map(|c| c.id.as_str())
            .collect()
    } else {
        body.ids.iter().map(String::as_str).collect()
    };
    if wanted.is_empty() {
        return fail(400, "ничего не выбрано");
    }
    for id in &wanted {
        if !plan.containers.iter().any(|c| c.id == *id) {
            return fail(409, "контейнера нет в показанном плане");
        }
        let Some(c) = state.containers.iter().find(|c| c.id == *id) else {
            return fail(409, "контейнер уже исчез с верфи");
        };
        if c.class == Class::Active || c.class == Class::Foreign {
            return fail(409, "контейнер задачи в работе или чужой: его не убирают");
        }
    }
    let ids: Vec<String> = plan
        .containers
        .iter()
        .filter(|c| wanted.contains(c.id.as_str()))
        .map(|c| c.id.clone())
        .collect();
    if ids.is_empty() {
        return fail(400, "ничего не выбрано");
    }
    if ids.len() > 200 || ids.iter().any(|i| !safe_id(i)) {
        return fail(400, "недопустимый id контейнера");
    }
    // The units follow the ticked containers, not the wider preview: recompute them from the same ids.
    let fresh = runners::plan_of(&state, Some(&ids));
    let nets: Vec<String> = fresh
        .networks
        .iter()
        .filter(|n| safe_id(&n.id))
        .map(|n| n.id.clone())
        .collect();
    let vols: Vec<String> = fresh
        .volumes
        .iter()
        .map(|v| v.name.clone())
        .filter(|v| safe_volume(v))
        .collect();
    let file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(r) = file.get(&body.runner).cloned() else {
        return fail(404, "такой верфи нет в списке");
    };
    let m = MachineRef {
        label: r.id.clone(),
        target: r.target.clone(),
        session: String::new(),
    };
    let cmd = format!("docker rm -f -v {}", ids.join(" "));
    let mut failed: Vec<Value> = Vec::new();
    match ssh::ssh_ended(&app.cfg, &m, &cmd, CLEAN_WAIT).await {
        Ok(e) if !e.timed_out && e.error.is_empty() && e.code == Some(0) => {}
        Ok(e) => {
            let line = if e.stderr.trim().is_empty() { e.stdout.trim().to_string() } else { e.stderr.trim().to_string() };
            for id in &ids {
                failed.push(json!({"id": id, "error": line.clone()}));
            }
        }
        Err(e) => {
            for id in &ids {
                failed.push(json!({"id": id, "error": e.clone()}));
            }
        }
    }
    let mut removed_nets = 0usize;
    if !nets.is_empty() && failed.is_empty() {
        let cmd = format!("docker network rm {}", nets.join(" "));
        if let Ok(e) = ssh::ssh_ended(&app.cfg, &m, &cmd, CLEAN_WAIT).await {
            if e.code == Some(0) {
                removed_nets = nets.len();
            }
        }
    }
    let mut removed_vols = 0usize;
    if !vols.is_empty() && failed.is_empty() {
        let cmd = format!("docker volume rm {}", vols.join(" "));
        if let Ok(e) = ssh::ssh_ended(&app.cfg, &m, &cmd, CLEAN_WAIT).await {
            if e.code == Some(0) {
                removed_vols = vols.len();
            }
        }
    }
    let fresh = runners::probe(&app.cfg, &r).await;
    let mut after = runners::keep_last(Some(&state), fresh);
    {
        let snap = app.snapshot.read().await.clone();
        classify_now(&app, &snap, &mut after);
    }
    if let Ok(json) = serde_json::to_string(&after) {
        app.db.runner_put(
            &after.id,
            &json,
            after.ok,
            after.why.as_ref().map(|w| w.line.as_str()).unwrap_or(""),
        );
    }
    {
        let mut list = app.runners.lock().await;
        list.retain(|s| s.id != after.id);
        list.push(after.clone());
        list.sort_by(|a, b| a.id.cmp(&b.id));
    }
    let removed = ids.len().saturating_sub(failed.len());
    let mb = plan.was.bytes as f64 / (1024.0 * 1024.0);
    app::journal_note(
        &app,
        &format!("runner/{}", r.id),
        "runner",
        &format!(
            "убрано {} контейнеров на верфи {} ({:.1} МБ), сетей {}, томов {}",
            removed, r.id, mb, removed_nets, removed_vols
        ),
    )
    .await;
    app::rebuild_runners(&app).await;
    app::publish(&app).await;
    app::index_snapshot(&app).await;
    Json(json!({
        "ok": failed.is_empty(),
        "removed": {"containers": removed, "networks": removed_nets, "volumes": removed_vols},
        "failed": failed,
        "after": after,
    }))
    .into_response()
}

/* ───────── the disk: counting and cleaning ───────── */

#[derive(Deserialize)]
pub struct DiskBody {
    id: String,
}

/// Counts a card from a fresh read of the yard's docker and keeps it under a token for the cleaning call.
async fn counted(app: &Shared, r: &Runner) -> Result<runners_disk::Report, String> {
    let disk = state_now(app, &r.id)
        .await
        .map(|s| (s.host.disk_size, s.host.disk_avail))
        .unwrap_or((0, 0));
    let index = {
        let snap = app.snapshot.read().await;
        index_of(app, &snap)
    };
    let mut report = runners_disk::measure(&app.cfg, r, &index, disk).await?;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    report.token = bytes.iter().map(|b| format!("{b:02x}")).collect();
    report.expires_ms = now_ms() + runners_disk::PLAN_TTL_MS;
    let mut plans = app.runner_disk_plans.lock().await;
    plans.retain(|_, p| (p.at.elapsed().as_millis() as u64) < runners_disk::PLAN_TTL_MS);
    plans.insert(report.token.clone(), runners_disk::Plan { report: report.clone(), at: std::time::Instant::now() });
    Ok(report)
}

/// `POST /api/runner/disk`: «Посчитать место». Read-only on the yard; nothing is removed.
pub async fn disk_measure(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<DiskBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    let file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(r) = file.get(&body.id).cloned() else {
        return fail(404, "такой верфи нет в списке");
    };
    match counted(&app, &r).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => fail(502, e),
    }
}

#[derive(Deserialize)]
pub struct DiskCleanBody {
    runner: String,
    token: String,
    #[serde(default)]
    images: bool,
    #[serde(default)]
    cache: bool,
    #[serde(default)]
    stopped: Vec<String>,
    #[serde(default)]
    builders: Vec<String>,
    #[serde(default)]
    volumes: Vec<String>,
    confirm: Option<bool>,
}

/// `POST /api/runner/disk/clean`: runs the ticked kinds of a counted card, and only those. Every tick must be an item
/// of the card the human saw, and of a count made again just now.
pub async fn disk_clean(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<DiskCleanBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return fail(400, "уборка требует подтверждения");
    }
    let plan = {
        let mut plans = app.runner_disk_plans.lock().await;
        let Some(p) = plans.remove(&body.token) else {
            return fail(409, "подсчёт не найден или истёк: посчитайте место заново");
        };
        p
    };
    if plan.at.elapsed().as_millis() as u64 >= runners_disk::PLAN_TTL_MS {
        return fail(409, "подсчёт истёк: посчитайте место заново");
    }
    if plan.report.runner != body.runner {
        return fail(409, "подсчёт принадлежит другой верфи");
    }
    let sel = runners_disk::Selection {
        images: body.images,
        cache: body.cache,
        stopped: body.stopped,
        builders: body.builders,
        volumes: body.volumes,
    };
    if sel.is_empty() {
        return fail(400, "ничего не выбрано");
    }
    if let Err(e) = runners_disk::commands(&plan.report, &sel) {
        return fail(409, e);
    }
    let file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(r) = file.get(&body.runner).cloned() else {
        return fail(404, "такой верфи нет в списке");
    };
    let state = match state_now(&app, &body.runner).await {
        Some(s) => s,
        None => return fail(404, "такой верфи нет в списке"),
    };
    // The count again, right before the first command: a builder started or a container restarted since the card was
    // shown is no longer on the list.
    let index = {
        let snap = app.snapshot.read().await;
        index_of(&app, &snap)
    };
    let disk = (state.host.disk_size, state.host.disk_avail);
    let fresh = match runners_disk::measure(&app.cfg, &r, &index, disk).await {
        Ok(f) => f,
        Err(e) => return fail(502, e),
    };
    let cmds = match runners_disk::commands(&fresh, &sel) {
        Ok(c) => c,
        Err(e) => return fail(409, e),
    };
    let steps = runners_disk::execute(&app.cfg, &r, &cmds).await;
    // Read the yard again for the page, then count anew for «стало».
    let fresh_state = runners::probe(&app.cfg, &r).await;
    let kept = runners::keep_last(Some(&state), fresh_state);
    if let Ok(json) = serde_json::to_string(&kept) {
        app.db.runner_put(&kept.id, &json, kept.ok, kept.why.as_ref().map(|w| w.line.as_str()).unwrap_or(""));
    }
    {
        let mut list = app.runners.lock().await;
        list.retain(|s| s.id != kept.id);
        list.push(kept.clone());
        list.sort_by(|a, b| a.id.cmp(&b.id));
    }
    let done = |kind: &str| -> usize {
        steps.iter().zip(&cmds).filter(|(s, c)| s.ok && c.kind == kind).map(|(_, c)| c.count).sum()
    };
    let ran = |kind: &str| steps.iter().zip(&cmds).any(|(s, c)| s.ok && c.kind == kind);
    let pick = |part: &runners_disk::Part, keys: &[String]| -> u64 {
        part.items.iter().filter(|i| keys.contains(&i.key)).map(|i| i.bytes).sum()
    };
    let mut bytes = pick(&plan.report.stopped, &sel.stopped)
        + pick(&plan.report.builders, &sel.builders)
        + pick(&plan.report.volumes, &sel.volumes);
    if ran("images") {
        bytes += plan.report.images.bytes;
    }
    if ran("cache") {
        bytes += plan.report.cache.bytes;
    }
    let mut parts: Vec<String> = Vec::new();
    for (kind, word) in [("stopped", "контейнеров"), ("builders", "сборщиков и их данных"), ("volumes", "томов")] {
        if done(kind) > 0 {
            parts.push(format!("{word} {}", done(kind)));
        }
    }
    if ran("images") {
        parts.push("образы без контейнеров".into());
    }
    if ran("cache") {
        parts.push("кэш сборки".into());
    }
    let failed: Vec<&runners_disk::Step> = steps.iter().filter(|s| !s.ok).collect();
    let mut note = format!(
        "уборка диска на верфи {}: {} (≈ {:.1} ГБ)",
        r.id,
        if parts.is_empty() { "ничего не убрано".to_string() } else { parts.join(", ") },
        bytes as f64 / 1e9
    );
    if let Some(f) = failed.first() {
        note.push_str(&format!("; не вышло: {}", f.error.lines().next().unwrap_or("")));
    }
    app::journal_note(&app, &format!("runner/{}", r.id), "runner", &note).await;
    app::rebuild_runners(&app).await;
    app::publish(&app).await;
    app::index_snapshot(&app).await;
    let after = counted(&app, &r).await.ok();
    Json(json!({
        "ok": failed.is_empty(),
        "steps": steps,
        "bytes": bytes,
        "after": after,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct AutoBody {
    id: String,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    hours: Option<u64>,
    confirm: Option<bool>,
}

/// `POST /api/runner/auto`: the switch and the time of the auto-clean. Switching on is the consent to remove what the
/// manual card would offer, without asking each time.
pub async fn runner_auto(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<AutoBody>) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return fail(400, "изменение требует подтверждения");
    }
    if body.enabled.is_none() && body.hours.is_none() {
        return fail(400, "нечего менять");
    }
    if body.hours.is_some_and(|h| h == 0 || h > runners_auto::MAX_HOURS) {
        return fail(422, format!("время автоуборки: от 1 до {} часов", runners_auto::MAX_HOURS));
    }
    let mut file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(entry) = file.runners.iter_mut().find(|r| r.id == body.id) else {
        return fail(404, "такой верфи нет в списке");
    };
    if let Some(on) = body.enabled {
        entry.auto.enabled = on;
    }
    if let Some(h) = body.hours {
        entry.auto.hours = h;
    }
    let auto = entry.auto.clone();
    file.updated_at_ms = now_ms();
    if let Err(e) = runners::save(&app.cfg.runners_file(), &file) {
        return fail(422, e);
    }
    app::journal_note(
        &app,
        &format!("runner/{}", body.id),
        "runner",
        &format!(
            "автоуборка на верфи {}: {}, через {} ч",
            body.id,
            if auto.enabled { "включена" } else { "выключена" },
            auto.hours
        ),
    )
    .await;
    {
        let mut tried = app.runner_auto_try.lock().await;
        tried.remove(&body.id);
    }
    app::rebuild_runners(&app).await;
    app::publish(&app).await;
    if auto.enabled {
        // Do not wait for the next poll.
        let a = app.clone();
        let r = file.get(&body.id).cloned();
        tokio::spawn(async move {
            if let Some(r) = r {
                runners_auto::run_one(&a, &r).await;
            }
        });
    }
    Json(json!({"ok": true, "auto": auto})).into_response()
}

/* ───────── the terminal ───────── */

#[derive(Deserialize)]
pub struct RunnerBody {
    id: String,
}

/// `POST /api/runner/terminal`: a terminal on this machine that ssh's into the yard.
pub async fn runner_terminal(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<RunnerBody>,
) -> Response {
    if !same_origin(&headers) {
        return fail(403, "cross-origin request refused");
    }
    if !client_is_local(addr, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Терминал открывается только за компьютером с доской", "remote": true})),
        )
            .into_response();
    }
    let file = match file_of(&app) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(r) = file.get(&body.id).cloned() else {
        return fail(404, "такой верфи нет в списке");
    };
    let args = match ssh::tty_args(&r.target) {
        Ok(a) => a,
        Err(e) => return fail(422, e),
    };
    // The same way the planner's other terminal actions go: a window of this machine's own terminal, not a bare
    // child with no tty (that one starts ssh and shows nobody anything).
    match tokio::process::Command::new("xdg-terminal-exec")
        .arg(&app.cfg.ssh_bin)
        .args(&args)
        .spawn()
    {
        Ok(_) => Json(json!({
            "ok": true,
            "command": format!("{} {}", app.cfg.ssh_bin, args.join(" ")),
        }))
        .into_response(),
        Err(e) => fail(
            500,
            format!("не удалось открыть терминал: {e}"),
        ),
    }
}

/* ───────── tests ───────── */

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn masks_are_suggested_from_foreign_names() {
        let c = |name: &str, l: &[(&str, &str)]| runners::Container {
            id: "x".repeat(64),
            name: name.into(),
            labels: labels(l),
            ..runners::Container::default()
        };
        let list = vec![
            c("neighbor-server-uxkwec-app-1", &[]),
            c("other-db", &[("com.docker.compose.project", "other")]),
            c("buildx_buildkit_x0", &[]),
            c("pg-ALP-3", &[("herdr.task", "ALP-3")]),
        ];
        let mut list2 = list.clone();
        for c in &mut list2 {
            if c.name.starts_with("pg-") {
                c.task = Some(runners::TaskRef::default());
            }
        }
        let got = suggest_protected(&list2);
        assert_eq!(got, vec!["buildx_*", "neighbor-server-*", "other"]);
    }

    #[test]
    fn tags_come_from_the_probe() {
        let mut h = runners::HostState::default();
        h.arch = "x86_64".into();
        h.docker_version = "28.3.0".into();
        h.gpu.push(runners::GpuState::default());
        assert_eq!(suggested_tags(&h), vec!["docker28", "gpu", "x86_64"]);
    }

    #[test]
    fn only_exact_ids_and_volume_names_reach_a_command() {
        assert!(safe_id(&"a".repeat(64)));
        assert!(!safe_id("a b"));
        assert!(!safe_id(&"a".repeat(63)));
        assert!(safe_volume("work-3_pgdata"));
        assert!(!safe_volume("a;rm"));
        assert!(!safe_volume("-x"));
    }
}
