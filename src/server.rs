//! HTTP: the page, the JSON API, login and the board commands behind it.

use crate::api;
use crate::app::{self, details, refresh, run_board, Shared};
use crate::attachments;
use crate::auth::{self, cookie_value, peer_ip, LoginError, COOKIE};
use crate::board::{self, Status};
use crate::config::now_ms;
use crate::history;
use crate::ship;
use crate::station;
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
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime};
use tokio::process::Command;
use tokio::time::timeout;

/// The longest text a person can write in a todo or a chat message: a page of a spec or a log,
/// far past the old one-line limits. Longer texts are refused with a message, never cut by the page.
const MAX_TEXT_CHARS: usize = 100_000;

/// The same text, in bytes: it goes to `herdr-projects` as one argument, and Linux refuses an
/// argument longer than 128 KiB (`MAX_ARG_STRLEN`). Room is kept for the attachment paths that
/// travel in the same argument. A Cyrillic letter is two bytes, so this is about 65 000 of them:
/// counting only characters would accept a text the process cannot even start with.
const MAX_ARG_BYTES: usize = 130_000;

/// Why a text cannot be sent to the board, if it cannot: too many characters, or more UTF-8 bytes
/// than one command-line argument may hold (the message names the real limit, never a cryptic E2BIG).
fn text_limit_error(text: &str, what: &str) -> Option<String> {
    if text.chars().count() > MAX_TEXT_CHARS {
        return Some(format!("{what} длиннее {MAX_TEXT_CHARS} символов"));
    }
    bytes_limit_error(text, what)
}

fn bytes_limit_error(text: &str, what: &str) -> Option<String> {
    if text.len() <= MAX_ARG_BYTES {
        return None;
    }
    Some(format!(
        "{what} не помещается в доску: {} байт в UTF-8, а предел — {MAX_ARG_BYTES} байт \
         (около 65000 русских букв); разбейте текст на части",
        text.len()
    ))
}

/// The built page (Vite): embedded into the binary; a debug build reads the folder.
#[derive(rust_embed::RustEmbed)]
#[folder = "$HERDR_PLANNER_DIST"]
struct Dist;

