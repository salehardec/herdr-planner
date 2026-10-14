//! The «Новый проект» form's server side: the folder browser, the machines
//! (`herdr machine list --json`), the profiles (`herdr-projects profile list`)
//! and the creation itself (`herdr-projects new`). Ported from the Tern plugin
//! (0.17.1): several repositories (`--repo PATH[@MACHINE]`, up to six), the
//! thread and coordinator profiles, a remote repository is never looked at.

use crate::actions::Failure;
use crate::app::Shared;
use crate::cards::argv_projects;
use crate::config::home;
use crate::model::slug_preview;
use crate::sources::profiles::valid_profile;
use crate::sources::{runner, tasks};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

pub const MAX_REPOS: usize = 6;
const LIST_TIMEOUT: Duration = Duration::from_secs(8);
const NEW_TIMEOUT: Duration = Duration::from_secs(20);
/// A folder with more subfolders than this is cut.
const MAX_ENTRIES: usize = 2000;

fn fail<T>(status: u16, text: impl Into<String>) -> Result<T, Failure> {
    Err((status, text.into()))
}

// ---------------------------------------------------------------------------
// Paths

/// `~` and `~/…` expanded; surrounding blanks and trailing slashes dropped.
pub fn clean_path(raw: &str) -> String {
    let p = raw.trim();
    let expanded = if p == "~" {
        home().display().to_string()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest).display().to_string()
    } else {
        p.to_string()
    };
    let trimmed = expanded.trim_end_matches('/');
    if trimmed.is_empty() && !expanded.is_empty() {
        "/".into()
    } else {
        trimmed.to_string()
    }
}

/// «all_gis_services_next» → «All Gis Services Next» (the words of a folder name).
pub fn name_from_basename(path: &str) -> String {
    let base = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    base.split(|c: char| matches!(c, '_' | '-' | '.') || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) if f.is_ascii() => f.to_ascii_uppercase().to_string() + cs.as_str(),
                Some(f) => f.to_string() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parent_dir(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) => Some("/".into()),
        Some((parent, _)) => Some(parent.to_string()),
        None => None,
    }
}

// ---------------------------------------------------------------------------
// The folder browser

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub name: String,
    /// The folder holds a `.git` (a directory, or the file of a worktree).
    pub repo: bool,
}

/// Subfolders of `path` by name without case; hidden ones and files are not
/// shown. None when the folder cannot be read.
pub fn list_dirs(path: &str) -> Option<Vec<Entry>> {
    let rd = std::fs::read_dir(path).ok()?;
    let mut out: Vec<Entry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !e.path().is_dir() {
                return None;
            }
            let repo = e.path().join(".git").exists();
            Some(Entry { name, repo })
        })
        .take(MAX_ENTRIES)
        .collect();
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.name.cmp(&b.name))
    });
    Some(out)
}

/// Where the browser starts: the path in the field, the folder of the previous
/// choice, ~/Projects, ~, «/» — the first that can be read wins; only absolute
/// paths count.
pub fn start_candidates(field: &str, last_dir: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut add = |p: String| {
        if p.starts_with('/') {
            out.push(p);
        }
    };
    if !field.trim().is_empty() {
        add(clean_path(field));
    }
    if let Some(d) = last_dir {
        add(d.to_string());
    }
    let h = home();
    add(h.join("Projects").display().to_string());
    add(h.display().to_string());
    add("/".into());
    out
}

/// `{path, parent, entries}` for the folder `path`, or the first readable start
/// candidate when `start` is set.
pub fn browse(path: &str, start: bool, last_dir: Option<&str>) -> Result<Value, String> {
    let cands = if start {
        start_candidates(path, last_dir)
    } else {
        vec![clean_path(path)]
    };
    for c in &cands {
        if !c.starts_with('/') {
            continue;
        }
        if let Some(entries) = list_dirs(c) {
            return Ok(json!({"path": c, "parent": parent_dir(c), "entries": entries}));
        }
    }
    Err(format!(
        "Папка не читается: {}",
        cands.first().map(String::as_str).unwrap_or("")
    ))
}

