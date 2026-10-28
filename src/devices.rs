//! The gate's devices: the key in the link, the devices that came in with it, and their sessions.
//!
//! The owner opens the gate and the plaque shows a link `https://<address>/#k=<key>` (and the QR of it). The key is 16
//! random bytes, lives ten minutes and works once. It sits after the `#`, so it never reaches a server: a preview bot
//! that "opens" the link burns nothing, and Cloudflare and the logs of `cloudflared` do not see it. The page the
//! address answers with (`gate_page/gate.html`) reads it, wipes it from the address bar and sends it to `/g/redeem`.
//!
//! A right key makes a *device*: a session with a cookie (`__Host-hp_g`, `HttpOnly; Secure; SameSite=Strict`). By
//! default it is `pending` until the owner presses «Let in» on this machine (it declines itself after two minutes);
//! the setting `confirm_devices` skips that. An `active` device lives 24 hours at most and 6 hours without a request,
//! and at most five devices are there at once. The owner can switch any of them off. When the tunnel closes, fails
//! or the planner restarts, the key and every session are gone: sessions are kept in memory only.
//!
//! The key is compared in constant time; five wrong keys in five minutes from one address (`Cf-Connecting-Ip`) block
//! that address for five minutes, and a ceiling over all addresses stops a spread-out guess. What a device may reach is
//! the gate's business (`gate::verdict`), this module only says who is signed in.
//!
//! The gate closes itself when nobody has used it for `idle_close_h` hours (4 by default, 0: never): the clock counts
//! requests of active devices and the owner's own moves (a new link, «Let in»).

use crate::app::Shared;
use crate::auth::cookie_value;
use crate::config::{now_ms, AppConfig, IDLE_CLOSE_CHOICES};
use crate::server::{error, same_origin};
use crate::tunnel::{self, Phase};
use crate::version::{self, Form};
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

/// The session cookie of a device. `__Host-`: the browser takes it only with `Secure`, `Path=/` and no `Domain`.
pub const COOKIE: &str = "__Host-hp_g";
/// How long the link's key is good.
pub const KEY_TTL_MS: u64 = 10 * 60 * 1000;
/// A session ends this long after it began, whatever happens.
pub const SESSION_MS: u64 = 24 * 60 * 60 * 1000;
/// …and this long after its last request.
pub const IDLE_MS: u64 = 6 * 60 * 60 * 1000;
/// A device that nobody let in is declined after this.
pub const ASK_MS: u64 = 2 * 60 * 1000;
pub const MAX_DEVICES: usize = 5;
/// How long a device that is gone is still answered ("denied", "ended") so its page can say why.
const GONE_KEEP_MS: u64 = 10 * 60 * 1000;
const GONE_MAX: usize = 50;
const WINDOW_MS: u64 = 5 * 60 * 1000;
const BLOCK_MS: u64 = 5 * 60 * 1000;
const MAX_FAILS: u32 = 5;
/// Wrong keys from all addresses together: a guess spread over many addresses meets this.
const MAX_FAILS_ALL: u32 = 40;
/// Wrong planner passwords in a row after which a device is switched off.
const MAX_WRONG: u32 = 5;
const ADDRESSES_MAX: usize = 256;

/// What a device may reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The phone's light page.
    Lite,
    /// The planner itself (needs `full_allowed` and the planner's password): a device is raised to it by entering the
    /// planner's password (`raise_at`) and falls back to the light page when the setting is switched off.
    Full,
}

/// Why a device is not in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Why {
    /// The owner pressed «Decline».
    Denied,
    /// Nobody answered in two minutes.
    Unanswered,
    /// The owner switched it off.
    Revoked,
    /// The device left by itself.
    Left,
    /// 24 hours passed.
    Expired,
    /// 6 hours without a request.
    Idle,
    /// Five wrong planner passwords in a row.
    Password,
}

