//! The page's newer endpoints: the event stream, search, models, translation,
//! sub-agent cards, the closing note of a task and the other machines.

use crate::app::{self, Shared};
use crate::board;
use crate::config::now_ms;
use crate::server::{clip, client_is_local, error, same_origin};
use crate::{machines, models, search, subagents, taskmeta, translate};
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
    let app2 = app.clone();
    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(Event::default().event("state").data(first));
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

/// `GET /api/search?q=`: the full-text part of the world search (descriptions, old chat, other machines).
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

#[derive(Deserialize)]
pub struct AskBody {
    label: String,
}

/// `POST /api/galaxy/ask`: connect to a far galaxy now (a fresh read over SSH).
pub async fn galaxy_ask(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<AskBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if !machines::valid_label(&body.label) {
        return error(StatusCode::BAD_REQUEST, "bad machine label");
    }
    match ask_galaxy(&app, &body.label).await {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(why) => Json(json!({"ok": false, "why": why})).into_response(),
    }
}

/// Reads a far galaxy afresh and stores the answer; the page hears of it on the event stream.
pub async fn ask_galaxy(app: &Shared, label: &str) -> Result<(), machines::Why> {
    {
        let mut c = app.connecting.lock().await;
        if !c.iter().any(|l| l == label) {
            c.push(label.to_string());
        }
    }
    app::rebuild_extras(app).await;
    app::publish(app).await;
    let result = machines::ask(&app.cfg, label).await;
    app.connecting.lock().await.retain(|l| l != label);
    let reply = match result {
        Ok(g) => {
            if let Ok(json) = serde_json::to_string(&g) {
                app.db.galaxy_put(&g.label, &json, g.ok, "");
            }
            let mut list = app.galaxies.lock().await;
            list.retain(|x| x.label != g.label);
            list.push(g);
            Ok(())
        }
        Err(why) => Err(why),
    };
    app::rebuild_extras(app).await;
    app::publish(app).await;
    reply
}

#[derive(Deserialize)]
pub struct AddGalaxyBody {
    target: String,
    label: Option<String>,
    confirm: Option<bool>,
}

/// `POST /api/galaxy/add`: opens a terminal with `herdr machine add <target> --label <label>` so the human
/// confirms there. Only for someone at this machine.
pub async fn galaxy_add(
    State(_app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<AddGalaxyBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if !client_is_local(addr, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Машину добавляют только за компьютером с доской", "remote": true})),
        )
            .into_response();
    }
    if body.confirm != Some(true) {
        return error(StatusCode::BAD_REQUEST, "adding a machine needs an explicit confirmation");
    }
    let label = body.label.clone().filter(|l| !l.trim().is_empty()).unwrap_or_else(|| {
        body.target.rsplit('@').next().unwrap_or(&body.target).split(':').next().unwrap_or("").to_string()
    });
    let argv = match machines::machine_add_command(&body.target, &label) {
        Ok(a) => a,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let herdr = std::env::var("HERDR_BIN").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "herdr".to_string());
    let mut cmd = if cfg!(target_os = "macos") {
        let line = std::iter::once(herdr.clone())
            .chain(argv.iter().skip(1).cloned())
            .collect::<Vec<_>>()
            .join(" ");
        let mut c = tokio::process::Command::new("osascript");
        c.args(["-e", &format!("tell application \"Terminal\" to do script \"{line}\"")]);
        c
    } else {
        let mut c = tokio::process::Command::new(std::env::var("HERDR_PLANNER_TERMINAL").unwrap_or_else(|_| "xdg-terminal-exec".to_string()));
        c.arg(&herdr).args(argv.iter().skip(1));
        c
    };
    match cmd.spawn() {
        Ok(_) => Json(json!({"ok": true, "label": label, "command": format!("herdr {}", argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" "))})).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, clip(format!("не удалось открыть терминал: {e}"))),
    }
}

/* ───────── universes (projects) ───────── */

#[derive(Deserialize)]
pub struct FsQuery {
    path: String,
    galaxy: Option<String>,
}

