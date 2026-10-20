//! HTTP: the page, the JSON API, login and the board commands behind it.

use crate::app::{self, details, refresh, run_board, save_todo, Shared, TodoItem};
use crate::attachments;
use crate::auth::{self, cookie_value, peer_ip, LoginError, COOKIE};
use crate::board::{self, Status};
use crate::config::now_ms;
use crate::usage;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::process::Stdio;
use std::time::{Duration, SystemTime};
use tokio::process::Command;
use tokio::time::timeout;

const INDEX: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");
const WORLD_JS: &str = include_str!("../web/world.js");
const CAMERA_JS: &str = include_str!("../web/camera.js");
const LAYOUT_JS: &str = include_str!("../web/layout.js");
const REQUESTS_JS: &str = include_str!("../web/requests.js");
/// three.js is vendored at a versioned path so the page may cache it for a year.
const THREE_JS: &str = include_str!("../web/vendor/three.module.js");
const THREE_CORE_JS: &str = include_str!("../web/vendor/three.core.js");
const ORBIT_JS: &str = include_str!("../web/vendor/OrbitControls.js");
const CSS2D_JS: &str = include_str!("../web/vendor/CSS2DRenderer.js");
const LOGIN: &str = include_str!("../web/login.html");
const LOGIN_CSS: &str = include_str!("../web/login.css");
const LOGIN_JS: &str = include_str!("../web/login.js");
const FAVICON: &str = include_str!("../web/favicon.svg");

