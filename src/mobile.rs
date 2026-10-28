//! The API of the phone's light page, `/m/api/*`: a narrow slice of the planner.
//!
//! What a phone sees is a projection of the snapshot (`project`): the same shape the page's store reads, with the
//! paths, the sessions of the agents, the shipyards' machines and everything the phone has no use for left out. The
//! projection is built field by field from what is allowed, so a field added to the snapshot later reaches the phone
//! only when somebody puts it here. Event streams do not pass a quick tunnel, so the page waits on `poll` instead: one
//! request in flight, answered when the projection changes or after 25 seconds with nothing.
//!
//! What a phone may do is `ACTIONS`, and the action route builds the board's request itself from four fields: the
//! phone cannot name a scope, a repository, keys, files or an `anyway`. Everything else goes through the same
//! handlers as the planner's page, so the checks stay in one place (a menu changed since it was shown is refused
//! there, a text has the same limits).
//!
//! Who may call these routes is the gate's business first (`gate::POLICY`: a device that is in); `guard` is the
//! second look, for a request that did not come through the tunnel and so is judged by the planner's own login.

use crate::app::{self, AuthInfo, BoardStatus, CoordinatorState, HerdrInfo, ServerInfo, Shared, Snapshot};
use crate::board::{Project, Task};
use crate::config::now_ms;
use crate::gate::Class;
use crate::server::{self, error, same_origin, ActionBody, ChatBody, HistoryQuery, ScreenQuery};
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::Instant;

/// The long-poll's path: the gate does not count it as the device being used (a tab left open must not keep the gate up).
pub const POLL_PATH: &str = "/m/api/poll";
/// How long `poll` waits for a change before it answers 204.
const POLL_WAIT: Duration = Duration::from_secs(25);
/// What a phone may ask of the board. `anyway` (merge past a failed review) and `stop` are not here on purpose.
pub const ACTIONS: [&str; 4] = ["approve", "retry", "tell", "pick"];
/// Where the human is, in the words the Coordinator's notices and the journal use.
const VIA: &str = "с телефона";
/// The most turns of the conversation the phone takes at once.
const HISTORY_MAX: usize = 20;
const SEARCH_MAX: usize = 200;

/// The snapshot as the phone gets it, and what the page needs to know to fetch the rest.
#[derive(Serialize)]
struct Out {
    #[serde(flatten)]
    snap: Snapshot,
    /// Grows when the Coordinator's transcript does: the page reads the history again.
    chat_rev: u64,
}

/// A task as the phone gets it: what it needs to draw a card and to act on it, not where the work is on disk.
fn task(t: &Task) -> Task {
    Task {
        id: t.id.clone(),
        repo: t.repo.clone(),
        n: t.n,
        tag: t.tag.clone(),
        title: t.title.clone(),
        state: t.state.clone(),
        kind: t.kind.clone(),
        stage: t.stage.clone(),
        msg: t.msg.clone(),
        app: t.app.clone(),
        progress: t.progress,
        eta: t.eta,
        pr: t.pr.clone(),
        review: t.review.clone(),
        round: t.round,
        queue: t.queue,
        status: t.status,
        label: t.label.clone(),
        board_label: t.board_label.clone(),
        review_failed: t.review_failed,
        merge_blocked: t.merge_blocked,
        archived: t.archived,
        // only what the phone can do; `answer` is the page's own word for opening the agent's screen
        actions: t.actions.iter().copied().filter(|a| *a == "answer" || ACTIONS.contains(a)).collect(),
        created: t.created,
        description: t.description.clone(),
        worker_agent: t.worker_agent.clone(),
        model: t.model.clone(),
        exited_at: t.exited_at,
        key: t.key.clone(),
        seat: t.seat.clone(),
        // not for the phone: `session`, `worktree`, the sub-agents' work
        ..Task::default()
    }
}

fn project_of(p: &Project) -> Project {
    Project {
        name: p.name.clone(),
        key: p.key.clone(),
        active: p.active,
        done: p.done,
        total: p.total,
        statuses: p.statuses.clone(),
        quiet: p.quiet,
        empty: p.empty,
        // not for the phone: the folders, the address it was cloned from, how far it is behind
        ..Project::default()
    }
}

/// The shipyards, reduced to their alarms: a name, whether it answers and what is wrong. Not the machine's address,
/// its containers or its settings.
fn runner(r: &crate::runners::RunnerState) -> crate::runners::RunnerState {
    crate::runners::RunnerState { id: r.id.clone(), ok: r.ok, alerts: r.alerts.clone(), ..Default::default() }
}

