//! Universes: a repository (a project) added to this machine's herdr-projects board.
//!
//! What the Galaxy's "add a universe" needs, without any HTTP here:
//! - `complete_path`: folder names as typed, with git and board marks;
//! - `parse_address`: a repository address on GitHub, GitLab (.com or self-hosted) or another host;
//! - `inspect`: what a folder or an address is: branch, origin, who may write (`gh` / `glab`),
//!   the suggested name and key, whether the board has it already;
//! - `add`: clone (or fork, or read a folder), register the folder on the board with
//!   `herdr-projects add "…" --repo PATH`, read the key the board gives, set the settings the host needs;
//! - `open_pull_request`, `make_fork`, `upstream_lag`: what a fork needs afterwards.
//!
//! The planner never writes the board's files: it runs the board's own commands. It reads the board
//! only through `taskmeta::repos` and `show --json`.

// Not wired to HTTP yet (the server belongs to the lead); remove once it is.


use crate::config::{expand_home, AppConfig};
use crate::gitlag;
use crate::taskmeta;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

/// Quick reads: git, the board's answers about a folder.
const LOCAL_WAIT: Duration = Duration::from_secs(10);
/// Hosting API calls.
const INFO_WAIT: Duration = Duration::from_secs(15);
/// Opening a pull request or a merge request.
const PR_WAIT: Duration = Duration::from_secs(60);
/// Board commands.
const BOARD_WAIT: Duration = Duration::from_secs(60);
/// A clone, or a fork with its clone.
const CLONE_WAIT: Duration = Duration::from_secs(900);
/// Folder entries the picker returns when the caller gives no limit.
/// GitLab access level that may push a branch (Developer).
const GITLAB_WRITE: u64 = 30;

// ---------------------------------------------------------------------------
// Folder picker
// ---------------------------------------------------------------------------

/// One folder the picker offers. `path` is as typed (`~/Projects/x`), without a trailing slash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub git: bool,
    pub on_board: bool,
    pub branch: Option<String>,
    pub origin: Option<String>,
}

/// Folder names that start with `input`'s last segment, case-insensitively, as typed. Hidden folders
/// only when the segment starts with `.`. One level, no recursion; at most `limit` entries, and the
/// git facts are read for those only. `board_roots` are the folders the board already has.
pub fn complete_path(input: &str, board_roots: &[String], limit: usize) -> Vec<FsEntry> {
    if input.contains('\0') || limit == 0 {
        return Vec::new();
    }
    let (typed_dir, prefix) = split_typed(input);
    let dir = if typed_dir.is_empty() {
        PathBuf::from(".")
    } else {
        expand_home(typed_dir)
    };
    let Ok(read) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let lower = prefix.to_lowercase();
    let hidden_ok = prefix.starts_with('.');
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_lowercase().starts_with(&lower) || (name.starts_with('.') && !hidden_ok) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            found.push((name, path));
        }
    }
    found.sort_by(|a, b| {
        a.0.to_lowercase()
            .cmp(&b.0.to_lowercase())
            .then_with(|| a.0.cmp(&b.0))
    });
    found.truncate(limit);
    let roots = canonical_roots(board_roots);
    found
        .into_iter()
        .map(|(name, path)| {
            let git = path.join(".git").exists();
            let on_board = path.canonicalize().is_ok_and(|c| roots.contains(&c));
            let (branch, origin) = if git {
                (
                    git_quick(&path, &["symbolic-ref", "--short", "-q", "HEAD"]),
                    git_quick(&path, &["remote", "get-url", "origin"]),
                )
            } else {
                (None, None)
            };
            FsEntry {
                path: format!("{typed_dir}{name}"),
                name,
                is_dir: true,
                git,
                on_board,
                branch,
                origin,
            }
        })
        .collect()
}

/// Splits what was typed into the directory part (kept as typed, with its slash) and the prefix.
fn split_typed(input: &str) -> (&str, &str) {
    match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => ("", input),
    }
}

fn canonical_roots(board_roots: &[String]) -> Vec<PathBuf> {
    board_roots
        .iter()
        .filter(|r| !r.trim().is_empty())
        .filter_map(|r| expand_home(r.trim()).canonicalize().ok())
        .collect()
}

/// `git -C dir ARGS`, read synchronously with a 2 s cap (the picker is a plain function).
fn git_quick(dir: &Path, args: &[&str]) -> Option<String> {
    let mut child = StdCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut text = String::new();
                child.stdout.take()?.read_to_string(&mut text).ok()?;
                return nonempty(text.trim());
            }
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(2) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Addresses
// ---------------------------------------------------------------------------

/// The hosting a repository lives on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Host {
    GitHub,
    GitLabCom,
    /// A self-hosted GitLab (its name has `gitlab` in it), e.g. `gitlab.example.com`.
    GitLabSelf(String),
    /// Any other host: clone only, no fork or permission features.
    Other(String),
}

/// A repository address read from a URL or the scp form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoAddr {
    pub host: String,
    pub kind: Host,
    /// Everything before the last segment: `group/sub` for `group/sub/repo`, `owner` for GitHub.
    pub owner_path: String,
    pub name: String,
    /// What `git clone` takes: `https://` or `user@host:path.git` (or `ssh://`, with a port).
    pub clone_url: String,
}

impl RepoAddr {
    /// The path on the host: `owner/repo` or `group/sub/repo`.
    pub fn path(&self) -> String {
        if self.owner_path.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.owner_path, self.name)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Https,
    Ssh,
    Scp,
}

/// Reads `https://`, `http://`, `ssh://`, `user@host:path` and `host/path` addresses. Credentials in an
/// https address are dropped, never kept.
pub fn parse_address(input: &str) -> Result<RepoAddr, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("укажите адрес репозитория или папку".into());
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("в адресе не должно быть пробелов".into());
    }
    let (form, user, hostport, path) = match s.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            let form = match scheme.as_str() {
                "https" | "http" => Form::Https,
                "ssh" | "git+ssh" | "ssh+git" | "git" => Form::Ssh,
                other => {
                    return Err(format!(
                        "схема «{other}» не поддерживается: нужен https:// или ssh://"
                    ))
                }
            };
            let (auth, path) = rest
                .split_once('/')
                .ok_or("в адресе нет пути к репозиторию")?;
            let (user, hostport) = match auth.rsplit_once('@') {
                Some((u, h)) => (u.to_string(), h),
                None => (String::new(), auth),
            };
            let user = match form {
                Form::Https => String::new(),
                _ if user.is_empty() => "git".to_string(),
                _ => user,
            };
            (form, user, hostport.to_string(), path.to_string())
        }
        None => match s.split_once(':') {
            Some((h, p)) if !h.is_empty() && !h.contains('/') && !p.starts_with('/') => {
                let (user, host) = match h.rsplit_once('@') {
                    Some((u, host)) => (u.to_string(), host.to_string()),
                    None => ("git".to_string(), h.to_string()),
                };
                (Form::Scp, user, host, p.to_string())
            }
            _ => {
                let (host, path) = s.split_once('/').ok_or_else(|| {
                    format!("укажите адрес с хостом, например github.com/owner/repo: {s}")
                })?;
                (
                    Form::Https,
                    String::new(),
                    host.to_string(),
                    path.to_string(),
                )
            }
        },
    };

    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, Some(p.to_string())),
        None => (hostport.as_str(), None),
    };
    let host = host.to_ascii_lowercase();
    let host = if host == "www.github.com" {
        "github.com".to_string()
    } else {
        host
    };
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && (host.contains('.') || host == "localhost");
    if !host_ok {
        return Err(format!("не понял хост в адресе: {hostport:?}"));
    }
    if let Some(p) = &port {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("порт в адресе должен быть числом: {p:?}"));
        }
    }

    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    let segs: Vec<&str> = path.split('/').collect();
    let good = segs.len() >= 2
        && segs.iter().all(|seg| {
            !seg.is_empty()
                && *seg != "."
                && *seg != ".."
                && !seg.starts_with('-')
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        });
    if !good {
        return Err(format!(
            "путь должен быть владелец/репозиторий (для GitLab: группа/подгруппа/репо): {path:?}"
        ));
    }

    let kind = match host.as_str() {
        "github.com" => Host::GitHub,
        "gitlab.com" => Host::GitLabCom,
        h if h.contains("gitlab") => Host::GitLabSelf(h.to_string()),
        h => Host::Other(h.to_string()),
    };
    if kind == Host::GitHub && segs.len() != 2 {
        return Err(format!(
            "на GitHub адрес — owner/repo, без вложенных путей: {path:?}"
        ));
    }
    let full = segs.join("/");
    let hp = match &port {
        Some(p) => format!("{host}:{p}"),
        None => host.clone(),
    };
    let clone_url = match form {
        Form::Https => format!("https://{hp}/{full}.git"),
        Form::Ssh if port.is_some() => format!("ssh://{user}@{hp}/{full}.git"),
        Form::Ssh | Form::Scp => format!("{user}@{host}:{full}.git"),
    };
    let name = segs[segs.len() - 1].to_string();
    let owner_path = segs[..segs.len() - 1].join("/");
    Ok(RepoAddr {
        host,
        kind,
        owner_path,
        name,
        clone_url,
    })
}

fn forkable(kind: &Host) -> bool {
    matches!(kind, Host::GitHub | Host::GitLabCom | Host::GitLabSelf(_))
}

/// The hosts where the board cannot open a pull request (it does that on GitHub only): merges happen
/// locally, so the settings `merge.pr false` and `merge.checks none` are set for them.
fn pr_by_board_off(kind: &Host) -> bool {
    match kind {
        Host::GitHub => false,
        Host::GitLabCom | Host::GitLabSelf(_) => true,
        Host::Other(h) => !h.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// Inspection
// ---------------------------------------------------------------------------

/// Whether a hosting CLI is logged in to a host, and as whom. `hint` says what to do when it is not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Login {
    pub ok: bool,
    pub user: Option<String>,
    pub hint: String,
}

/// What the hosting says about a repository. Every field is `None` when the hosting did not say.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepoInfo {
    pub can_write: Option<bool>,
    pub is_fork: Option<bool>,
    /// The original repository (`owner/repo`) when this one is a fork.
    pub parent: Option<String>,
    pub default_branch: Option<String>,
    pub private: Option<bool>,
}

/// Everything the add form shows before anything is cloned.
#[derive(Debug, Clone, Serialize)]
pub struct Inspect {
    /// `true` when the input was a folder, `false` when it was an address.
    pub is_path: bool,
    pub path: Option<String>,
    pub git: bool,
    pub branch: Option<String>,
    pub origin: Option<String>,
    pub address: Option<RepoAddr>,
    pub login: Option<Login>,
    pub can_write: Option<bool>,
    pub is_fork: Option<bool>,
    pub parent: Option<String>,
    pub default_branch: Option<String>,
    pub private: Option<bool>,
    pub suggested_name: String,
    pub suggested_key: String,
    pub clone_dir: String,
    pub exists_on_disk: bool,
    pub on_board: bool,
    /// The board's name of the repository, when the board has it already.
    pub board_name: Option<String>,
    pub notes: Vec<String>,
}

