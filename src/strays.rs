//! Processes left behind by finished tasks (PLN-53): an agent (`omp --resume=…`) or its helper (`node …/rea.mjs mcp`,
//! `omp __omp_worker_daemon_broker`) that outlives its task and keeps the task's working copy «in use», so the board
//! cannot remove it. The rule is the process's folder and the task's state, never the program's name: a process whose
//! current directory is inside the working copy of a task that is done or archived (or of a copy no task has any
//! more) is a leftover; a task still in work, a folder that is not a board working copy and another user's process
//! are never touched.
//!
//! An agent that is alive starts its helpers again, so a leftover is stopped together with everything that shares its
//! working copy, parents first: SIGTERM, a few seconds, then SIGKILL for what is still there. Before each signal the
//! process is looked up again and must be the same one (the same start time) and still be a leftover; a pid that was
//! reused, or a task that came back to work in the meantime, is refused.
//!
//! The checks replace the machine: `HERDR_PLANNER_PROC_FILE` names a JSON file that plays the process table (the
//! processes are never signalled then; a «stopped» one is deleted from the file and written to `FILE.signals`),
//! `HERDR_PLANNER_WORKTREES` (a `:` list) the folders that hold working copies, `HERDR_PLANNER_STRAY_MS`,
//! `HERDR_PLANNER_STRAY_GRACE_MS` and `HERDR_PLANNER_STRAY_TERM_MS` shorten the intervals.

use crate::app::Shared;
use crate::config::{home, now_ms};
use crate::runners_log::Entry;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The journal key of this machine's own rows (the shipyards' rows live under their labels).
pub const JOURNAL_KEY: &str = "@machine";
/// The kind of a journal row of this feature.
pub const KIND: &str = "stray";
pub const SCAN_EVERY_MS: u64 = 60_000;
/// How long a leftover must have been seen before the automatic cleaning stops it: a task that has just been marked
/// done may still be writing its last words.
/// A board read older than this is not a ground for stopping anything.
pub const BOARD_FRESH_MS: u64 = 5 * 60 * 1000;
pub const GRACE_MS: u64 = 5 * 60 * 1000;
/// How long SIGTERM is given before SIGKILL.
pub const TERM_WAIT_MS: u64 = 5000;
const KILL_WAIT_MS: u64 = 2000;
const CMD_CHARS: usize = 300;
const AUTO_KEY: &str = "stray_auto";
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "nu"];

fn env_ms(name: &str, min: u64, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok()).filter(|v| *v >= min).unwrap_or(default)
}

pub fn scan_every() -> Duration {
    Duration::from_millis(env_ms("HERDR_PLANNER_STRAY_MS", 100, SCAN_EVERY_MS))
}

pub fn grace_ms() -> u64 {
    env_ms("HERDR_PLANNER_STRAY_GRACE_MS", 0, GRACE_MS)
}

pub fn term_wait_ms() -> u64 {
    env_ms("HERDR_PLANNER_STRAY_TERM_MS", 50, TERM_WAIT_MS)
}

/* ───────── the process table ───────── */

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Proc {
    pub pid: u32,
    #[serde(default)]
    pub ppid: u32,
    #[serde(default)]
    pub pgid: u32,
    /// Absent in a fixture: the owner of the table.
    #[serde(default)]
    pub uid: Option<u32>,
    #[serde(default)]
    pub comm: String,
    #[serde(default)]
    pub cmd: String,
    #[serde(default)]
    pub cwd: String,
    /// Resident memory, bytes.
    #[serde(default)]
    pub rss: u64,
    #[serde(default)]
    pub age_ms: u64,
    /// The start time in clock ticks since boot: with the pid it names one process, not a later one with the same pid.
    #[serde(default)]
    pub start: u64,
    /// A fixture's process that ignores SIGTERM and goes only on SIGKILL.
    #[serde(default)]
    pub ignore_term: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    /// Whose processes may be touched: the planner's own user.
    pub uid: u32,
    /// The planner's own pid: it, its ancestors and its children are never listed.
    pub pid: u32,
    pub procs: Vec<Proc>,
}

pub trait Sys: Send + Sync {
    fn read(&self) -> Result<Table, String>;
    /// `kill` false is SIGTERM, true is SIGKILL. A process that is gone already is not an error.
    fn signal(&self, pid: u32, kill: bool) -> Result<(), String>;
}

pub fn sys() -> Arc<dyn Sys> {
    match std::env::var_os("HERDR_PLANNER_PROC_FILE").filter(|v| !v.is_empty()) {
        Some(p) => Arc::new(Fixture(PathBuf::from(p))),
        None => Arc::new(Real),
    }
}

pub struct Real;

/// `pid (comm) S ppid pgrp session …`: the comm may hold spaces and brackets, so the fields are counted from the
/// last `)`. Returns the comm, the state letter, ppid, pgrp and the start time in ticks.
pub fn parse_stat(text: &str) -> Option<(String, char, u32, u32, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let comm = text[open + 1..close].to_string();
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let state = rest.first()?.chars().next()?;
    let ppid = rest.get(1)?.parse().ok()?;
    let pgrp = rest.get(2)?.parse().ok()?;
    let start = rest.get(19)?.parse().ok()?;
    Some((comm, state, ppid, pgrp, start))
}

fn parse_rss(status: &str) -> u64 {
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(0, |kb| kb * 1024)
}

fn cmdline_of(raw: &[u8], comm: &str) -> String {
    let parts: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    if parts.is_empty() {
        format!("[{comm}]")
    } else {
        parts.join(" ")
    }
}

#[cfg(unix)]
fn own_uid() -> u32 {
    // SAFETY: getuid has no failure mode and touches no memory.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn own_uid() -> u32 {
    0
}

