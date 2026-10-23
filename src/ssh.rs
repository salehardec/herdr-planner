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

/// The shell code that gives the far side a terminal type it has terminfo for (PLN-57).
///
/// A terminal window of this machine sends its own `TERM` (`xterm-kitty` for kitty, `foot-extra`, …), and neither a
/// shipyard nor a task container has to have that entry: `htop` then says «Error opening terminal: xterm-kitty» and
/// `top` «unknown terminal type». The code looks the type up the way a program there would — `infocmp`, else
/// `tput longname`, else the compiled database itself — and replaces a type the far side does not know with
/// `xterm-256color`, and that one, if it is missing too, with `xterm`. `dumb`/`unknown`/an empty `TERM` are no
/// terminal at all (a full-screen program cannot draw on them either), so they take the same road. `COLORTERM` (the
/// terminal's `truecolor`) comes as the first argument — ssh carries `TERM` alone, since a shipyard's sshd keeps
/// `AcceptEnv LANG LC_*` — and is exported beside it, so the colours stay what they were. Nothing is written on the
/// far side: every lookup only reads.
const TERM_FALLBACK: &str = r#"t=${TERM:-}
case $t in ''|dumb|unknown) t= ;; esac
have() { [ -n "$1" ] || return 1
  if command -v infocmp >/dev/null 2>&1; then infocmp "$1" >/dev/null 2>&1; return; fi
  if command -v tput >/dev/null 2>&1; then tput -T "$1" longname >/dev/null 2>&1; return; fi
  case $1 in ?*) f=${1%"${1#?}"} ;; *) return 1 ;; esac
  h=$(printf %x "'$f" 2>/dev/null)
  for d in "${TERMINFO:-}" "${HOME:-/root}/.terminfo" /etc/terminfo /lib/terminfo /usr/share/terminfo; do
    [ -n "$d" ] || continue
    if [ -e "$d/$f/$1" ] || { [ -n "$h" ] && [ -e "$d/$h/$1" ]; }; then return 0; fi
  done
  return 1
}
if ! have "$t"; then TERM=xterm-256color; have "$TERM" || TERM=xterm; export TERM; fi
if [ -n "${1:-}" ]; then COLORTERM=$1; export COLORTERM; fi"#;

/// A shell script for a remote command: `sh -c '<script>'`, with the script's own quotes escaped for the login
/// shell that parses the command line first (`ssh -t` hands it to `$SHELL -c`).
fn sh_c(script: &str) -> String {
    format!("sh -c '{}'", script.replace('\'', "'\\''"))
}

/// The argv that opens a terminal on the yard: `ssh -t [-p port] dest sh -c '<term check>; exec "$SHELL" -l'`. The
/// check of [`TERM_FALLBACK`] runs first, so the login shell — the one the terminal would have had — starts with a
/// `TERM` the yard has terminfo for. The caller owns the terminal.
pub fn tty_args(target: &str) -> Result<Vec<String>, String> {
    let mut args = tty_prefix(target)?;
    args.push(sh_c(&format!(
        "{TERM_FALLBACK}\nexec \"${{SHELL:-/bin/sh}}\" -l"
    )));
    Ok(args)
}

/// `-t [-p port] dest`: the ssh side of a terminal, without its remote command.
fn tty_prefix(target: &str) -> Result<Vec<String>, String> {
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

/// The argv that opens a terminal inside one container (PLN-44): `ssh -t [-p port] dest docker exec -it <id> …`.
/// The shell is bash when the image has it and sh otherwise (`sh -c` picks one and `exec`s it, so the terminal is the
/// container's own), after the [`TERM_FALLBACK`] of the image's own terminal type. Only a full docker id may reach the
/// command; the caller checks the container is a task's and is running. The caller owns the terminal, as with
/// `tty_args`.
pub fn container_shell_args(target: &str, id: &str) -> Result<Vec<String>, String> {
    if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("недопустимый id контейнера: {id:?}"));
    }
    let mut args = tty_prefix(target)?;
    // The check runs inside the container, not on the yard: an image may have no terminfo for the terminal's type.
    let inner = format!(
        "{TERM_FALLBACK}\nif command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi"
    );
    args.push(format!("docker exec -it {id} {}", sh_c(&inner)));
    Ok(args)
}