fn coordinator(c: &CoordinatorState) -> CoordinatorState {
    let mut view = c.view.clone();
    if !view.error.is_empty() {
        // the board's own error names programs and folders of this machine
        view.error = "no answer from the Coordinator".into();
    }
    CoordinatorState {
        view,
        source: String::new(),
        history: c.history.iter().map(|m| app::ChatMessage { text: m.text.clone(), at: m.at, attachments: Vec::new() }).collect(),
        notices: c.notices.clone(),
        subagents: Vec::new(),
        subagents_note: String::new(),
        context: c.context.clone(),
        can_compact: false,
        program: c.program.clone(),
    }
}

/// The snapshot for a phone. Every field of `Snapshot` is named here, so a new one is a decision, not a leak.
pub fn project(s: &Snapshot) -> Snapshot {
    Snapshot {
        version: s.version.clone(),
        at: s.at,
        seq: s.seq,
        board: BoardStatus {
            ok: s.board.ok,
            error: if s.board.ok { String::new() } else { "the board does not answer".into() },
            stale_at: s.board.stale_at,
        },
        host: s.host.clone(),
        tasks: s.tasks.iter().map(task).collect(),
        archived: Vec::new(),
        projects: s.projects.iter().map(project_of).collect(),
        summary: s.summary.clone(),
        links: s.links.clone(),
        limits: s.limits.clone(),
        todo: Vec::new(),
        memory_suggestions: Vec::new(),
        journal: Vec::new(),
        coordinator: coordinator(&s.coordinator),
        runners: s.runners.iter().filter(|r| !r.alerts.is_empty()).map(runner).collect(),
        runner_picks: Default::default(),
        runner_repo_picks: Default::default(),
        runner_rules: Default::default(),
        runner_block: Default::default(),
        theme: crate::theme::Theme { source: String::new(), error: String::new(), ..s.theme.clone() },
        herdr: HerdrInfo { running: s.herdr.running, socket: String::new() },
        auth: AuthInfo { required: false, user: String::new() },
        server: ServerInfo { listen: String::new(), port: 0, local_only: false, tls: true, projects_dir: String::new(), attachments_dir: String::new() },
    }
}

/// What a phone is shown now, and the number that says whether it has changed.
pub struct Current {
    pub seq: u64,
    pub body: String,
}

#[derive(Default)]
struct FeedInner {
    seq: u64,
    digest: u64,
}

/// The projection's own counter: the snapshot's `seq` moves at every refresh, whether or not anything the phone
/// shows has changed, and a long-poll that answers at every refresh is no long-poll at all.
#[derive(Default)]
pub struct Feed {
    inner: tokio::sync::Mutex<FeedInner>,
}

fn digest_of(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

impl Feed {
    /// The projection now, numbered. The number moves when what the phone would draw moves (the time of the read does
    /// not count), and it starts from the clock, so a phone that kept a number from before a restart is not told
    /// "nothing new" by a planner that has counted its way back to the same one.
    pub async fn current(&self, app: &Shared) -> Current {
        let mut inner = self.inner.lock().await;
        let snapshot = app.snapshot.read().await;
        let mut state = Out { snap: project(&snapshot), chat_rev: app.bus.chat_rev() };
        drop(snapshot);
        let at = state.snap.at;
        state.snap.at = 0;
        state.snap.seq = 0;
        let digest = digest_of(&serde_json::to_vec(&state).unwrap_or_default());
        if inner.seq == 0 {
            inner.seq = now_ms() / 1000;
            inner.digest = digest;
        } else if inner.digest != digest {
            inner.seq += 1;
            inner.digest = digest;
        }
        state.snap.at = at;
        state.snap.seq = inner.seq;
                Current { seq: inner.seq, body: serde_json::to_string(&state).unwrap_or_else(|_| "{}".into()) }
    }
}

fn json_body(body: String) -> Response {
    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], body).into_response()
}

/// The second look at a request for the light page's API. A request that came through the tunnel has been judged by
/// the gate (it carries the class, and only a device that is in got this far); a request from this machine or the
/// network is judged like the rest of the planner's API, by its login.
pub async fn guard(State(app): State<Shared>, req: Request, next: Next) -> Response {
    match req.extensions().get::<Class>().copied() {
        Some(Class::Tunnel) => next.run(req).await,
        Some(Class::Local | Class::Direct) => crate::auth::require_auth(State(app), req, next).await,
        _ => error(StatusCode::FORBIDDEN, "not for this caller"),
    }
}

