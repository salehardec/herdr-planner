//! The page's newer endpoints: the event stream, search, models, translation,
//! sub-agent cards and the closing note of a task.

use crate::app::{self, Shared};
use crate::board;
use crate::config::now_ms;
use crate::server::{clip, client_is_local, error, refuse_remote, same_origin};
use crate::{models, search, subagents, taskmeta, translate};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// `GET /api/events`: the first event is the whole snapshot, then every change.
pub async fn events(State(app): State<Shared>) -> Response {
    // Subscribe first: nothing published between the two steps is lost.
    let mut rx = app.bus.subscribe();
    let first = serde_json::to_string(&*app.snapshot.read().await).unwrap_or_default();
    let load = serde_json::to_string(&crate::load::current(&app).await).unwrap_or_default();
    app.load_poke.notify_one();
    let app2 = app.clone();
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(Event::default().event("state").data(first));
        yield Ok(Event::default().event("load").data(load));
        loop {
            match rx.recv().await {
                Ok(m) => yield Ok(Event::default().event(m.event).data(m.data.clone())),
                Err(RecvError::Lagged(_)) => {
                    // Too slow a client: give it the current state instead of the missed ones.
                    let now = serde_json::to_string(&*app2.snapshot.read().await).unwrap_or_default();
                    yield Ok(Event::default().event("state").data(now));
                }
                Err(RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
        .into_response()
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    limit: Option<usize>,
}

/// `GET /api/search?q=`: the full-text part of the world search (descriptions, old chat, memory).
pub async fn search_api(State(app): State<Shared>, Query(q): Query<SearchQuery>) -> Response {
    let text = q.q.unwrap_or_default();
    let limit = q.limit.unwrap_or(20).clamp(1, 50);
    if text.trim().is_empty() {
        return Json(json!({"ok": true, "hits": []})).into_response();
    }
    let db = app.db.clone();
    match tokio::task::spawn_blocking(move || search::query(&db, &text, limit)).await {
        Ok(hits) => Json(json!({"ok": true, "hits": hits})).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct ModelsQuery {
    program: String,
}

/// `GET /api/models?program=`: names for «agent/model» (the board's `models <prog>`), in the background.
pub async fn models_api(State(app): State<Shared>, Query(q): Query<ModelsQuery>) -> Response {
    if !models::valid_program(&q.program) {
        return error(StatusCode::BAD_REQUEST, "unknown program");
    }
    let m = models::get(&app.cfg, &q.program).await;
    Json(json!({"ok": true, "program": q.program, "names": m.names, "note": m.note, "loading": m.loading})).into_response()
}

#[derive(Deserialize)]
pub struct TranslateBody {
    text: String,
    /// `ru` (default) or `en`: the language of the page.
    to: Option<String>,
}

/// `POST /api/translate`: the text in Russian, by a model on this machine.
pub async fn translate_api(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<TranslateBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let to = body.to.as_deref().filter(|t| *t == "en").unwrap_or("ru");
    // What was translated once stays translated, across restarts.
    let key = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(app.cfg.translator.as_bytes());
        h.update(to.as_bytes());
        h.update(body.text.as_bytes());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    if let Some(hit) = app.db.translation_get(&key) {
        return Json(json!({"ok": true, "text": hit, "cached": true})).into_response();
    }
    match translate::translate(&app.cfg, &body.text, to).await {
        Ok(text) => {
            app.db.translation_put(&key, &text, now_ms());
            Json(json!({"ok": true, "text": text})).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"ok": false, "error": e}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SubagentQuery {
    task: String,
    id: String,
}

/// `GET /api/subagent?task=&id=`: one sub-agent with its whole result. `task=quasar` is the Coordinator's.
pub async fn subagent_api(State(app): State<Shared>, Query(q): Query<SubagentQuery>) -> Response {
    if q.id.len() > 200 {
        return error(StatusCode::BAD_REQUEST, "bad id");
    }
    let snap = app.snapshot.read().await.clone();
    if q.task == "quasar" {
        return match snap.coordinator.subagents.iter().find(|s| s.id == q.id) {
            Some(s) => Json(json!({"ok": true, "subagent": s, "parent": {"kind": "quasar", "app": snap.coordinator.program}})).into_response(),
            None => error(StatusCode::NOT_FOUND, "no such sub-agent"),
        };
    }
    if !board::valid_id(&q.task) || !q.task.contains('/') {
        return error(StatusCode::BAD_REQUEST, "bad task id");
    }
    let Some(t) = snap.tasks.iter().find(|t| t.id == q.task).cloned() else {
        return error(StatusCode::NOT_FOUND, "no such task");
    };
    let id = q.id.clone();
    let root = taskmeta::repos(&app.cfg.data_dir).into_iter().find(|r| r.name == t.repo).map(|r| r.root).unwrap_or_default();
    let found = tokio::task::spawn_blocking(move || {
        let agent_name = format!("sw-{}-{}", t.repo, t.n);
        let query = subagents::Query {
            app: &t.app,
            session: &t.session,
            cwd: if t.worktree.is_empty() { &root } else { &t.worktree },
            agent_name: &agent_name,
            parent_active: matches!(t.status, board::Status::Working | board::Status::Review | board::Status::Plan | board::Status::Waiting)
                && t.exited_at == 0,
            parent_exited_at_ms: t.exited_at,
        };
        (subagents::card(&query, &id, now_ms()), t.app.clone(), t.model.clone())
    })
    .await;
    match found {
        Ok((Some(s), program, model)) => Json(json!({"ok": true, "subagent": s, "parent": {"kind": "task", "app": program, "model": model}})).into_response(),
        Ok((None, _, _)) => error(StatusCode::NOT_FOUND, "the sub-agent is gone"),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct ClosingQuery {
    id: String,
}

/// `GET /api/task/closing?id=`: what the agent reported when it finished (the task's `closing.md`).
pub async fn closing_api(State(app): State<Shared>, Query(q): Query<ClosingQuery>) -> Response {
    if !board::valid_id(&q.id) || !q.id.contains('/') {
        return error(StatusCode::BAD_REQUEST, "bad task id");
    }
    let (repo, n) = (board::repo_of(&q.id).to_string(), board::number_of(&q.id));
    let dir = app.cfg.data_dir.clone();
    let r = tokio::task::spawn_blocking(move || taskmeta::read_closing(&dir, &repo, n, taskmeta::CLOSING_MAX_BYTES)).await;
    match r {
        Ok(Some(c)) => Json(json!({"ok": true, "text": c.text, "truncated": c.truncated})).into_response(),
        Ok(None) => Json(json!({"ok": true, "text": "", "truncated": false})).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/* ───────── universes (projects) ───────── */

#[derive(Deserialize)]
pub struct FsQuery {
    path: String,
}

#[cfg(test)]
impl FsQuery {
    pub(crate) fn for_path(path: &str) -> Self {
        Self { path: path.into() }
    }
}

/// `GET /api/fs/complete?path=`: folders for the path field.
pub async fn fs_complete(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response {
    if let Some(refused) = refuse_remote(addr, &headers) {
        return refused;
    }
    if q.path.contains('\0') || q.path.len() > 1000 {
        return error(StatusCode::BAD_REQUEST, "bad path");
    }
    let roots: Vec<String> = taskmeta::repos(&app.cfg.data_dir).into_iter().map(|r| r.root).collect();
    let path = q.path.clone();
    match tokio::task::spawn_blocking(move || crate::universe::complete_path(&path, &roots, 24)).await {
        Ok(list) => Json(json!({"ok": true, "entries": list})).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct InspectBody {
    input: String,
}

/// `POST /api/universe/inspect`: what an address or a folder is, and whether the operator may write to it.
pub async fn universe_inspect(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<InspectBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    match crate::universe::inspect(&app.cfg, body.input.trim()).await {
        Ok(i) => Json(json!({"ok": true, "inspect": i})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct AddUniverseBody {
    #[serde(flatten)]
    req: crate::universe::AddRequest,
    confirm: Option<bool>,
}

/// `POST /api/universe/add`: clones and registers in the background; the steps arrive as `progress` events.
pub async fn universe_add(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<AddUniverseBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return error(StatusCode::BAD_REQUEST, "adding a universe needs an explicit confirmation");
    }
    if !client_is_local(addr, &headers) && app.cfg.is_loopback() {
        return error(StatusCode::FORBIDDEN, "only from this machine");
    }
    let job = format!("u{}", now_ms());
    let app2 = app.clone();
    let req = body.req;
    let job2 = job.clone();
    tokio::spawn(async move {
        let jid = job2.clone();
        let jobs = app2.clone();
        let progress = move |steps: &[crate::universe::Step]| {
            let payload = json!({"job": jid, "steps": steps, "done": false});
            jobs.add_job.note(&payload);
            jobs.bus.send("progress", payload.to_string());
        };
        let payload = match crate::universe::add(&app2.cfg, &req, &progress).await {
            Ok(r) => json!({"job": job2, "done": true, "ok": true, "result": r}),
            Err(e) => json!({"job": job2, "done": true, "ok": false, "error": e}),
        };
        app2.add_job.note(&payload);
        app2.bus.send("progress", payload.to_string());
        app::refresh(&app2).await;
    });
    Json(json!({"ok": true, "job": job})).into_response()
}

/// The last word of the newest «add a universe» job, kept for the dialog.
///
/// The page learns a job's steps and its end from `progress` events, and a stream that falls behind is given the
/// current state instead of the missed events (see [`events`]): a lost `done` would leave the dialog waiting for
/// good while the universe is already on the board. So the last payload is kept here, and the dialog can ask
/// ([`universe_job`]) while it waits.
#[derive(Default)]
pub struct AddJob(std::sync::Mutex<Option<(String, serde_json::Value)>>);

impl AddJob {
    /// Remembers a payload of the job it names; anything without a job id is not about a job.
    fn note(&self, payload: &serde_json::Value) {
        let Some(id) = payload.get("job").and_then(serde_json::Value::as_str) else { return };
        let id = id.to_string();
        if let Ok(mut last) = self.0.lock() {
            *last = Some((id, payload.clone()));
        }
    }

    fn get(&self, job: &str) -> Option<serde_json::Value> {
        let last = self.0.lock().ok()?;
        match last.as_ref() {
            Some((id, payload)) if id == job => Some(payload.clone()),
            _ => None,
        }
    }
}

/// A job id: `u` and the milliseconds it was made at (see [`universe_add`]).
fn valid_job(id: &str) -> bool {
    id.len() > 1 && id.len() <= 24 && id.starts_with('u') && id[1..].bytes().all(|b| b.is_ascii_digit())
}

#[derive(Deserialize)]
pub struct JobQuery {
    id: String,
}

/// `GET /api/universe/job?id=`: what that «add a universe» job said last — its steps, or how it ended. 404 while
/// it has said nothing (the dialog asks again).
pub async fn universe_job(State(app): State<Shared>, Query(q): Query<JobQuery>) -> Response {
    if !valid_job(&q.id) {
        return error(StatusCode::BAD_REQUEST, "bad job id");
    }
    match app.add_job.get(&q.id) {
        Some(payload) => Json(payload).into_response(),
        None => error(StatusCode::NOT_FOUND, "this job has said nothing yet"),
    }
}

#[derive(Deserialize)]
pub struct InfoQuery {
    repo: String,
}

/// `GET /api/universe/info?repo=`: the fork facts of a universe on this machine (read-only git, board settings).
pub async fn universe_info(State(app): State<Shared>, Query(q): Query<InfoQuery>) -> Response {
    if !taskmeta::valid_repo(&q.repo) {
        return error(StatusCode::BAD_REQUEST, "bad universe name");
    }
    let Some(info) = taskmeta::repos(&app.cfg.data_dir).into_iter().find(|r| r.name == q.repo) else {
        return error(StatusCode::NOT_FOUND, "no such universe");
    };
    let root = info.root.clone();
    let git = |args: Vec<String>| {
        let root = root.clone();
        async move {
            let out = tokio::process::Command::new("git")
                .arg("-C").arg(&root).args(args)
                .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
                .output().await.ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
        }
    };
    let upstream = git(vec!["remote".into(), "get-url".into(), "upstream".into()]).await;
    let origin = git(vec!["remote".into(), "get-url".into(), "origin".into()]).await;
    let behind = if upstream.is_some() { crate::universe::upstream_lag(&root).await } else { None };
    let settings = app::run_board(&app.cfg, &["show", "--repo", &q.repo, "--json"], Duration::from_secs(10))
        .await
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or(json!({}));
    let host = origin.as_deref().and_then(|o| crate::universe::parse_address(o).ok()).map(|a| json!({"host": a.host, "kind": a.kind, "path": a.path()}));
    Json(json!({
        "ok": true, "repo": q.repo, "root": root, "origin": origin, "upstream": upstream, "upstream_behind": behind,
        "settings": settings, "host": host,
    })).into_response()
}

#[derive(Deserialize)]
pub struct ForkBody {
    repo: String,
    op: String,
    title: Option<String>,
    body: Option<String>,
    confirm: Option<bool>,
}

/// `POST /api/universe/fork`: «подтянуть» (a board task), «PR/MR в оригинал» and «превратить в форк».
pub async fn universe_fork(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<ForkBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if b.confirm != Some(true) {
        return error(StatusCode::BAD_REQUEST, "this needs an explicit confirmation");
    }
    let Some(info) = taskmeta::repos(&app.cfg.data_dir).into_iter().find(|r| r.name == b.repo) else {
        return error(StatusCode::NOT_FOUND, "no such universe");
    };
    match b.op.as_str() {
        "pull" => {
            let text = "Влить свежий upstream/main в форк (git fetch upstream; git merge upstream/main), проверить и запушить".to_string();
            match app::run_board(&app.cfg, &["add", &text, "--repo", &b.repo], Duration::from_secs(60)).await {
                Ok(out) => { app::refresh(&app).await; Json(json!({"ok": true, "out": clip(out)})).into_response() }
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            }
        }
        "pr" => {
            let title = b.title.clone().unwrap_or_default();
            if title.trim().is_empty() || title.len() > 200 {
                return error(StatusCode::BAD_REQUEST, "give a title for the request");
            }
            match crate::universe::open_pull_request(&app.cfg, &info.root, title.trim(), b.body.as_deref().unwrap_or("")).await {
                Ok(url) => Json(json!({"ok": true, "url": url})).into_response(),
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            }
        }
        "fork" => match crate::universe::make_fork(&info.root).await {
            Ok(out) => { app::update_lag(&app).await; Json(json!({"ok": true, "out": clip(out)})).into_response() }
            Err(e) => error(StatusCode::BAD_GATEWAY, e),
        },
        _ => error(StatusCode::BAD_REQUEST, "unknown operation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_add_job_keeps_its_own_last_word_only() {
        let job = AddJob::default();
        assert!(job.get("u1").is_none());
        job.note(&json!({"job": "u1", "steps": [{"id": "check"}], "done": false}));
        assert_eq!(job.get("u1").unwrap()["steps"][0]["id"], "check");
        // A dialog that asks about another job gets nothing: nobody else's steps.
        assert!(job.get("u2").is_none());
        // The end of the job replaces its steps.
        job.note(&json!({"job": "u1", "done": true, "ok": true, "result": {"name": "newrepo"}}));
        assert_eq!(job.get("u1").unwrap()["result"]["name"], "newrepo");
        assert!(job.get("u1").unwrap().get("steps").is_none());
        // A payload that names no job is nobody's and does not erase the last one.
        job.note(&json!({"done": true}));
        assert_eq!(job.get("u1").unwrap()["result"]["name"], "newrepo");
    }

    #[test]
    fn only_an_add_job_id_is_asked_about() {
        assert!(valid_job("u1792694972479"));
        assert!(!valid_job("u"));
        assert!(!valid_job("1792694972479"));
        assert!(!valid_job("u12a"));
        assert!(!valid_job("u1234567890123456789012345"));
    }
}
