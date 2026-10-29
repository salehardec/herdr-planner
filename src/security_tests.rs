//! The gate's security as a sweep (PLN-82, the regressions of the plan's section 7.6): every route of the table against
//! every kind of caller that can reach an open gate, on a real socket, so that a route, a class or a header added
//! later cannot open a door unnoticed. The single cases live beside their code (`gate.rs`, `devices_tests.rs`,
//! `tunnel_tests.rs`, `mobile_tests.rs`, `version_tests.rs`); this file asks the whole table at once.
//!
//! What a caller may get is the table's word (`gate::POLICY`):
//! - a stranger (no cookie, a cookie nobody issued, a device that still waits for the owner): the sign-in page and its own
//!   calls, nothing else;
//! - a device with the light version: the light page and its API, never the planner's;
//! - a device raised to the full version: the planner's API, never what is for this machine only.

use crate::app::{self, Shared};
use crate::config::{now_ms, AppConfig, TunnelMode};
use crate::devices::{Settings, COOKIE as DEVICE_COOKIE};
use crate::gate::{Access, POLICY};
use crate::testutil::test_cfg;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ADDRESS: &str = "abc-def.trycloudflare.com";
const PASSWORD: &str = "hunter2";
const DESKTOP: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0";
const NO_ONE: &str = "";

struct Rig {
    _dir: tempfile::TempDir,
    app: Shared,
    addr: SocketAddr,
}

/// The planner with its gate open on a quick tunnel, a password set and the full version allowed (so that a device
/// *can* be raised: the sweep is about what a raised device still may not do).
async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AppConfig {
        password_hash: bcrypt::hash(PASSWORD, 4).unwrap(),
        user: "me".into(),
        tunnel_full_allowed: true,
        tunnel_mode: TunnelMode::Quick,
        ..test_cfg(dir.path())
    };
    let app = app::App::new(cfg);
    app.gate.set_tunnel(Some(ADDRESS));
    app.devices.begin_at(now_ms());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = crate::server::router(app.clone());
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.ok();
    });
    Rig { _dir: dir, app, addr }
}

/// One request as a client writes it: status, head, body.
async fn ask(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String, String) {
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut raw)).await.unwrap_or_else(|_| panic!("{method} {path}: no answer")).ok();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (status, head.to_string(), body.to_string())
}

/// What Cloudflare puts on a request from the internet, and the caller's cookies.
fn through_cloudflare<'a>(cookie: &'a str, extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut h = vec![("Host", ADDRESS), ("Cf-Ray", "x"), ("Cf-Visitor", "{\"scheme\":\"https\"}"), ("Cf-Connecting-Ip", "203.0.113.9"), ("User-Agent", DESKTOP)];
    if !cookie.is_empty() {
        h.push(("Cookie", cookie));
    }
    h.extend_from_slice(extra);
    h
}

fn set_cookie(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|l| {
        let rest = l.to_lowercase().starts_with("set-cookie:").then(|| l.split_once(':').unwrap().1.trim())?;
        let (k, v) = rest.split_once('=')?;
        (k == name).then(|| v.split(';').next().unwrap_or("").to_string())
    })
}

/// A device comes in with a fresh key. `ask_owner`: whether the owner is asked first (then it waits, `pending`).
/// Returns its cookie header.
async fn enter(rig: &Rig, ask_owner: bool) -> String {
    rig.app.devices.set_settings(Settings { confirm_devices: ask_owner, ..rig.app.devices.settings() });
    rig.app.devices.rotate_at(now_ms());
    let key = rig.app.devices.link_at(now_ms()).key.expect("a fresh key");
    let (status, head, body) = ask(rig.addr, "POST", "/g/redeem", &through_cloudflare("", &[]), &format!("{{\"key\":\"{key}\",\"touch\":false}}")).await;
    assert_eq!(status, 200, "{body}");
    format!("{DEVICE_COOKIE}={}", set_cookie(&head, DEVICE_COOKIE).expect("the cookie of the device"))
}