/// `GET /m/api/state`
pub async fn state(State(app): State<Shared>) -> Response {
    json_body(app.mobile.current(&app).await.body)
}

#[derive(Deserialize)]
pub struct PollQuery {
    /// The `seq` of the projection the page has.
    seq: Option<u64>,
}

/// `GET /m/api/poll?seq=N`: the projection when it is not the one numbered `N`, else when it changes; 204 if it does not.
pub async fn poll(State(app): State<Shared>, Query(q): Query<PollQuery>) -> Response {
    poll_for(&app, q.seq, POLL_WAIT).await
}

pub async fn poll_for(app: &Shared, seq: Option<u64>, wait: Duration) -> Response {
    // Listen before looking: a change that lands between the two is not lost.
    let mut changes = app.bus.watch();
    let end = Instant::now() + wait;
    loop {
        let now = app.mobile.current(app).await;
        if seq != Some(now.seq) {
            return json_body(now.body);
        }
        match tokio::time::timeout_at(end, changes.changed()).await {
            Ok(Ok(())) => continue,
            _ => return StatusCode::NO_CONTENT.into_response(),
        }
    }
}

/// `GET /m/api/chat/history?before=&limit=&q=`: a page of the conversation, in small bites.
pub async fn chat_history(State(app): State<Shared>, Query(mut q): Query<HistoryQuery>) -> Response {
    q.limit = Some(q.limit.unwrap_or(3).clamp(1, HISTORY_MAX));
    if q.q.as_ref().is_some_and(|t| t.chars().count() > SEARCH_MAX) {
        return error(StatusCode::BAD_REQUEST, "the search is too long");
    }
    server::api_chat_history(State(app), Query(q)).await
}

#[derive(Deserialize)]
pub struct ChatIn {
    #[serde(default)]
    text: String,
}

/// `POST /m/api/chat {text}`: a message to the Coordinator. Text only (pictures are for a later version).
pub async fn chat(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<ChatIn>) -> Response {
    server::api_chat(State(app), headers, Json(ChatBody { text: body.text, attachments: Vec::new() })).await
}

#[derive(Deserialize)]
pub struct ActionIn {
    action: String,
    id: Option<String>,
    text: Option<String>,
    letter: Option<String>,
    confirm: Option<bool>,
}

/// `POST /m/api/action {action, id, text?, letter?, confirm?}`: one of `ACTIONS`, nothing else.
pub async fn action(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<ActionIn>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if !ACTIONS.contains(&body.action.as_str()) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "this action is not for the phone", "action": body.action}))).into_response();
    }
    let what = match body.action.as_str() {
        "approve" => "одобрено",
        "retry" => "повтор",
        "tell" => "сообщение агенту",
        _ => "выбрана попытка",
    };
    let id = body.id.clone().unwrap_or_default();
    // built here, from the four fields: the phone cannot reach the others
    let act = ActionBody {
        action: body.action,
        id: body.id,
        text: body.text,
        letter: body.letter,
        confirm: body.confirm,
        via: Some(VIA.to_string()),
        ..Default::default()
    };
    let res = server::perform_action(&app, act, false).await;
    if res.status().is_success() {
        let tag = app.snapshot.read().await.tasks.iter().find(|t| t.id == id).map(|t| t.tag.clone()).unwrap_or(id.clone());
        app::journal_note(&app, &id, "tunnel", &format!("{what} {VIA}: {tag}")).await;
    }
    res
}

/// `POST /m/api/menu/answer`: a choice in an agent's menu or a permission, by the same checks as the planner's page.
pub async fn menu_answer(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<crate::menu_api::AnswerBody>) -> Response {
    crate::menu_api::answer(&app, &headers, body, Some(VIA)).await
}

/// `GET /m/api/coordinator/screen?target=&role=`: what an agent shows in its terminal (the colours stay at home).
pub async fn screen(State(app): State<Shared>, Query(q): Query<ScreenQuery>) -> Response {
    let (parts, body) = server::api_coordinator_screen(State(app), Query(q)).await.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 1 << 20).await else {
        return error(StatusCode::BAD_GATEWAY, "the screen could not be read");
    };
    let mut v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("ansi");
    }
    (parts.status, Json(v)).into_response()
}