impl Why {
    fn words(self) -> &'static str {
        match self {
            Why::Denied => "отклонено",
            Why::Unanswered => "не дождалось ответа",
            Why::Revoked => "владельцем",
            Why::Left => "само вышло",
            Why::Expired => "по сроку: 24 ч",
            Why::Idle => "по простою: 6 ч",
            Why::Password => "пять неверных паролей",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Life {
    Pending,
    Active,
    Gone(Why),
}

/// How a request through the tunnel stands, from its cookie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// No cookie, a stranger's, or a device that is gone.
    Nobody,
    /// A device that waits for the owner.
    Waiting,
    Lite,
    Full,
}

#[derive(Clone, Debug)]
struct Device {
    id: String,
    token: String,
    state: Life,
    scope: Scope,
    label: String,
    country: Option<String>,
    touch: bool,
    form: Form,
    /// Wrong planner passwords since the last right one.
    wrong: u32,
    /// When it came in (the 24 hours count from the owner's «Let in»).
    since: u64,
    last: u64,
    /// The time it is declined if nobody answers.
    ask_until: u64,
    gone_at: u64,
}

impl Device {
    fn ends(&self) -> u64 {
        (self.since + SESSION_MS).min(self.last + IDLE_MS)
    }
}

/// Who knocks, as far as the server can tell.
#[derive(Clone, Debug, Default)]
pub struct Who {
    pub label: String,
    pub country: Option<String>,
    pub touch: bool,
    pub form: Form,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Settings {
    pub full_allowed: bool,
    pub confirm_devices: bool,
    /// Hours (0: never).
    pub idle_close_h: u32,
}

#[derive(Debug)]
struct Key {
    value: String,
    expires: u64,
    used: bool,
}

/// What the plaque says about the link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkState {
    /// Good for one device.
    Fresh,
    /// A device came in with it.
    Used,
    Expired,
    /// No gate is open.
    None,
}

pub struct LinkInfo {
    pub key: Option<String>,
    pub state: LinkState,
    pub expires_ms: Option<u64>,
}

#[derive(Default)]
struct Attempt {
    fails: u32,
    first: u64,
    blocked_until: u64,
}

impl Attempt {
    fn blocked(&self, now: u64) -> Option<u64> {
        (self.blocked_until > now).then(|| (self.blocked_until - now).div_ceil(1000).max(1))
    }

    fn fail(&mut self, now: u64, max: u32) {
        if self.first == 0 || now.saturating_sub(self.first) > WINDOW_MS {
            self.first = now;
            self.fails = 0;
        }
        self.fails += 1;
        if self.fails >= max {
            self.blocked_until = now + BLOCK_MS;
        }
    }

    fn live(&self, now: u64) -> bool {
        self.blocked_until > now || now.saturating_sub(self.first) <= WINDOW_MS
    }
}

struct Inner {
    key: Option<Key>,
    devices: Vec<Device>,
    /// The last time someone used the gate (the idle clock).
    busy: u64,
    attempts: HashMap<String, Attempt>,
    everyone: Attempt,
    /// Lines for the journal, said by whoever holds the lock and told by the async layer.
    notes: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RedeemError {
    /// Wrong, used, expired or none: the same answer for all, nobody learns which.
    Stale,
    /// Seconds until the address may try again.
    TooMany(u64),
    /// Five devices are in.
    Full,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecideError {
    NotFound,
    /// «Let in» on a device that is not waiting.
    NotPending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Allow,
    Deny,
    Revoke,
}

pub struct Admitted {
    pub token: String,
    pub state: Life,
    pub label: String,
    pub form: Form,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct DeviceView {
    pub id: String,
    pub label: String,
    pub scope: Scope,
    /// `pending` or `active`.
    pub state: &'static str,
    pub country: Option<String>,
    pub touch: bool,
    pub form: Form,
    pub since_ms: u64,
    pub last_ms: u64,
    /// When the session ends if nothing changes (active).
    pub ends_ms: Option<u64>,
    /// When the request is declined if nobody answers (pending).
    pub answer_by_ms: Option<u64>,
}

/// What a device that is in is told about its own session.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct SessionView {
    pub label: String,
    pub scope: Scope,
    pub form: Form,
    pub since_ms: u64,
    /// When the session ends if nothing changes (24 hours from the start, 6 hours from the last request).
    pub ends_ms: u64,
}

/// What `/g/status` says about a cookie.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Status {
    /// `none`, `pending`, `active` or `gone`.
    pub state: &'static str,
    pub reason: Option<Why>,
    pub label: Option<String>,
    /// What the device is and whether it has the full version: where it goes next.
    pub form: Form,
    pub raised: bool,
}

pub struct Devices {
    inner: Mutex<Inner>,
    settings: Mutex<Settings>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

/// Equal, in time that depends on the lengths and not on where the bytes differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(1);
    }
    diff == 0
}

fn is_live(s: Life) -> bool {
    matches!(s, Life::Pending | Life::Active)
}

impl Inner {
    fn gone(&mut self, i: usize, why: Why, now: u64) {
        let d = &mut self.devices[i];
        let was = d.state;
        d.state = Life::Gone(why);
        d.gone_at = now;
        let line = match (was, why) {
            (Life::Pending, Why::Left) => None,
            (Life::Pending, _) => Some(format!("устройство не пущено ({}): {}", why.words(), d.label)),
            (Life::Active, _) => Some(format!("устройство отключено ({}): {}", why.words(), d.label)),
            _ => None,
        };
        if let Some(l) = line {
            self.notes.push(l);
        }
    }