impl Sys for Real {
    #[cfg(target_os = "linux")]
    fn read(&self) -> Result<Table, String> {
        use std::os::unix::fs::MetadataExt;
        let uid = own_uid();
        let up = std::fs::read_to_string("/proc/uptime").map_err(|e| format!("/proc/uptime: {e}"))?;
        let uptime: f64 = up.split_whitespace().next().and_then(|v| v.parse().ok()).ok_or("/proc/uptime: no number")?;
        // SAFETY: sysconf only reads a constant.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
        let mut procs = Vec::new();
        for e in std::fs::read_dir("/proc").map_err(|e| format!("/proc: {e}"))?.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            // Another user's process is not read at all.
            if e.metadata().map_or(true, |m| m.uid() != uid) {
                continue;
            }
            let dir = e.path();
            let Ok(stat) = std::fs::read_to_string(dir.join("stat")) else { continue };
            let Some((comm, state, ppid, pgid, start)) = parse_stat(&stat) else { continue };
            if state == 'Z' || state == 'X' {
                continue;
            }
            let Ok(cwd) = std::fs::read_link(dir.join("cwd")) else { continue };
            let cwd = cwd.to_string_lossy().trim_end_matches(" (deleted)").to_string();
            let cmd = cmdline_of(&std::fs::read(dir.join("cmdline")).unwrap_or_default(), &comm);
            let rss = std::fs::read_to_string(dir.join("status")).map(|s| parse_rss(&s)).unwrap_or(0);
            let age = (uptime - start as f64 / hz).max(0.0);
            procs.push(Proc { pid, ppid, pgid, uid: Some(uid), comm, cmd, cwd, rss, age_ms: (age * 1000.0) as u64, start, ignore_term: false });
        }
        Ok(Table { uid, pid: std::process::id(), procs })
    }

    #[cfg(not(target_os = "linux"))]
    fn read(&self) -> Result<Table, String> {
        Err("no /proc on this system".into())
    }

    #[cfg(unix)]
    fn signal(&self, pid: u32, kill: bool) -> Result<(), String> {
        if pid <= 1 || pid > i32::MAX as u32 {
            return Err(format!("pid {pid}: refused"));
        }
        let sig = if kill { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: a plain signal to one positive pid; never a group (negative) and never everyone (0, -1).
        let rc = unsafe { libc::kill(pid as i32, sig) };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(err.to_string())
        }
    }

    #[cfg(not(unix))]
    fn signal(&self, _pid: u32, _kill: bool) -> Result<(), String> {
        Err("signals are not available here".into())
    }
}

/// The checks' table: a JSON file `{"uid": 1000, "procs": [...]}`. «Signals» only edit the file.
pub struct Fixture(pub PathBuf);

#[derive(Deserialize, Serialize, Default)]
struct FixtureFile {
    #[serde(default = "fixture_uid")]
    uid: u32,
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    procs: Vec<Proc>,
}

fn fixture_uid() -> u32 {
    1000
}

impl Fixture {
    fn load(&self) -> Result<FixtureFile, String> {
        let text = std::fs::read_to_string(&self.0).map_err(|e| format!("{}: {e}", self.0.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", self.0.display()))
    }
}

impl Sys for Fixture {
    fn read(&self) -> Result<Table, String> {
        let f = self.load()?;
        let procs = f.procs.into_iter().map(|p| Proc { uid: Some(p.uid.unwrap_or(f.uid)), ..p }).collect();
        Ok(Table { uid: f.uid, pid: f.pid, procs })
    }

    fn signal(&self, pid: u32, kill: bool) -> Result<(), String> {
        let mut f = self.load()?;
        let word = if kill { "KILL" } else { "TERM" };
        let log = format!("{}.signals", self.0.display());
        let mut lines = std::fs::read_to_string(&log).unwrap_or_default();
        lines.push_str(&format!("{word} {pid}\n"));
        std::fs::write(&log, lines).map_err(|e| e.to_string())?;
        f.procs.retain(|p| p.pid != pid || (!kill && p.ignore_term));
        let text = serde_json::to_string_pretty(&f).map_err(|e| e.to_string())?;
        std::fs::write(&self.0, text).map_err(|e| e.to_string())
    }
}

/* ───────── what is a leftover ───────── */

/// A task of the board as far as this feature needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskInfo {
    pub id: String,
    pub tag: String,
    pub repo: String,
    pub n: u64,
    pub worktree: String,
    /// `active`, `done` or `archived`.
    pub state: &'static str,
}

/// Everything the rule needs from the board, taken at one moment.
#[derive(Debug, Clone, Default)]
pub struct Board {
    /// The board was read lately and without an error. A leftover is judged by what the board says about its task, so
    /// from a read that failed (the old list is kept) or is old nothing is judged: a task started since would look
    /// like a copy nobody has.
    pub ok: bool,
    pub tasks: Vec<TaskInfo>,
    pub repos: Vec<String>,
    /// The folders that hold working copies (`<root>/<repo>/work-<n>`).
    pub roots: Vec<PathBuf>,
    /// The repositories' own folders: a task record that names one of them (or a folder above one) as its working
    /// copy is not believed, because the human's own checkout is not a leftover.
    pub keep: Vec<PathBuf>,
}

pub fn board_of(snap: &crate::app::Snapshot) -> Board {
    let mut tasks: Vec<TaskInfo> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (list, archived) in [(&snap.tasks, false), (&snap.archived, true)] {
        for t in list {
            if !seen.insert(t.id.clone()) {
                continue;
            }
            let state = if archived || t.archived {
                "archived"
            } else if t.status == crate::board::Status::Done {
                "done"
            } else {
                "active"
            };
            tasks.push(TaskInfo {
                id: t.id.clone(),
                tag: t.tag.clone(),
                repo: t.repo.clone(),
                n: t.n,
                worktree: t.worktree.clone(),
                state,
            });
        }
    }
    let mut repos: BTreeSet<String> = snap.projects.iter().map(|p| p.name.clone()).collect();
    repos.extend(tasks.iter().map(|t| t.repo.clone()));
    let mut roots: Vec<PathBuf> = match std::env::var("HERDR_PLANNER_WORKTREES") {
        Ok(v) => v.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect(),
        Err(_) => vec![home().join(".herdr/worktrees")],
    };
    for t in &tasks {
        let p = Path::new(&t.worktree);
        if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("work-")) {
            if let Some(root) = p.parent().and_then(Path::parent) {
                roots.push(root.to_path_buf());
            }
        }
    }
    // A folder is named by its real path in /proc: a symlinked home must match too.
    let real: Vec<PathBuf> = roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).collect();
    roots.extend(real);
    roots.retain(|r| r.is_absolute() && r.components().count() >= 2);
    roots.sort();
    roots.dedup();
    let keep = snap.projects.iter().flat_map(|p| [p.root.as_str(), p.folder.as_str()]).filter(|p| !p.is_empty()).map(PathBuf::from).collect();
    let ok = snap.board.ok && snap.at > 0 && now_ms().saturating_sub(snap.at) < BOARD_FRESH_MS;
    Board { ok, tasks, repos: repos.into_iter().collect(), roots, keep }
}