/// The binaries behind the hosting CLIs. Tests point them at fake scripts.
#[derive(Debug, Clone)]
pub(crate) struct Tools {
    pub gh: String,
    pub glab: String,
}

impl Tools {
    pub(crate) fn from_env() -> Tools {
        Tools {
            gh: env_or("HERDR_PLANNER_GH", "gh"),
            glab: env_or("HERDR_PLANNER_GLAB", "glab"),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Reads a folder or an address: a folder when the input starts with `/`, `~`, `.` or is an existing
/// directory; an address otherwise.
pub async fn inspect(cfg: &AppConfig, input: &str) -> Result<Inspect, String> {
    inspect_with(cfg, input, &Tools::from_env()).await
}

pub(crate) async fn inspect_with(
    cfg: &AppConfig,
    input: &str,
    tools: &Tools,
) -> Result<Inspect, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("укажите папку или адрес репозитория".into());
    }
    if s.contains('\0') {
        return Err("в пути не должно быть NUL".into());
    }
    if looks_like_path(s) {
        inspect_path(cfg, s, tools).await
    } else {
        inspect_address(cfg, s, tools).await
    }
}

fn looks_like_path(s: &str) -> bool {
    if s.starts_with('/') || s.starts_with('~') || s.starts_with('.') {
        return true;
    }
    !s.contains("://") && !s.contains('@') && Path::new(s).is_dir()
}

async fn inspect_path(cfg: &AppConfig, s: &str, tools: &Tools) -> Result<Inspect, String> {
    let p = expand_home(s)
        .canonicalize()
        .map_err(|_| format!("папка не найдена: {s}"))?;
    if !p.is_dir() {
        return Err(format!("это не папка: {s}"));
    }
    let path = p.display().to_string();
    let git = p.join(".git").exists();
    let (branch, origin) = if git {
        (
            git_out(&p, &["symbolic-ref", "--short", "-q", "HEAD"]).await,
            git_out(&p, &["remote", "get-url", "origin"]).await,
        )
    } else {
        (None, None)
    };
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut notes = Vec::new();
    if !git {
        notes.push("это не git-репозиторий: добавить можно только папку с git".to_string());
    } else if origin.is_none() {
        notes.push("у папки нет origin: доска узнает её только по папке".to_string());
    }
    let board = taskmeta::repos(&cfg.data_dir);
    let board_name = board
        .iter()
        .find(|r| !r.root.is_empty() && Path::new(&r.root).canonicalize().is_ok_and(|c| c == p))
        .map(|r| r.name.clone());
    let address = origin.as_deref().and_then(|o| parse_address(o).ok());
    let (login, info) = match &address {
        Some(a) => {
            let (login, info) = remote_info(a, tools).await;
            let info = info.unwrap_or_else(|e| {
                notes.push(e);
                RepoInfo::default()
            });
            (login, info)
        }
        None => (None, RepoInfo::default()),
    };
    if let Some(a) = &address {
        if matches!(a.kind, Host::Other(_)) {
            notes.push("права на этом хостинге не проверяю".to_string());
        }
    }
    Ok(Inspect {
        is_path: true,
        path: Some(path),
        git,
        branch,
        origin,
        address,
        login,
        can_write: info.can_write,
        is_fork: info.is_fork,
        parent: info.parent,
        default_branch: info.default_branch,
        private: info.private,
        suggested_name: name.clone(),
        suggested_key: suggest_key(&name, &taken_keys(&cfg.data_dir)),
        clone_dir: cfg.projects_dir_now().join(&name).display().to_string(),
        exists_on_disk: true,
        on_board: board_name.is_some(),
        board_name,
        notes,
    })
}

async fn inspect_address(cfg: &AppConfig, s: &str, tools: &Tools) -> Result<Inspect, String> {
    let addr = parse_address(s)?;
    let name = addr.name.clone();
    let clone_dir = cfg.projects_dir_now().join(&name);
    let want = gitlag::normalize_origin(&addr.clone_url);
    let mut board_name = None;
    for r in taskmeta::repos(&cfg.data_dir)
        .into_iter()
        .filter(|r| !r.root.is_empty())
    {
        if let Some(o) = git_out(Path::new(&r.root), &["remote", "get-url", "origin"]).await {
            if gitlag::normalize_origin(&o) == want {
                board_name = Some(r.name);
                break;
            }
        }
    }
    let mut notes = Vec::new();
    if let Some(b) = &board_name {
        notes.push(format!("этот репозиторий уже на доске: {b}"));
    }
    let (login, info) = remote_info(&addr, tools).await;
    let info = info.unwrap_or_else(|e| {
        notes.push(e);
        RepoInfo::default()
    });
    if matches!(addr.kind, Host::Other(_)) {
        notes.push("права на этом хостинге не проверяю: клонирование как есть".to_string());
    }
    Ok(Inspect {
        is_path: false,
        path: None,
        git: false,
        branch: None,
        origin: None,
        suggested_key: suggest_key(&name, &taken_keys(&cfg.data_dir)),
        suggested_name: name,
        clone_dir: clone_dir.display().to_string(),
        exists_on_disk: clone_dir.exists(),
        on_board: board_name.is_some(),
        board_name,
        address: Some(addr),
        login,
        can_write: info.can_write,
        is_fork: info.is_fork,
        parent: info.parent,
        default_branch: info.default_branch,
        private: info.private,
        notes,
    })
}

/// The CLI's login state for an address's host (`None` for other hosts), and what the hosting says
/// about the repository. The login is kept even when the repository could not be read.
async fn remote_info(addr: &RepoAddr, tools: &Tools) -> (Option<Login>, Result<RepoInfo, String>) {
    match &addr.kind {
        Host::GitHub => {
            let login = gh_login(tools).await;
            (Some(login), github_repo(addr, tools).await)
        }
        Host::GitLabCom | Host::GitLabSelf(_) => {
            let login = glab_login(tools, &addr.host).await;
            (Some(login), gitlab_repo(addr, tools).await)
        }
        Host::Other(_) => (None, Ok(RepoInfo::default())),
    }
}

async fn github_repo(addr: &RepoAddr, tools: &Tools) -> Result<RepoInfo, String> {
    let path = addr.path();
    let out = run(
        &tools.gh,
        &own(&[
            "repo",
            "view",
            path.as_str(),
            "--json",
            "viewerPermission,isFork,parent,defaultBranchRef,nameWithOwner,isPrivate",
        ]),
        None,
        INFO_WAIT,
    )
    .await?;
    if !out.ok() {
        let low = out.text().to_lowercase();
        return Err(if low.contains("could not resolve") {
            format!("репозиторий {path} не найден на GitHub (или нет доступа: gh auth login)")
        } else {
            format!("gh repo view: {}", out.text())
        });
    }
    let v = parse_json(&out.stdout, "gh repo view")?;
    let can_write = match v.get("viewerPermission").and_then(Value::as_str) {
        Some("ADMIN") | Some("MAINTAIN") | Some("WRITE") => Some(true),
        Some("READ") | Some("TRIAGE") => Some(false),
        _ => None,
    };
    Ok(RepoInfo {
        can_write,
        is_fork: v.get("isFork").and_then(Value::as_bool),
        parent: v
            .get("parent")
            .and_then(|p| p.get("nameWithOwner"))
            .and_then(Value::as_str)
            .map(str::to_string),
        default_branch: v
            .get("defaultBranchRef")
            .and_then(|b| b.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        private: v.get("isPrivate").and_then(Value::as_bool),
    })
}

async fn gitlab_repo(addr: &RepoAddr, tools: &Tools) -> Result<RepoInfo, String> {
    let host = addr.host.as_str();
    let path = addr.path();
    let api = format!("projects/{}", encode_path(&path));
    let out = run(
        &tools.glab,
        &own(&["api", api.as_str(), "--hostname", host]),
        None,
        INFO_WAIT,
    )
    .await?;
    if !out.ok() {
        let low = out.text().to_lowercase();
        return Err(
            if low.contains("401") || low.contains("403") || low.contains("unauthorized") {
                format!("нет доступа к {path} на {host}: войди glab auth login --hostname {host}")
            } else if low.contains("404") || low.contains("not found") {
                format!("проект {path} на {host} не найден (или нет доступа)")
            } else {
                format!("glab api: {}", out.text())
            },
        );
    }
    let v = parse_json(&out.stdout, "glab api")?;
    let perms = v.get("permissions");
    let level = |k: &str| {
        perms
            .and_then(|p| p.get(k))
            .and_then(|a| a.get("access_level"))
            .and_then(Value::as_u64)
    };
    let best = [level("project_access"), level("group_access")]
        .into_iter()
        .flatten()
        .max();
    let can_write = if perms.is_some() {
        Some(best.unwrap_or(0) >= GITLAB_WRITE)
    } else {
        None
    };
    let parent = v
        .get("forked_from_project")
        .filter(|p| !p.is_null())
        .and_then(|p| p.get("path_with_namespace"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(RepoInfo {
        can_write,
        is_fork: Some(parent.is_some()),
        parent,
        default_branch: v
            .get("default_branch")
            .and_then(Value::as_str)
            .map(str::to_string),
        private: v
            .get("visibility")
            .and_then(Value::as_str)
            .map(|vis| vis != "public"),
    })
}

async fn gh_login(tools: &Tools) -> Login {
    match run(
        &tools.gh,
        &own(&["auth", "status", "--hostname", "github.com"]),
        None,
        LOCAL_WAIT,
    )
    .await
    {
        Err(hint) => Login {
            ok: false,
            user: None,
            hint,
        },
        Ok(out) if out.ok() => Login {
            ok: true,
            user: word_after(&out.combined(), "account"),
            hint: String::new(),
        },
        Ok(_) => Login {
            ok: false,
            user: None,
            hint: "Войди: gh auth login".to_string(),
        },
    }
}

async fn glab_login(tools: &Tools, host: &str) -> Login {
    match run(
        &tools.glab,
        &own(&["auth", "status", "--hostname", host]),
        None,
        LOCAL_WAIT,
    )
    .await
    {
        Err(hint) => Login {
            ok: false,
            user: None,
            hint,
        },
        Ok(out) if out.ok() => Login {
            ok: true,
            user: word_after(&out.combined(), "as"),
            hint: String::new(),
        },
        Ok(_) => Login {
            ok: false,
            user: None,
            hint: format!("Войди: glab auth login --hostname {host}"),
        },
    }
}

/// The key the board would give a repository of this name, taking the board's own rule: the last word's
/// first letter and the next consonants (`planner` → `PLN`, `omarchy` → `OMR`); when that key is taken,
/// the initials of the words (`summary-service` → `SS`), then longer keys. The board decides the real
/// key when it registers the folder; this is the suggestion.
pub fn suggest_key(name: &str, taken: &[String]) -> String {
    let words: Vec<&str> = name
        .split(['-', '_', '.'])
        .filter(|w| !w.is_empty())
        .collect();
    let last = words.last().copied().unwrap_or(name);
    let initials: String = words
        .iter()
        .filter_map(|w| w.chars().find(|c| c.is_ascii_alphabetic()))
        .collect::<String>()
        .to_ascii_uppercase();
    let candidates = [
        skeleton(last, 3),
        if initials.len() >= 2 {
            initials.chars().take(4).collect::<String>()
        } else {
            String::new()
        },
        skeleton(last, 4),
        skeleton(name, 3),
    ];
    let taken: Vec<String> = taken.iter().map(|t| t.to_ascii_uppercase()).collect();
    candidates
        .iter()
        .find(|c| c.len() >= 2 && !taken.contains(c))
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

/// The first letter of `word`, then its consonants, up to `n`; short words keep their first letters.
fn skeleton(word: &str, n: usize) -> String {
    let letters: Vec<char> = word
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if letters.is_empty() {
        return String::new();
    }
    let mut out = vec![letters[0]];
    for &c in &letters[1..] {
        if out.len() == n {
            break;
        }
        if !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y') {
            out.push(c);
        }
    }
    if out.len() < n {
        out = letters.iter().take(n).copied().collect();
    }
    out.into_iter().collect::<String>().to_ascii_uppercase()
}

/// The keys the board has in its `repo-keys.json` (`had`). Read only.
fn taken_keys(data_dir: &Path) -> Vec<String> {
    let text = fs::read_to_string(data_dir.join("repo-keys.json")).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v.get("had")
        .and_then(Value::as_object)
        .or_else(|| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Adding a universe
// ---------------------------------------------------------------------------

/// What the operator chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// A folder with git on the machine: only registered on the board.
    Folder,
    /// A repository the operator may write to: cloned to the default folder.
    Clone,
    /// Someone else's repository to change: a fork, origin = fork, upstream = original.
    Fork,
    /// Someone else's repository to read: a clone, tasks default to research.
    Study,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddRequest {
    pub mode: Mode,
    /// Folder mode: the folder with git.
    pub path: Option<String>,
    /// Clone, fork and study: the address (or a local repository path).
    pub url: Option<String>,
    /// The project's name: the folder's name, the board's name for it.
    pub name: String,
    /// Optional board key (2–5 capitals or digits); the board's own key when absent.
    pub key: Option<String>,
    /// Clone parent folder; the default is the planner's `projects_dir` (`~/Projects`).
    pub dir: Option<String>,
}

/// One step of the add, for the progress the page shows.
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub label: String,
    /// `wait`, `run`, `ok` or `err`.
    pub state: &'static str,
    pub detail: String,
    pub percent: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddResult {
    pub name: String,
    pub key: String,
    pub root: String,
    pub origin: Option<String>,
    pub upstream: Option<String>,
    pub fork_of: Option<String>,
    /// The settings set on the board, as `name value`.
    pub settings: Vec<String>,
    pub notes: Vec<String>,
}

/// Adds a universe on this machine. `progress` gets all the steps each time one changes.
pub async fn add(
    cfg: &AppConfig,
    req: &AddRequest,
    progress: &(dyn Fn(&[Step]) + Send + Sync),
) -> Result<AddResult, String> {
    add_at(cfg, req, progress, &Tools::from_env()).await
}

const STEP_CLONE: &str = "клонирование";
const STEP_BOARD: &str = "регистрация на доске";
const STEP_KEY: &str = "ключ проекта";
const STEP_SETTINGS: &str = "настройки";

fn step(id: &'static str, label: &str) -> Step {
    Step {
        id,
        label: label.to_string(),
        state: "wait",
        detail: String::new(),
        percent: None,
    }
}

/// The steps, reported to the caller as they change.
struct Progress<'a> {
    steps: Vec<Step>,
    cb: &'a (dyn Fn(&[Step]) + Send + Sync),
}

impl<'a> Progress<'a> {
    fn new(steps: Vec<Step>, cb: &'a (dyn Fn(&[Step]) + Send + Sync)) -> Self {
        let p = Progress { steps, cb };
        (p.cb)(&p.steps);
        p
    }

    fn set(
        &mut self,
        i: usize,
        state: &'static str,
        detail: impl Into<String>,
        percent: Option<u8>,
    ) {
        let st = &mut self.steps[i];
        st.state = state;
        st.detail = detail.into();
        st.percent = percent;
        (self.cb)(&self.steps);
    }

    fn run(&mut self, i: usize, detail: impl Into<String>) {
        self.set(i, "run", detail, None);
    }

    fn ok(&mut self, i: usize, detail: impl Into<String>) {
        self.set(i, "ok", detail, None);
    }

    fn percent(&mut self, i: usize, p: u8) {
        let detail = self.steps[i].detail.clone();
        self.set(i, "run", detail, Some(p));
    }
}

/// Records the failed step and returns the error that names it.
fn fail<T>(
    prog: &mut Progress<'_>,
    i: usize,
    step: &str,
    msg: impl Into<String>,
) -> Result<T, String> {
    let msg = msg.into();
    prog.set(i, "err", msg.clone(), None);
    Err(format!("шаг «{step}»: {msg}"))
}

pub(crate) async fn add_at(
    cfg: &AppConfig,
    req: &AddRequest,
    progress: &(dyn Fn(&[Step]) + Send + Sync),
    tools: &Tools,
) -> Result<AddResult, String> {
    let name = req.name.trim().to_string();
    if !valid_name(&name) {
        return Err(format!(
            "имя проекта — одно слово без пробелов и «/»: {:?}",
            req.name
        ));
    }
    let key_want = req
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string);
    if let Some(k) = &key_want {
        if !valid_key(k) {
            return Err(format!(
                "ключ проекта — 2–5 заглавных букв или цифр, с буквы: {k:?}"
            ));
        }
    }
    let folder = req.mode == Mode::Folder;
    let mut prog = Progress::new(
        vec![
            step(
                "clone",
                if folder {
                    "проверка папки"
                } else {
                    STEP_CLONE
                },
            ),
            step("board", STEP_BOARD),
            step("key", STEP_KEY),
            step("settings", STEP_SETTINGS),
        ],
        progress,
    );
    let step_clone = if folder {
        "проверка папки"
    } else {
        "клонирование"
    };
    let gh = tools.gh.clone();
    let glab = tools.glab.clone();
    let mut notes: Vec<String> = Vec::new();

    // 1. The source: a folder that exists, or a clone (or fork) into the target folder.
    let (root, source, fork_of, upstream_given) = if folder {
        let path = match req.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => p.to_string(),
            None => return fail(&mut prog, 0, step_clone, "укажите папку с git"),
        };
        let facts = match folder_facts(&path).await {
            Ok(f) => f,
            Err(e) => return fail(&mut prog, 0, step_clone, e),
        };
        if !facts.git {
            let msg = format!("это не git-репозиторий: {}", facts.root);
            return fail(&mut prog, 0, step_clone, msg);
        }
        let base = facts.root.rsplit('/').next().unwrap_or("").to_string();
        if base != name {
            let msg = format!("доска называет проект по имени папки: «{base}», а не «{name}»");
            return fail(&mut prog, 0, step_clone, msg);
        }
        if let Err(e) = check_board_name(cfg, &name, &facts.root) {
            return fail(&mut prog, 0, step_clone, e);
        }
        if taskmeta::repos(&cfg.data_dir)
            .iter()
            .any(|r| r.root == facts.root)
        {
            let msg = format!("эта папка уже на доске: {name}");
            return fail(&mut prog, 0, step_clone, msg);
        }
        prog.ok(0, format!("папка {}", facts.root));
        let source = facts.origin.as_deref().and_then(|o| parse_address(o).ok());
        let upstream = remote_url(&facts.root, "upstream").await;
        let fork_of = match (&upstream, &facts.origin) {
            (Some(up), Some(o)) if up != o => parse_address(up).ok().map(|a| a.path()),
            _ => None,
        };
        (facts.root, source, fork_of, upstream)
    } else {
        let url = req.url.as_deref().unwrap_or("");
        let (clone_url, addr) = match source_of(url) {
            Ok(v) => v,
            Err(e) => return fail(&mut prog, 0, step_clone, e),
        };
        if req.mode == Mode::Fork && !addr.as_ref().is_some_and(|a| forkable(&a.kind)) {
            let msg = "форк делаю для GitHub и GitLab: для другого адреса нужен клон";
            return fail(&mut prog, 0, step_clone, msg);
        }
        let base = match req.dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) => d.to_string(),
            None => cfg.projects_dir_now().display().to_string(),
        };
        let target = format!("{}/{}", base.trim_end_matches('/'), name);
        if let Err(e) = check_target(&target) {
            return fail(&mut prog, 0, step_clone, e);
        }
        if let Err(e) = check_board_name(cfg, &name, &target) {
            return fail(&mut prog, 0, step_clone, e);
        }
        prog.run(0, format!("{clone_url} → {target}"));
        if let Err(e) = make_dir(&base) {
            return fail(&mut prog, 0, step_clone, e);
        }
        let (fork_of, upstream) = if req.mode == Mode::Fork {
            let a = addr
                .clone()
                .unwrap_or_else(|| unreachable!("fork without an address"));
            let fork =
                match create_fork(&a, Some(name.as_str()), &base, &gh, &glab).await {
                    Ok(f) => f,
                    Err(e) => return fail(&mut prog, 0, step_clone, e),
                };
            prog.run(
                0,
                format!("клонирование форка {} → {target}", fork.fork_path),
            );
            if let Err(e) = clone_into(&mut prog, &fork.fork_url, &target).await {
                return fail(&mut prog, 0, step_clone, e);
            }
            if let Err(e) = git_ok(&[
                    "-C",
                    target.as_str(),
                    "remote",
                    "add",
                    "upstream",
                    clone_url.as_str(),
                ],
            )
            .await
            {
                return fail(&mut prog, 0, step_clone, format!("upstream: {e}"));
            }
            (Some(a.path()), Some(clone_url.clone()))
        } else {
            if let Err(e) = clone_into(&mut prog, &clone_url, &target).await {
                return fail(&mut prog, 0, step_clone, e);
            }
            (None, None)
        };
        let root = match resolve_root(&target) {
            Ok(r) => r,
            Err(e) => return fail(&mut prog, 0, step_clone, e),
        };
        prog.ok(0, format!("клонировано в {root}"));
        (root, addr, fork_of, upstream)
    };

    // 2. The board: `add "…" --repo PATH` is how it learns a repository; the task it makes is archived.
    let title = format!("Знакомство с проектом {name}");
    prog.run(1, format!("herdr-projects add «{title}» --repo {root}"));
    let out = match board(
        cfg,
        &own(&["add", title.as_str(), "--repo", root.as_str()]),
        BOARD_WAIT,
    )
    .await
    {
        Ok(out) => out,
        Err(e) => return fail(&mut prog, 1, STEP_BOARD, e),
    };
    let added = match added_id(&out) {
        Some(id) => id,
        None => {
            return fail(
                &mut prog,
                1,
                STEP_BOARD,
                format!("доска ответила не так, как ждали: {}", out.trim()),
            )
        }
    };
    match board(cfg, &own(&["archive", added.as_str()]), BOARD_WAIT).await {
        Ok(_) => notes.push(format!(
            "задача «{title}» ({added}) в архиве: без агента она ничего не делает"
        )),
        Err(e) => notes.push(format!("задача {added} осталась в очереди: {e}")),
    }
    prog.ok(1, format!("{name} на доске; задача {added} в архиве"));

    // 3. The key the board gives (or the one asked for).
    prog.run(2, "читаю ключ");
    let mut key = match board_key(cfg, &name).await {
        Some(k) => k,
        None => return fail(&mut prog, 2, STEP_KEY, "доска не сообщила ключ проекта"),
    };
    if let Some(want) = &key_want {
        if *want != key {
            let old = key.clone();
            let set = own(&["set", "repo.key", want.as_str(), "--repo", name.as_str()]);
            if let Err(e) = board(cfg, &set, BOARD_WAIT).await {
                return fail(&mut prog, 2, STEP_KEY, e);
            }
            key = match board_key(cfg, &name).await {
                Some(k) if k == *want => k,
                other => {
                    let got = other.unwrap_or_else(|| "нет".into());
                    let msg = format!("доска ответила ключ {got}, ждали {want}");
                    return fail(&mut prog, 2, STEP_KEY, msg);
                }
            };
            notes.push(format!("старый ключ {old} тоже открывает задачи проекта"));
        }
    }
    prog.ok(2, format!("ключ {key}"));

    // 4. Settings the host needs: research for a study, local merges where the board cannot open a PR.
    let mut want: Vec<(&str, &str)> = Vec::new();
    if req.mode == Mode::Study {
        want.push(("research.only", "true"));
    }
    if source.as_ref().is_some_and(|a| pr_by_board_off(&a.kind)) {
        want.push(("merge.pr", "false"));
        want.push(("merge.checks", "none"));
    }
    let mut applied: Vec<String> = Vec::new();
    if want.is_empty() {
        prog.ok(3, "по умолчанию");
    } else {
        prog.run(3, format!("{} настроек", want.len()));
        for &(n, v) in &want {
            let set = own(&["set", n, v, "--repo", name.as_str()]);
            if let Err(e) = board(cfg, &set, BOARD_WAIT).await {
                return fail(&mut prog, 3, STEP_SETTINGS, e);
            }
            let got = board_setting(cfg, &name, n).await;
            if got.as_deref() != Some(v) {
                let msg = format!(
                    "доска ответила {n} = {}, ждали {v}",
                    got.unwrap_or_else(|| "нет".into())
                );
                return fail(&mut prog, 3, STEP_SETTINGS, msg);
            }
            applied.push(format!("{n} {v}"));
        }
        prog.ok(3, applied.join(", "));
    }
    if want.iter().any(|(n, _)| *n == "merge.pr") {
        notes.push(
            "доска открывает PR только на GitHub: здесь слияние локальное (merge.pr false, merge.checks none), PR не создаётся"
                .to_string(),
        );
    }
    if want.iter().any(|(n, _)| *n == "research.only") {
        notes.push(
            "исследовательский проект: задачи по умолчанию только отчёт, репозиторий не меняется (research.only true)"
                .to_string(),
        );
    }
    if let Some(f) = &fork_of {
        notes.push(format!(
            "форк ← {f}: origin — ваш форк, upstream — оригинал"
        ));
    }
    let origin = remote_url(&root, "origin").await;
    let upstream = match upstream_given {
        Some(u) => Some(u),
        None => remote_url(&root, "upstream").await,
    };
    Ok(AddResult {
        name,
        key,
        root,
        origin,
        upstream,
        fork_of,
        settings: applied,
        notes,
    })
}

fn valid_name(name: &str) -> bool {
    taskmeta::valid_repo(name) && !name.starts_with('.') && name.len() <= 100
}

fn valid_key(k: &str) -> bool {
    let b = k.as_bytes();
    (2..=5).contains(&b.len())
        && b[0].is_ascii_uppercase()
        && b.iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// A source the clone can take: a local repository path, or a parsed address (its clone URL).
fn source_of(url: &str) -> Result<(String, Option<RepoAddr>), String> {
    let u = url.trim();
    if u.is_empty() {
        return Err("укажите адрес репозитория".into());
    }
    if u.starts_with('/') && Path::new(u).is_dir() {
        return Ok((u.to_string(), None));
    }
    let a = parse_address(u)?;
    Ok((a.clone_url.clone(), Some(a)))
}

/// Refuses a target folder that exists and is not empty.
fn check_target(target: &str) -> Result<(), String> {
    let p = expand_home(target);
    if !p.exists() {
        return Ok(());
    }
    let empty = p.is_dir()
        && fs::read_dir(&p)
            .map(|mut r| r.next().is_none())
            .unwrap_or(false);
    if empty {
        Ok(())
    } else {
        Err(format!("{target} уже есть и не пуста: не перезаписываю"))
    }
}

/// Refuses a name the board already gives to another folder (the board would name two folders alike).
fn check_board_name(cfg: &AppConfig, name: &str, root: &str) -> Result<(), String> {
    let want = expand_home(root);
    for r in taskmeta::repos(&cfg.data_dir) {
        if r.name == name && !r.root.is_empty() {
            let same = Path::new(&r.root).canonicalize().ok() == want.canonicalize().ok()
                && want.canonicalize().is_ok();
            if !same {
                return Err(format!(
                    "на доске уже есть проект «{name}» в другой папке: {}",
                    r.root
                ));
            }
        }
    }
    Ok(())
}

fn make_dir(base: &str) -> Result<(), String> {
    fs::create_dir_all(expand_home(base)).map_err(|e| format!("{base}: {e}"))
}

/// `git clone` into `target`; the percent is streamed from git's progress.
async fn clone_into(
    prog: &mut Progress<'_>,
    url: &str,
    target: &str,
) -> Result<(), String> {
    let mut cb = |p: u8| prog.percent(0, p);
    clone_local(url, target, &mut cb).await
}

async fn clone_local(
    url: &str,
    target: &str,
    on_progress: &mut (dyn FnMut(u8) + Send),
) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["clone", "--progress", url, target])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                missing_text("git")
            } else {
                format!("git: {e}")
            }
        })?;
    let mut err = child.stderr.take().ok_or("git: нет вывода")?;
    let work = async {
        let mut buf = [0u8; 4096];
        let mut pending = String::new();
        let mut last = String::new();
        loop {
            let n = err.read(&mut buf).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(i) = pending.find(['\r', '\n']) {
                let line: String = pending.drain(..=i).collect();
                let line = line.trim().to_string();
                if let Some(p) = parse_percent(&line) {
                    on_progress(p);
                }
                if !line.is_empty() {
                    last = line;
                }
            }
        }
        let rest = pending.trim().to_string();
        if !rest.is_empty() {
            if let Some(p) = parse_percent(&rest) {
                on_progress(p);
            }
            last = rest;
        }
        let status = child.wait().await.map_err(|e| e.to_string())?;
        Ok::<(bool, String), String>((status.success(), last))
    };
    let (ok, last) = match timeout(CLONE_WAIT, work).await {
        Err(_) => return Err("клонирование заняло больше 15 минут".into()),
        Ok(r) => r?,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("git clone: {}", clip(&last)))
    }
}

