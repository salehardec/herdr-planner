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
        std::fs::write(r.path("planner.yml"), text).unwrap();
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
    assert_eq!(v.link.as_deref(), Some("https://fake-quick-tunnel.trycloudflare.com"));
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
    r.until("reconnecting", |s| s == Phase::Reconnecting).await;
    assert!(r.app.gate.tunnel_open(), "the address is the same, the gate stays");
    assert!(view(&r.app).await.link.is_some());
    r.until("open again", |s| s == Phase::Open).await;
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
    assert_eq!(v.link.as_deref(), Some("https://planner.yanao.su"));
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
    for _ in 0..250 {
        if find_external(&r.path("planner.yml"), None).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
    assert_eq!(find_external(&r.path("planner.yml"), None), None);
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
    assert!(!json.contains("secret.json") && !json.contains("credentials") && !json.contains("6ff42ae2"), "{json}");
}

/* ───────── what a crash leaves, and the planner's own stop ───────── */

#[tokio::test]
async fn a_cloudflared_left_by_a_crash_is_stopped_at_the_next_start() {
    let r = rig("");
    let manual = Manual::start(&r, &["--config", "x.yml", "tunnel", "run"]);
    let pid = manual.pid();
    for _ in 0..250 {
        if std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains("cloudflared")) {
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
    std::fs::write(pid_file(&r.app.cfg), format!("{} {}\n", other.pid(), proc_start(other.pid()).unwrap() + 7)).unwrap();
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
    (status, text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
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
    assert_eq!((v["state"].as_str(), v["link"].as_str(), v["host"].as_str()), (Some("open"), Some("https://planner.yanao.su"), Some("planner.yanao.su")));

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
