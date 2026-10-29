//! The choice of version: the table of cases of the plan (section 6) first as plain functions, then as a device meets
//! it through an open gate (the redirect, the hand switch, the password that raises a device).

use super::*;
use crate::app::{self, Shared};
use crate::config::{now_ms, AppConfig, TunnelMode};
use crate::devices::COOKIE as DEVICE_COOKIE;
use crate::testutil::test_cfg;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
const ANDROID_PHONE: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36";
const ANDROID_TABLET: &str = "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const ANDROID_DESKTOP_SITE: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const FIREFOX_ANDROID: &str = "Mozilla/5.0 (Android 14; Mobile; rv:125.0) Gecko/125.0 Firefox/125.0";
const IPAD_OLD: &str = "Mozilla/5.0 (iPad; CPU OS 12_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/12.0 Mobile/15E148 Safari/604.1";
const MAC_SAFARI: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
const WINDOWS_CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const LINUX_FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0";

#[test]
fn the_kind_of_machine_comes_from_the_hint_the_user_agent_and_the_touch_screen() {
    // (what the browser says: user agent, Sec-CH-UA-Mobile, a touch screen the sign-in page found) → the kind of machine
    let table: &[(&str, &str, Option<&str>, bool, Form)] = &[
        ("iPhone", IPHONE, None, true, Form::Phone),
        ("Android phone: the hint", ANDROID_PHONE, Some("?1"), true, Form::Phone),
        ("Android phone: the user agent alone", ANDROID_PHONE, None, true, Form::Phone),
        ("Firefox on Android", FIREFOX_ANDROID, None, true, Form::Phone),
        ("the hint alone is enough", WINDOWS_CHROME, Some("?1"), false, Form::Phone),
        ("Android tablet: no «Mobile»", ANDROID_TABLET, Some("?0"), true, Form::Tablet),
        ("an old iPad that says so", IPAD_OLD, None, true, Form::Tablet),
        ("iPadOS 13+ says «Macintosh»: only the touch screen tells", MAC_SAFARI, None, true, Form::Tablet),
        ("a phone's «desktop site» is a Linux with a touch screen", ANDROID_DESKTOP_SITE, Some("?0"), true, Form::Tablet),
        ("a Mac without a touch screen", MAC_SAFARI, None, false, Form::Desktop),
        ("Windows", WINDOWS_CHROME, Some("?0"), false, Form::Desktop),
        ("Linux", LINUX_FIREFOX, None, false, Form::Desktop),
        ("a Windows laptop with a touch screen is still a computer", WINDOWS_CHROME, Some("?0"), true, Form::Desktop),
        ("nothing is said", "", None, false, Form::Desktop),
    ];
    for (name, ua, hint, touch, want) in table {
        assert_eq!(form_from(ua, *hint, *touch), *want, "{name}");
    }
}

#[test]
fn the_version_is_chosen_by_the_machine_the_switch_and_the_right_to_the_full_one() {
    use Choice::{Full, Lite, Password};
    // (machine, the person's switch, the device has the full right, the full version can be had) → where it goes
    let table: &[(&str, Form, Option<View>, bool, bool, Choice)] = &[
        ("a phone", Form::Phone, None, false, true, Lite),
        ("a tablet", Form::Tablet, None, false, true, Lite),
        ("a computer, the full version allowed and a password set: the password first", Form::Desktop, None, false, true, Password),
        ("a computer that has entered it", Form::Desktop, None, true, true, Full),
        ("a computer, the full version switched off or no password", Form::Desktop, None, false, false, Lite),
        ("a computer that chose the light version", Form::Desktop, Some(View::Lite), true, true, Lite),
        ("a phone that chose the full version: the password first", Form::Phone, Some(View::Full), false, true, Password),
        ("a phone that chose the full version and has it", Form::Phone, Some(View::Full), true, true, Full),
        ("a phone that chose the full version, which is closed", Form::Phone, Some(View::Full), false, false, Lite),
        ("a right the setting no longer backs gives nothing", Form::Desktop, Some(View::Full), true, false, Lite),
    ];
    for (name, form, pref, raised, can, want) in table {
        assert_eq!(choose(*form, *pref, *raised, *can), *want, "{name}");
    }
    assert_eq!([Lite.path(), Full.path(), Password.path()], ["/m", "/", "/login?next=/"]);
}

