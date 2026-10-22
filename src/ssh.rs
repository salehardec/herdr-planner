//! SSH calls of the planner: the board's own form for other machines and the user's form for a shipyard.
//!
//! Nothing here reads, stores or asks for SSH keys: the ssh agent and the user's `~/.ssh/config` decide who may
//! connect. The board's calls use a `known_hosts` of their own in the data folder; a shipyard is also checked the
//! way the agents will reach it (`DOCKER_HOST=ssh://…`), with the user's config and known hosts.

use crate::config::AppConfig;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// A saved machine as the planner reaches it: its label, its ssh target and its herdr session (`""` = default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineRef {
    pub label: String,
    pub target: String,
    pub session: String,
}

/// Why a machine did not answer: one line for the page, and what to do about it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Why {
    pub line: String,
    pub detail: Vec<String>,
}

/// A machine label: starts with a letter or digit, then letters, digits, `_`, `.`, `-`.
pub fn valid_label(s: &str) -> bool {
    s.len() <= 64
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// An ssh target: `host`, `user@host`, `user@host:port`, `ssh://user@host:port`. No spaces, no leading `-`.
pub fn valid_target(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('-')
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | ':' | '/' | '[' | ']' | '-')
        })
}

fn valid_port(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 5
        && s.chars().all(|c| c.is_ascii_digit())
        && s.parse::<u16>().is_ok_and(|p| p > 0)
}

/// A target as ssh takes it: `user@host:2222` and `ssh://user@host:2222` mean the same, and the port goes to `-p`:
/// ssh reads a bare `host:2222` as a hostname, so it would never connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetParts {
    pub dest: String,
    pub port: Option<String>,
}

pub fn split_target(raw: &str) -> TargetParts {
    let t = raw.strip_prefix("ssh://").unwrap_or(raw);
    let (user, hostpart) = match t.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, t),
    };
    let (host, port) = match hostpart.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((h, after)) => (
                format!("[{h}]"),
                after.strip_prefix(':').filter(|p| valid_port(p)).map(str::to_string),
            ),
            None => (hostpart.to_string(), None),
        },
        None => match hostpart.rsplit_once(':') {
            Some((h, p)) if valid_port(p) => (h.to_string(), Some(p.to_string())),
            _ => (hostpart.to_string(), None),
        },
    };
    TargetParts {
        dest: user.map(|u| format!("{u}@{host}")).unwrap_or(host),
        port,
    }
}

/// The target in the one form the planner stores and shows: no `ssh://`, a port after `:` when there is one.
pub fn normalize_target(raw: &str) -> String {
    let t = split_target(raw.trim());
    match t.port {
        Some(p) => format!("{}:{p}", t.dest),
        None => t.dest,
    }
}

/// A hand-check command line a person can paste: `ssh [-p port] [options] dest [remote command]`. Options go before
/// the destination (after it ssh would take them as the remote command), and a port is never left in the host name.
pub fn hand_check(target: &str, options: &[&str], remote: &str) -> String {
    let t = split_target(target);
    let mut s = String::from("ssh");
    if let Some(p) = t.port {
        s.push_str(&format!(" -p {p}"));
    }
    for o in options {
        s.push(' ');
        s.push_str(o);
    }
    s.push(' ');
    s.push_str(&t.dest);
    if !remote.is_empty() {
        s.push(' ');
        s.push_str(remote);
    }
    s
}

/// The argv that opens a terminal on the yard: `ssh -t [-p port] dest`. The caller owns the terminal.
pub fn tty_args(target: &str) -> Result<Vec<String>, String> {
    if !valid_target(target) {
        return Err(format!("недопустимый адрес машины: {target:?}"));
    }
    let t = split_target(target);
    let mut args = vec!["-t".to_string()];
    if let Some(p) = t.port {
        args.push("-p".into());
        args.push(p);
    }
    args.push(t.dest);
    Ok(args)
}