/// …and, with the planner's password, raised to the full version.
async fn raise(rig: &Rig, cookie: &str) -> String {
    let body = format!("{{\"user\":\"me\",\"password\":\"{PASSWORD}\"}}");
    let (status, head, text) = ask(rig.addr, "POST", "/api/login", &through_cloudflare(cookie, &[]), &body).await;
    assert_eq!(status, 200, "{text}");
    format!("{cookie}; {}={}", crate::auth::COOKIE, set_cookie(&head, crate::auth::COOKIE).expect("the planner's cookie"))
}

/// The five callers of the sweep: their cookie headers.
struct Callers {
    stranger: String,
    forged: String,
    pending: String,
    lite: String,
    full: String,
}

async fn callers(rig: &Rig) -> Callers {
    let zeros = "0".repeat(64);
    let pending = enter(rig, true).await;
    let lite = enter(rig, false).await;
    let full = enter(rig, false).await;
    let full = raise(rig, &full).await;
    Callers { stranger: NO_ONE.into(), forged: format!("{DEVICE_COOKIE}={zeros}; {}={zeros}; hp_view=full", crate::auth::COOKIE), pending, lite, full }
}

/// Calls that end the caller's own standing (or the gate): a sweep that makes them as a device that is in would saw off
/// the branch it sits on. They are asked of the strangers (who have nothing to lose) and, one by one, in
/// `a_device_may_leave_and_close_the_gate_and_nothing_else`.
fn ends_a_standing(method: &str, pattern: &str) -> bool {
    method == "POST" && matches!(pattern, "/m/api/tunnel/close" | "/m/api/logout" | "/api/logout" | "/g/cancel")
}

/// A concrete path for each pattern of the table.
fn concrete(pattern: &str) -> String {
    pattern.replace("{*path}", "x.js").replace("{n}", "1").replace("{i}", "2").replace("{id}", "3")
}

const METHODS: [&str; 4] = ["GET", "POST", "HEAD", "DELETE"];

/// What a tunnel may reach at all, written out a second time on purpose: the sweeps below take their expectations from
/// `POLICY`, so a route moved from "this machine only" to a door that is open to a device would pass them. This list is
/// the owner's decision in the plan (sections 7.4 and 7.5), and changing it is a decision again: a new door through the
/// tunnel is a line here and in the table, in one reviewed change.
const OPEN_TO_THE_TUNNEL: &[&str] = &[
    // the sign-in page
    "/g/gate.js", "/g/redeem", "/g/status", "/g/cancel",
    // the light version, for a device that is in (and the way to the full one: the planner's own login)
    "/login", "/api/login", "/assets/{*path}", "/favicon.svg", "/favicon.ico", "/g/view",
    "/m", "/m/api/state", "/m/api/poll", "/m/api/chat", "/m/api/chat/history", "/m/api/chat/image/{n}/{i}", "/m/api/action",
    "/m/api/menu/answer", "/m/api/coordinator/screen", "/m/api/details", "/m/api/session", "/m/api/logout", "/m/api/tunnel/close",
    // the full version, for a device raised by the password
    "/", "/api/logout", "/api/state", "/api/summary", "/api/load", "/api/details", "/api/coordinator/screen", "/api/action",
    "/api/menu/answer", "/api/chat", "/api/chat/history", "/api/chat/image/{n}/{i}", "/api/chat/attach", "/api/attachments/{id}",
    "/api/refresh", "/api/events", "/api/search", "/api/models", "/api/subagent", "/api/task/closing", "/api/hints/slash",
];