#[test]
fn what_the_light_page_says_about_the_full_version() {
    assert_eq!(full_state(false, true, false), FullState::Closed);
    assert_eq!(full_state(false, false, false), FullState::Closed);
    assert_eq!(full_state(true, false, false), FullState::Nopassword);
    assert_eq!(full_state(true, true, false), FullState::Login);
    assert_eq!(full_state(true, true, true), FullState::Yes);
}

#[test]
fn the_preference_is_read_from_its_cookie_and_nothing_else_counts() {
    let with = |c: &str| {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_str(c).unwrap());
        preference(&h)
    };
    assert_eq!(with("a=1; hp_view=lite"), Some(View::Lite));
    assert_eq!(with("hp_view=full"), Some(View::Full));
    for bad in ["hp_view=", "hp_view=FULL", "hp_view=admin", "x_hp_view=full", "hp_view_=full"] {
        assert_eq!(with(bad), None, "{bad}");
    }
    assert_eq!(preference(&HeaderMap::new()), None);
    let c = cookie_header(View::Full);
    assert!(c.starts_with("hp_view=full;") && c.contains("Secure") && c.contains("SameSite=Strict") && c.contains("HttpOnly"), "{c}");
}

// ---- as a device meets it through an open gate ----

const ADDRESS: &str = "abc-def.trycloudflare.com";
const PASSWORD: &str = "hunter2";

struct Rig {
    _dir: tempfile::TempDir,
    app: Shared,
    addr: SocketAddr,
}

/// The planner with its gate open. `full`: «the full version for computers» is on; `password`: the planner has one.
async fn rig(full: bool, password: bool) -> Rig {
    rig_of(full, password, TunnelMode::Quick).await
}

/// …and the kind of tunnel it is open on (only a named one passes an event stream).
async fn rig_of(full: bool, password: bool, mode: TunnelMode) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let cfg = AppConfig {
        password_hash: if password { bcrypt::hash(PASSWORD, 4).unwrap() } else { String::new() },
        user: "me".into(),
        tunnel_confirm_devices: false,
        tunnel_full_allowed: full,
        tunnel_mode: mode,
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

async fn ask(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> (u16, String, String) {
    let body = body.unwrap_or("");
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut raw)).await.unwrap().ok();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (status, head.to_string(), body.to_string())
}

fn location(head: &str) -> Option<String> {
    head.lines().find_map(|l| l.to_lowercase().starts_with("location:").then(|| l.split_once(':').unwrap().1.trim().to_string()))
}

/// The value of the cookie a response sets.
fn set_cookie(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|l| {
        let rest = l.to_lowercase().starts_with("set-cookie:").then(|| l.split_once(':').unwrap().1.trim())?;
        let (k, v) = rest.split_once('=')?;
        (k == name).then(|| v.split(';').next().unwrap_or("").to_string())
    })
}

/// A device: what its browser sends through Cloudflare, and the cookies it has.
struct Device<'a> {
    rig: &'a Rig,
    ua: &'a str,
    hint: Option<&'a str>,
    cookies: Vec<(String, String)>,
}