fn valid_session(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The argv of the planner's ssh call to a machine; the program is `cfg.ssh_bin`. Copied from herdr-projects 1.2.4
/// crates/flow/src/remote.rs (`ssh_command`): the board's own known-hosts file and control sockets, batch mode (no
/// password prompt), and a login shell on the far side so its PATH is set. `HERDR_SESSION=<s> ` is put in front of
/// the remote command only when the saved machine uses a session other than `default`.
pub fn ssh_args(cfg: &AppConfig, m: &MachineRef, remote_cmd: &str) -> Result<Vec<String>, String> {
    if !valid_label(&m.label) {
        return Err(format!("недопустимая метка машины: {:?}", m.label));
    }
    if !valid_target(&m.target) {
        return Err(format!("недопустимый адрес машины: {:?}", m.target));
    }
    let session = match m.session.as_str() {
        "" | "default" => String::new(),
        s if valid_session(s) => format!("HERDR_SESSION={s} "),
        s => return Err(format!("недопустимая сессия herdr: {s:?}")),
    };
    let d = &cfg.data_dir;
    let inner = format!("{session}{remote_cmd}");
    let t = split_target(&m.target);
    let mut args: Vec<String> = vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=6".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", d.join("known_hosts").display()),
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={}", d.join("ssh-%C").display()),
        "-o".into(),
        "ControlPersist=120".into(),
        "-o".into(),
        "LogLevel=ERROR".into(),
    ];
    if let Some(p) = t.port {
        args.push("-p".into());
        args.push(p);
    }
    args.push(t.dest);
    args.push(format!(
        "exec \"${{SHELL:-/bin/sh}}\" -lc '{}'",
        inner.replace('\'', "'\\''")
    ));
    Ok(args)
}

/// The argv of the call the agents make (`DOCKER_HOST=ssh://…`): the user's own ssh config and known hosts, batch
/// mode, a fresh host key accepted once. No control sockets and no board files: exactly the user's path.
pub fn user_ssh_args(_cfg: &AppConfig, target: &str, remote_cmd: &str) -> Result<Vec<String>, String> {
    if !valid_target(target) {
        return Err(format!("недопустимый адрес машины: {target:?}"));
    }
    let t = split_target(target);
    let mut args = vec![
        "-o".to_string(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=6".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
    ];
    if let Some(p) = t.port {
        args.push("-p".into());
        args.push(p);
    }
    args.push(t.dest);
    args.push(remote_cmd.to_string());
    Ok(args)
}

/// How an ssh call ended, as the board reads it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Ended {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub error: String,
    pub wait_ms: u64,
}

/// Runs the planner's ssh call. `Err` only when the machine's settings are refused before ssh starts.
pub(crate) async fn ssh_ended(
    cfg: &AppConfig,
    m: &MachineRef,
    remote_cmd: &str,
    wait: Duration,
) -> Result<Ended, String> {
    let args = ssh_args(cfg, m, remote_cmd)?;
    run_ended(cfg, &args, wait).await
}

/// Runs the call the agents make, with the user's own ssh configuration.
pub async fn user_ssh_ended(
    cfg: &AppConfig,
    target: &str,
    remote_cmd: &str,
    wait: Duration,
) -> Result<Ended, String> {
    let args = user_ssh_args(cfg, target, remote_cmd)?;
    run_ended(cfg, &args, wait).await
}

