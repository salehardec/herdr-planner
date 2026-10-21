//! Far files of the «Galaxy» (PLN-9): «Экран агента» and «Файлы (yazi)» for a far machine of the board.
//!
//! Everything reaches the far machine the way the board does (`machines::ssh_args`: batch mode, the board's known
//! hosts and control socket, the login-shell form). The interactive terminal uses the same options with `-t` and
//! without `BatchMode`, so a password prompt or a sudo prompt can be answered in the opened window.
//! Nothing from the browser reaches a shell unquoted: labels are validated, worktrees must be absolute, and every
//! path that goes to a remote shell is quoted with `sh_quote`.
//! Remote scripts are fixed text; the only values put into them are a download URL we built ourselves and a path
//! that `sh_quote` made safe.

// The server does not call these yet (the lead wires them into server.rs); drop this line once it does.


use crate::config::AppConfig;
use crate::machines::{self, Ended, MachineRef};
use serde::Serialize;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// The same waits as the board's own calls: a list or a check, and an ordinary action.
const CHECK_WAIT: Duration = Duration::from_secs(25);
const STEP_WAIT: Duration = Duration::from_secs(60);
/// A release zip or a `brew install` can take minutes on a slow link; ssh is not killed earlier than this.
const DOWNLOAD_WAIT: Duration = Duration::from_secs(600);

/// Where the release binaries are published (the `latest` redirect; asset names checked against sxyazi/yazi).
pub const RELEASE_URL: &str = "https://github.com/sxyazi/yazi/releases/latest/download";

/// A package manager the far machine has, and whether it can give yazi.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manager {
    /// `pacman`, `brew` or `apt`.
    pub id: &'static str,
    /// The command that installs yazi with it (shown to the person, not run from here when it needs sudo).
    pub command: String,
    pub sudo: bool,
    pub available: bool,
    pub why_not: Option<String>,
}

/// What the far machine has for yazi: where it is, the system, the helpers yazi uses, and how to install it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct YaziState {
    pub present: bool,
    /// The yazi on the far machine's login PATH, else the one this planner installed (`~/.local/bin/yazi`).
    pub path: Option<String>,
    /// `linux`, `macos` or `unknown` (from `uname -s`).
    pub os: String,
    /// `x86_64`, `aarch64`, or the raw `uname -m` when unknown.
    pub arch: String,
    /// `file` is what yazi uses to tell file types, and so what it previews; there is no need for more.
    pub has_file: bool,
    /// `chafa` draws images with characters, when the terminal cannot draw them.
    pub has_chafa: bool,
    pub managers: Vec<Manager>,
    /// The release zip for this system, when there is one.
    pub binary_url: Option<String>,
    pub notes: Vec<String>,
}

/// One remote call that prints the whole picture: yazi (login PATH, then the remembered install, then
/// `~/.local/bin`), `uname -sm`, the helpers and managers that exist, and whether the managers carry yazi.
const CHECK_SCRIPT: &str = r#"y=$(command -v yazi 2>/dev/null)
[ -n "$y" ] || { r=$(cat "$HOME/.local/share/herdr-planner-far/yazi-path" 2>/dev/null); [ -n "$r" ] && [ -x "$r" ] && y=$r; }
[ -n "$y" ] || { [ -x "$HOME/.local/bin/yazi" ] && y=$HOME/.local/bin/yazi; }
printf 'yazi=%s\n' "$y"
printf 'uname=%s\n' "$(uname -sm 2>/dev/null)"
for t in file chafa pacman brew apt-get; do command -v "$t" >/dev/null 2>&1 && printf 'tool=%s\n' "$t"; done
command -v pacman >/dev/null 2>&1 && pacman -Si yazi >/dev/null 2>&1 && printf 'pacman_yazi=1\n'
command -v brew >/dev/null 2>&1 && HOMEBREW_NO_AUTO_UPDATE=1 brew info --formula yazi >/dev/null 2>&1 && printf 'brew_yazi=1\n'
exit 0"#;

/// Unpacks the downloaded zip (unzip, else bsdtar, else python3) and puts `yazi` and `ya` into `~/.local/bin`.
/// Copies to a new name and renames, so a running yazi is not written over.
const UNPACK_SCRIPT: &str = r#"set -e
cd "$HOME/.cache/herdr-planner-far/yazi-dl"
rm -rf x; mkdir x
if command -v unzip >/dev/null 2>&1; then unzip -q yazi.zip -d x
elif command -v bsdtar >/dev/null 2>&1; then bsdtar -xf yazi.zip -C x
elif command -v python3 >/dev/null 2>&1; then python3 -I -c "import sys,zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" yazi.zip x
else echo "нет unzip, bsdtar и python3" >&2; exit 4; fi
y=$(find x -type f -name yazi | head -n 1)
a=$(find x -type f -name ya | head -n 1)
if [ -z "$y" ]; then echo "в архиве нет yazi" >&2; exit 4; fi
mkdir -p "$HOME/.local/bin"
cp "$y" "$HOME/.local/bin/.yazi.new" && chmod +x "$HOME/.local/bin/.yazi.new" && mv -f "$HOME/.local/bin/.yazi.new" "$HOME/.local/bin/yazi"
if [ -n "$a" ]; then cp "$a" "$HOME/.local/bin/.ya.new" && chmod +x "$HOME/.local/bin/.ya.new" && mv -f "$HOME/.local/bin/.ya.new" "$HOME/.local/bin/ya"; fi
echo installed"#;

/// Checks the installed binary runs, remembers its path, and removes the download folder. Prints the path last.
const VERIFY_SCRIPT: &str = r#"set -e
y="$HOME/.local/bin/yazi"
"$y" --version
mkdir -p "$HOME/.local/share/herdr-planner-far"
printf '%s\n' "$y" > "$HOME/.local/share/herdr-planner-far/yazi-path"
rm -rf "$HOME/.cache/herdr-planner-far/yazi-dl"
echo "$y""#;