impl<'a> Device<'a> {
    fn headers(&self) -> Vec<(&str, String)> {
        let mut h = vec![("Host", ADDRESS.to_string()), ("Cf-Ray", "x".into()), ("Cf-Visitor", "{\"scheme\":\"https\"}".into()), ("Cf-Connecting-Ip", "203.0.113.9".into()), ("User-Agent", self.ua.to_string())];
        if let Some(hint) = self.hint {
            h.push(("Sec-CH-UA-Mobile", hint.to_string()));
        }
        if !self.cookies.is_empty() {
            h.push(("Cookie", self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")));
        }
        h
    }

    async fn ask(&mut self, method: &str, path: &str, body: Option<&str>) -> (u16, String, String) {
        let owned = self.headers();
        let h: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let r = ask(self.rig.addr, method, path, &h, body).await;
        for name in [DEVICE_COOKIE, "herdr_planner_session", COOKIE] {
            if let Some(v) = set_cookie(&r.1, &name.to_lowercase()).or_else(|| set_cookie(&r.1, name)) {
                self.cookies.retain(|(k, _)| k != name);
                if !v.is_empty() {
                    self.cookies.push((name.to_string(), v));
                }
            }
        }
        r
    }

    /// Knocks with a fresh key; the answer of `/g/redeem`.
    async fn knock(rig: &'a Rig, ua: &'a str, hint: Option<&'a str>, touch: bool) -> (Device<'a>, serde_json::Value) {
        rig.app.devices.rotate_at(now_ms());
        let key = rig.app.devices.link_at(now_ms()).key.unwrap();
        let mut d = Device { rig, ua, hint, cookies: Vec::new() };
        let (status, _, body) = d.ask("POST", "/g/redeem", Some(&format!("{{\"key\":\"{key}\",\"touch\":{touch}}}"))).await;
        assert_eq!(status, 200, "{body}");
        (d, serde_json::from_str(&body).unwrap())
    }
}

#[tokio::test]
async fn the_first_page_of_a_device_follows_the_table_of_the_plan() {
    // (name, user agent, hint, touch screen, the full version allowed, a password set) → where /g/redeem sends the device
    let table: &[(&str, &str, Option<&str>, bool, bool, bool, &str)] = &[
        ("iPhone", IPHONE, None, true, true, true, "/m"),
        ("Android phone", ANDROID_PHONE, Some("?1"), true, true, true, "/m"),
        ("Android tablet", ANDROID_TABLET, Some("?0"), true, true, true, "/m"),
        ("iPad that looks like a Mac", MAC_SAFARI, None, true, true, true, "/m"),
        ("a computer, full allowed, password set", WINDOWS_CHROME, Some("?0"), false, true, true, "/login?next=/"),
        ("a computer, the full version switched off", WINDOWS_CHROME, Some("?0"), false, false, true, "/m"),
        ("a computer, no password", WINDOWS_CHROME, Some("?0"), false, true, false, "/m"),
        ("a computer, neither", LINUX_FIREFOX, None, false, false, false, "/m"),
    ];
    for (name, ua, hint, touch, full, password, want) in table {
        let r = rig(*full, *password).await;
        let (_, answer) = Device::knock(&r, ua, *hint, *touch).await;
        assert_eq!(answer["next"], *want, "{name}: {answer}");
        assert_eq!(answer["state"], "active", "{name}");
    }
}

#[tokio::test]
async fn the_planners_page_sends_a_device_to_its_version() {
    let r = rig(true, true).await;
    // a phone: the light page
    let (mut phone, _) = Device::knock(&r, IPHONE, None, true).await;
    let (status, head, _) = phone.ask("GET", "/", None).await;
    assert_eq!((status, location(&head).as_deref()), (303, Some("/m")), "{head}");
    // a computer without the password: the planner's own login
    let (mut pc, _) = Device::knock(&r, WINDOWS_CHROME, Some("?0"), false).await;
    let (status, head, _) = pc.ask("GET", "/", None).await;
    assert_eq!((status, location(&head).as_deref()), (303, Some("/login?next=/")), "{head}");
    // …and the planner's data stays shut to it
    assert_eq!(pc.ask("GET", "/api/state", None).await.0, 403);
    // the light page is open to both
    assert_eq!(pc.ask("GET", "/m", None).await.0, 200);
    assert_eq!(phone.ask("GET", "/m", None).await.0, 200);
    // a stranger meets the sign-in page and not a redirect
    let none = [("Host", ADDRESS), ("Cf-Ray", "x"), ("Cf-Visitor", "{\"scheme\":\"https\"}")];
    let (status, _, body) = ask(r.addr, "GET", "/", &none, None).await;
    assert_eq!(status, 200);
    assert!(body.contains("/g/gate.js"), "{body}");
    // a post to the planner's page is no navigation: a device with the light version is refused
    assert_eq!(phone.ask("POST", "/", Some("{}")).await.0, 403);
}

#[tokio::test]
async fn the_hand_switch_is_a_cookie_that_gives_no_right() {
    let r = rig(true, true).await;
    // a computer chooses the light version, by the link or by the path of the menu
    let (mut pc, _) = Device::knock(&r, WINDOWS_CHROME, Some("?0"), false).await;
    let (status, head, _) = pc.ask("GET", "/?view=lite", None).await;
    assert_eq!((status, location(&head).as_deref(), set_cookie(&head, "hp_view").as_deref()), (303, Some("/m"), Some("lite")), "{head}");
    assert!(head.contains("SameSite=Strict"), "{head}");
    assert_eq!(pc.cookies.last().map(|c| c.0.as_str()), Some("hp_view"));
    // now `/` goes to /m by itself
    let (status, head, _) = pc.ask("GET", "/", None).await;
    assert_eq!((status, location(&head).as_deref()), (303, Some("/m")), "{head}");
    // a phone chooses the full version: it must still enter the password
    let (mut phone, _) = Device::knock(&r, IPHONE, None, true).await;
    let (status, head, _) = phone.ask("GET", "/g/view?v=full", None).await;
    assert_eq!((status, location(&head).as_deref(), set_cookie(&head, "hp_view").as_deref()), (303, Some("/"), Some("full")), "{head}");
    let (_, head, _) = phone.ask("GET", "/", None).await;
    assert_eq!(location(&head).as_deref(), Some("/login?next=/"), "{head}");
    assert_eq!(phone.ask("GET", "/api/state", None).await.0, 403, "the cookie is a wish and not a right");
    // from the light page's own link
    let (_, head, _) = phone.ask("GET", "/m?view=lite", None).await;
    assert_eq!((location(&head).as_deref(), set_cookie(&head, "hp_view").as_deref()), (Some("/m"), Some("lite")), "{head}");
    // an unknown word is no switch
    assert_eq!(phone.ask("GET", "/g/view?v=admin", None).await.0, 400);
    let (status, _, _) = phone.ask("GET", "/m?view=admin", None).await;
    assert_eq!(status, 200);
    // a forged cookie of a device that is not in
    let none = [("Host", ADDRESS), ("Cf-Ray", "x"), ("Cf-Visitor", "{\"scheme\":\"https\"}"), ("Cookie", "hp_view=full")];
    assert_eq!(ask(r.addr, "GET", "/api/state", &none, None).await.0, 401);
    assert_eq!(ask(r.addr, "GET", "/g/view?v=full", &none, None).await.0, 401, "the switch is for a device that is in");
}

#[tokio::test]
async fn the_planners_password_raises_a_device_to_the_full_version() {
    let r = rig(true, true).await;
    let (mut pc, _) = Device::knock(&r, WINDOWS_CHROME, Some("?0"), false).await;
    // the login page opens for a device with the light version
    let (status, _, _) = pc.ask("GET", "/login?next=/", None).await;
    assert_eq!(status, 200);
    // a wrong password raises nothing
    let wrong = format!("{{\"user\":\"me\",\"password\":\"nope\"}}");
    let (status, _, body) = pc.ask("POST", "/api/login", Some(&wrong)).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(pc.ask("GET", "/api/state", None).await.0, 403);
    // the right one does, and sets the planner's cookie as well
    let right = format!("{{\"user\":\"me\",\"password\":\"{PASSWORD}\"}}");
    let (status, head, body) = pc.ask("POST", "/api/login", Some(&right)).await;
    assert_eq!(status, 200, "{body}");
    assert!(set_cookie(&head, "herdr_planner_session").is_some(), "{head}");
    let (status, _, body) = pc.ask("GET", "/api/state", None).await;
    assert_eq!(status, 200, "{body}");
    let (status, head, _) = pc.ask("GET", "/", None).await;
    assert_ne!(status, 303, "the planner's page is served: {head}");
    assert_eq!(r.app.devices.list_at(now_ms())[0].scope, crate::devices::Scope::Full);
    // the page of the login now goes on to the planner
    let (_, head, _) = pc.ask("GET", "/login", None).await;
    assert_eq!(location(&head).as_deref(), Some("/"), "{head}");
    // what is for this machine only stays so, also for a raised device
    for path in ["/api/tunnel", "/api/runners", "/api/memory", "/api/task/terminal"] {
        assert_eq!(pc.ask("GET", path, None).await.0, 404, "{path}");
    }
    // the person can still go back to the light version, which keeps the right
    let (_, head, _) = pc.ask("GET", "/?view=lite", None).await;
    assert_eq!(location(&head).as_deref(), Some("/m"));
    assert_eq!(pc.ask("GET", "/api/state", None).await.0, 200);
    // the setting is switched off: the right goes with it
    r.app.devices.set_settings(crate::devices::Settings { full_allowed: false, ..r.app.devices.settings() });
    assert_eq!(pc.ask("GET", "/api/state", None).await.0, 403);
    assert_eq!(r.app.devices.list_at(now_ms())[0].scope, crate::devices::Scope::Lite);
    assert!(r.app.devices.take_notes().iter().any(|l| l.contains("вернулось к лёгкой")), "the journal says it");
}

#[tokio::test]
async fn without_the_setting_or_a_password_there_is_no_way_to_the_full_version() {
    for (full, password) in [(false, true), (true, false), (false, false)] {
        let r = rig(full, password).await;
        let (mut pc, _) = Device::knock(&r, WINDOWS_CHROME, Some("?0"), false).await;
        let right = format!("{{\"user\":\"me\",\"password\":\"{PASSWORD}\"}}");
        let (status, _, _) = pc.ask("POST", "/api/login", Some(&right)).await;
        if password {
            assert_eq!(status, 403, "full={full}: the right password is not even looked at");
        }
        assert_eq!(pc.ask("GET", "/api/state", None).await.0, 403, "full={full} password={password}");
        // the login page has nothing to offer
        let (status, head, _) = pc.ask("GET", "/login", None).await;
        assert_eq!((status, location(&head).as_deref()), (303, Some("/m")), "full={full} password={password}");
        // a computer is on the light page, which says why
        let (_, _, body) = pc.ask("GET", "/m/api/session", None).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["form"], "desktop");
        assert_eq!(v["full"], if full { "nopassword" } else { "closed" }, "{body}");
        let (_, head, _) = pc.ask("GET", "/", None).await;
        assert_eq!(location(&head).as_deref(), Some("/m"));
        // `?view=full` changes nothing
        let (_, head, _) = pc.ask("GET", "/?view=full", None).await;
        assert_eq!(location(&head).as_deref(), Some("/"));
        let (_, head, _) = pc.ask("GET", "/", None).await;
        assert_eq!(location(&head).as_deref(), Some("/m"), "the wish is not granted");
    }
}

#[tokio::test]
async fn a_device_that_guesses_the_password_is_switched_off() {
    let r = rig(true, true).await;
    let (mut pc, _) = Device::knock(&r, WINDOWS_CHROME, Some("?0"), false).await;
    let wrong = "{\"user\":\"me\",\"password\":\"nope\"}";
    // the address limiter of the planner's login blocks the fifth try; the device's own count reaches five first only
    // when the address changes, so give each try another address
    for i in 0..4 {
        let owned = pc.headers().into_iter().map(|(k, v)| if k == "Cf-Connecting-Ip" { (k, format!("203.0.113.{}", 10 + i)) } else { (k, v) }).collect::<Vec<_>>();
        let h: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(ask(r.addr, "POST", "/api/login", &h, Some(wrong)).await.0, 401, "try {i}");
    }
    let owned = pc.headers().into_iter().map(|(k, v)| if k == "Cf-Connecting-Ip" { (k, "203.0.113.99".to_string()) } else { (k, v) }).collect::<Vec<_>>();
    let h: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (status, _, body) = ask(r.addr, "POST", "/api/login", &h, Some(wrong)).await;
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("dropped"), "{body}");
    assert!(r.app.devices.list_at(now_ms()).is_empty(), "the device is out");
    assert_eq!(pc.ask("GET", "/m/api/state", None).await.0, 401);
    // a right password between the wrong ones starts the count over
    let (mut other, _) = Device::knock(&r, LINUX_FIREFOX, None, false).await;
    let right = format!("{{\"user\":\"me\",\"password\":\"{PASSWORD}\"}}");
    for _ in 0..3 {
        r.app.devices.password_failed_at(now_ms(), &other.cookies[0].1);
    }
    assert_eq!(other.ask("POST", "/api/login", Some(&right)).await.0, 200);
    for _ in 0..4 {
        assert!(!r.app.devices.password_failed_at(now_ms(), &other.cookies[0].1));
    }
}

