//! Hints for the person writing to an agent: the commands a program knows (the `/` list) and the things
//! worth naming in a message (the `@` list — tasks, universes, shipyards, files and folders of the
//! repositories, pictures of the chat).
//!
//! A message reaches the Coordinator's program word for word, so a word that starts with `/` is one of
//! THAT program's own commands. The list is read from the program's own files on this machine: built-in
//! commands from a short table, skills (`SKILL.md`) and custom commands (`*.md` with a description in
//! their front matter) from the folders the programs keep. A program whose commands cannot be learnt has
//! no hints at all, and its input works as it always did.
//!
//! The files of the repositories are found by name under the folders the board knows (`repos.json`): the
//! person puts a full path into the message, because the Coordinator works in its own folder, not in the
//! repository.

use crate::app::Shared;
use crate::config::{self, AppConfig};
use crate::server::error;
use crate::station;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a list is kept before it is read again: a skill or a command installed in the meantime shows
/// up at the next look, and typing never reads a folder tree twice.
const TTL: Duration = Duration::from_secs(60);
/// The longest description kept from a skill or a command file.
const MAX_DESC: usize = 160;
/// How deep the custom commands of a folder are looked for (`commands/review/security.md`).
const MAX_CMD_DEPTH: usize = 3;
/// The files and folders of one repository that are indexed (a big build folder may hold more).
const MAX_ENTRIES: usize = 40_000;
/// How deep a repository is walked.
const MAX_DEPTH: usize = 16;
/// Folders that are never worth naming in a message: they hold what the tools build or cache.
const SKIP: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__", ".cache",
    ".next", ".svelte-kit", "coverage", ".tox", ".mypy_cache", ".pytest_cache",
];

/* ───────── the commands of a program ───────── */

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cmd {
    /// The word after the slash, without it (`compact`).
    pub name: String,
    /// The words about it: a key of the page's dictionary for a built-in (`cmd.claude.compact`), or the
    /// text from the skill's or the command's own file.
    pub desc: String,
    /// `builtin`, `skill` or `command`.
    pub kind: &'static str,
}

/// Where the programs keep their files on this machine. Tests point every field at a temporary folder.
#[derive(Debug, Clone)]
pub struct Roots {
    /// Claude Code's home: `CLAUDE_CONFIG_DIR` or `~/.claude`.
    pub claude: PathBuf,
    /// `CODEX_HOME` or `~/.codex`.
    pub codex: PathBuf,
    /// `~/.omp` (the agent's folder is `agent` inside it).
    pub omp: PathBuf,
    /// `~/.pi` (the agent's folder is `agent` inside it).
    pub pi: PathBuf,
}

impl Roots {
    pub fn live() -> Roots {
        let home = config::home();
        // The variables these programs read themselves; the rest live in the home folder.
        let from_env = |var: &str, fallback: PathBuf| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or(fallback)
        };
        Roots {
            claude: from_env("CLAUDE_CONFIG_DIR", home.join(".claude")),
            codex: from_env("CODEX_HOME", home.join(".codex")),
            omp: home.join(".omp"),
            pi: home.join(".pi"),
        }
    }
}

/// Claude Code's own commands, as its TUI documents them. The description is a key of the page's
/// dictionary: the page speaks the language of its reader.
const CLAUDE: &[(&str, &str)] = &[
    ("add-dir", "cmd.claude.addDir"),
    ("agents", "cmd.claude.agents"),
    ("bug", "cmd.claude.bug"),
    ("clear", "cmd.claude.clear"),
    ("compact", "cmd.claude.compact"),
    ("config", "cmd.claude.config"),
    ("context", "cmd.claude.context"),
    ("cost", "cmd.claude.cost"),
    ("doctor", "cmd.claude.doctor"),
    ("exit", "cmd.claude.exit"),
    ("export", "cmd.claude.export"),
    ("help", "cmd.claude.help"),
    ("hooks", "cmd.claude.hooks"),
    ("ide", "cmd.claude.ide"),
    ("init", "cmd.claude.init"),
    ("login", "cmd.claude.login"),
    ("logout", "cmd.claude.logout"),
    ("mcp", "cmd.claude.mcp"),
    ("memory", "cmd.claude.memory"),
    ("model", "cmd.claude.model"),
    ("permissions", "cmd.claude.permissions"),
    ("resume", "cmd.claude.resume"),
    ("review", "cmd.claude.review"),
    ("rewind", "cmd.claude.rewind"),
    ("status", "cmd.claude.status"),
    ("statusline", "cmd.claude.statusline"),
    ("terminal-setup", "cmd.claude.terminalSetup"),
    ("todos", "cmd.claude.todos"),
    ("vim", "cmd.claude.vim"),
];