pub fn router(app: Shared) -> Router {
    let protected = Router::new()
        .route("/api/state", get(api_state))
        .route("/api/summary", get(api_summary))
        .route("/api/load", get(api_load))
        .route("/api/details", get(api_details))
        .route("/api/coordinator/screen", get(api_coordinator_screen))
        .route("/api/action", post(api_action))
        .route("/api/chat", post(api_chat))
        .route("/api/chat/history", get(api_chat_history))
        .route(
            "/api/chat/attach",
            post(api_attach).layer(DefaultBodyLimit::max(attachments::MAX_BODY_BYTES)),
        )
        .route("/api/attachments/{id}", get(api_attachment))
        .route("/api/refresh", post(api_refresh))
        .route("/api/events", get(api::events))
        .route("/api/search", get(api::search_api))
        .route("/api/models", get(api::models_api))
        .route("/api/translate", post(api::translate_api))
        .route("/api/subagent", get(api::subagent_api))
        .route("/api/task/closing", get(api::closing_api))
        .route("/api/fs/complete", get(api::fs_complete))
        .route("/api/universe/inspect", post(api::universe_inspect))
        .route("/api/universe/add", post(api::universe_add))
        .route("/api/universe/info", get(api::universe_info))
        .route("/api/universe/fork", post(api::universe_fork))
        .route("/api/memory", get(crate::memory_api::memory_get))
        .route("/api/memory/save", post(crate::memory_api::memory_save))
        .route("/api/memory/delete", post(crate::memory_api::memory_delete))
        .route("/api/memory/commit", post(crate::memory_api::memory_commit))
        .route("/api/memory/discard", post(crate::memory_api::memory_discard))
        .route("/api/memory/settings", post(crate::memory_api::memory_settings))
        .route("/api/memory/connect", post(crate::memory_api::memory_connect))
        .route("/api/memory/suggestion", post(crate::memory_api::memory_suggestion))
        .route("/api/runners", get(crate::runners_api::runners_get))
        .route("/api/runner/ask", post(crate::runners_api::runner_ask))
        .route("/api/runner/probe", post(crate::runners_api::runner_probe))
        .route("/api/runner/add", post(crate::runners_api::runner_add))
        .route("/api/runner/set", post(crate::runners_api::runner_set))
        .route("/api/runner/remove", post(crate::runners_api::runner_remove))
        .route("/api/runner/connect", get(crate::runners_api::runner_connect_get).post(crate::runners_api::runner_connect))
        .route("/api/runner/clean/preview", post(crate::runners_api::clean_preview))
        .route("/api/runner/clean", post(crate::runners_api::runner_clean))
        .route("/api/runner/df", post(crate::runners_api::runner_df))
        .route("/api/runner/terminal", post(crate::runners_api::runner_terminal))
        .route("/api/station", get(station::station_get))
        .route("/api/station/models", get(station::station_models))
        .route("/api/station/preview", post(station::station_preview))
        .route("/api/station/set", post(station::station_set))
        .route("/api/station/light", post(station::station_light))
        .route("/api/ship", get(ship::ship_get).post(ship::ship_post))
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
        .route("/assets/{*path}", get(dist_asset))
        .route("/favicon.svg", get(dist_root_file))
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
             img-src 'self' data: blob:; media-src 'self' blob:; connect-src 'self'; form-action 'self'; \
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
    match Dist::get("index.html") {
        Some(f) => Html(String::from_utf8_lossy(&f.data).into_owned()).into_response(),
        None => error(StatusCode::NOT_FOUND, "the page is not built"),
    }
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "html" => "text/html; charset=utf-8",
        "json" => "application/json",
        "woff2" => "font/woff2",
        "png" => "image/png",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Hashed files under /assets: safe to keep for a year.
async fn dist_asset(Path(path): Path<String>) -> Response {
    let full = format!("assets/{path}");
    match Dist::get(&full) {
        Some(f) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(mime_of(&full))),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                ),
            ],
            f.data.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn dist_root_file(req: axum::extract::Request) -> Response {
    let name = req.uri().path().trim_start_matches('/').to_string();
    match Dist::get(&name) {
        Some(f) => (
            [(header::CONTENT_TYPE, HeaderValue::from_static(mime_of(&name)))],
            f.data.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn login_page(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if authed(&app, &headers) {
        return Redirect::to("/").into_response();
    }
    let (css, mode) = {
        let snap = app.snapshot.read().await;
        (theme_css(&snap.theme), snap.theme.mode.clone())
    };
    // The theme goes in as a <style> after the bundled stylesheet, so it wins.
    let page = Dist::get("login.html")
        .map(|f| String::from_utf8_lossy(&f.data).into_owned())
        .unwrap_or_default();
    let html = page
        .replace("__MODE__", &mode)
        .replace("</head>", &format!("<style>{css}</style></head>"));
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
pub(crate) fn client_is_local(addr: SocketAddr, headers: &HeaderMap) -> bool {
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
                Json(json!({"error": "Неверный пользователь или пароль", "code": "invalid"})),
            )
                .into_response()
        }
        Err(LoginError::TooMany(secs)) => {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let mut res = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": format!("Слишком много неудачных попыток. Подождите {} с.", secs),
                    "code": "too_many",
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

/// The machine's load as the bars show it (the page asks when its event stream is down).
async fn api_load(State(app): State<Shared>) -> Json<crate::load::Load> {
    Json(crate::load::current(&app).await)
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

/// A God's-list text with the lines a person wrote; the board's `add` takes it as a title.
fn todo_text(text: &str) -> Result<String, String> {
    // The line breaks and the indentation a person wrote stay (a nested list, code, a log): only
    // trailing blanks and the blank lines at the ends go.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().map(str::trim_end).collect();
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(start, |i| i + 1);
    let t = lines[start..end].join("\n");
    if t.is_empty() {
        return Err("пустое дело".into());
    }
    if let Some(e) = text_limit_error(&t, "дело") {
        return Err(e);
    }
    // A pasted markdown list starts with «-» and is kept as it is: only the task title of
    // `todo-create` reaches the board as a bare argument, and it is made safe there.
    Ok(t)
}

/// A task's title from the first line of a todo: one line, without a list marker, and short enough
/// for the board's 60 columns in ordinary letters. A wider script may be shortened by the board;
/// the full text is in `--desc` anyway, so its line breaks are kept.
fn task_title(text: &str) -> String {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    // A markdown list item (`- milk`, `* milk`, `- [ ] milk`) is a title in words: the board reads
    // the first word as a flag when it starts with «-», and the whole text stays in the description.
    let mut first = first
        .trim_start_matches(|c: char| matches!(c, '-' | '*' | '+'))
        .trim_start();
    for box_ in ["[ ] ", "[x] ", "[X] "] {
        if let Some(rest) = first.strip_prefix(box_) {
            first = rest.trim_start();
            break;
        }
    }
    let mut out: String = first.chars().take(55).collect();
    if first.chars().count() > 55 {
        out.push('…');
    }
    if out.is_empty() {
        "Дело".into()
    } else {
        out
    }
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
struct HistoryQuery {
    /// Turns older than this one (`n` of the oldest the page has); the newest when absent.
    before: Option<usize>,
    limit: Option<usize>,
    /// Words to look for in the whole conversation instead of paging.
    q: Option<String>,
}

/// The Coordinator's past questions and answers, read from its transcript.
async fn api_chat_history(State(app): State<Shared>, Query(q): Query<HistoryQuery>) -> Response {
    let cfg = app.cfg.clone();
    let limit = q.limit.unwrap_or(3);
    if let Some(text) = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let db = app.db.clone();
        let text = text.to_string();
        let limit = limit.clamp(1, 50);
        return match tokio::task::spawn_blocking(move || history::search(&db, &text, limit)).await {
            Ok(hits) => Json(json!({"ok": true, "hits": hits})).into_response(),
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
    }
    match tokio::task::spawn_blocking(move || history::page(&cfg, q.before, limit)).await {
        Ok(page) => Json(json!({
            "ok": true,
            "available": page.available,
            "turns": page.turns,
            "more": page.more,
            "total": page.total,
        }))
        .into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
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

/// Files the page names by id (it uploaded them first): their records and their
/// absolute paths. The Coordinator and the agents open these paths themselves:
/// they are paths on the machine with the board, never paths from the browser's
/// machine.
async fn resolve_attachments(
    dir: &std::path::Path,
    refs: &[AttachmentRef],
) -> Result<(Vec<attachments::Attachment>, Vec<PathBuf>), Response> {
    let mut files = Vec::new();
    let mut stored = Vec::new();
    let mut seen = HashSet::new();
    for reference in refs {
        let id = reference.id();
        let Some(kind) = attachments::parse_id(id) else {
            return Err(error(StatusCode::BAD_REQUEST, "unknown attachment"));
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let Some(path) = attachments::path(dir, id) else {
            return Err(error(StatusCode::BAD_REQUEST, "unknown attachment"));
        };
        let meta = match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_file() => meta,
            _ => return Err(error(StatusCode::BAD_REQUEST, "attachment is gone")),
        };
        let absolute = tokio::fs::canonicalize(&path).await.unwrap_or(path);
        files.push(absolute);
        stored.push(attachments::Attachment {
            id: id.to_string(),
            name: attachments::clean_name(reference.name(), kind),
            mime: kind.mime.to_string(),
            size: meta.len(),
        });
    }
    Ok((stored, files))
}

/// The absolute paths of a todo item's files that are still there.
async fn held_paths(dir: &std::path::Path, files: &[attachments::Attachment]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in files {
        if let Some(path) = attachments::path(dir, &a.id) {
            if tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_file()) {
                out.push(tokio::fs::canonicalize(&path).await.unwrap_or(path));
            }
        }
    }
    out
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
    if let Some(e) = text_limit_error(text, "сообщение") {
        return error(StatusCode::BAD_REQUEST, e);
    }
    let dir = app.cfg.attachments_dir();
    let (stored, files) = match resolve_attachments(&dir, &body.attachments).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    if text.is_empty() && files.is_empty() {
        return error(StatusCode::BAD_REQUEST, "empty message");
    }

    let message = attachments::message(text, &files);
    if let Some(e) = bytes_limit_error(&message, "сообщение с вложениями") {
        return error(StatusCode::BAD_REQUEST, e);
    }
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
    app::save_chat(&app, &chat);
    let app2 = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        refresh(&app2).await;
    });
    Json(json!({"ok": true})).into_response()
}

/// One file per request (`file` field). The browser, local or remote, sends
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
    if bytes.len() > attachments::MAX_AUDIO_BYTES {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "file is larger than {} MB",
                attachments::MAX_AUDIO_BYTES / (1024 * 1024)
            ),
        );
    }
    let Some(kind) = attachments::sniff(&bytes) else {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "only images (PNG, JPEG, WebP, GIF), sound (MP3, WAV, OGG, FLAC, M4A, AAC) and UTF-8 text are accepted",
        );
    };
    if bytes.len() > kind.limit() {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "this kind of file is larger than {} MB",
                kind.limit() / (1024 * 1024)
            ),
        );
    }

    let dir = app.cfg.attachments_dir();
    let attachment = match attachments::save(&dir, name.as_deref(), &bytes).await {
        Ok(a) => a,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    // Keep the folder bounded; the old files are no longer needed by any chat
    // entry the page shows (history is at most 20 messages old) nor by a todo item.
    let cleanup_dir = dir.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        let held = app::held_files(&app2.db.todo_list());
        let _ = tokio::task::spawn_blocking(move || {
            attachments::cleanup(&cleanup_dir, SystemTime::now(), &held)
        })
        .await;
        app::prune_chat_attachments(&app2).await;
    });
    Json(json!({"ok": true, "attachment": attachment})).into_response()
}

