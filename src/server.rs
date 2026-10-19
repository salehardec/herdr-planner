//! HTTP: the page, the JSON API, login and the board commands behind it.

use crate::app::{self, details, refresh, run_board, Shared};
use crate::auth::{self, cookie_value, peer_ip, LoginError, COOKIE};
use crate::board::{self, Status};
use crate::config::now_ms;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

const INDEX: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");
const LOGIN: &str = include_str!("../web/login.html");
const LOGIN_CSS: &str = include_str!("../web/login.css");
const LOGIN_JS: &str = include_str!("../web/login.js");
const FAVICON: &str = include_str!("../web/favicon.svg");

pub fn router(app: Shared) -> Router {
    let protected = Router::new()
        .route("/api/state", get(api_state))
        .route("/api/summary", get(api_summary))
        .route("/api/details", get(api_details))
        .route("/api/action", post(api_action))
        .route("/api/chat", post(api_chat))
        .route("/api/refresh", post(api_refresh))
        .route("/api/logout", post(api_logout))
        .route_layer(middleware::from_fn_with_state(
            app.clone(),
            auth::require_auth,
        ));

    Router::new()
        .route("/", get(index))
        .route("/login", get(login_page))
        .route("/api/login", post(api_login))
        .route(
            "/app.css",
            get(|| async { asset(APP_CSS, "text/css; charset=utf-8") }),
        )
        .route(
            "/app.js",
            get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/login.css",
            get(|| async { asset(LOGIN_CSS, "text/css; charset=utf-8") }),
        )
        .route(
            "/login.js",
            get(|| async { asset(LOGIN_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/favicon.svg",
            get(|| async { asset(FAVICON, "image/svg+xml") }),
        )
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .merge(protected)
        .layer(middleware::from_fn(headers_middleware))
        .with_state(app)
}

async fn headers_middleware(req: axum::extract::Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; connect-src 'self'; form-action 'self'; \
             frame-ancestors 'none'; base-uri 'self'",
        ),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, max-age=0"),
        );
    }
    res
}

fn asset(body: &'static str, mime: &'static str) -> Response {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static(mime))],
        body,
    )
        .into_response()
}

fn authed(app: &Shared, headers: &HeaderMap) -> bool {
    if !app.auth.required {
        return true;
    }
    cookie_value(headers, COOKIE)
        .map(|t| app.auth.valid(&t))
        .unwrap_or(false)
}

async fn index(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if !authed(&app, &headers) {
        return Redirect::to("/login").into_response();
    }
    Html(INDEX).into_response()
}

async fn login_page(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if authed(&app, &headers) {
        return Redirect::to("/").into_response();
    }
    let (css, mode) = {
        let snap = app.snapshot.read().await;
        (theme_css(&snap.theme), snap.theme.mode.clone())
    };
    let html = LOGIN.replace("__THEME__", &css).replace("__MODE__", &mode);
    Html(html).into_response()
}

fn theme_css(theme: &crate::theme::Theme) -> String {
    let mut s = String::from(":root{");
    for (k, v) in &theme.colors {
        s.push_str(&format!("--c-{}:{};", k, v));
    }
    s.push('}');
    s
}

/// The address a proxy just added to `X-Forwarded-For` (the last entry). It
/// is trusted only when the connection itself comes from loopback, so a
/// directly exposed service cannot be fooled by a client-supplied header.
fn forwarded_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .and_then(|v| v.trim().parse().ok())
}

fn client_ip(addr: SocketAddr, headers: &HeaderMap) -> IpAddr {
    let peer = peer_ip(addr);
    if peer.is_loopback() {
        forwarded_ip(headers).unwrap_or(peer)
    } else {
        peer
    }
}

/// A file action may open a terminal only for someone sitting at this machine.
/// Behind a same-host TLS proxy the peer is loopback for everyone, so the
/// forwarded address decides instead.
fn client_is_local(addr: SocketAddr, headers: &HeaderMap) -> bool {
    let peer = peer_ip(addr);
    if !peer.is_loopback() {
        return false;
    }
    forwarded_ip(headers)
        .map(|ip| ip.is_loopback())
        .unwrap_or(true)
}

#[derive(Deserialize)]
struct LoginBody {
    user: String,
    password: String,
}

async fn api_login(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    if !app.auth.required {
        return Json(json!({"ok": true, "login": false})).into_response();
    }
    if body.password.len() > 4096 || body.user.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "bad request"})),
        )
            .into_response();
    }
    match app
        .auth
        .login(client_ip(addr, &headers), &body.user, &body.password)
    {
        Ok(token) => {
            let mut res = Json(json!({"ok": true})).into_response();
            auth::set_cookie(res.headers_mut(), app.auth.cookie(&token));
            res
        }
        Err(LoginError::Invalid) => {
            tokio::time::sleep(Duration::from_millis(300)).await;
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Неверный пользователь или пароль"})),
            )
                .into_response()
        }
        Err(LoginError::TooMany(secs)) => {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let mut res = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": format!("Слишком много неудачных попыток. Подождите {} с.", secs),
                    "retry_after": secs
                })),
            )
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                res.headers_mut().insert(header::RETRY_AFTER, v);
            }
            res
        }
    }
}