/// The percent in a git progress line (`Receiving objects:  45% (450/1000)`), if the line has one.
pub fn parse_percent(line: &str) -> Option<u8> {
    let i = line.rfind('%')?;
    let digits: String = line[..i]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if digits.is_empty() || digits.len() > 3 {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (n <= 100).then_some(n as u8)
}

/// The board's id of a task it just added: `SRC-1: added` → `SRC-1`, and as the board itself prints it, `src/1: added` →
/// `src/1` (its commands take either).
fn added_id(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.contains(": added"))?;
    let id = line.split(':').next()?.trim();
    let ok = !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/'))
        && !id.starts_with('/')
        && !id.contains("//");
    ok.then(|| id.to_string())
}

/// The key and the folder of a repository the board knows, from its own `show --json`.
async fn board_key(cfg: &AppConfig, name: &str) -> Option<String> {
    board_setting(cfg, name, "repo.key")
        .await
        .or_else(|| {
            taskmeta::repos(&cfg.data_dir)
                .into_iter()
                .find(|r| r.name == name && !r.key.is_empty())
                .map(|r| r.key)
        })
}

/// The value of one setting for a repository, as the board's `show --repo NAME --json` gives it.
async fn board_setting(cfg: &AppConfig, repo: &str, setting: &str) -> Option<String> {
    let out = board(cfg, &own(&["show", "--repo", repo, "--json"]), BOARD_WAIT)
        .await
        .ok()?;
    let v = last_json(&out)?;
    setting_text(&v, setting)
}

