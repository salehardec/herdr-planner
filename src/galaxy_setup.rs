//! Adding a galaxy from SSH alone (PLN-9, section C): what a machine has, the steps that make it a galaxy, the logins
//! of its agents, and the git repositories in its Projects folder.
//!
//! Every call reaches the machine the board's way (`machines::ssh_args`: BatchMode, the board's known hosts, a login
//! shell). Two things are not the board's way. `herdr machine add` asks for the install in an interactive terminal, so
//! it runs in a pseudo-terminal (`run_in_pty`) and its question is answered `y`: the person confirmed the step in the
//! page. And an agent's login opens a terminal window, for a client at this machine only. Nothing here stores or asks
//! for ssh keys or passwords.

use crate::api::ask_galaxy;
use crate::app::{self, Shared};
use crate::config::{now_ms, AppConfig};
use crate::farfiles;
use crate::machines::{self, Ended, MachineRef, Why};
use crate::server::{client_is_local, clip, error, same_origin};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::timeout;

const PROBE_WAIT: Duration = Duration::from_secs(25);
const CHECK_WAIT: Duration = Duration::from_secs(25);
const STEP_WAIT: Duration = Duration::from_secs(60);
/// `herdr machine add`, a plugin install and an agent's installer can take minutes on a slow link.
const LONG_WAIT: Duration = Duration::from_secs(900);
const HOME_WAIT: Duration = Duration::from_secs(10);
/// The questions a pseudo-terminal run answers `y` to before it gives up (a run that asks more is not what we expect).
const MAX_ANSWERS: usize = 3;
const PLUGIN_REPO: &str = "eliasstravik/herdr-projects";
/// The step names the page sends.
const STEPS: &[&str] = &["herdr", "plugin", "agent-install", "agent-login", "agent-check"];
/// The programs of the crew (the station's list): probed, checked and logged in to.
pub const PROGRAMS: &[&str] = &[
    "claude", "codex", "omp", "pi", "opencode", "agy", "copilot", "cursor-agent", "droid", "grok", "devin", "gemini",
];
/// The text marker of the probe's answer: the first line of a probe that worked.
const PROBE_MARK: &str = "herdr-planner-probe 1";

/// One agent program the planner can install: the tools its installer needs and the command the machine runs.
/// The commands are the programs' own installers as far as the planner knows them; the ones marked unverified in the
/// report were not run against a real machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installer {
    pub program: &'static str,
    pub needs: &'static [&'static str],
    pub command: &'static str,
}

pub const INSTALLERS: &[Installer] = &[
    // Anthropic's native installer (claude.ai/install.sh); needs curl.
    Installer { program: "claude", needs: &["curl"], command: "curl -fsSL https://claude.ai/install.sh | bash" },
    Installer { program: "codex", needs: &["npm"], command: "npm install -g @openai/codex" },
    // UNVERIFIED package name for oh-my-pi (`omp`); check it against the project before relying on it.
    Installer { program: "omp", needs: &["npm"], command: "npm install -g @oh-my-pi/pi-coding-agent" },
    Installer { program: "pi", needs: &["npm"], command: "npm install -g @mariozechner/pi-coding-agent" },
    Installer { program: "opencode", needs: &["npm"], command: "npm install -g opencode-ai" },
];

pub(crate) fn installer_for(program: &str) -> Option<&'static Installer> {
    INSTALLERS.iter().find(|i| i.program == program)
}

/// The command that logs an agent in, run in a terminal over `ssh -t`: `claude auth login`, `codex login`, else
/// `<program> login`.
pub(crate) fn login_command(program: &str) -> String {
    match program {
        "claude" => "claude auth login".to_string(),
        "codex" => "codex login".to_string(),
        other => format!("{other} login"),
    }
}

pub(crate) fn valid_step(step: &str) -> bool {
    STEPS.contains(&step)
}

pub(crate) fn valid_program(program: &str) -> bool {
    PROGRAMS.contains(&program)
}

/// A git commit as `herdr plugin list` shows it (`@<sha>`): 7 to 40 hex digits.
pub(crate) fn valid_ref(r: &str) -> bool {
    (7..=40).contains(&r.len()) && r.chars().all(|c| c.is_ascii_hexdigit())
}

/// The plugin install line for the far machine. The ref is checked, so it needs no quoting.
pub(crate) fn plugin_install_command(home_ref: Option<&str>) -> Result<String, String> {
    let mut cmd = format!("herdr plugin install {PLUGIN_REPO}");
    if let Some(r) = home_ref {
        if !valid_ref(r) {
            return Err(format!("недопустимая ссылка плагина: {r:?}"));
        }
        cmd.push_str(&format!(" --ref {r}"));
    }
    cmd.push_str(" -y");
    Ok(cmd)
}

/* ───────── the probe ───────── */

/// Read-only, one call. Every line is `key<TAB>value` (or `tool`/`program` with more fields), so a login shell's
/// greeting before it is skipped. Nothing here changes the machine.
const PROBE_SCRIPT: &str = r#"echo herdr-planner-probe 1
printf 'os\t%s\n' "$(. /etc/os-release 2>/dev/null; printf '%s' "$PRETTY_NAME")"
printf 'kernel\t%s\n' "$(uname -s 2>/dev/null)"
printf 'arch\t%s\n' "$(uname -m 2>/dev/null)"
printf 'projects\t%s\n' "$HOME/Projects"
for t in git node npm curl; do
  command -v "$t" >/dev/null 2>&1 && printf 'tool\t%s\t%s\n' "$t" "$("$t" --version 2>/dev/null </dev/null | head -n 1)"
done
command -v herdr >/dev/null 2>&1 && printf 'herdr\t%s\n' "$(herdr --version 2>/dev/null </dev/null | head -n 1)"
command -v herdr-projects >/dev/null 2>&1 && printf 'board\t%s\n' "$(herdr-projects version 2>/dev/null </dev/null | head -n 1)"
for p in claude codex omp pi opencode agy copilot cursor-agent droid grok devin gemini; do
  b=$(command -v "$p" 2>/dev/null || true)
  [ -n "$b" ] || { [ -x "$HOME/.local/bin/$p" ] && b="$HOME/.local/bin/$p"; }
  [ -n "$b" ] && printf 'program\t%s\t%s\n' "$p" "$("$b" --version 2>/dev/null </dev/null | head -n 1)"
done
exit 0"#;

