//! What a provider's reading needs from the outside: a secret that cannot leak by accident, a request that goes out
//! through `curl` (no HTTP library to add, and the secrets travel on curl's standard input as a config, never in its
//! arguments or environment), and the ways a reading can fail, each with a code the page translates.
//!
//! Nothing here logs, serializes or prints a secret: `Secret` has no `Display` or `Serialize`, and its `Debug` is blank.

use serde_json::Value;
use std::fmt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// How long one request may take, and how much of an answer is read.
pub const TIMEOUT: Duration = Duration::from_secs(20);
const LIMIT: usize = 2 * 1024 * 1024;

/// A key, a token or a cookie: held, spliced into a request, never shown.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        let text = text.trim();
        (!text.is_empty()).then(|| Self(text.to_string()))
    }
    fn expose(&self) -> &str {
        &self.0
    }
    /// A string field of the JSON this secret holds (a token kept as `{"value":"…"}`), or none when it is no such JSON.
    pub fn json_field(&self, key: &str) -> Option<Secret> {
        let v: Value = serde_json::from_str(&self.0).ok()?;
        secret_at(&v, &[key])
    }
    /// For reading the public claims of a token (when it expires), never for showing it.
    pub(super) fn reveal_for_claims(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// Why a provider's usage could not be read. The text never carries account details; `code` is what the page translates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// No sign-in found for a provider the planner was told about.
    #[allow(dead_code)]
    NoLogin,
    /// The sign-in is there but expired and only its own program renews it (Kimi's access token lives 15 minutes).
    Expired,
    /// The service refused the sign-in (401, 403).
    Rejected,
    RateLimited,
    Status(u16),
    /// No answer: no network, a refused connection, a proxy.
    Network,
    Timeout,
    /// An answer that is not the JSON the service documents.
    Parse,
    /// The account has no plan with limits to show.
    NoPlan,
    NoCurl,
    TooBig,
}

impl Fail {
    pub fn code(&self) -> String {
        match self {
            Self::NoLogin => "no_login".into(),
            Self::Expired => "expired".into(),
            Self::Rejected => "rejected".into(),
            Self::RateLimited => "rate_limited".into(),
            Self::Status(n) => format!("status:{n}"),
            Self::Network => "network".into(),
            Self::Timeout => "timeout".into(),
            Self::Parse => "parse".into(),
            Self::NoPlan => "no_plan".into(),
            Self::NoCurl => "no_curl".into(),
            Self::TooBig => "too_big".into(),
        }
    }

    /// The same, in words (the crew screen and the logs read this one).
    pub fn ru(&self) -> String {
        match self {
            Self::NoLogin => "вход не найден".into(),
            Self::Expired => "вход просрочен: откройте программу, она обновит его".into(),
            Self::Rejected => "сервис отклонил вход: войдите в программу заново".into(),
            Self::RateLimited => "сервис ограничил частоту запросов; повтор позже".into(),
            Self::Status(n) => format!("сервис ответил HTTP {n}"),
            Self::Network => "сервис не отвечает".into(),
            Self::Timeout => "сервис не ответил вовремя".into(),
            Self::Parse => "ответ сервиса не похож на документированный".into(),
            Self::NoPlan => "у аккаунта нет тарифа с лимитами".into(),
            Self::NoCurl => "нет программы curl".into(),
            Self::TooBig => "ответ слишком велик".into(),
        }
    }
}

pub type Res<T> = Result<T, Fail>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    pub timeout: Duration,
    /// Which provider asks (the checks' stand-in answers are looked up by it).
    pub tag: &'static str,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        write!(f, "Request({:?} {} {:?})", self.method, self.url, names)
    }
}

impl Request {
    pub fn get(tag: &'static str, url: impl Into<String>) -> Self {
        Self { method: Method::Get, url: url.into(), headers: Vec::new(), body: None, timeout: TIMEOUT, tag }
    }
    pub fn post(tag: &'static str, url: impl Into<String>) -> Self {
        Self { method: Method::Post, ..Self::get(tag, url) }
    }
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
    pub fn secret_header(mut self, name: &str, prefix: &str, secret: &Secret) -> Self {
        self.headers.push((name.to_string(), format!("{prefix}{}", secret.expose())));
        self
    }
    pub fn bearer(self, token: &Secret) -> Self {
        self.secret_header("Authorization", "Bearer ", token)
    }
    pub fn json(mut self, body: &str) -> Self {
        self.headers.push(("Content-Type".into(), "application/json".into()));
        self.body = Some(body.to_string());
        self
    }
}