/// The last entry named `name` in a `show --json` answer, its value as text.
fn setting_text(v: &Value, name: &str) -> Option<String> {
    v.get("settings")?
        .as_array()?
        .iter()
        .filter(|s| s.get("name").and_then(Value::as_str) == Some(name))
        .last()
        .and_then(|s| s.get("value"))
        .map(|val| match val {
            Value::String(t) => t.clone(),
            other => other.to_string(),
        })
        .filter(|t| !t.is_empty())
}

/// Runs a board command through `cfg.herdr_projects`.
async fn board(cfg: &AppConfig, args: &[String], wait: Duration) -> Result<String, String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::app::run_board(cfg, &refs, wait).await
}

/// The board's answer: the last JSON object of stdout, where a shell's start-up files may print before it. An
/// object that is the board's own (a task list, or one with `ok`) wins over any other.
fn last_json(stdout: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) {
        if v.is_object() {
            return Some(v);
        }
    }
    let mut other = None;
    for line in stdout.lines().rev().map(str::trim) {
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if !v.is_object() {
                continue;
            }
            if v.get("tasks").is_some_and(Value::is_array)
                || v.get("ok").is_some_and(Value::is_boolean)
            {
                return Some(v);
            }
            other.get_or_insert(v);
        }
    }
    other
}

/// Where a folder is, and what git says about it.
#[allow(dead_code)]
struct Folder {
    root: String,
    git: bool,
    origin: Option<String>,
    branch: Option<String>,
}

