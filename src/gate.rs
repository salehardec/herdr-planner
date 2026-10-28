//! The gate: who a request comes from, before any route runs.
//!
//! A tunnel (`cloudflared`) connects to the planner from `127.0.0.1`, so for the routes' own checks a request from
//! the internet looks like one from this machine. The gate is the outermost layer: it gives every request a class,
//! and what a class may reach is decided by one table (`POLICY`), with a refusal for everything the table does not name.
//!
//! - `Local`: the peer is this machine, the `Host` is one of ours, and nothing says a proxy or a tunnel passed it on.
//! - `Tunnel`: the peer is this machine, but a `Cf-*` header or the tunnel's address says it came through a tunnel.
//!   Tunnel closed: 503 for everything. Open: only what the table allows for the device's standing (`devices.rs`):
//!   nobody gets the sign-in page and the calls it makes, a device the owner let in the light version (and its API,
//!   `/m/api/*`, see `mobile.rs`), and the full planner a device that was raised to it.
//! - `Direct`: the peer is another machine, or this machine's own reverse proxy (`X-Forwarded-For`…). The login and
//!   TLS rules of the routes apply, as they always did; the gate lets it through.
//! - `Foreign`: the peer is this machine, but the `Host` is not one of ours. That is a page whose name points at
//!   `127.0.0.1` (DNS rebinding): 421, nothing runs. A reverse proxy's own domain goes into `trusted_hosts`.

use crate::app::Shared;
use crate::auth::cookie_value;
use crate::config::{now_ms, AppConfig};
use crate::devices::Standing;
use crate::server::error;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::Response;
use std::net::{IpAddr, SocketAddr};
use std::sync::RwLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Local,
    Tunnel,
    Direct,
    Foreign,
}

/// What a request from a tunnel may reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The sign-in page and what it calls: anyone through the tunnel, no session needed.
    Public,
    /// What the light page uses: any device that is in.
    Lite,
    /// The planner's page and its API, for a device that has the full version.
    Full,
    /// Only the machine the planner runs on: terminals, files, shipyards, settings, memory, the tunnel itself.
    Local,
}

use Access::{Full, Lite, Local, Public};

/// Every route of the planner and who may use it through a tunnel. A route that is not here is invisible to a tunnel
/// client (404), and `policy_covers_every_route` fails until the new route is entered.
pub const POLICY: &[(&str, Access)] = &[
    ("/", Full),
    ("/login", Full),
    ("/api/login", Full),
    ("/api/logout", Full),
    ("/assets/{*path}", Lite),
    ("/favicon.svg", Lite),
    ("/favicon.ico", Lite),
    // The sign-in page: the key of the link for a session, what became of the request, the page's script.
    ("/g/gate.js", Public),
    ("/g/redeem", Public),
    ("/g/status", Public),
    ("/g/cancel", Public),
    ("/api/state", Full),
    ("/api/summary", Full),
    ("/api/load", Full),
    ("/api/details", Full),
    ("/api/coordinator/screen", Full),
    ("/api/action", Full),
    ("/api/menu/answer", Full),
    ("/api/chat", Full),
    ("/api/chat/history", Full),
    ("/api/chat/image/{n}/{i}", Full),
    ("/api/chat/attach", Full),
    ("/api/attachments/{id}", Full),
    ("/api/refresh", Full),
    ("/api/events", Full),
    ("/api/search", Full),
    ("/api/models", Full),
    ("/api/subagent", Full),
    ("/api/task/closing", Full),
    ("/api/hints/slash", Full),
    // A terminal on the owner's desktop, folders and files of this machine.
    ("/api/task/terminal", Local),
    ("/api/fs/complete", Local),
    ("/api/hints/files", Local),
    // A bar widget; the phone gets limits inside its own snapshot.
    ("/api/limits", Local),
    // Spends tokens.
    ("/api/translate", Local),
    // Cloning from any address, forks.
    ("/api/universe/inspect", Local),
    ("/api/universe/add", Local),
    ("/api/universe/job", Local),
    ("/api/universe/info", Local),
    ("/api/universe/fork", Local),
    ("/api/memory", Local),
    ("/api/memory/save", Local),
    ("/api/memory/delete", Local),
    ("/api/memory/send", Local),
    ("/api/memory/sent/forget", Local),
    ("/api/memory/discard", Local),
    ("/api/memory/discard-stuck", Local),
    ("/api/memory/settings", Local),
    ("/api/memory/connect", Local),
    ("/api/memory/suggestion", Local),
    ("/api/memory/digest", Local),
    // Shipyards: ssh to other machines, containers.
    ("/api/runners", Local),
    ("/api/runners/history", Local),
    ("/api/runner/log", Local),
    ("/api/runner/ask", Local),
    ("/api/runner/probe", Local),
    ("/api/runner/add", Local),
    ("/api/runner/set", Local),
    ("/api/runner/remove", Local),
    ("/api/runner/pick", Local),
    ("/api/runner/connect", Local),
    ("/api/runner/clean/preview", Local),
    ("/api/runner/clean", Local),
    ("/api/runner/disk", Local),
    ("/api/runner/disk/clean", Local),
    ("/api/runner/auto", Local),
    ("/api/runner/auto/preview", Local),
    ("/api/runner/container/logs", Local),
    ("/api/runner/container/preview", Local),
    ("/api/runner/container/act", Local),
    ("/api/runner/container/shell", Local),
    ("/api/runner/terminal", Local),
    ("/api/strays", Local),
    ("/api/strays/stop", Local),
    ("/api/strays/auto", Local),
    // The station and the ship: settings of this machine.
    ("/api/station", Local),
    ("/api/station/models", Local),
    ("/api/station/preview", Local),
    ("/api/station/set", Local),
    ("/api/station/light", Local),
    ("/api/station/presets", Local),
    ("/api/station/preset/preview", Local),
    ("/api/station/preset/apply", Local),
    ("/api/ship", Local),
    // The tunnel itself: its address, opening and closing it.
    ("/api/tunnel", Local),
    ("/api/tunnel/open", Local),
    ("/api/tunnel/close", Local),
    ("/api/tunnel/config", Local),
    ("/api/tunnel/rotate", Local),
    ("/api/tunnel/session", Local),
    ("/api/tunnel/settings", Local),
    // The light page's own API: a narrow slice of the planner's, for a device that is in (`mobile.rs`).
    ("/m/api/state", Lite),
    ("/m/api/poll", Lite),
    ("/m/api/chat", Lite),
    ("/m/api/chat/history", Lite),
    ("/m/api/chat/image/{n}/{i}", Lite),
    ("/m/api/action", Lite),
    ("/m/api/menu/answer", Lite),
    ("/m/api/coordinator/screen", Lite),
    ("/m/api/details", Lite),
    ("/m/api/session", Lite),
    ("/m/api/logout", Lite),
    ("/m/api/tunnel/close", Lite),
];