async fn api_logout(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_value(&headers, COOKIE) {
        app.auth.logout(&token);
    }
    let mut res = Json(json!({"ok": true})).into_response();
    auth::set_cookie(res.headers_mut(), app.auth.clear_cookie());
    res
}

async fn api_state(State(app): State<Shared>) -> Json<app::Snapshot> {
    Json(app.snapshot.read().await.clone())
}

async fn api_summary(State(app): State<Shared>) -> Json<serde_json::Value> {
    let s = app.snapshot.read().await.clone();
    let newest = s
        .tasks
        .iter()
        .filter(|t| t.status.needs_you())
        .min_by_key(|t| t.status.urgency())
        .map(|t| format!("{}: {}", t.tag, t.label))
        .unwrap_or_default();
    Json(json!({
        "waiting": s.summary.attention,
        "review": s.summary.review,
        "ready": s.summary.ready,
        "failed": s.summary.failed,
        "working": s.summary.working,
        "queued": s.summary.queued,
        "projects": s.summary.active_projects,
        "quiet": s.summary.quiet_projects,
        "archived": s.summary.archived,
        "waiting_hint": newest,
        "online": true,
        "login": s.auth.required,
    }))
}

#[derive(Deserialize)]
struct DetailsQuery {
    id: String,
}

async fn api_details(State(app): State<Shared>, Query(q): Query<DetailsQuery>) -> Response {
    match details(&app, &q.id).await {
        Ok(d) => Json(json!({"ok": true, "details": d})).into_response(),
        Err(e) => error(StatusCode::BAD_GATEWAY, e),
    }
}

async fn api_refresh(State(app): State<Shared>) -> Response {
    refresh(&app).await;
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct ChatBody {
    text: String,
}

async fn api_chat(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ChatBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let text = body.text.trim();
    if text.is_empty() {
        return error(StatusCode::BAD_REQUEST, "empty message");
    }
    if text.chars().count() > 4000 {
        return error(StatusCode::BAD_REQUEST, "message is too long");
    }
    if let Err(e) = run_board(
        &app.cfg,
        &["coordinator", "tell", "--", text],
        Duration::from_secs(30),
    )
    .await
    {
        return error(StatusCode::BAD_GATEWAY, e);
    }
    {
        let mut chat = app.chat.lock().await;
        chat.last_sent = text.to_string();
        chat.sent_at = now_ms();
        chat.busy_until = now_ms() + 120_000;
    }
    let chat = app.chat.lock().await.clone();
    // Persist right away; the next refresh also does it.
    app::save_chat(&app.cfg, &chat);
    let app2 = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        refresh(&app2).await;
    });
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct ActionBody {
    action: String,
    id: Option<String>,
    text: Option<String>,
    confirm: Option<bool>,
    scope: Option<String>,
    repo: Option<String>,
    kind: Option<String>,
}