async fn folder_facts(path: &str) -> Result<Folder, String> {
    let p = expand_home(path)
        .canonicalize()
        .map_err(|_| format!("папка не найдена: {path}"))?;
    if !p.is_dir() {
        return Err(format!("это не папка: {path}"));
    }
    let git = p.join(".git").exists();
    let origin = if git {
        git_out(&p, &["remote", "get-url", "origin"]).await
    } else {
        None
    };
    let branch = if git {
        git_out(&p, &["symbolic-ref", "--short", "-q", "HEAD"]).await
    } else {
        None
    };
    Ok(Folder {
        root: p.display().to_string(),
        git,
        origin,
        branch,
    })
}

/// The URL of a remote of a clone, if it has one.
async fn remote_url(root: &str, remote: &str) -> Option<String> {
    git_out(Path::new(root), &["remote", "get-url", remote]).await
}

/// The real path of a folder the clone made.
fn resolve_root(target: &str) -> Result<String, String> {
    expand_home(target)
        .canonicalize()
        .map(|p| p.display().to_string())
        .map_err(|e| format!("{target}: {e}"))
}

/// A fork made on the host: where it lives and the URL to clone it from.
struct ForkInfo {
    fork_path: String,
    fork_url: String,
}

/// Makes the fork of `addr` on its host (no clone). The fork is named `fork_name` (default: the same name).
async fn create_fork(
    addr: &RepoAddr,
    fork_name: Option<&str>,
    cwd: &str,
    gh: &str,
    glab: &str,
) -> Result<ForkInfo, String> {
    let name = fork_name.unwrap_or(&addr.name).to_string();
    let renamed = name != addr.name;
    match &addr.kind {
        Host::GitHub => {
            let path = addr.path();
            let mut args = own(&["repo", "fork", path.as_str(), "--clone=false"]);
            if renamed {
                args.extend(own(&["--fork-name", name.as_str()]));
            }
            let o = run(gh, &args, Some(Path::new(cwd)), PR_WAIT).await?;
            if !o.ok() {
                return Err(format!("gh repo fork: {}", o.text()));
            }
            let me = run(
                gh,
                &own(&["api", "user", "--jq", ".login"]),
                Some(Path::new(cwd)),
                INFO_WAIT,
            )
            .await?;
            let login = me.stdout.trim().to_string();
            if !me.ok() || login.is_empty() {
                return Err(format!("gh api user: {}", me.text()));
            }
            let fork_path = format!("{login}/{name}");
            Ok(ForkInfo {
                fork_url: format!("https://github.com/{fork_path}.git"),
                fork_path,
            })
        }
        Host::GitLabCom | Host::GitLabSelf(_) => {
            let host = addr.host.as_str();
            let original = format!("https://{host}/{}", addr.path());
            let mut args = own(&["repo", "fork", original.as_str(), "--clone=false"]);
            if renamed {
                args.extend(own(&["--name", name.as_str(), "--path", name.as_str()]));
            }
            let o = run(glab, &args, Some(Path::new(cwd)), PR_WAIT).await?;
            if !o.ok() {
                return Err(format!("glab repo fork: {}", o.text()));
            }
            let me = run(
                glab,
                &own(&["api", "user", "--hostname", host]),
                Some(Path::new(cwd)),
                INFO_WAIT,
            )
            .await?;
            let user = parse_json(&me.stdout, "glab api user")
                .ok()
                .and_then(|v| {
                    v.get("username")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .ok_or_else(|| format!("glab api user: {}", me.text()))?;
            let fork_path = format!("{user}/{name}");
            let api = format!("projects/{}", encode_path(&fork_path));
            let f = run(
                glab,
                &own(&["api", api.as_str(), "--hostname", host]),
                Some(Path::new(cwd)),
                INFO_WAIT,
            )
            .await?;
            let v = parse_json(&f.stdout, "glab api projects")
                .map_err(|_| format!("glab api projects/{fork_path}: {}", f.text()))?;
            let fork_url = v
                .get("http_url_to_repo")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("у форка {fork_path} нет адреса для клонирования"))?;
            Ok(ForkInfo {
                fork_path,
                fork_url,
            })
        }
        Host::Other(_) => Err("форк делаю только для GitHub и GitLab".into()),
    }
}

/// `git` with a check: an error text when the command fails.
async fn git_ok(args: &[&str]) -> Result<(), String> {
    let o = run("git", &own(args), None, LOCAL_WAIT).await?;
    if o.ok() {
        Ok(())
    } else {
        Err(o.text())
    }
}

// ---------------------------------------------------------------------------
// After the add: forks
// ---------------------------------------------------------------------------

/// Opens a pull request (GitHub) or a merge request (GitLab) from this clone's fork to the original.
/// `origin` is the fork, `upstream` the original; the branch is the current one. Returns the URL.
pub async fn open_pull_request(
    cfg: &AppConfig,
    root: &str,
    title: &str,
    body: &str,
) -> Result<String, String> {
    let _ = cfg;
    open_pr_with(root, title, body, &Tools::from_env()).await
}

pub(crate) async fn open_pr_with(
    root: &str,
    title: &str,
    body: &str,
    tools: &Tools,
) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("нужен заголовок".into());
    }
    let dir = checked_root(root)?;
    let origin = git_out(&dir, &["remote", "get-url", "origin"])
        .await
        .ok_or("нет origin: это не клон форка")?;
    let upstream = git_out(&dir, &["remote", "get-url", "upstream"])
        .await
        .ok_or("нет upstream: сначала сделайте форк («превратить в форк»)")?;
    let fork = parse_address(&origin)?;
    let orig = parse_address(&upstream)?;
    if fork.host == orig.host && fork.path() == orig.path() {
        return Err("origin и upstream — один репозиторий: PR в себя не открываю".into());
    }
    let branch = git_out(&dir, &["symbolic-ref", "--short", "-q", "HEAD"])
        .await
        .ok_or("нет текущей ветки")?;
    let (_, info) = remote_info(&orig, tools).await;
    let base = info?
        .default_branch
        .ok_or("не узнал основную ветку оригинала")?;
    if branch == base {
        return Err(format!(
            "ветка {base} — основная: создайте свою ветку для PR"
        ));
    }
    let out = match &orig.kind {
        Host::GitHub => {
            let args = own(&[
                "pr",
                "create",
                "--repo",
                orig.path().as_str(),
                "--head",
                format!("{}:{}", fork.owner_path, branch).as_str(),
                "--base",
                base.as_str(),
                "--title",
                title,
                "--body",
                body,
            ]);
            let o = run(&tools.gh, &args, Some(&dir), PR_WAIT).await?;
            if !o.ok() {
                return Err(format!("gh pr create: {}", o.text()));
            }
            o
        }
        Host::GitLabCom | Host::GitLabSelf(_) => {
            let args = own(&[
                "mr",
                "create",
                "--repo",
                format!("https://{}/{}", orig.host, orig.path()).as_str(),
                "--head",
                format!("https://{}/{}", fork.host, fork.path()).as_str(),
                "--source-branch",
                branch.as_str(),
                "--target-branch",
                base.as_str(),
                "--title",
                title,
                "--description",
                body,
                "--yes",
            ]);
            let o = run(&tools.glab, &args, Some(&dir), PR_WAIT).await?;
            if !o.ok() {
                return Err(format!("glab mr create: {}", o.text()));
            }
            o
        }
        Host::Other(_) => return Err("PR открываю только на GitHub и GitLab".into()),
    };
    out.stdout
        .lines()
        .rev()
        .find(|l| l.contains("http"))
        .map(|l| l.trim().to_string())
        .ok_or_else(|| "ответ без адреса PR".to_string())
}

/// "Превратить в форк": makes a fork of this clone's origin on its host, then origin = the fork and
/// upstream = the original (the clone's old origin).
pub async fn make_fork(root: &str) -> Result<String, String> {
    make_fork_with(root, &Tools::from_env()).await
}

pub(crate) async fn make_fork_with(root: &str, tools: &Tools) -> Result<String, String> {
    let dir = checked_root(root)?;
    let dir_s = dir.display().to_string();
    let origin = git_out(&dir, &["remote", "get-url", "origin"])
        .await
        .ok_or("нет origin")?;
    if git_out(&dir, &["remote", "get-url", "upstream"])
        .await
        .is_some()
    {
        return Err("upstream уже есть: похоже, это форк; ничего не меняю".into());
    }
    let addr = parse_address(&origin)?;
    if !forkable(&addr.kind) {
        return Err("форк делаю только для GitHub и GitLab".into());
    }
    let fork = create_fork(&addr, None, &dir_s, &tools.gh, &tools.glab).await?;
    git_ok(&["-C", dir_s.as_str(), "remote", "rename", "origin", "upstream"]).await?;
    git_ok(&["-C", dir_s.as_str(), "remote", "add", "origin", fork.fork_url.as_str()]).await?;
    Ok(format!(
        "origin → {} (ваш форк), upstream → {} (оригинал)",
        fork.fork_url, origin
    ))
}

/// How many commits `upstream/<default>` has that the local default does not. Reads refs only; the
/// caller fetches. `None` when either ref is missing.
pub async fn upstream_lag(root: &str) -> Option<u32> {
    let dir = expand_home(root).canonicalize().ok()?;
    let from_head = git_out(
        &dir,
        &["symbolic-ref", "--short", "refs/remotes/upstream/HEAD"],
    )
    .await
    .and_then(|s| s.strip_prefix("upstream/").map(str::to_string));
    let candidates = from_head
        .into_iter()
        .chain(["main", "master"].map(str::to_string));
    for b in candidates {
        let local = format!("refs/heads/{b}");
        let up = format!("refs/remotes/upstream/{b}");
        if !ref_exists(&dir, &local).await || !ref_exists(&dir, &up).await {
            continue;
        }
        let range = format!("{local}..{up}");
        return git_out(&dir, &["rev-list", "--count", range.as_str()])
            .await
            .and_then(|s| s.parse().ok());
    }
    None
}

async fn ref_exists(dir: &Path, reference: &str) -> bool {
    let dir_text = dir.display().to_string();
    let args = own(&[
        "-C",
        dir_text.as_str(),
        "show-ref",
        "--verify",
        "--quiet",
        reference,
    ]);
    run("git", &args, None, LOCAL_WAIT)
        .await
        .map(|o| o.ok())
        .unwrap_or(false)
}

fn checked_root(root: &str) -> Result<PathBuf, String> {
    let p = expand_home(root.trim())
        .canonicalize()
        .map_err(|_| format!("папка не найдена: {root}"))?;
    if !p.join(".git").exists() {
        return Err(format!("не git-репозиторий: {root}"));
    }
    Ok(p)
}

// ---------------------------------------------------------------------------
// Running commands
// ---------------------------------------------------------------------------

/// What a command printed and how it ended.
#[derive(Debug, Clone, Default)]
pub(crate) struct Out {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// The part of the output worth showing: the git `fatal:` line, else the last line.
    fn text(&self) -> String {
        let src = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let lines: Vec<&str> = src
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let pick = lines
            .iter()
            .rev()
            .find(|l| l.contains("fatal") || l.contains("error") || l.contains("Error"))
            .or(lines.last())
            .copied()
            .unwrap_or("");
        clip(pick)
    }