/// The access a path has in `POLICY`, if it is there.
pub fn policy_of(path: &str) -> Option<Access> {
    POLICY.iter().find(|(pattern, _)| path_matches(pattern, path)).map(|(_, a)| *a)
}

/// `{name}` stands for one non-empty segment, `{*name}` for the rest of the path (at least one segment): the same
/// matching the router does, so a path the table calls known is a path the router would route to that very handler.
fn path_matches(pattern: &str, path: &str) -> bool {
    let mut want = pattern.split('/');
    let mut have = path.split('/');
    loop {
        match (want.next(), have.next()) {
            (None, None) => return true,
            (Some(w), Some(h)) if w.starts_with("{*") => return !h.is_empty() || have.next().is_some(),
            (Some(w), Some(h)) if w.starts_with('{') => {
                if h.is_empty() {
                    return false;
                }
            }
            (Some(w), Some(h)) => {
                if w != h {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

/// A `Host` value as a lower-case name (an IPv6 literal without its brackets) and a port.
fn split_host(value: &str) -> Option<(String, Option<u16>)> {
    let v = value.trim().to_lowercase();
    if v.is_empty() || v.contains(|c: char| c.is_whitespace() || c == '/' || c == '@' || c == '\\') {
        return None;
    }
    let (name, port) = if let Some(rest) = v.strip_prefix('[') {
        let (inner, tail) = rest.split_once(']')?;
        match tail {
            "" => (inner.to_string(), None),
            _ => (inner.to_string(), Some(tail.strip_prefix(':')?.parse().ok()?)),
        }
    } else {
        match v.rsplit_once(':') {
            Some((n, p)) if !n.contains(':') => (n.to_string(), Some(p.parse().ok()?)),
            Some(_) => return None,
            None => (v, None),
        }
    };
    let name = name.trim_end_matches('.').to_string();
    (!name.is_empty()).then_some((name, port))
}

/// The names that always mean this machine. Any port: an `ssh -L` forward comes in on another one.
const OWN_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// Headers a proxy in front of the planner adds.
const PROXY_HEADERS: [&str; 6] = [
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-forwarded-port",
    "x-real-ip",
    "forwarded",
];

pub struct Gate {
    /// Hosts allowed besides our own: `trusted_hosts` (a rule without a port takes any).
    trusted: Vec<(String, Option<u16>)>,
    /// The named tunnel's address, if one is set up; a request for it is a tunnel request.
    tunnel_name: RwLock<Option<String>>,
    /// The address of the tunnel that is open now (set by whoever runs the tunnel).
    open: RwLock<Option<String>>,
}

impl Gate {
    pub fn new(cfg: &AppConfig) -> Self {
        let trusted: Vec<(String, Option<u16>)> = cfg.trusted_hosts.iter().filter_map(|h| split_host(h)).collect();
        // The tunnel's address is not a trusted name: a request for it is a tunnel request, whatever else it says.
        let tunnel_name = cfg.tunnel_hostname.as_deref().and_then(split_host).map(|(n, _)| n);
        Self { trusted, tunnel_name: RwLock::new(tunnel_name), open: RwLock::new(None) }
    }

    /// The named tunnel's address was changed in the settings (`None`: removed). The names `trusted_hosts` lists are
    /// not touched.
    pub fn set_tunnel_name(&self, host: Option<&str>) {
        *self.tunnel_name.write().unwrap() = host.and_then(split_host).map(|(n, _)| n);
    }

    /// The tunnel was opened at this address (`None`: it was closed). Closed is the state of a planner that has just
    /// started; the tunnel's launcher calls this.
    pub fn set_tunnel(&self, host: Option<&str>) {
        *self.open.write().unwrap() = host.and_then(split_host).map(|(n, _)| n);
    }

    pub fn tunnel_open(&self) -> bool {
        self.open.read().unwrap().is_some()
    }

    fn is_trusted(&self, name: &str, port: Option<u16>) -> bool {
        OWN_HOSTS.contains(&name)
            || self.trusted.iter().any(|(n, p)| n == name && (p.is_none() || *p == port))
            || self.tunnel_name.read().unwrap().as_deref() == Some(name)
    }

    fn names_the_tunnel(&self, name: &str) -> bool {
        name.ends_with(".trycloudflare.com")
            || self.tunnel_name.read().unwrap().as_deref() == Some(name)
            || self.open.read().unwrap().as_deref() == Some(name)
    }

    /// Who sent this request. `peer` is the socket's address, never a header.
    pub fn classify(&self, peer: IpAddr, headers: &HeaderMap, uri: &Uri) -> Class {
        // A dual-stack listener shows 127.0.0.1 as ::ffff:127.0.0.1.
        if !peer.to_canonical().is_loopback() {
            return Class::Direct;
        }
        let hosts = headers.get_all(header::HOST);
        let host = if hosts.iter().count() > 1 {
            None
        } else {
            hosts
                .iter()
                .next()
                .and_then(|v| v.to_str().ok())
                .or_else(|| uri.authority().map(|a| a.as_str()))
                .and_then(split_host)
        };
        let from_cloudflare = headers.keys().any(|k| k.as_str().starts_with("cf-") || k.as_str() == "cdn-loop");
        if from_cloudflare || host.as_ref().is_some_and(|(n, _)| self.names_the_tunnel(n)) {
            return Class::Tunnel;
        }
        match host {
            Some((name, port)) if self.is_trusted(&name, port) => {
                if PROXY_HEADERS.iter().any(|h| headers.contains_key(*h)) {
                    Class::Direct
                } else {
                    Class::Local
                }
            }
            _ => Class::Foreign,
        }
    }
}

/// What the gate does with a request.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// 421: the `Host` is not one of ours.
    Misdirected,
    /// 503: a tunnel request, and no tunnel is open.
    TunnelClosed,
    /// 403: a tunnel request that came over plain http.
    PlainHttp,
    /// 404: nothing at this path for a tunnel client.
    Hidden,
    /// 401: the path is open to a device that is in, and this one is not.
    SignIn,
    /// The sign-in page, in place of the planner's own page, for a device that is not in.
    SignInPage,
    /// 403: the path needs the full version, and this device has the light one.
    LiteOnly,
}

/// The tunnel's end of the gate: an HTTP request through Cloudflare carries `Cf-Visitor: {"scheme":"http"}` and
/// `X-Forwarded-Proto: http` (checked on a live tunnel, PLN-73): a key or a cookie must not go over that.
pub fn plain_http(headers: &HeaderMap) -> bool {
    let visitor = headers.get("cf-visitor").and_then(|v| v.to_str().ok());
    if visitor.is_some_and(|v| v.contains("\"http\"")) {
        return true;
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("http"))
}

/// `standing` is how the cookie of a tunnel request stands (`Devices::standing_at`); the other classes do not use it.
pub fn verdict(class: Class, tunnel_open: bool, headers: &HeaderMap, path: &str, standing: Standing) -> Verdict {
    match class {
        Class::Local | Class::Direct => Verdict::Pass,
        Class::Foreign => Verdict::Misdirected,
        Class::Tunnel if !tunnel_open => Verdict::TunnelClosed,
        Class::Tunnel if plain_http(headers) => Verdict::PlainHttp,
        Class::Tunnel => match (policy_of(path), standing) {
            (Some(Public), _) => Verdict::Pass,
            (Some(Lite), Standing::Lite | Standing::Full) | (Some(Full), Standing::Full) => Verdict::Pass,
            (Some(Full), Standing::Lite) => Verdict::LiteOnly,
            // the planner's own page: a device that is not in gets the sign-in page instead of a refusal
            (Some(Full), _) if path == "/" => Verdict::SignInPage,
            (Some(Full | Lite), _) => Verdict::SignIn,
            (Some(Local) | None, _) => Verdict::Hidden,
        },
    }
}

/// The outermost layer of the router (inside the security headers).
pub async fn middleware(State(app): State<Shared>, mut req: Request, next: Next) -> Response {
    let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied() else {
        return error(StatusCode::MISDIRECTED_REQUEST, "unknown peer");
    };
    let class = app.gate.classify(addr.ip(), req.headers(), req.uri());
    let open = app.gate.tunnel_open();
    // Only a request that may be answered at all is a use of the gate: the idle clocks must not be fed by refusals.
    let standing = if class == Class::Tunnel && open && !plain_http(req.headers()) {
        let token = cookie_value(req.headers(), crate::devices::COOKIE);
        if req.method() == Method::GET && req.uri().path() == crate::mobile::POLL_PATH {
            app.devices.standing_quiet_at(now_ms(), token.as_deref())
        } else {
            app.devices.standing_at(now_ms(), token.as_deref())
        }
    } else {
        Standing::Nobody
    };
    match verdict(class, open, req.headers(), req.uri().path(), standing) {
        // the routes behind the gate that care who is asking (the light page's API) read the class from here
        Verdict::Pass => {
            req.extensions_mut().insert(class);
            next.run(req).await
        }
        Verdict::Misdirected => error(
            StatusCode::MISDIRECTED_REQUEST,
            "this host name is not trusted; add it to trusted_hosts in config.toml",
        ),
        // a browser that opens the address of a closed gate gets a page, not JSON
        Verdict::TunnelClosed if wants_page(&req) => crate::devices::closed_page(),
        Verdict::TunnelClosed => error(StatusCode::SERVICE_UNAVAILABLE, "the tunnel is closed"),
        Verdict::PlainHttp => error(StatusCode::FORBIDDEN, "https only"),
        Verdict::Hidden => error(StatusCode::NOT_FOUND, "not found"),
        Verdict::SignInPage if req.method() == Method::GET || req.method() == Method::HEAD => crate::devices::gate_page(),
        Verdict::SignIn | Verdict::SignInPage => error(StatusCode::UNAUTHORIZED, "sign in"),
        Verdict::LiteOnly => error(StatusCode::FORBIDDEN, "this device has the light version only"),
    }
}

/// A navigation of a browser (it asks for a page), not a call of a script.
fn wants_page(req: &Request) -> bool {
    req.method() == Method::GET
        && req.headers().get(header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("text/html"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::Ipv4Addr;

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const LAN: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));

    fn gate_with(trusted: &[&str], tunnel: Option<&str>) -> Gate {
        let dir = std::path::Path::new("/nonexistent");
        let cfg = AppConfig {
            trusted_hosts: trusted.iter().map(|s| s.to_string()).collect(),
            tunnel_hostname: tunnel.map(str::to_string),
            ..crate::testutil::test_cfg(dir)
        };
        Gate::new(&cfg)
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn class(g: &Gate, peer: IpAddr, pairs: &[(&str, &str)]) -> Class {
        g.classify(peer, &headers(pairs), &Uri::from_static("/api/state"))
    }

    #[test]
    fn this_machine_is_local_by_its_own_names_on_any_port() {
        let g = gate_with(&[], None);
        for host in ["localhost:7117", "127.0.0.1:7117", "[::1]:7117", "LOCALHOST:8080", "localhost", "localhost.:7117"] {
            assert_eq!(class(&g, LOOPBACK, &[("host", host)]), Class::Local, "{host}");
        }
    }

    #[test]
    fn a_name_that_resolves_to_this_machine_is_foreign() {
        // DNS rebinding: the page's own name points at 127.0.0.1, so the connection is from loopback.
        let g = gate_with(&["planner.lan"], None);
        for host in ["attacker.example:7117", "attacker.example", "127.0.0.1.attacker.example:7117", "localhost.attacker.example", "evil@localhost", "localhost:abc", "10.0.0.5:7117", ""] {
            assert_eq!(class(&g, LOOPBACK, &[("host", host)]), Class::Foreign, "{host:?}");
        }
        assert_eq!(class(&g, LOOPBACK, &[]), Class::Foreign, "no Host at all");
        assert_eq!(class(&g, LOOPBACK, &[("host", "localhost"), ("host", "attacker.example")]), Class::Foreign, "two Hosts");
        // …even when the page adds a header that would make it look like a proxy's request
        assert_eq!(class(&g, LOOPBACK, &[("host", "attacker.example"), ("x-forwarded-for", "203.0.113.5")]), Class::Foreign);
        assert_eq!(class(&g, LOOPBACK, &[("host", "attacker.example"), ("x-forwarded-for", "127.0.0.1")]), Class::Foreign);
    }

    #[test]
    fn the_authority_of_a_request_line_stands_in_for_a_missing_host() {
        let g = gate_with(&[], None);
        let h = HeaderMap::new();
        assert_eq!(g.classify(LOOPBACK, &h, &Uri::from_static("http://localhost:7117/x")), Class::Local);
        assert_eq!(g.classify(LOOPBACK, &h, &Uri::from_static("http://attacker.example/x")), Class::Foreign);
    }

    #[test]
    fn trusted_hosts_are_the_owners_own_names() {
        let g = gate_with(&["planner.lan", "box.example.org:8443"], None);
        assert_eq!(class(&g, LOOPBACK, &[("host", "planner.lan")]), Class::Local);
        assert_eq!(class(&g, LOOPBACK, &[("host", "planner.lan:7117")]), Class::Local, "a name without a port takes any");
        assert_eq!(class(&g, LOOPBACK, &[("host", "box.example.org:8443")]), Class::Local);
        assert_eq!(class(&g, LOOPBACK, &[("host", "box.example.org:9999")]), Class::Foreign, "a rule with a port takes that one");
        assert_eq!(class(&g, LOOPBACK, &[("host", "other.lan")]), Class::Foreign);
        // behind the owner's own reverse proxy: as before (the login), not Local
        assert_eq!(class(&g, LOOPBACK, &[("host", "planner.lan"), ("x-forwarded-for", "203.0.113.5")]), Class::Direct);
        assert_eq!(class(&g, LOOPBACK, &[("host", "planner.lan"), ("forwarded", "for=203.0.113.5")]), Class::Direct);
        assert_eq!(class(&g, LOOPBACK, &[("host", "localhost:7117"), ("x-real-ip", "203.0.113.5")]), Class::Direct);
    }

    #[test]
    fn a_request_from_another_machine_is_direct_whatever_it_says() {
        let g = gate_with(&[], None);
        assert_eq!(class(&g, LAN, &[("host", "192.168.1.5:7117")]), Class::Direct);
        assert_eq!(class(&g, LAN, &[("host", "localhost:7117"), ("x-forwarded-for", "127.0.0.1")]), Class::Direct);
        assert_eq!(class(&g, LAN, &[]), Class::Direct);
    }

    #[test]
    fn a_loopback_peer_on_a_dual_stack_socket_is_still_loopback() {
        let g = gate_with(&[], None);
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(class(&g, mapped, &[("host", "localhost"), ("cf-ray", "x")]), Class::Tunnel);
        assert_eq!(class(&g, mapped, &[("host", "attacker.example")]), Class::Foreign);
        assert_eq!(class(&g, IpAddr::V6(std::net::Ipv6Addr::LOCALHOST), &[("host", "[::1]:7117")]), Class::Local);
    }

    #[test]
    fn cloudflare_headers_or_the_tunnels_name_make_a_tunnel_request() {
        let g = gate_with(&[], Some("planner.yanao.su"));
        let quick = "ab-cd-ef.trycloudflare.com";
        assert_eq!(class(&g, LOOPBACK, &[("host", quick)]), Class::Tunnel);
        assert_eq!(class(&g, LOOPBACK, &[("host", "planner.yanao.su")]), Class::Tunnel, "the named tunnel's address");
        assert_eq!(class(&g, LOOPBACK, &[("host", "Planner.Yanao.Su:443")]), Class::Tunnel);
        // any one of the Cloudflare headers is enough, with whatever Host (a client may send a Host it likes)
        for h in ["cf-ray", "cf-connecting-ip", "cf-visitor", "cf-ipcountry", "cf-warp-tag-id", "cdn-loop"] {
            assert_eq!(class(&g, LOOPBACK, &[("host", "localhost:7117"), (h, "x")]), Class::Tunnel, "{h}");
            assert_eq!(class(&g, LOOPBACK, &[("host", "attacker.example"), (h, "x")]), Class::Tunnel, "{h}");
        }
        // and a forged loopback address inside them changes nothing
        assert_eq!(
            class(&g, LOOPBACK, &[("host", quick), ("cf-connecting-ip", "127.0.0.1"), ("x-forwarded-for", "127.0.0.1, 203.0.113.5")]),
            Class::Tunnel
        );
        // the Cloudflare headers do not turn a LAN client into a tunnel: its login applies, as before
        assert_eq!(class(&g, LAN, &[("host", quick), ("cf-ray", "x")]), Class::Direct);
    }

    #[test]
    fn the_named_tunnel_address_is_trusted_without_trusted_hosts() {
        // It is a tunnel request first (the gate decides, not the Host list); the address in the settings is also
        // the one thing a manual `cloudflared tunnel run` answers on.
        let g = gate_with(&[], Some("planner.yanao.su"));
        assert!(g.is_trusted("planner.yanao.su", Some(443)));
        assert!(!g.is_trusted("other.yanao.su", None));
    }

    #[test]
    fn the_open_tunnel_address_counts_as_the_tunnel_too() {
        let g = gate_with(&[], None);
        assert_eq!(class(&g, LOOPBACK, &[("host", "my.example.org")]), Class::Foreign);
        assert!(!g.tunnel_open());
        g.set_tunnel(Some("my.example.org"));
        assert!(g.tunnel_open());
        assert_eq!(class(&g, LOOPBACK, &[("host", "my.example.org")]), Class::Tunnel);
        g.set_tunnel(None);
        assert!(!g.tunnel_open());
    }

    #[test]
    fn host_values_are_split_carefully() {
        assert_eq!(split_host("localhost:7117"), Some(("localhost".into(), Some(7117))));
        assert_eq!(split_host("[::1]:7117"), Some(("::1".into(), Some(7117))));
        assert_eq!(split_host("[::1]"), Some(("::1".into(), None)));
        assert_eq!(split_host("A.B."), Some(("a.b".into(), None)));
        assert_eq!(split_host("::1"), None, "an IPv6 literal needs brackets");
        assert_eq!(split_host("[::1]x"), None);
        assert_eq!(split_host("a b"), None);
        assert_eq!(split_host("a/b"), None);
        assert_eq!(split_host("a:99999"), None);
        assert_eq!(split_host(""), None);
    }

    #[test]
    fn paths_are_matched_like_the_router_does() {
        assert!(path_matches("/api/state", "/api/state"));
        assert!(!path_matches("/api/state", "/api/state/"));
        assert!(!path_matches("/api/state", "/api/states"));
        assert!(!path_matches("/api/state", "/api"));
        assert!(path_matches("/api/chat/image/{n}/{i}", "/api/chat/image/3/1"));
        assert!(!path_matches("/api/chat/image/{n}/{i}", "/api/chat/image//1"));
        assert!(!path_matches("/api/chat/image/{n}/{i}", "/api/chat/image/3"));
        assert!(path_matches("/assets/{*path}", "/assets/a.js"));
        assert!(path_matches("/assets/{*path}", "/assets/x/y.js"));
        assert!(!path_matches("/assets/{*path}", "/assets"));
        assert!(!path_matches("/assets/{*path}", "/asset/a.js"));
        assert!(path_matches("/", "/"));
        assert!(!path_matches("/", "/x"));
    }

    #[test]
    fn a_path_dressed_up_does_not_borrow_another_routes_access() {
        assert_eq!(policy_of("/api/limits"), Some(Local));
        for p in ["/api/limits/", "//api/limits", "/api//limits", "/api/limits/../state", "/api/%6cimits", "/API/limits", "/api/limits?x=1", "/api/tunnel", "/m", "/g/redeem"] {
            assert_ne!(policy_of(p), Some(Full), "{p}");
        }
    }

    /// The `.route("…")` paths of the router in `server.rs`.
    fn routed_paths() -> Vec<String> {
        let src = include_str!("server.rs");
        let from = src.find("pub fn router(").expect("the router");
        let to = from + src[from..].find("async fn headers_middleware").expect("the end of the router");
        let mut out = Vec::new();
        let mut rest = &src[from..to];
        while let Some(i) = rest.find(".route(") {
            rest = rest[i + ".route(".len()..].trim_start();
            let body = rest.strip_prefix('"').expect("a route starts with its path in quotes");
            let end = body.find('"').unwrap();
            out.push(body[..end].to_string());
            rest = &body[end..];
        }
        out
    }

    #[test]
    fn the_policy_names_every_route_and_only_routes() {
        let routed = routed_paths();
        assert!(routed.len() > 70, "found only {} routes: has the router moved?", routed.len());
        for p in &routed {
            assert!(
                POLICY.iter().any(|(pattern, _)| pattern == p),
                "{p} is a route of the router, but not in gate::POLICY: say who may use it through a tunnel"
            );
        }
        for (pattern, _) in POLICY {
            assert!(routed.iter().any(|p| p == pattern), "{pattern} is in gate::POLICY, but no route has it");
        }
        let mut seen = std::collections::HashSet::new();
        for (pattern, _) in POLICY {
            assert!(seen.insert(*pattern), "{pattern} is in gate::POLICY twice");
        }
    }

    #[test]
    fn what_reaches_a_terminal_a_file_or_another_machine_is_for_this_machine_only() {
        for p in [
            "/api/task/terminal",
            "/api/runner/terminal",
            "/api/runner/container/shell",
            "/api/runner/container/act",
            "/api/runner/add",
            "/api/runner/connect",
            "/api/fs/complete",
            "/api/hints/files",
            "/api/universe/add",
            "/api/universe/fork",
            "/api/station/set",
            "/api/memory/save",
            "/api/memory/discard-stuck",
            "/api/ship",
            "/api/limits",
            "/api/translate",
            "/api/strays/stop",
        ] {
            assert_eq!(policy_of(p), Some(Local), "{p}");
        }
        for p in ["/api/state", "/api/chat", "/api/action", "/api/menu/answer"] {
            assert_eq!(policy_of(p), Some(Full), "{p}");
        }
    }

    #[test]
    fn verdicts_of_a_tunnel_request() {
        let none = HeaderMap::new();
        use Standing::{Full as FullDevice, Lite as LiteDevice, Nobody, Waiting};
        // nothing is open: everything is 503, a known path or not
        for p in ["/", "/api/state", "/api/limits", "/nope", "/g/redeem"] {
            assert_eq!(verdict(Class::Tunnel, false, &none, p, Nobody), Verdict::TunnelClosed, "{p}");
            assert_eq!(verdict(Class::Tunnel, false, &none, p, FullDevice), Verdict::TunnelClosed, "{p}");
        }
        // open, nobody in: the sign-in page and its calls, and nothing else
        for who in [Nobody, Waiting] {
            assert_eq!(verdict(Class::Tunnel, true, &none, "/", who), Verdict::SignInPage);
            for p in ["/g/redeem", "/g/status", "/g/gate.js", "/g/cancel"] {
                assert_eq!(verdict(Class::Tunnel, true, &none, p, who), Verdict::Pass, "{p}");
            }
            for p in ["/api/state", "/api/action", "/assets/a.js", "/favicon.svg", "/login"] {
                assert_eq!(verdict(Class::Tunnel, true, &none, p, who), Verdict::SignIn, "{p}");
            }
            for p in ["/api/limits", "/api/task/terminal", "/api/tunnel", "/api/tunnel/rotate", "/api/tunnel/session", "/api/tunnel/settings", "/nope", "/m"] {
                assert_eq!(verdict(Class::Tunnel, true, &none, p, who), Verdict::Hidden, "{p}");
            }
        }
        // a device that is in with the light version: what the light page uses, not the planner
        assert_eq!(verdict(Class::Tunnel, true, &none, "/assets/a.js", LiteDevice), Verdict::Pass);
        assert_eq!(verdict(Class::Tunnel, true, &none, "/g/status", LiteDevice), Verdict::Pass);
        for p in ["/", "/api/state", "/api/action", "/api/chat"] {
            assert_eq!(verdict(Class::Tunnel, true, &none, p, LiteDevice), Verdict::LiteOnly, "{p}");
        }
        for p in ["/api/limits", "/api/tunnel/session", "/api/memory"] {
            assert_eq!(verdict(Class::Tunnel, true, &none, p, LiteDevice), Verdict::Hidden, "{p}");
        }
        // the full version: the planner's routes, and still nothing that is for this machine only
        for p in ["/", "/api/state", "/api/action", "/assets/a.js"] {
            assert_eq!(verdict(Class::Tunnel, true, &none, p, FullDevice), Verdict::Pass, "{p}");
        }
        for p in ["/api/task/terminal", "/api/runner/add", "/api/tunnel/close", "/api/tunnel/rotate", "/nope"] {
            assert_eq!(verdict(Class::Tunnel, true, &none, p, FullDevice), Verdict::Hidden, "{p}");
        }
        // plain http through Cloudflare, whoever asks
        for h in [("cf-visitor", "{\"scheme\":\"http\"}"), ("x-forwarded-proto", "http"), ("x-forwarded-proto", "http, https")] {
            assert_eq!(verdict(Class::Tunnel, true, &headers(&[h]), "/api/state", FullDevice), Verdict::PlainHttp, "{h:?}");
            assert_eq!(verdict(Class::Tunnel, true, &headers(&[h]), "/g/redeem", Nobody), Verdict::PlainHttp, "{h:?}");
        }
        for h in [("cf-visitor", "{\"scheme\":\"https\"}"), ("x-forwarded-proto", "https")] {
            assert_eq!(verdict(Class::Tunnel, true, &headers(&[h]), "/api/state", Nobody), Verdict::SignIn, "{h:?}");
        }
        assert_eq!(verdict(Class::Foreign, true, &none, "/", FullDevice), Verdict::Misdirected);
        assert_eq!(verdict(Class::Local, false, &none, "/api/limits", Nobody), Verdict::Pass);
        assert_eq!(verdict(Class::Direct, false, &none, "/api/limits", Nobody), Verdict::Pass);
    }

    // ---- the whole router, on a real socket (the peer address is the real one) ----

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve(app: Shared) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = crate::server::router(app);
        tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.ok();
        });
        addr
    }

    /// One request, as a client would write it; the status code and the body of the answer.
    async fn ask(addr: SocketAddr, method: &str, path: &str, extra: &[(&str, &str)]) -> (u16, String) {
        let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: 0\r\n");
        for (k, v) in extra {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(10), s.read_to_end(&mut raw)).await.unwrap().ok();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        (status, text.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
    }

    fn app_with(trusted: &[&str], tunnel: Option<&str>) -> (tempfile::TempDir, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AppConfig {
            trusted_hosts: trusted.iter().map(|s| s.to_string()).collect(),
            tunnel_hostname: tunnel.map(str::to_string),
            ..crate::testutil::test_cfg(dir.path())
        };
        (dir, crate::app::App::new(cfg))
    }

    /// A concrete path for each pattern of the table.
    fn concrete(pattern: &str) -> String {
        pattern.replace("{*path}", "x.js").replace("{n}", "1").replace("{i}", "2").replace("{id}", "3")
    }

    #[tokio::test]
    async fn a_page_with_a_rebound_name_reaches_nothing() {
        let (_dir, app) = app_with(&[], None);
        let addr = serve(app).await;
        for v in ["attacker.example:7117", "attacker.example"] {
            for (method, path) in [("GET", "/api/state"), ("GET", "/"), ("POST", "/api/action"), ("POST", "/api/task/terminal"), ("GET", "/api/limits"), ("GET", "/favicon.ico")] {
                let (status, body) = ask(addr, method, path, &[("Host", v), ("Origin", "http://attacker.example:7117")]).await;
                assert_eq!(status, 421, "{method} {path} {v}: {body}");
                assert!(body.contains("trusted_hosts"), "{body}");
            }
        }
        // the owner's own page still works
        let (status, _) = ask(addr, "GET", "/favicon.ico", &[("Host", &format!("localhost:{}", addr.port()))]).await;
        assert_eq!(status, 204);
        let (status, _) = ask(addr, "GET", "/api/state", &[("Host", &format!("127.0.0.1:{}", addr.port()))]).await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn a_trusted_host_name_gets_through() {
        let (_dir, app) = app_with(&["planner.lan"], None);
        let addr = serve(app).await;
        let (status, _) = ask(addr, "GET", "/api/state", &[("Host", "planner.lan"), ("X-Forwarded-For", "203.0.113.5")]).await;
        assert_eq!(status, 200);
        let (status, _) = ask(addr, "GET", "/api/state", &[("Host", "other.lan")]).await;
        assert_eq!(status, 421);
    }

    #[tokio::test]
    async fn a_closed_tunnel_answers_503_to_everything() {
        let (_dir, app) = app_with(&[], Some("planner.example.org"));
        let addr = serve(app.clone()).await;
        let mut probes: Vec<(&str, String, Vec<(&str, &str)>)> = Vec::new();
        for (pattern, _) in POLICY {
            probes.push(("GET", concrete(pattern), vec![("Host", "planner.example.org"), ("Cf-Ray", "x")]));
            probes.push(("POST", concrete(pattern), vec![("Host", "localhost"), ("Cf-Connecting-Ip", "203.0.113.5")]));
            probes.push(("GET", concrete(pattern), vec![("Host", "abc-def.trycloudflare.com")]));
        }
        probes.push(("GET", "/api/tunnel".into(), vec![("Host", "abc-def.trycloudflare.com")]));
        probes.push(("GET", "/nope".into(), vec![("Cf-Ray", "x"), ("Host", "localhost")]));
        for (method, path, headers) in &probes {
            let (status, body) = ask(addr, method, path, headers).await;
            assert_eq!(status, 503, "{method} {path} {headers:?}: {body}");
        }
        // …while this machine is not affected by the tunnel being closed
        let (status, _) = ask(addr, "GET", "/api/state", &[("Host", "localhost")]).await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn an_open_tunnel_shows_a_stranger_only_a_refusal() {
        let (_dir, app) = app_with(&[], Some("planner.example.org"));
        app.gate.set_tunnel(Some("planner.example.org"));
        let addr = serve(app).await;
        let https = ("Cf-Visitor", "{\"scheme\":\"https\"}");
        for (pattern, access) in POLICY {
            for method in ["GET", "POST"] {
                let (status, body) = ask(addr, method, &concrete(pattern), &[("Host", "planner.example.org"), ("Cf-Ray", "x"), https]).await;
                match (access, method, *pattern) {
                    (Local, _, _) => assert_eq!(status, 404, "{method} {pattern}: {body}"),
                    // the planner's page is the sign-in page for a stranger
                    (_, "GET", "/") => {
                        assert_eq!(status, 200, "{body}");
                        assert!(body.contains("/g/gate.js") && !body.contains("herdr-projects"), "{body}");
                    }
                    // the sign-in page's own calls are answered by themselves (a refusal of their own kind, never 404/503)
                    (Public, _, _) => assert!(![401, 404, 503].contains(&status), "{method} {pattern}: {status} {body}"),
                    _ => assert_eq!(status, 401, "{method} {pattern}: {body}"),
                }
            }
        }
        for path in ["/api/tunnel", "/api/tunnel/open", "/m", "/nope"] {
            let (status, _) = ask(addr, "GET", path, &[("Host", "planner.example.org"), ("Cf-Ray", "x"), https]).await;
            assert_eq!(status, 404, "{path}");
        }
        // a forged "I am local" changes nothing
        let (status, _) = ask(addr, "GET", "/api/limits", &[("Host", "localhost"), ("Cf-Ray", "x"), ("X-Forwarded-For", "127.0.0.1"), ("Cf-Connecting-Ip", "127.0.0.1"), https]).await;
        assert_eq!(status, 404);
        // a cookie nobody issued is no cookie
        let (status, _) = ask(addr, "GET", "/api/state", &[("Host", "planner.example.org"), ("Cf-Ray", "x"), https, ("Cookie", "__Host-hp_g=00; herdr_planner_session=00")]).await;
        assert_eq!(status, 401);
        // plain http through the tunnel
        let (status, _) = ask(addr, "GET", "/api/state", &[("Host", "planner.example.org"), ("Cf-Visitor", "{\"scheme\":\"http\"}")]).await;
        assert_eq!(status, 403);
    }

    #[tokio::test]
    async fn a_closed_gate_shows_a_browser_a_page() {
        let (_dir, app) = app_with(&[], None);
        let addr = serve(app).await;
        let (status, body) = ask(addr, "GET", "/", &[("Host", "abc-def.trycloudflare.com"), ("Accept", "text/html,application/xhtml+xml")]).await;
        assert_eq!(status, 503);
        assert!(body.contains("<h1>") && !body.contains("<script"), "{body}");
        let (status, body) = ask(addr, "GET", "/api/state", &[("Host", "abc-def.trycloudflare.com"), ("Accept", "application/json")]).await;
        assert_eq!((status, body.contains("closed")), (503, true));
    }
}