    /// Time passes: unanswered requests are declined, sessions run out, old tombstones go.
    fn maintain(&mut self, now: u64) {
        for i in 0..self.devices.len() {
            let d = &self.devices[i];
            let why = match d.state {
                Life::Pending if now >= d.ask_until => Some(Why::Unanswered),
                Life::Active if now >= d.since + SESSION_MS => Some(Why::Expired),
                Life::Active if now >= d.last + IDLE_MS => Some(Why::Idle),
                _ => None,
            };
            if let Some(w) = why {
                self.gone(i, w, now);
            }
        }
        self.devices.retain(|d| !matches!(d.state, Life::Gone(_)) || now.saturating_sub(d.gone_at) <= GONE_KEEP_MS);
        let gone: Vec<usize> = (0..self.devices.len()).filter(|i| matches!(self.devices[*i].state, Life::Gone(_))).collect();
        if gone.len() > GONE_MAX {
            let drop: Vec<String> = gone.iter().take(gone.len() - GONE_MAX).map(|i| self.devices[*i].id.clone()).collect();
            self.devices.retain(|d| !drop.contains(&d.id));
        }
        self.attempts.retain(|_, a| a.live(now));
        if self.attempts.len() > ADDRESSES_MAX {
            self.attempts.clear();
        }
    }

    fn find_token(&self, token: &str) -> Option<usize> {
        // every device is compared: where the match is must not show in the time
        let mut found = None;
        for (i, d) in self.devices.iter().enumerate() {
            if same(d.token.as_bytes(), token.as_bytes()) {
                found = Some(i);
            }
        }
        found
    }
}

impl Devices {
    pub fn new(cfg: &AppConfig) -> Self {
        let settings = Settings { full_allowed: cfg.tunnel_full_allowed, confirm_devices: cfg.tunnel_confirm_devices, idle_close_h: cfg.tunnel_idle_close_h };
        Self {
            inner: Mutex::new(Inner { key: None, devices: Vec::new(), busy: 0, attempts: HashMap::new(), everyone: Attempt::default(), notes: Vec::new() }),
            settings: Mutex::new(settings),
        }
    }

    pub fn settings(&self) -> Settings {
        *self.settings.lock().unwrap()
    }

    pub fn set_settings(&self, s: Settings) {
        *self.settings.lock().unwrap() = s;
        if !s.full_allowed {
            // the right to the full version goes with the setting
            let mut g = self.inner.lock().unwrap();
            let mut lines = Vec::new();
            for d in g.devices.iter_mut().filter(|d| d.scope == Scope::Full) {
                d.scope = Scope::Lite;
                lines.push(format!("устройство вернулось к лёгкой версии (полная выключена): {}", d.label));
            }
            g.notes.extend(lines);
        }
    }

    fn new_key(now: u64) -> Key {
        Key { value: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random(16)), expires: now + KEY_TTL_MS, used: false }
    }

    /// The gate opened: a first link, and nobody in.
    pub fn begin_at(&self, now: u64) {
        let mut g = self.inner.lock().unwrap();
        g.devices.clear();
        g.attempts.clear();
        g.everyone = Attempt::default();
        g.key = Some(Self::new_key(now));
        g.busy = now;
    }

    /// The gate closed or broke: no key and no session is left.
    pub fn end(&self) {
        let mut g = self.inner.lock().unwrap();
        g.key = None;
        g.devices.clear();
    }