/// Tab in the «Папка» field: the path completed as far as the subfolders agree.
/// `candidates` are the folders it could still be; `path` is unchanged (and
/// there are none) when there is nothing to complete.
pub fn complete(raw: &str) -> Value {
    let expanded = if raw.trim() == "~" || raw.trim().starts_with("~/") {
        let t = raw.trim();
        if t == "~" {
            format!("{}/", home().display())
        } else {
            home().join(&t[2..]).display().to_string() + if t.ends_with('/') { "/" } else { "" }
        }
    } else {
        raw.trim().to_string()
    };
    let (base, prefix) = match expanded.rsplit_once('/') {
        Some(("", p)) => ("/".to_string(), p.to_string()),
        Some((b, p)) => (b.to_string(), p.to_string()),
        None => {
            return json!({"path": expanded, "candidates": Vec::<String>::new(), "unchanged": true})
        }
    };
    let mut found: Vec<String> = list_dirs(&base)
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.starts_with(&prefix))
        .collect();
    // Hidden folders are offered only when asked for with a dot.
    if prefix.starts_with('.') {
        found = std::fs::read_dir(&base)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with(&prefix))
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
    }
    let join = |name: &str| format!("{}/{}", base.trim_end_matches('/'), name);
    let path = match found.as_slice() {
        [] => expanded.clone(),
        [one] => format!("{}/", join(one)),
        many => {
            let mut common: Vec<char> = many[0].chars().collect();
            for n in &many[1..] {
                let len = common
                    .iter()
                    .zip(n.chars())
                    .take_while(|(a, b)| **a == *b)
                    .count();
                common.truncate(len);
            }
            join(&common.into_iter().collect::<String>())
        }
    };
    json!({"unchanged": path == expanded && found.is_empty(), "path": path, "candidates": found})
}

// ---------------------------------------------------------------------------
// Machines and profiles