/// An address a key may go to: https, or plain http to this very machine (the checks' own server), and no credentials in it.
pub fn safe_url(url: &str) -> bool {
    let rest = if let Some(r) = url.strip_prefix("https://") {
        r
    } else if let Some(r) = url.strip_prefix("http://") {
        let host = r.split(['/', ':']).next().unwrap_or("");
        if host != "127.0.0.1" && host != "localhost" {
            return false;
        }
        r
    } else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    !authority.is_empty() && !authority.contains('@') && !url.chars().any(|c| c.is_control() || c == ' ')
}

/// A curl config string: in double quotes, with the backslash and the quote escaped. A line break cannot be carried, so
/// a value with one is refused (it could only be a header injection).
fn quoted(value: &str) -> Option<String> {
    if value.contains(['\n', '\r', '\0']) {
        return None;
    }
    Some(format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// The config curl reads on its standard input (`-K -`).
fn config(req: &Request) -> Option<String> {
    let mut out = String::new();
    out.push_str(&format!("url = {}\n", quoted(&req.url)?));
    if req.method == Method::Post {
        out.push_str("request = \"POST\"\n");
    }
    for (name, value) in &req.headers {
        out.push_str(&format!("header = {}\n", quoted(&format!("{name}: {value}"))?));
    }
    if let Some(body) = &req.body {
        out.push_str(&format!("data = {}\n", quoted(body)?));
    }
    Some(out)
}

pub fn curl_bin() -> String {
    std::env::var("HERDR_PLANNER_CURL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "curl".into())
}

/// The answer's status and body.
pub async fn send(req: Request) -> Res<(u16, String)> {
    if let Some(dir) = std::env::var_os("HERDR_PLANNER_USAGE_STUB") {
        return stub(Path::new(&dir), &req);
    }
    if !safe_url(&req.url) {
        return Err(Fail::Network);
    }
    let cfg = config(&req).ok_or(Fail::Parse)?;
    let secs = req.timeout.as_secs().max(1).to_string();
    let mut child = tokio::process::Command::new(curl_bin())
        // -q: not a word of a curl rc file; -s -S: quiet, but errors on stderr; no redirects are followed, so a key never
        // goes where the service sent it
        .args(["-q", "-s", "-S", "-K", "-", "--max-time", &secs, "--max-filesize", "4000000", "-w", "\n%{http_code}"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { Fail::NoCurl } else { Fail::Network })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(cfg.as_bytes()).await.map_err(|_| Fail::Network)?;
    }
    let out = tokio::time::timeout(req.timeout + Duration::from_secs(5), child.wait_with_output())
        .await
        .map_err(|_| Fail::Timeout)?
        .map_err(|_| Fail::Network)?;
    if out.stdout.len() > LIMIT {
        return Err(Fail::TooBig);
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let Some((body, code)) = text.rsplit_once('\n') else {
        return Err(if out.status.code() == Some(28) { Fail::Timeout } else { Fail::Network });
    };
    let status: u16 = code.trim().parse().unwrap_or(0);
    if status == 0 {
        return Err(if out.status.code() == Some(28) { Fail::Timeout } else { Fail::Network });
    }
    Ok((status, body.to_string()))
}

/// The checks' stand-in for the services: `<dir>/<tag>.json` is the body, `<dir>/<tag>.status` (when there) the status.
fn stub(dir: &Path, req: &Request) -> Res<(u16, String)> {
    let body = std::fs::read_to_string(dir.join(format!("{}.json", req.tag))).map_err(|_| Fail::Network)?;
    let status = std::fs::read_to_string(dir.join(format!("{}.status", req.tag)))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(200);
    Ok((status, body))
}

/// The body when the status is a success, else the matching failure.
pub fn ok((status, body): (u16, String)) -> Res<String> {
    match status {
        200..=299 => Ok(body),
        401 | 403 => Err(Fail::Rejected),
        429 => Err(Fail::RateLimited),
        n => Err(Fail::Status(n)),
    }
}

pub async fn body(req: Request) -> Res<String> {
    ok(send(req).await?)
}

/* ───────── the sign-in files ───────── */

/// A JSON file of an agent's own sign-in. What is in it is read through `secret_at` and `text_at` and goes no further.
pub fn read_json(path: &Path) -> Option<Value> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 256 * 1024 {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, k| v.get(*k))
}

pub fn secret_at(v: &Value, path: &[&str]) -> Option<Secret> {
    match at(v, path)? {
        Value::String(s) => Secret::new(s.as_str()),
        Value::Number(n) => Secret::new(n.to_string()),
        _ => None,
    }
}

/// A field that is no secret (a plan name, an email), as text.
pub fn text_at(v: &Value, path: &[&str]) -> Option<String> {
    at(v, path)?.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_blank_in_debug_and_empty_ones_are_none() {
        let s = Secret::new("sk-very-secret").unwrap();
        assert_eq!(format!("{s:?}"), "Secret(…)");
        let r = Request::get("x", "https://a.example/u").bearer(&s).header("X-Plain", "1");
        let shown = format!("{r:?}");
        assert!(!shown.contains("sk-very-secret"), "{shown}");
        assert!(shown.contains("Authorization"));
        assert!(Secret::new("  \n").is_none());
    }

    #[test]
    fn keys_go_only_to_https_or_this_machine() {
        assert!(safe_url("https://api.anthropic.com/api/oauth/usage"));
        assert!(safe_url("http://127.0.0.1:8080/x"));
        assert!(safe_url("http://localhost/x"));
        assert!(!safe_url("http://api.example.com/x"));
        assert!(!safe_url("https://user:pw@api.example.com/x"));
        assert!(!safe_url("https://a.example/x y"));
        assert!(!safe_url("file:///etc/passwd"));
        assert!(!safe_url("ftp://a.example"));
    }

    #[test]
    fn the_curl_config_quotes_and_refuses_line_breaks() {
        let s = Secret::new("a\"b\\c").unwrap();
        let r = Request::post("x", "https://a.example/u").bearer(&s).json("{\"k\":1}");
        let c = config(&r).unwrap();
        assert!(c.contains("url = \"https://a.example/u\""));
        assert!(c.contains("request = \"POST\""));
        assert!(c.contains(r#"header = "Authorization: Bearer a\"b\\c""#), "{c}");
        assert!(c.contains(r#"data = "{\"k\":1}""#), "{c}");
        let evil = Secret::new("tok\nheader = \"X: y\"").unwrap();
        assert!(config(&Request::get("x", "https://a.example/").bearer(&evil)).is_none());
    }

    #[test]
    fn statuses_become_failures() {
        assert_eq!(ok((200, "{}".into())).unwrap(), "{}");
        assert_eq!(ok((401, String::new())), Err(Fail::Rejected));
        assert_eq!(ok((403, String::new())), Err(Fail::Rejected));
        assert_eq!(ok((429, String::new())), Err(Fail::RateLimited));
        assert_eq!(ok((502, String::new())), Err(Fail::Status(502)));
        assert_eq!(Fail::Status(502).code(), "status:502");
    }

    #[test]
    fn fields_of_a_sign_in_file() {
        let v: Value = serde_json::from_str(r#"{"a":{"token":"t0k","plan":" max ","n":7,"empty":""}}"#).unwrap();
        assert_eq!(secret_at(&v, &["a", "token"]).unwrap().expose(), "t0k");
        assert_eq!(secret_at(&v, &["a", "n"]).unwrap().expose(), "7");
        assert!(secret_at(&v, &["a", "empty"]).is_none());
        assert!(secret_at(&v, &["a", "missing"]).is_none());
        assert_eq!(text_at(&v, &["a", "plan"]).as_deref(), Some("max"));
    }

    /// A real round trip through curl to a server on this machine: the secret arrives as a header, the status and body come back.
    #[tokio::test]
    async fn curl_round_trip_sends_the_header_and_reads_the_answer() {
        if std::process::Command::new(curl_bin()).arg("--version").output().is_err() {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut c, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = c.read(&mut buf).unwrap();
            *seen2.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
            c.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 7\r\nConnection: close\r\n\r\n{\"a\":1}").unwrap();
        });
        let s = Secret::new("tok-123").unwrap();
        let r = Request::get("x", format!("http://127.0.0.1:{port}/usage")).bearer(&s).header("anthropic-beta", "b1");
        let (status, body) = send(r).await.unwrap();
        assert_eq!((status, body.as_str()), (429, "{\"a\":1}"));
        let got = seen.lock().unwrap().to_lowercase();
        assert!(got.contains("authorization: bearer tok-123"), "{got}");
        assert!(got.contains("anthropic-beta: b1"), "{got}");
        assert!(got.starts_with("get /usage"), "{got}");
    }

    #[tokio::test]
    async fn a_refused_connection_is_a_network_failure() {
        if std::process::Command::new(curl_bin()).arg("--version").output().is_err() {
            return;
        }
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let r = Request::get("x", format!("http://127.0.0.1:{port}/"));
        assert_eq!(send(r).await, Err(Fail::Network));
    }
}