    /// A new link: the old key is void, the address stays.
    pub fn rotate_at(&self, now: u64) {
        let mut g = self.inner.lock().unwrap();
        g.key = Some(Self::new_key(now));
        g.busy = now;
    }

    pub fn link_at(&self, now: u64) -> LinkInfo {
        let g = self.inner.lock().unwrap();
        match &g.key {
            None => LinkInfo { key: None, state: LinkState::None, expires_ms: None },
            Some(k) if k.used => LinkInfo { key: None, state: LinkState::Used, expires_ms: None },
            Some(k) if now >= k.expires => LinkInfo { key: None, state: LinkState::Expired, expires_ms: None },
            Some(k) => LinkInfo { key: Some(k.value.clone()), state: LinkState::Fresh, expires_ms: Some(k.expires) },
        }
    }

    /// A device knocks with a key.
    pub fn redeem_at(&self, now: u64, address: &str, key: &str, who: Who) -> Result<Admitted, RedeemError> {
        let confirm = self.settings().confirm_devices;
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        if let Some(s) = g.everyone.blocked(now).or_else(|| g.attempts.get(address).and_then(|a| a.blocked(now))) {
            return Err(RedeemError::TooMany(s));
        }
        let fresh = g.key.as_ref().filter(|k| !k.used && now < k.expires).map(|k| k.value.clone());
        // both sides of the `&` are always evaluated
        let right = same(key.as_bytes(), fresh.as_deref().unwrap_or("").as_bytes()) & fresh.is_some();
        if !right {
            g.attempts.entry(address.to_string()).or_default().fail(now, MAX_FAILS);
            g.everyone.fail(now, MAX_FAILS_ALL);
            return Err(RedeemError::Stale);
        }
        if g.devices.iter().filter(|d| is_live(d.state)).count() >= MAX_DEVICES {
            return Err(RedeemError::Full);
        }
        if let Some(k) = g.key.as_mut() {
            k.used = true;
        }
        g.attempts.remove(address);
        let state = if confirm { Life::Pending } else { Life::Active };
        let device = Device {
            id: hex(&random(6)),
            token: hex(&random(32)),
            state,
            scope: Scope::Lite,
            label: who.label,
            country: who.country,
            touch: who.touch,
            form: who.form,
            wrong: 0,
            since: now,
            last: now,
            ask_until: now + ASK_MS,
            gone_at: 0,
        };
        g.notes.push(match state {
            Life::Pending => format!("устройство просит войти: {}", device.label),
            _ => format!("устройство подключено: {}", device.label),
        });
        let admitted = Admitted { token: device.token.clone(), state, label: device.label.clone(), form: device.form };
        g.devices.push(device);
        g.busy = now;
        Ok(admitted)
    }

    pub fn status_at(&self, now: u64, token: Option<&str>) -> Status {
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        let Some(d) = token.and_then(|t| g.find_token(t)).map(|i| &g.devices[i]) else {
            return Status { state: "none", reason: None, label: None, form: Form::default(), raised: false };
        };
        let (label, form, raised) = (Some(d.label.clone()), d.form, d.scope == Scope::Full);
        match d.state {
            Life::Pending => Status { state: "pending", reason: None, label, form, raised },
            Life::Active => Status { state: "active", reason: None, label, form, raised },
            Life::Gone(w) => Status { state: "gone", reason: Some(w), label, form, raised },
        }
    }

    /// How a request with this cookie stands. A device that is in is touched: the idle clocks start over.
    pub fn standing_at(&self, now: u64, token: Option<&str>) -> Standing {
        self.standing(now, token, true)
    }

    /// The same, for a request that is a page looking at the screen and not someone using it (the light page's
    /// long-poll, which a forgotten tab repeats all night): the device stays in, the idle clocks run on.
    pub fn standing_quiet_at(&self, now: u64, token: Option<&str>) -> Standing {
        self.standing(now, token, false)
    }

    fn standing(&self, now: u64, token: Option<&str>, touch: bool) -> Standing {
        let Some(token) = token else { return Standing::Nobody };
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        let Some(i) = g.find_token(token) else { return Standing::Nobody };
        match g.devices[i].state {
            Life::Pending => Standing::Waiting,
            Life::Gone(_) => Standing::Nobody,
            Life::Active => {
                if touch {
                    g.devices[i].last = now;
                    g.busy = now;
                }
                match g.devices[i].scope {
                    Scope::Lite => Standing::Lite,
                    Scope::Full => Standing::Full,
                }
            }
        }
    }