/// `herdr machine list --json`: the labels of the enabled machines, in order.
/// None when the reply is no JSON list at all (the form then asks for the name
/// by hand, as it does for an empty list).
pub fn parse_machines(stdout: &str) -> Option<Vec<String>> {
    let rows = serde_json::from_str::<Value>(stdout).ok()?;
    let rows = rows
        .as_array()
        .cloned()
        .or_else(|| rows.get("machines").and_then(Value::as_array).cloned())?;
    let mut out: Vec<String> = Vec::new();
    for row in rows {
        let enabled = row.get("enabled").and_then(Value::as_bool) != Some(false);
        let label = row
            .get("label")
            .or_else(|| row.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if enabled && !label.is_empty() && !out.iter().any(|m| m == label) {
            out.push(label.to_string());
        }
    }
    Some(out)
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Profiles {
    pub all: Vec<String>,
    /// Allowed for threads / the coordinator in projects without their own list.
    pub threads: Vec<String>,
    pub coordinators: Vec<String>,
    pub thread_default: Option<String>,
    pub coordinator_default: Option<String>,
}

/// `herdr-projects profile list`: the profile names, the defaults for new
/// projects and the profiles threads / the coordinator may use («every
/// profile» = all of them). None when no profile line was found.
pub fn parse_profiles(stdout: &str) -> Option<Profiles> {
    let mut out = Profiles::default();
    let (mut threads, mut coordinators): (Option<Vec<String>>, Option<Vec<String>>) = (None, None);
    let mut in_list = false;
    let names = |value: &str, all: &[String]| -> Vec<String> {
        if value == "every profile" {
            all.to_vec()
        } else if value == "none" {
            vec![]
        } else {
            value
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|n| !n.is_empty())
                .map(String::from)
                .collect()
        }
    };
    for line in stdout.lines() {
        if line.starts_with("Profiles (") {
            in_list = true;
        } else if in_list && line.starts_with("  ") && !line.trim().is_empty() {
            if let Some(name) = line.split_whitespace().next() {
                if !out.all.iter().any(|n| n == name) {
                    out.all.push(name.to_string());
                }
            }
        } else {
            in_list = false;
            if line.starts_with("Defaults for new projects") {
                let pick = |key: &str| {
                    line.split(key)
                        .nth(1)
                        .and_then(|r| r.trim_start().strip_prefix('='))
                        .and_then(|r| {
                            r.split(|c: char| c == ',' || c.is_whitespace())
                                .find(|w| !w.is_empty())
                        })
                        .map(String::from)
                };
                out.thread_default = pick("thread_profile");
                out.coordinator_default = pick("coordinator_profile");
            } else if let Some(rest) = line.strip_prefix("Allowed for ") {
                if let Some((who, value)) = rest.split_once(':') {
                    if who.contains(" in projects") {
                        let list = names(value.trim(), &out.all);
                        if who.contains("coordinator") {
                            coordinators = Some(list);
                        } else if who.contains("thread") {
                            threads = Some(list);
                        }
                    }
                }
            }
        }
    }
    if out.all.is_empty() {
        return None;
    }
    out.threads = threads.unwrap_or_else(|| out.all.clone());
    out.coordinators = coordinators.unwrap_or_else(|| out.all.clone());
    Some(out)
}

/// What the form reads each time it opens; a failed read is `null` and the
/// form works without it.
pub async fn options(app: &Shared) -> Value {
    let machines_argv = runner::argv([app.config.herdr.as_str(), "machine", "list", "--json"]);
    let profiles_argv = argv_projects(app, &["profile", "list"]);
    let (machines, profiles) = tokio::join!(
        runner::run(&machines_argv, None, LIST_TIMEOUT),
        runner::run(&profiles_argv, None, LIST_TIMEOUT)
    );
    let machines = machines
        .ok()
        .filter(|o| o.ok())
        .and_then(|o| parse_machines(&o.stdout));
    let profiles = profiles
        .ok()
        .filter(|o| o.ok())
        .and_then(|o| parse_profiles(&o.stdout));
    let last_dir = app.state.lock().unwrap().last_dir.clone();
    json!({"machines": machines, "profiles": profiles, "last_dir": last_dir})
}

// ---------------------------------------------------------------------------
// Creating

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RepoReq {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub machine: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewReq {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub repos: Vec<RepoReq>,
    #[serde(default)]
    pub thread_profile: String,
    #[serde(default)]
    pub coordinator_profile: String,
}

/// A repository to pass: its cleaned path and machine («» = this one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub path: String,
    pub machine: String,
}

/// The repositories to pass. A spare row with nothing in it is skipped; a
/// machine without a path, or a machine name `--repo` could not carry, is an
/// error. A remote path is taken as typed.
pub fn repos_of(rows: &[RepoReq]) -> Result<Vec<Repo>, String> {
    if rows.len() > MAX_REPOS {
        return Err(format!("репозиториев не больше {MAX_REPOS}"));
    }
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let machine = r.machine.trim().to_string();
        let path = if machine.is_empty() {
            clean_path(&r.path)
        } else {
            let t = r.path.trim();
            let t = t.trim_end_matches('/');
            if t.is_empty() && !r.path.trim().is_empty() {
                "/"
            } else {
                t
            }
            .to_string()
        };
        if path.is_empty() {
            if i == 0 || !machine.is_empty() {
                return Err(if i == 0 {
                    "Нужна папка проекта".into()
                } else {
                    format!("Нужна папка репозитория {}", i + 1)
                });
            }
            continue;
        }
        if machine.chars().any(|c| c.is_whitespace() || c == '@') {
            return Err(format!("Имя машины без пробелов и @: {machine}"));
        }
        if path.chars().any(char::is_control) || (path.contains('@') && machine.is_empty()) {
            return Err(format!("Неверный путь: {path}"));
        }
        out.push(Repo { path, machine });
    }
    if out.is_empty() {
        return Err("Нужна папка проекта".into());
    }
    Ok(out)
}

