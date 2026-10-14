//! HTTP: the embedded page, the JSON API and the SSE event stream.

use crate::actions::{self, Failure};
use crate::app::{Event, Shared};
use crate::cards;
use crate::model::OTHER;
use crate::newproject;
use crate::sources::runner;
use crate::{fresh, suggest};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{self, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

pub const INDEX_HTML: &str = include_str!("../web/index.html");
pub const APP_CSS: &str = include_str!("../web/app.css");
pub const APP_JS: &str = include_str!("../web/app.js");
pub const UTIL_JS: &str = include_str!("../web/util.js");
pub const CARD_JS: &str = include_str!("../web/card.js");
pub const SETTINGS_JS: &str = include_str!("../web/settings.js");
pub const NEW_JS: &str = include_str!("../web/new.js");

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/util.js", get(util_js))
        .route("/card.js", get(card_js))
        .route("/settings.js", get(settings_js))
        .route("/new.js", get(new_js))
        .route("/api/snapshot", get(snapshot))
        .route("/api/theme", get(theme))
        .route("/api/events", get(events))
        .route("/api/refresh", post(refresh))
        .route("/api/groups", post(groups))
        .route("/api/settings", post(settings))
        .route("/api/new/options", get(new_options))
        .route("/api/browse", get(browse))
        .route("/api/complete", get(complete))
        .route("/api/new", post(new_project))
        .route("/api/action", post(action))
        .route("/api/project/{slug}", get(project))
        .route("/api/project/{slug}/thread/{id}", get(thread_detail))
        .route("/api/project/{slug}/thread/{id}/worktree", post(worktree))
        .route("/api/project/{slug}/assignable", get(assignable))
        .route("/api/project/{slug}/solve", post(solve))
        .route("/api/project/{slug}/remove", post(remove))
        .route("/api/project/{slug}/ask", post(ask))
        .route("/api/project/{slug}/fresh-keep", post(fresh_keep))
        .layer(middleware::from_fn(local_only))
        .with_state(app)
}

/// The name part of `host[:port]`, `[::1]:port` included.
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("");
    }
    host.rsplit_once(':').map_or(host, |(name, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            name
        } else {
            host
        }
    })
}

/// Whether `host` (a Host header, or the host part of an Origin) is this machine.
pub fn host_is_local(host: &str) -> bool {
    matches!(host_name(host), "127.0.0.1" | "localhost" | "::1")
}

/// The server listens on 127.0.0.1 only, but a web page on another site can
/// still reach it through the browser: by a DNS name that is made to point
/// here (the Host header then names that site), or by a cross-site request
/// (the Origin header names it). Both are refused; the card can send prompts
/// to coordinators and start agents, so this matters.
async fn local_only(req: Request, next: Next) -> Response {
    let headers = req.headers();
    let host_ok = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(host_is_local);
    let origin_ok = headers
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .is_none_or(|o| {
            o.split_once("://")
                .is_some_and(|(_, host)| host_is_local(host))
        });
    if !host_ok || !origin_ok {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "только с этого компьютера"})),
        )
            .into_response();
    }
    next.run(req).await
}