    /// What a device that is in knows about its own session (no touch: asking who one is is not using the gate).
    pub fn session_at(&self, now: u64, token: &str) -> Option<SessionView> {
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        let d = g.find_token(token).map(|i| &g.devices[i]).filter(|d| d.state == Life::Active)?;
        Some(SessionView { label: d.label.clone(), scope: d.scope, form: d.form, since_ms: d.since, ends_ms: d.ends() })
    }

    /// The owner's decision on a device.
    pub fn decide_at(&self, now: u64, id: &str, op: Op) -> Result<(), DecideError> {
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        let i = g.devices.iter().position(|d| d.id == id && is_live(d.state)).ok_or(DecideError::NotFound)?;
        match op {
            Op::Allow => {
                if g.devices[i].state != Life::Pending {
                    return Err(DecideError::NotPending);
                }
                let d = &mut g.devices[i];
                d.state = Life::Active;
                d.since = now;
                d.last = now;
                let line = format!("устройство подключено: {}", d.label);
                g.notes.push(line);
            }
            Op::Deny => {
                if g.devices[i].state != Life::Pending {
                    return Err(DecideError::NotPending);
                }
                g.gone(i, Why::Denied, now);
            }
            Op::Revoke => g.gone(i, Why::Revoked, now),
        }
        g.busy = now;
        Ok(())
    }

    /// The device entered the planner's password: it has the full version from now on. `false`: no such device in, or the
    /// setting is off (the caller has checked it too; this is the last look).
    pub fn raise_at(&self, now: u64, token: &str) -> bool {
        let full_allowed = self.settings().full_allowed;
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        let Some(i) = g.find_token(token).filter(|i| g.devices[*i].state == Life::Active) else { return false };
        if !full_allowed {
            return false;
        }
        let d = &mut g.devices[i];
        d.wrong = 0;
        if d.scope == Scope::Full {
            return true;
        }
        d.scope = Scope::Full;
        let line = format!("устройство получило полную версию (пароль): {}", d.label);
        g.notes.push(line);
        g.busy = now;
        true
    }

    /// A wrong planner password from this device. Five in a row end its session (`true`): a device that guesses is not
    /// let go on.
    pub fn password_failed_at(&self, now: u64, token: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        let Some(i) = g.find_token(token).filter(|i| g.devices[*i].state == Life::Active) else { return false };
        g.devices[i].wrong += 1;
        if g.devices[i].wrong >= MAX_WRONG {
            g.gone(i, Why::Password, now);
            return true;
        }
        false
    }

    /// What kind of machine a device that is in is (without touching its clocks).
    pub fn form_at(&self, now: u64, token: &str) -> Option<Form> {
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        g.find_token(token).map(|i| &g.devices[i]).filter(|d| d.state == Life::Active).map(|d| d.form)
    }

    /// A device leaves by itself (the page's «Cancel»).
    pub fn leave_at(&self, now: u64, token: &str) {
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        if let Some(i) = g.find_token(token).filter(|i| is_live(g.devices[*i].state)) {
            g.gone(i, Why::Left, now);
        }
    }

    /// The devices the plaque lists: waiting ones first.
    pub fn list_at(&self, now: u64) -> Vec<DeviceView> {
        let mut g = self.inner.lock().unwrap();
        g.maintain(now);
        let mut out: Vec<DeviceView> = g
            .devices
            .iter()
            .filter(|d| is_live(d.state))
            .map(|d| {
                let pending = d.state == Life::Pending;
                DeviceView {
                    id: d.id.clone(),
                    label: d.label.clone(),
                    scope: d.scope,
                    state: if pending { "pending" } else { "active" },
                    country: d.country.clone(),
                    touch: d.touch,
                    form: d.form,
                    since_ms: d.since,
                    last_ms: d.last,
                    ends_ms: (!pending).then(|| d.ends()),
                    answer_by_ms: pending.then_some(d.ask_until),
                }
            })
            .collect();
        out.sort_by_key(|d| (d.state != "pending", d.since_ms));
        out
    }

