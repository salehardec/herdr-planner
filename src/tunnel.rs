//! The gate's tunnel: the planner starts and stops its own `cloudflared`, and nothing else of Cloudflare's.
//!
//! Two modes (`tunnel.mode` in `config.toml`):
//!
//! - **quick**: `cloudflared --config <empty> tunnel --url http://127.0.0.1:<port>`. No domain; the address is a random
//!   `*.trycloudflare.com` at every start, read from the metrics endpoint `/quicktunnel` (the frame in the log is the
//!   second source). Quick tunnels have no SSE.
//! - **named**: `cloudflared --config <the owner's file> tunnel run`. The owner made the tunnel and the DNS record
//!   themselves (`cloudflared tunnel create`, `tunnel route dns`); the planner neither creates nor deletes them, it
//!   only starts and stops its own process. The address is fixed: `tunnel.hostname`, checked against the `ingress`
//!   host names of that file. If a tunnel with that config is already running (the owner started it by hand), it is
//!   shown as `external` and a second one is not started.
//!
//! The state machine: `closed → starting → open ⇄ reconnecting`, `open → closing → closed`, `failed` after a start
//! that did not work or a process that left, `external` for a tunnel somebody else runs. `open` means the metrics
//! endpoint `/ready` says there is a connection to Cloudflare. The address is told to the gate (`Gate::set_tunnel`)
//! when the tunnel opens and taken back before anything else when it closes. There is no restart on a failure: a new
//! start is a new address (quick), and nobody could tell the phone.
//!
//! The owner's `credentials-file` and `cert.pem` are never read here, and a log line of `cloudflared` that names
//! them is dropped before it is kept.
//!
//! The process dies with the planner: `kill_on_drop`, a stop from `shutdown_signal` (HTTP and TLS alike), and a pid
//! file (`cloudflared.pid`: the pid and its start time) that the next start uses to stop what a crash left behind.

use crate::app::Shared;
use crate::config::{now_ms, AppConfig, TunnelMode};
use crate::server::{error, refuse_remote, same_origin};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::watch;

/// How many lines of `cloudflared`'s output are kept.
const LOG_LINES: usize = 200;
/// How many of them go into an error.
const TAIL_LINES: usize = 20;
/// The usual places of the program, after `$PATH` (a service's `$PATH` is short).
const BIN_DIRS: [&str; 4] = ["~/.local/bin", "/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"];
/// A config of `cloudflared` is a few lines; a file much bigger than that is not one.
const CONFIG_MAX: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Closed,
    Starting,
    Open,
    /// The process is alive, but has no connection to Cloudflare (it reconnects to the same address).
    Reconnecting,
    Closing,
    Failed,
    /// A tunnel with the configured file is running, started by somebody else. Not ours, not opened for the gate.
    External,
}

impl Phase {
    fn running(self) -> bool {
        matches!(self, Phase::Starting | Phase::Open | Phase::Reconnecting | Phase::Closing)
    }
}

/// Why a tunnel is not up. `kind` is for the page (it has the words); `message` says it in English with the details.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Problem {
    /// `not-installed`, `config-missing`, `no-hostname`, `hostname-mismatch`, `spawn`, `timeout`, `exited`, `lost`.
    pub kind: &'static str,
    pub message: String,
    pub log_tail: Vec<String>,
}

impl Problem {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), log_tail: Vec::new() }
    }
}

/// The settings the page changes while the planner runs (`[tunnel]` of `config.toml` at the start).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub mode: TunnelMode,
    /// The `cloudflared` config of a named tunnel.
    pub config: Option<PathBuf>,
    /// The address of a named tunnel; empty: the first host name in the config.
    pub hostname: Option<String>,
}

/// How long things take. The tests make them short.
#[derive(Clone, Debug)]
pub struct Timing {
    /// Nothing ready after this long: the start failed.
    pub start: Duration,
    pub poll_start: Duration,
    pub poll_open: Duration,
    /// No connection this long: the tunnel is closed (it is not coming back).
    pub lost: Duration,
    /// SIGTERM, then SIGKILL after this long, when the owner closes the tunnel.
    pub term_grace: Duration,
    /// The same when the planner itself is stopping: its own exit is two seconds away.
    pub shutdown_grace: Duration,
    /// A quick tunnel that Cloudflare refused is asked for once more after this.
    pub retry_pause: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            start: Duration::from_secs(45),
            poll_start: Duration::from_secs(1),
            poll_open: Duration::from_secs(5),
            lost: Duration::from_secs(120),
            term_grace: Duration::from_secs(5),
            shutdown_grace: Duration::from_millis(1500),
            retry_pause: Duration::from_secs(3),
        }
    }
}

struct Inner {
    state: Phase,
    since: u64,
    /// The address, once known. Told to the page only through the local API.
    host: Option<String>,
    problem: Option<Problem>,
    log: VecDeque<String>,
    /// A supervisor task is alive (it owns the process).
    active: bool,
    external: Option<u32>,
    /// `--version` of the program, with the path it was asked of.
    version: Option<(PathBuf, String)>,
}

pub struct Tunnel {
    inner: Mutex<Inner>,
    settings: Mutex<Settings>,
    timing: Mutex<Timing>,
    /// Where a `cloudflared` started without `--config` reads its config from (see `default_config_files`).
    defaults: Mutex<Vec<PathBuf>>,
    /// Sent to: the supervisor stops its process. The value is the grace, in milliseconds.
    stop: watch::Sender<u64>,
}

impl Tunnel {
    pub fn new(cfg: &AppConfig) -> Self {
        let settings = Settings { mode: cfg.tunnel_mode, config: cfg.tunnel_config.clone(), hostname: cfg.tunnel_hostname.clone() };
        Self {
            inner: Mutex::new(Inner {
                state: Phase::Closed,
                since: now_ms(),
                host: None,
                problem: None,
                log: VecDeque::new(),
                active: false,
                external: None,
                version: None,
            }),
            settings: Mutex::new(settings),
            timing: Mutex::new(Timing::default()),
            defaults: Mutex::new(default_config_files()),
            stop: watch::channel(0).0,
        }
    }