pub fn router(app: Shared) -> Router {
    let protected = Router::new()
        .route("/api/state", get(api_state))
        .route("/api/summary", get(api_summary))
        .route("/api/details", get(api_details))
        .route("/api/coordinator/screen", get(api_coordinator_screen))
        .route("/api/action", post(api_action))
        .route("/api/chat", post(api_chat))
        .route(
            "/api/chat/attach",
            post(api_attach).layer(DefaultBodyLimit::max(attachments::MAX_BODY_BYTES)),
        )
        .route("/api/attachments/{id}", get(api_attachment))
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
        // The Omarchy bar widget is local and has no login cookie; only a
        // non-loopback caller must authenticate for this small summary.
        .route("/api/limits", get(api_limits))
        .route(
            "/app.css",
            get(|| async { asset(APP_CSS, "text/css; charset=utf-8") }),
        )
        .route(
            "/app.js",
            get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/world.js",
            get(|| async { asset(WORLD_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/camera.js",
            get(|| async { asset(CAMERA_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/layout.js",
            get(|| async { asset(LAYOUT_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/requests.js",
            get(|| async { asset(REQUESTS_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/vendor/0.186.1/three.module.js",
            get(|| async { asset_cached(THREE_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/vendor/0.186.1/three.core.js",
            get(|| async { asset_cached(THREE_CORE_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/vendor/0.186.1/OrbitControls.js",
            get(|| async { asset_cached(ORBIT_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/vendor/0.186.1/CSS2DRenderer.js",
            get(|| async { asset_cached(CSS2D_JS, "text/javascript; charset=utf-8") }),
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
             img-src 'self' data: blob:; connect-src 'self'; form-action 'self'; \
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

/// A vendored file whose path carries its version: safe to keep for a year.
fn asset_cached(body: &'static str, mime: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(mime)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        ],
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

/// Just enough for the bar widget to draw a dot: the worst limit and whether
/// it is stale, never the board's tasks or chat.
async fn api_limits(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let local = client_is_local(addr, &headers);
    if app.auth.required && !local && !authed(&app, &headers) {
        return error(StatusCode::UNAUTHORIZED, "login required");
    }
    let snap = app.snapshot.read().await;
    Json(usage::alert(&snap.limits)).into_response()
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

#[derive(Deserialize)]
struct ScreenQuery {
    target: Option<String>,
    /// `reviewer` selects the board's `<agent>-r` reviewer pane; otherwise the worker.
    role: Option<String>,
}

/// The text an agent shows in its own terminal, through `herdr agent read`.
/// Without herdr the planner says so; it never pretends to see the dialog.
async fn api_coordinator_screen(
    State(app): State<Shared>,
    Query(q): Query<ScreenQuery>,
) -> Response {
    let target = q.target.as_deref().unwrap_or("coordinator");
    let name = match agent_name(target, q.role.as_deref()) {
        Ok(name) => name,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let running = app.cfg.herdr_socket.exists();
    if !running {
        return Json(json!({
            "ok": false,
            "running": false,
            "target": name,
            "error": "herdr не запущен: диалог видно только в окне агента"
        }))
        .into_response();
    }
    match run_herdr(
        &[
            "agent", "read", &name, "--source", "visible", "--lines", "40", "--format", "text",
        ],
        Duration::from_secs(8),
    )
    .await
    {
        Ok(out) => Json(json!({
            "ok": true,
            "running": true,
            "target": name,
            "text": clip(out),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"ok": false, "running": true, "target": name, "error": e})),
        )
            .into_response(),
    }
}

/// `sw-coordinator` for the Coordinator, `sw-<repo>-<n>` for a task's agent.
fn agent_name(target: &str, role: Option<&str>) -> Result<String, String> {
    if target == "coordinator" {
        return Ok("sw-coordinator".into());
    }
    let id = target;
    if !board::valid_id(id) || !id.contains('/') {
        return Err("bad task id".into());
    }
    let base = format!("sw-{}-{}", board::repo_of(id), board::number_of(id));
    // The board keeps a task's reviewer in `<worker>-r` (finish/review_once.rs).
    if role == Some("reviewer") {
        Ok(format!("{base}-r"))
    } else {
        Ok(base)
    }
}

fn valid_todo_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn valid_repo(repo: &str) -> bool {
    !repo.is_empty()
        && repo.len() <= 200
        && repo
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

/// A God's-list text, made one line; the board's `add` takes it as a title.
fn todo_text(text: &str) -> Result<String, String> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return Err("пустое дело".into());
    }
    if t.chars().count() > 400 {
        return Err("дело длиннее 400 символов".into());
    }
    if t.starts_with('-') {
        return Err("дело не может начинаться с «-»".into());
    }
    Ok(t)
}

fn valid_key(k: &str) -> bool {
    matches!(
        k,
        "up" | "down"
            | "left"
            | "right"
            | "enter"
            | "esc"
            | "tab"
            | "space"
            | "backspace"
            | "y"
            | "n"
    ) || (k.len() == 1 && k.chars().all(|c| c.is_ascii_digit()))
}

/// Runs the herdr CLI (`HERDR_BIN` overrides the binary name for checks).
async fn run_herdr(args: &[&str], wait: Duration) -> Result<String, String> {
    let bin = std::env::var("HERDR_BIN")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "herdr".to_string());
    let mut cmd = Command::new(&bin);
    cmd.args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let out = match timeout(wait, cmd.output()).await {
        Err(_) => {
            return Err(format!(
                "{bin} {}: no answer in {}s",
                args.join(" "),
                wait.as_secs()
            ))
        }
        Ok(Err(e)) => return Err(format!("{bin}: {e}")),
        Ok(Ok(o)) => o,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(if stderr.is_empty() {
            format!("{bin} {}: exit {}", args.join(" "), out.status)
        } else {
            stderr
        });
    }
    Ok(stdout)
}

async fn api_refresh(State(app): State<Shared>) -> Response {
    refresh(&app).await;
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct ChatBody {
    #[serde(default)]
    text: String,
    #[serde(default)]
    attachments: Vec<AttachmentRef>,
}

/// A message may name uploaded files by id alone (older callers) or carry a
/// display name too. The id is always the authority; the name never becomes a
/// path.
#[derive(Deserialize)]
#[serde(untagged)]
enum AttachmentRef {
    Id(String),
    Named {
        id: String,
        #[serde(default)]
        name: String,
    },
}

impl AttachmentRef {
    fn id(&self) -> &str {
        match self {
            Self::Id(id) => id,
            Self::Named { id, .. } => id,
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::Named { name, .. } if !name.is_empty() => Some(name),
            _ => None,
        }
    }
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
    if body.attachments.len() > attachments::MAX_PER_MESSAGE {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "at most {} images per message",
                attachments::MAX_PER_MESSAGE
            ),
        );
    }
    if text.chars().count() > 4000 {
        return error(StatusCode::BAD_REQUEST, "message is too long");
    }

    let dir = app.cfg.attachments_dir();
    let mut files = Vec::new();
    let mut stored = Vec::new();
    let mut seen = HashSet::new();
    for reference in &body.attachments {
        let id = reference.id();
        let Some(kind) = attachments::parse_id(id) else {
            return error(StatusCode::BAD_REQUEST, "unknown attachment");
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let Some(path) = attachments::path(&dir, id) else {
            return error(StatusCode::BAD_REQUEST, "unknown attachment");
        };
        let meta = match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_file() => meta,
            _ => return error(StatusCode::BAD_REQUEST, "attachment is gone"),
        };
        // The Coordinator opens these paths itself: they are absolute paths on
        // the machine with the board, never paths from the browser's machine.
        let absolute = tokio::fs::canonicalize(&path).await.unwrap_or(path);
        files.push(absolute);
        stored.push(attachments::Attachment {
            id: id.to_string(),
            name: attachments::clean_name(reference.name(), kind),
            mime: kind.mime.to_string(),
            size: meta.len(),
        });
    }
    if text.is_empty() && files.is_empty() {
        return error(StatusCode::BAD_REQUEST, "empty message");
    }

    let message = attachments::message(text, &files);
    if let Err(e) = run_board(
        &app.cfg,
        &["coordinator", "tell", "--", &message],
        Duration::from_secs(30),
    )
    .await
    {
        return error(StatusCode::BAD_GATEWAY, e);
    }
    let at = now_ms();
    {
        let mut chat = app.chat.lock().await;
        chat.last_sent = text.to_string();
        chat.sent_at = at;
        chat.busy_until = at + 120_000;
        chat.history.push(app::ChatMessage {
            text: text.to_string(),
            at,
            attachments: stored,
        });
        let extra = chat.history.len().saturating_sub(app::CHAT_HISTORY_MAX);
        if extra > 0 {
            chat.history.drain(0..extra);
        }
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

/// One image per request (`file` field). The browser, local or remote, sends
/// the bytes over HTTP(S); nothing is read from the browser's filesystem.
async fn api_attach(
    State(app): State<Shared>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let mut upload = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() != Some("file") {
                    continue;
                }
                let name = field.file_name().map(str::to_owned);
                match field.bytes().await {
                    Ok(bytes) => {
                        upload = Some((name, bytes));
                        break;
                    }
                    Err(e) => {
                        return error(e.status(), format!("could not read the upload: {e}"))
                    }
                }
            }
            Ok(None) => break,
            Err(e) => return error(e.status(), format!("could not read the upload: {e}")),
        }
    }
    let Some((name, bytes)) = upload else {
        return error(StatusCode::BAD_REQUEST, "no file in the upload");
    };
    if bytes.len() > attachments::MAX_IMAGE_BYTES {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "image is larger than {} MB",
                attachments::MAX_IMAGE_BYTES / (1024 * 1024)
            ),
        );
    }
    if attachments::sniff(&bytes).is_none() {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "only PNG, JPEG, WebP and GIF are accepted",
        );
    }

    let dir = app.cfg.attachments_dir();
    let attachment = match attachments::save(&dir, name.as_deref(), &bytes).await {
        Ok(a) => a,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    // Keep the folder bounded; the old files are no longer needed by any chat
    // entry the page shows (history is at most 20 messages old).
    let cleanup_dir = dir.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            attachments::cleanup(&cleanup_dir, SystemTime::now())
        })
        .await;
        app::prune_chat_attachments(&app2).await;
    });
    Json(json!({"ok": true, "attachment": attachment})).into_response()
}

/// A thumbnail or a full image for the chat history. Behind the login, with an
/// id that can only be one of the files `save` created.
async fn api_attachment(State(app): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(path) = attachments::path(&app.cfg.attachments_dir(), &id) else {
        return error(StatusCode::NOT_FOUND, "image not found");
    };
    let Some(mime) = attachments::mime(&id) else {
        return error(StatusCode::NOT_FOUND, "image not found");
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(mime)),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("private, max-age=3600"),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => error(StatusCode::NOT_FOUND, "image not found"),
    }
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
    /// The complete `after` set for a task; empty means `--clear`.
    after: Option<Vec<String>>,
    /// An attempt letter for `pick`.
    letter: Option<String>,
    /// Keys for `herdr agent send-keys`.
    keys: Option<Vec<String>>,
    /// `coordinator` or a task id, for `keys` and the screen endpoint.
    target: Option<String>,
    /// `reviewer` asks for `<agent>-r`; the worker is the default.
    role: Option<String>,
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
        "fix" => run_id(&app, &body, &["fix"], 60).await,
        "finish" => run_id(&app, &body, &["finish"], 60).await,
        "archive" => run_id(&app, &body, &["archive"], 30).await,
        "restore" => run_id(&app, &body, &["restore"], 30).await,
        "pick" => {
            let id = match task_id(&body) {
                Ok(id) => id,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            let letter = body.letter.as_deref().unwrap_or("");
            if !board::valid_letter(letter) {
                return error(StatusCode::BAD_REQUEST, "bad attempt letter");
            }
            run_board(&app.cfg, &["pick", id, letter], Duration::from_secs(60)).await
        }
        "after" => {
            let id = match task_id(&body) {
                Ok(id) => id,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            let Some(refs) = body.after.clone() else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "after needs the list of tasks; an empty list clears the wait",
                );
            };
            if let Some(bad) = refs.iter().find(|r| !board::valid_ref(r)) {
                return error(
                    StatusCode::BAD_REQUEST,
                    format!("bad task reference: {bad}"),
                );
            }
            let joined = refs.join(",");
            let mut args: Vec<&str> = vec!["after", id];
            if refs.is_empty() {
                args.push("--clear");
            } else {
                args.push(&joined);
            }
            app.force_after
                .store(true, std::sync::atomic::Ordering::SeqCst);
            run_board(&app.cfg, &args, Duration::from_secs(30)).await
        }
        "todo-add" => {
            let text = match todo_text(body.text.as_deref().unwrap_or("")) {
                Ok(t) => t,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            let mut snap = app.snapshot.write().await;
            let id = format!("t{}-{}", now_ms(), snap.todo.len() + 1);
            snap.todo.push(TodoItem {
                id,
                text,
                at: now_ms(),
            });
            let todo = snap.todo.clone();
            drop(snap);
            save_todo(&app.cfg, &todo);
            return Json(json!({"ok": true, "todo": todo})).into_response();
        }
        "todo-update" => {
            let id = body.id.as_deref().unwrap_or("");
            if !valid_todo_id(id) {
                return error(StatusCode::BAD_REQUEST, "bad todo id");
            }
            let text = match todo_text(body.text.as_deref().unwrap_or("")) {
                Ok(t) => t,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            let mut snap = app.snapshot.write().await;
            let Some(item) = snap.todo.iter_mut().find(|x| x.id == id) else {
                return error(StatusCode::NOT_FOUND, "no such todo item");
            };
            item.text = text;
            let todo = snap.todo.clone();
            drop(snap);
            save_todo(&app.cfg, &todo);
            return Json(json!({"ok": true, "todo": todo})).into_response();
        }
        "todo-remove" => {
            let id = body.id.as_deref().unwrap_or("");
            if !valid_todo_id(id) {
                return error(StatusCode::BAD_REQUEST, "bad todo id");
            }
            let mut snap = app.snapshot.write().await;
            let before = snap.todo.len();
            snap.todo.retain(|x| x.id != id);
            if snap.todo.len() == before {
                return error(StatusCode::NOT_FOUND, "no such todo item");
            }
            let todo = snap.todo.clone();
            drop(snap);
            save_todo(&app.cfg, &todo);
            return Json(json!({"ok": true, "todo": todo})).into_response();
        }
        "todo-create" => {
            let id = body.id.as_deref().unwrap_or("");
            if !valid_todo_id(id) {
                return error(StatusCode::BAD_REQUEST, "bad todo id");
            }
            let repo = body.repo.as_deref().unwrap_or("").trim().to_string();
            if !valid_repo(&repo) {
                return error(StatusCode::BAD_REQUEST, "bad universe name");
            }
            let (item, known) = {
                let snap = app.snapshot.read().await;
                (
                    snap.todo.iter().find(|x| x.id == id).cloned(),
                    snap.projects.iter().any(|p| p.name == repo),
                )
            };
            let Some(item) = item else {
                return error(StatusCode::NOT_FOUND, "no such todo item");
            };
            if !known {
                return error(
                    StatusCode::BAD_REQUEST,
                    format!("нет такой вселенной: {repo}"),
                );
            }
            let title = match todo_text(&item.text) {
                Ok(t) => t,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            match run_board(
                &app.cfg,
                &["add", title.as_str(), "--repo", repo.as_str()],
                Duration::from_secs(60),
            )
            .await
            {
                Ok(out) => {
                    let todo = {
                        let mut snap = app.snapshot.write().await;
                        snap.todo.retain(|x| x.id != id);
                        let todo = snap.todo.clone();
                        todo
                    };
                    save_todo(&app.cfg, &todo);
                    refresh(&app).await;
                    return Json(json!({"ok": true, "out": clip(out), "todo": todo, "repo": repo}))
                        .into_response();
                }
                Err(e) => return error(StatusCode::BAD_GATEWAY, e),
            }
        }
        "keys" => {
            let target = body.target.as_deref().unwrap_or("coordinator");
            let name = match agent_name(target, body.role.as_deref()) {
                Ok(name) => name,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            let keys = body.keys.clone().unwrap_or_default();
            if keys.is_empty() || keys.len() > 12 {
                return error(StatusCode::BAD_REQUEST, "give one to twelve keys");
            }
            if let Some(bad) = keys.iter().find(|k| !valid_key(k)) {
                return error(StatusCode::BAD_REQUEST, format!("key not allowed: {bad}"));
            }
            let mut args: Vec<&str> = vec!["agent", "send-keys", &name];
            args.extend(keys.iter().map(String::as_str));
            return match run_herdr(&args, Duration::from_secs(10)).await {
                Ok(out) => Json(json!({"ok": true, "out": clip(out)})).into_response(),
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            };
        }
        "delegate" => {
            let id = match task_id(&body) {
                Ok(id) => id,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            app.force_after
                .store(true, std::sync::atomic::Ordering::SeqCst);
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
    fn only_known_keys_reach_send_keys() {
        assert!(valid_key("enter"));
        assert!(valid_key("esc"));
        assert!(valid_key("up"));
        assert!(valid_key("y"));
        assert!(valid_key("7"));
        assert!(!valid_key("rm"));
        assert!(!valid_key("shift+tab"));
        assert!(!valid_key("-1"));
        assert!(!valid_key(""));
        assert!(!valid_key("1; rm -rf /"));
    }

    #[test]
    fn todo_text_is_one_safe_title() {
        assert_eq!(
            todo_text("  Починить   кэш \n отчётов ").unwrap(),
            "Починить кэш отчётов"
        );
        assert!(todo_text("").is_err());
        assert!(todo_text("   ").is_err());
        assert!(todo_text("--all").is_err());
        assert!(todo_text(&"x".repeat(401)).is_err());
        assert!(valid_todo_id("t1792407710774-1"));
        assert!(!valid_todo_id("../../etc"));
        assert!(!valid_todo_id(""));
        assert!(valid_repo("herdr-planner"));
        assert!(valid_repo("vm-str:ela/3") == false);
        assert!(!valid_repo(""));
    }

    #[test]
    fn agent_names_follow_the_board() {
        assert_eq!(agent_name("coordinator", None).unwrap(), "sw-coordinator");
        assert_eq!(agent_name("alpha/6", None).unwrap(), "sw-alpha-6");
        assert_eq!(
            agent_name("alpha/6", Some("reviewer")).unwrap(),
            "sw-alpha-6-r"
        );
        assert_eq!(
            agent_name("herdr-planner/7", None).unwrap(),
            "sw-herdr-planner-7"
        );
        assert!(agent_name("--all", None).is_err());
        assert!(agent_name("nothing", None).is_err());
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