fn asset(mime: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn index() -> Response {
    asset("text/html; charset=utf-8", INDEX_HTML)
}

async fn css() -> Response {
    asset("text/css; charset=utf-8", APP_CSS)
}

async fn js() -> Response {
    asset("text/javascript; charset=utf-8", APP_JS)
}

async fn util_js() -> Response {
    asset("text/javascript; charset=utf-8", UTIL_JS)
}

async fn card_js() -> Response {
    asset("text/javascript; charset=utf-8", CARD_JS)
}

async fn settings_js() -> Response {
    asset("text/javascript; charset=utf-8", SETTINGS_JS)
}

async fn new_js() -> Response {
    asset("text/javascript; charset=utf-8", NEW_JS)
}

async fn snapshot(State(app): State<Shared>) -> Json<Value> {
    Json(serde_json::to_value(app.full()).unwrap_or(Value::Null))
}

async fn theme(State(app): State<Shared>) -> Json<Value> {
    Json(serde_json::to_value(app.theme.read().unwrap().clone()).unwrap_or(Value::Null))
}

/// `snapshot`, `theme` and `workspace` events; the first two are sent at once
/// so a page needs no separate fetch.
async fn events(
    State(app): State<Shared>,
) -> Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
    let rx = app.events.subscribe();
    let first = vec![
        Event {
            kind: "theme",
            data: serde_json::to_string(&*app.theme.read().unwrap()).unwrap_or_default(),
        },
        Event {
            kind: "snapshot",
            data: serde_json::to_string(&app.full()).unwrap_or_default(),
        },
    ];
    let live = BroadcastStream::new(rx).filter_map(|r| async move { r.ok() });
    let all = stream::iter(first)
        .chain(live)
        .map(|e| Ok(sse::Event::default().event(e.kind).data(e.data)));
    Sse::new(all).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

async fn refresh(State(app): State<Shared>) -> Json<Value> {
    app.refresh_now.notify_one();
    Json(json!({"ok": true}))
}

#[derive(Deserialize)]
struct GroupOp {
    op: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    folded: Option<bool>,
    #[serde(default)]
    on: Option<bool>,
    /// `rename`: the new name; `delete`: the group the projects go to ("" = «Прочее»).
    #[serde(default)]
    to: String,
    /// `move`: places up (negative) or down.
    #[serde(default)]
    delta: i32,
}

/// `assign` (slug, name: "" = no group), `add` (name), `fold` (name, folded),
/// `toggle` (on), `rename` (name, to), `move` (name, delta), `delete` (name,
/// to), `resort`. Every op saves and re-broadcasts the snapshot.
async fn groups(State(app): State<Shared>, Json(op): Json<GroupOp>) -> Response {
    let result: Result<Value, String> = {
        let mut st = app.state.lock().unwrap();
        match op.op.as_str() {
            "assign" => {
                if op.slug.is_empty() {
                    Err("нет проекта".into())
                } else if op.name.is_empty() || op.name == OTHER {
                    st.groups.assign(&op.slug, "", false);
                    Ok(json!({"group": ""}))
                } else {
                    match st.groups.add(&op.name) {
                        Ok(name) => {
                            st.groups.assign(&op.slug, &name, false);
                            Ok(json!({"group": name}))
                        }
                        Err(e) => Err(e),
                    }
                }
            }
            "add" => st.groups.add(&op.name).map(|n| json!({"group": n})),
            "fold" => {
                st.groups.fold(&op.name, op.folded);
                Ok(json!({"collapsed": st.groups.is_collapsed(&op.name)}))
            }
            "toggle" => {
                match op.on {
                    Some(v) => st.groups.on = Some(v),
                    None => st.groups.toggle_grouping(),
                }
                Ok(json!({"on": st.groups.grouping_on()}))
            }
            "rename" => st
                .groups
                .rename(&op.name, &op.to)
                .map(|n| json!({"group": n})),
            "move" => {
                if st.groups.move_group(&op.name, op.delta) {
                    Ok(json!({"ok": true}))
                } else {
                    Err("дальше некуда".into())
                }
            }
            "delete" => {
                if st.groups.delete(&op.name, Some(op.to.as_str())) {
                    Ok(json!({"ok": true}))
                } else {
                    Err("нет такой группы".into())
                }
            }
            "resort" => Ok(json!({"ok": true})),
            other => Err(format!("неизвестная операция «{other}»")),
        }
    };
    if op.op == "resort" {
        // The state lock is gone: the suggestion takes it itself.
        suggest::resort(&app);
        return Json(json!({"ok": true})).into_response();
    }
    match result {
        Ok(v) => {
            app.save_state();
            app.emit_snapshot();
            Json(v).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    }
}

#[derive(Deserialize)]
struct SettingReq {
    key: String,
    value: String,
}

/// One setting changed. A value the setting does not have is refused;
/// turning the group suggestion on runs it for the projects that wait.
async fn settings(State(app): State<Shared>, Json(r): Json<SettingReq>) -> Response {
    let was = {
        let mut st = app.state.lock().unwrap();
        let was = st.settings.get(&r.key).to_string();
        if !st.settings.set(&r.key, &r.value) {
            drop(st);
            return failure((400, "неизвестная настройка или значение".into()));
        }
        was
    };
    app.save_state();
    app.emit_snapshot();
    if r.key == "autogroup" && was != r.value && r.value == "on" {
        suggest::tick(&app);
    }
    Json(json!({"ok": true, "settings": app.state.lock().unwrap().settings.clone()}))
        .into_response()
}

#[derive(Deserialize)]
struct Action {
    kind: String,
    slug: String,
}

/// `focus` runs `herdr-projects focus <slug>`, `open` runs `herdr-projects
/// open <slug>` (starts the coordinator in the project's workspace).
async fn action(State(app): State<Shared>, Json(a): Json<Action>) -> Response {
    let sub = match a.kind.as_str() {
        "focus" => "focus",
        "open" => "open",
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "неизвестное действие"})),
            )
                .into_response()
        }
    };
    if a.slug.is_empty() || a.slug.starts_with('-') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "нет проекта"})),
        )
            .into_response();
    }
    let argv = runner::argv([
        app.config.herdr_projects.as_str(),
        "--root",
        &app.config.root.display().to_string(),
        sub,
        &a.slug,
    ]);
    match runner::run(&argv, None, Duration::from_secs(30)).await {
        Ok(out) if out.ok() => {
            app.refresh_now.notify_one();
            Json(json!({"ok": true, "stdout": out.stdout.trim()})).into_response()
        }
        Ok(out) => (StatusCode::BAD_GATEWAY, Json(json!({"error": out.error()}))).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response(),
    }
}

fn failure(f: Failure) -> Response {
    let status = StatusCode::from_u16(f.0).unwrap_or(StatusCode::BAD_REQUEST);
    (status, Json(json!({"error": f.1}))).into_response()
}

fn bad_slug() -> Response {
    failure((400, "нет проекта".into()))
}

