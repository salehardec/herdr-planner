//! HTTP: the embedded page, the JSON API and the SSE event stream.

use crate::actions::{self, Failure, TaskReq};
use crate::app::{Event, Shared};
use crate::model::{self, summarize, OTHER};
use crate::suggest;
use axum::extract::{Query, Request, State};
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

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/util.js", get(util_js))
        .route("/card.js", get(card_js))
        .route("/settings.js", get(settings_js))
        .route("/api/snapshot", get(snapshot))
        .route("/api/summary", get(summary))
        .route("/api/overview", get(overview))
        .route("/api/theme", get(theme))
        .route("/api/events", get(events))
        .route("/api/refresh", post(refresh))
        .route("/api/groups", post(groups))
        .route("/api/settings", post(settings))
        .route("/api/action", post(action))
        .route("/api/task", get(task_details))
        .route("/api/task/action", post(task_action))
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
/// (the Origin header names it). Both are refused; the page can approve
/// merges and start agents on the board, so this matters.
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

async fn snapshot(State(app): State<Shared>) -> Json<Value> {
    Json(serde_json::to_value(app.full()).unwrap_or(Value::Null))
}

/// Attention totals for the Omarchy bar icon: small, polled every few seconds.
async fn summary(State(app): State<Shared>) -> Json<Value> {
    let (s, updated, loading, error) = {
        let snap = app.snapshot.read().unwrap();
        (
            summarize(&snap.projects),
            snap.updated,
            snap.loading,
            snap.error.is_some(),
        )
    };
    Json(json!({
        "waiting": s.waiting,
        "needs": s.needs,
        "ready": s.ready,
        "failed": s.failed,
        "waiting_hint": s.waiting_hint,
        "attention_since": s.attention_since,
        "review": s.review,
        "working": s.working,
        "queued": s.queued,
        "projects": s.projects,
        "updated": updated,
        "loading": loading,
        "error": error,
        "version": crate::app::VERSION,
    }))
}

/// The bar popup's list: attention projects first, then the planner's groups.
async fn overview(State(app): State<Shared>) -> Json<Value> {
    let projects = app.snapshot.read().unwrap().projects.clone();
    Json(serde_json::to_value(model::overview(&projects)).unwrap_or(Value::Null))
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
            return failure(Failure(400, "неизвестная настройка или значение".into()));
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
    #[serde(default)]
    slug: String,
}

/// `unfocus`: the herdr sidebar lists every agent again (see `actions::unfocus`).
/// `show` (slug): the bar popup asks the page to open that project's card.
async fn action(State(app): State<Shared>, Json(a): Json<Action>) -> Response {
    match a.kind.as_str() {
        "unfocus" => match actions::unfocus(&app).await {
            Ok(()) => Json(json!({"ok": true})).into_response(),
            Err(e) => failure(Failure(502, e)),
        },
        "show" => {
            let known = app.snapshot.read().unwrap().projects.iter().any(|p| p.slug == a.slug);
            if !known {
                return failure(Failure(404, "такого проекта нет на доске".into()));
            }
            let show = crate::app::Show { slug: a.slug, at: crate::app::now_ms() };
            *app.show.lock().unwrap() = Some(show.clone());
            app.emit("show", &show);
            Json(json!({"ok": true})).into_response()
        }
        _ => failure(Failure(400, "неизвестное действие".into())),
    }
}

fn failure(f: Failure) -> Response {
    let status = StatusCode::from_u16(f.0).unwrap_or(StatusCode::BAD_REQUEST);
    (status, Json(json!({"error": f.1}))).into_response()
}

#[derive(Deserialize)]
struct IdQuery {
    #[serde(default)]
    id: String,
}

/// A task's details from the board: the stage, the message, the review, the branch and pull request.
async fn task_details(State(app): State<Shared>, Query(q): Query<IdQuery>) -> Response {
    match actions::details(&app, &q.id).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => failure(f),
    }
}

/// A command of the board for one task: `approve`, `review`, `retry`, `delegate`, `stop`, `tell`.
async fn task_action(State(app): State<Shared>, Json(r): Json<TaskReq>) -> Response {
    match actions::run_task_action(&app, &r).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => failure(f),
    }
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