/// `brew install yazi`, then the same memory of the path. No sudo: Homebrew does not need it.
const BREW_SCRIPT: &str = r#"set -e
brew install yazi
y=$(command -v yazi)
if [ -z "$y" ]; then echo "yazi не нашёлся в PATH после brew" >&2; exit 5; fi
mkdir -p "$HOME/.local/share/herdr-planner-far"
printf '%s\n' "$y" > "$HOME/.local/share/herdr-planner-far/yazi-path"
echo "$y""#;

/// The release asset for a system, as the release names it (`yazi-<arch>-<triple>.zip`).
pub fn asset_name(os: &str, arch: &str) -> Option<String> {
    match (os, arch) {
        ("linux", "x86_64" | "aarch64") => Some(format!("yazi-{arch}-unknown-linux-musl.zip")),
        ("macos", "x86_64" | "aarch64") => Some(format!("yazi-{arch}-apple-darwin.zip")),
        _ => None,
    }
}

/// Reads the check script's output. Unknown lines (a login shell's greetings) are skipped.
pub(crate) fn parse_check(out: &str) -> YaziState {
    let mut st = YaziState {
        os: "unknown".into(),
        ..YaziState::default()
    };
    let mut tools: Vec<&str> = Vec::new();
    let (mut pacman_yazi, mut brew_yazi) = (false, false);
    let mut uname = String::new();
    for line in out.lines().map(str::trim_end) {
        if let Some(p) = line.strip_prefix("yazi=") {
            if p.starts_with('/') {
                st.path = Some(p.to_string());
            }
        } else if let Some(u) = line.strip_prefix("uname=") {
            uname = u.trim().to_string();
        } else if let Some(t) = line.strip_prefix("tool=") {
            tools.push(t);
        } else if line == "pacman_yazi=1" {
            pacman_yazi = true;
        } else if line == "brew_yazi=1" {
            brew_yazi = true;
        }
    }
    st.present = st.path.is_some();
    let mut parts = uname.split_whitespace();
    let kernel = parts.next().unwrap_or("");
    let machine = parts.next().unwrap_or("");
    st.os = match kernel {
        "Linux" => "linux",
        "Darwin" => "macos",
        _ => "unknown",
    }
    .to_string();
    st.arch = match machine {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        other => other,
    }
    .to_string();
    st.has_file = tools.contains(&"file");
    st.has_chafa = tools.contains(&"chafa");
    st.binary_url = asset_name(&st.os, &st.arch).map(|a| format!("{RELEASE_URL}/{a}"));
    if st.binary_url.is_none() {
        st.notes.push(if uname.is_empty() {
            "не удалось узнать систему машины (uname)".to_string()
        } else {
            format!("двоичного релиза yazi для «{uname}» нет: поставьте его вручную")
        });
    }
    if !st.has_file {
        st.notes.push(
            "на машине нет file: yazi не определит типы файлов, картинки не покажутся".into(),
        );
    }
    if tools.contains(&"pacman") {
        let ok = pacman_yazi && st.os == "linux";
        st.managers.push(Manager {
            id: "pacman",
            command: "sudo pacman -S --needed yazi".into(),
            sudo: true,
            available: ok,
            why_not: (!ok).then(|| "в репозиториях pacman нет yazi".to_string()),
        });
    }
    if tools.contains(&"brew") {
        st.managers.push(Manager {
            id: "brew",
            command: "brew install yazi".into(),
            sudo: false,
            available: brew_yazi,
            why_not: (!brew_yazi)
                .then(|| "в Homebrew нет формулы yazi или brew не ответил".to_string()),
        });
    }
    if tools.contains(&"apt-get") {
        st.managers.push(Manager {
            id: "apt",
            command: "sudo apt-get install yazi".into(),
            sudo: true,
            available: false,
            why_not: Some(
                "в Ubuntu/Debian пакета yazi нет: вариант выключен, ставьте двоичным файлом".into(),
            ),
        });
    }
    st
}

/// A valid worktree for the remote shell: an absolute path, one line, no NUL.
fn valid_worktree(p: &str) -> Result<(), String> {
    if !p.starts_with('/') || p.len() > 1000 || p.chars().any(|c| c == '\0' || c == '\n' || c == '\r') {
        return Err(format!("недопустимая папка на галактике: {p:?}"));
    }
    Ok(())
}

/// An agent name as the board makes it (`sw-<repo>-<n>`): letters, digits, `-`, `_`, `.`.
fn valid_agent(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 120
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        return Err(format!("недопустимое имя агента: {name:?}"));
    }
    Ok(())
}