    /// `(waiting, in)`.
    pub fn counts_at(&self, now: u64) -> (usize, usize) {
        let list = self.list_at(now);
        let waiting = list.iter().filter(|d| d.state == "pending").count();
        (waiting, list.len() - waiting)
    }

    /// The hours the gate may stay unused, if it has stood unused for that long.
    pub fn idle_due_at(&self, now: u64) -> Option<u32> {
        let h = self.settings().idle_close_h;
        let g = self.inner.lock().unwrap();
        (h > 0 && now.saturating_sub(g.busy) >= u64::from(h) * 3_600_000).then_some(h)
    }

    /// When the gate closes by itself if nobody uses it.
    pub fn idle_closes_at(&self) -> Option<u64> {
        let h = self.settings().idle_close_h;
        let g = self.inner.lock().unwrap();
        (h > 0 && g.key.is_some()).then(|| g.busy + u64::from(h) * 3_600_000)
    }

    /// The time moves on without anyone asking (the sweep).
    pub fn maintain_at(&self, now: u64) {
        self.inner.lock().unwrap().maintain(now);
    }

    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut self.inner.lock().unwrap().notes)
    }
}

/* ───────── who is knocking ───────── */

fn after<'a>(ua: &'a str, token: &str) -> Option<&'a str> {
    ua.find(token).map(|i| &ua[i + token.len()..])
}

fn major(rest: &str) -> String {
    rest.chars().take_while(|c| c.is_ascii_digit()).take(3).collect()
}

/// `iPhone · Safari 17`: only words and numbers this function itself knows, never the text of the header (it is the
/// client's own, and the owner's page shows the result).
pub fn device_label(ua: &str) -> String {
    let os = if ua.contains("iPhone") {
        "iPhone"
    } else if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("Android") {
        if ua.contains("Mobile") {
            "Android"
        } else {
            "Android tablet"
        }
    } else if ua.contains("CrOS") {
        "ChromeOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        "Mac"
    } else if ua.contains("Linux") || ua.contains("X11") {
        "Linux"
    } else {
        ""
    };
    let browser = [("EdgA/", "Edge"), ("EdgiOS/", "Edge"), ("Edg/", "Edge"), ("OPR/", "Opera"), ("YaBrowser/", "Yandex"), ("SamsungBrowser/", "Samsung Internet"), ("FxiOS/", "Firefox"), ("Firefox/", "Firefox"), ("CriOS/", "Chrome"), ("Chrome/", "Chrome")]
        .iter()
        .find_map(|(t, name)| after(ua, t).map(|r| (*name, major(r))))
        .or_else(|| {
            (ua.contains("Safari/")).then(|| ("Safari", after(ua, "Version/").map(major).unwrap_or_default()))
        });
    let browser = browser.map(|(n, v)| if v.is_empty() { n.to_string() } else { format!("{n} {v}") });
    match (os, browser) {
        ("", None) => "device".to_string(),
        ("", Some(b)) => b,
        (o, None) => o.to_string(),
        (o, Some(b)) => format!("{o} · {b}"),
    }
}

/// The two letters Cloudflare puts in `Cf-Ipcountry`, if they are letters of a country.
fn country_of(headers: &HeaderMap) -> Option<String> {
    let v = headers.get("cf-ipcountry")?.to_str().ok()?.trim().to_ascii_uppercase();
    (v.len() == 2 && v.bytes().all(|b| b.is_ascii_uppercase()) && v != "XX" && v != "T1").then_some(v)
}

fn who_from(headers: &HeaderMap, touch: bool) -> Who {
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let form = version::form_of(headers, touch);
    let mut label = device_label(ua);
    // an iPad says «Macintosh»; the owner should read what it is
    if form == Form::Tablet && label.starts_with("Mac ·") {
        label = label.replacen("Mac", "iPad", 1);
    }
    Who { label, country: country_of(headers), touch, form }
}

/// The address Cloudflare saw (`Cf-Connecting-Ip`), if it is one.
pub(crate) fn client_address(headers: &HeaderMap) -> Option<IpAddr> {
    headers.get("cf-connecting-ip").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<IpAddr>().ok())
}