#[test]
fn only_the_doors_the_plan_names_are_open_to_the_tunnel() {
    let mut open: Vec<&str> = POLICY.iter().filter(|(_, a)| *a != Access::Local).map(|(p, _)| *p).collect();
    let mut named: Vec<&str> = OPEN_TO_THE_TUNNEL.to_vec();
    open.sort_unstable();
    named.sort_unstable();
    assert_eq!(open, named, "a route went into (or out of) the doors a tunnel may use: is that the owner's decision?");
    // …and whatever is a terminal, a file, a yard, the station, the ship, the memory, a clone, a translation or the tunnel itself stays out
    for (pattern, access) in POLICY {
        let p = *pattern;
        let machine = ["/api/runner", "/api/runners", "/api/strays", "/api/station", "/api/crew", "/api/ship", "/api/memory", "/api/universe", "/api/fs/", "/api/hints/files", "/api/task/terminal", "/api/translate", "/api/tunnel", "/api/limits"];
        if machine.iter().any(|m| p == *m || p.starts_with(&format!("{m}/"))) {
            assert_eq!(*access, Access::Local, "{p}");
        }
    }
    // the light version's door is /m/api (a narrow slice) and the planner's /api is the full version's
    for (pattern, access) in POLICY {
        if pattern.starts_with("/m/api/") {
            assert_eq!(*access, Access::Lite, "{pattern}");
        }
        if pattern.starts_with("/api/") && *access == Access::Lite {
            assert!(matches!(*pattern, "/api/login"), "{pattern}: a light device may use the planner's API only to log in");
        }
    }
}

#[tokio::test]
async fn every_route_gives_every_caller_what_the_table_says() {
    let r = rig().await;
    let c = callers(&r).await;
    let mut asked = 0;
    for (pattern, access) in POLICY {
        let path = concrete(pattern);
        for method in METHODS {
            for (who, cookie) in [("a stranger", &c.stranger), ("a forged cookie", &c.forged), ("a device that waits", &c.pending), ("a light device", &c.lite), ("a full device", &c.full)] {
                let standing_ends = who != "a stranger" && who != "a forged cookie" && ends_a_standing(method, pattern);
                if standing_ends {
                    continue;
                }
                let (status, _, body) = ask(r.addr, method, &path, &through_cloudflare(cookie, &[]), "").await;
                asked += 1;
                let here = format!("{who}: {method} {pattern} -> {status} {body}");
                let page = matches!(*pattern, "/" | "/m") && matches!(method, "GET" | "HEAD");
                let not_in = matches!(who, "a stranger" | "a forged cookie" | "a device that waits");
                match access {
                    // this machine only: nobody reaches it through a tunnel, whatever they hold
                    Access::Local => assert_eq!(status, 404, "{here}"),
                    // the sign-in page's own calls: answered by themselves, never hidden, never asked to sign in
                    Access::Public => assert!(![401, 404, 421, 503].contains(&status), "{here}"),
                    Access::Lite | Access::Full if not_in => {
                        if page {
                            assert_eq!(status, 200, "a page asked for by someone who is not in is the sign-in page: {here}");
                        } else {
                            assert_eq!(status, 401, "{here}");
                        }
                    }
                    Access::Full if who == "a light device" => {
                        let landing = *pattern == "/" && method != "POST" && method != "DELETE";
                        if landing {
                            assert_eq!(status, 303, "the planner's page sends a light device on: {here}");
                        } else {
                            assert_eq!(status, 403, "{here}");
                        }
                    }
                    // a device that is in, at the door it was given: the gate lets it by (the route answers as it will)
                    Access::Lite | Access::Full => assert!(![401, 421, 503].contains(&status), "{here}"),
                }
            }
        }
    }
    assert!(asked > 2000, "only {asked} calls: has the table shrunk?");
}

#[tokio::test]
async fn a_name_that_is_not_ours_gets_nothing_whoever_it_holds() {
    let r = rig().await;
    let c = callers(&r).await;
    for cookie in [&c.stranger, &c.forged, &c.pending, &c.lite, &c.full] {
        for (pattern, _) in POLICY {
            for method in ["GET", "POST"] {
                // no Cf-* header: a page of another site whose name points at 127.0.0.1 (with one, the request is the tunnel's, and a stranger's)
                let mut h = vec![("Host", "attacker.example:7117"), ("Cookie", cookie.as_str()), ("Origin", "http://attacker.example:7117")];
                if cookie.is_empty() {
                    h.retain(|(k, _)| *k != "Cookie");
                }
                let (status, _, body) = ask(r.addr, method, &concrete(pattern), &h, "").await;
                assert_eq!(status, 421, "{method} {pattern}: {body}");
            }
        }
    }
}