fn valid_session(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The remote command that opens yazi in a worktree. If it cannot, the window waits for Enter instead of closing.
pub(crate) fn open_remote(worktree: &str, yazi: &str) -> String {
    format!(
        "cd {} && exec {} || {{ echo \"yazi не открылся: проверьте папку и установку\"; read -r _; }}",
        machines::sh_quote(worktree),
        machines::sh_quote(yazi)
    )
}

/// The remote command that installs yazi with pacman. The human types the sudo password in the window.
pub fn terminal_install_command(method: &str) -> Result<String, String> {
    match method {
        "pacman" => Ok("sudo pacman -S --needed yazi".to_string()),
        other => Err(format!("в терминале ставится только pacman, а не {other:?}")),
    }
}

/// The ssh call of an interactive terminal: the board's options, but no `BatchMode` (a prompt may be answered) and
/// `-t` (a tty for yazi, sudo or a pager). The remote command keeps the login-shell form of `ssh_args`.
pub fn ssh_tty_args(
    cfg: &AppConfig,
    m: &MachineRef,
    remote_cmd: &str,
) -> Result<Vec<String>, String> {
    let mut args = machines::ssh_args(cfg, m, remote_cmd)?;
    let i = args
        .iter()
        .position(|a| a == "BatchMode=yes")
        .ok_or_else(|| "ssh_args без BatchMode: форма изменилась".to_string())?;
    // `ssh_args` puts `-o` right before each option, so the pair is i-1 and i.
    args.drain(i - 1..=i);
    args.insert(0, "-t".into());
    Ok(args)
}

/// A command line as a shell would read it back: each word quoted when needed. For display and for AppleScript.
pub fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|a| machines::sh_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn applescript_text(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn with_head(head: &[&str], cmd: &[String]) -> Vec<String> {
    head.iter()
        .map(|s| s.to_string())
        .chain(cmd.iter().cloned())
        .collect()
}

/// The argv that opens a terminal window running `cmd`. Pure, so it is tested as data.
///
/// Linux: `HERDR_PLANNER_TERMINAL` if set, else `xdg-terminal-exec` (on the planner's own machines the first one
/// it finds is kitty, which draws PNG/JPG previews over ssh). macOS: the first of kitty, Ghostty, WezTerm, iTerm2
/// that is installed (`mac_app`, found by the caller), started as a new instance with `open -na`; Terminal.app
/// when none is.
pub fn terminal_argv(
    os: &str,
    term_env: Option<&str>,
    mac_app: Option<&str>,
    cmd: &[String],
) -> Vec<String> {
    if os != "macos" {
        let term = term_env
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or("xdg-terminal-exec");
        return with_head(&[term], cmd);
    }
    let script = |text: String| vec!["osascript".to_string(), "-e".to_string(), text];
    let line = applescript_text(&shell_line(cmd));
    match mac_app {
        Some("kitty") => with_head(&["open", "-na", "kitty", "--args"], cmd),
        Some("Ghostty") => with_head(&["open", "-na", "Ghostty", "--args", "-e"], cmd),
        Some("WezTerm") => with_head(&["open", "-na", "WezTerm", "--args", "start", "--"], cmd),
        Some("iTerm2") => script(format!(
            "tell application \"iTerm\" to create window with default profile command \"{line}\""
        )),
        _ => script(format!("tell application \"Terminal\" to do script \"{line}\"")),
    }
}

/// The first graphics-capable terminal app installed on this Mac, in the mockup's order.
fn detect_mac_app() -> Option<&'static str> {
    let home = std::env::var("HOME").unwrap_or_default();
    [
        ("kitty", "kitty.app"),
        ("Ghostty", "Ghostty.app"),
        ("WezTerm", "WezTerm.app"),
        ("iTerm2", "iTerm.app"),
    ]
    .into_iter()
    .find(|(_, bundle)| {
        Path::new("/Applications").join(bundle).exists()
            || Path::new(&home).join("Applications").join(bundle).exists()
    })
    .map(|(name, _)| name)
}

/// Starts a terminal window running `cmd`. Does not wait for it: the window is the person's now.
pub(crate) fn spawn_in_terminal(cmd: &[String], term_env: Option<&str>) -> Result<(), String> {
    let os = std::env::consts::OS;
    let mac_app = if os == "macos" { detect_mac_app() } else { None };
    let argv = terminal_argv(os, term_env, mac_app, cmd);
    let (prog, rest) = argv
        .split_first()
        .ok_or_else(|| "нет команды терминала".to_string())?;
    Command::new(prog)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_child| ())
        .map_err(|e| format!("не удалось открыть терминал ({prog}): {e}"))
}

