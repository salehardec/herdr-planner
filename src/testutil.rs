//! Helpers for the unit tests that run fake scripts.
//!
//! Executing a file that some process still holds open for writing fails with ETXTBSY ("text file busy").
//! Tests run on several threads, and a thread that forks while another one is between `open` and `close` of a
//! script hands the child a copy of that write handle until the child's own `exec`. The script then cannot be
//! run for a moment, the fake program "does not exist", and the test fails now and then.

use crate::config::AppConfig;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// A configuration for the unit tests: everything under `dir`, nothing read from the user's own files.
pub(crate) fn test_cfg(dir: &Path) -> AppConfig {
    AppConfig {
        listen: "127.0.0.1".into(),
        port: 0,
        data_dir: dir.join("data"),
        data_named: true,
        config_file: dir.join("config.toml"),
        state_dir: dir.join("state"),
        herdr_projects: "herdr-projects".into(),
        herdr_socket: dir.join("herdr.sock"),
        user: "herdr".into(),
        password_hash: String::new(),
        tls_cert: None,
        tls_key: None,
        translator: "claude/claude-haiku-5-5".into(),
        ssh_bin: "ssh".into(),
        projects_dir: dir.join("Projects"),
        trusted_hosts: Vec::new(),
        tunnel_hostname: None,
    }
}

/// Writes `body` to `path` as an executable script, and returns once no process holds it open for writing.
pub fn write_exec(path: &Path, body: impl AsRef<[u8]>) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    settle(path);
}

/// Waits (at most a few seconds) until no process has `path` open.
pub fn settle(path: &Path) {
    let Ok(want) = std::fs::canonicalize(path) else { return };
    let end = Instant::now() + Duration::from_secs(5);
    while held(&want) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn held(want: &Path) -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else { return false };
    for p in procs.flatten() {
        let name = p.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else { continue };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).is_ok_and(|t| t == want) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_script_runs_at_once() {
        let dir = std::env::temp_dir().join(format!("hp-testutil-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("hello");
        write_exec(&p, "#!/bin/sh\necho hi\n");
        let out = std::process::Command::new(&p).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