/// `work-12` or `work-12-slug` → 12.
fn number_of(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("work-")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest[digits.len()..].is_empty() && !rest[digits.len()..].starts_with('-') {
        return None;
    }
    digits.parse().ok()
}

/// Why a folder's processes are leftovers: the task's state (`done`, `archived`) or `gone` (no task has this copy any
/// more). `None`: not a board working copy, or a task in work.
fn verdict<'a>(cwd: &str, board: &'a Board) -> Option<(&'static str, Option<&'a TaskInfo>, PathBuf)> {
    let cwd = Path::new(cwd);
    // The task's own record says where its working copy is: what runs inside it belongs to that task.
    let recorded = board
        .tasks
        .iter()
        .filter(|t| {
            let w = Path::new(&t.worktree);
            !t.worktree.is_empty() && w.is_absolute() && w.components().count() >= 4 && cwd.starts_with(w) && !board.keep.iter().any(|k| k.starts_with(w))
        })
        .max_by_key(|t| t.worktree.len());
    if let Some(t) = recorded {
        return if t.state == "active" { None } else { Some((t.state, Some(t), PathBuf::from(&t.worktree))) };
    }
    for root in &board.roots {
        let Ok(rel) = cwd.strip_prefix(root) else { continue };
        let mut parts = rel.components();
        let (Some(repo), Some(name)) = (parts.next(), parts.next()) else { continue };
        let (repo, name) = (repo.as_os_str().to_string_lossy(), name.as_os_str().to_string_lossy());
        let copy = root.join(repo.as_ref()).join(name.as_ref());
        let by_path = board.tasks.iter().find(|t| !t.worktree.is_empty() && Path::new(&t.worktree) == copy);
        let n = number_of(&name);
        let by_name = || board.tasks.iter().find(|t| t.repo == repo && Some(t.n) == n);
        match by_path.or_else(by_name) {
            Some(t) if t.state == "active" => return None,
            Some(t) => return Some((t.state, Some(t), copy)),
            None => {
                // Nobody has this copy: a leftover only when it is plainly a board copy (the board's own name for it,
                // in a repository the board knows), never a folder somebody else made.
                // Numbers only grow: a number above the newest the board knows in that repository is a task started
                // since the board was read, not one that left.
                let newest = board.tasks.iter().filter(|t| t.repo == repo).map(|t| t.n).max();
                if n.is_some_and(|n| newest.is_some_and(|m| n < m)) && board.repos.iter().any(|r| *r == repo) {
                    return Some(("gone", None, copy));
                }
                return None;
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stray {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub comm: String,
    pub cmd: String,
    pub cwd: String,
    /// The task's id and tag; for a copy no task has any more, `repo/n` from the folder's name.
    pub task: String,
    pub tag: String,
    /// `done`, `archived` or `gone`.
    pub why: &'static str,
    pub worktree: String,
    pub rss: u64,
    pub age_ms: u64,
    pub start: u64,
    /// How many of the listed processes are above this one: 0 is the top of a tree.
    pub depth: u32,
    /// A shell: the human's terminal may be in it, so the automatic cleaning leaves it.
    pub shell: bool,
    /// When this leftover was first seen (unix ms); filled by the look, 0 in a pure `find`.
    pub seen_at: u64,
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// The leftovers of a table, parents first. Pure: nothing is read or signalled here.
pub fn find(table: &Table, board: &Board) -> Vec<Stray> {
    if !board.ok {
        return Vec::new();
    }
    let by_pid: HashMap<u32, &Proc> = table.procs.iter().map(|p| (p.pid, p)).collect();
    // The planner itself, whatever it runs from, and the line above and below it.
    let mut safe: HashSet<u32> = HashSet::new();
    let mut at = table.pid;
    while at > 1 && safe.insert(at) {
        match by_pid.get(&at) {
            Some(p) => at = p.ppid,
            None => break,
        }
    }
    let mut changed = table.pid > 1;
    while changed {
        changed = false;
        for p in &table.procs {
            if safe.contains(&p.ppid) && safe.insert(p.pid) {
                changed = true;
            }
        }
    }
    let mut found: Vec<Stray> = Vec::new();
    for p in &table.procs {
        if p.pid <= 1 || safe.contains(&p.pid) || p.uid.unwrap_or(table.uid) != table.uid || !p.cwd.starts_with('/') {
            continue;
        }
        let Some((why, task, copy)) = verdict(&p.cwd, board) else { continue };
        let (id, tag) = match task {
            Some(t) => (t.id.clone(), t.tag.clone()),
            None => {
                let name = copy.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let repo = copy.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                (format!("{repo}/{}", number_of(&name).unwrap_or(0)), String::new())
            }
        };
        found.push(Stray {
            pid: p.pid,
            ppid: p.ppid,
            pgid: p.pgid,
            comm: p.comm.clone(),
            cmd: clip(&p.cmd, CMD_CHARS),
            cwd: p.cwd.clone(),
            task: id,
            tag,
            why,
            worktree: copy.to_string_lossy().into_owned(),
            rss: p.rss,
            age_ms: p.age_ms,
            start: p.start,
            depth: 0,
            shell: SHELLS.contains(&p.comm.as_str()),
            seen_at: 0,
        });
    }
    let listed: HashSet<u32> = found.iter().map(|s| s.pid).collect();
    for s in &mut found {
        let mut depth = 0;
        let mut at = s.ppid;
        let mut guard = 0;
        while listed.contains(&at) && guard < 64 {
            depth += 1;
            at = by_pid.get(&at).map_or(0, |p| p.ppid);
            guard += 1;
        }
        s.depth = depth;
    }
    found.sort_by(|a, b| (&a.task, a.depth, a.pid).cmp(&(&b.task, b.depth, b.pid)));
    found
}

/* ───────── stopping ───────── */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub struct Target {
    pub pid: u32,
    pub start: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    pub pid: u32,
    pub cmd: String,
    pub task: String,
    pub tag: String,
    /// `term` (stopped by SIGTERM), `kill` (needed SIGKILL), `gone` (it was gone already), `refused` (no longer a
    /// leftover, or not the same process) or `failed`.
    pub result: &'static str,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    pub items: Vec<Outcome>,
    /// How many leftovers are listed after the stop.
    pub left: usize,
}

impl Report {
    pub fn count(&self, result: &str) -> usize {
        self.items.iter().filter(|o| o.result == result).count()
    }

    pub fn stopped(&self) -> usize {
        self.count("term") + self.count("kill")
    }
}

fn present(table: &Table, pid: u32, start: u64) -> bool {
    table.procs.iter().any(|p| p.pid == pid && p.start == start)
}

async fn wait_gone(sys: &dyn Sys, pids: &[(u32, u64)], ms: u64) -> Vec<(u32, u64)> {
    let end = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let alive: Vec<(u32, u64)> = match sys.read() {
            Ok(t) => pids.iter().copied().filter(|(p, s)| present(&t, *p, *s)).collect(),
            Err(_) => pids.to_vec(),
        };
        if alive.is_empty() || tokio::time::Instant::now() >= end {
            return alive;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Stops the asked leftovers, and only those: each one is found again as a leftover first (the same pid and start
/// time, still a leftover), and what is not is refused. SIGTERM first (parents first, so no agent starts its helpers
/// again), `term_wait_ms` of waiting, then SIGKILL for what is left.
pub async fn stop(sys: &dyn Sys, board: &Board, asked: &[Target], term_wait_ms: u64) -> Report {
    let mut report = Report::default();
    let table = match sys.read() {
        Ok(t) => t,
        Err(e) => {
            for t in asked {
                report.items.push(Outcome { pid: t.pid, cmd: String::new(), task: String::new(), tag: String::new(), result: "failed", error: e.clone() });
            }
            return report;
        }
    };
    let found = find(&table, board);
    let mut seen: HashSet<Target> = HashSet::new();
    let mut valid: Vec<&Stray> = Vec::new();
    for t in asked {
        if !seen.insert(*t) {
            continue;
        }
        match found.iter().find(|s| s.pid == t.pid && s.start == t.start) {
            Some(s) => valid.push(s),
            None => {
                let was_there = table.procs.iter().any(|p| p.pid == t.pid);
                let cmd = table.procs.iter().find(|p| p.pid == t.pid).map(|p| clip(&p.cmd, CMD_CHARS)).unwrap_or_default();
                report.items.push(Outcome {
                    pid: t.pid,
                    cmd,
                    task: String::new(),
                    tag: String::new(),
                    result: if was_there { "refused" } else { "gone" },
                    error: String::new(),
                });
            }
        }
    }
    valid.sort_by_key(|s| (s.depth, s.pid));
    let mut signalled: Vec<&Stray> = Vec::new();
    let outcome = |s: &Stray, result: &'static str, error: String| Outcome {
        pid: s.pid,
        cmd: s.cmd.clone(),
        task: s.task.clone(),
        tag: s.tag.clone(),
        result,
        error,
    };
    let mut failed: HashMap<u32, String> = HashMap::new();
    for s in &valid {
        match sys.signal(s.pid, false) {
            Ok(()) => signalled.push(s),
            Err(e) => {
                failed.insert(s.pid, e);
            }
        }
    }
    let ids: Vec<(u32, u64)> = signalled.iter().map(|s| (s.pid, s.start)).collect();
    let alive = wait_gone(sys, &ids, term_wait_ms).await;
    let mut killed: HashSet<u32> = HashSet::new();
    if !alive.is_empty() {
        // Look again: only a process that is still the same and still a leftover gets the second signal.
        let again = sys.read().map(|t| find(&t, board)).unwrap_or_default();
        for s in &signalled {
            if !alive.contains(&(s.pid, s.start)) || !again.iter().any(|a| a.pid == s.pid && a.start == s.start) {
                continue;
            }
            match sys.signal(s.pid, true) {
                Ok(()) => {
                    killed.insert(s.pid);
                }
                Err(e) => {
                    failed.insert(s.pid, e);
                }
            }
        }
        let k: Vec<(u32, u64)> = alive.iter().copied().filter(|(p, _)| killed.contains(p)).collect();
        let still = wait_gone(sys, &k, KILL_WAIT_MS).await;
        for (pid, _) in still {
            failed.insert(pid, "не завершился после SIGKILL".into());
        }
    }
    let after = sys.read().map(|t| t.procs).unwrap_or_default();
    for s in &valid {
        let result = if let Some(e) = failed.get(&s.pid) {
            if after.iter().any(|p| p.pid == s.pid && p.start == s.start) {
                report.items.push(outcome(s, "failed", e.clone()));
                continue;
            }
            // The signal failed but the process is gone anyway: it left by itself.
            "gone"
        } else if killed.contains(&s.pid) {
            "kill"
        } else if alive.iter().any(|(p, _)| *p == s.pid) {
            report.items.push(outcome(s, "failed", "не завершился и больше не относится к завершённой задаче".into()));
            continue;
        } else {
            "term"
        };
        report.items.push(outcome(s, result, String::new()));
    }
    report.left = sys.read().map(|t| find(&t, board).len()).unwrap_or(0);
    report
}

/* ───────── the periodic look, the switch, the journal ───────── */

#[derive(Debug, Clone, Default, Serialize)]
pub struct Scan {
    pub at: u64,
    pub ok: bool,
    /// The board could not be trusted at that moment, so no process was judged.
    pub board_ok: bool,
    pub error: String,
    pub strays: Vec<Stray>,
}

#[derive(Default)]
struct State {
    last: Option<Scan>,
    /// When each leftover was first seen, by pid and start time.
    first_seen: HashMap<(u32, u64), u64>,
    /// The ones the journal has already said something about.
    announced: HashSet<(u32, u64)>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

/// Whether this system can be looked at: `/proc` (Linux), or the checks' table.
pub fn supported() -> bool {
    cfg!(target_os = "linux") || std::env::var_os("HERDR_PLANNER_PROC_FILE").is_some_and(|v| !v.is_empty())
}

pub fn auto_on(app: &Shared) -> bool {
    app.db.meta_get(AUTO_KEY).is_some_and(|v| v == "1")
}

pub fn set_auto(app: &Shared, on: bool) {
    app.db.meta_set(AUTO_KEY, if on { "1" } else { "0" });
}

/// Looks at the machine now and remembers what it saw (the page asks for this, and so does the loop).
pub async fn scan(app: &Shared) -> Scan {
    let board = {
        let snap = app.snapshot.read().await;
        board_of(&snap)
    };
    let board_ok = board.ok;
    let result = tokio::task::spawn_blocking(move || sys().read().map(|t| find(&t, &board))).await.map_err(|e| e.to_string()).and_then(|r| r);
    let now = now_ms();
    let mut scan = match result {
        Ok(strays) => Scan { at: now, ok: true, board_ok, error: String::new(), strays },
        Err(error) => Scan { at: now, ok: false, board_ok, error, strays: Vec::new() },
    };
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let keys: HashSet<(u32, u64)> = scan.strays.iter().map(|s| (s.pid, s.start)).collect();
    st.first_seen.retain(|k, _| keys.contains(k));
    for k in &keys {
        st.first_seen.entry(*k).or_insert(now);
    }
    for s in &mut scan.strays {
        s.seen_at = st.first_seen.get(&(s.pid, s.start)).copied().unwrap_or(now);
    }
    st.announced.retain(|k| keys.contains(k));
    st.last = Some(scan.clone());
    scan
}

/// The last look, or a new one when there is none or it is old.
pub async fn latest(app: &Shared, fresh: bool) -> Scan {
    if !fresh {
        let st = state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = &st.last {
            if now_ms().saturating_sub(s.at) < scan_every().as_millis() as u64 * 2 {
                return s.clone();
            }
        }
    }
    scan(app).await
}

/// The journal row of one stop (`who` is `human` or `auto`).
pub fn stopped_entry(at: u64, who: &str, report: &Report) -> Entry {
    let tags: BTreeSet<String> = report.items.iter().map(|o| if o.tag.is_empty() { o.task.clone() } else { o.tag.clone() }).filter(|t| !t.is_empty()).collect();
    let failed = report.count("failed");
    let why = report.items.iter().find(|o| o.result == "failed").map(|o| o.error.clone()).unwrap_or_default();
    let items: Vec<serde_json::Value> = report
        .items
        .iter()
        .map(|o| json!({"pid": o.pid, "cmd": clip(&o.cmd, 120), "task": o.task, "tag": o.tag, "result": o.result}))
        .collect();
    Entry {
        at,
        kind: KIND.to_string(),
        who: who.to_string(),
        task: report.items.first().map(|o| o.task.clone()).unwrap_or_default(),
        tag: report.items.first().map(|o| o.tag.clone()).unwrap_or_default(),
        name: report.items.first().map(|o| clip(&o.cmd, 80)).unwrap_or_default(),
        ok: failed == 0,
        why,
        data: json!({
            "action": "stop",
            "count": report.items.len(),
            "term": report.count("term"),
            "kill": report.count("kill"),
            "gone": report.count("gone"),
            "refused": report.count("refused"),
            "failed": failed,
            "tags": tags,
            "items": items,
        }),
        ..Entry::default()
    }
}

pub fn switch_entry(at: u64, on: bool) -> Entry {
    Entry {
        at,
        kind: KIND.to_string(),
        who: crate::runners_log::HUMAN.to_string(),
        ok: true,
        data: json!({"action": "switch", "on": on}),
        ..Entry::default()
    }
}

/// One pass of the automatic cleaning: the leftovers that have been seen for the grace time (shells stay for the
/// human), stopped and written to the journal. Returns how many were stopped.
pub async fn auto_pass(app: &Shared) -> usize {
    if !auto_on(app) {
        return 0;
    }
    let now = now_ms();
    let grace = grace_ms();
    let asked: Vec<Target> = {
        let st = state().lock().unwrap_or_else(|e| e.into_inner());
        let Some(last) = &st.last else { return 0 };
        last.strays
            .iter()
            .filter(|s| !s.shell && now.saturating_sub(st.first_seen.get(&(s.pid, s.start)).copied().unwrap_or(now)) >= grace)
            .map(|s| Target { pid: s.pid, start: s.start })
            .collect()
    };
    if asked.is_empty() {
        return 0;
    }
    let board = {
        let snap = app.snapshot.read().await;
        board_of(&snap)
    };
    let report = stop(sys().as_ref(), &board, &asked, term_wait_ms()).await;
    if report.items.iter().all(|o| matches!(o.result, "gone" | "refused")) {
        return 0;
    }
    app.db.log_add(JOURNAL_KEY, &stopped_entry(now_ms(), crate::runners_log::AUTO_ACTOR, &report));
    crate::app::journal_note(app, "strays", "runner", &format!("автоуборка: остановлено процессов завершённых задач — {}", report.stopped())).await;
    scan(app).await;
    report.stopped()
}

/// The loop: a look every minute, a journal line when new leftovers appear, the automatic cleaning when it is on.
pub async fn run(app: Shared) {
    tokio::time::sleep(Duration::from_millis(env_ms("HERDR_PLANNER_STRAY_FIRST_MS", 0, 5000))).await;
    loop {
        let found = scan(&app).await;
        let fresh: Vec<&Stray> = {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            let new: Vec<&Stray> = found.strays.iter().filter(|s| !st.announced.contains(&(s.pid, s.start))).collect();
            for s in &new {
                st.announced.insert((s.pid, s.start));
            }
            new
        };
        if !fresh.is_empty() {
            let tasks: BTreeSet<&str> = fresh.iter().map(|s| if s.tag.is_empty() { s.task.as_str() } else { s.tag.as_str() }).collect();
            let list: Vec<&str> = tasks.into_iter().collect();
            crate::app::journal_note(
                &app,
                "strays",
                "runner",
                &format!("остались процессы завершённых задач: {} ({})", fresh.len(), list.join(", ")),
            )
            .await;
        }
        auto_pass(&app).await;
        tokio::time::sleep(scan_every()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const WT: &str = "/home/me/.herdr/worktrees";

    fn task(repo: &str, n: u64, state: &'static str) -> TaskInfo {
        TaskInfo {
            id: format!("{repo}/{n}"),
            tag: format!("PLN-{n}"),
            repo: repo.into(),
            n,
            worktree: format!("{WT}/{repo}/work-{n}-omp"),
            state,
        }
    }

    fn board() -> Board {
        Board {
            ok: true,
            tasks: vec![task("herdr-planner", 7, "done"), task("herdr-planner", 8, "active"), task("herdr-planner", 9, "archived"), task("herdr-planner", 20, "active")],
            repos: vec!["herdr-planner".into()],
            roots: vec![PathBuf::from(WT)],
            keep: vec![PathBuf::from("/home/me/Projects/herdr-planner")],
        }
    }

    fn proc(pid: u32, ppid: u32, cwd: &str, cmd: &str) -> Proc {
        Proc { pid, ppid, pgid: pid, uid: Some(1000), comm: cmd.split(' ').next().unwrap_or("").rsplit('/').next().unwrap_or("").into(), cmd: cmd.into(), cwd: cwd.into(), rss: 1 << 20, age_ms: 60_000, start: 100 + pid as u64, ..Proc::default() }
    }

    fn table(procs: Vec<Proc>) -> Table {
        Table { uid: 1000, pid: 5, procs }
    }

    #[test]
    fn a_finished_tasks_agent_and_helpers_are_leftovers_by_folder_not_by_name() {
        let t = table(vec![
            proc(10, 1, &format!("{WT}/herdr-planner/work-7-omp"), "omp --resume=abc"),
            proc(11, 10, &format!("{WT}/herdr-planner/work-7-omp"), "node /x/rea.mjs mcp"),
            proc(12, 11, &format!("{WT}/herdr-planner/work-7-omp/src/deep"), "omp __omp_worker_daemon_broker"),
            proc(13, 1, &format!("{WT}/herdr-planner/work-9-omp"), "python3 serve.py"),
        ]);
        let f = find(&t, &board());
        assert_eq!(f.iter().map(|s| s.pid).collect::<Vec<_>>(), vec![10, 11, 12, 13]);
        assert_eq!(f.iter().map(|s| s.depth).collect::<Vec<_>>(), vec![0, 1, 2, 0]);
        assert_eq!(f[0].why, "done");
        assert_eq!(f[0].tag, "PLN-7");
        assert_eq!(f[3].why, "archived");
    }

    #[test]
    fn what_is_in_work_or_outside_a_board_copy_or_foreign_is_never_listed() {
        let mut other = proc(20, 1, &format!("{WT}/herdr-planner/work-7-omp"), "node other-user");
        other.uid = Some(0);
        let t = table(vec![
            proc(21, 1, &format!("{WT}/herdr-planner/work-8-omp"), "omp --auto-approve"),
            proc(22, 1, &format!("{WT}/herdr-planner/work-8-omp/web"), "vite"),
            proc(23, 1, "/home/me/Projects/herdr-planner", "node outside.js"),
            proc(24, 1, &format!("{WT}/APICache/hp-apicache-t-0161"), "claude"),
            proc(25, 1, &format!("{WT}/herdr-planner/scratch"), "sleep 9"),
            proc(26, 1, &format!("{WT}/unknown-repo/work-3"), "sleep 9"),
            proc(27, 1, &format!("{WT}/herdr-planner/work-12-omp"), "not-the-copy-of-7"),
            // a task started after the board was read: its number is above the newest one known
            proc(29, 1, &format!("{WT}/herdr-planner/work-70-omp"), "omp --auto-approve"),
            proc(28, 1, "relative/path", "sleep 9"),
            other,
        ]);
        let f = find(&t, &board());
        // work-12 has no task (a number the board has passed) in a known repository: it is a copy the board no longer has.
        assert_eq!(f.iter().map(|s| s.pid).collect::<Vec<_>>(), vec![27]);
        assert_eq!(f[0].why, "gone");
        assert_eq!(f[0].task, "herdr-planner/12");
    }

    #[test]
    fn nothing_is_judged_from_a_board_that_failed_or_is_old() {
        let mut b = board();
        let t = table(vec![proc(10, 1, &format!("{WT}/herdr-planner/work-7-omp"), "omp --resume=abc")]);
        assert_eq!(find(&t, &b).len(), 1);
        b.ok = false;
        assert!(find(&t, &b).is_empty());
    }

    #[test]
    fn a_copy_without_a_task_is_a_leftover_only_by_the_boards_own_name() {
        let mut b = board();
        b.tasks.retain(|t| t.n != 9);
        let t = table(vec![
            proc(30, 1, &format!("{WT}/herdr-planner/work-9-omp"), "node rea.mjs mcp"),
            proc(31, 1, &format!("{WT}/herdr-planner/worktree-9"), "node thing"),
            proc(32, 1, &format!("{WT}/herdr-planner/work-x"), "node thing"),
        ]);
        let f = find(&t, &b);
        assert_eq!(f.iter().map(|s| s.pid).collect::<Vec<_>>(), vec![30]);
        assert_eq!(f[0].why, "gone");
    }

    #[test]
    fn the_planner_its_line_and_pid_one_are_safe_wherever_they_stand() {
        let wd = format!("{WT}/herdr-planner/work-7-omp");
        let t = Table {
            uid: 1000,
            pid: 40,
            procs: vec![
                proc(1, 0, &wd, "init"),
                proc(38, 1, &wd, "omp"),
                proc(39, 38, &wd, "bash"),
                proc(40, 39, &wd, "herdr-planner"),
                proc(41, 40, &wd, "herdr-projects chat"),
                proc(50, 1, &wd, "node rea.mjs mcp"),
            ],
        };
        let f = find(&t, &board());
        assert_eq!(f.iter().map(|s| s.pid).collect::<Vec<_>>(), vec![50]);
    }

    #[test]
    fn a_task_record_that_names_the_humans_checkout_is_not_believed() {
        let mut b = board();
        b.tasks.push(TaskInfo { worktree: "/home/me/Projects/herdr-planner".into(), ..task("herdr-planner", 3, "done") });
        b.tasks.push(TaskInfo { worktree: "/home/me".into(), ..task("herdr-planner", 4, "done") });
        b.tasks.push(TaskInfo { worktree: "/home/me/elsewhere/copy-5".into(), ..task("herdr-planner", 5, "done") });
        let t = table(vec![
            proc(70, 1, "/home/me/Projects/herdr-planner/web", "vite"),
            proc(71, 1, "/home/me/other", "sleep 9"),
            proc(72, 1, "/home/me/elsewhere/copy-5/src", "node rea.mjs mcp"),
        ]);
        let f = find(&t, &b);
        assert_eq!(f.iter().map(|s| s.pid).collect::<Vec<_>>(), vec![72]);
        assert_eq!(f[0].tag, "PLN-5");
    }

    #[test]
    fn a_shell_is_marked() {
        let t = table(vec![proc(60, 1, &format!("{WT}/herdr-planner/work-7-omp"), "zsh -l")]);
        let f = find(&t, &board());
        assert!(f[0].shell);
    }

    #[test]
    fn stat_lines_with_odd_names_are_read() {
        let line = "1234 (Web Content (x)) S 1000 1234 1234 0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 987654 1 2 3";
        let (comm, state, ppid, pgrp, start) = parse_stat(line).unwrap();
        assert_eq!((comm.as_str(), state, ppid, pgrp, start), ("Web Content (x)", 'S', 1000, 1234, 987654));
        assert!(parse_stat("garbage").is_none());
        assert_eq!(parse_rss("Name: x\nVmRSS:\t  2048 kB\n"), 2048 * 1024);
        assert_eq!(cmdline_of(b"node\0/x/rea.mjs\0mcp\0", "node"), "node /x/rea.mjs mcp");
        assert_eq!(cmdline_of(b"", "kworker"), "[kworker]");
        assert_eq!(number_of("work-12"), Some(12));
        assert_eq!(number_of("work-12-omp"), Some(12));
        assert_eq!(number_of("work-12x"), None);
        assert_eq!(number_of("work-"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_real_table_finds_a_child_by_its_folder_and_never_the_planner() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let copy = root.join("alpha/work-5-omp");
        fs::create_dir_all(&copy).unwrap();
        let mut child = std::process::Command::new("sleep").arg("30").current_dir(&copy).spawn().unwrap();
        let board = Board {
            ok: true,
            tasks: vec![TaskInfo { id: "alpha/5".into(), tag: "ALP-5".into(), repo: "alpha".into(), n: 5, worktree: String::new(), state: "done" }],
            repos: vec!["alpha".into()],
            roots: vec![root],
            keep: vec![],
        };
        // Between fork and exec the child still wears its parent's command line: wait for the real one.
        let end = std::time::Instant::now() + Duration::from_secs(5);
        let mut table = loop {
            let t = Real.read().unwrap();
            if t.procs.iter().any(|p| p.pid == child.id() && p.cmd.contains("sleep 30")) || std::time::Instant::now() > end {
                break t;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // The planner's own children are safe (the board's commands run in a copy)…
        let own = find(&table, &board);
        // …and the same process is a leftover once it is nobody's child of ours.
        let child_row = table.procs.iter_mut().find(|p| p.pid == child.id()).expect("the child is in the table");
        child_row.ppid = 1;
        let found = find(&table, &board);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(table.pid, std::process::id());
        assert!(own.iter().all(|s| s.pid != child.id()));
        let mine = found.iter().find(|s| s.pid == child.id()).expect("the child is a leftover");
        assert_eq!((mine.why, mine.tag.as_str()), ("done", "ALP-5"));
        assert!(mine.cmd.contains("sleep 30"), "{}", mine.cmd);
        assert!(mine.start > 0 && mine.age_ms < 60_000);
        assert!(found.iter().all(|s| s.pid != std::process::id()));
        // The signal helper refuses what it must before it reaches the kernel.
        assert!(Real.signal(1, false).is_err());
        assert!(Real.signal(0, true).is_err());
    }

    fn fixture(dir: &Path, procs: Vec<Proc>) -> Fixture {
        let file = dir.join("procs.json");
        fs::write(&file, serde_json::to_string(&FixtureFile { uid: 1000, pid: 5, procs }).unwrap()).unwrap();
        Fixture(file)
    }

    fn targets(fx: &Fixture, board: &Board) -> Vec<Target> {
        find(&fx.read().unwrap(), board).iter().map(|s| Target { pid: s.pid, start: s.start }).collect()
    }

    fn signals(fx: &Fixture) -> Vec<String> {
        fs::read_to_string(format!("{}.signals", fx.0.display())).unwrap_or_default().lines().map(String::from).collect()
    }

    #[tokio::test]
    async fn a_tree_gets_sigterm_parents_first_and_sigkill_only_when_it_ignores_it() {
        let dir = tempfile::tempdir().unwrap();
        let wd = format!("{WT}/herdr-planner/work-7-omp");
        let mut stubborn = proc(12, 10, &wd, "omp __omp_worker_daemon_broker");
        stubborn.ignore_term = true;
        let fx = fixture(
            dir.path(),
            vec![
                proc(11, 10, &wd, "node /x/rea.mjs mcp"),
                proc(10, 1, &wd, "omp --resume=abc"),
                stubborn,
                proc(21, 1, &format!("{WT}/herdr-planner/work-8-omp"), "omp --auto-approve"),
            ],
        );
        let b = board();
        let asked = targets(&fx, &b);
        assert_eq!(asked.iter().map(|t| t.pid).collect::<Vec<_>>(), vec![10, 11, 12]);
        let r = stop(&fx, &b, &asked, 150).await;
        assert_eq!(r.items.iter().map(|o| (o.pid, o.result)).collect::<Vec<_>>(), vec![(10, "term"), (11, "term"), (12, "kill")]);
        assert_eq!(r.left, 0);
        // The agent is signalled before its helpers; the stubborn one gets TERM then KILL; the task in work is untouched.
        assert_eq!(signals(&fx), vec!["TERM 10", "TERM 11", "TERM 12", "KILL 12"]);
        let left: Vec<u32> = fx.read().unwrap().procs.iter().map(|p| p.pid).collect();
        assert_eq!(left, vec![21]);
    }

    #[tokio::test]
    async fn a_process_that_is_no_longer_a_leftover_or_not_the_same_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let wd = format!("{WT}/herdr-planner/work-7-omp");
        let fx = fixture(
            dir.path(),
            vec![
                proc(10, 1, &wd, "omp --resume=abc"),
                proc(21, 1, &format!("{WT}/herdr-planner/work-8-omp"), "omp --auto-approve"),
                proc(30, 1, &wd, "node reused"),
            ],
        );
        let b = board();
        let asked = vec![
            Target { pid: 10, start: 110 },
            // a task in work, even when the page asks for it by its exact pid and start
            Target { pid: 21, start: 121 },
            // the pid was reused by another process
            Target { pid: 30, start: 1 },
            // not there at all
            Target { pid: 99, start: 1 },
            // and twice in the list: once
            Target { pid: 10, start: 110 },
        ];
        let r = stop(&fx, &b, &asked, 100).await;
        let got: Vec<(u32, &str)> = r.items.iter().map(|o| (o.pid, o.result)).collect();
        assert!(got.contains(&(10, "term")));
        assert!(got.contains(&(21, "refused")));
        assert!(got.contains(&(30, "refused")));
        assert!(got.contains(&(99, "gone")));
        assert_eq!(got.len(), 4);
        assert_eq!(signals(&fx), vec!["TERM 10"]);
        assert!(fx.read().unwrap().procs.iter().any(|p| p.pid == 21));
        assert!(fx.read().unwrap().procs.iter().any(|p| p.pid == 30));
    }

    #[test]
    fn the_journal_row_counts_what_happened() {
        let report = Report {
            items: vec![
                Outcome { pid: 1, cmd: "omp".into(), task: "a/7".into(), tag: "PLN-7".into(), result: "term", error: String::new() },
                Outcome { pid: 2, cmd: "node".into(), task: "a/7".into(), tag: "PLN-7".into(), result: "kill", error: String::new() },
                Outcome { pid: 3, cmd: "node".into(), task: "a/9".into(), tag: "PLN-9".into(), result: "failed", error: "EPERM".into() },
            ],
            left: 1,
        };
        let e = stopped_entry(5, "auto", &report);
        assert_eq!(e.kind, "stray");
        assert_eq!(e.who, "auto");
        assert!(!e.ok);
        assert_eq!(e.why, "EPERM");
        assert_eq!(e.data["term"], 1);
        assert_eq!(e.data["kill"], 1);
        assert_eq!(e.data["failed"], 1);
        assert_eq!(e.data["tags"], json!(["PLN-7", "PLN-9"]));
        assert_eq!(report.stopped(), 2);
    }
}