pub(crate) fn env_terminal() -> Option<String> {
    std::env::var("HERDR_PLANNER_TERMINAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// The herdr binary and the saved machine's label, as the board's own calls resolve them.
async fn resolve(herdr: &str, label: &str) -> Result<MachineRef, String> {
    machines::resolve_with(herdr, label).await
}

/// The check call, on a machine already resolved. Its error is the board's own one-line reason.
async fn check_machine(cfg: &AppConfig, m: &MachineRef) -> Result<YaziState, String> {
    let end = machines::ssh_ended(cfg, m, CHECK_SCRIPT, CHECK_WAIT).await?;
    if end.code != Some(0) || end.timed_out {
        return Err(machines::why_not(&end, m).line);
    }
    Ok(parse_check(&end.stdout))
}

/// A step's error, in words for the person: a timeout, a spawn failure, or the last line the step printed.
pub(crate) fn step_error(what: &str, end: &Ended, m: &MachineRef) -> String {
    if end.timed_out {
        return format!(
            "{what}: машина «{}» не ответила за {} с",
            m.label,
            end.wait_ms / 1000
        );
    }
    if !end.error.is_empty() {
        return format!("{what}: {}", end.error);
    }
    let text = if end.stderr.trim().is_empty() {
        &end.stdout
    } else {
        &end.stderr
    };
    match text.lines().map(str::trim).rfind(|l| !l.is_empty()) {
        Some(last) => format!("{what}: {last}"),
        None => format!(
            "{what}: код выхода {}",
            end.code.map_or("неизвестен".to_string(), |c| c.to_string())
        ),
    }
}

/// One install step on the far machine, through the board's own ssh form. Returns its stdout.
async fn run_step(
    cfg: &AppConfig,
    m: &MachineRef,
    script: &str,
    wait: Duration,
    what: &str,
) -> Result<String, String> {
    let end = machines::ssh_ended(cfg, m, script, wait).await?;
    if end.code == Some(0) && !end.timed_out && end.error.is_empty() {
        Ok(end.stdout)
    } else {
        Err(step_error(what, &end, m))
    }
}

/// The last line that is an absolute path: the path a step printed last.
fn last_path(out: &str) -> Option<String> {
    out.lines()
        .map(str::trim)
        .rfind(|l| l.starts_with('/'))
        .map(str::to_string)
}

fn download_script(url: &str) -> String {
    format!(
        "set -e\n\
         d=\"$HOME/.cache/herdr-planner-far/yazi-dl\"; rm -rf \"$d\"; mkdir -p \"$d\"; cd \"$d\"\n\
         if command -v curl >/dev/null 2>&1; then curl -fsSL -o yazi.zip {u}\n\
         elif command -v wget >/dev/null 2>&1; then wget -q -O yazi.zip {u}\n\
         else echo \"нет curl и wget\" >&2; exit 3; fi\n\
         ls -l yazi.zip",
        u = machines::sh_quote(url)
    )
}

/// The install of yazi, one step at a time (each a call on the far machine). `method`: `binary` puts the release
/// into `~/.local/bin` without sudo; `brew` runs `brew install yazi`; `pacman` is refused here (sudo is typed by the
/// person in a terminal: see `open_install_terminal`). `progress` gets a Russian line per step.
/// Returns the path of the installed yazi.
pub async fn install_yazi(
    cfg: &AppConfig,
    label: &str,
    method: &str,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<String, String> {
    install_with(cfg, &machines::herdr_bin(), label, method, progress).await
}

pub(crate) async fn install_with(
    cfg: &AppConfig,
    herdr: &str,
    label: &str,
    method: &str,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<String, String> {
    match method {
        "binary" | "brew" => {}
        "pacman" => {
            return Err(format!(
                "pacman ставит yazi с sudo, пароль вводит человек: откройте терминал и выполните `{}`",
                terminal_install_command("pacman")?
            ))
        }
        other => return Err(format!("неизвестный способ установки yazi: {other:?}")),
    }
    let m = resolve(herdr, label).await?;
    progress("проверяю машину…");
    let st = check_machine(cfg, &m).await?;
    if method == "brew" {
        let brew = st.managers.iter().find(|x| x.id == "brew");
        if !brew.is_some_and(|b| b.available) {
            let why = brew
                .and_then(|b| b.why_not.clone())
                .unwrap_or_else(|| "на машине нет brew".to_string());
            return Err(format!("brew не поставит yazi на «{label}»: {why}"));
        }
        progress("ставлю через brew (это может занять минуты)…");
        let out = run_step(cfg, &m, BREW_SCRIPT, DOWNLOAD_WAIT, "brew install yazi").await?;
        return last_path(&out)
            .ok_or_else(|| format!("на «{label}» brew не назвал путь yazi"));
    }
    let url = st.binary_url.clone().ok_or_else(|| {
        format!(
            "на «{label}» не выбрать релиз yazi: {}",
            st.notes.join("; ")
        )
    })?;
    let asset = url.rsplit('/').next().unwrap_or("yazi.zip").to_string();
    progress(&format!("скачиваю {asset}…"));
    run_step(cfg, &m, &download_script(&url), DOWNLOAD_WAIT, "скачивание yazi").await?;
    progress("распаковываю и ставлю в ~/.local/bin…");
    run_step(cfg, &m, UNPACK_SCRIPT, STEP_WAIT, "установка yazi").await?;
    progress("проверяю версию и запоминаю путь…");
    let out = run_step(cfg, &m, VERIFY_SCRIPT, STEP_WAIT, "проверка yazi").await?;
    last_path(&out).ok_or_else(|| format!("на «{label}» проверка yazi не назвала путь"))
}

/// Whether yazi is on the far machine and where, with the system and the ways to install it.
pub async fn check_yazi(cfg: &AppConfig, label: &str) -> Result<YaziState, String> {
    check_with(cfg, &machines::herdr_bin(), label).await
}

pub(crate) async fn check_with(
    cfg: &AppConfig,
    herdr: &str,
    label: &str,
) -> Result<YaziState, String> {
    let m = resolve(herdr, label).await?;
    check_machine(cfg, &m).await
}

/// Opens a local terminal running `yazi` in `worktree` on the far machine (over `ssh -t`). Returns the command for
/// display. It does not wait for the window.
pub async fn open_yazi(cfg: &AppConfig, label: &str, worktree: &str) -> Result<String, String> {
    let env = env_terminal();
    open_yazi_with(cfg, &machines::herdr_bin(), label, worktree, env.as_deref()).await
}

pub(crate) async fn open_yazi_with(
    cfg: &AppConfig,
    herdr: &str,
    label: &str,
    worktree: &str,
    term_env: Option<&str>,
) -> Result<String, String> {
    valid_worktree(worktree)?;
    let m = resolve(herdr, label).await?;
    let st = check_machine(cfg, &m).await?;
    let yazi = st.path.clone().ok_or_else(|| {
        format!("на «{label}» нет yazi: поставьте его (релиз или менеджер пакетов)")
    })?;
    let remote = open_remote(worktree, &yazi);
    let mut cmd = vec![cfg.ssh_bin.clone()];
    cmd.extend(ssh_tty_args(cfg, &m, &remote)?);
    spawn_in_terminal(&cmd, term_env)?;
    Ok(format!(
        "ssh -t {} {}",
        machines::sh_quote(&m.target),
        machines::sh_quote(&remote)
    ))
}

/// The `herdr --remote` argv for a far machine: `herdr --remote <target> [--session <s>]`.
pub fn herdr_remote_argv(herdr: &str, m: &MachineRef) -> Result<Vec<String>, String> {
    if !machines::valid_target(&m.target) {
        return Err(format!("недопустимый адрес машины: {:?}", m.target));
    }
    let mut argv = vec![herdr.to_string(), "--remote".into(), m.target.clone()];
    match m.session.as_str() {
        "" | "default" => {}
        s if valid_session(s) => argv.extend(["--session".to_string(), s.to_string()]),
        s => return Err(format!("недопустимая сессия herdr: {s:?}")),
    }
    Ok(argv)
}

/// The terminal's command for «Экран агента»: a line that names the agent, then `herdr --remote`. `herdr --remote`
/// has no flag that focuses an agent's tab, so the person finds `agent` in the list the remote herdr shows.
pub fn screen_argv(herdr: &str, m: &MachineRef, agent: &str) -> Result<Vec<String>, String> {
    valid_agent(agent)?;
    let remote = herdr_remote_argv(herdr, m)?;
    let note = format!(
        "Агент «{agent}» на галактике «{}»: herdr --remote не открывает вкладку по имени, найдите «{agent}» в списке.",
        m.label
    );
    let mut argv = vec![
        "sh".to_string(),
        "-c".into(),
        "printf '%s\\n' \"$1\"; shift; exec \"$@\"".into(),
        "sh".into(),
        note,
    ];
    argv.extend(remote);
    Ok(argv)
}

/// Opens a local terminal with `herdr --remote` to a far machine, for a board agent. Returns the command for display.
pub async fn open_agent_screen(
    _cfg: &AppConfig,
    label: &str,
    agent_name: &str,
) -> Result<String, String> {
    let env = env_terminal();
    open_agent_screen_with(&machines::herdr_bin(), label, agent_name, env.as_deref()).await
}

pub(crate) async fn open_agent_screen_with(
    herdr: &str,
    label: &str,
    agent_name: &str,
    term_env: Option<&str>,
) -> Result<String, String> {
    valid_agent(agent_name)?;
    let m = resolve(herdr, label).await?;
    let argv = screen_argv(herdr, &m, agent_name)?;
    spawn_in_terminal(&argv, term_env)?;
    Ok(shell_line(&herdr_remote_argv(herdr, &m)?))
}

/// Opens a terminal that runs `sudo pacman -S --needed yazi` on the far machine. The window waits at the end, so
/// the person sees the result. Only for a machine whose pacman carries yazi.
pub async fn open_install_terminal(
    cfg: &AppConfig,
    label: &str,
    method: &str,
) -> Result<String, String> {
    let env = env_terminal();
    open_install_terminal_with(cfg, &machines::herdr_bin(), label, method, env.as_deref()).await
}

pub(crate) async fn open_install_terminal_with(
    cfg: &AppConfig,
    herdr: &str,
    label: &str,
    method: &str,
    term_env: Option<&str>,
) -> Result<String, String> {
    let cmd = terminal_install_command(method)?;
    let m = resolve(herdr, label).await?;
    let st = check_machine(cfg, &m).await?;
    match st.managers.iter().find(|x| x.id == method) {
        Some(mg) if mg.available => {}
        Some(mg) => {
            return Err(format!(
                "на «{label}» pacman не поставит yazi: {}",
                mg.why_not.clone().unwrap_or_default()
            ))
        }
        None => return Err(format!("на «{label}» нет pacman")),
    }
    let inner = format!(
        "{cmd}; r=$?; echo; if [ $r -eq 0 ]; then echo \"yazi установлен: можно закрыть окно\"; else echo \"pacman завершился с кодом $r\"; fi; read -r _"
    );
    let mut argv = vec![cfg.ssh_bin.clone()];
    argv.extend(ssh_tty_args(cfg, &m, &inner)?);
    spawn_in_terminal(&argv, term_env)?;
    Ok(format!(
        "ssh -t {} {}",
        machines::sh_quote(&m.target),
        machines::sh_quote(&inner)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn mref(session: &str) -> MachineRef {
        MachineRef {
            label: "far".into(),
            target: "far.example".into(),
            session: session.into(),
        }
    }

    /// Writes a script and makes it executable.
    fn write_exec(p: &Path, body: &str) {
        crate::testutil::write_exec(p, body);
    }

    /// A far machine under `dir`: a fake herdr (its saved machines), a fake ssh that runs the remote command
    /// locally with this HOME and this PATH only, and the few tools a far machine has here. No yazi, file, brew.
    struct Far {
        dir: PathBuf,
        home: PathBuf,
        bin: PathBuf,
        herdr: String,
        cfg: AppConfig,
    }

    impl Far {
        fn new(dir: &Path) -> Far {
            let home = dir.join("home");
            let bin = dir.join("bin");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&bin).unwrap();
            for t in ["cat", "cp", "mv", "mkdir", "rm", "find", "head", "chmod", "ls"] {
                for d in ["/usr/bin", "/bin"] {
                    let src = Path::new(d).join(t);
                    if src.exists() {
                        std::os::unix::fs::symlink(&src, bin.join(t)).unwrap();
                        break;
                    }
                }
            }
            let d = dir.display().to_string();
            let fill = |s: &str| {
                s.replace("@DIR@", &d)
                    .replace("@HOME@", &home.display().to_string())
                    .replace("@BIN@", &bin.display().to_string())
                    .replace("@FAKESH@", &dir.join("fakesh").display().to_string())
            };
            write_exec(&dir.join("fakesh"), &fill("#!/bin/sh\nshift\nexec /bin/sh -c \"$1\"\n"));
            let ssh = dir.join("ssh");
            write_exec(
                &ssh,
                &fill(
                    "#!/bin/sh\nlast=\"\"\nfor a; do last=$a; done\nexport HOME='@HOME@'\nexport SHELL='@FAKESH@'\nexport PATH='@BIN@'\nexec /bin/sh -c \"$last\"\n",
                ),
            );
            write_exec(&bin.join("uname"), "#!/bin/sh\necho \"Linux x86_64\"\n");
            write_exec(
                &bin.join("curl"),
                &fill("#!/bin/sh\nprev=\"\"; out=\"\"\nfor a; do\n  echo \"$a\" >> '@DIR@/curl.log'\n  if [ \"$prev\" = \"-o\" ]; then out=$a; fi\n  prev=$a\ndone\necho zip > \"$out\"\n"),
            );
            write_exec(
                &bin.join("unzip"),
                &fill("#!/bin/sh\nd=\"\"; prev=\"\"\nfor a; do\n  if [ \"$prev\" = \"-d\" ]; then d=$a; fi\n  prev=$a\ndone\nmkdir -p \"$d/yazi-x86_64-unknown-linux-musl\"\nprintf '#!/bin/sh\\necho \"Yazi 0.0.0-test\"\\n' > \"$d/yazi-x86_64-unknown-linux-musl/yazi\"\nprintf '#!/bin/sh\\necho \"ya 0.0.0-test\"\\n' > \"$d/yazi-x86_64-unknown-linux-musl/ya\"\n"),
            );
            write_exec(
                &dir.join("herdr"),
                &fill("#!/bin/sh\ncat '@DIR@/saved.json'\n"),
            );
            std::fs::write(
                dir.join("saved.json"),
                r#"[{"id":"m1","label":"far","ssh_target":"far.example","remote_session":""}]"#,
            )
            .unwrap();
            let cfg = AppConfig {
                ssh_bin: ssh.display().to_string(),
                ..machines::test_cfg(dir)
            };
            Far {
                dir: dir.to_path_buf(),
                home,
                bin,
                herdr: dir.join("herdr").display().to_string(),
                cfg,
            }
        }

        /// A yazi on the far machine's `~/.local/bin`, which prints its version and writes `$HOME/ran.txt`.
        fn seed_yazi(&self) -> PathBuf {
            let p = self.home.join(".local/bin/yazi");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            write_exec(
                &p,
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo \"Yazi 0.0.0-test\"; exit 0; fi\npwd > \"$HOME/ran.txt\"\n",
            );
            p
        }

        fn add_tool(&self, name: &str, body: &str) {
            write_exec(&self.bin.join(name), &format!("#!/bin/sh\n{body}\n"));
        }

        /// A terminal stand-in: writes its argv, one per line, to `term.txt`.
        fn terminal(&self) -> PathBuf {
            let t = self.dir.join("term");
            write_exec(
                &t,
                &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", self.dir.join("term.txt").display()),
            );
            t
        }
    }

    async fn wait_for(p: &Path) -> String {
        for _ in 0..200 {
            if let Ok(s) = std::fs::read_to_string(p) {
                if s.ends_with('\n') {
                    return s;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("terminal never ran: {}", p.display());
    }

    #[test]
    fn the_tty_ssh_is_the_boards_form_without_batch_mode() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = machines::test_cfg(dir.path());
        let args = ssh_tty_args(&cfg, &mref(""), "cd /w && exec yazi").unwrap();
        assert_eq!(args[0], "-t");
        assert!(!args.iter().any(|a| a == "BatchMode=yes" || a == "BatchMode"));
        assert!(args.contains(&"ConnectTimeout=6".to_string()));
        assert!(args.contains(&"ControlMaster=auto".to_string()));
        assert!(args.iter().any(|a| a.starts_with("ControlPath=")));
        assert!(args.iter().any(|a| a.starts_with("UserKnownHostsFile=")));
        let n = args.len();
        assert_eq!(args[n - 2], "far.example");
        assert_eq!(
            args[n - 1],
            "exec \"${SHELL:-/bin/sh}\" -lc 'cd /w && exec yazi'"
        );
        // Every option still has its value: the list is pairs after `-t`.
        assert!(args[1..n - 2].chunks(2).all(|p| p[0] == "-o"));
    }

    #[test]
    fn the_terminal_is_xdg_on_linux_and_the_env_overrides_it() {
        let cmd = vec!["ssh".to_string(), "-t".to_string(), "far".to_string()];
        assert_eq!(
            terminal_argv("linux", None, None, &cmd),
            ["xdg-terminal-exec", "ssh", "-t", "far"]
        );
        assert_eq!(
            terminal_argv("linux", Some("  "), None, &cmd)[0],
            "xdg-terminal-exec"
        );
        assert_eq!(
            terminal_argv("linux", Some("/opt/term"), None, &cmd)[0],
            "/opt/term"
        );
    }

    #[test]
    fn the_mac_terminals_follow_the_mockup() {
        let cmd = vec!["ssh".to_string(), "far".to_string()];
        assert_eq!(
            terminal_argv("macos", None, Some("kitty"), &cmd),
            ["open", "-na", "kitty", "--args", "ssh", "far"]
        );
        assert_eq!(
            terminal_argv("macos", None, Some("Ghostty"), &cmd),
            ["open", "-na", "Ghostty", "--args", "-e", "ssh", "far"]
        );
        assert_eq!(
            terminal_argv("macos", None, Some("WezTerm"), &cmd),
            ["open", "-na", "WezTerm", "--args", "start", "--", "ssh", "far"]
        );
        let v = terminal_argv("macos", None, Some("iTerm2"), &cmd);
        assert_eq!(v[..2], ["osascript", "-e"]);
        assert!(v[2].contains("create window with default profile command \"ssh far\""));
        let v = terminal_argv("macos", None, None, &cmd);
        assert!(v[2].starts_with("tell application \"Terminal\" to do script"));
    }

    #[test]
    fn a_quote_in_the_command_survives_the_applescript() {
        let cmd = vec!["ssh".to_string(), "a\"b c".to_string()];
        let v = terminal_argv("macos", None, Some("iTerm2"), &cmd);
        assert!(v[2].contains("\"ssh 'a\\\"b c'\""), "{}", v[2]);
    }

    #[test]
    fn the_screen_is_herdr_remote_with_the_session_only_when_named() {
        let argv = herdr_remote_argv("herdr", &mref("work")).unwrap();
        assert_eq!(argv, ["herdr", "--remote", "far.example", "--session", "work"]);
        let argv = herdr_remote_argv("herdr", &mref("default")).unwrap();
        assert_eq!(argv, ["herdr", "--remote", "far.example"]);
        assert!(herdr_remote_argv("herdr", &mref("a b")).is_err());
        let bad = MachineRef {
            target: "-oProxyCommand=x".into(),
            ..mref("")
        };
        assert!(herdr_remote_argv("herdr", &bad).is_err());
    }

    #[test]
    fn the_screen_names_the_agent_and_refuses_a_bad_name() {
        let argv = screen_argv("herdr", &mref("work"), "sw-ela-3").unwrap();
        assert_eq!(argv[..4], ["sh", "-c", "printf '%s\\n' \"$1\"; shift; exec \"$@\"", "sh"]);
        assert!(argv[4].contains("«sw-ela-3»"));
        assert_eq!(argv[5..], ["herdr", "--remote", "far.example", "--session", "work"]);
        assert!(screen_argv("herdr", &mref(""), "x'; rm -rf ~").is_err());
        assert!(screen_argv("herdr", &mref(""), "").is_err());
    }

    #[test]
    fn a_worktree_is_quoted_for_the_remote_shell() {
        let cmd = open_remote("/srv/it's here", "/home/u/.local/bin/yazi");
        assert!(cmd.starts_with("cd '/srv/it'\\''s here' && exec /home/u/.local/bin/yazi"), "{cmd}");
        assert!(valid_worktree("relative").is_err());
        assert!(valid_worktree("/a\nb").is_err());
        assert!(valid_worktree("/a\0b").is_err());
        assert!(valid_worktree("/a b/c").is_ok());
    }

    #[test]
    fn the_release_assets_are_the_published_names() {
        assert_eq!(
            asset_name("linux", "x86_64").unwrap(),
            "yazi-x86_64-unknown-linux-musl.zip"
        );
        assert_eq!(
            asset_name("linux", "aarch64").unwrap(),
            "yazi-aarch64-unknown-linux-musl.zip"
        );
        assert_eq!(asset_name("macos", "aarch64").unwrap(), "yazi-aarch64-apple-darwin.zip");
        assert_eq!(asset_name("macos", "x86_64").unwrap(), "yazi-x86_64-apple-darwin.zip");
        assert!(asset_name("unknown", "x86_64").is_none());
        assert!(asset_name("linux", "armv7l").is_none());
    }

    #[test]
    fn the_check_reads_the_system_the_tools_and_the_managers() {
        let st = parse_check(
            "Welcome to the login shell\nyazi=\nuname=Linux x86_64\ntool=file\ntool=pacman\npacman_yazi=1\n",
        );
        assert!(!st.present);
        assert_eq!((st.os.as_str(), st.arch.as_str()), ("linux", "x86_64"));
        assert!(st.has_file && !st.has_chafa);
        assert_eq!(
            st.binary_url.as_deref(),
            Some("https://github.com/sxyazi/yazi/releases/latest/download/yazi-x86_64-unknown-linux-musl.zip")
        );
        assert_eq!(st.managers.len(), 1);
        let pm = &st.managers[0];
        assert_eq!((pm.id, pm.sudo, pm.available), ("pacman", true, true));
        assert_eq!(pm.command, "sudo pacman -S --needed yazi");
    }

    #[test]
    fn the_debian_case_says_why_apt_is_off() {
        let st = parse_check("yazi=/home/u/.local/bin/yazi\nuname=Linux aarch64\ntool=apt-get\ntool=chafa\n");
        assert!(st.present);
        assert_eq!(st.path.as_deref(), Some("/home/u/.local/bin/yazi"));
        assert!(st.has_chafa && !st.has_file);
        let apt = &st.managers[0];
        assert_eq!(apt.id, "apt");
        assert!(!apt.available);
        assert!(apt.why_not.as_deref().unwrap().contains("Ubuntu/Debian"));
    }

    #[test]
    fn a_mac_gets_brew_and_the_darwin_release() {
        let st = parse_check("uname=Darwin arm64\ntool=brew\nbrew_yazi=1\n");
        assert_eq!((st.os.as_str(), st.arch.as_str()), ("macos", "aarch64"));
        assert!(st.binary_url.unwrap().ends_with("/yazi-aarch64-apple-darwin.zip"));
        assert_eq!(st.managers[0].id, "brew");
        assert!(st.managers[0].available);
        assert!(!st.has_file);
    }

    #[test]
    fn an_unknown_system_has_no_release_and_says_so() {
        let st = parse_check("uname=FreeBSD amd64\n");
        assert_eq!(st.os, "unknown");
        assert!(st.binary_url.is_none());
        assert!(st.notes.iter().any(|n| n.contains("FreeBSD amd64")));
    }

    #[test]
    fn a_relative_or_missing_path_is_not_yazi() {
        let st = parse_check("yazi=yazi\nuname=Linux x86_64\n");
        assert!(!st.present && st.path.is_none());
    }

    #[test]
    fn a_name_for_an_agent_is_the_board_s_name() {
        assert!(valid_agent("sw-ela-3").is_ok());
        assert!(valid_agent("sw ela").is_err());
        assert!(valid_agent("sw/ela").is_err());
    }

    #[test]
    fn the_pacman_command_is_the_one_the_person_types() {
        assert_eq!(terminal_install_command("pacman").unwrap(), "sudo pacman -S --needed yazi");
        assert!(terminal_install_command("brew").is_err());
    }

    #[tokio::test]
    async fn the_check_on_a_far_machine_without_yazi() {
        let dir = tempfile::tempdir().unwrap();
        let far = Far::new(dir.path());
        let st = check_with(&far.cfg, &far.herdr, "far").await.unwrap();
        assert!(!st.present);
        assert_eq!((st.os.as_str(), st.arch.as_str()), ("linux", "x86_64"));
        assert!(!st.has_file);
        assert!(st.managers.is_empty());
    }

    #[tokio::test]
    async fn the_check_finds_yazi_in_local_bin_and_the_remembered_path() {
        let dir = tempfile::tempdir().unwrap();
        let far = Far::new(dir.path());
        let p = far.seed_yazi();
        let st = check_with(&far.cfg, &far.herdr, "far").await.unwrap();
        assert!(st.present);
        assert_eq!(st.path.as_deref(), Some(p.to_str().unwrap()));
        // A remembered install somewhere else wins over nothing on PATH, and is the one checked.
        let other = far.home.join("opt/yazi");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        write_exec(&other, "#!/bin/sh\necho y\n");
        std::fs::remove_file(&p).unwrap();
        std::fs::create_dir_all(far.home.join(".local/share/herdr-planner-far")).unwrap();
        std::fs::write(
            far.home.join(".local/share/herdr-planner-far/yazi-path"),
            format!("{}\n", other.display()),
        )
        .unwrap();
        let st = check_with(&far.cfg, &far.herdr, "far").await.unwrap();
        assert_eq!(st.path.as_deref(), Some(other.to_str().unwrap()));
    }

    #[tokio::test]
    async fn a_binary_install_lands_in_local_bin_and_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let far = Far::new(dir.path());
        let log = Mutex::new(Vec::<String>::new());
        let progress = |s: &str| log.lock().unwrap().push(s.to_string());
        let path = install_with(&far.cfg, &far.herdr, "far", "binary", &progress)
            .await
            .unwrap();
        let want = far.home.join(".local/bin/yazi");
        assert_eq!(path, want.display().to_string());
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(std::fs::metadata(&want).unwrap().permissions().mode() & 0o111, 0);
        assert!(far.home.join(".local/bin/ya").exists());
        let remembered =
            std::fs::read_to_string(far.home.join(".local/share/herdr-planner-far/yazi-path")).unwrap();
        assert_eq!(remembered.trim(), path);
        assert!(!far.home.join(".cache/herdr-planner-far/yazi-dl").exists());
        let curl = std::fs::read_to_string(far.dir.join("curl.log")).unwrap();
        assert!(curl.contains(
            "https://github.com/sxyazi/yazi/releases/latest/download/yazi-x86_64-unknown-linux-musl.zip"
        ));
        let steps = log.into_inner().unwrap();
        assert!(steps.iter().any(|l| l.contains("скачиваю yazi-x86_64")), "{steps:?}");
        assert!(steps.len() >= 4, "{steps:?}");
        let st = check_with(&far.cfg, &far.herdr, "far").await.unwrap();
        assert_eq!(st.path.as_deref(), Some(path.as_str()));
    }

    #[tokio::test]
    async fn pacman_is_never_run_from_here_and_unknown_methods_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = machines::test_cfg(dir.path());
        let none = |_: &str| {};
        let err = install_with(&cfg, "/nonexistent/herdr", "far", "pacman", &none)
            .await
            .unwrap_err();
        assert!(err.contains("sudo pacman -S --needed yazi"), "{err}");
        let err = install_with(&cfg, "/nonexistent/herdr", "far", "rm -rf", &none)
            .await
            .unwrap_err();
        assert!(err.contains("неизвестный"), "{err}");
    }

    #[tokio::test]
    async fn the_pacman_terminal_has_a_tty_and_only_runs_where_pacman_has_yazi() {
        let dir = tempfile::tempdir().unwrap();
        let far = Far::new(dir.path());
        let term = far.terminal();
        let no = open_install_terminal_with(&far.cfg, &far.herdr, "far", "pacman", Some(term.to_str().unwrap()))
            .await
            .unwrap_err();
        assert!(no.contains("pacman"), "{no}");
        far.add_tool("pacman", "exit 0");
        let shown = open_install_terminal_with(&far.cfg, &far.herdr, "far", "pacman", Some(term.to_str().unwrap()))
            .await
            .unwrap();
        assert!(shown.contains("sudo pacman -S --needed yazi"), "{shown}");
        let argv = wait_for(&far.dir.join("term.txt")).await;
        let lines: Vec<&str> = argv.lines().collect();
        assert!(lines.contains(&"-t"));
        assert!(!argv.contains("BatchMode"));
        assert!(lines.last().unwrap().contains("sudo pacman -S --needed yazi"));
    }

    #[tokio::test]
    async fn open_yazi_runs_yazi_in_the_worktree_and_opens_a_tty_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let far = Far::new(dir.path());
        let yazi = far.seed_yazi();
        let wt = dir.path().join("my wt");
        std::fs::create_dir_all(&wt).unwrap();
        let term = far.terminal();
        let shown = open_yazi_with(
            &far.cfg,
            &far.herdr,
            "far",
            wt.to_str().unwrap(),
            Some(term.to_str().unwrap()),
        )
        .await
        .unwrap();
        assert!(shown.starts_with("ssh -t far.example "), "{shown}");
        let argv = wait_for(&far.dir.join("term.txt")).await;
        assert!(argv.lines().any(|l| l == "-t"));
        assert!(!argv.contains("BatchMode"));
        // The same remote command, run through the fake ssh, lands in the worktree with yazi.
        let m = mref("");
        let remote = open_remote(wt.to_str().unwrap(), yazi.to_str().unwrap());
        let end = machines::ssh_ended(&far.cfg, &m, &remote, STEP_WAIT).await.unwrap();
        assert_eq!(end.code, Some(0), "{}", end.stderr);
        let ran = std::fs::read_to_string(far.home.join("ran.txt")).unwrap();
        assert_eq!(ran.trim(), wt.to_str().unwrap());
    }

    #[tokio::test]
    async fn open_yazi_refuses_a_relative_worktree_before_any_call() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = machines::test_cfg(dir.path());
        let err = open_yazi_with(&cfg, "/nonexistent/herdr", "far", "relative", None)
            .await
            .unwrap_err();
        assert!(err.contains("недопустимая папка"), "{err}");
    }

    #[tokio::test]
    async fn the_agent_screen_opens_herdr_remote_in_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let far = Far::new(dir.path());
        let term = far.terminal();
        // The saved machine has no session, so the screen is the default one: no --session.
        let shown = open_agent_screen_with(&far.herdr, "far", "sw-ela-3", Some(term.to_str().unwrap()))
            .await
            .unwrap();
        assert!(shown.ends_with(" --remote far.example"), "{shown}");
        let argv = wait_for(&far.dir.join("term.txt")).await;
        let lines: Vec<&str> = argv.lines().collect();
        assert!(lines.iter().any(|l| l.contains("«sw-ela-3»")));
        assert_eq!(lines[lines.len() - 2..], ["--remote", "far.example"]);
        assert!(!argv.contains("--session"));
        assert!(open_agent_screen_with(&far.herdr, "bad label!", "sw-ela-3", None).await.is_err());
    }
}
