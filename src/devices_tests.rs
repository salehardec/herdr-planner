//! The key, the devices and their clocks, with the time given by the test (every `*_at` takes it).

use super::*;
use crate::testutil::test_cfg;
use std::sync::Arc;

const T0: u64 = 1_000_000_000_000;
const MIN: u64 = 60_000;
const HOUR: u64 = 60 * MIN;

fn devices_with(edit: impl FnOnce(&mut AppConfig)) -> Devices {
    let dir = std::path::Path::new("/nonexistent");
    let mut cfg = test_cfg(dir);
    edit(&mut cfg);
    Devices::new(&cfg)
}

/// A gate that has just opened.
fn opened() -> Devices {
    let d = devices_with(|_| {});
    d.begin_at(T0);
    d
}

fn who(label: &str) -> Who {
    Who { label: label.into(), country: Some("RU".into()), touch: true, form: crate::version::Form::Phone }
}

fn key_of(d: &Devices, now: u64) -> String {
    d.link_at(now).key.expect("a fresh key")
}

/// A device in, with its token.
fn let_in(d: &Devices, now: u64, address: &str, label: &str) -> String {
    d.rotate_at(now);
    let key = key_of(d, now);
    let a = d.redeem_at(now, address, &key, who(label)).expect("the key is right");
    if a.state == Life::Pending {
        let id = d.list_at(now).into_iter().find(|v| v.label == label && v.state == "pending").expect("it waits").id;
        d.decide_at(now, &id, Op::Allow).unwrap();
    }
    a.token
}