/// Codex's own commands, as its TUI documents them.
const CODEX: &[(&str, &str)] = &[
    ("approvals", "cmd.codex.approvals"),
    ("compact", "cmd.codex.compact"),
    ("diff", "cmd.codex.diff"),
    ("exit", "cmd.codex.exit"),
    ("init", "cmd.codex.init"),
    ("mcp", "cmd.codex.mcp"),
    ("mention", "cmd.codex.mention"),
    ("model", "cmd.codex.model"),
    ("new", "cmd.codex.new"),
    ("review", "cmd.codex.review"),
    ("status", "cmd.codex.status"),
    ("undo", "cmd.codex.undo"),
];

/// The commands a program knows, read from its files. `dir` is the folder the agent works in (the
/// Coordinator's, or a task's worktree): its own `.claude` counts too.
pub fn commands_at(roots: &Roots, program: &str, dir: Option<&Path>) -> Vec<Cmd> {
    let mut out: Vec<Cmd> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut builtin = |table: &[(&str, &str)]| {
        for (name, desc) in table {
            push(
                &mut out,
                &mut seen,
                Cmd { name: (*name).to_string(), desc: (*desc).to_string(), kind: "builtin" },
            );
        }
    };
    match program {
        "claude" => {
            builtin(CLAUDE);
            skills_in(&roots.claude.join("skills"), None, &mut out, &mut seen);
            commands_in(&roots.claude.join("commands"), "", 0, &mut out, &mut seen);
            for (name, plugin) in plugin_roots(&roots.claude) {
                // Claude Code calls a plugin's skills and commands under the plugin's own name.
                skills_in(&plugin.join("skills"), Some(&name), &mut out, &mut seen);
                commands_in(&plugin.join("commands"), &name, 0, &mut out, &mut seen);
            }
            if let Some(dir) = dir {
                skills_in(&dir.join(".claude/skills"), None, &mut out, &mut seen);
                commands_in(&dir.join(".claude/commands"), "", 0, &mut out, &mut seen);
            }
        }
        "codex" => {
            builtin(CODEX);
            commands_in(&roots.codex.join("prompts"), "", 0, &mut out, &mut seen);
        }
        "omp" => skills_in(&roots.omp.join("agent/skills"), None, &mut out, &mut seen),
        "pi" => skills_in(&roots.pi.join("agent/skills"), None, &mut out, &mut seen),
        // A program whose commands are not known here: no hints (the field works as it always did).
        _ => {}
    }
    out
}

fn push(out: &mut Vec<Cmd>, seen: &mut HashSet<String>, c: Cmd) {
    if c.name.is_empty() || !c.name.chars().all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | ':')) {
        return;
    }
    if seen.insert(c.name.clone()) {
        out.push(c);
    }
}

/// The skills of a folder: one sub-folder per skill, its words in `SKILL.md`. Skills are often links to a
/// folder kept elsewhere (`~/.agents/skills`), so the entry's own type is not trusted: the link is followed.
fn skills_in(base: &Path, ns: Option<&str>, out: &mut Vec<Cmd>, seen: &mut HashSet<String>) {
    let Ok(read) = std::fs::read_dir(base) else { return };
    let mut dirs: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_dir()))
        .collect();
    dirs.sort();
    for dir in dirs {
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else { continue };
        let (name, desc) = front_matter(&text);
        let name = clean_name(&if name.is_empty() { folder } else { name });
        let name = match ns {
            Some(p) if !p.is_empty() => format!("{p}:{name}"),
            _ => name,
        };
        push(out, seen, Cmd { name, desc: clean_desc(&desc), kind: "skill" });
    }
}