/// The `herdr-projects new` arguments after `--root`. Every value is joined to
/// its flag (`--goal=…`), so a value that starts with a dash is still a value,
/// and the name follows `--`.
pub fn new_args(name: &str, repos: &[Repo], goal: &str, tprof: &str, cprof: &str) -> Vec<String> {
    let mut args = vec!["new".to_string()];
    for r in repos {
        args.push(if r.machine.is_empty() {
            format!("--repo={}", r.path)
        } else {
            format!("--repo={}@{}", r.path, r.machine)
        });
    }
    if !goal.is_empty() {
        args.push(format!("--goal={goal}"));
    }
    if !tprof.is_empty() {
        args.push(format!("--thread-profile={tprof}"));
    }
    if !cprof.is_empty() {
        args.push(format!("--coordinator-profile={cprof}"));
    }
    args.push("--".into());
    args.push(name.to_string());
    args
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `herdr-projects new` and then, when the name has no ASCII to make a slug of,
/// `set SLUG name NAME`. Returns the new project's slug and warnings.
pub async fn create(app: &Shared, req: &NewReq) -> Result<Value, Failure> {
    let repos = repos_of(&req.repos).map_err(|e| (400, e))?;
    let mut warnings: Vec<String> = Vec::new();
    let mut all_git = true;
    for r in repos.iter().filter(|r| r.machine.is_empty()) {
        let p = Path::new(&r.path);
        if !p.is_dir() {
            return fail(400, format!("Папка не найдена: {}", r.path));
        }
        all_git &= p.join(".git").exists();
    }
    if !all_git {
        warnings.push("В папке нет .git — репозиторий без git".into());
    }
    let first = &repos[0];
    let mut name = one_line(
        &req.name
            .chars()
            .filter(|c| !c.is_control())
            .collect::<String>(),
    );
    if name.is_empty() {
        name = name_from_basename(&first.path);
    }
    if name.is_empty() {
        return fail(400, "Нужно название");
    }
    if name.chars().count() > 120 {
        return fail(400, "Название длиннее 120 знаков");
    }
    let goal = one_line(&req.goal);
    if goal.chars().count() > 2000 {
        return fail(400, "Цель длиннее 2000 знаков");
    }
    for p in [&req.thread_profile, &req.coordinator_profile] {
        if !p.is_empty() && !valid_profile(p) {
            return fail(400, "неверное имя профиля");
        }
    }
    let mut slug = slug_preview(&name);
    let (mut pass_name, mut set_name) = (name.clone(), None);
    if slug.is_empty() {
        pass_name = name_from_basename(&first.path);
        slug = slug_preview(&pass_name);
        set_name = Some(name.clone());
        if slug.is_empty() {
            return fail(
                400,
                "Не удалось получить имя проекта из папки, введите название латиницей",
            );
        }
    }
    let exists = app
        .snapshot
        .read()
        .unwrap()
        .projects
        .iter()
        .any(|p| p.slug == slug)
        || app.config.project_dir(&slug).exists();
    if exists {
        return fail(409, format!("Проект {slug} уже есть"));
    }
    let args = new_args(
        &pass_name,
        &repos,
        &goal,
        &req.thread_profile,
        &req.coordinator_profile,
    );
    let argv = argv_projects(app, &args.iter().map(String::as_str).collect::<Vec<_>>());
    let out = match runner::run(&argv, None, NEW_TIMEOUT).await {
        Ok(o) => o,
        Err(e) => return fail(502, e),
    };
    if !out.ok() {
        let text = if out.stderr.trim().is_empty() {
            out.stdout.trim()
        } else {
            out.stderr.trim()
        };
        let msg = tasks::clip(&one_line(text), 400);
        return fail(
            502,
            if msg.is_empty() {
                format!("exit {}", out.status)
            } else {
                msg
            },
        );
    }
    let created = out
        .stdout
        .split("created `")
        .nth(1)
        .and_then(|r| r.split('`').next())
        .filter(|s| !s.is_empty() && !s.contains('/'))
        .map(String::from)
        .unwrap_or(slug);
    if let Some(n) = set_name {
        let argv = argv_projects(app, &["set", &created, "name", &n]);
        match runner::run(&argv, None, Duration::from_secs(15)).await {
            Ok(o) if o.ok() => {}
            Ok(o) => warnings.push(format!(
                "Имя не записано: {}",
                tasks::clip(&one_line(&o.error()), 200)
            )),
            Err(e) => warnings.push(format!("Имя не записано: {e}")),
        }
    }
    if first.machine.is_empty() {
        if let Some(parent) = parent_dir(&first.path) {
            app.state.lock().unwrap().last_dir = Some(parent);
            app.save_state();
        }
    }
    app.refresh_now.notify_one();
    Ok(json!({"ok": true, "slug": created, "warnings": warnings}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_folders() {
        assert_eq!(
            name_from_basename("/home/u/all_gis_services_next"),
            "All Gis Services Next"
        );
        assert_eq!(name_from_basename("/p/herdr-planner/"), "Herdr Planner");
        assert_eq!(name_from_basename("/p/проект_один"), "проект один");
        assert_eq!(name_from_basename("/"), "");
    }

    #[test]
    fn paths_are_cleaned_and_split() {
        assert_eq!(clean_path("  /a/b///"), "/a/b");
        assert_eq!(clean_path("/"), "/");
        assert_eq!(clean_path("~/x/"), home().join("x").display().to_string());
        assert_eq!(clean_path("~"), home().display().to_string());
        assert_eq!(parent_dir("/a/b"), Some("/a".into()));
        assert_eq!(parent_dir("/a"), Some("/".into()));
        assert_eq!(parent_dir("/"), None);
        assert_eq!(parent_dir("rel"), None);
    }

    fn tree(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("hp-np-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in [
            "Beta/.git",
            "alpha",
            "Zed/inner",
            ".hidden",
            "proj_one",
            "proj_two",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("file.txt"), "x").unwrap();
        // The .git of a worktree is a file.
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: x").unwrap();
        root
    }

    #[test]
    fn folders_are_listed_sorted_without_hidden_ones_and_files() {
        let root = tree("list");
        let got = list_dirs(root.to_str().unwrap()).unwrap();
        let names: Vec<(&str, bool)> = got.iter().map(|e| (e.name.as_str(), e.repo)).collect();
        assert_eq!(
            names,
            [
                ("alpha", false),
                ("Beta", true),
                ("proj_one", false),
                ("proj_two", false),
                ("wt", true),
                ("Zed", false)
            ]
        );
        assert!(list_dirs("/нет/такой").is_none());
        let b = browse(root.to_str().unwrap(), false, None).unwrap();
        assert_eq!(b["path"], root.to_str().unwrap());
        assert_eq!(b["entries"].as_array().unwrap().len(), 6);
        assert!(browse("/нет/такой", false, None).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_browser_starts_at_the_first_readable_candidate() {
        let root = tree("start");
        let c = start_candidates("/нет/такой", Some(root.to_str().unwrap()));
        assert_eq!(c[0], "/нет/такой");
        assert_eq!(c[1], root.to_str().unwrap());
        assert_eq!(c.last().map(String::as_str), Some("/"));
        assert!(start_candidates("относительный", None)
            .iter()
            .all(|p| p.starts_with('/')));
        let b = browse("/нет/такой", true, Some(root.to_str().unwrap())).unwrap();
        assert_eq!(
            b["path"],
            root.to_str().unwrap(),
            "the unreadable field path is passed over"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn tab_completes_as_far_as_the_folders_agree() {
        let root = tree("complete");
        let r = root.to_str().unwrap();
        let one = complete(&format!("{r}/Ze"));
        assert_eq!(one["path"], format!("{r}/Zed/"));
        let many = complete(&format!("{r}/proj_"));
        assert_eq!(many["path"], format!("{r}/proj_"));
        assert_eq!(many["candidates"], json!(["proj_one", "proj_two"]));
        let dir = complete(&format!("{r}/"));
        assert!(dir["candidates"].as_array().unwrap().len() == 6 && dir["path"] == format!("{r}/"));
        let none = complete(&format!("{r}/qqq"));
        assert_eq!(none["unchanged"], true);
        let hidden = complete(&format!("{r}/.hid"));
        assert_eq!(hidden["path"], format!("{r}/.hidden/"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn machines_from_the_json_list() {
        assert_eq!(
            parse_machines(
                r#"[{"label":"mac","enabled":true},{"name":"old","enabled":false},{"label":"mac"},{"label":"nas"},{"label":""}]"#
            ),
            Some(vec!["mac".into(), "nas".into()])
        );
        assert_eq!(parse_machines("[]"), Some(vec![]));
        assert_eq!(
            parse_machines(r#"{"machines":[{"label":"a"}]}"#),
            Some(vec!["a".into()])
        );
        assert_eq!(parse_machines("не json"), None);
        assert_eq!(parse_machines("42"), None);
    }

    const PROFILE_LIST: &str = "Profiles (built-ins: the agent CLIs installed and signed in here: pi, claude, omp):\n  claude         claude · model claude-opus-5-5 — Координатор\n  claude-sonnet  claude · model claude-sonnet-5-5 — Тред по умолчанию\n  pi             pi · default model  (built-in)\n\nDefaults for new projects: thread_profile = claude-sonnet, coordinator_profile = claude\nAllowed for threads in projects without their own list: claude-sonnet, pi\nAllowed for the coordinator in projects without their own list: claude\n";

    #[test]
    fn profiles_from_the_list() {
        let p = parse_profiles(PROFILE_LIST).unwrap();
        assert_eq!(p.all, ["claude", "claude-sonnet", "pi"]);
        assert_eq!(p.threads, ["claude-sonnet", "pi"]);
        assert_eq!(p.coordinators, ["claude"]);
        assert_eq!(p.thread_default.as_deref(), Some("claude-sonnet"));
        assert_eq!(p.coordinator_default.as_deref(), Some("claude"));
        let every = parse_profiles("Profiles (x):\n  a  b\n  c  d\nAllowed for threads in projects without their own list: every profile\nAllowed for the coordinator in projects without their own list: none\n").unwrap();
        assert_eq!(every.threads, ["a", "c"]);
        assert!(every.coordinators.is_empty());
        let bare = parse_profiles("Profiles (x):\n  a  b\n").unwrap();
        assert_eq!(bare.threads, ["a"], "no «Allowed» lines: every profile");
        assert_eq!(parse_profiles("Нет профилей"), None);
    }

    fn row(path: &str, machine: &str) -> RepoReq {
        RepoReq {
            path: path.into(),
            machine: machine.into(),
        }
    }

    #[test]
    fn repositories_to_pass() {
        let ok = repos_of(&[row(" /a/b/ ", ""), row("", ""), row("/srv/x/", "mac")]).unwrap();
        assert_eq!(
            ok,
            [
                Repo {
                    path: "/a/b".into(),
                    machine: String::new()
                },
                Repo {
                    path: "/srv/x".into(),
                    machine: "mac".into()
                }
            ]
        );
        assert_eq!(repos_of(&[row("", "")]).unwrap_err(), "Нужна папка проекта");
        assert_eq!(
            repos_of(&[row("/a", ""), row("", "mac")]).unwrap_err(),
            "Нужна папка репозитория 2"
        );
        assert_eq!(
            repos_of(&[row("/a", "m ac")]).unwrap_err(),
            "Имя машины без пробелов и @: m ac"
        );
        assert_eq!(
            repos_of(&[row("/a", "m@c")]).unwrap_err(),
            "Имя машины без пробелов и @: m@c"
        );
        assert!(
            repos_of(&[row("/a@b", "")]).is_err(),
            "an @ in a local path would read as a machine"
        );
        assert!(repos_of(&vec![row("/a", ""); 7]).is_err());
        assert_eq!(
            repos_of(&[row("~/x", "mac")]).unwrap()[0].path,
            "~/x",
            "a remote path is taken as typed"
        );
    }

    #[test]
    fn arguments_of_new() {
        let repos = [
            Repo {
                path: "/a".into(),
                machine: String::new(),
            },
            Repo {
                path: "/b".into(),
                machine: "mac".into(),
            },
        ];
        assert_eq!(
            new_args("Мой проект", &repos, "цель", "claude-sonnet", "claude"),
            [
                "new",
                "--repo=/a",
                "--repo=/b@mac",
                "--goal=цель",
                "--thread-profile=claude-sonnet",
                "--coordinator-profile=claude",
                "--",
                "Мой проект"
            ]
        );
        assert_eq!(
            new_args("-x", &repos[..1], "", "", ""),
            ["new", "--repo=/a", "--", "-x"]
        );
    }
}