async fn api_action(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<ActionBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let local = client_is_local(addr, &headers);
    if matches!(
        body.action.as_str(),
        "retry" | "review" | "stop" | "archive" | "restore"
    ) {
        if let Err(e) = task_id(&body) {
            return error(StatusCode::BAD_REQUEST, e);
        }
    }
    let _guard = app.action_lock.lock().await;
    let result = match body.action.as_str() {
        "approve" | "anyway" => {
            let id = match task_id(&body) {
                Ok(id) => id,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            if body.confirm != Some(true) {
                return error(
                    StatusCode::BAD_REQUEST,
                    "approval needs an explicit confirmation",
                );
            }
            let mut args = vec!["approve", id];
            if body.action == "anyway" {
                args.push("--anyway");
            }
            run_board(&app.cfg, &args, Duration::from_secs(60)).await
        }
        "retry" => run_id(&app, &body, &["retry"], 60).await,
        "review" => run_id(&app, &body, &["review"], 60).await,
        "stop" => run_id(&app, &body, &["stop"], 30).await,
        "archive" => run_id(&app, &body, &["archive"], 30).await,
        "restore" => run_id(&app, &body, &["restore"], 30).await,
        "delegate" => {
            let id = match task_id(&body) {
                Ok(id) => id,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            run_board(
                &app.cfg,
                &["delegate", id, "--now"],
                Duration::from_secs(60),
            )
            .await
        }
        "tell" => {
            let id = match task_id(&body) {
                Ok(id) => id,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            let text = body.text.as_deref().unwrap_or("").trim();
            if text.is_empty() {
                return error(StatusCode::BAD_REQUEST, "empty message");
            }
            run_board(&app.cfg, &["tell", id, "--", text], Duration::from_secs(60)).await
        }
        "archive-done" => return archive_done(&app, &body).await,
        "yazi" => return yazi(&app, &body, local).await,
        other => return error(StatusCode::BAD_REQUEST, format!("unknown action: {other}")),
    };
    match result {
        Ok(out) => {
            refresh(&app).await;
            Json(json!({"ok": true, "out": clip(out)})).into_response()
        }
        Err(e) => error(StatusCode::BAD_GATEWAY, e),
    }
}

fn task_id(body: &ActionBody) -> Result<&str, String> {
    let id = body.id.as_deref().unwrap_or("");
    if !board::valid_id(id) || !id.contains('/') {
        return Err("bad task id".into());
    }
    Ok(id)
}

async fn run_id(
    app: &Shared,
    body: &ActionBody,
    command: &[&str],
    secs: u64,
) -> Result<String, String> {
    let id = task_id(body)?;
    let mut args: Vec<&str> = command.to_vec();
    args.push(id);
    run_board(&app.cfg, &args, Duration::from_secs(secs)).await
}

async fn archive_done(app: &Shared, body: &ActionBody) -> Response {
    let scope = body.scope.as_deref().unwrap_or("all");
    let repo = body.repo.as_deref().unwrap_or("");
    let ids: Vec<String> = {
        let snap = app.snapshot.read().await;
        snap.tasks
            .iter()
            .filter(|t| t.status == Status::Done)
            .filter(|t| scope != "project" || t.repo == repo)
            .map(|t| t.id.clone())
            .collect()
    };
    let mut done = 0usize;
    let mut errors = Vec::new();
    for id in ids {
        match run_board(&app.cfg, &["archive", &id], Duration::from_secs(30)).await {
            Ok(_) => done += 1,
            Err(e) => errors.push(format!("{id}: {e}")),
        }
    }
    refresh(app).await;
    Json(json!({"ok": errors.is_empty(), "archived": done, "errors": errors})).into_response()
}

async fn yazi(app: &Shared, body: &ActionBody, local: bool) -> Response {
    let id = match task_id(body) {
        Ok(id) => id,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let d = match details(app, id).await {
        Ok(d) => d,
        Err(e) => return error(StatusCode::BAD_GATEWAY, e),
    };
    let reports = body.kind.as_deref() == Some("reports");
    let path = if reports { d.reports_dir } else { d.worktree };
    if path.is_empty() {
        return error(
            StatusCode::NOT_FOUND,
            "у задачи ещё нет папки: worktree появится, когда её начнут",
        );
    }
    if !local {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "Файлы открываются только на машине с доской",
                "path": path,
                "remote": true
            })),
        )
            .into_response();
    }
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return error(StatusCode::NOT_FOUND, format!("папки нет: {path}"));
    }
    match tokio::process::Command::new("xdg-terminal-exec")
        .arg("yazi")
        .arg(p)
        .spawn()
    {
        Ok(_) => Json(json!({"ok": true, "path": path})).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("не удалось запустить xdg-terminal-exec yazi: {e}"),
        ),
    }
}

/// A cheap same-origin check for JSON POSTs: a browser's `Origin` must end in the
/// `Host` it is talking to. Requests without `Origin` (curl, tests) are allowed;
/// SameSite=Strict still keeps other sites from sending the session cookie.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    origin.ends_with(&format!("://{host}")) || origin.ends_with(&format!("://{host}/"))
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

fn clip(s: String) -> String {
    const MAX: usize = 4000;
    if s.chars().count() <= MAX {
        return s;
    }
    let mut out: String = s.chars().take(MAX).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn forwarded(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn a_direct_client_ignores_forwarded_headers() {
        let addr = SocketAddr::from(([10, 0, 0, 7], 9000));
        let h = forwarded("127.0.0.1");
        assert_eq!(client_ip(addr, &h), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)));
        assert!(!client_is_local(addr, &h));
    }

    #[test]
    fn a_same_host_proxy_uses_the_last_forwarded_address() {
        let proxy = SocketAddr::from(([127, 0, 0, 1], 9000));
        let h = forwarded("198.51.100.9, 203.0.113.5");
        assert_eq!(
            client_ip(proxy, &h),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5))
        );
        assert!(!client_is_local(proxy, &h));
        let local = forwarded("127.0.0.1");
        assert!(client_is_local(proxy, &local));
    }

    #[test]
    fn loopback_without_a_proxy_is_local() {
        let proxy = SocketAddr::from((Ipv6Addr::LOCALHOST, 9000));
        assert!(client_is_local(proxy, &HeaderMap::new()));
        assert_eq!(
            client_ip(proxy, &HeaderMap::new()),
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        );
    }
}