/// Custom commands of a folder: every `*.md` under it, its path inside the folder making the name
/// (`review/security.md` → `review:security`), the description from its front matter. A command that is a
/// link to a file is read as well.
fn commands_in(base: &Path, prefix: &str, depth: usize, out: &mut Vec<Cmd>, seen: &mut HashSet<String>) {
    if depth > MAX_CMD_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(base) else { return };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let Ok(kind) = std::fs::metadata(&path) else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if kind.is_dir() {
            let next = if prefix.is_empty() { name.clone() } else { format!("{prefix}:{name}") };
            commands_in(&path, &next, depth + 1, out, seen);
            continue;
        }
        if !kind.is_file() || !name.ends_with(".md") {
            continue;
        }
        let stem = name.trim_end_matches(".md");
        if stem.is_empty() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let (fm_name, fm_desc) = front_matter(&text);
        let own = if fm_name.is_empty() { stem.to_string() } else { clean_name(&fm_name) };
        let full = if prefix.is_empty() { own } else { format!("{prefix}:{own}") };
        let desc = if fm_desc.is_empty() { first_line(&text) } else { fm_desc };
        push(out, seen, Cmd { name: full, desc: clean_desc(&desc), kind: "command" });
    }
}

/// The folders of the plugins Claude Code has installed, each with the name it is called by (Claude Code
/// namespaces a plugin's skills and commands with it: `/superpowers:brainstorm`).
///
/// `plugins/installed_plugins.json` has two shapes: newer files map a plugin's id (`name@marketplace`) to
/// the copies installed for each scope, each with its `installPath`; older ones list a marketplace's
/// plugins by name. A plugin without an install path is looked for in its marketplace's folder.
fn plugin_roots(claude: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let file = claude.join("plugins/installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(&file) else { return out };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else { return out };
    let Some(plugins) = doc.get("plugins").and_then(|p| p.as_object()) else { return out };
    for (key, listed) in plugins {
        // `name@marketplace` (the current file) or a marketplace's name (the older one).
        let (key_name, key_market) = match key.split_once('@') {
            Some((name, market)) => (name.to_string(), market.to_string()),
            None => (key.clone(), String::new()),
        };
        let mut entries: Vec<(String, String, Option<String>)> = Vec::new();
        match listed {
            serde_json::Value::Array(items) => {
                for item in items {
                    let serde_json::Value::Object(o) = item else { continue };
                    match o.get("name").and_then(|v| v.as_str()).filter(|n| !n.is_empty()) {
                        // An older file: the key is the marketplace, the item names its plugin.
                        Some(name) => entries.push((name.to_string(), key.clone(), install_path(o))),
                        // The current file: the key is `name@marketplace`, the item is one installed copy.
                        None => entries.push((key_name.clone(), key_market.clone(), install_path(o))),
                    }
                }
            }
            serde_json::Value::Object(map) => {
                for (name, item) in map {
                    entries.push((name.clone(), key.clone(), item.as_object().and_then(install_path)));
                }
            }
            _ => {}
        }
        for (name, market, path) in entries {
            for candidate in plugin_candidates(claude, &market, &name, path.as_deref()) {
                if candidate.is_dir() && !out.iter().any(|(_, p)| *p == candidate) {
                    out.push((name.clone(), candidate));
                }
            }
        }
    }
    out
}

/// An entry's `installPath` (older files used `install_path` or `path`).
fn install_path(o: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    o.get("installPath")
        .or_else(|| o.get("install_path"))
        .or_else(|| o.get("path"))
        .and_then(|v| v.as_str())
        .filter(|p| !p.trim().is_empty())
        .map(str::to_string)
}

fn plugin_candidates(claude: &Path, market: &str, name: &str, path: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = path.filter(|p| !p.trim().is_empty()) {
        out.push(config::expand_home(p));
    }
    if !market.is_empty() && !name.is_empty() {
        let base = claude.join("plugins/marketplaces").join(market);
        out.push(base.join("plugins").join(name));
        out.push(base.join("external_plugins").join(name));
    }
    out
}

/// The front matter of a Markdown file: `name` and `description` between `---` lines (a `>` or `|`
/// block of the description is joined into one line).
fn front_matter(text: &str) -> (String, String) {
    let Some(rest) = text.strip_prefix("---") else { return (String::new(), String::new()) };
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n')).unwrap_or(rest);
    let Some(end) = rest.find("\n---") else { return (String::new(), String::new()) };
    let block = &rest[..end];
    let mut name = String::new();
    let mut desc = String::new();
    let mut lines = block.lines().peekable();
    while let Some(line) = lines.next() {
        let (key, value) = match line.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        let key = key.trim().to_ascii_lowercase();
        if key != "name" && key != "description" {
            continue;
        }
        let mut value = value.trim().to_string();
        if value == ">" || value == "|" || value == ">-" || value == "|-" {
            let mut parts: Vec<String> = Vec::new();
            while let Some(next) = lines.peek() {
                if !next.starts_with(' ') && !next.starts_with('\t') {
                    break;
                }
                parts.push(next.trim().to_string());
                lines.next();
            }
            value = parts.join(" ");
        }
        let value = unquote(&value);
        if key == "name" && name.is_empty() {
            name = value;
        } else if key == "description" && desc.is_empty() {
            desc = value;
        }
    }
    (name, desc)
}

fn unquote(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 2 {
        let first = t.chars().next().unwrap_or(' ');
        let last = t.chars().last().unwrap_or(' ');
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return t[first.len_utf8()..t.len() - last.len_utf8()].trim().to_string();
        }
    }
    t.to_string()
}