/// `GET /api/fs/complete?path=&galaxy=`: folders for the path field, on this machine or a far one.
pub async fn fs_complete(State(app): State<Shared>, Query(q): Query<FsQuery>) -> Response {
    if q.path.contains('\0') || q.path.len() > 1000 {
        return error(StatusCode::BAD_REQUEST, "bad path");
    }
    match q.galaxy.as_deref().filter(|g| !g.is_empty()) {
        Some(label) => {
            if !machines::valid_label(label) {
                return error(StatusCode::BAD_REQUEST, "bad machine label");
            }
            match crate::universe::complete_path_remote(&app.cfg, label, &q.path).await {
                Ok(list) => Json(json!({"ok": true, "entries": list})).into_response(),
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            }
        }
        None => {
            let roots: Vec<String> = taskmeta::repos(&app.cfg.data_dir).into_iter().map(|r| r.root).collect();
            let path = q.path.clone();
            match tokio::task::spawn_blocking(move || crate::universe::complete_path(&path, &roots, 24)).await {
                Ok(list) => Json(json!({"ok": true, "entries": list})).into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
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
    galaxy: Option<String>,
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
    let galaxy = body.galaxy.clone().filter(|g| !g.is_empty());
    if galaxy.is_none() && !client_is_local(addr, &headers) && app.cfg.is_loopback() {
        return error(StatusCode::FORBIDDEN, "only from this machine");
    }
    if let Some(g) = &galaxy {
        if !machines::valid_label(g) {
            return error(StatusCode::BAD_REQUEST, "bad machine label");
        }
    }
    let job = format!("u{}", now_ms());
    let app2 = app.clone();
    let req = body.req;
    let job2 = job.clone();
    tokio::spawn(async move {
        let bus = app2.bus.clone();
        let jid = job2.clone();
        let progress = move |steps: &[crate::universe::Step]| {
            bus.send("progress", json!({"job": jid, "steps": steps, "done": false}).to_string());
        };
        let result = match &galaxy {
            Some(label) => crate::universe::add_remote(&app2.cfg, label, &req, &progress).await,
            None => crate::universe::add(&app2.cfg, &req, &progress).await,
        };
        let payload = match result {
            Ok(r) => json!({"job": job2, "done": true, "ok": true, "result": r}),
            Err(e) => json!({"job": job2, "done": true, "ok": false, "error": e}),
        };
        app2.bus.send("progress", payload.to_string());
        app::refresh(&app2).await;
        if let Some(label) = &galaxy {
            let _ = ask_galaxy(&app2, label).await;
        }
    });
    Json(json!({"ok": true, "job": job})).into_response()
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
        "fork" => match crate::universe::make_fork(&app.cfg, &info.root).await {
            Ok(out) => { app::update_lag(&app).await; Json(json!({"ok": true, "out": clip(out)})).into_response() }
            Err(e) => error(StatusCode::BAD_GATEWAY, e),
        },
        _ => error(StatusCode::BAD_REQUEST, "unknown operation"),
    }
}

/* ───────── agent screen and files on another machine ───────── */

#[derive(Deserialize)]
pub struct FarFilesBody {
    galaxy: String,
    /// The task id on that machine (`repo/n`), for `yazi` and `screen`.
    task: Option<String>,
    /// `check`, `yazi`, `screen`, `install`, `install-terminal`.
    op: String,
    /// `binary`, `brew` or `pacman`.
    method: Option<String>,
    confirm: Option<bool>,
}

/// `POST /api/far/files`: the agent's screen and yazi of a task on another machine, in a terminal on this one.
pub async fn far_files(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(b): Json<FarFilesBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if !machines::valid_label(&b.galaxy) {
        return error(StatusCode::BAD_REQUEST, "bad machine label");
    }
    let galaxy = b.galaxy.clone();
    if b.op == "check" {
        return match crate::farfiles::check_yazi(&app.cfg, &galaxy).await {
            Ok(state) => Json(json!({"ok": true, "state": state})).into_response(),
            Err(e) => error(StatusCode::BAD_GATEWAY, e),
        };
    }
    // Everything else opens a terminal: only for someone at this machine.
    if !client_is_local(addr, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Терминал открывается только за компьютером с доской", "remote": true})),
        )
            .into_response();
    }
    let task = b.task.clone().unwrap_or_default();
    let found = {
        let list = app.galaxies.lock().await;
        list.iter()
            .find(|g| g.label == galaxy)
            .and_then(|g| g.tasks.iter().find(|t| t.id == task).cloned())
    };
    match b.op.as_str() {
        "yazi" => {
            let Some(t) = found.filter(|t| !t.worktree.is_empty()) else {
                return error(StatusCode::NOT_FOUND, "у задачи ещё нет папки на той машине");
            };
            match crate::farfiles::open_yazi(&app.cfg, &galaxy, &t.worktree).await {
                Ok(line) => Json(json!({"ok": true, "command": line, "path": t.worktree})).into_response(),
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            }
        }
        "screen" => {
            let Some(t) = found else {
                return error(StatusCode::NOT_FOUND, "no such task on that machine");
            };
            let name = format!("sw-{}-{}", t.repo, t.n);
            match crate::farfiles::open_agent_screen(&app.cfg, &galaxy, &name).await {
                Ok(line) => Json(json!({"ok": true, "command": line, "agent": name})).into_response(),
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            }
        }
        "install-terminal" => {
            let method = b.method.clone().unwrap_or_else(|| "pacman".into());
            match crate::farfiles::open_install_terminal(&app.cfg, &galaxy, &method).await {
                Ok(line) => Json(json!({"ok": true, "command": line})).into_response(),
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            }
        }
        "install" => {
            if b.confirm != Some(true) {
                return error(StatusCode::BAD_REQUEST, "installing needs an explicit confirmation");
            }
            let method = b.method.clone().unwrap_or_else(|| "binary".into());
            let job = format!("y{}", now_ms());
            let app2 = app.clone();
            let job2 = job.clone();
            tokio::spawn(async move {
                let bus = app2.bus.clone();
                let jid = job2.clone();
                let progress = move |line: &str| {
                    bus.send("progress", json!({"job": jid, "steps": [{"id": "install", "label": line, "state": "run", "detail": "", "percent": null}], "done": false}).to_string());
                };
                let payload = match crate::farfiles::install_yazi(&app2.cfg, &galaxy, &method, &progress).await {
                    Ok(path) => json!({"job": job2, "done": true, "ok": true, "result": {"path": path}}),
                    Err(e) => json!({"job": job2, "done": true, "ok": false, "error": e}),
                };
                app2.bus.send("progress", payload.to_string());
            });
            Json(json!({"ok": true, "job": job})).into_response()
        }
        _ => error(StatusCode::BAD_REQUEST, "unknown operation"),
    }
}