    fn combined(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

fn base(prog: &str) -> &str {
    Path::new(prog)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(prog)
}

fn missing_text(prog: &str) -> String {
    match base(prog) {
        "gh" => "gh не найден: поставьте GitHub CLI (https://cli.github.com), затем войдите: gh auth login".into(),
        "glab" => "glab не найден: поставьте GitLab CLI, затем войдите: glab auth login --hostname ХОСТ".into(),
        "git" => "git не найден: поставьте git".into(),
        other => format!("{other} не найден"),
    }
}

/// Runs a local program with a timeout; a missing program is said in words, not as an io error.
async fn run(
    prog: &str,
    args: &[String],
    cwd: Option<&Path>,
    wait: Duration,
) -> Result<Out, String> {
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    match timeout(wait, cmd.output()).await {
        Err(_) => Err(format!(
            "{}: нет ответа за {} с",
            base(prog),
            wait.as_secs()
        )),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Err(missing_text(prog)),
        Ok(Err(e)) => Err(format!("{}: {e}", base(prog))),
        Ok(Ok(o)) => Ok(Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }),
    }
}

/// `git -C dir ARGS` (the dir is the first arg); the caller passes `-C` itself.
async fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    if !dir.exists() {
        return None;
    }
    let mut full = vec!["-C".to_string(), dir.display().to_string()];
    full.extend(args.iter().map(|a| a.to_string()));
    let o = run("git", &full, None, LOCAL_WAIT).await.ok()?;
    if o.ok() {
        nonempty(o.stdout.trim())
    } else {
        None
    }
}

fn own(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn clip(s: &str) -> String {
    let t: String = s.chars().take(300).collect();
    t.trim().to_string()
}

fn parse_json(text: &str, what: &str) -> Result<Value, String> {
    serde_json::from_str(text.trim()).map_err(|_| format!("{what}: ответ не JSON"))
}

/// `group/sub/repo` as the hosting API takes it: one path segment, slashes encoded.
fn encode_path(path: &str) -> String {
    path.replace('/', "%2F")
}

/// The word after `key` in a CLI's status text (`account me (keyring)` → `me`).
fn word_after(text: &str, key: &str) -> Option<String> {
    text.split_whitespace()
        .skip_while(|w| *w != key)
        .nth(1)
        .map(|w| {
            w.trim_matches(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.'))
                .to_string()
        })
        .filter(|w| !w.is_empty())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::test_cfg;
    use std::process::Command as StdCmd;

    /// A folder under the system temp dir, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let d = std::env::temp_dir()
                .join(format!("hp-universe-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&d).unwrap();
            Scratch(d)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Writes an executable sh script `name` with `body`; returns its path.
    fn script(dir: &Path, name: &str, body: &str) -> String {
        let p = dir.join(name);
        crate::testutil::write_exec(&p, format!("#!/bin/sh\n{body}"));
        p.display().to_string()
    }

    /// Runs git with the given args (test setup only; the tests make their own repositories).
    fn git(args: &[&str]) {
        let st = StdCmd::new("git")
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = StdCmd::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git -C {} {args:?}", dir.display());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repository with one commit on `main`, made in `dir`.
    fn repo_with_commit(dir: &Path) {
        git(&["init", "-q", "-b", "main", dir.to_str().unwrap()]);
        git(&[
            "-C",
            dir.to_str().unwrap(),
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
    }

    /// A bare repository with one commit on `main`, the "remote" of the clone tests.
    fn bare_remote(dir: &Path) -> PathBuf {
        let work = dir.join("work");
        let bare = dir.join("remote.git");
        repo_with_commit(&work);
        git(&["init", "-q", "--bare", bare.to_str().unwrap()]);
        git(&[
            "-C",
            work.to_str().unwrap(),
            "push",
            "-q",
            bare.to_str().unwrap(),
            "main",
        ]);
        git(&[
            "--git-dir",
            bare.to_str().unwrap(),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ]);
        bare
    }

    /// A fake herdr-projects board: records its argv, keeps repos and settings in `data`, answers `show --json`.
    fn fake_board(dir: &Path, data: &Path) -> String {
        let body = r#"D='@DATA@'
mkdir -p "$D"; touch "$D/repos.txt" "$D/settings.txt"
echo "$*" >> '@LOG@'
case "$1" in
  add)
    repo=""; prev=""
    for a; do if [ "$prev" = "--repo" ]; then repo=$a; fi; prev=$a; done
    name=${repo##*/}
    key=$(printf '%s' "$name" | cut -c1-3 | tr 'a-z' 'A-Z')
    printf '%s\t%s\t%s\n' "$key" "$name" "$repo" >> "$D/repos.txt"
    echo "$key-1: added" ;;
  archive) echo "$2: archived" ;;
  set)
    repo=""; prev=""
    for a; do if [ "$prev" = "--repo" ]; then repo=$a; fi; prev=$a; done
    printf '%s\t%s\t%s\n' "$2" "$3" "$repo" >> "$D/settings.txt"
    echo "$2 for $repo: $3" ;;
  show)
    repo=""; prev=""
    for a; do if [ "$prev" = "--repo" ]; then repo=$a; fi; prev=$a; done
    awk -F '\t' -v r="$repo" 'BEGIN{ORS=""; print "{\"settings\":["; n=0}
      FILENAME==ARGV[1] && $2==r { if (n++) print ","; print "{\"name\":\"repo.key\",\"value\":\"" $1 "\"}" }
      FILENAME==ARGV[2] && $3==r { if (n++) print ","; print "{\"name\":\"" $1 "\",\"value\":\"" $2 "\"}" }
      END{print "]}\n"}' "$D/repos.txt" "$D/settings.txt" ;;
esac
"#;
        let body = body
            .replace("@DATA@", &data.display().to_string())
            .replace("@LOG@", &dir.join("board.log").display().to_string());
        script(dir, "herdr-projects", &body)
    }

    fn board_log(dir: &Path) -> String {
        fs::read_to_string(dir.join("board.log")).unwrap_or_default()
    }

    fn cfg_for(dir: &Path, board: String) -> AppConfig {
        AppConfig {
            herdr_projects: board,
            ..test_cfg(dir)
        }
    }

    fn no_progress(_: &[Step]) {}

    fn tools_with(gh: String, glab: String) -> Tools {
        Tools { gh, glab }
    }

    fn nothing_installed() -> Tools {
        tools_with(
            "/nonexistent/no-such-dir/gh".into(),
            "/nonexistent/no-such-dir/glab".into(),
        )
    }

    // -- folder picker ------------------------------------------------------

    #[test]
    fn complete_path_lists_folders_as_typed_without_hidden_or_files() {
        let s = Scratch::new("pick");
        let root = s.path();
        fs::create_dir_all(root.join("Projects/alpha/.git")).unwrap();
        fs::create_dir_all(root.join("Projects/Alpha2")).unwrap();
        fs::create_dir_all(root.join("Projects/beta")).unwrap();
        fs::create_dir_all(root.join("Projects/.cache")).unwrap();
        fs::write(root.join("Projects/file.txt"), "x").unwrap();
        let typed = format!("{}/Projects/al", root.display());
        let got = complete_path(&typed, &[], 10);
        let names: Vec<&str> = got.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Alpha2"]);
        assert_eq!(got[0].path, format!("{}/Projects/alpha", root.display()));
        assert!(got[0].git && !got[1].git);

        let all = complete_path(&format!("{}/Projects/", root.display()), &[], 10);
        let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha", "Alpha2", "beta"],
            "no hidden, no files"
        );

        let hidden = complete_path(&format!("{}/Projects/.", root.display()), &[], 10);
        assert_eq!(hidden.len(), 1);
        assert_eq!(hidden[0].name, ".cache");
    }

    #[test]
    fn complete_path_marks_board_folders_and_refuses_nul() {
        let s = Scratch::new("mark");
        let root = s.path();
        fs::create_dir_all(root.join("beta")).unwrap();
        let roots = vec![root.join("beta").display().to_string()];
        let got = complete_path(&format!("{}/", root.display()), &roots, 10);
        assert_eq!(got.len(), 1);
        assert!(got[0].on_board);
        assert!(complete_path("/tmp/\0x", &[], 10).is_empty());
        assert!(complete_path(&format!("{}/", root.display()), &[], 0).is_empty());
    }

    #[test]
    fn complete_path_reads_branch_and_origin_of_a_git_folder() {
        let s = Scratch::new("gitfacts");
        let repo = s.path().join("proj");
        repo_with_commit(&repo);
        git(&[
            "-C",
            repo.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "git@gitlab.example.com:group/proj.git",
        ]);
        let got = complete_path(&format!("{}/pr", s.path().display()), &[], 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].branch.as_deref(), Some("main"));
        assert_eq!(
            got[0].origin.as_deref(),
            Some("git@gitlab.example.com:group/proj.git")
        );
    }

    // -- addresses -----------------------------------------------------------

    #[test]
    fn parse_address_reads_each_form_and_host_kind() {
        let gh = parse_address("https://github.com/o/r.git").unwrap();
        assert_eq!(gh.kind, Host::GitHub);
        assert_eq!((gh.owner_path.as_str(), gh.name.as_str()), ("o", "r"));
        assert_eq!(gh.clone_url, "https://github.com/o/r.git");
        assert_eq!(gh.path(), "o/r");

        let gh2 = parse_address("github.com/o/r").unwrap();
        assert_eq!(gh2.clone_url, "https://github.com/o/r.git");

        let scp = parse_address("git@gitlab.example.com:group/sub/r.git").unwrap();
        assert_eq!(scp.kind, Host::GitLabSelf("gitlab.example.com".into()));
        assert_eq!(scp.owner_path, "group/sub");
        assert_eq!(scp.name, "r");
        assert_eq!(scp.clone_url, "git@gitlab.example.com:group/sub/r.git");

        let com = parse_address("gitlab.com/g/r").unwrap();
        assert_eq!(com.kind, Host::GitLabCom);
        assert_eq!(com.clone_url, "https://gitlab.com/g/r.git");

        let trailing = parse_address("https://gitlab.example.com/group/sub/r/").unwrap();
        assert_eq!(trailing.path(), "group/sub/r");

        let ported = parse_address("ssh://git@gitlab.example.com:2222/a/b/c.git").unwrap();
        assert_eq!(ported.clone_url, "ssh://git@gitlab.example.com:2222/a/b/c.git");
        assert_eq!(ported.name, "c");

        let other = parse_address("https://git.example.org/team/tool.git").unwrap();
        assert_eq!(other.kind, Host::Other("git.example.org".into()));
    }

    #[test]
    fn parse_address_drops_credentials_and_refuses_bad_input() {
        let a = parse_address("https://user:tok3n@gitlab.example.com/g/r.git").unwrap();
        assert_eq!(a.clone_url, "https://gitlab.example.com/g/r.git");
        for bad in [
            "",
            "o/r",
            "https://github.com/o",
            "https://github.com/o/r/tree/main",
            "https://github.com/o/r;rm",
            "https://github.com/../r",
            "ftp://github.com/o/r",
            "github.com/o r",
            "https://github.com/o/r?x=1",
        ] {
            assert!(parse_address(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn percent_is_read_from_git_progress_lines() {
        assert_eq!(
            parse_percent("Receiving objects:  45% (450/1000), 1.2 MiB"),
            Some(45)
        );
        assert_eq!(
            parse_percent("Receiving objects: 100% (1000/1000), done."),
            Some(100)
        );
        assert_eq!(parse_percent("Resolving deltas: 0% (0/3)"), Some(0));
        assert_eq!(parse_percent("Cloning into 'x'..."), None);
        assert_eq!(parse_percent("Receiving objects: 150%"), None);
    }

    #[test]
    fn the_key_suggestion_follows_the_boards_rule() {
        let none: Vec<String> = Vec::new();
        assert_eq!(suggest_key("herdr-planner", &none), "PLN");
        assert_eq!(suggest_key("gigabit-server", &none), "SRV");
        assert_eq!(suggest_key("omarchy", &none), "OMR");
        assert_eq!(suggest_key("apicache", &none), "APC");
        assert_eq!(suggest_key("speakr", &none), "SPK");
        assert_eq!(suggest_key("ditis", &none), "DTS");
        assert_eq!(suggest_key("vllm-ansible", &none), "ANS");
        assert_eq!(suggest_key("summary-service", &["SRV".to_string()]), "SS");
        assert_eq!(suggest_key("summary-service", &none), "SRV");
    }

    // -- inspect --------------------------------------------------------------

    #[tokio::test]
    async fn inspect_reads_github_write_access_and_login() {
        let s = Scratch::new("gh-inspect");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &s.path().join("data")));
        let gh = script(
            s.path(),
            "gh",
            r#"case "$1 $2" in
  "auth status") echo "Logged in to github.com account me (keyring)" >&2; exit 0 ;;
  "repo view") echo '{"viewerPermission":"WRITE","isFork":false,"parent":null,"defaultBranchRef":{"name":"main"},"nameWithOwner":"o/r","isPrivate":false}'; exit 0 ;;
esac
exit 1
"#,
        );
        let t = tools_with(gh, "/nonexistent/glab".into());
        let got = inspect_with(&cfg, "https://github.com/o/r", &t)
            .await
            .unwrap();
        assert!(!got.is_path);
        assert_eq!(got.can_write, Some(true));
        assert_eq!(got.default_branch.as_deref(), Some("main"));
        let login = got.login.unwrap();
        assert!(login.ok);
        assert_eq!(login.user.as_deref(), Some("me"));
        assert_eq!(got.suggested_name, "r");
        assert_eq!(got.clone_dir, format!("{}/r", cfg.projects_dir.display()));

        let read_only = script(
            s.path(),
            "gh-read",
            r#"case "$1 $2" in
  "auth status") exit 1 ;;
  "repo view") echo '{"viewerPermission":"READ","isFork":false,"parent":null,"defaultBranchRef":{"name":"main"},"nameWithOwner":"o/r","isPrivate":false}' ;;
esac
"#,
        );
        let t = tools_with(read_only, "/nonexistent/glab".into());
        let got = inspect_with(&cfg, "github.com/o/r", &t).await.unwrap();
        assert_eq!(got.can_write, Some(false));
        let login = got.login.unwrap();
        assert!(!login.ok);
        assert_eq!(login.hint, "Войди: gh auth login");
    }

    #[tokio::test]
    async fn inspect_reads_gitlab_access_levels_and_login_hints() {
        let s = Scratch::new("gl-inspect");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &s.path().join("data")));
        let glab = |level: u32| {
            script(
                s.path(),
                &format!("glab-{level}"),
                &format!(
                    r#"case "$1" in
  auth) echo "Logged in to gitlab.example.com as me" >&2; exit 0 ;;
  api) echo '{{"permissions":{{"project_access":{{"access_level":{level}}},"group_access":null}},"forked_from_project":null,"default_branch":"main","visibility":"internal"}}'; exit 0 ;;
esac
exit 1
"#
                ),
            )
        };
        let t = tools_with("/nonexistent/gh".into(), glab(30));
        let got = inspect_with(&cfg, "git@gitlab.example.com:group/sub/r.git", &t)
            .await
            .unwrap();
        assert_eq!(got.can_write, Some(true));
        assert_eq!(got.private, Some(true));
        assert_eq!(got.login.unwrap().user.as_deref(), Some("me"));

        let t = tools_with("/nonexistent/gh".into(), glab(10));
        let got = inspect_with(&cfg, "git@gitlab.example.com:group/sub/r.git", &t)
            .await
            .unwrap();
        assert_eq!(got.can_write, Some(false));

        let not_in = script(
            s.path(),
            "glab-out",
            r#"case "$1" in
  auth) echo "x gitlab.example.com: not logged in" >&2; exit 1 ;;
  api) echo "GET https://gitlab.example.com/api/v4/projects: 401 {message: 401 Unauthorized}" >&2; exit 1 ;;