/// The first line of a command file that is not blank, not front matter and not a heading.
fn first_line(text: &str) -> String {
    let body = match text.strip_prefix("---") {
        Some(rest) => {
            let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n')).unwrap_or(rest);
            match rest.find("\n---") {
                Some(i) => rest[i + 4..].trim_start_matches(['\r', '\n']),
                None => "",
            }
        }
        None => text,
    };
    for line in body.lines() {
        let t = line.trim();
        if !t.is_empty() && !t.starts_with('#') && !t.starts_with("---") {
            return t.to_string();
        }
    }
    String::new()
}

fn clean_name(v: &str) -> String {
    v.trim().trim_start_matches('/').replace(' ', "-")
}

fn clean_desc(v: &str) -> String {
    let one = v.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= MAX_DESC {
        return one;
    }
    let cut: String = one.chars().take(MAX_DESC - 1).collect();
    format!("{}…", cut.trim_end())
}

/* ───────── the files and folders of the repositories ───────── */

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileHit {
    pub name: String,
    /// The full path: what goes into the message (the agent works in another folder).
    pub path: String,
    /// The path inside the repository.
    pub rel: String,
    pub repo: String,
    pub dir: bool,
}

#[derive(Debug, Clone)]
struct Entry {
    rel: String,
    /// The same path in lowercase, and the name's, kept from indexing time: typing must not lowercase a whole tree.
    lower: String,
    name: String,
    name_lower: String,
    dir: bool,
}

/// Finds files and folders by name under the repositories of the board. `roots` are `(repo, folder)`.
/// Every word of the query must be in the path; the name that starts with it comes first.
pub fn search_files(roots: &[(String, String)], query: &str, limit: usize) -> Vec<FileHit> {
    let limit = limit.clamp(1, 30);
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let terms: Vec<String> = q.split_whitespace().take(4).map(str::to_string).collect();
    let mut scored: Vec<(u32, usize, String, FileHit)> = Vec::new();
    for (repo, root) in roots {
        let dir = Path::new(root);
        if !dir.is_dir() {
            continue;
        }
        for e in index_of(dir).iter() {
            if !terms.iter().all(|t| e.lower.contains(t.as_str())) {
                continue;
            }
            let in_name = terms.iter().filter(|t| e.name_lower.contains(t.as_str())).count();
            let score = if e.name_lower.starts_with(&q) {
                0
            } else if e.name_lower.contains(&q) {
                1
            } else if e.lower.contains(&q) {
                2
            } else {
                3 + (terms.len() - in_name) as u32
            };
            scored.push((
                score,
                e.rel.len(),
                e.lower.clone(),
                FileHit {
                    name: e.name.clone(),
                    path: dir.join(&e.rel).display().to_string(),
                    rel: e.rel.clone(),
                    repo: repo.clone(),
                    dir: e.dir,
                },
            ));
        }
    }
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.rel.cmp(&b.3.rel))
    });
    scored.into_iter().take(limit).map(|x| x.3).collect()
}

/// The files and folders of one repository, indexed once in a while (see [`TTL`]).
fn index_of(root: &Path) -> Arc<Vec<Entry>> {
    let key = root.display().to_string();
    {
        let cache = FILES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, list)) = cache.get(&key) {
            if at.elapsed() < TTL {
                return Arc::clone(list);
            }
        }
    }
    let list = Arc::new(walk(root));
    let mut cache = FILES.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 32 {
        cache.clear();
    }
    cache.insert(key, (Instant::now(), Arc::clone(&list)));
    list
}