#[test]
fn a_gate_that_opens_has_a_key_of_22_characters_and_nobody_in() {
    let d = opened();
    let l = d.link_at(T0);
    let key = l.key.unwrap();
    assert_eq!((key.len(), l.state, l.expires_ms), (22, LinkState::Fresh, Some(T0 + 10 * MIN)));
    assert!(key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{key}");
    assert!(d.list_at(T0).is_empty());
    // two gates never share a key
    let other = opened();
    assert_ne!(key, key_of(&other, T0));
    // closed: nothing is left
    d.end();
    assert_eq!((d.link_at(T0).key, d.link_at(T0).state), (None, LinkState::None));
}

#[test]
fn the_key_works_once_and_for_ten_minutes() {
    let d = opened();
    let key = key_of(&d, T0);
    assert!(d.redeem_at(T0 + 1, "1.1.1.1", &key, who("iPhone · Safari 17")).is_ok());
    assert_eq!(d.link_at(T0 + 2).state, LinkState::Used);
    assert_eq!(d.redeem_at(T0 + 3, "2.2.2.2", &key, who("Android · Chrome 126")).err(), Some(RedeemError::Stale), "used");

    let d = opened();
    let key = key_of(&d, T0);
    assert!(d.link_at(T0 + 10 * MIN - 1).key.is_some());
    assert_eq!(d.redeem_at(T0 + 10 * MIN, "1.1.1.1", &key, who("x")).err(), Some(RedeemError::Stale), "ten minutes are over");
    assert_eq!(d.link_at(T0 + 10 * MIN).state, LinkState::Expired);

    // a wrong key, a key of the length of the right one, no key, a very long one: the same answer
    let d = opened();
    for k in ["", "x", "AAAAAAAAAAAAAAAAAAAAAA", &"k".repeat(5000)] {
        assert_eq!(d.redeem_at(T0, "1.1.1.1", k, who("x")).err(), Some(RedeemError::Stale), "{k:.8}");
    }
    // …and a gate that is not open has no key to guess
    d.end();
    assert_eq!(d.redeem_at(T0, "9.9.9.9", "", who("x")).err(), Some(RedeemError::Stale));
}

#[test]
fn a_new_link_voids_the_old_one() {
    let d = opened();
    let old = key_of(&d, T0);
    d.rotate_at(T0 + MIN);
    let new = key_of(&d, T0 + MIN);
    assert_ne!(old, new);
    assert_eq!(d.redeem_at(T0 + MIN, "1.1.1.1", &old, who("x")).err(), Some(RedeemError::Stale));
    assert!(d.redeem_at(T0 + MIN, "1.1.1.1", &new, who("x")).is_ok());
    // after a used link, a new one is good for another device
    d.rotate_at(T0 + 2 * MIN);
    assert!(d.redeem_at(T0 + 2 * MIN, "2.2.2.2", &key_of(&d, T0 + 2 * MIN), who("y")).is_ok());
}

#[test]
fn two_devices_racing_for_one_key_get_one_session() {
    let d = Arc::new(opened());
    let key = key_of(&d, T0);
    let handles: Vec<_> = (0..16)
        .map(|i| {
            let (d, key) = (d.clone(), key.clone());
            std::thread::spawn(move || d.redeem_at(T0 + 1, &format!("10.0.0.{i}"), &key, who(&format!("device {i}"))).is_ok())
        })
        .collect();
    let wins = handles.into_iter().map(|h| h.join().unwrap()).filter(|w| *w).count();
    assert_eq!(wins, 1);
    assert_eq!(d.list_at(T0 + 1).len(), 1);
}

#[test]
fn a_new_device_waits_for_the_owner_unless_that_is_switched_off() {
    let d = opened();
    let a = d.redeem_at(T0, "1.1.1.1", &key_of(&d, T0), who("iPhone · Safari 17")).unwrap();
    assert_eq!(a.state, Life::Pending);
    assert_eq!(d.standing_at(T0 + 1, Some(&a.token)), Standing::Waiting);
    let v = d.list_at(T0 + 1);
    assert_eq!((v.len(), v[0].state, v[0].label.as_str(), v[0].country.as_deref()), (1, "pending", "iPhone · Safari 17", Some("RU")));
    assert_eq!(v[0].answer_by_ms, Some(T0 + 2 * MIN));
    assert_eq!(d.status_at(T0 + 1, Some(&a.token)).state, "pending");

    d.decide_at(T0 + 5, &v[0].id, Op::Allow).unwrap();
    assert_eq!(d.standing_at(T0 + 6, Some(&a.token)), Standing::Lite);
    assert_eq!(d.status_at(T0 + 6, Some(&a.token)).state, "active");
    assert_eq!(d.decide_at(T0 + 7, &v[0].id, Op::Allow), Err(DecideError::NotPending));
    assert_eq!(d.decide_at(T0 + 7, &v[0].id, Op::Deny), Err(DecideError::NotPending));
    assert_eq!(d.decide_at(T0 + 7, "nobody", Op::Allow), Err(DecideError::NotFound));

    // without the question
    let off = devices_with(|c| c.tunnel_confirm_devices = false);
    off.begin_at(T0);
    let a = off.redeem_at(T0, "1.1.1.1", &key_of(&off, T0), who("Mac · Safari 17")).unwrap();
    assert_eq!(a.state, Life::Active);
    assert_eq!(off.standing_at(T0 + 1, Some(&a.token)), Standing::Lite);
}

#[test]
fn a_device_nobody_answers_is_declined_after_two_minutes() {
    let d = opened();
    let a = d.redeem_at(T0, "1.1.1.1", &key_of(&d, T0), who("iPhone · Safari 17")).unwrap();
    assert_eq!(d.standing_at(T0 + 2 * MIN - 1, Some(&a.token)), Standing::Waiting);
    assert_eq!(d.standing_at(T0 + 2 * MIN, Some(&a.token)), Standing::Nobody);
    let s = d.status_at(T0 + 2 * MIN, Some(&a.token));
    assert_eq!((s.state, s.reason), ("gone", Some(Why::Unanswered)));
    assert!(d.list_at(T0 + 2 * MIN).is_empty());
    // it cannot be let in after that
    let id = d.inner.lock().unwrap().devices[0].id.clone();
    assert_eq!(d.decide_at(T0 + 2 * MIN, &id, Op::Allow), Err(DecideError::NotFound));
    // and the tombstone goes after a while, the cookie is nobody's then
    assert_eq!(d.status_at(T0 + 2 * MIN + GONE_KEEP_MS + 1, Some(&a.token)).state, "none");
}

#[test]
fn the_owner_can_decline_and_switch_off() {
    let d = opened();
    let a = d.redeem_at(T0, "1.1.1.1", &key_of(&d, T0), who("iPhone · Safari 17")).unwrap();
    let id = d.list_at(T0)[0].id.clone();
    d.decide_at(T0 + 1, &id, Op::Deny).unwrap();
    let s = d.status_at(T0 + 2, Some(&a.token));
    assert_eq!((s.state, s.reason), ("gone", Some(Why::Denied)));
    assert_eq!(d.standing_at(T0 + 2, Some(&a.token)), Standing::Nobody);

    let token = let_in(&d, T0 + 10, "2.2.2.2", "Android · Chrome 126");
    assert_eq!(d.standing_at(T0 + 11, Some(&token)), Standing::Lite);
    let id = d.list_at(T0 + 11)[0].id.clone();
    d.decide_at(T0 + 12, &id, Op::Revoke).unwrap();
    assert_eq!(d.standing_at(T0 + 13, Some(&token)), Standing::Nobody);
    assert_eq!(d.status_at(T0 + 13, Some(&token)).reason, Some(Why::Revoked));
    // the cookie of a device that is gone is not brought back by anything
    d.rotate_at(T0 + 14);
    assert_eq!(d.standing_at(T0 + 15, Some(&token)), Standing::Nobody);
}

#[test]
fn a_session_lasts_a_day_and_six_hours_without_a_request() {
    let d = opened();
    let t = let_in(&d, T0, "1.1.1.1", "iPhone · Safari 17");
    // idle: six hours after the last request
    assert_eq!(d.standing_at(T0 + 6 * HOUR - 1, Some(&t)), Standing::Lite);
    assert_eq!(d.standing_at(T0 + 12 * HOUR - 2, Some(&t)), Standing::Lite, "the request above started the six hours over");
    assert_eq!(d.standing_at(T0 + 18 * HOUR - 2, Some(&t)), Standing::Nobody);
    assert_eq!(d.status_at(T0 + 18 * HOUR - 2, Some(&t)).reason, Some(Why::Idle));

    // a day, however busy
    let d = opened();
    let t = let_in(&d, T0, "1.1.1.1", "iPhone · Safari 17");
    let mut now = T0;
    while now < T0 + 24 * HOUR - HOUR {
        now += HOUR;
        assert_eq!(d.standing_at(now, Some(&t)), Standing::Lite);
    }
    assert_eq!(d.standing_at(T0 + 24 * HOUR, Some(&t)), Standing::Nobody);
    assert_eq!(d.status_at(T0 + 24 * HOUR, Some(&t)).reason, Some(Why::Expired));
}

#[test]
fn a_cookie_of_nobody_is_nobody() {
    let d = opened();
    let t = let_in(&d, T0, "1.1.1.1", "iPhone · Safari 17");
    for c in [None, Some(""), Some("00"), Some(t.as_str().get(..63).unwrap()), Some("zz")] {
        assert_eq!(d.standing_at(T0 + 1, c), Standing::Nobody, "{c:?}");
        assert_eq!(d.status_at(T0 + 1, c).state, "none", "{c:?}");
    }
    // the cookie of another gate is not good for this one
    let other = opened();
    let t2 = let_in(&other, T0, "1.1.1.1", "iPhone · Safari 17");
    assert_ne!(t, t2);
    assert_eq!(d.standing_at(T0 + 1, Some(&t2)), Standing::Nobody);
}

#[test]
fn at_most_five_devices_are_in_and_a_full_gate_does_not_burn_the_key() {
    let d = opened();
    let mut tokens = Vec::new();
    for i in 0..MAX_DEVICES {
        tokens.push(let_in(&d, T0 + i as u64, &format!("10.0.0.{i}"), &format!("device {i}")));
    }
    assert_eq!(d.list_at(T0 + 10).len(), 5);
    d.rotate_at(T0 + 10);
    let key = key_of(&d, T0 + 10);
    assert_eq!(d.redeem_at(T0 + 10, "10.0.0.9", &key, who("one too many")).err(), Some(RedeemError::Full));
    assert_eq!(d.link_at(T0 + 10).state, LinkState::Fresh, "the key is still good once there is room");
    // a device that is gone makes room; a waiting one takes a place too
    let id = d.list_at(T0 + 11)[0].id.clone();
    d.decide_at(T0 + 11, &id, Op::Revoke).unwrap();
    assert!(d.redeem_at(T0 + 12, "10.0.0.9", &key, who("room now")).is_ok());
    d.rotate_at(T0 + 13);
    assert_eq!(d.redeem_at(T0 + 13, "10.0.0.8", &key_of(&d, T0 + 13), who("full again")).err(), Some(RedeemError::Full), "the waiting one counts");
    assert_eq!(d.standing_at(T0 + 14, Some(&tokens[0])), Standing::Nobody);
}

#[test]
fn five_wrong_keys_block_an_address_and_a_spread_guess_meets_a_ceiling() {
    let d = opened();
    for i in 0..4 {
        assert_eq!(d.redeem_at(T0 + i, "5.5.5.5", "wrong", who("x")).err(), Some(RedeemError::Stale));
    }
    assert_eq!(d.redeem_at(T0 + 4, "5.5.5.5", "wrong", who("x")).err(), Some(RedeemError::Stale), "the fifth is still an answer");
    // blocked: even the right key waits, and it is not burned by that
    let key = key_of(&d, T0 + 5);
    assert_eq!(d.redeem_at(T0 + 5, "5.5.5.5", &key, who("x")).err(), Some(RedeemError::TooMany(300)));
    assert_eq!(d.link_at(T0 + 5).state, LinkState::Fresh);
    // another address is not affected, and the owner's own device gets in
    assert!(d.redeem_at(T0 + 6, "6.6.6.6", &key, who("x")).is_ok());
    // five minutes later the first address may try again
    d.rotate_at(T0 + 5 * MIN + 10);
    assert!(d.redeem_at(T0 + 5 * MIN + 10, "5.5.5.5", &key_of(&d, T0 + 5 * MIN + 10), who("y")).is_ok());

    // forty wrong keys from forty addresses: everybody waits
    let d = opened();
    for i in 0..MAX_FAILS_ALL {
        let _ = d.redeem_at(T0 + u64::from(i), &format!("7.7.7.{i}"), "wrong", who("x"));
    }
    let key = key_of(&d, T0 + 50);
    assert!(matches!(d.redeem_at(T0 + 50, "8.8.8.8", &key, who("x")), Err(RedeemError::TooMany(_))));
    assert!(d.redeem_at(T0 + 6 * MIN, "8.8.8.8", &key, who("x")).is_ok(), "and the ceiling is lifted after five minutes");
}

#[test]
fn closing_the_gate_ends_every_session() {
    let d = opened();
    let t = let_in(&d, T0, "1.1.1.1", "iPhone · Safari 17");
    assert_eq!(d.standing_at(T0 + 1, Some(&t)), Standing::Lite);
    d.end();
    assert_eq!(d.standing_at(T0 + 2, Some(&t)), Standing::Nobody);
    assert!(d.list_at(T0 + 2).is_empty());
    // a gate that opens again (the same address of a named tunnel) does not know that cookie
    d.begin_at(T0 + 3);
    assert_eq!(d.standing_at(T0 + 4, Some(&t)), Standing::Nobody);
    assert_eq!(d.status_at(T0 + 4, Some(&t)).state, "none");
}

#[test]
fn the_gate_is_unused_after_the_hours_of_the_setting() {
    let d = opened();
    assert_eq!(d.settings().idle_close_h, 4);
    assert_eq!(d.idle_due_at(T0 + 4 * HOUR - 1), None);
    assert_eq!(d.idle_due_at(T0 + 4 * HOUR), Some(4));
    assert_eq!(d.idle_closes_at(), Some(T0 + 4 * HOUR));
    // a request of a device that is in starts the clock over; a request that is refused does not
    let t = let_in(&d, T0 + HOUR, "1.1.1.1", "iPhone · Safari 17");
    assert_eq!(d.idle_due_at(T0 + 5 * HOUR - 1), None);
    d.standing_at(T0 + 3 * HOUR, Some(&t));
    d.standing_at(T0 + 4 * HOUR, Some("a stranger's cookie"));
    assert_eq!(d.idle_due_at(T0 + 7 * HOUR - 1), None);
    assert_eq!(d.idle_due_at(T0 + 7 * HOUR), Some(4));
    // the choices: never, 1, 12
    d.set_settings(Settings { idle_close_h: 0, ..d.settings() });
    assert_eq!((d.idle_due_at(T0 + 1000 * HOUR), d.idle_closes_at()), (None, None));
    d.set_settings(Settings { idle_close_h: 1, ..d.settings() });
    assert_eq!(d.idle_due_at(T0 + 4 * HOUR), Some(1));
    d.set_settings(Settings { idle_close_h: 12, ..d.settings() });
    assert_eq!(d.idle_due_at(T0 + 15 * HOUR - 1), None);
}

#[test]
fn a_page_that_only_looks_does_not_keep_the_gate_open() {
    // The light page's long-poll repeats every half minute for as long as the tab lives.
    let d = opened();
    let t = let_in(&d, T0, "1.1.1.1", "iPhone · Safari 17");
    for h in 1..4 {
        assert_eq!(d.standing_quiet_at(T0 + h * HOUR, Some(&t)), Standing::Lite, "still in at {h} h");
    }
    assert_eq!(d.idle_due_at(T0 + 4 * HOUR), Some(4), "four hours of looking is four hours unused");
    // …and the device's own idle clock runs on too
    assert_eq!(d.standing_quiet_at(T0 + 6 * HOUR - 1, Some(&t)), Standing::Lite);
    assert_eq!(d.standing_quiet_at(T0 + 6 * HOUR, Some(&t)), Standing::Nobody);
    assert_eq!(d.status_at(T0 + 6 * HOUR, Some(&t)).reason, Some(Why::Idle));
    // a stranger's cookie is nobody here as everywhere, and a device that waits is still waiting
    let d = opened();
    assert_eq!(d.standing_quiet_at(T0 + 1, Some("00")), Standing::Nobody);
    assert_eq!(d.standing_quiet_at(T0 + 1, None), Standing::Nobody);
}

#[test]
fn a_device_that_is_in_can_ask_who_it_is_without_using_the_gate() {
    let d = opened();
    let t = let_in(&d, T0, "1.1.1.1", "iPhone · Safari 17");
    let s = d.session_at(T0 + 2 * HOUR, &t).expect("in");
    assert_eq!((s.label.as_str(), s.scope, s.since_ms, s.ends_ms), ("iPhone · Safari 17", Scope::Lite, T0, T0 + 6 * HOUR));
    assert_eq!(d.idle_due_at(T0 + 4 * HOUR), Some(4), "asking is no use of the gate");
    assert_eq!(d.session_at(T0 + 2 * HOUR, "00"), None);
    assert_eq!(d.session_at(T0 + 7 * HOUR, &t), None, "idle for six hours: gone");
    // waiting for the owner is not being in
    let d = devices_with(|_| {});
    d.begin_at(T0);
    let key = key_of(&d, T0);
    let a = d.redeem_at(T0, "1.1.1.1", &key, who("Android · Chrome 126")).unwrap();
    assert_eq!(a.state, Life::Pending);
    assert_eq!(d.session_at(T0 + 1, &a.token), None);
}

#[test]
fn what_happens_goes_to_the_journal_once() {
    let d = opened();
    let t = let_in(&d, T0, "1.1.1.1", "iPhone · Safari 17");
    let notes = d.take_notes();
    assert_eq!(notes, ["устройство просит войти: iPhone · Safari 17", "устройство подключено: iPhone · Safari 17"]);
    assert!(d.take_notes().is_empty());
    d.standing_at(T0 + 6 * HOUR, Some(&t));
    assert_eq!(d.take_notes(), ["устройство отключено (по простою: 6 ч): iPhone · Safari 17"]);
    // no key, nor cookie, in any line
    let d = opened();
    let key = key_of(&d, T0);
    let a = d.redeem_at(T0, "1.1.1.1", &key, who("x")).unwrap();
    for n in d.take_notes() {
        assert!(!n.contains(&key) && !n.contains(&a.token), "{n}");
    }
}

#[test]
fn a_device_is_named_by_what_we_know_and_never_by_what_it_says() {
    for (ua, want) in [
        ("Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1", "iPhone · Safari 17"),
        ("Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/126.0.6478.153 Mobile/15E148 Safari/604.1", "iPhone · Chrome 126"),
        ("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36", "Android · Chrome 126"),
        ("Mozilla/5.0 (Linux; Android 13; SM-X700) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36", "Android tablet · Chrome 125"),
        ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15", "Mac · Safari 17"),
        ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0", "Windows · Edge 126"),
        ("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0", "Linux · Firefox 128"),
        ("Mozilla/5.0 (Linux; Android 14; SM-S911B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/25.0 Chrome/121.0.0.0 Mobile Safari/537.36", "Android · Samsung Internet 25"),
        ("", "device"),
        ("curl/8.5.0", "device"),
        ("<script>alert(1)</script>", "device"),
    ] {
        assert_eq!(device_label(ua), want, "{ua}");
    }
    // nothing of the header's own text gets through
    let hostile = "Mozilla/5.0 (iPhone) \"><img src=x onerror=alert(1)> Safari/604.1";
    let l = device_label(hostile);
    assert!(!l.contains('<') && !l.contains('"') && l.len() < 40, "{l}");
}

#[test]
fn the_country_is_two_letters_or_none() {
    let h = |v: &str| {
        let mut m = HeaderMap::new();
        m.insert("cf-ipcountry", HeaderValue::from_str(v).unwrap());
        m
    };
    assert_eq!(country_of(&h("ru")).as_deref(), Some("RU"));
    assert_eq!(country_of(&h("DE")).as_deref(), Some("DE"));
    for bad in ["XX", "T1", "", "RUS", "<b", "1x"] {
        assert_eq!(country_of(&h(bad)), None, "{bad}");
    }
    assert_eq!(country_of(&HeaderMap::new()), None);
}

#[test]
fn the_address_for_the_limit_is_the_one_cloudflare_saw() {
    let mut h = HeaderMap::new();
    assert_eq!(address_of(&h), "-");
    h.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.9"));
    assert_eq!(address_of(&h), "203.0.113.9");
    h.insert("cf-connecting-ip", HeaderValue::from_static("not an address"));
    assert_eq!(address_of(&h), "-");
    h.insert("cf-connecting-ip", HeaderValue::from_static("2001:db8::1"));
    assert_eq!(address_of(&h), "2001:db8::1");
}

#[test]
fn the_cookie_is_a_host_cookie() {
    let c = cookie_header("abc", SESSION_MS / 1000);
    assert_eq!(c, "__Host-hp_g=abc; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=86400");
    assert!(!c.contains("Domain"));
}

#[test]
fn constant_time_compare_agrees_with_equality() {
    assert!(same(b"", b""));
    assert!(same(b"abc", b"abc"));
    for (a, b) in [(&b"abc"[..], &b"abd"[..]), (b"abc", b"ab"), (b"ab", b"abc"), (b"", b"a"), (b"a", b""), (b"abc\0", b"abc")] {
        assert!(!same(a, b), "{a:?} {b:?}");
    }
}

#[test]
fn the_sign_in_page_script_does_what_the_page_promises() {
    // the script against a stand-in browser: scripts/gate-page-check.mjs
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/gate-page-check.mjs");
    let out = std::process::Command::new("node").arg(script).output().expect("node is needed for the page checks");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("all ok"), "{text}");
}

#[test]
fn the_sign_in_page_is_static_and_holds_no_secret() {
    for (name, text) in [("gate.html", GATE_HTML), ("gate.js", GATE_JS), ("the closed page", CLOSED_HTML)] {
        assert!(!text.contains("herdr-projects") && !text.contains("/api/"), "{name} knows nothing of the planner");
    }
    assert!(GATE_JS.contains("history.replaceState") && GATE_JS.contains("/g/redeem"));
    // the CSP of the planner allows no inline script: the page has one, from the same origin
    assert!(!GATE_HTML.contains("onclick") && GATE_HTML.matches("<script").count() == 1 && GATE_HTML.contains("<script src=\"/g/gate.js\">"));
}