esac
exit 1
"#,
        );
        let t = tools_with("/nonexistent/gh".into(), not_in);
        let got = inspect_with(&cfg, "git@gitlab.example.com:group/sub/r.git", &t)
            .await
            .unwrap();
        assert_eq!(got.can_write, None);
        let login = got.login.unwrap();
        assert!(!login.ok);
        assert_eq!(
            login.hint,
            "Войди: glab auth login --hostname gitlab.example.com"
        );
        assert!(got
            .notes
            .iter()
            .any(|n| n.contains("glab auth login --hostname gitlab.example.com")));
    }

    #[tokio::test]
    async fn inspect_says_how_to_install_when_the_cli_is_missing() {
        let s = Scratch::new("no-cli");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &s.path().join("data")));
        let got = inspect_with(&cfg, "https://github.com/o/r", &nothing_installed())
            .await
            .unwrap();
        let login = got.login.unwrap();
        assert!(!login.ok);
        assert!(login.hint.contains("cli.github.com"), "{}", login.hint);
        assert!(got.notes.iter().any(|n| n.contains("gh не найден")));
    }

    #[tokio::test]
    async fn inspect_a_folder_reads_branch_origin_and_the_board() {
        let s = Scratch::new("inspect-path");
        let data = s.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let cfg = cfg_for(s.path(), fake_board(s.path(), &data));
        let repo = s.path().join("gigabit-server");
        repo_with_commit(&repo);
        git(&[
            "-C",
            repo.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "https://github.com/o/gigabit-server.git",
        ]);
        fs::write(
            data.join("repos.json"),
            format!("{{\"gigabit-server\": \"{}\"}}", repo.display()),
        )
        .unwrap();
        fs::write(
            data.join("repo-keys.json"),
            r#"{"had":{"SRV":"gigabit-server"}}"#,
        )
        .unwrap();
        let got = inspect_with(&cfg, &repo.display().to_string(), &nothing_installed())
            .await
            .unwrap();
        assert!(got.is_path && got.git);
        assert_eq!(got.branch.as_deref(), Some("main"));
        assert_eq!(
            got.origin.as_deref(),
            Some("https://github.com/o/gigabit-server.git")
        );
        assert!(got.on_board);
        assert_eq!(got.board_name.as_deref(), Some("gigabit-server"));
        assert_eq!(got.suggested_name, "gigabit-server");
        assert_eq!(
            got.suggested_key, "GS",
            "SRV is taken, so the initials are next"
        );
    }

    // -- adding ---------------------------------------------------------------

    fn request(mode: Mode, name: &str) -> AddRequest {
        AddRequest {
            mode,
            path: None,
            url: None,
            name: name.into(),
            key: None,
            dir: None,
        }
    }

    #[tokio::test]
    async fn add_folder_on_gitlab_registers_it_and_sets_the_local_merge_settings() {
        let s = Scratch::new("add-folder");
        let data = s.path().join("data");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &data));
        let repo = s.path().join("gigabit-server");
        repo_with_commit(&repo);
        git(&[
            "-C",
            repo.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "git@gitlab.example.com:group/gigabit-server.git",
        ]);
        let mut req = request(Mode::Folder, "gigabit-server");
        req.path = Some(repo.display().to_string());
        let t = nothing_installed();
        let seen = std::sync::Mutex::new(Vec::<String>::new());
        let progress = |steps: &[Step]| {
            let line: Vec<String> = steps
                .iter()
                .map(|s| format!("{}:{}", s.id, s.state))
                .collect();
            seen.lock().unwrap().push(line.join(" "));
        };
        let got = add_at(&cfg, &req, &progress, &t).await.unwrap();
        assert_eq!(got.key, "GIG");
        assert_eq!(got.root, repo.canonicalize().unwrap().display().to_string());
        assert_eq!(
            got.settings,
            vec![
                "merge.pr false".to_string(),
                "merge.checks none".to_string()
            ]
        );
        assert!(got.notes.iter().any(|n| n.contains("merge.pr false")));
        assert!(got.notes.iter().any(|n| n.contains("архиве")));
        assert_eq!(
            got.origin.as_deref(),
            Some("git@gitlab.example.com:group/gigabit-server.git")
        );
        let log = board_log(s.path());
        assert!(
            log.contains("add Знакомство с проектом gigabit-server --repo"),
            "{log}"
        );
        assert!(log.contains("archive GIG-1"), "{log}");
        assert!(
            log.contains("set merge.pr false --repo gigabit-server"),
            "{log}"
        );
        let states = seen.lock().unwrap();
        assert!(states.first().unwrap().contains("clone:wait"));
        assert!(states.last().unwrap().contains("settings:ok"));
    }

    #[tokio::test]
    async fn add_folder_refuses_a_folder_that_is_not_git_and_a_name_that_differs() {
        let s = Scratch::new("add-refuse");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &s.path().join("data")));
        let plain = s.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        let mut req = request(Mode::Folder, "plain");
        req.path = Some(plain.display().to_string());
        let err = add_at(&cfg, &req, &no_progress, &nothing_installed())
            .await
            .unwrap_err();
        assert!(err.contains("не git-репозиторий"), "{err}");
        assert!(err.starts_with("шаг «проверка папки»"), "{err}");

        let repo = s.path().join("real");
        repo_with_commit(&repo);
        let mut req = request(Mode::Folder, "other-name");
        req.path = Some(repo.display().to_string());
        let err = add_at(&cfg, &req, &no_progress, &nothing_installed())
            .await
            .unwrap_err();
        assert!(err.contains("по имени папки"), "{err}");
        assert!(!board_log(s.path()).contains("add "), "nothing registered");
    }

    #[tokio::test]
    async fn add_clone_study_sets_research_only_and_clones_into_the_folder() {
        let s = Scratch::new("add-study");
        let data = s.path().join("data");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &data));
        let bare = bare_remote(s.path());
        let mut req = request(Mode::Study, "lib");
        req.url = Some(bare.display().to_string());
        req.dir = Some(s.path().join("Projects").display().to_string());
        let got = add_at(&cfg, &req, &no_progress, &nothing_installed())
            .await
            .unwrap();
        assert_eq!(got.settings, vec!["research.only true".to_string()]);
        assert_eq!(got.key, "LIB");
        assert!(s.path().join("Projects/lib/.git").exists());
        assert_eq!(
            got.origin.as_deref(),
            Some(bare.display().to_string().as_str())
        );
        assert!(got.upstream.is_none());
        assert!(got.notes.iter().any(|n| n.contains("research.only")));
        assert!(board_log(s.path()).contains("set research.only true --repo lib"));
    }

    #[tokio::test]
    async fn add_clone_refuses_a_target_that_is_not_empty() {
        let s = Scratch::new("add-busy");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &s.path().join("data")));
        let bare = bare_remote(s.path());
        let target = s.path().join("Projects/lib");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), "mine").unwrap();
        let mut req = request(Mode::Clone, "lib");
        req.url = Some(bare.display().to_string());
        req.dir = Some(s.path().join("Projects").display().to_string());
        let err = add_at(&cfg, &req, &no_progress, &nothing_installed())
            .await
            .unwrap_err();
        assert!(err.contains("не перезаписываю"), "{err}");
        assert_eq!(fs::read_to_string(target.join("keep.txt")).unwrap(), "mine");
        assert!(!target.join(".git").exists());
        assert!(!board_log(s.path()).contains("add "), "nothing registered");
    }

    #[tokio::test]
    async fn add_fork_clones_the_fork_and_names_the_original_as_upstream() {
        let s = Scratch::new("add-fork");
        let data = s.path().join("data");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &data));
        let bare = bare_remote(s.path());
        // The fork host: `glab repo fork` does nothing here; `glab api` answers with the fork's clone URL.
        let glab = script(
            s.path(),
            "glab",
            &format!(
                r#"case "$1 $2" in
  "repo fork") echo "created" ; exit 0 ;;
  "api user") echo '{{"username":"tester"}}' ; exit 0 ;;
  "api projects/tester%2Fproj") echo '{{"http_url_to_repo":"{}"}}' ; exit 0 ;;