#[tokio::test]
async fn the_light_page_knows_the_two_versions() {
    let r = rig(true, true).await;
    let (mut pc, _) = Device::knock(&r, WINDOWS_CHROME, Some("?0"), false).await;
    let (_, _, body) = pc.ask("GET", "/m/api/session", None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((v["form"].as_str(), v["full"].as_str(), v["scope"].as_str()), (Some("desktop"), Some("login"), Some("lite")), "{body}");
    let (mut phone, _) = Device::knock(&r, IPHONE, None, true).await;
    let (_, _, body) = phone.ask("GET", "/m/api/session", None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["form"], "phone");
}

#[tokio::test]
async fn from_this_machine_the_planner_is_always_the_full_version() {
    let r = rig(true, false).await;
    // no password: no login to go through; `/` is the planner whatever the browser says, and `?view=` still works
    let local = |extra: &'static str| -> Vec<(&'static str, &'static str)> { vec![("Host", "localhost"), ("User-Agent", IPHONE), ("Cookie", extra)] };
    let (status, head, _) = ask(r.addr, "GET", "/", &local("hp_view=lite"), None).await;
    assert_ne!(location(&head).as_deref(), Some("/m"), "{status} {head}");
    assert_ne!(status, 303, "{head}");
    let (status, head, _) = ask(r.addr, "GET", "/?view=lite", &local("a=1"), None).await;
    assert_eq!((status, location(&head).as_deref()), (303, Some("/m")), "{head}");
    assert_eq!(ask(r.addr, "GET", "/m", &local("a=1"), None).await.0, 200, "/m opens here as well");
    let (_, _, body) = ask(r.addr, "GET", "/m/api/session", &local("hp_view=lite"), None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((v["kind"].as_str(), v["form"].as_str(), v["full"].as_str(), v["view"].as_str()), (Some("local"), Some("desktop"), Some("yes"), Some("lite")), "{body}");
}

#[tokio::test]
async fn five_wrong_passwords_from_one_address_switch_the_device_off() {
    let r = rig(true, true).await;
    let (mut pc, _) = Device::knock(&r, WINDOWS_CHROME, Some("?0"), false).await;
    let wrong = "{\"user\":\"me\",\"password\":\"nope\"}";
    for i in 0..4 {
        assert_eq!(pc.ask("POST", "/api/login", Some(wrong)).await.0, 401, "try {i}");
    }
    // the planner's limiter answers "too many" on this fifth try; the device is out all the same
    let (status, _, body) = pc.ask("POST", "/api/login", Some(wrong)).await;
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("dropped"), "{body}");
    assert!(r.app.devices.list_at(now_ms()).is_empty());
    assert_eq!(pc.ask("GET", "/m/api/state", None).await.0, 401);
}

/// Opens `/api/events` as the device and reads until the first event is there; the rest of the stream is read by the caller.
async fn open_stream(r: &Rig, d: &Device<'_>) -> tokio::net::TcpStream {
    let mut req = String::from("GET /api/events HTTP/1.1\r\nConnection: close\r\nAccept: text/event-stream\r\n");
    for (k, v) in d.headers() {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    let mut s = tokio::net::TcpStream::connect(r.addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut seen = String::new();
    let mut buf = [0u8; 4096];
    while !seen.contains("event: state") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf)).await.expect("the stream starts").unwrap();
        assert!(n > 0, "closed before the first event: {seen}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(seen.starts_with("HTTP/1.1 200"), "{seen}");
    s
}

/// The stream has ended (the server closed it) within a few seconds.
async fn ended(mut s: tokio::net::TcpStream) -> bool {
    let mut sink = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), s.read_to_end(&mut sink)).await.is_ok()
}