/// The project card's data: the project, its tasks, its threads (with git
/// lines and model badges), saved answers and «sent» marks.
async fn project(State(app): State<Shared>, Path(slug): Path<String>) -> Response {
    if !cards::valid_slug(&slug) {
        return bad_slug();
    }
    Json(cards::build(&app, &slug).await).into_response()
}

/// An opened thread: its git in full (diff stat, last commits, files not committed).
async fn thread_detail(
    State(app): State<Shared>,
    Path((slug, id)): Path<(String, String)>,
) -> Response {
    if !cards::valid_slug(&slug) {
        return bad_slug();
    }
    match cards::thread_detail(&app, &slug, &id).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => failure((404, e)),
    }
}

/// `w`: opens the thread's worktree in herdr.
async fn worktree(State(app): State<Shared>, Path((slug, id)): Path<(String, String)>) -> Response {
    if !cards::valid_slug(&slug) {
        return bad_slug();
    }
    match cards::open_worktree(&app, &slug, &id).await {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => failure((502, e)),
    }
}

/// The profiles a task of the project can be handed to.
async fn assignable(State(app): State<Shared>, Path(slug): Path<String>) -> Response {
    if !cards::valid_slug(&slug) {
        return bad_slug();
    }
    match cards::assignable(&app, &slug).await {
        Ok(a) => Json(json!({"profiles": a.profiles, "error": null})).into_response(),
        Err(e) => Json(json!({"profiles": [], "error": e})).into_response(),
    }
}

#[derive(Deserialize)]
struct TaskReq {
    #[serde(default)]
    task: String,
    /// «Убрать» on a freshness badge: the verdict goes into the request.
    #[serde(default)]
    fresh: bool,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    question: String,
}

fn done(r: Result<(), Failure>) -> Response {
    match r {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(f) => failure(f),
    }
}

/// «Решить»: the coordinator is asked to take the task (the project is
/// opened first when it is not).
async fn solve(
    State(app): State<Shared>,
    Path(slug): Path<String>,
    Json(r): Json<TaskReq>,
) -> Response {
    done(actions::solve(&app, &slug, &r.task, r.profile.as_deref()).await)
}

/// «Не решать»: the coordinator is asked to remove the line from TASKS.md.
async fn remove(
    State(app): State<Shared>,
    Path(slug): Path<String>,
    Json(r): Json<TaskReq>,
) -> Response {
    done(actions::remove(&app, &slug, &r.task, r.fresh).await)
}

/// «Оставить»: the freshness badge stays hidden until main gets a new commit.
async fn fresh_keep(
    State(app): State<Shared>,
    Path(slug): Path<String>,
    Json(r): Json<TaskReq>,
) -> Response {
    if !cards::valid_slug(&slug) {
        return bad_slug();
    }
    if fresh::keep(&app, &slug, &r.task) {
        app.emit("card", &json!({ "slug": slug }));
        Json(json!({"ok": true})).into_response()
    } else {
        failure((409, "проект ещё не проверялся".into()))
    }
}

/// The «Новый проект» form's lists: machines and profiles.
async fn new_options(State(app): State<Shared>) -> Json<Value> {
    Json(newproject::options(&app).await)
}

#[derive(Deserialize)]
struct PathQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    start: Option<String>,
}

/// The folder browser: `path` is the folder, or with `start` the field's text
/// from which a starting folder is found.
async fn browse(State(app): State<Shared>, Query(q): Query<PathQuery>) -> Response {
    let last_dir = app.state.lock().unwrap().last_dir.clone();
    let start = q.start.is_some();
    let result = tokio::task::spawn_blocking(move || {
        newproject::browse(&q.path, start, last_dir.as_deref())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => failure((400, e)),
    }
}

/// Tab in the «Папка» field.
async fn complete(Query(q): Query<PathQuery>) -> Response {
    let v = tokio::task::spawn_blocking(move || newproject::complete(&q.path))
        .await
        .unwrap_or(Value::Null);
    Json(v).into_response()
}

/// Creates the project (`herdr-projects new`); the page opens it afterwards.
async fn new_project(State(app): State<Shared>, Json(r): Json<newproject::NewReq>) -> Response {
    match newproject::create(&app, &r).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => failure(f),
    }
}

/// «Задать вопрос»: the cheap agent answers in the background.
async fn ask(
    State(app): State<Shared>,
    Path(slug): Path<String>,
    Json(r): Json<TaskReq>,
) -> Response {
    done(actions::ask(&app, &slug, &r.task, &r.question).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_machine_is_local() {
        assert!(host_is_local("127.0.0.1:7117"));
        assert!(host_is_local("localhost:7117"));
        assert!(host_is_local("localhost"));
        assert!(host_is_local("[::1]:7117"));
        assert!(!host_is_local("evil.example:7117"));
        assert!(!host_is_local("127.0.0.1.evil.example"));
        assert!(!host_is_local("evil.example"));
        assert!(!host_is_local(""));
        assert_eq!(host_name("a:b"), "a:b");
    }
}