/// The key of the limiter: that address, or one shared name when there is none.
fn address_of(headers: &HeaderMap) -> String {
    client_address(headers).map(|ip| ip.to_string()).unwrap_or_else(|| "-".into())
}

pub(crate) fn cookie_header(value: &str, max_age: u64) -> String {
    format!("{COOKIE}={value}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}")
}

pub(crate) fn with_cookie(mut res: Response, cookie: String) -> Response {
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

/* ───────── the journal, the page, the clock ───────── */

/// Tells the journal what the devices did since the last time, and the page how many wait.
pub async fn flush(app: &Shared) {
    for line in app.devices.take_notes() {
        crate::app::journal_note(app, "tunnel", "tunnel", &line).await;
    }
    let (waiting, active) = app.devices.counts_at(now_ms());
    app.bus.send("devices", json!({"waiting": waiting, "active": active}).to_string());
}

/// The clock: time-outs of devices, and the gate closing itself when it has stood unused too long.
pub async fn sweep_at(app: &Shared, now: u64) {
    app.devices.maintain_at(now);
    if matches!(app.tunnel.state(), Phase::Open | Phase::Reconnecting) {
        if let Some(h) = app.devices.idle_due_at(now) {
            tunnel::close_idle(app, h).await;
        }
    }
    flush(app).await;
}

pub async fn sweep(app: &Shared) {
    sweep_at(app, now_ms()).await;
}

const GATE_HTML: &str = include_str!("gate_page/gate.html");
const GATE_JS: &str = include_str!("gate_page/gate.js");
/// What a browser gets from a closed gate: a page with no script (the script is behind the gate too).
const CLOSED_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"color-scheme\" content=\"dark light\"><meta name=\"robots\" content=\"noindex\"><title>Herdr Planner</title><style>body{font:16px/1.5 system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0;padding:1rem;text-align:center}main{max-width:22rem}p{opacity:.7;font-size:.9rem}</style></head><body><main><h1>Доступ закрыт</h1><p>Хозяин закрыл шлюз или планировщик перезапустился. Нужна новая ссылка.</p><h1>Access closed</h1><p>The owner closed the gate or the planner restarted. A new link is needed.</p></main></body></html>";

/// The page a stranger gets at `/`.
pub fn gate_page() -> Response {
    Html(GATE_HTML).into_response()
}

pub fn closed_page() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Html(CLOSED_HTML)).into_response()
}

/// `GET /g/gate.js`
pub async fn gate_js() -> Response {
    ([(header::CONTENT_TYPE, HeaderValue::from_static("text/javascript; charset=utf-8"))], GATE_JS).into_response()
}

/* ───────── the device's side: the /g paths ───────── */

/// Whether a device may be given the full version at all: the gate's setting is on and the planner has a password.
pub fn can_full(app: &Shared) -> bool {
    app.devices.settings().full_allowed && app.cfg.auth_required()
}

/// Where a device that is in goes first: the light page or the planner (`version::choose`).
fn next_of(app: &Shared, headers: &HeaderMap, form: Form, raised: bool) -> &'static str {
    version::choose(form, version::preference(headers), raised, can_full(app)).path()
}

fn gate_open(app: &Shared) -> Option<Response> {
    (!app.gate.tunnel_open()).then(|| error(StatusCode::SERVICE_UNAVAILABLE, "the tunnel is closed"))
}

#[derive(Deserialize)]
pub struct RedeemBody {
    key: String,
    #[serde(default)]
    touch: bool,
}

