//! HTTP: the embedded page, the JSON API and the SSE event stream.

use crate::app::{Event, Shared};
use crate::model::OTHER;
use crate::poller;
use crate::sources::{runner, tasks, threads};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
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

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/api/snapshot", get(snapshot))
        .route("/api/theme", get(theme))
        .route("/api/events", get(events))
        .route("/api/refresh", post(refresh))
        .route("/api/groups", post(groups))
        .route("/api/action", post(action))
        .route("/api/project/{slug}", get(project))
        .with_state(app)
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
}

/// `assign` (slug, name: "" = no group), `add` (name), `fold` (name, folded),
/// `toggle` (on). Every op saves and re-broadcasts the snapshot.
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
            other => Err(format!("неизвестная операция «{other}»")),
        }
    };
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

/// The project card's data (stage 2 draws it): the project, its tasks and
/// its threads grouped as herdr groups them.
async fn project(State(app): State<Shared>, Path(slug): Path<String>) -> Response {
    if slug.is_empty() || slug.contains('/') || slug.starts_with('.') || slug.starts_with('-') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "нет проекта"})),
        )
            .into_response();
    }
    let known = app
        .snapshot
        .read()
        .unwrap()
        .projects
        .iter()
        .find(|p| p.slug == slug)
        .cloned();
    let dir = app.config.project_dir(&slug);
    let project = known.unwrap_or_else(|| {
        let mut p = crate::model::Project {
            slug: slug.clone(),
            name: slug.clone(),
            status: "active".into(),
            ..Default::default()
        };
        poller::enrich(&mut p, &dir);
        p
    });
    let task_text = tokio::fs::read_to_string(dir.join("TASKS.md")).await.ok();
    let task_list = task_text
        .as_deref()
        .map(tasks::parse_tasks)
        .unwrap_or_default();
    let argv = runner::argv([
        app.config.herdr_projects.as_str(),
        "--root",
        &app.config.root.display().to_string(),
        "thread",
        "list",
        &slug,
        "--json",
    ]);
    let (groups, error) = match runner::run(&argv, None, Duration::from_secs(20)).await {
        Ok(out) if out.ok() => match threads::parse_threads(&out.stdout) {
            Ok(g) => (g, None),
            Err(e) => (vec![], Some(e)),
        },
        Ok(out) => (vec![], Some(out.error())),
        Err(e) => (vec![], Some(e)),
    };
    Json(json!({
        "project": project,
        "tasks": task_list,
        "tasks_read": task_text.is_some(),
        "threads": groups,
        "error": error,
    }))
    .into_response()
}