    pub fn state(&self) -> Phase {
        self.inner.lock().unwrap().state
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn timing(&self) -> Timing {
        self.timing.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub fn set_default_configs(&self, files: Vec<PathBuf>) {
        *self.defaults.lock().unwrap() = files;
    }

    #[cfg(test)]
    pub fn set_timing(&self, timing: Timing) {
        *self.timing.lock().unwrap() = timing;
    }

    fn push_log(&self, line: &str) {
        let Some(line) = clean_line(line) else { return };
        let mut g = self.inner.lock().unwrap();
        if g.log.len() >= LOG_LINES {
            g.log.pop_front();
        }
        g.log.push_back(line);
    }

    /// The lines that say what went wrong: errors and warnings (the empty-config line of a quick tunnel is none).
    fn tail(&self) -> Vec<String> {
        let g = self.inner.lock().unwrap();
        let bad: Vec<&String> = g.log.iter().filter(|l| (l.contains(" ERR ") || l.contains(" WRN ")) && !l.contains("was empty")).collect();
        let pick: Vec<&String> = if bad.is_empty() { g.log.iter().rev().take(5).rev().collect() } else { bad };
        pick.iter().rev().take(TAIL_LINES).rev().map(|s| s.to_string()).collect()
    }
}

/* ───────── what the page may know ───────── */

/// What `GET /api/tunnel` says. Only for this machine: `host` is the tunnel's address.
#[derive(Serialize)]
pub struct View {
    pub installed: bool,
    pub bin: Option<String>,
    pub version: Option<String>,
    pub mode: &'static str,
    /// The settings as they stand (the `cloudflared` config and the address of a named tunnel).
    pub config: Option<String>,
    pub hostname: Option<String>,
    /// For a named tunnel: what the config file says. `ok: false` carries the problem a start would hit.
    pub config_check: Option<ConfigCheck>,
    pub state: Phase,
    pub since_ms: u64,
    pub host: Option<String>,
    /// `https://<host>/#k=<key>` while the tunnel is up and the key is good; the bare address after that.
    pub link: Option<String>,
    /// `fresh` (good for one device), `used`, `expired` or `none` (no gate open).
    pub link_state: crate::devices::LinkState,
    /// When the key stops working.
    pub link_expires_ms: Option<u64>,
    pub settings: crate::devices::Settings,
    /// A password is set: only then the full planner through the gate is possible.
    pub password_set: bool,
    /// The devices that wait for «Let in» and those that are in.
    pub sessions: Vec<crate::devices::DeviceView>,
    /// When the gate closes by itself, if nobody uses it.
    pub idle_closes_ms: Option<u64>,
    pub error: Option<Problem>,
    /// The pid of a tunnel somebody else runs (`state` is `external`).
    pub external_pid: Option<u32>,
    /// Whether the server-sent events work through this tunnel (a quick tunnel has none).
    pub sse: bool,
    /// The QR code of `link` as an SVG, while the link carries a good key (made here: the key goes to nobody else).
    pub qr_svg: Option<String>,
    /// The planner's port and this user's home: the page writes them into the commands and the config it shows.
    pub port: u16,
    pub home: String,
}

#[derive(Serialize)]
pub struct ConfigCheck {
    pub ok: bool,
    pub problem: Option<Problem>,
    /// The host names under `ingress` (wildcards included), the address that would be used.
    pub hosts: Vec<String>,
    pub host: Option<String>,
    /// What «Check» in the page shows: one line per thing the config must have, passed or not. Nothing here comes from
    /// a secret; the `credentials-file` is only named, never opened.
    pub checks: Vec<Check>,
}

/// One thing about the config of a named tunnel. `id` is for the page (it has the words): `file`, `hostname`,
/// `tunnel`, `credentials`, `credentials-file`, `service`, `catch-all`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Check {
    pub id: &'static str,
    pub ok: bool,
    /// A check that failed but does not stop the tunnel (it may still run) is a warning, not an error.
    pub warn: bool,
    /// What the config has (a file name, an address, a port) and what it should have; either may be empty.
    pub have: Option<String>,
    pub want: Option<String>,
}

impl Check {
    fn new(id: &'static str, ok: bool, warn: bool, have: Option<String>, want: Option<String>) -> Self {
        Self { id, ok, warn, have, want }
    }
}

pub async fn view(app: &Shared) -> View {
    let settings = app.tunnel.settings();
    let bin = find_bin(&app.cfg);
    if app.tunnel.state() == Phase::External {
        // Somebody else's tunnel may be gone by now.
        recheck_external(app);
    }
    let version = match &bin {
        Some(b) => version_of(app, b).await,
        None => None,
    };
    let config_check = (settings.mode == TunnelMode::Named).then(|| {
        let checks = config_checks(&settings, app.cfg.port);
        match check_named(&settings) {
            Ok(n) => ConfigCheck { ok: true, problem: None, hosts: n.hosts, host: Some(n.host), checks },
            Err((p, hosts)) => ConfigCheck { ok: false, problem: Some(p), hosts, host: None, checks },
        }
    });
    let now = now_ms();
    let link = app.devices.link_at(now);
    let sessions = app.devices.list_at(now);
    let g = app.tunnel.inner.lock().unwrap();
    let up = matches!(g.state, Phase::Open | Phase::Reconnecting);
    let link_text = g.host.as_ref().filter(|_| up).map(|h| match &link.key {
        Some(k) => format!("https://{h}/#k={k}"),
        None => format!("https://{h}"),
    });
    let qr_svg = link.key.as_ref().and(link_text.as_deref()).and_then(qr_svg);
    View {
        installed: bin.is_some(),
        bin: bin.map(|b| b.display().to_string()),
        version,
        mode: settings.mode.as_str(),
        config: settings.config.map(|p| p.display().to_string()),
        hostname: settings.hostname,
        config_check,
        state: g.state,
        since_ms: g.since,
        host: g.host.clone(),
        link: link_text,
        link_state: link.state,
        link_expires_ms: link.expires_ms.filter(|_| up),
        settings: app.devices.settings(),
        password_set: app.cfg.auth_required(),
        sessions: if up { sessions } else { Vec::new() },
        idle_closes_ms: app.devices.idle_closes_at().filter(|_| up),
        error: g.problem.clone(),
        external_pid: g.external,
        sse: settings.mode == TunnelMode::Named,
        qr_svg,
        port: app.cfg.port,
        home: crate::config::home().display().to_string(),
    }
}

/// The QR code of a link as an SVG: one path of modules and a four-module quiet zone, no program and no network.
pub fn qr_svg(text: &str) -> Option<String> {
    use qrcode::{render::svg, EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).ok()?;
    let svg = code.render::<svg::Color>().min_dimensions(1, 1).quiet_zone(true).dark_color(svg::Color("#000")).light_color(svg::Color("#fff")).build();
    // the XML header is not for a page that holds the picture inline
    svg.find("<svg").map(|i| svg[i..].to_string())
}

/* ───────── the program ───────── */

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Where `cloudflared` is: `HERDR_PLANNER_CLOUDFLARED` (the checks use it), `tunnel.bin`, then `$PATH` and the usual
/// folders.
pub fn find_bin(cfg: &AppConfig) -> Option<PathBuf> {
    let named = std::env::var("HERDR_PLANNER_CLOUDFLARED").ok().filter(|v| !v.trim().is_empty()).or_else(|| cfg.tunnel_bin.clone());
    let name = named.as_deref().map(str::trim).unwrap_or("cloudflared");
    if name.contains('/') {
        let p = crate::config::expand_home(name);
        return is_executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    dirs.extend(BIN_DIRS.iter().map(|d| crate::config::expand_home(d)));
    dirs.into_iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

async fn version_of(app: &Shared, bin: &Path) -> Option<String> {
    if let Some((p, v)) = &app.tunnel.inner.lock().unwrap().version {
        if p == bin {
            return Some(v.clone());
        }
    }
    let out = tokio::time::timeout(Duration::from_secs(3), Command::new(bin).arg("--version").stdin(Stdio::null()).kill_on_drop(true).output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line: String = text.lines().next()?.trim().chars().take(120).collect();
    if line.is_empty() {
        return None;
    }
    app.tunnel.inner.lock().unwrap().version = Some((bin.to_path_buf(), line.clone()));
    Some(line)
}

/* ───────── the owner's config of a named tunnel ───────── */

/// The `hostname` values under `ingress:` of a `cloudflared` config. Nothing else is taken from the file: its
/// `credentials-file` is a path the planner does not follow, and `cert.pem` is not mentioned there at all.
pub fn ingress_hosts(text: &str) -> Vec<String> {
    let mut in_ingress = false;
    let mut hosts: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = match raw.find('#') {
            Some(0) => "",
            Some(i) if raw[..i].ends_with(char::is_whitespace) => &raw[..i],
            _ => raw,
        };
        let body = line.trim();
        if body.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 && !body.starts_with('-') {
            in_ingress = body.strip_prefix("ingress:").is_some();
            continue;
        }
        if !in_ingress {
            continue;
        }
        let body = body.trim_start_matches('-').trim_start();
        if let Some(value) = body.strip_prefix("hostname:") {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            if let Some(h) = crate::config::tunnel_host(value).filter(|h| valid_host(h)) {
                if !hosts.contains(&h) {
                    hosts.push(h);
                }
            }
        }
    }
    hosts
}

/// A DNS name, with a leading `*.` allowed (an ingress rule for a whole zone).
fn valid_host(h: &str) -> bool {
    let name = h.strip_prefix("*.").unwrap_or(h);
    !name.is_empty()
        && name.len() <= 253
        && !name.starts_with(['.', '-'])
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Whether an ingress rule (`planner.example.org`, `*.example.org`) covers a host.
fn covers(rule: &str, host: &str) -> bool {
    match rule.strip_prefix("*.") {
        Some(zone) => host.len() > zone.len() + 1 && host.ends_with(zone) && host[..host.len() - zone.len()].ends_with('.'),
        None => rule == host,
    }
}

struct Named {
    /// All the host names of the config.
    hosts: Vec<String>,
    /// The tunnel's address.
    host: String,
}

/// Reads the config of a named tunnel and finds the address. The errors a start would hit, said before it starts:
/// the file is not there, it has no `hostname`, the address in the settings is not one of its host names.
fn check_named(settings: &Settings) -> Result<Named, (Problem, Vec<String>)> {
    let none = |p: Problem| (p, Vec::new());
    let Some(path) = &settings.config else {
        return Err(none(Problem::new("config-missing", "tunnel.config is not set: the path of the cloudflared config of the tunnel")));
    };
    let meta = std::fs::metadata(path).map_err(|e| none(Problem::new("config-missing", format!("cloudflared config not found: {} ({})", path.display(), e.kind()))))?;
    if !meta.is_file() || meta.len() > CONFIG_MAX {
        return Err(none(Problem::new("config-missing", format!("{} is not a cloudflared config", path.display()))));
    }
    let text = std::fs::read_to_string(path).map_err(|e| none(Problem::new("config-missing", format!("cloudflared config cannot be read: {} ({})", path.display(), e.kind()))))?;
    let hosts = ingress_hosts(&text);
    if hosts.is_empty() {
        let p = Problem::new("no-hostname", format!("{} has no `hostname` under `ingress`: the tunnel has no address", path.display()));
        return Err((p, hosts));
    }
    match &settings.hostname {
        Some(want) if hosts.iter().any(|r| covers(r, want)) => Ok(Named { host: want.clone(), hosts }),
        Some(want) => {
            let p = Problem::new("hostname-mismatch", format!("{want} is not among the `ingress` host names of {}: {}", path.display(), hosts.join(", ")));
            Err((p, hosts))
        }
        None => match hosts.iter().find(|h| !h.starts_with("*.")).cloned() {
            Some(host) => Ok(Named { host, hosts }),
            None => {
                let p = Problem::new("no-hostname", format!("{} names only wildcard hosts: set tunnel.hostname", path.display()));
                Err((p, hosts))
            }
        },
    }
}

/// The value of a key at the top of a `cloudflared` config (`tunnel: …`), without comments and quotes.
fn top_value(text: &str, key: &str) -> Option<String> {
    for raw in text.lines() {
        if raw.starts_with([' ', '\t', '#', '-']) {
            continue;
        }
        let line = match raw.find(" #") {
            Some(i) => &raw[..i],
            None => raw,
        };
        if let Some(v) = line.trim().strip_prefix(key).and_then(|r| r.strip_prefix(':')) {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            return (!v.is_empty()).then(|| v.to_string());
        }
    }
    None
}

/// The `service` values under `ingress:`, in order.
fn ingress_services(text: &str) -> Vec<String> {
    let mut in_ingress = false;
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = match raw.find(" #") {
            Some(i) => &raw[..i],
            None => raw,
        };
        let body = line.trim();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 && !body.starts_with('-') {
            in_ingress = body.starts_with("ingress:");
            continue;
        }
        if !in_ingress {
            continue;
        }
        if let Some(v) = body.trim_start_matches('-').trim_start().strip_prefix("service:") {
            out.push(v.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    out
}

/// The port of `http://127.0.0.1:7117` (also `localhost`, `[::1]`).
fn local_port(service: &str) -> Option<u16> {
    let rest = service.strip_prefix("http://").or_else(|| service.strip_prefix("https://"))?;
    let hostport = rest.split('/').next()?;
    let (host, port) = hostport.rsplit_once(':')?;
    matches!(host, "127.0.0.1" | "localhost" | "[::1]").then(|| port.parse().ok()).flatten()
}

/// Whether `s` is a tunnel's id (a UUID: 8-4-4-4-12 hex digits).
fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5 && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// What the page's «Check» shows for the config of a named tunnel: the file is there, it names the address, the tunnel's id
/// and the credentials file agree (by the file's *name*; the file itself is not opened), the tunnel leads to the planner's
/// port and ends with the catch-all rule. `credentials_exist` says whether a path is a file (the planner only asks, it does
/// not read it).
pub fn checks_of(text: &str, hostname: Option<&str>, port: u16, credentials_exist: &dyn Fn(&str) -> bool) -> Vec<Check> {
    let mut out = vec![Check::new("file", true, false, None, None)];
    let hosts = ingress_hosts(text);
    let host_ok = match hostname {
        Some(want) => hosts.iter().any(|r| covers(r, want)),
        None => hosts.iter().any(|h| !h.starts_with("*.")),
    };
    out.push(Check::new("hostname", host_ok, false, hosts.first().cloned(), hostname.map(str::to_string)));
    let tunnel = top_value(text, "tunnel");
    out.push(Check::new("tunnel", tunnel.is_some(), false, tunnel.clone(), None));
    let creds = top_value(text, "credentials-file");
    let stem = creds.as_deref().map(|c| {
        let name = c.rsplit('/').next().unwrap_or(c);
        name.strip_suffix(".json").unwrap_or(name).to_string()
    });
    match (&creds, &stem, &tunnel) {
        (None, ..) => out.push(Check::new("credentials", false, false, None, tunnel.clone())),
        (Some(_), Some(stem), Some(id)) if is_uuid(id) => out.push(Check::new("credentials", stem.eq_ignore_ascii_case(id), false, Some(stem.clone()), Some(id.clone()))),
        // the tunnel is named, not given by its id: nothing to compare the file's name with
        (Some(_), Some(stem), _) => out.push(Check::new("credentials", true, false, Some(stem.clone()), None)),
        _ => {}
    }
    if let Some(c) = &creds {
        // whether it is there, not where: the path of the owner's secret is not for the page
        out.push(Check::new("credentials-file", credentials_exist(c), true, None, None));
    }
    let services = ingress_services(text);
    let ports: Vec<u16> = services.iter().filter_map(|s| local_port(s)).collect();
    out.push(Check::new("service", ports.contains(&port), true, ports.first().map(|p| p.to_string()), Some(port.to_string())));
    out.push(Check::new("catch-all", services.last().is_some_and(|s| s == "http_status:404"), true, None, None));
    out
}

/// `checks_of` for the settings as they stand; a file that cannot be read is the one failed check.
fn config_checks(settings: &Settings, port: u16) -> Vec<Check> {
    let Some(path) = &settings.config else {
        return vec![Check::new("file", false, false, None, None)];
    };
    let text = match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() <= CONFIG_MAX => std::fs::read_to_string(path).ok(),
        _ => None,
    };
    let Some(text) = text else {
        return vec![Check::new("file", false, false, Some(path.display().to_string()), None)];
    };
    checks_of(&text, settings.hostname.as_deref(), port, &|c| {
        let p = crate::config::expand_home(c);
        // a relative path is read from wherever cloudflared runs: not ours to judge
        !p.is_absolute() || p.metadata().is_ok_and(|m| m.is_file())
    })
}

/* ───────── the process table ───────── */

/// Where processes are read from: `/proc` where there is one (exact arguments, exact start time), else `ps`, which
/// macOS has (the arguments come as one line, so a path with spaces in it is not matched).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Proc,
    Ps,
}

fn backend() -> Backend {
    if cfg!(target_os = "linux") && Path::new("/proc/self/stat").exists() {
        Backend::Proc
    } else {
        Backend::Ps
    }
}

struct Proc {
    pid: u32,
    args: Vec<String>,
}

fn processes(b: Backend) -> Vec<Proc> {
    match b {
        Backend::Proc => std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let pid = e.file_name().to_str()?.parse::<u32>().ok()?;
                let raw = std::fs::read(e.path().join("cmdline")).ok()?;
                let args = raw.split(|c| *c == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
                Some(Proc { pid, args })
            })
            .collect(),
        Backend::Ps => ps(&["-axo", "pid=,command="])
            .map(|text| {
                text.lines()
                    .filter_map(|l| {
                        let mut it = l.split_whitespace();
                        let pid = it.next()?.parse().ok()?;
                        Some(Proc { pid, args: it.map(String::from).collect() })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The output of `ps` (the same words on Linux and macOS), if it ran.
fn ps(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("ps").args(args).env("LC_ALL", "C").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (!text.trim().is_empty()).then_some(text)
}

fn ps_field(pid: u32, field: &str) -> Option<String> {
    ps(&["-o", &format!("{field}="), "-p", &pid.to_string()]).map(|t| t.trim().to_string())
}

/// What names one process and not a later one that got the same pid: the start time (clock ticks since boot from
/// `/proc`, the `lstart` text from `ps`).
fn proc_start_with(b: Backend, pid: u32) -> Option<String> {
    match b {
        Backend::Proc => {
            let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            crate::strays::parse_stat(&text).map(|s| s.4.to_string())
        }
        Backend::Ps => ps_field(pid, "lstart"),
    }
}

fn proc_alive_with(b: Backend, pid: u32) -> bool {
    // A zombie is gone for our purposes.
    match b {
        Backend::Proc => std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|t| crate::strays::parse_stat(&t)).is_some_and(|s| s.1 != 'Z'),
        Backend::Ps => ps_field(pid, "stat").is_some_and(|s| !s.starts_with('Z')),
    }
}

fn proc_cmdline_with(b: Backend, pid: u32) -> String {
    match b {
        Backend::Proc => String::from_utf8_lossy(&std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default()).replace('\0', " "),
        Backend::Ps => ps_field(pid, "command").unwrap_or_default(),
    }
}

#[cfg(test)]
fn proc_alive(pid: u32) -> bool {
    proc_alive_with(backend(), pid)
}

/* ───────── a tunnel somebody else started ───────── */

/// Where `cloudflared` looks for its config when it is not told: the first of these that exists.
pub fn default_config_files() -> Vec<PathBuf> {
    let dirs = ["~/.cloudflared", "~/.cloudflare-warp", "~/cloudflare-warp", "/etc/cloudflared", "/usr/local/etc/cloudflared"];
    dirs.iter().flat_map(|d| ["config.yml", "config.yaml"].map(|f| crate::config::expand_home(d).join(f))).collect()
}

/// What tells a tunnel somebody started from the one this config is for.
struct Wanted {
    config: PathBuf,
    /// The `tunnel:` of the config (an id or a name): `cloudflared tunnel run <it>` is the same tunnel.
    tunnel: Option<String>,
    /// Where a `cloudflared` without `--config` reads its config from.
    defaults: Vec<PathBuf>,
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y)
}

/// Whether these arguments are a `cloudflared … run` of the tunnel `want` describes: it names this config, or names
/// none and reads this one by default, or names this tunnel.
fn runs_tunnel(args: &[String], want: &Wanted) -> bool {
    let program = args.iter().take(2).any(|a| a.rsplit('/').next().is_some_and(|n| n.contains("cloudflared")));
    let Some(run) = args.iter().position(|a| a == "run") else { return false };
    if !program {
        return false;
    }
    let mut explicit = false;
    let mut uses = false;
    for (i, a) in args.iter().enumerate() {
        let value = match a.strip_prefix("--config=") {
            Some(v) => Some(v),
            None if a == "--config" || a == "-c" => args.get(i + 1).map(String::as_str),
            None => None,
        };
        if let Some(v) = value {
            explicit = true;
            uses |= same_file(&crate::config::expand_home(v), &want.config);
        }
    }
    if !explicit {
        uses = want.defaults.iter().find(|d| d.is_file()).is_some_and(|d| same_file(d, &want.config));
    }
    let by_name = want.tunnel.as_deref().is_some_and(|t| args[run + 1..].iter().any(|a| a == t));
    uses || by_name
}

fn external_in(procs: &[Proc], want: &Wanted, own: Option<u32>, b: Backend) -> Option<u32> {
    let me = std::process::id();
    procs.iter().find(|p| p.pid != me && Some(p.pid) != own && runs_tunnel(&p.args, want) && proc_alive_with(b, p.pid)).map(|p| p.pid)
}

/// The `tunnel:` line of a config (an id or a name), and nothing else of it.
fn config_tunnel(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("tunnel:")).map(|v| v.split('#').next().unwrap_or("").trim().trim_matches(|c| c == '"' || c == '\'').to_string()).filter(|v| !v.is_empty())
}

fn wanted(app: &Shared, config: &Path) -> Wanted {
    let tunnel = std::fs::metadata(config).ok().filter(|m| m.len() <= CONFIG_MAX).and_then(|_| std::fs::read_to_string(config).ok()).and_then(|t| config_tunnel(&t));
    Wanted { config: config.to_path_buf(), tunnel, defaults: app.tunnel.defaults.lock().unwrap().clone() }
}

/// The pid of a running `cloudflared … run` of this tunnel, if it is not ours.
fn find_external(app: &Shared, config: &Path, own: Option<u32>) -> Option<u32> {
    let b = backend();
    external_in(&processes(b), &wanted(app, config), own, b)
}

fn recheck_external(app: &Shared) {
    let settings = app.tunnel.settings();
    let still = settings.config.as_deref().and_then(|c| find_external(app, c, None));
    let mut g = app.tunnel.inner.lock().unwrap();
    if g.state == Phase::External && still.is_none() {
        g.state = Phase::Closed;
        g.since = now_ms();
        g.external = None;
        g.host = None;
    }
}

/* ───────── the processes ───────── */

#[cfg(unix)]
fn signal(pid: u32, sig: i32) {
    // SAFETY: kill(2) takes two integers and touches no memory; a pid that has gone is an error we ignore.
    unsafe {
        libc::kill(pid as i32, sig);
    }
}

#[cfg(not(unix))]
fn signal(_: u32, _: i32) {}

fn pid_file(cfg: &AppConfig) -> PathBuf {
    cfg.state_dir.join("cloudflared.pid")
}

fn write_pid_file(cfg: &AppConfig, pid: u32) {
    write_pid_file_with(backend(), cfg, pid);
}

/// `<pid> <start time>`: the start time is what tells the process from a later one with the same pid.
fn write_pid_file_with(b: Backend, cfg: &AppConfig, pid: u32) {
    let _ = std::fs::create_dir_all(&cfg.state_dir);
    let _ = std::fs::write(pid_file(cfg), format!("{pid} {}\n", proc_start_with(b, pid).unwrap_or_default()));
}

/// At the planner's start: a `cloudflared` it started before and a crash left running is stopped. It is stopped only
/// when the pid and the start time are the recorded ones and the process is still a `cloudflared`; a pid that was
/// reused by something else is left alone.
pub async fn reap_orphan(cfg: &AppConfig) -> Option<u32> {
    reap_orphan_with(backend(), cfg).await
}

async fn reap_orphan_with(b: Backend, cfg: &AppConfig) -> Option<u32> {
    let file = pid_file(cfg);
    let text = std::fs::read_to_string(&file).ok()?;
    let _ = std::fs::remove_file(&file);
    let (pid, start) = text.trim().split_once(' ')?;
    let pid: u32 = pid.parse().ok()?;
    let start = start.trim();
    if start.is_empty() || start == "0" || proc_start_with(b, pid).as_deref() != Some(start) || !proc_alive_with(b, pid) {
        return None;
    }
    if !proc_cmdline_with(b, pid).contains("cloudflared") {
        return None;
    }
    signal(pid, libc::SIGTERM);
    let end = Instant::now() + Duration::from_secs(3);
    while proc_alive_with(b, pid) && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if proc_alive_with(b, pid) {
        signal(pid, libc::SIGKILL);
    }
    Some(pid)
}

/// SIGTERM, then SIGKILL when the process has not left within `grace`.
async fn terminate(child: &mut Child, grace: Duration) {
    if let Some(pid) = child.id() {
        signal(pid, libc::SIGTERM);
    }
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
}

/* ───────── metrics ───────── */

/// `GET` on the metrics port of `cloudflared`: the status and the body, or `None` when nobody answers.
async fn metrics_get(port: u16, path: &str) -> Option<(u16, String)> {
    let work = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        s.write_all(format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes()).await.ok()?;
        let mut raw = Vec::new();
        s.take(64 * 1024).read_to_end(&mut raw).await.ok()?;
        let text = String::from_utf8_lossy(&raw).into_owned();
        let status = text.split(' ').nth(1)?.parse().ok()?;
        Some((status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default()))
    };
    tokio::time::timeout(Duration::from_secs(2), work).await.ok().flatten()
}

/// `/ready` is 200 with `readyConnections` ≥ 1 once the tunnel is connected.
async fn is_ready(port: u16) -> bool {
    match metrics_get(port, "/ready").await {
        Some((200, body)) => serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v.get("readyConnections").and_then(|n| n.as_u64())).map_or(true, |n| n >= 1),
        _ => false,
    }
}

async fn quick_host_from_metrics(port: u16) -> Option<String> {
    let (200, body) = metrics_get(port, "/quicktunnel").await? else { return None };
    let host = serde_json::from_str::<serde_json::Value>(&body).ok()?.get("hostname")?.as_str()?.to_lowercase();
    quick_name(&host).then_some(host)
}

/// `abc-def.trycloudflare.com`, but not the service's own `api.trycloudflare.com`.
fn quick_name(host: &str) -> bool {
    host.strip_suffix(".trycloudflare.com").is_some_and(|label| !label.is_empty() && label != "api" && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// The address in a line of `cloudflared`'s frame («Your quick Tunnel has been created!»).
pub fn quick_host_in_line(line: &str) -> Option<String> {
    let rest = &line[line.find("https://")? + "https://".len()..];
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.')).unwrap_or(rest.len());
    let host = rest[..end].to_lowercase();
    quick_name(&host).then_some(host)
}

/// A line of the program's output as it is kept: control characters out, short, and nothing that names the owner's
/// credentials.
fn clean_line(line: &str) -> Option<String> {
    let line: String = line.chars().filter(|c| !c.is_control()).collect();
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let low = line.to_lowercase();
    if ["credentials", "cert.pem", "tunnelsecret", "origincert", "token"].iter().any(|w| low.contains(w)) {
        return Some("(a line about credentials is not kept)".to_string());
    }
    Some(line.chars().take(300).collect())
}

/* ───────── changing state ───────── */

/// Moves to `to`, lets `edit` change the rest, and tells the gate, the page and the journal. The gate hears about the
/// address only while the tunnel is up, and first of all when it is not.
async fn change(app: &Shared, to: Phase, edit: impl FnOnce(&mut Inner)) {
    change_from(app, None, to, edit).await;
}

/// The step of a running tunnel: only from the state the supervisor saw. An owner who closed the tunnel while the
/// supervisor was asking `/ready` is not overruled by its answer.
async fn advance(app: &Shared, from: Phase, to: Phase) {
    change_from(app, Some(from), to, |_| {}).await;
}

async fn change_from(app: &Shared, only_from: Option<Phase>, to: Phase, edit: impl FnOnce(&mut Inner)) {
    let (host, text, from) = {
        let mut g = app.tunnel.inner.lock().unwrap();
        let from = g.state;
        if only_from.is_some_and(|f| f != from) {
            return;
        }
        g.state = to;
        g.since = now_ms();
        edit(&mut g);
        let text = match to {
            Phase::Open if from != Phase::Reconnecting => Some(format!("шлюз открыт ({})", if app.tunnel.settings().mode == TunnelMode::Named { "именованный туннель" } else { "быстрый туннель" })),
            Phase::Closed if from == Phase::Closing => Some("шлюз закрыт".to_string()),
            Phase::Failed => Some(format!("шлюз не работает: {}", g.problem.as_ref().map(|p| p.message.as_str()).unwrap_or("?"))),
            _ => None,
        };
        (g.host.clone().filter(|_| matches!(to, Phase::Open | Phase::Reconnecting)), text, from)
    };
    app.gate.set_tunnel(host.as_deref());
    // A gate that has just opened has a first link and nobody in; one that is not open has no key and no session.
    match to {
        Phase::Open if from != Phase::Reconnecting => app.devices.begin_at(now_ms()),
        Phase::Open | Phase::Reconnecting => {}
        _ => app.devices.end(),
    }
    // The bus reaches every subscriber, so it carries the state and nothing else.
    app.bus.send("tunnel", json!({"state": to}).to_string());
    if let Some(text) = text {
        crate::app::journal_note(app, "tunnel", "tunnel", &text).await;
    }
}

/* ───────── opening ───────── */

struct Launch {
    bin: PathBuf,
    args: Vec<String>,
    metrics: u16,
    /// A named tunnel's address; a quick tunnel's is found while it starts.
    host: Option<String>,
    quick: bool,
}

enum Prepared {
    Run(Launch),
    External(u32, String),
}

fn free_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?.local_addr().ok().map(|a| a.port())
}

/// The empty config that keeps `cloudflared` from reading `config.yml` (which may be the owner's named tunnel): the
/// owner's `~/.cloudflared/quick-empty.yml`, else one in the planner's state folder.
fn quick_config(cfg: &AppConfig) -> Result<PathBuf, Problem> {
    let theirs = crate::config::home().join(".cloudflared/quick-empty.yml");
    if theirs.is_file() {
        return Ok(theirs);
    }
    let ours = cfg.state_dir.join("quick-empty.yml");
    if !ours.is_file() {
        std::fs::create_dir_all(&cfg.state_dir).and_then(|_| std::fs::write(&ours, "")).map_err(|e| Problem::new("spawn", format!("cannot write {}: {e}", ours.display())))?;
    }
    Ok(ours)
}

fn prepare(app: &Shared) -> Result<Prepared, Problem> {
    let settings = app.tunnel.settings();
    let Some(bin) = find_bin(&app.cfg) else {
        return Err(Problem::new("not-installed", "cloudflared is not installed (or not where the planner looks: $PATH, ~/.local/bin, /usr/local/bin, /opt/homebrew/bin, /usr/bin; tunnel.bin in config.toml)"));
    };
    let metrics = free_port().ok_or_else(|| Problem::new("spawn", "no free port for the metrics of cloudflared"))?;
    let metrics_arg = format!("127.0.0.1:{metrics}");
    match settings.mode {
        TunnelMode::Quick => {
            let config = quick_config(&app.cfg)?;
            // The target is this planner on this machine, whatever it listens on; with TLS the certificate is for
            // another name.
            let (scheme, extra) = if app.cfg.tls().is_some() { ("https", Some("--no-tls-verify")) } else { ("http", None) };
            let mut args = vec!["--config".to_string(), config.display().to_string(), "tunnel".into(), "--no-autoupdate".into(), "--metrics".into(), metrics_arg];
            args.extend(extra.map(String::from));
            args.extend(["--url".to_string(), format!("{scheme}://127.0.0.1:{}", app.cfg.port)]);
            Ok(Prepared::Run(Launch { bin, args, metrics, host: None, quick: true }))
        }
        TunnelMode::Named => {
            let named = check_named(&settings).map_err(|(p, _)| p)?;
            let config = settings.config.unwrap_or_default();
            if let Some(pid) = find_external(app, &config, None) {
                return Ok(Prepared::External(pid, named.host));
            }
            let args = vec!["--config".to_string(), config.display().to_string(), "tunnel".into(), "--no-autoupdate".into(), "--metrics".into(), metrics_arg, "run".into()];
            Ok(Prepared::Run(Launch { bin, args, metrics, host: Some(named.host), quick: false }))
        }
    }
}

/// Starts the tunnel (a no-op when one is up or starting). Returns at once: the state changes as the process comes up.
pub async fn open(app: &Shared) {
    let rx = {
        let mut g = app.tunnel.inner.lock().unwrap();
        if g.state.running() {
            return;
        }
        g.state = Phase::Starting;
        g.since = now_ms();
        g.host = None;
        g.problem = None;
        g.external = None;
        g.log.clear();
        g.active = true;
        app.tunnel.stop.subscribe()
    };
    app.bus.send("tunnel", json!({"state": Phase::Starting}).to_string());
    let launch = match prepare(app) {
        Ok(Prepared::Run(l)) => l,
        Ok(Prepared::External(pid, host)) => {
            app.tunnel.inner.lock().unwrap().active = false;
            change(app, Phase::External, |g| {
                g.external = Some(pid);
                g.host = Some(host);
            })
            .await;
            return;
        }
        Err(p) => {
            app.tunnel.inner.lock().unwrap().active = false;
            change(app, Phase::Failed, |g| g.problem = Some(p)).await;
            return;
        }
    };
    if app.tunnel.state() != Phase::Starting {
        // Closed while it was being prepared: there is no process to wait for.
        finish(app, Phase::Closed, None).await;
        return;
    }
    app.tunnel.inner.lock().unwrap().host = launch.host.clone();
    let app = app.clone();
    tokio::spawn(async move { supervise(app, launch, rx).await });
}

/// The lines the program writes, into the ring of the log.
fn pump<R: tokio::io::AsyncRead + Send + Unpin + 'static>(app: &Shared, stream: R) {
    let app = app.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            app.tunnel.push_log(&line);
        }
    });
}

enum End {
    Stopped(Duration),
    Exited(Option<i32>),
    Timeout,
    Lost,
}

async fn supervise(app: Shared, launch: Launch, mut stop: watch::Receiver<u64>) {
    let t = app.tunnel.timing();
    let mut attempt = 0;
    let end = loop {
        let mut cmd = Command::new(&launch.bin);
        cmd.args(&launch.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let p = Problem::new("spawn", format!("cannot start {}: {e}", launch.bin.display()));
                finish(&app, Phase::Failed, Some(p)).await;
                return;
            }
        };
        if let Some(pid) = child.id() {
            write_pid_file(&app.cfg, pid);
        }
        if let Some(out) = child.stdout.take() {
            pump(&app, out);
        }
        if let Some(err) = child.stderr.take() {
            pump(&app, err);
        }
        let end = watch_child(&app, &launch, &mut child, &mut stop, &t).await;
        match end {
            End::Stopped(grace) => {
                terminate(&mut child, grace).await;
            }
            End::Timeout | End::Lost => terminate(&mut child, t.term_grace).await,
            End::Exited(_) => {
                let _ = child.wait().await;
            }
        }
        // Cloudflare refuses a quick tunnel now and then; asking again once is what a person would do.
        if let End::Exited(_) = end {
            if attempt == 0 && launch.quick && app.tunnel.state() == Phase::Starting && retry_worthy(&app) {
                attempt += 1;
                app.tunnel.inner.lock().unwrap().host = None;
                tokio::select! {
                    _ = tokio::time::sleep(t.retry_pause) => continue,
                    _ = stop.changed() => break End::Stopped(Duration::ZERO),
                }
            }
        }
        break end;
    };
    let _ = std::fs::remove_file(pid_file(&app.cfg));
    match end {
        End::Stopped(_) => finish(&app, Phase::Closed, None).await,
        End::Exited(code) => {
            let how = code.map_or("a signal".to_string(), |c| format!("code {c}"));
            let mut p = Problem::new("exited", format!("cloudflared exited ({how})"));
            p.log_tail = app.tunnel.tail();
            finish(&app, Phase::Failed, Some(p)).await;
        }
        End::Timeout => {
            let mut p = Problem::new("timeout", format!("no connection to Cloudflare within {} s", t.start.as_secs().max(1)));
            p.log_tail = app.tunnel.tail();
            finish(&app, Phase::Failed, Some(p)).await;
        }
        End::Lost => {
            let mut p = Problem::new("lost", format!("no connection to Cloudflare for {} s: the tunnel was closed", t.lost.as_secs().max(1)));
            p.log_tail = app.tunnel.tail();
            finish(&app, Phase::Failed, Some(p)).await;
        }
    }
}