/// `POST /g/redeem {key, touch}`: the key of the link for a session (and its cookie).
pub async fn redeem(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<RedeemBody>) -> Response {
    if let Some(r) = gate_open(&app) {
        return r;
    }
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let key = body.key.trim();
    let result = if key.len() > 64 {
        Err(RedeemError::Stale)
    } else {
        app.devices.redeem_at(now_ms(), &address_of(&headers), key, who_from(&headers, body.touch))
    };
    flush(&app).await;
    match result {
        Ok(a) => {
            let state = if a.state == Life::Pending { "pending" } else { "active" };
            let res = Json(json!({"state": state, "next": next_of(&app, &headers, a.form, false), "computer": crate::app::hostname(), "label": a.label})).into_response();
            with_cookie(res, cookie_header(&a.token, SESSION_MS / 1000))
        }
        Err(RedeemError::Stale) => error(StatusCode::GONE, "the link no longer works"),
        Err(RedeemError::Full) => error(StatusCode::CONFLICT, "too many devices"),
        Err(RedeemError::TooMany(s)) => {
            let mut res = (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": "too many attempts", "retry_s": s}))).into_response();
            if let Ok(v) = HeaderValue::from_str(&s.to_string()) {
                res.headers_mut().insert(header::RETRY_AFTER, v);
            }
            res
        }
    }
}

/// `GET /g/status`: what became of this device's request.
pub async fn status(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(r) = gate_open(&app) {
        return r;
    }
    let token = cookie_value(&headers, COOKIE);
    let s = app.devices.status_at(now_ms(), token.as_deref());
    // The name of this machine is for a device that knocked, not for whoever asks.
    let computer = (s.state != "none").then(crate::app::hostname);
    Json(json!({"state": s.state, "reason": s.reason, "label": s.label, "next": next_of(&app, &headers, s.form, s.raised), "computer": computer})).into_response()
}

/// `POST /g/cancel`: the device withdraws its request, or signs itself out.
pub async fn cancel(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(r) = gate_open(&app) {
        return r;
    }
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if let Some(t) = cookie_value(&headers, COOKIE) {
        app.devices.leave_at(now_ms(), &t);
    }
    flush(&app).await;
    with_cookie(Json(json!({"ok": true})).into_response(), cookie_header("", 0))
}

/* ───────── the owner's side: this machine only ───────── */

/// `POST /api/tunnel/rotate`: a new link; the address stays, the old key is void.
pub async fn tunnel_rotate(State(app): State<Shared>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    if let Some(r) = tunnel::guard(addr, &headers) {
        return r;
    }
    if !matches!(app.tunnel.state(), Phase::Open | Phase::Reconnecting) {
        return error(StatusCode::CONFLICT, "the gate is not open");
    }
    app.devices.rotate_at(now_ms());
    flush(&app).await;
    Json(tunnel::view(&app).await).into_response()
}

#[derive(Deserialize)]
pub struct SessionBody {
    id: String,
    op: Op,
}

/// `POST /api/tunnel/session {id, op: allow|deny|revoke}`
pub async fn tunnel_session(State(app): State<Shared>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<SessionBody>) -> Response {
    if let Some(r) = tunnel::guard(addr, &headers) {
        return r;
    }
    let done = app.devices.decide_at(now_ms(), &body.id, body.op);
    flush(&app).await;
    match done {
        Ok(()) => Json(tunnel::view(&app).await).into_response(),
        Err(DecideError::NotFound) => error(StatusCode::NOT_FOUND, "no such device"),
        Err(DecideError::NotPending) => error(StatusCode::CONFLICT, "the device is not waiting"),
    }
}

#[derive(Deserialize)]
pub struct SettingsBody {
    full_allowed: Option<bool>,
    confirm_devices: Option<bool>,
    idle_close_h: Option<u32>,
}

/// `POST /api/tunnel/settings {full_allowed?, confirm_devices?, idle_close_h?}`
pub async fn tunnel_settings(State(app): State<Shared>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<SettingsBody>) -> Response {
    if let Some(r) = tunnel::guard(addr, &headers) {
        return r;
    }
    let mut next = app.devices.settings();
    if let Some(h) = body.idle_close_h {
        if !IDLE_CLOSE_CHOICES.contains(&h) {
            return error(StatusCode::BAD_REQUEST, "idle_close_h is 0 (never), 1, 4 or 12");
        }
        next.idle_close_h = h;
    }
    if let Some(c) = body.confirm_devices {
        next.confirm_devices = c;
    }
    if let Some(f) = body.full_allowed {
        if f && !app.cfg.auth_required() {
            return error(StatusCode::CONFLICT, "the full planner through the gate needs a password: set one first");
        }
        next.full_allowed = f;
    }
    if let Err(e) = crate::config::set_tunnel_access(&app.cfg.config_file, next.confirm_devices, next.idle_close_h, next.full_allowed) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, format!("config.toml was not written: {e}"));
    }
    app.devices.set_settings(next);
    Json(tunnel::view(&app).await).into_response()
}

#[cfg(test)]
#[path = "devices_tests.rs"]
mod tests;