#[tokio::test]
async fn a_device_that_is_in_cannot_pass_itself_off_as_this_machine() {
    let r = rig().await;
    let c = callers(&r).await;
    let port = r.addr.port().to_string();
    let local_host = format!("127.0.0.1:{port}");
    let origin = format!("http://127.0.0.1:{port}");
    // everything a client can write to look like the machine itself, with and without the marks of the tunnel
    let forged: Vec<Vec<(&str, &str)>> = vec![
        vec![("Host", "localhost")],
        vec![("Host", &local_host)],
        vec![("Host", "[::1]")],
        vec![("X-Forwarded-For", "127.0.0.1")],
        vec![("X-Forwarded-For", "::1"), ("X-Real-Ip", "127.0.0.1")],
        vec![("Forwarded", "for=127.0.0.1;host=localhost;proto=http")],
        vec![("Cf-Connecting-Ip", "127.0.0.1")],
        vec![("X-Forwarded-Host", "localhost")],
        vec![("Origin", &origin), ("Referer", &origin)],
        vec![("Host", "localhost"), ("X-Forwarded-For", "127.0.0.1"), ("Origin", &origin)],
    ];
    for cookie in [&c.lite, &c.full] {
        for extra in &forged {
            // the marks of the tunnel stay or go: a request with the address of the tunnel as its Host, or with a Cf-* header,
            // is the tunnel's whatever else it says
            for with_marks in [true, false] {
                let mut h: Vec<(&str, &str)> = through_cloudflare(cookie, &[]);
                if !with_marks {
                    h.retain(|(k, _)| !k.starts_with("Cf-"));
                }
                for (k, v) in extra {
                    h.retain(|(old, _)| old != k);
                    h.push((k, v));
                }
                // Without a Cf-* header the tunnel is told by its address in `Host`. A request with neither, and with a name of ours,
                // is the owner's own browser or reverse proxy (it meets the planner's login): not a case for the gate.
                if !with_marks && h.iter().any(|(k, v)| *k == "Host" && *v != ADDRESS) {
                    continue;
                }
                for (pattern, access) in POLICY {
                    if *access != Access::Local {
                        continue;
                    }
                    for method in ["GET", "POST"] {
                        let (status, _, body) = ask(r.addr, method, &concrete(pattern), &h, "").await;
                        assert_eq!(status, 404, "{method} {pattern} {extra:?} marks={with_marks}: {body}");
                        assert!(!body.contains("\"runners\"") && !body.contains("\"limits\""), "{method} {pattern}: {body}");
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_path_dressed_up_borrows_no_access() {
    let r = rig().await;
    let c = callers(&r).await;
    // each of these names a route that is for this machine only, or one that does not exist, in a way a router or a proxy might
    // take for the real thing
    let dressed = [
        "//api/limits",
        "/api/limits/",
        "/api/limits/.",
        "/API/limits",
        "/api/Limits",
        "/api/%6cimits",
        "/api/limits%00",
        "/api/limits%2f",
        "/api/limits;x=1",
        "/api/./limits",
        "/api/state/../limits",
        "/m/../api/limits",
        "/m/api/../../api/limits",
        "/%2e%2e/api/limits",
        "/assets/../api/limits",
        "/assets/%2e%2e/api/limits",
        "/g/../api/tunnel",
        "/api/tunnel/",
        "/api/tunnel/open/",
        "/api//tunnel",
        "/api/tunnel%2fopen",
        "/m/api/tunnel/open",
        "/m/api/limits",
        "/m/api/runners",
        "/m/api/tunnel",
        "/m/api/task/terminal",
        "/m/api/memory",
        "/m/api/station",
        "/m/api/state/",
        "/m/api/",
        "/m//api/state",
    ];
    for cookie in [&c.stranger, &c.lite, &c.full] {
        for path in dressed {
            for method in ["GET", "POST"] {
                let (status, _, body) = ask(r.addr, method, path, &through_cloudflare(cookie, &[]), "{}").await;
                // 405: a route of the table that is there for another method (`/assets/` is read with GET), nothing is given
                assert!(matches!(status, 404 | 401 | 400 | 405), "{method} {path}: {status} {body}");
                assert!(!body.contains("\"runners\"") && !body.contains("\"level\"") && !body.contains("\"sessions\""), "{method} {path}: {body}");
            }
        }
    }
}

#[tokio::test]
async fn a_key_and_a_cookie_do_not_travel_over_plain_http() {
    let r = rig().await;
    r.app.devices.set_settings(Settings { confirm_devices: false, ..r.app.devices.settings() });
    r.app.devices.rotate_at(now_ms());
    let key = r.app.devices.link_at(now_ms()).key.unwrap();
    // Cloudflare says the visitor came by plain http, or X-Forwarded-Proto does
    for mark in [("Cf-Visitor", "{\"scheme\":\"http\"}"), ("X-Forwarded-Proto", "http")] {
        let mut h = through_cloudflare("", &[]);
        h.retain(|(k, _)| *k != "Cf-Visitor");
        h.push(mark);
        let (status, head, _) = ask(r.addr, "POST", "/g/redeem", &h, &format!("{{\"key\":\"{key}\"}}")).await;
        assert_eq!(status, 403, "{mark:?}");
        assert!(!head.to_lowercase().contains("set-cookie"), "{head}");
    }
    // the key was not spent by those: it works over https
    let (status, _, body) = ask(r.addr, "POST", "/g/redeem", &through_cloudflare("", &[]), &format!("{{\"key\":\"{key}\"}}")).await;
    assert_eq!(status, 200, "{body}");
    // a device that is in is refused over plain http, whatever it holds
    let c = callers(&r).await;
    for cookie in [&c.lite, &c.full] {
        let mut h = through_cloudflare(cookie, &[]);
        h.retain(|(k, _)| *k != "Cf-Visitor");
        h.push(("Cf-Visitor", "{\"scheme\":\"http\"}"));
        for path in ["/m/api/state", "/api/state", "/m"] {
            assert_eq!(ask(r.addr, "GET", path, &h, "").await.0, 403, "{path}");
        }
    }
}

#[tokio::test]
async fn a_device_that_is_taken_off_loses_the_planner_with_it() {
    let r = rig().await;
    let c = callers(&r).await;
    let full_token = c.full.split(';').next().unwrap().trim_start_matches(&format!("{DEVICE_COOKIE}=")).to_string();
    let (status, _, _) = ask(r.addr, "GET", "/api/state", &through_cloudflare(&c.full, &[]), "").await;
    assert_eq!(status, 200);
    // the owner switches it off: the device cookie *and* the planner's session it earned are of no use through the tunnel
    let id = r.app.devices.list_at(now_ms()).into_iter().find(|d| d.scope == crate::devices::Scope::Full).expect("the full device").id;
    r.app.devices.decide_at(now_ms(), &id, crate::devices::Op::Revoke).unwrap();
    for (path, want) in [("/api/state", 401), ("/m/api/state", 401), ("/api/events", 401), ("/api/chat/history", 401)] {
        let (status, _, body) = ask(r.addr, "GET", path, &through_cloudflare(&c.full, &[]), "").await;
        assert_eq!(status, want, "{path}: {body}");
    }
    // the planner's cookie alone, without a device, is nobody's either
    let planner_only = c.full.split("; ").last().unwrap().to_string();
    assert!(planner_only.starts_with(crate::auth::COOKIE), "{planner_only}");
    assert_eq!(ask(r.addr, "GET", "/api/state", &through_cloudflare(&planner_only, &[]), "").await.0, 401);
    assert!(!full_token.is_empty());

    // closing the gate takes everybody: 503 to the rest of the callers, the open ones included
    r.app.devices.end();
    r.app.gate.set_tunnel(None);
    for cookie in [&c.lite, &c.pending, &c.forged] {
        for path in ["/m/api/state", "/api/state", "/g/status", "/"] {
            assert_eq!(ask(r.addr, "GET", path, &through_cloudflare(cookie, &[]), "").await.0, 503, "{path}");
        }
    }
    // a gate that opens again at the same address does not know the cookies of the time before
    r.app.gate.set_tunnel(Some(ADDRESS));
    r.app.devices.begin_at(now_ms());
    for cookie in [&c.lite, &c.full] {
        assert_eq!(ask(r.addr, "GET", "/m/api/state", &through_cloudflare(cookie, &[]), "").await.0, 401);
        assert_eq!(ask(r.addr, "GET", "/api/state", &through_cloudflare(cookie, &[]), "").await.0, 401);
    }
}

#[tokio::test]
async fn a_device_may_leave_and_close_the_gate_and_nothing_else() {
    let r = rig().await;
    // leaving ends the device's standing only
    let me = enter(&r, false).await;
    let other = enter(&r, false).await;
    assert_eq!(ask(r.addr, "POST", "/m/api/logout", &through_cloudflare(&me, &[]), "{}").await.0, 200);
    assert_eq!(ask(r.addr, "GET", "/m/api/state", &through_cloudflare(&me, &[]), "").await.0, 401);
    assert_eq!(ask(r.addr, "GET", "/m/api/state", &through_cloudflare(&other, &[]), "").await.0, 200, "the other device stays");
    // closing from a phone is the one decision about the gate a device has; opening, rotating, deciding are not
    for path in ["/api/tunnel/open", "/api/tunnel/rotate", "/api/tunnel/session", "/api/tunnel/settings", "/api/tunnel/config", "/api/tunnel/close"] {
        assert_eq!(ask(r.addr, "POST", path, &through_cloudflare(&other, &[]), "{}").await.0, 404, "{path}");
    }
    assert!(r.app.gate.tunnel_open(), "none of them closed it");
    let (status, _, body) = ask(r.addr, "POST", "/m/api/tunnel/close", &through_cloudflare(&other, &[("Origin", &format!("https://{ADDRESS}"))]), "{}").await;
    // (the stand-in rig has no tunnel process behind the gate to close: the handler's own "not open" is an answer, not the gate's refusal)
    assert!(![401, 403, 404, 421, 503].contains(&status), "{status} {body}");
}

#[tokio::test]
async fn what_the_gate_says_carries_the_security_headers() {
    let r = rig().await;
    let c = callers(&r).await;
    // an answer of every kind the gate gives: a page, a refusal, a redirect, a hidden route, a misdirected request
    let cases: Vec<(&str, &str, Vec<(&str, &str)>)> = vec![
        ("GET", "/", through_cloudflare(&c.stranger, &[])),
        ("GET", "/api/state", through_cloudflare(&c.stranger, &[])),
        ("GET", "/api/limits", through_cloudflare(&c.full, &[])),
        ("GET", "/api/state", through_cloudflare(&c.lite, &[])),
        ("GET", "/", through_cloudflare(&c.lite, &[])),
        ("GET", "/m/api/state", through_cloudflare(&c.lite, &[])),
        ("GET", "/g/gate.js", through_cloudflare(&c.stranger, &[])),
        ("GET", "/api/state", {
            let mut h = through_cloudflare(&c.stranger, &[]);
            h.retain(|(k, _)| *k != "Host");
            h.push(("Host", "attacker.example"));
            h
        }),
    ];
    for (method, path, headers) in &cases {
        let (_, head, _) = ask(r.addr, method, path, headers, "").await;
        let head = head.to_lowercase();
        for want in ["referrer-policy: no-referrer", "x-frame-options: deny", "x-content-type-options: nosniff", "content-security-policy: default-src 'self'", "frame-ancestors 'none'", "cache-control: "] {
            assert!(head.contains(want), "{method} {path}: no {want} in {head}");
        }
        assert!(!head.contains("access-control-allow-origin"), "{method} {path}: {head}");
    }
    // …and the closed gate
    r.app.gate.set_tunnel(None);
    let (_, head, _) = ask(r.addr, "GET", "/", &through_cloudflare("", &[("Accept", "text/html")]), "").await;
    assert!(head.to_lowercase().contains("referrer-policy: no-referrer") && head.to_lowercase().contains("x-frame-options: deny"), "{head}");
}