#[derive(Deserialize)]
pub struct DetailsQuery {
    id: String,
}

/// Keys that name a place on this machine, wherever they stand in what the board says about a task.
const PLACES: [&str; 8] = ["worktree", "session", "sessionFile", "sessionPath", "cwd", "reportsDir", "reports_dir", "transcript"];

fn without_places(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(o.iter().filter(|(k, _)| !PLACES.contains(&k.as_str())).map(|(k, v)| (k.clone(), without_places(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(without_places).collect()),
        other => other.clone(),
    }
}

/// What the card of a task shows beyond the snapshot: the review, the plan, the PR, the attempts, why it failed.
/// Not where the work is on disk, and not the address of the review report (it is on this machine's loopback).
pub fn details_of(d: &crate::board::Details) -> Value {
    let mut task = serde_json::Map::new();
    for key in ["id", "title", "stage", "state", "msg", "description", "kind"] {
        if let Some(v) = d.task.get(key) {
            task.insert(key.to_string(), v.clone());
        }
    }
    json!({
        "id": d.id,
        "task": task,
        "failure": without_places(&d.failure),
        "review": without_places(&d.review),
        "reviews": without_places(&d.reviews),
        "agent": d.agent,
        "agent_model": d.agent_model,
        "branch": d.branch,
        "after": without_places(&d.after),
        "limits": without_places(&d.limits),
        "plan": without_places(&d.plan),
        "pr": without_places(&d.pr),
        "attempts": without_places(&d.attempts),
        "comparison": without_places(&d.comparison),
        "picked": without_places(&d.picked),
    })
}

/// `GET /m/api/details?id=`
pub async fn details(State(app): State<Shared>, Query(q): Query<DetailsQuery>) -> Response {
    match app::details(&app, &q.id).await {
        Ok(d) => Json(json!({"ok": true, "details": details_of(&d)})).into_response(),
        Err(e) if e == "not a task id" => error(StatusCode::BAD_REQUEST, e),
        Err(_) => error(StatusCode::BAD_GATEWAY, "the board does not answer"),
    }
}

/// `GET /m/api/session`: who this is and until when, for the page's header and its settings.
pub async fn session(State(app): State<Shared>, Extension(class): Extension<Class>, headers: HeaderMap) -> Response {
    let computer = app::hostname();
    if class != Class::Tunnel {
        return Json(json!({"kind": "local", "computer": computer})).into_response();
    }
    let token = crate::auth::cookie_value(&headers, crate::devices::COOKIE).unwrap_or_default();
    match app.devices.session_at(now_ms(), &token) {
        Some(s) => Json(json!({
            "kind": "device",
            "computer": computer,
            "label": s.label,
            "scope": s.scope,
            "since_ms": s.since_ms,
            "ends_ms": s.ends_ms,
            "gate_since_ms": app.tunnel.since_ms(),
            "gate_closes_ms": app.devices.idle_closes_at(),
        }))
        .into_response(),
        None => error(StatusCode::UNAUTHORIZED, "sign in"),
    }
}

/// `POST /m/api/logout`: this device leaves; its cookie is no good from now on.
pub async fn logout(State(app): State<Shared>, Extension(class): Extension<Class>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if class != Class::Tunnel {
        return error(StatusCode::BAD_REQUEST, "this is not a device of the gate");
    }
    if let Some(t) = crate::auth::cookie_value(&headers, crate::devices::COOKIE) {
        app.devices.leave_at(now_ms(), &t);
    }
    crate::devices::flush(&app).await;
    crate::devices::with_cookie(Json(json!({"ok": true})).into_response(), crate::devices::cookie_header("", 0))
}

/// `POST /m/api/tunnel/close`: the gate closes for everybody (it can be opened again only on the computer). The
/// answer goes out first: the connection it travels on is the one that is about to go.
pub async fn tunnel_close(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if !matches!(app.tunnel.state(), crate::tunnel::Phase::Open | crate::tunnel::Phase::Reconnecting) {
        return error(StatusCode::CONFLICT, "the gate is not open");
    }
    let app2 = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        app::journal_note(&app2, "tunnel", "tunnel", &format!("шлюз закрыт {VIA}")).await;
        crate::tunnel::close(&app2, app2.tunnel.timing().term_grace).await;
        crate::devices::flush(&app2).await;
    });
    Json(json!({"ok": true})).into_response()
}

#[cfg(test)]
#[path = "mobile_tests.rs"]
mod tests;