async fn raised(r: &Rig) -> Device<'_> {
    let (mut pc, _) = Device::knock(r, WINDOWS_CHROME, Some("?0"), false).await;
    let right = format!("{{\"user\":\"me\",\"password\":\"{PASSWORD}\"}}");
    assert_eq!(pc.ask("POST", "/api/login", Some(&right)).await.0, 200);
    pc
}

#[tokio::test]
async fn an_open_event_stream_ends_when_the_full_version_is_taken_away() {
    // the setting is switched off
    let r = rig_of(true, true, TunnelMode::Named).await;
    let pc = raised(&r).await;
    let s = open_stream(&r, &pc).await;
    r.app.devices.set_settings(crate::devices::Settings { full_allowed: false, ..r.app.devices.settings() });
    assert!(ended(s).await, "the stream outlived the setting");
    // the owner switches the device off
    let r = rig_of(true, true, TunnelMode::Named).await;
    let pc = raised(&r).await;
    let s = open_stream(&r, &pc).await;
    let id = r.app.devices.list_at(now_ms())[0].id.clone();
    r.app.devices.decide_at(now_ms(), &id, crate::devices::Op::Revoke).unwrap();
    assert!(ended(s).await, "the stream outlived the device");
    // the gate closes
    let r = rig_of(true, true, TunnelMode::Named).await;
    let pc = raised(&r).await;
    let s = open_stream(&r, &pc).await;
    r.app.devices.end();
    assert!(ended(s).await, "the stream outlived the gate");
    // …and a device that keeps its right keeps its stream
    let r = rig_of(true, true, TunnelMode::Named).await;
    let pc = raised(&r).await;
    let s = open_stream(&r, &pc).await;
    r.app.bus.send("devices", "{}".to_string());
    assert!(!ended(s).await, "the stream of a device that is in with the full version goes on");
}