/// A name for a machine that is not given one: the host part of its ssh target, without the domain.
pub fn short_label(raw: &str) -> String {
    raw.rsplit('@')
        .next()
        .unwrap_or("")
        .split('.')
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_.-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// The hostname `ssh -G <target>` resolves, without connecting: the address to use for the containers' ports.
pub fn config_host(cfg: &AppConfig, target: &str) -> Option<String> {
    if !valid_target(target) {
        return None;
    }
    let t = split_target(target);
    let mut cmd = std::process::Command::new(&cfg.ssh_bin);
    cmd.arg("-G");
    if let Some(p) = &t.port {
        cmd.arg("-p").arg(p);
    }
    let out = cmd
        .arg(&t.dest)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().find_map(|line| {
        let (k, v) = line.split_once(' ')?;
        (k.eq_ignore_ascii_case("hostname") && !v.trim().is_empty()).then(|| v.trim().to_string())
    })
}

async fn run_ended(cfg: &AppConfig, args: &[String], wait: Duration) -> Result<Ended, String> {
    let mut cmd = Command::new(&cfg.ssh_bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let wait_ms = wait.as_millis() as u64;
    Ok(match timeout(wait, cmd.output()).await {
        Err(_) => Ended {
            timed_out: true,
            wait_ms,
            ..Ended::default()
        },
        Ok(Err(e)) => Ended {
            error: format!("{}: {e}", cfg.ssh_bin),
            wait_ms,
            ..Ended::default()
        },
        Ok(Ok(out)) => Ended {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            wait_ms,
            ..Ended::default()
        },
    })
}

/// What a failed call is checked with by hand, and which program the far side was expected to find.
pub(crate) struct CheckHint {
    pub test: String,
    pub program: String,
}

/// Why an ssh call did not give what was asked. The last detail is a command to check by hand.
pub(crate) fn why_not_with(end: &Ended, m: &MachineRef, hint: &CheckHint) -> Why {
    let name = if m.label.is_empty() {
        "эта машина".to_string()
    } else {
        m.label.clone()
    };
    let make = |line: &str, mut detail: Vec<String>| {
        detail.push(hint.test.clone());
        Why {
            line: line.to_string(),
            detail,
        }
    };
    let err = end.stderr.to_lowercase();
    let all = format!("{}\n{}", end.stderr, end.stdout).to_lowercase();
    if end.timed_out {
        return make(
            "не ответила вовремя",
            vec![
                format!(
                    "{name} не ответила за {} с. Планировщик спросит снова сам.",
                    (end.wait_ms as f64 / 1000.0).round()
                ),
                "Если так и будет: машина занята, или файл запуска её оболочки ждёт ввода (пароль, сетевой диск).".into(),
            ],
        );
    }
    if !end.error.is_empty() {
        return make(
            "ssh здесь не запускается",
            vec![
                format!("Этот компьютер не смог запустить ssh: {}", end.error),
                "Установите клиент OpenSSH, чтобы `ssh` был в PATH.".into(),
            ],
        );
    }
    if end.code == Some(255) || err.contains("ssh:") {
        if err.contains("host key verification failed")
            || err.contains("remote host identification has changed")
        {
            return make(
                "у машины другой ключ хоста",
                vec![
                    format!("{name} показывает другой ключ, чем тот, что сохранён, поэтому ssh отказал."),
                    "Если машину переустановили, забудьте старый ключ в known_hosts (ssh-keygen -R) и повторите.".into(),
                ],
            );
        }
        if err.contains("permission denied") || err.contains("too many authentication failures") {
            return make(
                "она не пускает эту машину",
                vec![
                    format!("{name} отказала во входе. ssh работает в пакетном режиме: ключ должен подходить сам, пароль не спрашивается."),
                    format!("Положите свой публичный ключ на машину (ssh-copy-id {}) или загрузите ключ в ssh-agent, из которого работает этот сервис.", m.target),
                ],
            );
        }
        let how = if err.contains("could not resolve hostname") {
            "её имя не известно"
        } else if err.contains("timed out") {
            "соединение не успело"
        } else if err.contains("connection refused") {
            "она отказала в соединении"
        } else if err.contains("no route to host") || err.contains("network is unreachable") {
            "до неё нет пути отсюда"
        } else {
            ""
        };
        let line = if how.is_empty() {
            "ssh не смог подключиться".to_string()
        } else {
            format!("не подключиться ({how})")
        };
        return make(
            &line,
            vec![
                format!(
                    "Эта машина не достаёт {name} ({}){}.",
                    m.target,
                    if how.is_empty() {
                        "; что ответил ssh, см. ниже".to_string()
                    } else {
                        format!(": {how}")
                    }
                ),
                "Проверьте, что она включена и в сети, и что адрес и порт верны.".into(),
            ],
        );
    }
    if end.code == Some(127) || all.contains(&format!("{}: not found", hint.program.to_lowercase())) {
        return make(
            &format!("{} там не найден", hint.program),
            vec![
                format!("Оболочка входа на {name} не находит `{}`.", hint.program),
                format!("Установите его на {name} или положите в PATH входа."),
            ],
        );
    }
    if let Some(code) = end.code.filter(|c| *c != 0) {
        return make(
            "ответила ошибкой и без ответа",
            vec![format!("Команда на {name} завершилась с кодом {code} и не ответила.")],
        );
    }
    if end.stdout.trim().is_empty() {
        return make(
            "ничего не ответила",
            vec![format!("{name} выполнила команду и ничего не напечатала.")],
        );
    }
    make(
        "ответила не то, что ждёт планировщик",
        vec![format!("{name} напечатала текст, но в нём нет нужного ответа.")],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::test_cfg;

    fn mref(label: &str, target: &str, session: &str) -> MachineRef {
        MachineRef {
            label: label.into(),
            target: target.into(),
            session: session.into(),
        }
    }

    #[test]
    fn ssh_args_are_the_boards_form() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let args = ssh_args(
            &cfg,
            &mref("vm-a", "user@vm-a.lan", ""),
            "herdr-projects remote list",
        )
        .unwrap();
        for want in [
            "BatchMode=yes",
            "ConnectTimeout=6",
            "StrictHostKeyChecking=accept-new",
            "ControlMaster=auto",
            "ControlPersist=120",
            "LogLevel=ERROR",
        ] {
            assert!(args.contains(&want.to_string()), "{want} in {args:?}");
        }
        let known = format!(
            "UserKnownHostsFile={}",
            dir.path().join("data/known_hosts").display()
        );
        assert!(args.contains(&known));
        let n = args.len();
        assert_eq!(args[n - 2], "user@vm-a.lan");
        assert_eq!(
            args[n - 1],
            "exec \"${SHELL:-/bin/sh}\" -lc 'herdr-projects remote list'"
        );
    }

    #[test]
    fn the_agents_call_uses_the_users_own_ssh_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let args = user_ssh_args(&cfg, "vm-r", "docker version").unwrap();
        assert!(!args.iter().any(|a| a.contains("known_hosts") || a.contains("ControlPath")));
        assert_eq!(args.last().unwrap(), "docker version");
        assert_eq!(args[args.len() - 2], "vm-r");
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(user_ssh_args(&cfg, "-oProxyCommand=x", "true").is_err());
    }

    #[test]
    fn the_ports_host_comes_from_the_local_ssh_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let bin = dir.path().join("ssh");
        std::fs::write(
            &bin,
            "#!/bin/sh\n[ \"$1\" = \"-G\" ] || exit 2\necho 'user me'\necho 'hostname 192.0.2.29'\necho 'port 22'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = AppConfig { ssh_bin: bin.display().to_string(), ..cfg };
        assert_eq!(config_host(&cfg, "vm-r").as_deref(), Some("192.0.2.29"));
        assert_eq!(config_host(&cfg, "vm-r"), Some("192.0.2.29".into()));
    }

    #[test]
    fn a_port_is_split_from_the_host_and_goes_to_p() {
        let t = split_target("me@192.0.2.29:2222");
        assert_eq!(t.dest, "me@192.0.2.29");
        assert_eq!(t.port.as_deref(), Some("2222"));
        assert_eq!(normalize_target("ssh://me@192.0.2.29:2222"), "me@192.0.2.29:2222");
        assert_eq!(normalize_target("vm-r"), "vm-r");
        let t = split_target("[::1]:2200");
        assert_eq!(t.dest, "[::1]");
        assert_eq!(t.port.as_deref(), Some("2200"));
        // A colon that is not a port is left alone (it may be part of an unbracketed IPv6).
        assert_eq!(normalize_target("2001:db8::1"), "2001:db8::1");
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let args = ssh_args(&cfg, &mref("vm-r", "me@192.0.2.29:2222", ""), "true").unwrap();
        let n = args.len();
        assert_eq!(args[n - 2], "me@192.0.2.29");
        assert_eq!(args[n - 3], "2222");
        assert_eq!(args[n - 4], "-p");
        let args = user_ssh_args(&cfg, "ssh://me@192.0.2.29:2222", "docker version").unwrap();
        let n = args.len();
        assert_eq!(args[n - 1], "docker version");
        assert_eq!(args[n - 2], "me@192.0.2.29");
        assert_eq!(args[n - 3], "2222");
        assert_eq!(args[n - 4], "-p");
        assert_eq!(tty_args("me@192.0.2.29:2222").unwrap().join(" "), "-t -p 2222 me@192.0.2.29");
        assert_eq!(tty_args("vm-r").unwrap().join(" "), "-t vm-r");
        assert_eq!(
            hand_check("me@192.0.2.29:2222", &["-o", "BatchMode=yes"], "true"),
            "ssh -p 2222 -o BatchMode=yes me@192.0.2.29 true"
        );
        assert_eq!(
            hand_check("vm-r", &[], "docker version"),
            "ssh vm-r docker version"
        );
    }

    #[test]
    fn the_session_prefix_only_for_a_named_session() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let with = ssh_args(
            &cfg,
            &mref("vm-a", "vm-a", "work"),
            "herdr-projects remote list",
        )
        .unwrap();
        assert!(with
            .last()
            .unwrap()
            .ends_with("-lc 'HERDR_SESSION=work herdr-projects remote list'"));
        for s in ["default", ""] {
            let plain =
                ssh_args(&cfg, &mref("vm-a", "vm-a", s), "herdr-projects remote list").unwrap();
            assert!(!plain.last().unwrap().contains("HERDR_SESSION"), "{s}");
        }
    }

    #[test]
    fn a_quote_in_the_remote_command_is_escaped_for_the_login_shell() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_cfg(dir.path());
        let args = ssh_args(&cfg, &mref("vm-a", "vm-a", ""), "echo 'a'").unwrap();
        assert_eq!(
            args.last().unwrap(),
            "exec \"${SHELL:-/bin/sh}\" -lc 'echo '\\''a'\\'''"
        );
    }
}