/// What the probe found on the far machine, before the home board's facts are added.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FarFacts {
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub projects_dir: String,
    /// `(tool, first line of its --version)` for the tools that are there.
    pub tools: Vec<(String, String)>,
    /// The first line of `herdr --version`, when herdr is there.
    pub herdr: Option<String>,
    /// The first line of `herdr-projects version`, when the plugin is there.
    pub board: Option<String>,
    /// `(program, first line of its --version)` for the agents that are there.
    pub programs: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ToolsState {
    pub git: Option<String>,
    pub node: Option<String>,
    pub npm: Option<String>,
    pub curl: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HerdrState {
    pub installed: bool,
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BoardState {
    pub installed: bool,
    pub version: String,
    /// The home board's version (`herdr-projects version` here).
    pub home_version: String,
    /// The commit the home plugin was installed from (`@<sha>` in `herdr plugin list`), when known.
    pub home_ref: Option<String>,
    /// The far plugin is installed and has the home version.
    pub same: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProgramState {
    pub name: String,
    pub installed: bool,
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Probe {
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub supported: bool,
    pub tools: ToolsState,
    pub herdr: HerdrState,
    pub projects: BoardState,
    pub programs: Vec<ProgramState>,
    pub projects_dir: String,
}

/// The first version number in a line (`herdr-projects 1.2.4 (abc)` → `1.2.4`): dotted, digits only.
pub(crate) fn version_token(line: &str) -> Option<String> {
    line.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';'))
        .find(|t| {
            let parts: Vec<&str> = t.split('.').collect();
            parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
        .map(str::to_string)
}

/// The version a tool prints, as the planner shows it: git's last word, curl's second word, node's and npm's line.
fn tool_version(name: &str, line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    match name {
        "git" => words.last().map(|w| w.to_string()).unwrap_or_default(),
        "curl" => words.get(1).map(|w| w.to_string()).unwrap_or_default(),
        _ => line.trim().to_string(),
    }
}

/// Reads the probe's answer. Lines it does not know are skipped; the marker must be there, or the answer is not the
/// probe's (a shell that printed something else, a board of another version).
pub(crate) fn parse_probe(out: &str) -> Result<FarFacts, String> {
    if !out.lines().any(|l| l.contains(PROBE_MARK)) {
        return Err("на машине нет ответа планировщика: вход в оболочку печатает что-то своё или скрипт не запустился".into());
    }
    let mut f = FarFacts::default();
    for line in out.lines().map(|l| l.trim_end_matches('\r')) {
        let Some((key, rest)) = line.split_once('\t') else {
            continue;
        };
        match key {
            "os" => f.os = rest.trim().to_string(),
            "kernel" => f.kernel = rest.trim().to_string(),
            "arch" => f.arch = rest.trim().to_string(),
            "projects" => f.projects_dir = rest.trim().to_string(),
            "herdr" => f.herdr = Some(rest.trim().to_string()),
            "board" => f.board = Some(rest.trim().to_string()),
            "tool" => {
                let (name, ver) = rest.split_once('\t').unwrap_or((rest, ""));
                f.tools.push((name.trim().to_string(), ver.trim().to_string()));
            }
            "program" => {
                let (name, ver) = rest.split_once('\t').unwrap_or((rest, ""));
                f.programs.push((name.trim().to_string(), ver.trim().to_string()));
            }
            _ => {}
        }
    }
    Ok(f)
}

/// Whether a machine can take a galaxy: Linux or macOS on a 64-bit processor the herdr release has.
pub(crate) fn supported(kernel: &str, arch: &str) -> bool {
    matches!(kernel, "Linux" | "Darwin") && matches!(arch, "x86_64" | "aarch64" | "arm64")
}

impl FarFacts {
    /// The page's answer: the far facts with the home board's version and commit.
    pub(crate) fn into_probe(self, home_version: &str, home_ref: Option<String>) -> Probe {
        let tool = |name: &str| -> Option<String> {
            self.tools
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| tool_version(name, v))
        };
        let board_version = self.board.as_deref().and_then(version_token).unwrap_or_default();
        let installed = self.board.is_some();
        let same = installed && !board_version.is_empty() && board_version == home_version;
        let programs = PROGRAMS
            .iter()
            .map(|p| {
                let found = self.programs.iter().find(|(n, _)| n == p);
                ProgramState {
                    name: p.to_string(),
                    installed: found.is_some(),
                    version: found.map(|(_, v)| v.clone()).unwrap_or_default(),
                }
            })
            .collect();
        Probe {
            supported: supported(&self.kernel, &self.arch),
            tools: ToolsState {
                git: tool("git"),
                node: tool("node"),
                npm: tool("npm"),
                curl: tool("curl"),
            },
            herdr: HerdrState {
                installed: self.herdr.is_some(),
                version: self.herdr.as_deref().and_then(version_token).unwrap_or_default(),
            },
            projects: BoardState {
                installed,
                version: board_version,
                home_version: home_version.to_string(),
                home_ref,
                same,
            },
            programs,
            projects_dir: self.projects_dir,
            os: self.os,
            kernel: self.kernel,
            arch: self.arch,
        }
    }
}

/// The version of the planner's own board, from `herdr-projects version` (its first line).
pub(crate) fn board_version(out: &str) -> Option<String> {
    out.lines().next().and_then(version_token)
}

/// The commit a plugin was installed from: the first `@<hex>` (7 to 40 digits) on a `herdr-projects` line.
pub(crate) fn plugin_ref(list: &str) -> Option<String> {
    list.lines()
        .filter(|l| l.contains("herdr-projects"))
        .find_map(|l| {
            l.split_whitespace().find_map(|tok| {
                let at = tok.find('@')?;
                let hex: String = tok[at + 1..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
                (hex.len() >= 7).then_some(hex)
            })
        })
}

/// The home board's version and the commit its herdr-projects plugin was installed from.
async fn home_facts(cfg: &AppConfig, herdr: &str) -> (String, Option<String>) {
    let version = app::run_board(cfg, &["version"], HOME_WAIT)
        .await
        .ok()
        .and_then(|o| board_version(&o))
        .unwrap_or_default();
    let home_ref = herdr_out(herdr, &["plugin", "list"], HOME_WAIT)
        .await
        .and_then(|o| plugin_ref(&o));
    (version, home_ref)
}

/// Runs herdr for a short answer (no pseudo-terminal): its stdout when it succeeds.
async fn herdr_out(herdr: &str, args: &[&str], wait: Duration) -> Option<String> {
    let mut cmd = Command::new(herdr);
    cmd.args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match timeout(wait, cmd.output()).await {
        Ok(Ok(o)) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
        _ => None,
    }
}

/// The plain-text reason for a failed call: the line, then what to do.
fn plain(why: &Why) -> String {
    let parts: Vec<&str> = std::iter::once(why.line.as_str())
        .chain(why.detail.iter().map(String::as_str))
        .map(|p| p.trim().trim_end_matches('.'))
        .filter(|p| !p.is_empty())
        .collect();
    clip(format!("{}.", parts.join(". ")))
}

#[derive(Deserialize)]
pub struct ProbeBody {
    target: String,
    label: Option<String>,
}

/// `POST /api/galaxy/probe {target, label?}`: what the machine has, read over one ssh call. Nothing is changed.
pub async fn probe(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<ProbeBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let target = body.target.trim().to_string();
    if !machines::valid_target(&target) {
        return error(StatusCode::BAD_REQUEST, format!("адрес машины не похож на ssh-адрес (как в ~/.ssh/config): {target:?}"));
    }
    let label = body
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| machines::short_label(&target));
    if !machines::valid_label(&label) {
        return error(StatusCode::BAD_REQUEST, format!("метка галактики: буквы, цифры, «-», «_», «.»: {label:?}"));
    }
    let m = MachineRef {
        label: label.clone(),
        target: target.clone(),
        session: String::new(),
    };
    let herdr = machines::herdr_bin();
    let saved = machines::saved_machines_with(&herdr).await.iter().any(|x| x.label == label);
    let end = match machines::ssh_ended(&app.cfg, &m, PROBE_SCRIPT, PROBE_WAIT).await {
        Ok(e) => e,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    if end.timed_out || !end.error.is_empty() || end.code != Some(0) {
        let why = machines::why_not(&end, &m);
        return Json(json!({"ok": false, "step": "ssh", "error": plain(&why), "why": why})).into_response();
    }
    let far = match parse_probe(&end.stdout) {
        Ok(f) => f,
        Err(msg) => {
            let why = Why {
                line: "ответила не то, что ждёт планировщик".into(),
                detail: vec![msg],
            };
            return Json(json!({"ok": false, "step": "probe", "error": plain(&why), "why": why})).into_response();
        }
    };
    let (home_version, home_ref) = home_facts(&app.cfg, &herdr).await;
    let probe = far.into_probe(&home_version, home_ref);
    let mut out = serde_json::to_value(&probe).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut out {
        o.insert("ok".into(), json!(true));
        o.insert("target".into(), json!(target));
        o.insert("label".into(), json!(label));
        o.insert("saved".into(), json!(saved));
    }
    Json(out).into_response()
}

/* ───────── progress ───────── */

#[derive(Debug, Clone, Serialize)]
struct Row {
    id: &'static str,
    label: &'static str,
    /// `wait`, `run`, `ok` or `err`.
    state: &'static str,
    detail: String,
    percent: Option<u8>,
}

/// The steps of one job, sent on the bus as `progress` events like `universe_add`'s.
struct Progress {
    app: Shared,
    job: String,
    rows: Vec<Row>,
}

impl Progress {
    fn new(app: &Shared, job: &str, steps: &[(&'static str, &'static str)]) -> Progress {
        let rows = steps
            .iter()
            .map(|&(id, label)| Row {
                id,
                label,
                state: "wait",
                detail: String::new(),
                percent: None,
            })
            .collect();
        let p = Progress {
            app: app.clone(),
            job: job.to_string(),
            rows,
        };
        p.emit(json!({}));
        p
    }

    fn emit(&self, extra: Value) {
        let mut v = json!({"job": self.job, "steps": self.rows, "done": false});
        if let (Value::Object(base), Value::Object(more)) = (&mut v, extra) {
            base.extend(more);
        }
        self.app.bus.send("progress", v.to_string());
    }

    fn set(&mut self, i: usize, state: &'static str, detail: impl Into<String>) {
        let r = &mut self.rows[i];
        r.state = state;
        r.detail = detail.into();
        self.emit(json!({}));
    }

    fn run(&mut self, i: usize, detail: impl Into<String>) {
        self.set(i, "run", detail);
    }

    fn ok(&mut self, i: usize, detail: impl Into<String>) {
        self.set(i, "ok", detail);
    }

    /// Marks a step failed and returns its message, for `return Err(p.err(i, …))`.
    fn err(&mut self, i: usize, msg: impl Into<String>) -> String {
        let msg = clip(msg.into());
        self.set(i, "err", msg.clone());
        msg
    }

    /// The last word of a streamed line goes to the step's detail while it runs.
    fn live(&mut self, i: usize, line: &str) {
        let r = &mut self.rows[i];
        r.detail = clip(line.to_string());
        self.emit(json!({}));
    }

    /// The final event: the result, or the error. Every failure can be retried (`retry`).
    fn finish(&self, r: Result<Value, String>) {
        let tail = match r {
            Ok(result) => json!({"ok": true, "result": result, "retry": true}),
            Err(e) => json!({"ok": false, "error": e, "retry": true}),
        };
        let mut v = json!({"job": self.job, "steps": self.rows, "done": true});
        if let (Value::Object(base), Value::Object(more)) = (&mut v, tail) {
            base.extend(more);
        }
        self.app.bus.send("progress", v.to_string());
    }
}

/// The text of a failed ssh call, as the other steps say it.
fn ssh_fail(what: &str, end: &Ended, m: &MachineRef) -> String {
    if end.timed_out || !end.error.is_empty() || end.code == Some(255) {
        return machines::why_not(end, m).line;
    }
    farfiles::step_error(what, end, m)
}

/* ───────── the pseudo-terminal run ───────── */

/// What a pseudo-terminal run ended with.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PtyEnd {
    pub code: Option<i32>,
    /// How many yes/no questions were answered `y`.
    pub answers: usize,
    /// The last part of what the program printed, without the terminal's escape codes.
    pub tail: String,
    pub timed_out: bool,
    /// What went wrong before or around the run (no `script`, too many questions); empty otherwise.
    pub error: String,
}

/// The argv that runs `argv` in a pseudo-terminal. Linux: util-linux `script` with `-e` (its exit code is the
/// program's) and `-c` (one shell line). macOS: BSD `script` takes the program directly.
pub(crate) fn pty_argv(os: &str, argv: &[String]) -> Vec<String> {
    if os == "macos" {
        let mut v = vec!["script".to_string(), "-q".into(), "/dev/null".into()];
        v.extend(argv.iter().cloned());
        v
    } else {
        vec![
            "script".to_string(),
            "-qefc".into(),
            farfiles::shell_line(argv),
            "/dev/null".into(),
        ]
    }
}

/// Whether printed text ends in a yes/no question: `[y/N]`, `(y/n)`, `[Y/n]`, `(yes/no)`, in any case.
pub(crate) fn is_confirm_prompt(text: &str) -> bool {
    let t = text.to_lowercase();
    ["[y/n", "(y/n", "[yes/no", "(yes/no"].iter().any(|p| t.contains(p))
}

/// Takes out the terminal's escape sequences and the control characters a line does not show (CR, bell, backspace).
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            match it.peek() {
                Some('[') => {
                    it.next();
                    // CSI: parameters, then one final byte in 0x40..=0x7e.
                    for d in it.by_ref() {
                        if ('\x40'..='\x7e').contains(&d) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    it.next();
                    for d in it.by_ref() {
                        if d == '\x07' {
                            break;
                        }
                    }
                }
                Some(_) => {
                    it.next();
                }
                None => {}
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// Keeps the last `max` bytes of `s` (on a character boundary), once it has grown past twice that.
fn push_capped(s: &mut String, add: &str, max: usize) {
    s.push_str(add);
    if s.len() > max * 2 {
        let mut cut = s.len() - max;
        while !s.is_char_boundary(cut) {
            cut += 1;
        }
        s.drain(..cut);
    }
}

/// Runs `argv` in a pseudo-terminal with its stdin piped. Every complete output line goes to `on_line`. When the
/// output ends in a yes/no question, `y` is written to the program (at most `MAX_ANSWERS` times). The run ends when
/// the program exits, or after `wait`.
pub(crate) async fn run_in_pty(
    argv: &[String],
    wait: Duration,
    on_line: &mut (dyn FnMut(&str) + Send),
) -> PtyEnd {
    let full = pty_argv(std::env::consts::OS, argv);
    let mut cmd = Command::new(&full[0]);
    cmd.args(&full[1..])
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return PtyEnd {
                error: format!(
                    "нет программы «{}»: herdr machine add спрашивает подтверждение в терминале, без него не пройти (на Linux это util-linux)",
                    full[0]
                ),
                ..PtyEnd::default()
            }
        }
        Err(e) => {
            return PtyEnd {
                error: format!("{}: {e}", full[0]),
                ..PtyEnd::default()
            }
        }
    };
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut answers = 0usize;
    let mut tail = String::new();
    let mut error = String::new();
    let read = async {
        let Some(out) = stdout.as_mut() else { return };
        let mut buf = [0u8; 4096];
        let mut pending = String::new();
        let mut line = String::new();
        loop {
            let n = match out.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let text = strip_ansi(&String::from_utf8_lossy(&buf[..n]));
            push_capped(&mut tail, &text, 4000);
            pending.push_str(&text);
            line.push_str(&text);
            while let Some(i) = line.find('\n') {
                let done: String = line.drain(..=i).collect();
                let done = done.trim_end_matches('\n');
                if !done.trim().is_empty() {
                    on_line(done);
                }
            }
            if is_confirm_prompt(&pending) {
                if answers >= MAX_ANSWERS {
                    error = "программа задаёт больше вопросов, чем ждёт планировщик: остановлено".into();
                    break;
                }
                answers += 1;
                pending.clear();
                if let Some(w) = stdin.as_mut() {
                    let _ = w.write_all(b"y\n").await;
                    let _ = w.flush().await;
                }
            } else if pending.len() > 8000 {
                push_capped(&mut pending, "", 4000);
            }
        }
    };
    if timeout(wait, read).await.is_err() {
        let _ = child.kill().await;
        return PtyEnd {
            timed_out: true,
            answers,
            tail,
            ..PtyEnd::default()
        };
    }
    if !error.is_empty() {
        let _ = child.kill().await;
    }
    drop(stdin);
    let code = child.wait().await.ok().and_then(|s| s.code());
    if let Some(mut e) = stderr.take() {
        let mut text = String::new();
        if timeout(Duration::from_secs(2), e.read_to_string(&mut text)).await.is_ok() && error.is_empty() && code != Some(0) {
            let last = text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
            if !last.is_empty() {
                error = last.to_string();
            }
        }
    }
    PtyEnd {
        code,
        answers,
        tail,
        timed_out: false,
        error,
    }
}

/* ───────── jobs ───────── */

#[derive(Debug, Clone, PartialEq)]
enum Action {
    Herdr,
    Plugin,
    Install(String),
    Check(String),
}

/// `POST /api/galaxy/step {step, target, label, program?, confirm:true}`. Each step but `agent-check` changes a
/// machine, so it needs `confirm`. The answer is `{ok, job}` (progress comes on the bus), or for `agent-login` the
/// command, once the terminal is open.
#[derive(Deserialize)]
pub struct StepBody {
    step: String,
    target: String,
    label: String,
    program: Option<String>,
    confirm: Option<bool>,
}

pub async fn step(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<StepBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if !valid_step(&body.step) {
        return error(StatusCode::BAD_REQUEST, format!("такого шага нет: {:?}", body.step));
    }
    let target = body.target.trim().to_string();
    if !machines::valid_target(&target) {
        return error(StatusCode::BAD_REQUEST, format!("адрес машины не похож на ssh-адрес: {target:?}"));
    }
    let label = body.label.trim().to_string();
    if !machines::valid_label(&label) {
        return error(StatusCode::BAD_REQUEST, format!("метка галактики: буквы, цифры, «-», «_», «.»: {label:?}"));
    }
    if body.step != "agent-check" && body.confirm != Some(true) {
        return error(StatusCode::BAD_REQUEST, "этот шаг меняет машину: нужно подтверждение");
    }
    // Installing on a machine follows the rule of `universe_add`: a loopback planner takes it from this machine only.
    let installs = matches!(body.step.as_str(), "herdr" | "plugin" | "agent-install");
    if installs && !client_is_local(addr, &headers) && app.cfg.is_loopback() {
        return error(StatusCode::FORBIDDEN, "ставить на машину можно только с этого компьютера");
    }
    let program = body.program.as_deref().map(str::trim).unwrap_or("").to_string();
    let m = MachineRef {
        label: label.clone(),
        target: target.clone(),
        session: String::new(),
    };
    let action = match body.step.as_str() {
        "herdr" => Action::Herdr,
        "plugin" => Action::Plugin,
        "agent-install" => {
            if installer_for(&program).is_none() {
                let have: Vec<&str> = INSTALLERS.iter().map(|i| i.program).collect();
                return error(
                    StatusCode::BAD_REQUEST,
                    format!("установщика для «{program}» нет: планировщик ставит {}; остальные ставьте вручную", have.join(", ")),
                );
            }
            Action::Install(program)
        }
        "agent-check" | "agent-login" => {
            if !valid_program(&program) {
                return error(StatusCode::BAD_REQUEST, format!("программы {program:?} в списке экипажа нет"));
            }
            if body.step == "agent-login" {
                return login(&app, addr, &headers, &m, &program).await;
            }
            Action::Check(program)
        }
        _ => return error(StatusCode::BAD_REQUEST, "такого шага нет"),
    };
    let job = format!("g{}", now_ms());
    let app2 = app.clone();
    let job2 = job.clone();
    tokio::spawn(async move {
        run_job(app2, job2, action, m).await;
    });
    Json(json!({"ok": true, "job": job})).into_response()
}

/// Opens a terminal window that runs the agent's login over `ssh -t`. Local clients only: a remote client gets the
/// command text to run itself.
async fn login(app: &Shared, addr: SocketAddr, headers: &HeaderMap, m: &MachineRef, program: &str) -> Response {
    let login = login_command(program);
    let display = format!("ssh -t {} {}", farfiles::shell_line(&[m.target.clone()]), login);
    if !client_is_local(addr, headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Вход открывают за компьютером с доской: выполните команду в своём терминале", "remote": true, "command": display})),
        )
            .into_response();
    }
    let end = match machines::ssh_ended(&app.cfg, m, &installed_check(program, &[]), CHECK_WAIT).await {
        Ok(e) => e,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    if end.timed_out || !end.error.is_empty() || end.code == Some(255) {
        return error(StatusCode::BAD_GATEWAY, ssh_fail("проверка программы", &end, m));
    }
    if !parse_install_check(&end.stdout).1 {
        return error(StatusCode::CONFLICT, format!("{program} на «{}» не установлен: сначала поставьте его", m.label));
    }
    let inner = format!(
        "{login}; r=$?; echo; if [ $r -eq 0 ]; then echo \"вход завершён: можно закрыть окно\"; else echo \"вход не прошёл (код $r)\"; fi; read -r _"
    );
    let mut argv = vec![app.cfg.ssh_bin.clone()];
    match farfiles::ssh_tty_args(&app.cfg, m, &inner) {
        Ok(a) => argv.extend(a),
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    }
    match farfiles::spawn_in_terminal(&argv, farfiles::env_terminal().as_deref()) {
        Ok(()) => Json(json!({"ok": true, "label": m.label, "program": program, "command": display})).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, clip(e)),
    }
}

/// The galaxy list read again at once, after a machine changed: the page shows a new galaxy without waiting for the
/// 30 s poll. The same steps as the poll loop in `main.rs`.
async fn galaxies_again(app: &Shared) {
    let prev = app.galaxies.lock().await.clone();
    let next = machines::poll(&app.cfg, &prev, true).await;
    for g in &next {
        if let Ok(json) = serde_json::to_string(g) {
            app.db.galaxy_put(&g.label, &json, g.ok, g.why.as_ref().map(|w| w.line.as_str()).unwrap_or(""));
        }
    }
    *app.galaxies.lock().await = next;
    app::rebuild_extras(app).await;
    app::publish(app).await;
}

/// Runs one step to its end in the background, reporting each change on the bus. A machine that changed (a new
/// saved machine, a new plugin) is asked again afterwards, so the galaxy list shows it.
async fn run_job(app: Shared, job: String, action: Action, m: MachineRef) {
    match action {
        Action::Herdr => {
            let mut p = Progress::new(&app, &job, &[("herdr", "машина herdr"), ("list", "список машин")]);
            let r = herdr_job(&mut p, &m).await;
            let changed = r.is_ok();
            p.finish(r);
            if changed {
                galaxies_again(&app).await;
                let _ = ask_galaxy(&app, &m.label).await;
            }
        }
        Action::Plugin => {
            let mut p = Progress::new(
                &app,
                &job,
                &[
                    ("check", "что стоит на машине"),
                    ("plugin", "плагин herdr-projects"),
                    ("board", "herdr-projects install"),
                    ("version", "проверка версии"),
                ],
            );
            let r = plugin_job(&app, &mut p, &m).await;
            let changed = r.is_ok();
            p.finish(r);
            if changed {
                galaxies_again(&app).await;
                let _ = ask_galaxy(&app, &m.label).await;
            }
        }
        Action::Install(program) => {
            let mut p = Progress::new(
                &app,
                &job,
                &[("check", "что нужно и что стоит"), ("install", "установщик"), ("verify", "проверка программы")],
            );
            let r = install_job(&app, &mut p, &m, &program).await;
            p.finish(r);
        }
        Action::Check(program) => {
            let mut p = Progress::new(&app, &job, &[("check", "herdr-projects doctor")]);
            let r = check_job(&app, &mut p, &m, &program).await;
            p.finish(r);
        }
    }
}

/// `herdr machine add TARGET --label LABEL` in a pseudo-terminal: herdr installs itself on the machine and saves it.
/// Idempotent: a machine already saved is a finished step.
async fn herdr_job(p: &mut Progress, m: &MachineRef) -> Result<Value, String> {
    let herdr = machines::herdr_bin();
    if machines::saved_machines_with(&herdr).await.iter().any(|x| x.label == m.label) {
        p.ok(0, "машина уже сохранена");
        p.ok(1, "есть в списке");
        return Ok(json!({"label": m.label, "target": m.target, "saved": true, "skipped": true}));
    }
    let mut argv = machines::machine_add_command(&m.target, &m.label).map_err(|e| p.err(0, e))?;
    argv[0] = herdr.clone();
    p.run(0, format!("herdr machine add {} --label {}", m.target, m.label));
    let end = {
        let mut on_line = |line: &str| p.live(0, line);
        run_in_pty(&argv, LONG_WAIT, &mut on_line).await
    };
    if end.timed_out {
        return Err(p.err(0, format!("herdr machine add: нет ответа за {} с", LONG_WAIT.as_secs())));
    }
    if !end.error.is_empty() {
        return Err(p.err(0, end.error.clone()));
    }
    if end.code != Some(0) {
        let last = end.tail.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("").to_string();
        let what = match end.code {
            Some(c) => format!("herdr machine add завершился с кодом {c}"),
            None => "herdr machine add прервался".to_string(),
        };
        return Err(p.err(0, if last.is_empty() { what } else { format!("{what}: {last}") }));
    }
    p.ok(0, format!("herdr machine add: готово (вопросов отвечено: {})", end.answers));
    p.run(1, "проверяю список машин");
    if !machines::saved_machines_with(&herdr).await.iter().any(|x| x.label == m.label) {
        return Err(p.err(1, format!("herdr machine add отработал, но «{}» нет в списке машин", m.label)));
    }
    p.ok(1, "машина в списке");
    Ok(json!({"label": m.label, "target": m.target, "saved": true, "skipped": false, "questions": end.answers}))
}

/// The far check of the plugin step: is herdr there, and what version does the far board say.
const PLUGIN_CHECK: &str = r#"if command -v herdr >/dev/null 2>&1; then echo HERDR=1; else echo HERDR=0; fi
echo "BOARD=$(herdr-projects version 2>/dev/null | head -n 1)"
exit 0"#;

/// Reads `PLUGIN_CHECK`: whether herdr is there, and the board's version line when the plugin is.
pub(crate) fn parse_plugin_check(out: &str) -> (bool, Option<String>) {
    let herdr = out.lines().any(|l| l.trim() == "HERDR=1");
    let board = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("BOARD=").map(str::to_string))
        .filter(|b| !b.trim().is_empty());
    (herdr, board)
}

/// The plugin step: install the same plugin version as the home board (from its commit), then `herdr-projects
/// install`, then read the version back. A machine that has the home version already is a finished step.
async fn plugin_job(app: &Shared, p: &mut Progress, m: &MachineRef) -> Result<Value, String> {
    let cfg = &app.cfg;
    let (home_version, home_ref) = home_facts(cfg, &machines::herdr_bin()).await;
    if home_version.is_empty() {
        return Err(p.err(0, "на этой машине herdr-projects не отвечает, поэтому версию для машины не выбрать"));
    }
    p.run(0, "смотрю, что стоит на машине");
    let end = machines::ssh_ended(cfg, m, PLUGIN_CHECK, CHECK_WAIT)
        .await
        .map_err(|e| p.err(0, e))?;
    if end.timed_out || !end.error.is_empty() || end.code == Some(255) {
        return Err(p.err(0, ssh_fail("проверка", &end, m)));
    }
    let (herdr, board) = parse_plugin_check(&end.stdout);
    if !herdr {
        return Err(p.err(0, format!("на «{}» нет herdr: сначала шаг «машина herdr»", m.label)));
    }
    let far = board.as_deref().and_then(version_token).unwrap_or_default();
    if !far.is_empty() && far == home_version {
        p.ok(0, format!("herdr-projects {far} уже стоит"));
        p.ok(1, "не нужно");
        p.ok(2, "не нужно");
        p.ok(3, format!("версия {far}"));
        return Ok(json!({"version": far, "home_version": home_version, "skipped": true}));
    }
    p.ok(0, if far.is_empty() { "herdr-projects не стоит".to_string() } else { format!("стоит {far}, нужна {home_version}") });

    let install = plugin_install_command(home_ref.as_deref()).map_err(|e| p.err(1, e))?;
    p.run(1, install.clone());
    let end = machines::ssh_ended(cfg, m, &install, LONG_WAIT).await.map_err(|e| p.err(1, e))?;
    if end.timed_out || !end.error.is_empty() || end.code != Some(0) {
        return Err(p.err(1, ssh_fail("herdr plugin install", &end, m)));
    }
    p.ok(1, "плагин поставлен");

    p.run(2, "herdr-projects install");
    let end = machines::ssh_ended(cfg, m, "herdr-projects install", STEP_WAIT).await.map_err(|e| p.err(2, e))?;
    if end.timed_out || !end.error.is_empty() || end.code != Some(0) {
        return Err(p.err(2, ssh_fail("herdr-projects install", &end, m)));
    }
    p.ok(2, "установлено");

    p.run(3, "herdr-projects version");
    let end = machines::ssh_ended(cfg, m, "herdr-projects version", CHECK_WAIT).await.map_err(|e| p.err(3, e))?;
    let version = (!end.timed_out && end.code == Some(0))
        .then(|| board_version(&end.stdout))
        .flatten();
    match version {
        Some(v) => {
            p.ok(3, format!("версия {v}"));
            Ok(json!({"version": v, "home_version": home_version, "ref": home_ref, "skipped": false}))
        }
        None => Err(p.err(3, "herdr-projects не назвал версию после установки: плагин не встал")),
    }
}

/// The check line that tells whether an installer's tools and the agent are there: one `MISSING=` line per tool
/// that is not, and `INSTALLED=1|0`. `program` and `tools` come from the fixed table, never from the page.
fn installed_check(program: &str, tools: &[&str]) -> String {
    let mut s = String::new();
    for t in tools {
        s.push_str(&format!("command -v {t} >/dev/null 2>&1 || echo MISSING={t}\n"));
    }
    s.push_str(&format!(
        "if command -v {program} >/dev/null 2>&1 || [ -x \"$HOME/.local/bin/{program}\" ]; then echo INSTALLED=1; else echo INSTALLED=0; fi\nexit 0"
    ));
    s
}

/// Reads `installed_check`: the tools that are missing, and whether the agent is installed.
pub(crate) fn parse_install_check(out: &str) -> (Vec<String>, bool) {
    let missing = out
        .lines()
        .filter_map(|l| l.trim().strip_prefix("MISSING=").map(str::to_string))
        .collect();
    let installed = out.lines().any(|l| l.trim() == "INSTALLED=1");
    (missing, installed)
}

fn missing_hint(tool: &str, label: &str) -> String {
    match tool {
        "npm" => format!("на «{label}» нет npm: поставьте node с npm, потом повторите этот шаг"),
        "curl" => format!("на «{label}» нет curl: поставьте curl, потом повторите этот шаг"),
        other => format!("на «{label}» нет {other}: поставьте его, потом повторите этот шаг"),
    }
}

/// The agent's own installer over ssh (the table's command), after checking the tools it needs.
async fn install_job(app: &Shared, p: &mut Progress, m: &MachineRef, program: &str) -> Result<Value, String> {
    let inst = installer_for(program).ok_or_else(|| format!("установщика для «{program}» нет"))?;
    p.run(0, "смотрю, что нужно и что стоит");
    let end = machines::ssh_ended(&app.cfg, m, &installed_check(program, inst.needs), CHECK_WAIT)
        .await
        .map_err(|e| p.err(0, e))?;
    if end.timed_out || !end.error.is_empty() || end.code == Some(255) {
        return Err(p.err(0, ssh_fail("проверка", &end, m)));
    }
    let (missing, installed) = parse_install_check(&end.stdout);
    if installed {
        p.ok(0, format!("{program} уже стоит"));
        p.ok(1, "не нужно");
        p.ok(2, "не нужно");
        return Ok(json!({"program": program, "command": inst.command, "installed": true, "skipped": true}));
    }
    if let Some(tool) = missing.first() {
        return Err(p.err(0, missing_hint(tool, &m.label)));
    }
    p.ok(0, "нужное есть");

    p.run(1, clip(inst.command.to_string()));
    let end = machines::ssh_ended(&app.cfg, m, inst.command, LONG_WAIT)
        .await
        .map_err(|e| p.err(1, e))?;
    if end.timed_out || !end.error.is_empty() || end.code != Some(0) {
        return Err(p.err(1, ssh_fail("установщик", &end, m)));
    }
    let last = end.stdout.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("готово");
    p.ok(1, clip(last.to_string()));

    p.run(2, format!("{program} --version"));
    let end = machines::ssh_ended(&app.cfg, m, &installed_check(program, &[]), CHECK_WAIT)
        .await
        .map_err(|e| p.err(2, e))?;
    if !parse_install_check(&end.stdout).1 {
        return Err(p.err(2, format!("установщик отработал, но {program} на «{}» не найден", m.label)));
    }
    p.ok(2, format!("{program} на месте"));
    Ok(json!({"program": program, "command": inst.command, "installed": true, "skipped": false}))
}

/// The line `herdr-projects doctor` prints for one program (`✓ claude 2.1.303, signed in (...)`).
pub(crate) fn doctor_line(out: &str, program: &str) -> Option<String> {
    out.lines()
        .map(str::trim)
        .find(|l| {
            l.trim_start_matches(|c: char| !c.is_alphanumeric())
                .split_whitespace()
                .next()
                == Some(program)
        })
        .map(str::to_string)
}

/// Whether a doctor line says the agent is signed in. Negative phrases are looked at first (`not signed in`).
pub(crate) fn signed_in_of(line: &str) -> bool {
    let t = line.to_lowercase();
    let negative = ["not signed", "signed out", "not logged", "sign in", "login required", "не вошёл", "нет входа", "не вошел"];
    if negative.iter().any(|n| t.contains(n)) {
        return false;
    }
    ["signed in", "logged in", "вошёл", "вошел"].iter().any(|p| t.contains(p))
}

/// `herdr-projects doctor` on the far machine, one program's line and whether it is signed in.
async fn check_job(app: &Shared, p: &mut Progress, m: &MachineRef, program: &str) -> Result<Value, String> {
    p.run(0, "herdr-projects doctor");
    let end = machines::ssh_ended(&app.cfg, m, "herdr-projects doctor", CHECK_WAIT)
        .await
        .map_err(|e| p.err(0, e))?;
    if end.timed_out || !end.error.is_empty() || end.code == Some(255) {
        return Err(p.err(0, ssh_fail("herdr-projects doctor", &end, m)));
    }
    match doctor_line(&end.stdout, program) {
        Some(line) => {
            let signed_in = signed_in_of(&line);
            p.ok(0, clip(line.clone()));
            Ok(json!({"program": program, "signed_in": signed_in, "line": line}))
        }
        None => Err(p.err(
            0,
            format!("herdr-projects doctor на «{}» не назвал {program}: программы там нет или плагин старый", m.label),
        )),
    }
}

/* ───────── far repositories ───────── */

/// Git repositories one or two folders deep in `~/Projects`, as `REPO<TAB>path<TAB>relative<TAB>origin<TAB>branch`.
const PROJECTS_SCRIPT: &str = r#"d="$HOME/Projects"
echo "DIR=$d"
[ -d "$d" ] || exit 0
for g in "$d"/* "$d"/*/*; do
  [ -e "$g/.git" ] || continue
  rel=${g#"$d"/}
  o=$(git -C "$g" remote get-url origin 2>/dev/null | tr -d '\t\r\n')
  b=$(git -C "$g" symbolic-ref --short -q HEAD 2>/dev/null | tr -d '\t\r\n')
  printf 'REPO\t%s\t%s\t%s\t%s\n' "$g" "$rel" "$o" "$b"
done
exit 0"#;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RepoLine {
    pub path: String,
    pub rel: String,
    pub origin: Option<String>,
    pub branch: Option<String>,
}

/// Reads `PROJECTS_SCRIPT`: the folder that was looked at, and the repositories in it. None when there is no answer.
pub(crate) fn parse_projects(out: &str) -> Option<(String, Vec<RepoLine>)> {
    let dir = out.lines().find_map(|l| l.trim().strip_prefix("DIR=").map(str::to_string))?;
    let mut repos = Vec::new();
    for line in out.lines().map(|l| l.trim_end_matches('\r')) {
        let Some(rest) = line.strip_prefix("REPO\t") else {
            continue;
        };
        let mut f = rest.split('\t');
        let (Some(path), Some(rel)) = (f.next(), f.next()) else {
            continue;
        };
        let opt = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        repos.push(RepoLine {
            path: path.to_string(),
            rel: rel.to_string(),
            origin: opt(f.next()),
            branch: opt(f.next()),
        });
    }
    Some((dir, repos))
}

#[derive(Deserialize)]
pub struct ProjectsQuery {
    galaxy: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FarRepo {
    pub name: String,
    pub path: String,
    pub origin: Option<String>,
    pub branch: Option<String>,
    /// The far board has this project (by name, or by folder).
    pub on_board: bool,
}

/// `GET /api/galaxy/projects?galaxy=L`: the git repositories in that machine's `~/Projects`, and whether its board
/// has them. Adding one is `POST /api/universe/add` with `galaxy`.
pub async fn projects(State(app): State<Shared>, Query(q): Query<ProjectsQuery>) -> Response {
    let label = q.galaxy.unwrap_or_default().trim().to_string();
    if label.is_empty() {
        return error(StatusCode::BAD_REQUEST, "нужна галактика: ?galaxy=метка");
    }
    if !machines::valid_label(&label) {
        return error(StatusCode::BAD_REQUEST, "метка галактики: буквы, цифры, «-», «_», «.»");
    }
    let m = match machines::resolve_with(&machines::herdr_bin(), &label).await {
        Ok(m) => m,
        Err(e) => return error(StatusCode::NOT_FOUND, e),
    };
    let end = match machines::ssh_ended(&app.cfg, &m, PROJECTS_SCRIPT, CHECK_WAIT).await {
        Ok(e) => e,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    if end.timed_out || !end.error.is_empty() || end.code != Some(0) {
        let why = machines::why_not(&end, &m);
        return (StatusCode::BAD_GATEWAY, Json(json!({"ok": false, "error": plain(&why), "why": why}))).into_response();
    }
    let Some((dir, lines)) = parse_projects(&end.stdout) else {
        return error(StatusCode::BAD_GATEWAY, format!("«{label}» ответила не то, что ждёт планировщик: нет папки проектов"));
    };
    // The far board's projects as the last read of that machine has them.
    let known: Vec<(String, String)> = app
        .galaxies
        .lock()
        .await
        .iter()
        .find(|g| g.label == label)
        .map(|g| g.projects.iter().map(|p| (p.name.clone(), p.folder.clone())).collect())
        .unwrap_or_default();
    let repos: Vec<FarRepo> = lines
        .into_iter()
        .map(|r| {
            let name = r.rel.rsplit('/').next().unwrap_or(&r.rel).to_string();
            let on_board = known.iter().any(|(n, f)| *n == name || (!f.is_empty() && *f == r.path));
            FarRepo {
                name,
                path: r.path,
                origin: r.origin,
                branch: r.branch,
                on_board,
            }
        })
        .collect();
    Json(json!({"ok": true, "dir": dir, "repos": repos})).into_response()
}

/* ───────── tests ───────── */

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE_OUT: &str = "Welcome to vm-c (Ubuntu 22.04.5 LTS)\n\
herdr-planner-probe 1\n\
os\tUbuntu 22.04.5 LTS\n\
kernel\tLinux\n\
arch\tx86_64\n\
projects\t/home/dev/Projects\n\
tool\tgit\tgit version 2.34.1\n\
tool\tnode\tv18.19.1\n\
program\tclaude\t2.1.303 (Claude Code)\n";

    #[test]
    fn probe_reads_the_far_facts_and_skips_the_greeting() {
        let f = parse_probe(PROBE_OUT).unwrap();
        assert_eq!(f.os, "Ubuntu 22.04.5 LTS");
        assert_eq!(f.kernel, "Linux");
        assert_eq!(f.arch, "x86_64");
        assert_eq!(f.projects_dir, "/home/dev/Projects");
        assert_eq!(
            f.tools,
            vec![
                ("git".to_string(), "git version 2.34.1".to_string()),
                ("node".to_string(), "v18.19.1".to_string())
            ]
        );
        assert_eq!(f.herdr, None);
        assert_eq!(f.board, None);
        assert_eq!(f.programs, vec![("claude".to_string(), "2.1.303 (Claude Code)".to_string())]);
    }

    #[test]
    fn probe_without_the_marker_is_not_a_probe() {
        let err = parse_probe("Welcome\nos\tUbuntu\n").unwrap_err();
        assert!(err.contains("нет ответа"), "{err}");
        assert!(parse_probe("").is_err());
    }

    #[test]
    fn probe_turns_into_the_page_shape() {
        let far = parse_probe(PROBE_OUT).unwrap();
        let p = far.into_probe("1.2.4", Some("7d8bba7c1f2e3a4b5c6d7e8f9a0b1c2d3e4f5a6b".into()));
        assert!(p.supported);
        assert_eq!(p.tools.git.as_deref(), Some("2.34.1"));
        assert_eq!(p.tools.node.as_deref(), Some("v18.19.1"));
        assert_eq!(p.tools.npm, None);
        assert_eq!(p.tools.curl, None);
        assert!(!p.herdr.installed);
        assert!(!p.projects.installed);
        assert!(!p.projects.same);
        assert_eq!(p.projects.home_version, "1.2.4");
        assert_eq!(p.programs.len(), PROGRAMS.len());
        let claude = p.programs.iter().find(|x| x.name == "claude").unwrap();
        assert!(claude.installed);
        assert_eq!(claude.version, "2.1.303 (Claude Code)");
        assert!(!p.programs.iter().find(|x| x.name == "codex").unwrap().installed);
    }

    #[test]
    fn probe_sees_the_same_board_version() {
        let out = format!("{PROBE_OUT}herdr\therdr 0.9.4\nboard\therdr-projects 1.2.4 (7d8bba7)\n");
        let p = parse_probe(&out).unwrap().into_probe("1.2.4", None);
        assert!(p.herdr.installed);
        assert_eq!(p.herdr.version, "0.9.4");
        assert!(p.projects.installed);
        assert_eq!(p.projects.version, "1.2.4");
        assert!(p.projects.same);
        let other = parse_probe(&out).unwrap().into_probe("1.3.0", None);
        assert!(!other.projects.same);
    }

    #[test]
    fn support_needs_linux_or_macos_on_a_64_bit_processor() {
        assert!(supported("Linux", "x86_64"));
        assert!(supported("Darwin", "arm64"));
        assert!(supported("Linux", "aarch64"));
        assert!(!supported("Linux", "armv7l"));
        assert!(!supported("FreeBSD", "x86_64"));
        assert!(!supported("", ""));
    }

    #[test]
    fn version_tokens() {
        assert_eq!(version_token("herdr-projects 1.2.4 (7d8bba7)").as_deref(), Some("1.2.4"));
        assert_eq!(version_token("1.2.4").as_deref(), Some("1.2.4"));
        assert_eq!(version_token("herdr 0.9"), Some("0.9".into()));
        assert_eq!(version_token("nothing here"), None);
        assert_eq!(version_token(""), None);
        assert_eq!(board_version("herdr-projects 1.2.4 (x)\nmore\n").as_deref(), Some("1.2.4"));
        assert_eq!(board_version(""), None);
    }

    #[test]
    fn plugin_ref_is_the_commit_after_the_at_sign() {
        let list = "herdr-projects 1.2.4  eliasstravik/herdr-projects@7d8bba7c1f2e3a4b5c6d7e8f9a0b1c2d3e4f5a6b\nother@aaaaaaaaaa\n";
        assert_eq!(plugin_ref(list).as_deref(), Some("7d8bba7c1f2e3a4b5c6d7e8f9a0b1c2d3e4f5a6b"));
        assert_eq!(plugin_ref("herdr-projects 1.2.4 eliasstravik/herdr-projects@abc1234\n").as_deref(), Some("abc1234"));
        assert_eq!(plugin_ref("herdr-projects 1.2.4 eliasstravik/herdr-projects@abc\n"), None);
        assert_eq!(plugin_ref("herdr-projects 1.2.4\n"), None);
        assert_eq!(plugin_ref(""), None);
    }

    #[test]
    fn plugin_install_line_pins_the_ref_when_known() {
        assert_eq!(
            plugin_install_command(Some("7d8bba7c1f2e")).unwrap(),
            "herdr plugin install eliasstravik/herdr-projects --ref 7d8bba7c1f2e -y"
        );
        assert_eq!(plugin_install_command(None).unwrap(), "herdr plugin install eliasstravik/herdr-projects -y");
        assert!(plugin_install_command(Some("7d8bba7; rm -rf ~")).is_err());
    }

    #[test]
    fn plugin_check_reads_herdr_and_the_board() {
        assert_eq!(parse_plugin_check("HERDR=1\nBOARD=herdr-projects 1.2.4 (x)\n"), (true, Some("herdr-projects 1.2.4 (x)".into())));
        assert_eq!(parse_plugin_check("HERDR=0\nBOARD=\n"), (false, None));
    }

    #[test]
    fn installer_table_names_the_tools_each_program_needs() {
        let claude = installer_for("claude").unwrap();
        assert_eq!(claude.needs, &["curl"]);
        assert!(claude.command.starts_with("curl "));
        for p in ["codex", "omp", "pi", "opencode"] {
            let i = installer_for(p).unwrap();
            assert_eq!(i.needs, &["npm"], "{p}");
            assert!(i.command.starts_with("npm install -g "), "{p}");
        }
        assert!(installer_for("gemini").is_none(), "only the five the planner installs");
        assert!(installer_for("").is_none());
        for i in INSTALLERS {
            assert!(valid_program(i.program), "{} is in the crew list", i.program);
            assert!(!i.command.contains('\n') && !i.command.contains('\''), "{}: a plain command line", i.program);
        }
    }

    #[test]
    fn login_table() {
        assert_eq!(login_command("claude"), "claude auth login");
        assert_eq!(login_command("codex"), "codex login");
        assert_eq!(login_command("omp"), "omp login");
        assert_eq!(login_command("opencode"), "opencode login");
    }

    #[test]
    fn install_check_lists_the_missing_tools() {
        let script = installed_check("codex", &["npm"]);
        assert!(script.contains("command -v npm"));
        assert!(script.contains("command -v codex"));
        assert_eq!(parse_install_check("MISSING=npm\nINSTALLED=0\n"), (vec!["npm".to_string()], false));
        assert_eq!(parse_install_check("INSTALLED=1\n"), (Vec::<String>::new(), true));
        assert_eq!(parse_install_check("junk"), (Vec::<String>::new(), false));
        assert!(missing_hint("npm", "vm-c").contains("npm"));
        assert!(missing_hint("curl", "vm-c").contains("curl"));
    }

    #[test]
    fn validators_refuse_what_the_shell_must_not_see() {
        assert!(valid_step("herdr") && valid_step("agent-check"));
        assert!(!valid_step("herdr; rm") && !valid_step(""));
        assert!(valid_program("claude") && valid_program("cursor-agent"));
        assert!(!valid_program("claude;ls") && !valid_program(""));
        assert!(valid_ref("7d8bba7") && valid_ref("7d8bba7c1f2e3a4b5c6d7e8f9a0b1c2d3e4f5a6b"));
        assert!(!valid_ref("7d8bba") && !valid_ref("zzzzzzz") && !valid_ref(&"a".repeat(41)));
        assert!(machines::valid_target("dev@vm-c") && !machines::valid_target("-oProxyCommand=x"));
    }

    #[test]
    fn doctor_line_is_the_one_for_the_program_and_it_says_signed_in() {
        let out = "herdr-projects doctor\n✓ claude 2.1.303, signed in (oauth)\n✗ codex not installed\n⚠ omp 1.0, not signed in\n";
        assert_eq!(doctor_line(out, "claude").as_deref(), Some("✓ claude 2.1.303, signed in (oauth)"));
        assert!(signed_in_of(&doctor_line(out, "claude").unwrap()));
        assert!(!signed_in_of(&doctor_line(out, "omp").unwrap()));
        assert!(!signed_in_of(&doctor_line(out, "codex").unwrap()));
        assert_eq!(doctor_line(out, "pi"), None);
        assert_eq!(doctor_line(out, "clau"), None, "a prefix is not a name");
        assert!(signed_in_of("вошёл как dev"));
        assert!(!signed_in_of("не вошёл"));
    }

    #[test]
    fn projects_are_the_repository_lines_under_the_folder() {
        let out = "Welcome\nDIR=/home/dev/Projects\n\
REPO\t/home/dev/Projects/api\tapi\thttps://github.com/acme/api.git\tmain\n\
REPO\t/home/dev/Projects/web\tweb\t\tmain\n\
REPO\t/home/dev/Projects/labs/tools\tlabs/tools\tgit@github.com:acme/tools.git\t\n\
junk line\n\
REPO\tbroken\n";
        let (dir, repos) = parse_projects(out).unwrap();
        assert_eq!(dir, "/home/dev/Projects");
        assert_eq!(repos.len(), 3);
        assert_eq!(repos[0].rel, "api");
        assert_eq!(repos[0].origin.as_deref(), Some("https://github.com/acme/api.git"));
        assert_eq!(repos[1].origin, None);
        assert_eq!(repos[1].branch.as_deref(), Some("main"));
        assert_eq!(repos[2].rel, "labs/tools");
        assert_eq!(repos[2].branch, None);
        assert_eq!(parse_projects("no folder here\n"), None);
        assert_eq!(parse_projects("DIR=/home/dev/Projects\n").unwrap().1.len(), 0);
    }

    #[test]
    fn terminal_codes_and_prompts() {
        assert_eq!(strip_ansi("\x1b[1;32mok\x1b[0m\r\n"), "ok\n");
        assert_eq!(strip_ansi("a\x1b]0;title\x07b"), "ab");
        assert_eq!(strip_ansi("back\x08space\x07"), "backspace");
        assert!(is_confirm_prompt("Install herdr on dev@vm-c? [y/N] "));
        assert!(is_confirm_prompt("Continue (Y/n)"));
        assert!(is_confirm_prompt("Proceed? (yes/no) "));
        assert!(is_confirm_prompt("Install? [Y/n]"));
        assert!(!is_confirm_prompt("Enter y/n"), "no brackets, no question");
        assert!(!is_confirm_prompt("Installing herdr 1.2.4…"));
    }

    #[test]
    fn pty_argv_per_system() {
        let argv = vec!["/bin/herdr".to_string(), "machine".into(), "add".into(), "dev@vm-c".into()];
        assert_eq!(
            pty_argv("linux", &argv),
            vec!["script", "-qefc", "/bin/herdr machine add dev@vm-c", "/dev/null"]
        );
        assert_eq!(pty_argv("macos", &argv)[..3], ["script", "-q", "/dev/null"]);
        assert_eq!(pty_argv("macos", &argv)[3..], argv[..]);
    }

    #[test]
    fn plain_text_error_is_the_reason_then_the_way_out() {
        let why = Why {
            line: "она отказала в соединении".into(),
            detail: vec!["Проверьте, что она включена.".into()],
        };
        assert_eq!(plain(&why), "она отказала в соединении. Проверьте, что она включена.");
        let dotted = Why {
            line: "она отказала в соединении.".into(),
            detail: vec!["Эта машина не достаёт vm-b.".into(), "".into()],
        };
        assert_eq!(plain(&dotted), "она отказала в соединении. Эта машина не достаёт vm-b.");
    }

    /// Runs `script` for real when the machine has it: the program's question is answered `y` and its answer comes back.
    #[tokio::test]
    async fn pty_answers_the_question_it_is_asked() {
        let has = |p: &str| std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(p).exists()));
        if !has("script") || !has("sh") || std::env::consts::OS != "linux" {
            return;
        }
        let argv = vec![
            "sh".to_string(),
            "-c".into(),
            "printf 'Install herdr on dev@vm-c? [y/N] '; read a; echo \"got:$a\"".into(),
        ];
        let mut lines: Vec<String> = Vec::new();
        let mut on_line = |l: &str| lines.push(l.to_string());
        let end = run_in_pty(&argv, Duration::from_secs(20), &mut on_line).await;
        assert_eq!(end.code, Some(0), "{end:?}");
        assert_eq!(end.answers, 1, "{end:?}");
        assert!(end.tail.contains("got:y"), "{end:?}");
        assert!(lines.iter().any(|l| l.contains("got:y")), "{lines:?}");
    }

    /// A program that asks more than `MAX_ANSWERS` times is stopped, not answered forever.
    #[tokio::test]
    async fn pty_stops_a_program_that_keeps_asking() {
        let has = |p: &str| std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(p).exists()));
        if !has("script") || !has("sh") || std::env::consts::OS != "linux" {
            return;
        }
        let argv = vec![
            "sh".to_string(),
            "-c".into(),
            "i=0; while [ $i -lt 20 ]; do printf 'Again? [y/N] '; read a || break; i=$((i+1)); done".into(),
        ];
        let mut on_line = |_: &str| {};
        let end = run_in_pty(&argv, Duration::from_secs(20), &mut on_line).await;
        assert!(end.answers <= MAX_ANSWERS, "{end:?}");
        assert!(!end.error.is_empty(), "{end:?}");
    }
}