#[tokio::test]
async fn a_quick_tunnel_has_no_event_stream_and_the_page_is_told_to_ask() {
    let r = rig(true, true).await;
    let pc = raised(&r).await;
    let owned = pc.headers();
    let h: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (status, _, body) = ask(r.addr, "GET", "/api/events", &h, None).await;
    assert_eq!(status, 410, "{body}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["poll_ms"], 5000);
    // the planner's page says how it comes: through the gate, without the stream; and the number that tells it of a new message
    let (status, _, page) = ask(r.addr, "GET", "/", &h, None).await;
    assert_eq!(status, 200);
    assert!(page.contains("<meta name=\"herdr-gate\" content=\"poll\">"), "{page}");
    let (_, _, state) = ask(r.addr, "GET", "/api/state", &h, None).await;
    let before = serde_json::from_str::<serde_json::Value>(&state).unwrap()["chat_rev"].as_u64().unwrap();
    r.app.bus.send("chat", "{}".to_string());
    let (_, _, state) = ask(r.addr, "GET", "/api/state", &h, None).await;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&state).unwrap()["chat_rev"].as_u64().unwrap(), before + 1);
    // this machine's own page is what it was: a stream, and no mention of a gate
    let r = rig(true, false).await;
    let local = [("Host", "localhost:7117")];
    let (_, _, page) = ask(r.addr, "GET", "/", &local, None).await;
    assert!(page.contains("<html") && !page.contains("herdr-gate"), "{page}");
    let mut s = tokio::net::TcpStream::connect(r.addr).await.unwrap();
    s.write_all(b"GET /api/events HTTP/1.1\r\nHost: localhost:7117\r\nAccept: text/event-stream\r\n\r\n").await.unwrap();
    let mut buf = [0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf)).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
}

#[tokio::test]
async fn a_named_tunnel_passes_the_event_stream() {
    let r = rig_of(true, true, TunnelMode::Named).await;
    let pc = raised(&r).await;
    drop(open_stream(&r, &pc).await);
    let owned = pc.headers();
    let h: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (_, _, page) = ask(r.addr, "GET", "/", &h, None).await;
    assert!(page.contains("<meta name=\"herdr-gate\" content=\"stream\">"), "{page}");
}