fn walk(root: &Path) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if out.len() >= MAX_ENTRIES {
                break;
            }
            let Ok(kind) = e.file_type() else { continue };
            // A link may point back into the tree (or away from it): never followed.
            if kind.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = match dir.strip_prefix(root) {
                Ok(p) if !p.as_os_str().is_empty() => format!("{}/{}", p.display(), name),
                _ => name.clone(),
            };
            if kind.is_dir() {
                if SKIP.contains(&name.as_str()) {
                    continue;
                }
                out.push(Entry { lower: rel.to_lowercase(), name: name.clone(), name_lower: name.to_lowercase(), rel, dir: true });
                stack.push((e.path(), depth + 1));
            } else if kind.is_file() {
                out.push(Entry { lower: rel.to_lowercase(), name: name.clone(), name_lower: name.to_lowercase(), rel, dir: false });
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

type FilesCache = HashMap<String, (Instant, Arc<Vec<Entry>>)>;
type CmdsCache = HashMap<String, (Instant, Vec<Cmd>)>;

static FILES: std::sync::LazyLock<Mutex<FilesCache>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static CMDS: std::sync::LazyLock<Mutex<CmdsCache>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn cached_commands(program: &str, dir: Option<&Path>) -> Vec<Cmd> {
    let key = format!("{program}|{}", dir.map(|d| d.display().to_string()).unwrap_or_default());
    let mut cache = CMDS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, list)) = cache.get(&key) {
        if at.elapsed() < TTL {
            return list.clone();
        }
    }
    let list = commands_at(&Roots::live(), program, dir);
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(key, (Instant::now(), list.clone()));
    list
}

/* ───────── the endpoints ───────── */

#[derive(Deserialize)]
pub struct SlashQuery {
    program: Option<String>,
    /// The folder the agent works in: its own `.claude` counts. Empty — the Coordinator's folder.
    dir: Option<String>,
}

/// `GET /api/hints/slash?program=claude&dir=`: the commands a program knows, for its `/` list.
pub async fn slash_get(State(app): State<Shared>, Query(q): Query<SlashQuery>) -> Response {
    let program = q.program.unwrap_or_default();
    let program = program.trim().to_string();
    if program.is_empty() {
        return Json(json!({"ok": true, "program": "", "commands": []})).into_response();
    }
    if !station::PROGRAMS.contains(&program.as_str()) {
        return error(StatusCode::BAD_REQUEST, "there is no such program");
    }
    let cfg = app.cfg.clone();
    let dir = agent_dir(&cfg, q.dir.as_deref());
    let wanted = program.clone();
    let list = tokio::task::spawn_blocking(move || cached_commands(&wanted, dir.as_deref()))
        .await
        .unwrap_or_default();
    Json(json!({"ok": true, "program": program, "commands": list})).into_response()
}

/// The folder the agent works in, when the page may name it: the Coordinator's own, a repository, or a
/// worktree. Anything else is not read.
fn agent_dir(cfg: &AppConfig, raw: Option<&str>) -> Option<PathBuf> {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return canonical(&cfg.data_dir.join("coordinator/default/chat"));
    }
    if raw.contains('\0') || raw.len() > 1000 {
        return None;
    }
    let path = canonical(&config::expand_home(raw))?;
    let mut allowed: Vec<PathBuf> = vec![cfg.data_dir.clone(), cfg.projects_dir.clone()];
    for repo in crate::taskmeta::repos(&cfg.data_dir) {
        if !repo.root.trim().is_empty() {
            allowed.push(config::expand_home(&repo.root));
        }
    }
    allowed.push(config::home().join(".herdr/worktrees"));
    if allowed
        .iter()
        .filter_map(|p| p.canonicalize().ok())
        .any(|p| path.starts_with(p))
    {
        Some(path)
    } else {
        None
    }
}

fn canonical(path: &Path) -> Option<PathBuf> {
    let p = path.canonicalize().ok()?;
    p.is_dir().then_some(p)
}

#[derive(Deserialize)]
pub struct FilesQuery {
    q: Option<String>,
    limit: Option<usize>,
}

