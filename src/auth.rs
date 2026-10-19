//! Login and sessions for the dashboard.
//!
//! Without a password hash the planner is local-only and needs no login (the
//! server refuses a non-loopback address in that case, see `main`). With one,
//! every `/api/*` call needs a session cookie: `HttpOnly; Secure; SameSite=Strict`.
//! Failed logins are limited per client address.

use crate::app::Shared;
use crate::config::{now_ms, verify_password};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

pub const COOKIE: &str = "herdr_planner_session";
const SESSION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const WINDOW_MS: u64 = 5 * 60 * 1000;
const BLOCK_MS: u64 = 5 * 60 * 1000;
const MAX_FAILS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginError {
    Invalid,
    TooMany(u64),
}

#[derive(Debug, Default)]
struct Attempt {
    fails: u32,
    first: u64,
    blocked_until: u64,
}

pub struct Auth {
    pub required: bool,
    user: String,
    hash: String,
    sessions: Mutex<HashMap<String, u64>>,
    attempts: Mutex<HashMap<IpAddr, Attempt>>,
}

impl Auth {
    pub fn new(required: bool, user: String, hash: String) -> Self {
        Self {
            required,
            user,
            hash,
            sessions: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
        }
    }

    /// Checks the password, then the name. Failures are counted against the address.
    pub fn login(&self, peer: IpAddr, user: &str, password: &str) -> Result<String, LoginError> {
        let now = now_ms();
        {
            let mut attempts = self.attempts.lock().unwrap();
            // Internet-facing: keep only addresses with a live block or a
            // failure inside the window, so scans cannot grow the map forever.
            attempts
                .retain(|_, a| a.blocked_until > now || now.saturating_sub(a.first) <= WINDOW_MS);
            if let Some(a) = attempts.get(&peer) {
                if a.blocked_until > now {
                    return Err(LoginError::TooMany(
                        (a.blocked_until - now).div_ceil(1000).max(1),
                    ));
                }
            }
        }
        let password_ok = verify_password(&self.hash, password);
        let user_ok = user == self.user;
        if !password_ok || !user_ok {
            let mut attempts = self.attempts.lock().unwrap();
            let a = attempts.entry(peer).or_default();
            if a.first == 0 || now.saturating_sub(a.first) > WINDOW_MS {
                a.first = now;
                a.fails = 0;
            }
            a.fails += 1;
            if a.fails >= MAX_FAILS {
                a.blocked_until = now + BLOCK_MS;
                return Err(LoginError::TooMany(BLOCK_MS.div_ceil(1000)));
            }
            return Err(LoginError::Invalid);
        }
        self.attempts.lock().unwrap().remove(&peer);
        let token = new_token();
        self.sessions
            .lock()
            .unwrap()
            .insert(token.clone(), now + SESSION_MS);
        Ok(token)
    }

    pub fn valid(&self, token: &str) -> bool {
        let now = now_ms();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, until| *until > now);
        sessions.get(token).is_some_and(|until| *until > now)
    }

    pub fn logout(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }

    pub fn cookie(&self, token: &str) -> String {
        format!(
            "{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
            SESSION_MS / 1000
        )
    }

    pub fn clear_cookie(&self) -> String {
        format!("{COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0")
    }
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        let text = value.to_str().ok()?;
        for part in text.split(';') {
            let part = part.trim();
            if let Some((k, v)) = part.split_once('=') {
                if k == name && !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Axum middleware for `/api/*`: 401 JSON unless the session cookie is valid.
pub async fn require_auth(State(app): State<Shared>, req: Request, next: Next) -> Response {
    if !app.auth.required {
        return next.run(req).await;
    }
    if let Some(token) = cookie_value(req.headers(), COOKIE) {
        if app.auth.valid(&token) {
            return next.run(req).await;
        }
    }
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized", "login": "/login"})),
    )
        .into_response()
}

pub fn set_cookie(headers: &mut HeaderMap, cookie: String) {
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        headers.insert(header::SET_COOKIE, v);
    }
}

pub fn peer_ip(addr: SocketAddr) -> IpAddr {
    addr.ip()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn auth() -> Auth {
        let hash = bcrypt::hash("hunter2", 4).unwrap();
        Auth::new(true, "me".into(), hash)
    }

    #[test]
    fn login_sets_a_session_and_logout_removes_it() {
        let a = auth();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let token = a.login(ip, "me", "hunter2").unwrap();
        assert!(a.valid(&token));
        assert!(!a.valid("another"));
        a.logout(&token);
        assert!(!a.valid(&token));
    }

    #[test]
    fn wrong_password_and_wrong_user_fail() {
        let a = auth();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(a.login(ip, "me", "wrong"), Err(LoginError::Invalid));
        assert_eq!(a.login(ip, "you", "hunter2"), Err(LoginError::Invalid));
    }

    #[test]
    fn five_failures_block_the_address() {
        let a = auth();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..4 {
            assert_eq!(a.login(ip, "me", "wrong"), Err(LoginError::Invalid));
        }
        assert!(matches!(
            a.login(ip, "me", "wrong"),
            Err(LoginError::TooMany(_))
        ));
        // The right password from the same address waits too.
        assert!(matches!(
            a.login(ip, "me", "hunter2"),
            Err(LoginError::TooMany(_))
        ));
    }

    #[test]
    fn cookies_are_parsed_and_secure() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; herdr_planner_session=abc; b=2"),
        );
        assert_eq!(cookie_value(&h, COOKIE).as_deref(), Some("abc"));
        assert_eq!(cookie_value(&h, "missing"), None);
        let a = auth();
        let c = a.cookie("tok");
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("Secure"));
        assert!(c.contains("SameSite=Strict"));
        assert!(c.contains("Path=/"));
    }
}