fn retry_worthy(app: &Shared) -> bool {
    let g = app.tunnel.inner.lock().unwrap();
    g.log.iter().any(|l| l.contains("failed to request quick Tunnel") || l.contains("address already in use"))
}

async fn finish(app: &Shared, to: Phase, problem: Option<Problem>) {
    change(app, to, |g| {
        g.problem = problem;
        g.host = None;
        g.active = false;
    })
    .await;
}

async fn watch_child(app: &Shared, launch: &Launch, child: &mut Child, stop: &mut watch::Receiver<u64>, t: &Timing) -> End {
    let started = Instant::now();
    loop {
        let poll = if app.tunnel.state() == Phase::Starting { t.poll_start } else { t.poll_open };
        tokio::select! {
            status = child.wait() => return End::Exited(status.ok().and_then(|s| s.code())),
            _ = stop.changed() => return End::Stopped(Duration::from_millis(*stop.borrow())),
            _ = tokio::time::sleep(poll) => {}
        }
        match app.tunnel.state() {
            Phase::Starting => {
                let host = app.tunnel.inner.lock().unwrap().host.clone();
                let host = match host {
                    Some(h) => Some(h),
                    None if launch.quick => {
                        let found = match quick_host_from_metrics(launch.metrics).await {
                            Some(h) => Some(h),
                            None => app.tunnel.inner.lock().unwrap().log.iter().find_map(|l| quick_host_in_line(l)),
                        };
                        if let Some(h) = &found {
                            app.tunnel.inner.lock().unwrap().host = Some(h.clone());
                        }
                        found
                    }
                    None => None,
                };
                if host.is_some() && is_ready(launch.metrics).await {
                    advance(app, Phase::Starting, Phase::Open).await;
                } else if started.elapsed() >= t.start {
                    return End::Timeout;
                }
            }
            Phase::Open => {
                if !is_ready(launch.metrics).await {
                    advance(app, Phase::Open, Phase::Reconnecting).await;
                }
            }
            Phase::Reconnecting => {
                if is_ready(launch.metrics).await {
                    advance(app, Phase::Reconnecting, Phase::Open).await;
                } else if app.tunnel.inner.lock().unwrap().since + t.lost.as_millis() as u64 <= now_ms() {
                    return End::Lost;
                }
            }
            _ => {}
        }
    }
}