/// A thumbnail, a full image, a sound or a text for the chat history and the todo list. Behind the login, with an
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
                // (every response carries `nosniff`: text is served as text, whatever it looks like)
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
    /// The worker agent for a new task: `claude/claude-sonnet-5-5`.
    agent: Option<String>,
    /// Uploaded files a todo item holds (todo-add; todo-update replaces the set).
    attachments: Option<Vec<AttachmentRef>>,
    /// The order of the God's list.
    ids: Option<Vec<String>>,
    /// To whom a list item is addressed: empty, `quasar` or a task id.
    to: Option<String>,
    /// A number the action carries (how far a clone is behind).
    n: Option<u32>,
}

/// `claude/claude-sonnet-5-5`, `omp/deepseek/deepseek-v4-flash`: what `--worker.agent` takes.
pub(crate) fn valid_agent(a: &str) -> bool {
    !a.is_empty()
        && a.len() <= 120
        && a.split('/').count() >= 2
        && a.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && a.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
        && !a.contains("..")
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
            let files = match todo_files(&app, body.attachments.as_deref().unwrap_or(&[])).await {
                Ok(f) => f,
                Err(e) => return e,
            };
            let db = app.db.clone();
            let item = tokio::task::spawn_blocking(move || {
                let item = db.todo_add(&text, now_ms());
                if !files.is_empty() && !item.id.is_empty() {
                    db.todo_files(&item.id, &files);
                }
                item
            })
            .await
            .unwrap_or_default();
            if item.id.is_empty() {
                return error(StatusCode::INTERNAL_SERVER_ERROR, "the list could not be saved");
            }
            return todo_reply(&app).await;
        }
        "todo-update" => {
            let id = body.id.as_deref().unwrap_or("");
            if !valid_todo_id(id) {
                return error(StatusCode::BAD_REQUEST, "bad todo id");
            }
            if body.text.is_none() && body.attachments.is_none() {
                return error(StatusCode::BAD_REQUEST, "nothing to change");
            }
            let text = match body.text.as_deref().map(todo_text).transpose() {
                Ok(t) => t,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            let files = match body.attachments.as_deref() {
                Some(refs) => match todo_files(&app, refs).await {
                    Ok(f) => Some(f),
                    Err(e) => return e,
                },
                None => None,
            };
            if let Some(text) = &text {
                if !app.db.todo_update(id, text) {
                    return error(StatusCode::NOT_FOUND, "no such todo item");
                }
            }
            if let Some(files) = &files {
                if !app.db.todo_files(id, files) {
                    return error(StatusCode::NOT_FOUND, "no such todo item");
                }
            }
            return todo_reply(&app).await;
        }
        "todo-remove" => {
            let id = body.id.as_deref().unwrap_or("");
            if !valid_todo_id(id) {
                return error(StatusCode::BAD_REQUEST, "bad todo id");
            }
            if !app.db.todo_remove(id) {
                return error(StatusCode::NOT_FOUND, "no such todo item");
            }
            return todo_reply(&app).await;
        }
        "todo-reorder" => {
            let ids = body.ids.clone().unwrap_or_default();
            if ids.len() > 500 || ids.iter().any(|i| !valid_todo_id(i)) {
                return error(StatusCode::BAD_REQUEST, "bad order");
            }
            app.db.todo_reorder(&ids);
            return todo_reply(&app).await;
        }
        "todo-assign" => {
            let id = body.id.as_deref().unwrap_or("");
            let to = body.to.as_deref().unwrap_or("");
            if !valid_todo_id(id) || !(to.is_empty() || to == "quasar" || (board::valid_id(to) && to.contains('/'))) {
                return error(StatusCode::BAD_REQUEST, "bad addressee");
            }
            if !app.db.todo_assign(id, to) {
                return error(StatusCode::NOT_FOUND, "no such todo item");
            }
            return todo_reply(&app).await;
        }
        "todo-send" => {
            // The item goes to its addressee as a message and leaves the list.
            let id = body.id.as_deref().unwrap_or("");
            if !valid_todo_id(id) {
                return error(StatusCode::BAD_REQUEST, "bad todo id");
            }
            let Some(item) = app.db.todo_list().into_iter().find(|x| x.id == id) else {
                return error(StatusCode::NOT_FOUND, "no such todo item");
            };
            let text = match todo_text(&item.text) {
                Ok(t) => t,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            // The files go along as absolute paths on this machine.
            let paths = held_paths(&app.cfg.attachments_dir(), &item.attachments).await;
            let text = attachments::message(&text, &paths);
            if let Some(e) = bytes_limit_error(&text, "дело с вложениями") {
                return error(StatusCode::BAD_REQUEST, e);
            }
            let sent = if item.to == "quasar" {
                run_board(&app.cfg, &["coordinator", "tell", "--", &text], Duration::from_secs(30)).await
            } else if board::valid_id(&item.to) && item.to.contains('/') {
                run_board(&app.cfg, &["tell", &item.to, "--", &text], Duration::from_secs(60)).await
            } else {
                return error(StatusCode::BAD_REQUEST, "у дела нет адресата");
            };
            return match sent {
                Ok(_) => {
                    app.db.todo_remove(id);
                    refresh(&app).await;
                    todo_reply(&app).await
                }
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            };
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
            let agent = body.agent.as_deref().map(str::trim).filter(|a| !a.is_empty());
            if let Some(a) = agent {
                if !valid_agent(a) {
                    return error(StatusCode::BAD_REQUEST, "bad agent/model");
                }
            }
            let known = {
                let snap = app.snapshot.read().await;
                snap.projects.iter().any(|p| p.name == repo)
            };
            let Some(item) = app.db.todo_list().into_iter().find(|x| x.id == id) else {
                return error(StatusCode::NOT_FOUND, "no such todo item");
            };
            if !known {
                return error(StatusCode::BAD_REQUEST, format!("нет такой вселенной: {repo}"));
            }
            let text = match todo_text(&item.text) {
                Ok(t) => t,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            };
            let paths = held_paths(&app.cfg.attachments_dir(), &item.attachments).await;
            // The board's `add` cleans the title (every line break becomes a space) and keeps it as
            // the description only when it had to shorten it: a multi-line todo is sent as a short
            // one-line title plus the whole text as `--desc`, which is kept as it is.
            let title = task_title(&text);
            let desc = if title == text {
                attachments::message("", &paths)
            } else {
                attachments::message(&text, &paths)
            };
            if let Some(e) = bytes_limit_error(&desc, "дело с вложениями") {
                return error(StatusCode::BAD_REQUEST, e);
            }
            let mut args: Vec<&str> = vec!["add", title.as_str(), "--repo", repo.as_str()];
            if !desc.is_empty() {
                args.push("--desc");
                args.push(desc.as_str());
            }
            if let Some(a) = agent {
                args.push("--worker.agent");
                args.push(a);
            }
            return match run_board(&app.cfg, &args, Duration::from_secs(60)).await {
                Ok(out) => {
                    app.db.todo_remove(id);
                    refresh(&app).await;
                    let mut reply = todo_json(&app).await;
                    reply["repo"] = json!(repo);
                    reply["out"] = json!(clip(out));
                    Json(reply).into_response()
                }
                Err(e) => error(StatusCode::BAD_GATEWAY, e),
            };
        }
        "suggest-update" => {
            let repo = body.repo.as_deref().unwrap_or("").trim().to_string();
            if !valid_repo(&repo) {
                return error(StatusCode::BAD_REQUEST, "bad universe name");
            }
            let behind = body.n.unwrap_or(0);
            let title = format!("Обновить главную ветку из origin (git pull --ff-only), отстаёт на {behind}");
            run_board(&app.cfg, &["add", title.as_str(), "--repo", repo.as_str()], Duration::from_secs(60)).await
        }
        "coordinator-compact" => {
            if body.confirm != Some(true) {
                return error(StatusCode::BAD_REQUEST, "compacting needs an explicit confirmation");
            }
            if !app.snapshot.read().await.coordinator.can_compact {
                return error(StatusCode::CONFLICT, "Координатор сейчас отвечает или это не Claude: сжать нельзя");
            }
            run_board(&app.cfg, &["coordinator", "tell", "--", "/compact"], Duration::from_secs(30)).await
        }
        "coordinator-new" => {
            if body.confirm != Some(true) {
                return error(StatusCode::BAD_REQUEST, "a new Coordinator needs an explicit confirmation");
            }
            run_board(&app.cfg, &["coordinator", "new"], Duration::from_secs(60)).await
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
            if let Some(e) = text_limit_error(text, "сообщение") {
                return error(StatusCode::BAD_REQUEST, e);
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

/// The files named for a todo item: at most `MAX_PER_MESSAGE`, each one uploaded before.
async fn todo_files(
    app: &Shared,
    refs: &[AttachmentRef],
) -> Result<Vec<attachments::Attachment>, Response> {
    if refs.len() > attachments::MAX_PER_MESSAGE {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("at most {} files per item", attachments::MAX_PER_MESSAGE),
        ));
    }
    resolve_attachments(&app.cfg.attachments_dir(), refs)
        .await
        .map(|(stored, _)| stored)
}

async fn todo_json(app: &Shared) -> serde_json::Value {
    let todo = app.db.todo_list();
    app.snapshot.write().await.todo = todo.clone();
    json!({"ok": true, "todo": todo})
}

async fn todo_reply(app: &Shared) -> Response {
    let reply = todo_json(app).await;
    app::publish(app).await;
    Json(reply).into_response()
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
pub(crate) fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    origin.ends_with(&format!("://{host}")) || origin.ends_with(&format!("://{host}/"))
}

pub(crate) fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

pub(crate) fn clip(s: String) -> String {
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
    fn todo_text_keeps_lines_and_stays_safe() {
        assert_eq!(
            todo_text("  Починить   кэш \n отчётов ").unwrap(),
            "  Починить   кэш\n отчётов"
        );
        assert_eq!(todo_text("first\r\nsecond\rthird").unwrap(), "first\nsecond\nthird");
        assert_eq!(todo_text("\n  first\n\nsecond  \n\n").unwrap(), "  first\n\nsecond");
        assert!(todo_text("").is_err());
        assert!(todo_text("\n \n").is_err());
        assert_eq!(todo_text("--all").unwrap(), "--all");
        assert_eq!(
            todo_text("- купить молоко\n- позвонить в банк").unwrap(),
            "- купить молоко\n- позвонить в банк"
        );
        assert!(todo_text(&"x".repeat(401)).is_ok());
        assert!(todo_text(&"x".repeat(MAX_TEXT_CHARS)).is_ok());
        assert!(todo_text(&"x".repeat(MAX_TEXT_CHARS + 1)).is_err());
        // A Cyrillic letter is two bytes: the real bound is the command-line argument, not the count.
        assert!(todo_text(&"ж".repeat(60_000)).is_ok());
        assert!(text_limit_error(&"ж".repeat(MAX_ARG_BYTES / 2), "дело").is_none());
        let too_many_bytes = todo_text(&"ж".repeat(MAX_ARG_BYTES / 2 + 1)).unwrap_err();
        assert!(too_many_bytes.contains("130000"), "{too_many_bytes}");
        assert!(!too_many_bytes.contains("Argument list"), "{too_many_bytes}");
        assert_eq!(
            text_limit_error(&"ж".repeat(60_000), "дело"),
            None
        );
        assert!(text_limit_error(&"x".repeat(MAX_TEXT_CHARS + 1), "сообщение")
            .unwrap()
            .contains("100000"));
        assert!(bytes_limit_error(&"ж".repeat(MAX_ARG_BYTES / 2 + 1), "сообщение")
            .unwrap()
            .contains("130000"));
        assert_eq!(task_title("  Первая строка дела\nвторая строка"), "Первая строка дела");
        assert_eq!(task_title("- купить молоко\n- позвонить в банк"), "купить молоко");
        assert_eq!(task_title("- [ ] купить молоко"), "купить молоко");
        assert_eq!(task_title("---"), "Дело");
        assert_eq!(task_title("коротко"), "коротко");
        assert_eq!(task_title(&"ж".repeat(50)), "ж".repeat(50));
        assert_eq!(task_title(&"ж".repeat(60)), format!("{}…", "ж".repeat(55)));
        assert!(!task_title(&"x".repeat(50)).starts_with('\n'));
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
