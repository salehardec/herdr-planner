//! The tunnel against a stand-in `cloudflared` (`scripts/fake-cloudflared.mjs`): a real process with a real metrics
//! port, started and stopped by the real code. Each test has its own folder, its own program and its own planner.

use super::*;
use crate::testutil::{test_cfg, write_exec};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/fake-cloudflared.mjs");

/// Short times: a test waits for a state, not for the clock.
fn fast() -> Timing {
    Timing {
        start: Duration::from_secs(8),
        poll_start: Duration::from_millis(40),
        poll_open: Duration::from_millis(60),
        lost: Duration::from_millis(900),
        term_grace: Duration::from_millis(500),
        shutdown_grace: Duration::from_millis(400),
        retry_pause: Duration::from_millis(100),
    }
}

struct Rig {
    dir: tempfile::TempDir,
    app: Shared,
}

impl Rig {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The pid the stand-in wrote, once it has started.
    async fn fake_pid(&self) -> u32 {
        for _ in 0..250 {
            if let Some(pid) = std::fs::read_to_string(self.path("pid")).ok().and_then(|t| t.trim().parse().ok()) {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the stand-in did not start");
    }

    fn fake_args(&self) -> Vec<String> {
        serde_json::from_str(&std::fs::read_to_string(self.path("args")).expect("the stand-in wrote its arguments")).unwrap()
    }

    async fn until(&self, what: &str, want: impl Fn(Phase) -> bool) -> Phase {
        for _ in 0..500 {
            let s = self.app.tunnel.state();
            if want(s) {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("waiting for {what}: the state is {:?}, the log {:?}", self.app.tunnel.state(), self.app.tunnel.inner.lock().unwrap().log);
    }

    fn problem(&self) -> Problem {
        self.app.tunnel.inner.lock().unwrap().problem.clone().expect("a problem")
    }

    /// The `tunnel:` of this rig's config.
    fn tunnel_id(&self) -> String {
        format!("tunnel-{}", self.dir.path().file_name().unwrap().to_string_lossy().trim_start_matches('.'))
    }

    fn host(&self) -> Option<String> {
        self.app.tunnel.inner.lock().unwrap().host.clone()
    }
}

/// A planner whose `cloudflared` is the stand-in, doing what `scenario` says (see the script).
fn rig_with(scenario: &str, edit: impl FnOnce(&mut AppConfig)) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let wrapper = dir.path().join("cloudflared");
    write_exec(
        &wrapper,
        format!(
            "#!/bin/sh\nFAKE_CF='{scenario}' FAKE_CF_ARGS='{d}/args' FAKE_CF_PIDFILE='{d}/pid' FAKE_CF_STATE='{d}/starts' exec node '{SCRIPT}' \"$@\"\n",
            d = dir.path().display()
        ),
    );
    let mut cfg = AppConfig { tunnel_bin: Some(wrapper.display().to_string()), ..test_cfg(dir.path()) };
    edit(&mut cfg);
    let app = crate::app::App::new(cfg);
    app.tunnel.set_timing(fast());
    // the real ~/.cloudflared is nobody's business here
    app.tunnel.set_default_configs(Vec::new());
    Rig { dir, app }
}

fn rig(scenario: &str) -> Rig {
    rig_with(scenario, |_| {})
}

const NAMED: &str = "tunnel: 6ff42ae2-765d-4adf-8112-31c55c1551ef\ncredentials-file: /nonexistent/6ff42ae2.json\n\ningress:\n  - hostname: planner.yanao.su   # the planner\n    service: http://127.0.0.1:7117\n  - service: http_status:404\n";

/// A planner for a named tunnel whose config is `config`.
fn named_rig(scenario: &str, config: Option<&str>) -> Rig {
    let r = rig(scenario);
    if let Some(text) = config {
        // a tunnel of its own: tests run side by side, and a hand-started `cloudflared` of another is none of ours
        std::fs::write(r.path("planner.yml"), text.replace("6ff42ae2-765d-4adf-8112-31c55c1551ef", &r.tunnel_id())).unwrap();
    }
    *r.app.tunnel.settings.lock().unwrap() = Settings { mode: TunnelMode::Named, config: Some(r.path("planner.yml")), hostname: None };
    r
}

/// A stand-in started by hand, the way an owner would: killed when the test is over.
struct Manual(std::process::Child);

impl Manual {
    fn start(r: &Rig, args: &[&str]) -> Manual {
        Manual(std::process::Command::new(r.path("cloudflared")).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap())
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Manual {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn wait_gone(pid: u32) {
    for _ in 0..250 {
        if !proc_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("process {pid} is still there");
}

/* ───────── the config of a named tunnel, and the lines of the program ───────── */

#[test]
fn the_ingress_host_names_are_read_and_nothing_else() {
    assert_eq!(ingress_hosts(NAMED), ["planner.yanao.su"]);
    let flat = "ingress:\n- hostname: \"A.example.org\"\n  service: http://x\n- hostname: '*.zone.example.org'\n  service: http://y\n- service: http_status:404\nmetrics: 127.0.0.1:1\n";
    assert_eq!(ingress_hosts(flat), ["a.example.org", "*.zone.example.org"]);
    // a `hostname` outside `ingress`, a commented one and a bad one are not host names of the tunnel
    let other = "warp-routing:\n  hostname: not-here.example.org\n# ingress:\n#   - hostname: no.example.org\ningress:\n  - hostname: bad host!\n  - service: http_status:404\n";
    assert!(ingress_hosts(other).is_empty(), "{:?}", ingress_hosts(other));
    assert!(ingress_hosts("").is_empty());
}

#[test]
fn a_wildcard_rule_covers_the_hosts_under_it_only() {
    assert!(covers("planner.example.org", "planner.example.org"));
    assert!(!covers("planner.example.org", "other.example.org"));
    assert!(covers("*.example.org", "planner.example.org"));
    assert!(covers("*.example.org", "a.b.example.org"));
    assert!(!covers("*.example.org", "example.org"));
    assert!(!covers("*.example.org", "badexample.org"));
}

#[test]
fn the_quick_address_is_found_in_the_frame_but_not_in_an_error() {
    let frame = "2026-08-31T09:00:00Z INF |  https://Dolls-Unique-Rid-Geek.trycloudflare.com                          |";
    assert_eq!(quick_host_in_line(frame).as_deref(), Some("dolls-unique-rid-geek.trycloudflare.com"));
    assert_eq!(quick_host_in_line("ERR failed to request quick Tunnel: Post \"https://api.trycloudflare.com/tunnel\": EOF"), None);
    assert_eq!(quick_host_in_line("INF visit https://example.org/"), None);
    assert_eq!(quick_host_in_line("no address here"), None);
}

#[test]
fn a_log_line_that_names_credentials_is_not_kept() {
    for line in ["INF Settings: map[credentials-file:/home/me/.cloudflared/x.json]", "ERR open /home/me/.cloudflared/cert.pem: no such file", "INF using TunnelSecret", "WRN Invalid token"] {
        assert_eq!(clean_line(line).as_deref(), Some("(a line about credentials is not kept)"), "{line}");
    }
    assert_eq!(clean_line("INF Registered tunnel connection connIndex=0").as_deref(), Some("INF Registered tunnel connection connIndex=0"));
    assert_eq!(clean_line("  \u{1b}[31m  \n"), Some("[31m".to_string()), "control characters are dropped");
    assert_eq!(clean_line("   "), None);
    assert_eq!(clean_line(&"x".repeat(1000)).unwrap().chars().count(), 300);
}

/* ───────── a quick tunnel ───────── */

#[tokio::test]
async fn a_quick_tunnel_opens_shows_its_address_and_closes() {
    let r = rig("");
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(!r.app.gate.tunnel_open());
    open(&r.app).await;
    assert_eq!(r.app.tunnel.state(), Phase::Starting);
    assert!(!r.app.gate.tunnel_open(), "the gate stays shut until the tunnel is ready");
    // a second «open» while it starts starts nothing
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    open(&r.app).await;

    assert_eq!(r.host().as_deref(), Some("fake-quick-tunnel.trycloudflare.com"));
    assert!(r.app.gate.tunnel_open());
    let v = view(&r.app).await;
    assert_eq!(v.state, Phase::Open);
    // the link carries the key after a `#`: the one thing a server never sees
    let link = v.link.clone().unwrap();
    let key = link.strip_prefix("https://fake-quick-tunnel.trycloudflare.com/#k=").unwrap_or_else(|| panic!("{link}"));
    assert_eq!((key.len(), v.link_state), (22, crate::devices::LinkState::Fresh));
    assert!(key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{key}");
    assert!(v.installed && !v.sse && v.error.is_none());
    assert_eq!(v.version.as_deref(), Some("cloudflared version 2026.8.3 (fake, for the planner's tests)"));

    // the command: an empty config, no autoupdate, the metrics on a port of ours, the planner as the target
    let args = r.fake_args();
    assert_eq!(args[0], "--config");
    assert!(args[1].ends_with("quick-empty.yml"), "{args:?}");
    assert_eq!(&args[2..4], ["tunnel", "--no-autoupdate"]);
    assert_eq!(args[4], "--metrics");
    assert!(args[5].starts_with("127.0.0.1:") && args[5] != "127.0.0.1:0", "{args:?}");
    assert_eq!(&args[6..], ["--url", "http://127.0.0.1:0"]);
    let pid = r.fake_pid().await;
    assert_eq!(std::fs::read_to_string(pid_file(&r.app.cfg)).unwrap().split_whitespace().next(), Some(pid.to_string().as_str()));

    close(&r.app, Duration::from_secs(2)).await;
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(!r.app.gate.tunnel_open());
    assert!(!proc_alive(pid), "cloudflared is gone");
    assert!(!pid_file(&r.app.cfg).exists());
    assert!(view(&r.app).await.link.is_none());
    // and again: nothing to close
    close(&r.app, Duration::from_secs(1)).await;
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
}

#[tokio::test]
async fn the_gate_is_closed_before_the_process_is_told_to_leave() {
    // a stand-in that does not leave on SIGTERM: for the grace the gate is already shut
    let r = rig("ignoreterm=1");
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    let pid = r.fake_pid().await;
    let app = r.app.clone();
    let closing = tokio::spawn(async move { close(&app, Duration::from_millis(1500)).await });
    r.until("closing", |s| s == Phase::Closing).await;
    assert!(!r.app.gate.tunnel_open());
    assert!(proc_alive(pid), "still within the grace");
    closing.await.unwrap();
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(!proc_alive(pid), "SIGKILL after the grace");
}

#[tokio::test]
async fn a_quick_address_missing_from_the_metrics_is_read_from_the_log() {
    let r = rig("noapi=1");
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    assert_eq!(r.host().as_deref(), Some("fake-quick-tunnel.trycloudflare.com"));
    close(&r.app, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn a_tunnel_that_does_not_connect_fails_by_the_clock() {
    let r = rig("never=1");
    r.app.tunnel.set_timing(Timing { start: Duration::from_millis(1200), ..fast() });
    open(&r.app).await;
    r.until("failed", |s| s == Phase::Failed).await;
    let p = r.problem();
    assert_eq!(p.kind, "timeout");
    assert!(!p.log_tail.is_empty());
    assert!(!r.app.gate.tunnel_open());
    wait_gone(r.fake_pid().await).await;
    assert!(!pid_file(&r.app.cfg).exists());
    // «Повторить»: the same call opens again
    r.app.tunnel.set_timing(fast());
    open(&r.app).await;
    assert_eq!(r.app.tunnel.state(), Phase::Starting);
    assert!(view(&r.app).await.error.is_none(), "the old error is gone");
    close(&r.app, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn a_process_that_leaves_fails_the_tunnel_and_is_not_restarted() {
    let r = rig("die=700");
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    assert!(r.app.gate.tunnel_open());
    r.until("failed", |s| s == Phase::Failed).await;
    let p = r.problem();
    assert_eq!(p.kind, "exited");
    assert!(p.message.contains("code 1"), "{p:?}");
    assert!(p.log_tail.iter().any(|l| l.contains("Unable to establish connection")), "{p:?}");
    assert!(!p.log_tail.iter().any(|l| l.contains("was empty")), "that line is no error: {p:?}");
    assert!(!r.app.gate.tunnel_open());
    assert!(r.host().is_none());
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(r.app.tunnel.state(), Phase::Failed, "no restart: a new start is a new address");
    // closing a failed tunnel clears it
    close(&r.app, Duration::from_secs(1)).await;
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(view(&r.app).await.error.is_none());
}

#[tokio::test]
async fn cloudflares_refusal_of_a_quick_tunnel_is_tried_once_more() {
    let r = rig("fail=1");
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    assert_eq!(std::fs::read_to_string(r.path("starts")).unwrap(), "2");
    close(&r.app, Duration::from_secs(2)).await;

    let r = rig("fail=9");
    open(&r.app).await;
    r.until("failed", |s| s == Phase::Failed).await;
    assert_eq!(std::fs::read_to_string(r.path("starts")).unwrap(), "2", "once more, not forever");
    let p = r.problem();
    assert_eq!(p.kind, "exited");
    assert!(p.log_tail.iter().any(|l| l.contains("failed to request quick Tunnel")), "{p:?}");
}

#[tokio::test]
async fn a_lost_connection_is_reconnecting_and_comes_back_to_open() {
    let r = rig("ready=100,blip=100:500");
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    let link = view(&r.app).await.link.unwrap();
    r.until("reconnecting", |s| s == Phase::Reconnecting).await;
    assert!(r.app.gate.tunnel_open(), "the address is the same, the gate stays");
    assert_eq!(view(&r.app).await.link.as_deref(), Some(link.as_str()), "so does the link, key and all");
    r.until("open again", |s| s == Phase::Open).await;
    assert_eq!(view(&r.app).await.link.as_deref(), Some(link.as_str()));
    close(&r.app, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn a_connection_that_does_not_come_back_closes_the_tunnel() {
    let r = rig("ready=100,lost=100");
    open(&r.app).await;
    r.until("reconnecting", |s| s == Phase::Reconnecting).await;
    r.until("failed", |s| s == Phase::Failed).await;
    assert_eq!(r.problem().kind, "lost");
    assert!(!r.app.gate.tunnel_open());
    wait_gone(r.fake_pid().await).await;
}

#[tokio::test]
async fn a_tunnel_can_be_closed_while_it_starts() {
    let r = rig("ready=3000");
    open(&r.app).await;
    let pid = r.fake_pid().await;
    close(&r.app, Duration::from_secs(2)).await;
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    wait_gone(pid).await;
}

#[tokio::test]
async fn without_cloudflared_the_tunnel_says_so() {
    for bin in ["/nonexistent/dir/cloudflared", "no-such-cloudflared-qq"] {
        let r = rig_with("", |c| c.tunnel_bin = Some(bin.to_string()));
        let v = view(&r.app).await;
        assert!(!v.installed && v.bin.is_none() && v.version.is_none());
        open(&r.app).await;
        assert_eq!(r.app.tunnel.state(), Phase::Failed, "{bin}");
        assert_eq!(r.problem().kind, "not-installed");
        assert!(!r.app.gate.tunnel_open());
        // the check again after installing it: the same call
        close(&r.app, Duration::from_secs(1)).await;
        assert_eq!(r.app.tunnel.state(), Phase::Closed);
    }
}

#[tokio::test]
async fn a_program_that_will_not_start_is_a_spawn_error() {
    let r = rig_with("", |c| {
        let bad = c.state_dir.parent().unwrap().join("not-a-program");
        std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
        write_exec(&bad, "not a script, no interpreter line\n");
        c.tunnel_bin = Some(bad.display().to_string());
    });
    open(&r.app).await;
    r.until("failed", |s| s == Phase::Failed).await;
    assert_eq!(r.problem().kind, "spawn");
}

/* ───────── a named tunnel ───────── */

#[tokio::test]
async fn a_named_tunnel_runs_the_owners_config_at_the_owners_address() {
    let r = named_rig("", Some(NAMED));
    let v = view(&r.app).await;
    assert_eq!(v.mode, "named");
    let check = v.config_check.expect("a named tunnel has its config checked");
    assert!(check.ok && check.host.as_deref() == Some("planner.yanao.su") && check.hosts == ["planner.yanao.su"]);

    open(&r.app).await;
    assert_eq!(r.host().as_deref(), Some("planner.yanao.su"), "the address is known at once");
    assert!(!r.app.gate.tunnel_open());
    r.until("open", |s| s == Phase::Open).await;
    assert!(r.app.gate.tunnel_open());
    let v = view(&r.app).await;
    assert!(v.link.as_deref().unwrap().starts_with("https://planner.yanao.su/#k="), "{:?}", v.link);
    assert!(v.sse, "server-sent events work through a named tunnel");

    let args = r.fake_args();
    let config = r.path("planner.yml").display().to_string();
    assert_eq!(args[..2], ["--config".to_string(), config]);
    assert_eq!(args.last().map(String::as_str), Some("run"));
    assert!(!args.iter().any(|a| a == "--url"), "the owner's config says where the tunnel leads: {args:?}");
    assert!(args.contains(&"--metrics".to_string()) && args.contains(&"--no-autoupdate".to_string()));

    let pid = r.fake_pid().await;
    close(&r.app, Duration::from_secs(2)).await;
    wait_gone(pid).await;
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(r.path("planner.yml").exists(), "the owner's file is theirs: it stays");
}

#[tokio::test]
async fn a_named_tunnel_with_no_config_file_does_not_start() {
    let r = named_rig("", None);
    let v = view(&r.app).await;
    assert!(!v.config_check.as_ref().unwrap().ok);
    open(&r.app).await;
    assert_eq!(r.app.tunnel.state(), Phase::Failed);
    let p = r.problem();
    assert_eq!(p.kind, "config-missing");
    assert!(p.message.contains("not found") && p.message.contains("planner.yml"), "{p:?}");
    assert!(!r.path("args").exists(), "nothing was started");

    // no path at all
    *r.app.tunnel.settings.lock().unwrap() = Settings { mode: TunnelMode::Named, config: None, hostname: None };
    close(&r.app, Duration::from_secs(1)).await;
    open(&r.app).await;
    assert_eq!(r.problem().kind, "config-missing");
}

#[tokio::test]
async fn a_named_tunnel_whose_config_has_no_hostname_does_not_start() {
    for text in ["tunnel: x\ncredentials-file: /x.json\ningress:\n  - service: http_status:404\n", "tunnel: x\n", ""] {
        let r = named_rig("", Some(text));
        open(&r.app).await;
        assert_eq!(r.app.tunnel.state(), Phase::Failed, "{text:?}");
        let p = r.problem();
        assert_eq!(p.kind, "no-hostname");
        assert!(p.message.contains("no `hostname`"), "{p:?}");
        assert!(!r.path("args").exists());
    }
}

#[tokio::test]
async fn the_address_in_the_settings_must_be_one_of_the_configs() {
    let r = named_rig("", Some(NAMED));
    r.app.tunnel.settings.lock().unwrap().hostname = Some("elsewhere.example.org".into());
    open(&r.app).await;
    let p = r.problem();
    assert_eq!(p.kind, "hostname-mismatch");
    assert!(p.message.contains("elsewhere.example.org") && p.message.contains("planner.yanao.su"), "{p:?}");
    assert!(!r.path("args").exists());

    // a wildcard rule covers the hosts under it; the setting then names the address
    std::fs::write(r.path("planner.yml"), "ingress:\n  - hostname: \"*.yanao.su\"\n    service: http://127.0.0.1:7117\n  - service: http_status:404\n").unwrap();
    close(&r.app, Duration::from_secs(1)).await;
    r.app.tunnel.settings.lock().unwrap().hostname = Some("planner.yanao.su".into());
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    assert_eq!(r.host().as_deref(), Some("planner.yanao.su"));
    close(&r.app, Duration::from_secs(2)).await;
    // …but with only a wildcard and no address in the settings there is nothing to show
    r.app.tunnel.settings.lock().unwrap().hostname = None;
    open(&r.app).await;
    assert_eq!(r.problem().kind, "no-hostname");
}

#[tokio::test]
async fn a_tunnel_the_owner_started_is_shown_not_started_twice() {
    let r = named_rig("", Some(NAMED));
    let config = r.path("planner.yml").display().to_string();
    let mut manual = Manual::start(&r, &["--config", &config, "tunnel", "run"]);
    let mut seen = false;
    for _ in 0..500 {
        seen = find_external(&r.app, &r.path("planner.yml"), None).is_some();
        if seen {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(seen, "the hand-started tunnel is found by its config");
    // (the wrapper script becomes `node` by exec; the process table is read again once that is over)
    tokio::time::sleep(Duration::from_millis(400)).await;
    open(&r.app).await;
    assert_eq!(r.app.tunnel.state(), Phase::External);
    let v = view(&r.app).await;
    assert_eq!(v.external_pid, Some(manual.pid()));
    assert_eq!(v.host.as_deref(), Some("planner.yanao.su"), "the address it serves");
    assert!(v.link.is_none(), "not ours, and not open for the gate");
    assert!(!r.app.gate.tunnel_open());
    assert!(!r.app.tunnel.inner.lock().unwrap().active, "no process of ours");
    // «close» forgets it and leaves the process alone
    close(&r.app, Duration::from_secs(1)).await;
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(proc_alive(manual.pid()));
    // it is noticed again, and noticed gone
    open(&r.app).await;
    assert_eq!(r.app.tunnel.state(), Phase::External);
    manual.stop();
    assert_eq!(view(&r.app).await.state, Phase::Closed);
    // and now there is room for ours
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    close(&r.app, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn a_tunnel_with_another_config_is_not_ours_to_notice() {
    let r = named_rig("", Some(NAMED));
    let other = r.path("other.yml");
    std::fs::write(&other, NAMED).unwrap();
    let _manual = Manual::start(&r, &["--config", &other.display().to_string(), "tunnel", "run"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(find_external(&r.app, &r.path("planner.yml"), None), None);
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    close(&r.app, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn the_owners_credentials_are_not_in_the_log_nor_in_what_the_page_gets() {
    let r = named_rig("leak=1,ready=100,die=500", Some(NAMED));
    open(&r.app).await;
    r.until("failed", |s| s == Phase::Failed).await;
    let text = {
        let g = r.app.tunnel.inner.lock().unwrap();
        g.log.iter().cloned().collect::<Vec<_>>().join("\n")
    };
    assert!(text.contains("Starting tunnel"), "the rest of the log is kept: {text}");
    assert!(!text.contains("secret.json") && !text.to_lowercase().contains("credentials-file"), "{text}");
    let json = serde_json::to_string(&view(&r.app).await).unwrap();
    // the checks name the file's UUID (the tunnel's id, public in its DNS record) but never the path of the secret
    assert!(!json.contains("secret.json") && !json.contains("/nonexistent"), "{json}");
}

/* ───────── what a crash leaves, and the planner's own stop ───────── */

#[tokio::test]
async fn a_cloudflared_left_by_a_crash_is_stopped_at_the_next_start() {
    let r = rig("");
    let manual = Manual::start(&r, &["--config", "x.yml", "tunnel", "run"]);
    let pid = manual.pid();
    for _ in 0..250 {
        if std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains("fake-cloudflared.mjs")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    write_pid_file(&r.app.cfg, pid);
    assert_eq!(reap_orphan(&r.app.cfg).await, Some(pid));
    for _ in 0..100 {
        if !proc_alive(pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!proc_alive(pid));
    assert!(!pid_file(&r.app.cfg).exists());
    drop(manual);
    assert_eq!(reap_orphan(&r.app.cfg).await, None, "no file, nothing to do");
}

#[tokio::test]
async fn a_pid_that_belongs_to_something_else_now_is_left_alone() {
    let r = rig("");
    let mut other = Manual(std::process::Command::new("sleep").arg("30").spawn().unwrap());
    // the recorded start time is not this process's: the pid was reused
    std::fs::create_dir_all(&r.app.cfg.state_dir).unwrap();
    std::fs::write(pid_file(&r.app.cfg), format!("{} {}x\n", other.pid(), proc_start_with(backend(), other.pid()).unwrap())).unwrap();
    assert_eq!(reap_orphan(&r.app.cfg).await, None);
    assert!(proc_alive(other.pid()));
    assert!(!pid_file(&r.app.cfg).exists(), "the stale record goes");
    // the right start time, but not a cloudflared
    write_pid_file(&r.app.cfg, other.pid());
    assert_eq!(reap_orphan(&r.app.cfg).await, None);
    assert!(proc_alive(other.pid()));
    other.stop();
    // a record that is not one
    std::fs::write(pid_file(&r.app.cfg), "garbage").unwrap();
    assert_eq!(reap_orphan(&r.app.cfg).await, None);
}

#[tokio::test]
async fn the_planners_own_stop_takes_the_tunnel_down_in_time() {
    let r = rig("ignoreterm=1");
    shutdown(&r.app).await; // nothing open: nothing to do
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    let pid = r.fake_pid().await;
    let t = Instant::now();
    shutdown(&r.app).await;
    assert!(t.elapsed() < Duration::from_millis(1900), "the planner's exit is two seconds after the signal: {:?}", t.elapsed());
    assert!(!proc_alive(pid), "SIGKILL after the short grace");
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(!r.app.gate.tunnel_open());
}

/* ───────── the page's API ───────── */

async fn serve(app: Shared) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = crate::server::router(app);
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.ok();
    });
    addr
}

/// One request as a client writes it: the status and the body.
async fn call(addr: SocketAddr, method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> (u16, String) {
    let (status, _, body) = call_head(addr, method, path, extra, body).await;
    (status, body)
}

/// The same, with the head of the answer (its headers) kept: the status, the head, the body.
async fn call_head(addr: SocketAddr, method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> (u16, String, String) {
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!("\r\n{body}"));
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut raw)).await.unwrap().ok();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
    (status, head, body)
}

fn json_of(body: &str) -> serde_json::Value {
    // axum answers JSON with a Content-Length; a chunked body would carry its size lines
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

#[tokio::test]
async fn the_api_opens_and_closes_the_tunnel_for_this_machine() {
    let r = named_rig("", Some(NAMED));
    let addr = serve(r.app.clone()).await;
    let local = [("Host", "localhost")];

    let (status, body) = call(addr, "GET", "/api/tunnel", &local, "").await;
    assert_eq!(status, 200, "{body}");
    let v = json_of(&body);
    assert_eq!((v["state"].as_str(), v["mode"].as_str(), v["installed"].as_bool()), (Some("closed"), Some("named"), Some(true)));
    assert_eq!(v["config_check"]["ok"], true);

    // a page of another site cannot press the button
    let (status, _) = call(addr, "POST", "/api/tunnel/open", &[("Host", "localhost"), ("Origin", "http://evil.example")], "").await;
    assert_eq!(status, 403);
    assert_eq!(r.app.tunnel.state(), Phase::Closed);

    let (status, body) = call(addr, "POST", "/api/tunnel/open", &local, "").await;
    assert_eq!(status, 200, "{body}");
    r.until("open", |s| s == Phase::Open).await;
    let v = json_of(&call(addr, "GET", "/api/tunnel", &local, "").await.1);
    assert_eq!((v["state"].as_str(), v["host"].as_str()), (Some("open"), Some("planner.yanao.su")));
    assert!(v["link"].as_str().unwrap().starts_with("https://planner.yanao.su/#k="), "{v}");

    // the gate lets the tunnel's address in as a tunnel request (a refusal for lack of a session, not the 503 of a closed tunnel)
    let https = ("Cf-Visitor", "{\"scheme\":\"https\"}");
    let (status, _) = call(addr, "GET", "/api/state", &[("Host", "planner.yanao.su"), ("Cf-Ray", "x"), https], "").await;
    assert_eq!(status, 401);
    // …and what it may not see is hidden from it, the tunnel's own controls above all
    for (m, p) in [("GET", "/api/tunnel"), ("POST", "/api/tunnel/close"), ("POST", "/api/tunnel/open"), ("POST", "/api/tunnel/config")] {
        let (status, _) = call(addr, m, p, &[("Host", "planner.yanao.su"), ("Cf-Ray", "x"), https], "{}").await;
        assert_eq!(status, 404, "{m} {p}");
        let (status, _) = call(addr, m, p, &[("Host", "localhost"), ("Cf-Connecting-Ip", "203.0.113.5"), https], "{}").await;
        assert_eq!(status, 404, "{m} {p} with a forged Host");
    }
    // a reverse proxy in front: not this machine, whatever the peer address
    let (status, _) = call(addr, "POST", "/api/tunnel/close", &[("Host", "localhost"), ("X-Forwarded-For", "203.0.113.5")], "").await;
    assert_eq!(status, 403);
    let (status, _) = call(addr, "GET", "/api/tunnel", &[("Host", "localhost"), ("X-Forwarded-For", "203.0.113.5")], "").await;
    assert_eq!(status, 403);
    assert_eq!(r.app.tunnel.state(), Phase::Open, "none of those closed it");

    let pid = r.fake_pid().await;
    let (status, body) = call(addr, "POST", "/api/tunnel/close", &local, "").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(json_of(&body)["state"], "closed");
    wait_gone(pid).await;
    // closed: a request through the tunnel's address gets the 503 again
    let (status, _) = call(addr, "GET", "/api/state", &[("Host", "planner.yanao.su"), ("Cf-Ray", "x"), https], "").await;
    assert_eq!(status, 503);
}

#[tokio::test]
async fn the_api_changes_the_mode_the_config_and_the_address() {
    let r = rig("");
    let addr = serve(r.app.clone()).await;
    let local = [("Host", "localhost")];
    let config = r.path("planner.yml");
    std::fs::write(&config, NAMED).unwrap();
    let body = format!("{{\"mode\":\"named\",\"config\":\"{}\",\"hostname\":\"Planner.Yanao.Su\"}}", config.display());
    let (status, text) = call(addr, "POST", "/api/tunnel/config", &local, &body).await;
    assert_eq!(status, 200, "{text}");
    let v = json_of(&text);
    assert_eq!((v["mode"].as_str(), v["hostname"].as_str(), v["config"].as_str()), (Some("named"), Some("planner.yanao.su"), Some(config.display().to_string().as_str())));
    assert_eq!(v["config_check"]["ok"], true);
    let saved = std::fs::read_to_string(&r.app.cfg.config_file).unwrap();
    assert!(saved.contains("mode = \"named\"") && saved.contains("hostname = \"planner.yanao.su\""), "{saved}");
    // the gate knows the address as a tunnel's at once
    let (status, _) = call(addr, "GET", "/api/state", &[("Host", "planner.yanao.su")], "").await;
    assert_eq!(status, 503, "a request for the tunnel's address is a tunnel request");

    // mistakes
    for bad in ["{\"mode\":\"fast\"}", "{\"config\":\"relative/x.yml\"}", "{\"hostname\":\"not a host\"}", "{\"hostname\":\"*.example.org\"}", "{\"hostname\":\"a.example.org:8443\"}"] {
        let (status, text) = call(addr, "POST", "/api/tunnel/config", &local, bad).await;
        assert_eq!(status, 400, "{bad}: {text}");
    }
    let (status, _) = call(addr, "POST", "/api/tunnel/config", &[("Host", "localhost"), ("Origin", "http://evil.example")], "{\"mode\":\"quick\"}").await;
    assert_eq!(status, 403);
    assert_eq!(r.app.tunnel.settings().mode, TunnelMode::Named, "nothing of that was applied");

    // removing the address and the config, back to quick
    let (status, text) = call(addr, "POST", "/api/tunnel/config", &local, "{\"mode\":\"quick\",\"config\":\"\",\"hostname\":\"\"}").await;
    assert_eq!(status, 200, "{text}");
    let v = json_of(&text);
    assert!(v["config"].is_null() && v["hostname"].is_null() && v["mode"] == "quick" && v["config_check"].is_null(), "{v}");

    // not while a tunnel is up
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    let (status, _) = call(addr, "POST", "/api/tunnel/config", &local, "{\"mode\":\"named\"}").await;
    assert_eq!(status, 409);
    close(&r.app, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn a_changed_setting_clears_an_old_failure() {
    let r = named_rig("", None);
    let addr = serve(r.app.clone()).await;
    open(&r.app).await;
    assert_eq!(r.app.tunnel.state(), Phase::Failed);
    let (status, text) = call(addr, "POST", "/api/tunnel/config", &[("Host", "localhost")], &format!("{{\"config\":\"{}\"}}", r.path("planner.yml").display())).await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(json_of(&text)["error"].is_null());
}

/* ───────── a tunnel started by hand without --config, and the table on a machine without /proc ───────── */

async fn wait_found(r: &Rig) -> bool {
    for _ in 0..500 {
        if find_external(&r.app, &r.path("planner.yml"), None).is_some() {
            // (the wrapper script becomes `node` by exec; look again once that is over)
            tokio::time::sleep(Duration::from_millis(400)).await;
            return find_external(&r.app, &r.path("planner.yml"), None).is_some();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn a_tunnel_started_by_hand_with_the_default_config_is_found() {
    // `cloudflared tunnel run`: no --config, the config is the one it finds by itself
    let r = named_rig("", Some(NAMED));
    let other = r.path("other.yml");
    std::fs::write(&other, NAMED.replace("6ff42ae2-765d-4adf-8112-31c55c1551ef", "another-tunnel")).unwrap();
    assert_ne!(r.tunnel_id(), "another-tunnel");
    // the default is another file: the hand-started tunnel is not this one
    r.app.tunnel.set_default_configs(vec![r.path("missing.yml"), other, r.path("planner.yml")]);
    let mut manual = Manual::start(&r, &["tunnel", "run"]);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!wait_found(&r).await);

    // the default is the planner's own file
    r.app.tunnel.set_default_configs(vec![r.path("missing.yml"), r.path("planner.yml")]);
    assert!(wait_found(&r).await, "a bare `tunnel run` reads the default config, which is this one");
    open(&r.app).await;
    assert_eq!(r.app.tunnel.state(), Phase::External, "a second cloudflared is not started");
    assert_eq!(view(&r.app).await.external_pid, Some(manual.pid()));
    assert!(!pid_file(&r.app.cfg).exists(), "no process of ours");
    assert!(!r.app.gate.tunnel_open());
    manual.stop();
    assert_eq!(view(&r.app).await.state, Phase::Closed);
}

#[tokio::test]
async fn a_tunnel_started_by_hand_by_name_is_found_whatever_config_it_reads() {
    let r = named_rig("", Some(NAMED));
    // (with a config of its own: a bare `tunnel run` of another test would be taken for the default one)
    let elsewhere = r.path("elsewhere.yml").display().to_string();
    let mut manual = Manual::start(&r, &["--config", &elsewhere, "tunnel", "run", &r.tunnel_id()]);
    assert!(wait_found(&r).await);
    open(&r.app).await;
    assert_eq!(r.app.tunnel.state(), Phase::External);
    manual.stop();
    // another tunnel's name is another tunnel
    let mut manual = Manual::start(&r, &["--config", &elsewhere, "tunnel", "run", "some-other-tunnel"]);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!wait_found(&r).await);
    manual.stop();
}

#[test]
fn which_command_lines_are_this_tunnel() {
    let want = Wanted { config: PathBuf::from("/nonexistent/planner.yml"), tunnel: Some("planner".into()), defaults: Vec::new() };
    let args = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
    assert!(runs_tunnel(&args("/usr/bin/cloudflared --config /nonexistent/planner.yml tunnel run"), &want));
    assert!(runs_tunnel(&args("cloudflared tunnel --config=/nonexistent/planner.yml run"), &want));
    assert!(runs_tunnel(&args("cloudflared tunnel -c /nonexistent/planner.yml run"), &want));
    assert!(runs_tunnel(&args("cloudflared tunnel run planner"), &want));
    assert!(runs_tunnel(&args("cloudflared --config /else.yml tunnel run planner"), &want));
    assert!(!runs_tunnel(&args("cloudflared tunnel run"), &want), "no default config here");
    assert!(!runs_tunnel(&args("cloudflared --config /else.yml tunnel run"), &want));
    assert!(!runs_tunnel(&args("cloudflared tunnel run other"), &want));
    assert!(!runs_tunnel(&args("cloudflared tunnel --config /nonexistent/planner.yml list"), &want), "only `run` is a running tunnel");
    assert!(!runs_tunnel(&args("vim planner.yml run"), &want), "only cloudflared");
    assert_eq!(config_tunnel("tunnel: 6ff4\ncredentials-file: /x.json\n").as_deref(), Some("6ff4"));
    assert_eq!(config_tunnel("tunnel: \"planner\"  # mine\n").as_deref(), Some("planner"));
    assert_eq!(config_tunnel("ingress:\n  - service: x\n"), None);
}

#[tokio::test]
async fn the_process_table_comes_from_ps_where_there_is_no_proc() {
    // macOS has `ps` and no /proc; the same questions are put to `ps` here
    let r = named_rig("", Some(NAMED));
    let config = r.path("planner.yml").display().to_string();
    let mut manual = Manual::start(&r, &["--config", &config, "tunnel", "run"]);
    let pid = manual.pid();
    let mut found = None;
    for _ in 0..500 {
        let want = wanted(&r.app, &r.path("planner.yml"));
        found = external_in(&processes(Backend::Ps), &want, None, Backend::Ps);
        if found.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(found, Some(pid), "the hand-started tunnel is found through ps");
    assert!(proc_alive_with(Backend::Ps, pid));
    assert!(proc_cmdline_with(Backend::Ps, pid).contains("cloudflared"));
    let start = proc_start_with(Backend::Ps, pid).expect("ps knows when it started");
    assert_eq!(proc_start_with(Backend::Ps, pid).as_deref(), Some(start.as_str()));

    // a crash's orphan: stopped by pid and start time, and only that one
    write_pid_file_with(Backend::Ps, &r.app.cfg, pid);
    assert!(std::fs::read_to_string(pid_file(&r.app.cfg)).unwrap().contains(&start));
    assert_eq!(reap_orphan_with(Backend::Ps, &r.app.cfg).await, Some(pid));
    manual.0.wait().unwrap();
    assert!(!proc_alive_with(Backend::Ps, pid));
    assert!(!pid_file(&r.app.cfg).exists());

    // not a cloudflared, or not the recorded start: left alone
    let mut other = Manual(std::process::Command::new("sleep").arg("30").spawn().unwrap());
    write_pid_file_with(Backend::Ps, &r.app.cfg, other.pid());
    assert_eq!(reap_orphan_with(Backend::Ps, &r.app.cfg).await, None);
    std::fs::write(pid_file(&r.app.cfg), format!("{} Thu Jan  1 00:00:00 1970\n", other.pid())).unwrap();
    assert_eq!(reap_orphan_with(Backend::Ps, &r.app.cfg).await, None);
    assert!(proc_alive_with(Backend::Ps, other.pid()));
    // a record with no start time stops nothing
    std::fs::write(pid_file(&r.app.cfg), format!("{} \n", other.pid())).unwrap();
    assert_eq!(reap_orphan_with(Backend::Ps, &r.app.cfg).await, None);
    other.stop();
}

/* ───────── the key, the devices, the sessions ───────── */

const QUICK: &str = "fake-quick-tunnel.trycloudflare.com";
const PHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1";
const LOCAL: [(&str, &str); 1] = [("Host", "localhost")];
const HOUR: u64 = 3_600_000;

/// A request that comes through the tunnel, from a phone in Russia, with the cookie of its session if it has one.
async fn via(addr: SocketAddr, method: &str, path: &str, cookie: Option<&str>, body: &str) -> (u16, String, String) {
    let cookie_line = cookie.map(|c| format!("{}={c}", crate::devices::COOKIE));
    let mut h = vec![("Host", QUICK), ("Cf-Ray", "x"), ("Cf-Visitor", "{\"scheme\":\"https\"}"), ("Cf-Connecting-Ip", "203.0.113.7"), ("Cf-Ipcountry", "RU"), ("User-Agent", PHONE)];
    if let Some(c) = cookie_line.as_deref() {
        h.push(("Cookie", c));
    }
    call_head(addr, method, path, &h, body).await
}

/// The value of the session cookie a response sets.
fn cookie_of(head: &str) -> String {
    let line = head.lines().find(|l| l.to_lowercase().starts_with("set-cookie:")).unwrap_or_else(|| panic!("no cookie: {head}"));
    let (_, rest) = line.split_once(':').unwrap();
    let pair = rest.trim().split(';').next().unwrap();
    pair.strip_prefix(&format!("{}=", crate::devices::COOKIE)).unwrap_or_else(|| panic!("{line}")).to_string()
}

fn key_in(v: &serde_json::Value) -> Option<String> {
    v["link"].as_str().and_then(|l| l.split("#k=").nth(1)).map(str::to_string)
}

async fn plaque(addr: SocketAddr) -> serde_json::Value {
    let (status, body) = call(addr, "GET", "/api/tunnel", &LOCAL, "").await;
    assert_eq!(status, 200, "{body}");
    json_of(&body)
}

async fn opened_rig(scenario: &str) -> (Rig, SocketAddr) {
    let r = rig(scenario);
    let addr = serve(r.app.clone()).await;
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    (r, addr)
}

#[tokio::test]
async fn a_phone_comes_in_with_the_key_and_the_owner_lets_it_in() {
    let (r, addr) = opened_rig("").await;
    let v = plaque(addr).await;
    let key = key_in(&v).expect("the link has the key");
    assert_eq!((v["link_state"].as_str(), v["sessions"].as_array().map(Vec::len)), (Some("fresh"), Some(0)));
    assert_eq!((v["settings"]["confirm_devices"].as_bool(), v["settings"]["idle_close_h"].as_u64(), v["settings"]["full_allowed"].as_bool()), (Some(true), Some(4), Some(false)));
    assert_eq!(v["password_set"], false);

    // the address alone is the sign-in page, and the key is in none of what it sends
    let (status, head, page) = via(addr, "GET", "/", None, "").await;
    assert_eq!(status, 200);
    assert!(page.contains("/g/gate.js") && !page.contains(&key) && !head.contains(&key), "{head}");
    let (status, _, js) = via(addr, "GET", "/g/gate.js", None, "").await;
    assert_eq!(status, 200);
    assert!(js.contains("/g/redeem") && !js.contains(&key));

    // a wrong key gets nothing; the right one a cookie, and the question goes to the owner
    let (status, head, _) = via(addr, "POST", "/g/redeem", None, "{\"key\":\"AAAAAAAAAAAAAAAAAAAAAA\"}").await;
    assert_eq!(status, 410);
    assert!(!head.to_lowercase().contains("set-cookie"), "{head}");
    let (status, head, body) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"{key}\",\"touch\":true}}")).await;
    assert_eq!(status, 200, "{body}");
    let j = json_of(&body);
    assert_eq!((j["state"].as_str(), j["label"].as_str(), j["next"].as_str()), (Some("pending"), Some("iPhone · Safari 17"), Some("/m")));
    let cookie_line = head.lines().find(|l| l.to_lowercase().starts_with("set-cookie:")).unwrap().to_lowercase();
    for part in ["__host-hp_g=", "path=/", "httponly", "secure", "samesite=strict", "max-age=86400"] {
        assert!(cookie_line.contains(part), "{part}: {cookie_line}");
    }
    assert!(!cookie_line.contains("domain"), "{cookie_line}");
    let token = cookie_of(&head);
    assert_eq!(token.len(), 64);
    // the key is spent: the same link again, from another phone or the same, is dead
    let (status, _, _) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"{key}\"}}")).await;
    assert_eq!(status, 410);

    // waiting is not being in
    for p in ["/favicon.ico", "/api/state"] {
        let (status, _, _) = via(addr, "GET", p, Some(&token), "").await;
        assert_eq!(status, 401, "{p}");
    }
    let (_, _, body) = via(addr, "GET", "/g/status", Some(&token), "").await;
    assert_eq!(json_of(&body)["state"], "pending");
    let (_, _, body) = via(addr, "GET", "/g/status", None, "").await;
    let j = json_of(&body);
    assert_eq!(j["state"], "none");
    assert!(j["computer"].is_null() && j["label"].is_null(), "a stranger is not told the name of this machine: {j}");

    // the owner sees who, from where, and that the link is spent
    let v = plaque(addr).await;
    assert_eq!(v["link_state"], "used");
    assert!(key_in(&v).is_none() && v["link"].as_str().unwrap().ends_with(QUICK), "{v}");
    let s = &v["sessions"][0];
    assert_eq!((s["state"].as_str(), s["label"].as_str(), s["country"].as_str(), s["scope"].as_str()), (Some("pending"), Some("iPhone · Safari 17"), Some("RU"), Some("lite")));
    let id = s["id"].as_str().unwrap().to_string();

    // «Let in»
    let (status, body) = call(addr, "POST", "/api/tunnel/session", &LOCAL, &format!("{{\"id\":\"{id}\",\"op\":\"allow\"}}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(json_of(&body)["sessions"][0]["state"], "active");
    let (_, _, body) = via(addr, "GET", "/g/status", Some(&token), "").await;
    assert_eq!(json_of(&body)["state"], "active");
    // in: what the light page uses; not the planner, and not a thing of this machine
    let (status, _, _) = via(addr, "GET", "/favicon.ico", Some(&token), "").await;
    assert_eq!(status, 204);
    let (status, _, _) = via(addr, "GET", "/api/state", Some(&token), "").await;
    assert_eq!(status, 403, "the light version only");
    for p in ["/api/limits", "/api/tunnel", "/api/task/terminal", "/api/memory"] {
        let (status, _, _) = via(addr, "GET", p, Some(&token), "").await;
        assert_eq!(status, 404, "{p}");
    }
    for (m, p) in [("POST", "/api/tunnel/close"), ("POST", "/api/tunnel/rotate"), ("POST", "/api/tunnel/session"), ("POST", "/api/tunnel/settings")] {
        let (status, _, _) = via(addr, m, p, Some(&token), "{}").await;
        assert_eq!(status, 404, "{m} {p}: a device cannot decide about devices");
    }
    // another browser with this cookie's name and any value is nobody
    let (status, _, _) = via(addr, "GET", "/favicon.ico", Some("0".repeat(64).as_str()), "").await;
    assert_eq!(status, 401);

    // a new link: the old key is void, and the owner can let a second device in
    let (status, body) = call(addr, "POST", "/api/tunnel/rotate", &LOCAL, "").await;
    assert_eq!(status, 200, "{body}");
    let v2 = json_of(&body);
    let key2 = key_in(&v2).expect("a new key");
    assert_ne!(key, key2);
    assert_eq!(v2["link_state"], "fresh");
    assert!(v2["link_expires_ms"].as_u64().unwrap() > crate::config::now_ms());

    // switched off: the cookie is dead at once
    let (status, _) = call(addr, "POST", "/api/tunnel/session", &LOCAL, &format!("{{\"id\":\"{id}\",\"op\":\"revoke\"}}")).await;
    assert_eq!(status, 200);
    let (status, _, _) = via(addr, "GET", "/favicon.ico", Some(&token), "").await;
    assert_eq!(status, 401);
    let (_, _, body) = via(addr, "GET", "/g/status", Some(&token), "").await;
    let j = json_of(&body);
    assert_eq!((j["state"].as_str(), j["reason"].as_str()), (Some("gone"), Some("revoked")));
    assert_eq!(plaque(addr).await["sessions"].as_array().unwrap().len(), 0);

    // the journal tells it, without the key or the cookie
    let lines: Vec<String> = r.app.snapshot.read().await.journal.iter().map(|e| e.text.clone()).collect();
    for want in ["устройство просит войти: iPhone · Safari 17", "устройство подключено: iPhone · Safari 17", "устройство отключено (владельцем): iPhone · Safari 17"] {
        assert!(lines.iter().any(|l| l == want), "{want} in {lines:?}");
    }
    assert!(!lines.iter().any(|l| l.contains(&key) || l.contains(&token)), "{lines:?}");

    // closing the gate: the key is void with the rest, the answers are 503
    let (status, _) = call(addr, "POST", "/api/tunnel/close", &LOCAL, "").await;
    assert_eq!(status, 200);
    for (m, p, body) in [("GET", "/g/status", ""), ("POST", "/g/redeem", "{\"key\":\"x\"}"), ("GET", "/favicon.ico", "")] {
        let (status, _, _) = via(addr, m, p, Some(&token), body).await;
        assert_eq!(status, 503, "{p}");
    }
    let (status, _, _) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"{key2}\"}}")).await;
    assert_eq!(status, 503);
    let v = plaque(addr).await;
    assert_eq!((v["link_state"].as_str(), v["sessions"].as_array().map(Vec::len)), (Some("none"), Some(0)));
}

#[tokio::test]
async fn a_browser_that_opens_the_closed_address_gets_a_page_and_a_reopened_gate_forgets_old_cookies() {
    let (r, addr) = opened_rig("").await;
    r.app.devices.set_settings(crate::devices::Settings { confirm_devices: false, ..r.app.devices.settings() });
    let key = key_in(&plaque(addr).await).unwrap();
    let (status, head, _) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"{key}\"}}")).await;
    assert_eq!(status, 200);
    let token = cookie_of(&head);
    let (status, _, _) = via(addr, "GET", "/favicon.ico", Some(&token), "").await;
    assert_eq!(status, 204, "no question to the owner: in at once");

    close(&r.app, Duration::from_secs(2)).await;
    let (status, _, page) = call_head(addr, "GET", "/", &[("Host", QUICK), ("Cf-Ray", "x"), ("Accept", "text/html")], "").await;
    assert_eq!(status, 503);
    assert!(page.contains("<h1>") && !page.contains("<script"), "{page}");

    // the same address again (a named tunnel's is always the same): the cookie of the last time means nothing
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    let (status, _, _) = via(addr, "GET", "/favicon.ico", Some(&token), "").await;
    assert_eq!(status, 401);
    let (_, _, body) = via(addr, "GET", "/g/status", Some(&token), "").await;
    assert_eq!(json_of(&body)["state"], "none");
    assert!(key_in(&plaque(addr).await).is_some_and(|k| k != key), "a gate that opens again has a key of its own");
    close(&r.app, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn a_tunnel_that_falls_takes_the_key_and_the_sessions_with_it() {
    let (r, addr) = opened_rig("die=2500").await;
    let key = key_in(&plaque(addr).await).unwrap();
    let (status, head, _) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"{key}\"}}")).await;
    assert_eq!(status, 200);
    assert_eq!(r.app.devices.list_at(crate::config::now_ms()).len(), 1);
    r.until("failed", |s| s == Phase::Failed).await;
    assert!(r.app.devices.list_at(crate::config::now_ms()).is_empty());
    assert_eq!(r.app.devices.link_at(crate::config::now_ms()).state, crate::devices::LinkState::None);
    let (status, _, _) = via(addr, "GET", "/favicon.ico", Some(&cookie_of(&head)), "").await;
    assert_eq!(status, 503);
    let v = plaque(addr).await;
    assert_eq!((v["state"].as_str(), v["sessions"].as_array().map(Vec::len), v["link"].is_null()), (Some("failed"), Some(0), true));
}

#[tokio::test]
async fn the_owners_calls_are_checked() {
    let (r, addr) = opened_rig("").await;
    let key = key_in(&plaque(addr).await).unwrap();
    let (_, head, _) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"{key}\"}}")).await;
    let _ = head;
    let id = plaque(addr).await["sessions"][0]["id"].as_str().unwrap().to_string();

    // a page of another site cannot press «Let in»
    let evil = [("Host", "localhost"), ("Origin", "http://evil.example")];
    for (p, body) in [("/api/tunnel/session", format!("{{\"id\":\"{id}\",\"op\":\"allow\"}}")), ("/api/tunnel/rotate", String::new()), ("/api/tunnel/settings", "{\"idle_close_h\":12}".into())] {
        let (status, _) = call(addr, "POST", p, &evil, &body).await;
        assert_eq!(status, 403, "{p}");
    }
    // …nor can a reverse proxy's client
    let (status, _) = call(addr, "POST", "/api/tunnel/session", &[("Host", "localhost"), ("X-Forwarded-For", "203.0.113.5")], &format!("{{\"id\":\"{id}\",\"op\":\"allow\"}}")).await;
    assert_eq!(status, 403);
    assert_eq!(r.app.devices.counts_at(crate::config::now_ms()), (1, 0), "still waiting");

    // mistakes
    let (status, _) = call(addr, "POST", "/api/tunnel/session", &LOCAL, "{\"id\":\"nobody\",\"op\":\"allow\"}").await;
    assert_eq!(status, 404);
    let (status, _) = call(addr, "POST", "/api/tunnel/session", &LOCAL, &format!("{{\"id\":\"{id}\",\"op\":\"destroy\"}}")).await;
    assert!((400..500).contains(&status), "{status}");
    let (status, _) = call(addr, "POST", "/api/tunnel/session", &LOCAL, &format!("{{\"id\":\"{id}\",\"op\":\"deny\"}}")).await;
    assert_eq!(status, 200);
    let (status, _) = call(addr, "POST", "/api/tunnel/session", &LOCAL, &format!("{{\"id\":\"{id}\",\"op\":\"allow\"}}")).await;
    assert_eq!(status, 404, "it is gone");

    // the settings: the hours are the ones offered, the full version needs a password, the file keeps them
    for bad in ["{\"idle_close_h\":3}", "{\"idle_close_h\":-1}", "{\"idle_close_h\":\"4\"}"] {
        let (status, _) = call(addr, "POST", "/api/tunnel/settings", &LOCAL, bad).await;
        assert!((400..500).contains(&status), "{bad}: {status}");
    }
    let (status, _) = call(addr, "POST", "/api/tunnel/settings", &LOCAL, "{\"full_allowed\":true}").await;
    assert_eq!(status, 409, "no password");
    let (status, body) = call(addr, "POST", "/api/tunnel/settings", &LOCAL, "{\"idle_close_h\":12,\"confirm_devices\":false}").await;
    assert_eq!(status, 200, "{body}");
    let v = json_of(&body);
    assert_eq!((v["settings"]["idle_close_h"].as_u64(), v["settings"]["confirm_devices"].as_bool()), (Some(12), Some(false)));
    assert!(v["idle_closes_ms"].as_u64().unwrap() > crate::config::now_ms() + 11 * 3_600_000);
    let saved = std::fs::read_to_string(&r.app.cfg.config_file).unwrap();
    assert!(saved.contains("idle_close_h = 12") && saved.contains("confirm_devices = false") && saved.contains("full_allowed = false"), "{saved}");
    let (status, _) = call(addr, "POST", "/api/tunnel/settings", &LOCAL, "{\"idle_close_h\":0}").await;
    assert_eq!(status, 200);
    assert_eq!(r.app.devices.idle_closes_at(), None, "never");

    // a gate that is not open has no link to renew
    close(&r.app, Duration::from_secs(2)).await;
    let (status, _) = call(addr, "POST", "/api/tunnel/rotate", &LOCAL, "").await;
    assert_eq!(status, 409);
}

#[tokio::test]
async fn an_unanswered_device_is_declined_by_the_clock_and_a_gate_nobody_uses_closes() {
    let (r, addr) = opened_rig("").await;
    let now = crate::config::now_ms();
    let key = key_in(&plaque(addr).await).unwrap();
    let (_, head, _) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"{key}\"}}")).await;
    let token = cookie_of(&head);

    // two minutes without an answer
    crate::devices::sweep_at(&r.app, now + crate::devices::ASK_MS + 1000).await;
    let (_, _, body) = via(addr, "GET", "/g/status", Some(&token), "").await;
    let j = json_of(&body);
    assert_eq!((j["state"].as_str(), j["reason"].as_str()), (Some("gone"), Some("unanswered")));
    assert_eq!(r.app.tunnel.state(), Phase::Open, "a gate that was just asked about is not idle");

    // one hour is the setting; a device that is in, and busy, keeps the gate open
    r.app.devices.set_settings(crate::devices::Settings { idle_close_h: 1, confirm_devices: false, ..r.app.devices.settings() });
    r.app.devices.rotate_at(now + 5 * HOUR);
    let key = r.app.devices.link_at(now + 5 * HOUR).key.unwrap();
    let t = r.app.devices.redeem_at(now + 5 * HOUR, "1.2.3.4", &key, crate::devices::Who { label: "iPhone · Safari 17".into(), ..Default::default() }).unwrap().token;
    r.app.devices.standing_at(now + 5 * HOUR + HOUR / 2, Some(&t));
    crate::devices::sweep_at(&r.app, now + 5 * HOUR + HOUR + 1000).await;
    assert_eq!(r.app.tunnel.state(), Phase::Open, "the device used it half an hour ago");

    let pid = r.fake_pid().await;
    crate::devices::sweep_at(&r.app, now + 5 * HOUR + HOUR / 2 + HOUR + 1000).await;
    assert_eq!(r.app.tunnel.state(), Phase::Closed);
    assert!(!r.app.gate.tunnel_open());
    wait_gone(pid).await;
    assert_eq!(r.app.devices.standing_at(now, Some(&t)), crate::devices::Standing::Nobody, "its session went with the gate");
    let lines: Vec<String> = r.app.snapshot.read().await.journal.iter().map(|e| e.text.clone()).collect();
    assert!(lines.iter().any(|l| l == "шлюз закрыт: им не пользовались 1 ч"), "{lines:?}");
    assert!(lines.iter().any(|l| l == "шлюз закрыт"), "{lines:?}");
}

#[tokio::test]
async fn a_stranger_that_guesses_is_stopped_at_five() {
    let (_r, addr) = opened_rig("").await;
    for i in 0..5 {
        let (status, _, _) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"BBBBBBBBBBBBBBBBBBBB{i:02}\"}}")).await;
        assert_eq!(status, 410, "{i}");
    }
    let (status, head, body) = via(addr, "POST", "/g/redeem", None, "{\"key\":\"BBBBBBBBBBBBBBBBBBBB99\"}").await;
    assert_eq!(status, 429, "{body}");
    assert!(head.to_lowercase().contains("retry-after: "), "{head}");
    assert!(json_of(&body)["retry_s"].as_u64().unwrap() > 0);
    // a key that is too long is no key; a body that is no JSON is refused before anything is compared
    let (status, _, _) = via(addr, "POST", "/g/redeem", None, "not json").await;
    assert!((400..500).contains(&status), "{status}");
}

#[tokio::test]
async fn a_device_can_withdraw_its_request() {
    let (r, addr) = opened_rig("").await;
    let key = key_in(&plaque(addr).await).unwrap();
    let (_, head, _) = via(addr, "POST", "/g/redeem", None, &format!("{{\"key\":\"{key}\"}}")).await;
    let token = cookie_of(&head);
    let (status, head, _) = via(addr, "POST", "/g/cancel", Some(&token), "{}").await;
    assert_eq!(status, 200);
    assert!(head.to_lowercase().contains("max-age=0"), "the cookie is cleared: {head}");
    assert_eq!(r.app.devices.counts_at(crate::config::now_ms()), (0, 0));
    let (_, _, body) = via(addr, "GET", "/g/status", Some(&token), "").await;
    assert_eq!(json_of(&body)["reason"], "left");
    // a page of another site cannot do it for the device
    let (status, _, _) = call_head(addr, "POST", "/g/cancel", &[("Host", QUICK), ("Cf-Ray", "x"), ("Origin", "https://evil.example")], "{}").await;
    assert_eq!(status, 403);
}

/* ───────── «Check» of a named tunnel's config, and the QR code ───────── */

const UUID: &str = "6ff42ae2-765d-4adf-8112-31c55c1551ef";

fn good_config() -> String {
    format!("tunnel: {UUID}\ncredentials-file: /home/me/.cloudflared/{UUID}.json\nprotocol: http2\n\ningress:\n  - hostname: planner.example.org\n    service: http://127.0.0.1:7117\n  - service: http_status:404\n")
}

fn failed(checks: &[Check]) -> Vec<&'static str> {
    checks.iter().filter(|c| !c.ok).map(|c| c.id).collect()
}

#[test]
fn a_good_config_passes_every_check() {
    let checks = checks_of(&good_config(), Some("planner.example.org"), 7117, &|_| true);
    assert!(failed(&checks).is_empty(), "{checks:?}");
    let ids: Vec<_> = checks.iter().map(|c| c.id).collect();
    assert_eq!(ids, ["file", "hostname", "tunnel", "credentials", "credentials-file", "service", "catch-all"]);
}

#[test]
fn the_credentials_file_of_another_tunnel_is_found_by_its_name_alone() {
    let other = good_config().replace(&format!("{UUID}.json"), "0a1b2c3d-1111-2222-3333-444455556666.json");
    let checks = checks_of(&other, Some("planner.example.org"), 7117, &|_| true);
    assert_eq!(failed(&checks), ["credentials"]);
    let c = checks.iter().find(|c| c.id == "credentials").unwrap();
    assert_eq!(c.have.as_deref(), Some("0a1b2c3d-1111-2222-3333-444455556666"));
    assert_eq!(c.want.as_deref(), Some(UUID));
    assert!(!c.warn, "a file of another tunnel stops the tunnel: an error");
}

#[test]
fn the_other_slips_of_a_config_are_named() {
    // no credentials-file, no tunnel
    let bare = "ingress:\n  - hostname: planner.example.org\n    service: http://127.0.0.1:7117\n  - service: http_status:404\n";
    assert_eq!(failed(&checks_of(bare, None, 7117, &|_| true)), ["tunnel", "credentials"]);
    // the planner is on another port than the tunnel leads to
    let port = good_config().replace("7117", "8080");
    let checks = checks_of(&port, None, 7117, &|_| true);
    assert_eq!(failed(&checks), ["service"]);
    let c = checks.iter().find(|c| c.id == "service").unwrap();
    assert!(c.warn && c.have.as_deref() == Some("8080") && c.want.as_deref() == Some("7117"));
    // no last rule: cloudflared would not start without it, but it is a warning here (the version may differ)
    let open = good_config().replace("  - service: http_status:404\n", "");
    assert_eq!(failed(&checks_of(&open, None, 7117, &|_| true)), ["catch-all"]);
    // the file named in the config is not there
    assert_eq!(failed(&checks_of(&good_config(), None, 7117, &|_| false)), ["credentials-file"]);
    // the address typed in the page is not among the config's
    assert_eq!(failed(&checks_of(&good_config(), Some("other.example.org"), 7117, &|_| true)), ["hostname"]);
    // a comment is not a value
    let commented = good_config().replace("tunnel: ", "# tunnel: ");
    assert!(failed(&checks_of(&commented, None, 7117, &|_| true)).contains(&"tunnel"));
}

#[test]
fn a_tunnel_named_instead_of_numbered_has_nothing_to_compare_the_file_with() {
    let by_name = good_config().replace(&format!("tunnel: {UUID}"), "tunnel: herdr-planner");
    assert!(failed(&checks_of(&by_name, None, 7117, &|_| true)).is_empty());
}

#[tokio::test]
async fn check_reads_the_config_and_never_the_credentials() {
    let r = named_rig("", None);
    let creds = r.path(&format!("{UUID}.json"));
    std::fs::write(&creds, "SECRET-DO-NOT-READ").unwrap();
    std::fs::write(r.path("planner.yml"), good_config().replace(&format!("/home/me/.cloudflared/{UUID}.json"), &creds.display().to_string())).unwrap();
    let v = view(&r.app).await;
    let check = v.config_check.unwrap();
    assert!(check.ok);
    assert!(failed(&check.checks).is_empty() || failed(&check.checks) == ["service"], "{:?}", check.checks);
    let json = serde_json::to_string(&check).unwrap();
    assert!(!json.contains("SECRET"), "the credentials are not in the answer: {json}");
    // no file at all: one failed check, and it says which file
    std::fs::remove_file(r.path("planner.yml")).unwrap();
    let v = view(&r.app).await;
    let checks = v.config_check.unwrap().checks;
    assert_eq!(failed(&checks), ["file"]);
    assert_eq!(checks[0].have.as_deref(), Some(r.path("planner.yml").display().to_string().as_str()));
}

#[test]
fn the_qr_code_is_an_inline_svg_made_here() {
    let svg = qr_svg("https://planner.example.org/#k=AAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert!(svg.starts_with("<svg") && svg.contains("viewBox") && !svg.contains("<?xml"), "{svg}");
    assert!(!svg.contains("<script") && !svg.contains("href"), "a picture, nothing to run or to follow");
}

#[tokio::test]
async fn the_page_gets_a_qr_only_while_the_link_has_a_good_key() {
    let r = rig("");
    open(&r.app).await;
    r.until("open", |s| s == Phase::Open).await;
    let v = view(&r.app).await;
    assert!(v.qr_svg.as_deref().is_some_and(|s| s.starts_with("<svg")), "an open gate with a fresh link has its QR");
    let key = v.link.as_deref().unwrap().split("#k=").nth(1).unwrap().to_string();
    assert!(!v.qr_svg.unwrap().contains(&key), "the key is in the picture's modules, not in its text");
    close(&r.app, Duration::from_secs(2)).await;
    assert!(view(&r.app).await.qr_svg.is_none(), "a closed gate has none");
}