/// The local wrapper of a terminal window that runs `ssh …` (see [`terminal_argv`]): the terminal's `COLORTERM`
/// travels as the first argument of the far side's shell, because ssh passes `TERM` alone.
const COLOR_AS_ARGUMENT: &str = r#"c=${COLORTERM:-}
case $c in ''|*[!A-Za-z0-9._+-]*) exec "$@" ;; esac
exec "$@" herdr "$c""#;

/// The argv of the terminal window that opens `ssh_bin args…`: `sh -c <wrapper> herdr ssh_bin args…`.
///
/// The window is the one that knows the terminal's `COLORTERM` (`truecolor`), while the board does not, and a
/// shipyard's sshd takes only `LANG LC_*` from the client: without the wrapper a truecolor terminal would reach the
/// far side as a plain 256-colour one as soon as `TERM` is fixed up. The wrapper puts the colour after the remote
/// command as a plain word, where [`TERM_FALLBACK`] exports it (`sh -c '…' herdr truecolor`); anything that is not
/// such a word is left out rather than quoted through two shells.
pub fn terminal_argv(ssh_bin: &str, args: &[String]) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "sh".into(),
        "-c".into(),
        COLOR_AS_ARGUMENT.into(),
        "herdr".into(),
        ssh_bin.to_string(),
    ];
    argv.extend(args.iter().cloned());
    argv
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
        let yard = tty_args("me@192.0.2.29:2222").unwrap();
        assert_eq!(yard[..4], ["-t", "-p", "2222", "me@192.0.2.29"]);
        // The terminal checks the far side's terminfo first, then starts the login shell it would have started
        // anyway (PLN-57).
        assert!(yard[4].starts_with("sh -c '"), "{yard:?}");
        assert!(yard[4].contains("infocmp") && yard[4].contains("xterm-256color"), "{yard:?}");
        assert!(yard[4].ends_with("exec \"${SHELL:-/bin/sh}\" -l'"), "{yard:?}");
        assert_eq!(tty_args("vm-r").unwrap()[..2], ["-t", "vm-r"]);
        // A shell goes into the container by its full id: `ssh -t … docker exec -it …`.
        let id = "a".repeat(64);
        let shell = container_shell_args("me@192.0.2.29:2222", &id).unwrap();
        assert_eq!(shell[..4], ["-t", "-p", "2222", "me@192.0.2.29"]);
        assert!(shell[4].starts_with(&format!("docker exec -it {id} sh -c '")));
        assert!(shell[4].contains("xterm-256color"));
        assert!(shell[4].contains("exec bash") && shell[4].contains("exec sh"));
        assert!(container_shell_args("vm-r", "a b").is_err());
        assert!(container_shell_args("vm-r", &"a".repeat(63)).is_err());
        assert!(container_shell_args("-x", &id).is_err());
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

    /// The terminal check run the way the far side runs it: `sh -c '<check>; printf …'`, with `path` naming the
    /// terminfo tools it may find and `color` as the first argument (the terminal's own `COLORTERM`).
    #[cfg(unix)]
    fn run_term(
        path: &std::path::Path,
        envs: &[(&str, &str)],
        term: &str,
        color: Option<&str>,
    ) -> String {
        let script = format!("{TERM_FALLBACK}\nprintf '%s|%s' \"$TERM\" \"${{COLORTERM:-}}\"");
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(script).arg("herdr");
        if let Some(c) = color {
            cmd.arg(c);
        }
        cmd.env_clear().env("PATH", path).env("TERM", term);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    #[cfg(unix)]
    fn the_terminal_type_falls_back_to_one_the_far_side_has() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // A shipyard as it is: no entry for the terminal's own type, one for xterm-256color. The colour of the
        // terminal travels beside the type.
        crate::testutil::write_exec(&bin.join("infocmp"), "#!/bin/sh\n[ \"$1\" = xterm-256color ]\n");
        assert_eq!(
            run_term(&bin, &[], "xterm-kitty", Some("truecolor")),
            "xterm-256color|truecolor"
        );
        // The far side knows the terminal's type: nothing changes.
        crate::testutil::write_exec(&bin.join("infocmp"), "#!/bin/sh\nexit 0\n");
        assert_eq!(
            run_term(&bin, &[], "xterm-kitty", Some("truecolor")),
            "xterm-kitty|truecolor"
        );
        // Neither it nor xterm-256color: xterm.
        crate::testutil::write_exec(&bin.join("infocmp"), "#!/bin/sh\nexit 1\n");
        assert_eq!(run_term(&bin, &[], "xterm-kitty", None), "xterm|");
        // A type that is no terminal at all takes the same road, although the far side "knows" it.
        crate::testutil::write_exec(&bin.join("infocmp"), "#!/bin/sh\nexit 0\n");
        assert_eq!(run_term(&bin, &[], "dumb", None), "xterm-256color|");
        assert_eq!(run_term(&bin, &[], "", None), "xterm-256color|");
    }

    #[test]
    #[cfg(unix)]
    fn without_terminfo_tools_the_database_itself_is_looked_at() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("bin"); // neither infocmp nor tput: a bare image
        std::fs::create_dir_all(&empty).unwrap();
        let ti = dir.path().join("terminfo");
        std::fs::create_dir_all(ti.join("x")).unwrap();
        std::fs::write(ti.join("x/xterm-kitty"), b"").unwrap();
        let ti_s = ti.to_str().unwrap();
        assert_eq!(
            run_term(&empty, &[("TERMINFO", ti_s)], "xterm-kitty", None),
            "xterm-kitty|"
        );
        // Some terminfo trees keep the entry under the hex code of its first letter.
        let hex = dir.path().join("hex");
        std::fs::create_dir_all(hex.join("78")).unwrap();
        std::fs::write(hex.join("78/xterm-kitty"), b"").unwrap();
        assert_eq!(
            run_term(&empty, &[("TERMINFO", hex.to_str().unwrap())], "xterm-kitty", None),
            "xterm-kitty|"
        );
        // A type no tree has: the fallback whose own chain the tools are asked along.
        let none = dir.path().join("none");
        std::fs::create_dir_all(&none).unwrap();
        let got = run_term(&empty, &[("TERMINFO", none.to_str().unwrap())], "herdr-none", None);
        assert!(got.starts_with("xterm"), "{got}");
    }

    #[test]
    #[cfg(unix)]
    fn the_terminal_colour_travels_with_the_ssh_call() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("args");
        let ssh = dir.path().join("ssh");
        crate::testutil::write_exec(
            &ssh,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\036' \"$a\"; done > {}\n",
                log.display()
            ),
        );
        let args = tty_args("me@192.0.2.29").unwrap();
        let argv = terminal_argv(&ssh.display().to_string(), &args);
        assert_eq!(argv[0], "sh");
        assert_eq!(argv[1], "-c");
        assert_eq!(argv[2], COLOR_AS_ARGUMENT);
        assert_eq!(argv[3], "herdr");
        assert_eq!(argv[4], ssh.display().to_string());
        let run = |color: Option<&str>| {
            let _ = std::fs::remove_file(&log);
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.args(&argv[1..]);
            match color {
                Some(c) => cmd.env("COLORTERM", c),
                None => cmd.env_remove("COLORTERM"),
            };
            let out = cmd.output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            std::fs::read_to_string(&log)
                .unwrap()
                .split('\x1e')
                .filter(|a| !a.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        let seen = run(Some("truecolor"));
        assert_eq!(seen[seen.len() - 2..], ["herdr", "truecolor"]);
        // Without a colour, or with something that is not a plain word, nothing is added: a word of the far side's
        // command line cannot be quoted through two shells safely.
        let plain = run(None);
        assert_eq!(plain.last().unwrap(), args.last().unwrap());
        let odd = run(Some("true color; rm -rf /"));
        assert_eq!(odd.last().unwrap(), args.last().unwrap());
    }
}
