//! The planner binary and its tunnel: `cloudflared` goes when the planner is told to stop (plain http and TLS), and a
//! `cloudflared` that a crash left behind is stopped by the next start. The stand-in is `scripts/fake-cloudflared.mjs`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/fake-cloudflared.mjs");

struct Planner {
    dir: tempfile::TempDir,
    port: u16,
    tls: bool,
    child: Option<Child>,
}

impl Drop for Planner {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(pid) = self.fake_pid() {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|t| !t.contains(") Z "))
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn wait_for(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

impl Planner {
    fn new(tls: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let port = free_port();
        std::fs::create_dir_all(d.join("data")).unwrap();
        std::fs::write(
            d.join("cloudflared"),
            format!("#!/bin/sh\nFAKE_CF='' FAKE_CF_PIDFILE='{p}/pid' exec node '{SCRIPT}' \"$@\"\n", p = d.display()),
        )
        .unwrap();
        Command::new("chmod").arg("755").arg(d.join("cloudflared")).status().unwrap();
        let mut config = format!("port = {port}\n[tunnel]\nbin = \"{}\"\n", d.join("cloudflared").display());
        if tls {
            let (cert, key) = (d.join("cert.pem"), d.join("key.pem"));
            let made = Command::new("openssl")
                .args(["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes", "-days", "1", "-subj", "/CN=localhost", "-keyout"])
                .arg(&key)
                .arg("-out")
                .arg(&cert)
                .stderr(Stdio::null())
                .status()
                .expect("openssl is needed to make a test certificate");
            assert!(made.success());
            config = format!("tls_cert = \"{}\"\ntls_key = \"{}\"\n{config}", cert.display(), key.display());
        }
        std::fs::create_dir_all(d.join("cfg/herdr-planner")).unwrap();
        std::fs::write(d.join("cfg/herdr-planner/config.toml"), config).unwrap();
        Planner { dir, port, tls, child: None }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn fake_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.path("pid")).ok().and_then(|t| t.trim().parse().ok())
    }

    fn start(&mut self) {
        let d = self.dir.path();
        let child = Command::new(env!("CARGO_BIN_EXE_herdr-planner"))
            .env("HOME", d.join("home"))
            .env("XDG_CONFIG_HOME", d.join("cfg"))
            .env("XDG_STATE_HOME", d.join("state"))
            .env("HERDR_PROJECTS_DATA_DIR", d.join("data"))
            .env("HERDR_PROJECTS_BIN", "/nonexistent/herdr-projects")
            .env_remove("HERDR_PLANNER_CLOUDFLARED")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(std::fs::File::options().create(true).append(true).open(d.join("planner.log")).unwrap()))
            .spawn()
            .unwrap();
        self.child = Some(child);
        wait_for("the planner to answer", 40, || self.curl("GET", "/api/tunnel").is_some_and(|b| b.contains("\"state\"")));
    }

    /// The body of an answer from this machine's own planner, through `curl` (it speaks TLS, the tests do not need to).
    fn curl(&self, method: &str, path: &str) -> Option<String> {
        let url = format!("{}://127.0.0.1:{}{path}", if self.tls { "https" } else { "http" }, self.port);
        let out = Command::new("curl").args(["-sk", "--max-time", "5", "-X", method, &url]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn open_tunnel(&self) -> u32 {
        self.curl("POST", "/api/tunnel/open").expect("the open call answered");
        wait_for("the tunnel to open", 30, || self.curl("GET", "/api/tunnel").is_some_and(|b| b.contains("\"state\":\"open\"")));
        self.fake_pid().expect("cloudflared started")
    }

    fn signal(&self, sig: i32) {
        unsafe { libc::kill(self.child.as_ref().unwrap().id() as i32, sig) };
    }

    fn wait_exit(&mut self, secs: u64) -> Duration {
        let t = Instant::now();
        let child = self.child.as_mut().unwrap();
        wait_for("the planner to exit", secs, || child.try_wait().unwrap().is_some());
        t.elapsed()
    }
}

fn stopped_by_sigterm(tls: bool) {
    let mut p = Planner::new(tls);
    p.start();
    let fake = p.open_tunnel();
    assert!(alive(fake));
    assert!(Path::new(&p.path("state/herdr-planner/cloudflared.pid")).exists());
    p.signal(libc::SIGTERM);
    let took = p.wait_exit(10);
    assert!(took < Duration::from_secs(3), "the planner left within its two seconds and a bit: {took:?}");
    wait_for("cloudflared to be gone", 5, || !alive(fake));
}

#[test]
fn sigterm_stops_cloudflared_with_the_planner_over_http() {
    stopped_by_sigterm(false);
}

#[test]
fn sigterm_stops_cloudflared_with_the_planner_over_tls() {
    stopped_by_sigterm(true);
}

#[test]
fn a_cloudflared_left_by_a_crashed_planner_is_stopped_by_the_next_one() {
    let mut p = Planner::new(false);
    p.start();
    let fake = p.open_tunnel();
    // the service is killed outright: nothing is stopped on the way
    p.signal(libc::SIGKILL);
    p.wait_exit(5);
    assert!(alive(fake), "the orphan is still there");
    p.start();
    wait_for("the orphan to be stopped", 10, || !alive(fake));
    let log = std::fs::read_to_string(p.path("planner.log")).unwrap_or_default();
    assert!(log.contains("previous run left behind"), "{log}");
    // and the tunnel is closed in the new run: nobody told it to open
    assert!(p.curl("GET", "/api/tunnel").unwrap().contains("\"state\":\"closed\""));
}