esac
exit 1
"#,
                bare.display()
            ),
        );
        let t = tools_with("/nonexistent/gh".into(), glab);
        let mut req = request(Mode::Fork, "proj");
        req.url = Some("git@gitlab.example.com:group/proj.git".into());
        req.dir = Some(s.path().join("Projects").display().to_string());
        let got = add_at(&cfg, &req, &no_progress, &t).await.unwrap();
        assert_eq!(got.fork_of.as_deref(), Some("group/proj"));
        assert_eq!(
            got.origin.as_deref(),
            Some(bare.display().to_string().as_str())
        );
        assert_eq!(
            got.upstream.as_deref(),
            Some("git@gitlab.example.com:group/proj.git")
        );
        assert_eq!(
            got.settings,
            vec![
                "merge.pr false".to_string(),
                "merge.checks none".to_string()
            ]
        );
        assert!(got.notes.iter().any(|n| n.contains("форк ← group/proj")));
    }

    #[tokio::test]
    async fn add_fork_refuses_a_plain_clone_address() {
        let s = Scratch::new("add-fork-other");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &s.path().join("data")));
        let mut req = request(Mode::Fork, "x");
        req.url = Some("https://git.example.org/team/x.git".into());
        let err = add_at(&cfg, &req, &no_progress, &nothing_installed())
            .await
            .unwrap_err();
        assert!(err.contains("форк делаю для GitHub и GitLab"), "{err}");
    }

    #[tokio::test]
    async fn add_takes_the_key_asked_for_and_keeps_the_old_one_working() {
        let s = Scratch::new("add-key");
        let data = s.path().join("data");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &data));
        let bare = bare_remote(s.path());
        let mut req = request(Mode::Clone, "planner");
        req.url = Some(bare.display().to_string());
        req.dir = Some(s.path().join("Projects").display().to_string());
        req.key = Some("PLX".into());
        let got = add_at(&cfg, &req, &no_progress, &nothing_installed())
            .await
            .unwrap();
        assert_eq!(got.key, "PLX");
        assert!(
            got.notes.iter().any(|n| n.contains("старый ключ PLA")),
            "{:?}",
            got.notes
        );
        assert!(board_log(s.path()).contains("set repo.key PLX --repo planner"));
    }

    #[tokio::test]
    async fn add_refuses_a_name_that_is_not_one_safe_segment() {
        let s = Scratch::new("add-name");
        let cfg = cfg_for(s.path(), fake_board(s.path(), &s.path().join("data")));
        for bad in ["a/b", "..", ".hidden", "", "-x"] {
            let mut req = request(Mode::Clone, bad);
            req.url = Some("https://github.com/o/r".into());
            assert!(add_at(&cfg, &req, &no_progress, &nothing_installed())
                .await
                .is_err());
        }
    }

    // -- forks after the add -----------------------------------------------------

    #[tokio::test]
    async fn open_pull_request_asks_github_for_a_pr_from_the_fork() {
        let s = Scratch::new("pr-gh");
        let repo = s.path().join("r");
        repo_with_commit(&repo);
        git(&[
            "-C",
            repo.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "https://github.com/tester/r.git",
        ]);
        git(&[
            "-C",
            repo.to_str().unwrap(),
            "remote",
            "add",
            "upstream",
            "https://github.com/o/r.git",
        ]);
        git(&[
            "-C",
            repo.to_str().unwrap(),
            "checkout",
            "-q",
            "-b",
            "feature",
        ]);
        let log = s.path().join("gh.log");
        let gh = script(
            s.path(),
            "gh",
            &format!(
                r#"echo "$*" >> '{log}'
case "$1 $2" in
  "repo view") echo '{{"viewerPermission":"READ","isFork":false,"parent":null,"defaultBranchRef":{{"name":"main"}},"nameWithOwner":"o/r","isPrivate":false}}' ;;
  "pr create") echo "https://github.com/o/r/pull/7" ;;
esac
"#,
                log = log.display()
            ),
        );
        let t = tools_with(gh, "/nonexistent/glab".into());
        let url = open_pr_with(&repo.display().to_string(), "Фича", "тело", &t)
            .await
            .unwrap();
        assert_eq!(url, "https://github.com/o/r/pull/7");
        let calls = fs::read_to_string(&log).unwrap();
        assert!(
            calls.contains("pr create --repo o/r --head tester:feature --base main"),
            "{calls}"
        );
    }

    #[tokio::test]
    async fn open_pull_request_refuses_without_an_upstream() {
        let s = Scratch::new("pr-none");
        let repo = s.path().join("r");
        repo_with_commit(&repo);
        let err = open_pr_with(&repo.display().to_string(), "x", "", &nothing_installed())
            .await
            .unwrap_err();
        assert!(err.contains("нет origin"), "{err}");
    }

    #[tokio::test]
    async fn make_fork_turns_the_clone_into_a_fork_with_the_original_as_upstream() {
        let s = Scratch::new("make-fork");
        let bare = bare_remote(s.path());
        let repo = s.path().join("proj");
        repo_with_commit(&repo);
        git(&[
            "-C",
            repo.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "git@gitlab.example.com:group/proj.git",
        ]);
        let glab = script(
            s.path(),
            "glab",
            &format!(
                r#"case "$1 $2" in
  "repo fork") echo created; exit 0 ;;
  "api user") echo '{{"username":"tester"}}'; exit 0 ;;
  "api projects/tester%2Fproj") echo '{{"http_url_to_repo":"{}"}}'; exit 0 ;;
esac
exit 1
"#,
                bare.display()
            ),
        );
        let t = tools_with("/nonexistent/gh".into(), glab);
        let msg = make_fork_with(&repo.display().to_string(), &t)
            .await
            .unwrap();
        assert!(msg.contains("ваш форк"), "{msg}");
        assert_eq!(
            git_in(&repo, &["remote", "get-url", "origin"]),
            bare.display().to_string()
        );
        assert_eq!(
            git_in(&repo, &["remote", "get-url", "upstream"]),
            "git@gitlab.example.com:group/proj.git"
        );
    }

    #[tokio::test]
    async fn upstream_lag_counts_the_commits_upstream_has_and_local_does_not() {
        let s = Scratch::new("lag");
        let repo = s.path().join("r");
        repo_with_commit(&repo);
        let dir = repo.to_str().unwrap();
        let first = git_in(&repo, &["rev-parse", "HEAD"]);
        git(&[
            "-C",
            dir,
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "two",
        ]);
        let second = git_in(&repo, &["rev-parse", "HEAD"]);
        git(&[
            "-C",
            dir,
            "update-ref",
            "refs/remotes/upstream/main",
            second.as_str(),
        ]);
        git(&["-C", dir, "update-ref", "refs/heads/main", first.as_str()]);
        assert_eq!(upstream_lag(dir).await, Some(1));
        git(&["-C", dir, "update-ref", "refs/heads/main", second.as_str()]);
        assert_eq!(upstream_lag(dir).await, Some(0));
        git(&["-C", dir, "update-ref", "-d", "refs/remotes/upstream/main"]);
        assert_eq!(upstream_lag(dir).await, None);
    }

    #[test]
    fn the_board_task_id_and_the_key_setting_are_read() {
        assert_eq!(added_id("SRC-1: added\n").as_deref(), Some("SRC-1"));
        assert_eq!(added_id("newrepo/1: added\n").as_deref(), Some("newrepo/1"), "the board's own form");
        assert_eq!(added_id("../x: added"), None);
        assert_eq!(added_id("/abs: added"), None);
        assert_eq!(added_id("nothing here"), None);
        let v: Value = serde_json::from_str(
            r#"{"settings":[{"name":"repo.key","value":"PLN"},{"name":"repo.key","value":"PLX"},{"name":"research.only","value":true}]}"#,
        )
        .unwrap();
        assert_eq!(setting_text(&v, "repo.key").as_deref(), Some("PLX"));
        assert_eq!(setting_text(&v, "research.only").as_deref(), Some("true"));
        assert_eq!(setting_text(&v, "merge.pr"), None);
    }
    #[test]
    fn the_boards_json_is_found_after_what_a_shell_printed() {
        let out = "Welcome\n{\"motd\": 1}\n{\"ok\": true, \"settings\": []}\n";
        assert_eq!(last_json(out).unwrap()["ok"], true, "the board's own object wins");
        assert_eq!(last_json("{\"a\": 2}").unwrap()["a"], 2);
        assert!(last_json("no json").is_none());
    }
}