/// `GET /api/hints/files?q=`: files and folders of the repositories whose path holds the words.
pub async fn files_get(State(app): State<Shared>, Query(q): Query<FilesQuery>) -> Response {
    let text = q.q.unwrap_or_default();
    if text.chars().count() > 200 {
        return error(StatusCode::BAD_REQUEST, "the query is too long");
    }
    if text.trim().is_empty() {
        return Json(json!({"ok": true, "items": []})).into_response();
    }
    let roots: Vec<(String, String)> = crate::taskmeta::repos(&app.cfg.data_dir)
        .into_iter()
        .filter(|r| !r.root.trim().is_empty())
        .map(|r| (r.name.clone(), r.root.clone()))
        .collect();
    let limit = q.limit.unwrap_or(20).clamp(1, 30);
    match tokio::task::spawn_blocking(move || search_files(&roots, &text, limit)).await {
        Ok(items) => Json(json!({"ok": true, "items": items})).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/* ───────── tests ───────── */

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let d = std::env::temp_dir().join(format!("hp-hints-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&d).unwrap();
            Scratch(d)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, text).unwrap();
            p
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn roots_at(s: &Scratch) -> Roots {
        Roots {
            claude: s.path().join("claude"),
            codex: s.path().join("codex"),
            omp: s.path().join("omp"),
            pi: s.path().join("pi"),
        }
    }

    fn names(list: &[Cmd]) -> Vec<&str> {
        list.iter().map(|c| c.name.as_str()).collect()
    }

    fn desc_of<'a>(list: &'a [Cmd], name: &str) -> &'a str {
        list.iter().find(|c| c.name == name).map(|c| c.desc.as_str()).unwrap_or("")
    }

    #[test]
    fn claude_has_its_own_commands_then_skills_then_folder_commands() {
        let s = Scratch::new("claude");
        s.write(
            "claude/skills/ui-ux-pro-max/SKILL.md",
            "---\nname: ui-ux-pro-max\ndescription: \"Designing, building and reviewing interfaces\"\n---\n\n# Skill\n",
        );
        s.write("claude/commands/review/security.md", "---\ndescription: Check a diff for leaks\n---\nBody\n");
        s.write("claude/commands/plain.md", "Do the plain thing.\n");
        s.write("chat/.claude/commands/here.md", "---\ndescription: A command of this folder\n---\n");
        let list = commands_at(&roots_at(&s), "claude", Some(&s.path().join("chat")));
        let got = names(&list);
        assert!(got.contains(&"compact"), "{got:?}");
        assert!(got.contains(&"ui-ux-pro-max"), "{got:?}");
        assert!(got.contains(&"review:security"), "{got:?}");
        assert!(got.contains(&"plain"), "{got:?}");
        assert!(got.contains(&"here"), "{got:?}");
        // The built-ins come first, and the description of a built-in is a key of the page's dictionary.
        assert_eq!(list[0].name, "add-dir");
        assert_eq!(list[0].kind, "builtin");
        assert_eq!(list[0].desc, "cmd.claude.addDir");
        assert_eq!(desc_of(&list, "ui-ux-pro-max"), "Designing, building and reviewing interfaces");
        assert_eq!(desc_of(&list, "review:security"), "Check a diff for leaks");
        assert_eq!(desc_of(&list, "plain"), "Do the plain thing.");
        assert_eq!(desc_of(&list, "here"), "A command of this folder");
        // No duplicates, whatever the folders say.
        let mut sorted = got.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), got.len());
    }

    #[test]
    #[cfg(unix)]
    fn a_skill_or_a_command_that_is_a_link_is_read_too() {
        use std::os::unix::fs::symlink;
        let s = Scratch::new("links");
        // The skills of this machine live elsewhere and are linked into `~/.claude/skills`.
        s.write("agents/ui-ux-pro-max/SKILL.md", "---\nname: ui-ux-pro-max\ndescription: Interfaces\n---\n");
        fs::create_dir_all(s.path().join("claude/skills")).unwrap();
        symlink(s.path().join("agents/ui-ux-pro-max"), s.path().join("claude/skills/ui-ux-pro-max")).unwrap();
        // So may a command file, and a whole folder of them.
        s.write("shared/check.md", "---\ndescription: Check it\n---\n");
        s.write("shared/review/security.md", "---\ndescription: Look for leaks\n---\n");
        fs::create_dir_all(s.path().join("claude/commands")).unwrap();
        symlink(s.path().join("shared/check.md"), s.path().join("claude/commands/check.md")).unwrap();
        symlink(s.path().join("shared/review"), s.path().join("claude/commands/review")).unwrap();
        let list = commands_at(&roots_at(&s), "claude", None);
        assert_eq!(desc_of(&list, "ui-ux-pro-max"), "Interfaces");
        assert_eq!(desc_of(&list, "check"), "Check it");
        assert_eq!(desc_of(&list, "review:security"), "Look for leaks");
    }

    #[test]
    fn plugins_leave_their_skills_and_commands_under_their_own_name() {
        let s = Scratch::new("plugins");
        // An older file: a marketplace's plugins by name, one with an install path that is gone.
        s.write(
            "claude/plugins/installed_plugins.json",
            r#"{"version":1,"plugins":{"official":[{"name":"code-review","installPath":"/nope"},{"name":"commit-commands"}]}}"#,
        );
        s.write("claude/plugins/marketplaces/official/plugins/code-review/commands/safety.md", "Review it.\n");
        s.write(
            "claude/plugins/marketplaces/official/plugins/commit-commands/skills/commit/SKILL.md",
            "---\nname: commit\ndescription: Commit the work\n---\n",
        );
        let list = commands_at(&roots_at(&s), "claude", None);
        // A plugin's commands and skills are called under the plugin's name, as Claude Code calls them.
        assert_eq!(desc_of(&list, "code-review:safety"), "Review it.");
        assert_eq!(desc_of(&list, "commit-commands:commit"), "Commit the work");
        assert_eq!(list.iter().filter(|c| c.name == "safety" || c.name == "commit").count(), 0);
    }

    #[test]
    fn the_current_plugin_file_names_each_plugin_by_its_id() {
        let s = Scratch::new("pluginsv2");
        // Claude Code 2.x: the key is `name@marketplace`, the value is one installed copy per scope.
        s.write(
            "claude/plugins/installed_plugins.json",
            r#"{"version":2,"plugins":{"superpowers@claude-plugins-official":[{"scope":"user","installPath":"INSTALL","version":"5.0.0"},{"scope":"project","installPath":"INSTALL"}],"plain@some-market":[{"scope":"user","installPath":"/nope"}]}}"#,
        );
        let install = s.path().join("store/superpowers");
        s.write("store/superpowers/skills/brainstorm/SKILL.md", "---\nname: brainstorm\ndescription: Ideas first\n---\n");
        s.write("store/superpowers/commands/commit.md", "---\ndescription: Commit the work\n---\n");
        s.write("store/superpowers/commands/review/security.md", "Check the diff.\n");
        // The marketplace's folder is the fallback when the install path is not there.
        s.write("claude/plugins/marketplaces/some-market/plugins/plain/commands/run.md", "Run it.\n");
        let text = std::fs::read_to_string(s.path().join("claude/plugins/installed_plugins.json"))
            .unwrap()
            .replace("INSTALL", &install.display().to_string());
        s.write("claude/plugins/installed_plugins.json", &text);
        let list = commands_at(&roots_at(&s), "claude", None);
        assert_eq!(desc_of(&list, "superpowers:brainstorm"), "Ideas first");
        assert_eq!(desc_of(&list, "superpowers:commit"), "Commit the work");
        assert_eq!(desc_of(&list, "superpowers:review:security"), "Check the diff.");
        assert_eq!(desc_of(&list, "plain:run"), "Run it.");
        // Both scopes of one copy are one plugin, not two.
        assert_eq!(list.iter().filter(|c| c.name == "superpowers:commit").count(), 1);
    }

    #[test]
    fn other_programs_have_their_files_and_no_made_up_commands() {
        let s = Scratch::new("others");
        s.write("omp/agent/skills/archify/SKILL.md", "---\nname: archify\ndescription: Diagrams\n---\n");
        s.write("pi/agent/skills/note/SKILL.md", "---\nname: note\ndescription: Write a note\n---\n");
        s.write("codex/prompts/fix.md", "---\ndescription: Fix the bug\n---\n");
        let roots = roots_at(&s);
        let omp = commands_at(&roots, "omp", None);
        assert_eq!(names(&omp), vec!["archify"]);
        assert_eq!(omp[0].kind, "skill");
        let pi = commands_at(&roots, "pi", None);
        assert_eq!(names(&pi), vec!["note"]);
        let codex = commands_at(&roots, "codex", None);
        assert_eq!(desc_of(&codex, "fix"), "Fix the bug");
        assert_eq!(codex[0].kind, "builtin");
        // A program whose commands are not known here: nothing, and its input is left alone.
        for p in ["opencode", "agy", "gemini", "nonsense", ""] {
            assert!(commands_at(&roots, p, None).is_empty(), "{p}");
        }
    }

    #[test]
    fn descriptions_are_one_line_and_short() {
        let s = Scratch::new("desc");
        s.write(
            "claude/skills/long/SKILL.md",
            &format!("---\nname: long\ndescription: >\n  first part\n  second part\n---\n"),
        );
        s.write("claude/skills/wide/SKILL.md", &format!("---\ndescription: {}\n---\n", "word ".repeat(80)));
        let list = commands_at(&roots_at(&s), "claude", None);
        assert_eq!(desc_of(&list, "long"), "first part second part");
        let wide = desc_of(&list, "wide");
        assert!(wide.chars().count() <= MAX_DESC, "{}", wide.chars().count());
        assert!(wide.ends_with('…'));
    }

    #[test]
    fn a_skill_without_a_description_still_lists() {
        let s = Scratch::new("nodesc");
        s.write("claude/skills/bare/SKILL.md", "---\nname: bare\n---\n");
        let list = commands_at(&roots_at(&s), "claude", None);
        assert_eq!(desc_of(&list, "bare"), "");
        assert_eq!(list.iter().filter(|c| c.name == "bare").count(), 1);
    }

    #[test]
    fn the_files_of_the_repositories_are_found_by_name_with_full_paths() {
        let s = Scratch::new("files");
        fs::create_dir_all(s.path().join("repo/src/ui")).unwrap();
        fs::write(s.path().join("repo/src/ui/Chat.tsx"), "x").unwrap();
        fs::write(s.path().join("repo/src/ui/chat.css"), "x").unwrap();
        fs::write(s.path().join("repo/README.md"), "x").unwrap();
        fs::create_dir_all(s.path().join("repo/node_modules/pkg")).unwrap();
        fs::write(s.path().join("repo/node_modules/pkg/chat.js"), "x").unwrap();
        fs::create_dir_all(s.path().join("repo/.git")).unwrap();
        fs::write(s.path().join("repo/.git/chat"), "x").unwrap();
        let roots = vec![("repo".to_string(), s.path().join("repo").display().to_string())];

        let hits = search_files(&roots, "chat", 10);
        let rels: Vec<&str> = hits.iter().map(|h| h.rel.as_str()).collect();
        assert_eq!(rels, vec!["src/ui/chat.css", "src/ui/Chat.tsx"]);
        assert!(hits[0].path.ends_with("repo/src/ui/chat.css"));
        assert_eq!(hits[0].repo, "repo");
        assert!(!hits[0].dir);
        assert!(!rels.iter().any(|r| r.contains("node_modules") || r.starts_with(".git")));

        // A folder is a hit too, and a folder named in the query narrows the files.
        let dir_hit = search_files(&roots, "ui", 10);
        assert!(dir_hit.iter().any(|h| h.rel == "src/ui" && h.dir));

        // Every word of the query must be in the path; the name that starts with it comes first.
        let both = search_files(&roots, "src chat", 10);
        assert_eq!(both.len(), 2);
        let readme = search_files(&roots, "readme", 10);
        assert_eq!(readme.len(), 1);
        assert_eq!(readme[0].name, "README.md");
        assert!(search_files(&roots, "no-such-thing", 10).is_empty());
        assert!(search_files(&roots, "  ", 10).is_empty());
    }

    #[test]
    fn a_name_that_starts_with_the_query_comes_before_one_that_merely_holds_it() {
        let s = Scratch::new("order");
        fs::create_dir_all(s.path().join("repo/a")).unwrap();
        fs::write(s.path().join("repo/a/chat.ts"), "x").unwrap();
        fs::write(s.path().join("repo/a/my-chat.ts"), "x").unwrap();
        let roots = vec![("r".to_string(), s.path().join("repo").display().to_string())];
        let hits = search_files(&roots, "chat", 10);
        assert_eq!(hits[0].name, "chat.ts");
        assert_eq!(hits[1].name, "my-chat.ts");
    }

    #[test]
    fn the_folder_of_the_agent_is_read_only_when_it_is_one_of_ours() {
        let s = Scratch::new("dirs");
        let cfg = crate::testutil::test_cfg(s.path());
        fs::create_dir_all(cfg.data_dir.join("coordinator/default/chat")).unwrap();
        fs::create_dir_all(s.path().join("repo")).unwrap();
        fs::create_dir_all(s.path().join("elsewhere")).unwrap();
        // The Coordinator's own folder is the default.
        assert!(agent_dir(&cfg, None).is_some());
        assert!(agent_dir(&cfg, Some("")).is_some());
        assert!(agent_dir(&cfg, Some(s.path().join("elsewhere").to_str().unwrap())).is_none());
        assert!(agent_dir(&cfg, Some("/")).is_none());
        assert!(agent_dir(&cfg, Some("no/such/dir")).is_none());
    }
}