/* ───────── closing ───────── */

/// Closes the tunnel: the gate first (a request through the tunnel gets its 503 at once), then SIGTERM to the
/// process, SIGKILL after `grace`. Returns when it is closed. A tunnel somebody else runs is not touched, only
/// forgotten.
pub async fn close(app: &Shared, grace: Duration) {
    let supervised = {
        let mut g = app.tunnel.inner.lock().unwrap();
        match g.state {
            Phase::Closed | Phase::Closing => g.state == Phase::Closing,
            Phase::Starting | Phase::Open | Phase::Reconnecting if g.active => {
                g.state = Phase::Closing;
                g.since = now_ms();
                true
            }
            _ => {
                // Failed, External, or Starting with no process yet.
                g.state = Phase::Closed;
                g.since = now_ms();
                g.host = None;
                g.problem = None;
                g.external = None;
                g.active = false;
                false
            }
        }
    };
    app.gate.set_tunnel(None);
    // The key and every session go before the process is told to leave: a stolen cookie is dead from this moment.
    app.devices.end();
    app.bus.send("tunnel", json!({"state": app.tunnel.state()}).to_string());
    if !supervised {
        return;
    }
    app.tunnel.stop.send_replace(grace.as_millis() as u64);
    let end = Instant::now() + grace + Duration::from_secs(2);
    while app.tunnel.state() == Phase::Closing && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Nobody has used the gate for `hours`: it closes itself.
pub async fn close_idle(app: &Shared, hours: u32) {
    crate::app::journal_note(app, "tunnel", "tunnel", &format!("шлюз закрыт: им не пользовались {hours} ч")).await;
    close(app, app.tunnel.timing().term_grace).await;
}

/// The planner is stopping (SIGTERM or Ctrl-C): the tunnel goes first, before the process's own exit.
pub async fn shutdown(app: &Shared) {
    if app.tunnel.state().running() {
        close(app, app.tunnel.timing().shutdown_grace).await;
    }
}

/* ───────── the page's API (this machine only) ───────── */

/// `GET /api/tunnel`
pub async fn tunnel_get(State(app): State<Shared>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    if let Some(r) = refuse_remote(addr, &headers) {
        return r;
    }
    Json(view(&app).await).into_response()
}

/// A POST of the tunnel: from this machine, from this page.
pub(crate) fn guard(addr: SocketAddr, headers: &HeaderMap) -> Option<Response> {
    if !same_origin(headers) {
        return Some(error(StatusCode::FORBIDDEN, "cross-origin request refused"));
    }
    refuse_remote(addr, headers)
}

/// `POST /api/tunnel/open`
pub async fn tunnel_open(State(app): State<Shared>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(addr, &headers) {
        return r;
    }
    open(&app).await;
    Json(view(&app).await).into_response()
}

/// `POST /api/tunnel/close`
pub async fn tunnel_close(State(app): State<Shared>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    if let Some(r) = guard(addr, &headers) {
        return r;
    }
    close(&app, app.tunnel.timing().term_grace).await;
    Json(view(&app).await).into_response()
}

#[derive(Deserialize)]
pub struct ConfigBody {
    mode: Option<String>,
    config: Option<String>,
    hostname: Option<String>,
}

/// `POST /api/tunnel/config`: the mode, the `cloudflared` config and the address of a named tunnel. Written to
/// `config.toml`; changed only while no tunnel is up. Fields that are left out stay as they are; an empty `config` or
/// `hostname` removes the setting.
pub async fn tunnel_config(State(app): State<Shared>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<ConfigBody>) -> Response {
    if let Some(r) = guard(addr, &headers) {
        return r;
    }
    if app.tunnel.state().running() {
        return error(StatusCode::CONFLICT, "close the tunnel first");
    }
    let mut next = app.tunnel.settings();
    if let Some(m) = &body.mode {
        match TunnelMode::parse(m) {
            Some(m) => next.mode = m,
            None => return error(StatusCode::BAD_REQUEST, "mode is `quick` or `named`"),
        }
    }
    if let Some(c) = &body.config {
        let c = c.trim();
        next.config = (!c.is_empty()).then(|| crate::config::expand_home(c));
        if next.config.as_ref().is_some_and(|p| !p.is_absolute()) {
            return error(StatusCode::BAD_REQUEST, "the config path must be absolute (or start with ~/)");
        }
    }
    if let Some(h) = &body.hostname {
        next.hostname = match h.trim() {
            "" => None,
            t => match crate::config::tunnel_host(t).filter(|h| valid_host(h) && !h.starts_with("*.") && !h.contains(':')) {
                Some(h) => Some(h),
                None => return error(StatusCode::BAD_REQUEST, "hostname is a DNS name, like planner.example.org"),
            },
        };
    }
    let saved = crate::config::set_tunnel(
        &app.cfg.config_file,
        next.mode,
        &next.config.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        next.hostname.as_deref().unwrap_or_default(),
    );
    if let Err(e) = saved {
        return error(StatusCode::INTERNAL_SERVER_ERROR, format!("config.toml was not written: {e}"));
    }
    app.gate.set_tunnel_name(next.hostname.as_deref());
    *app.tunnel.settings.lock().unwrap() = next;
    {
        // A failure or a forgotten external tunnel belonged to the old settings.
        let mut g = app.tunnel.inner.lock().unwrap();
        if matches!(g.state, Phase::Failed | Phase::External) {
            g.state = Phase::Closed;
            g.since = now_ms();
            g.problem = None;
            g.external = None;
            g.host = None;
        }
    }
    Json(view(&app).await).into_response()
}

#[cfg(test)]
#[path = "tunnel_tests.rs"]
mod tests;
