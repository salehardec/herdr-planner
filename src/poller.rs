//! Background work: the board's tasks every 20 s (or on demand), the focused
//! herdr workspace every 3 s, the theme files every 2 s. Each loop publishes
//! on the event bus; nothing here blocks a request.

use crate::app::{now_ms, Shared, Snapshot, Workspace, REFRESH_SECS, THEME_SECS, WORKSPACE_SECS};
use crate::board::{self, Task};
use crate::model::{project_for_workspace, Project};
use crate::sources::{repos, runner, workspace};
use crate::theme;
use std::collections::BTreeMap;
use std::time::Duration;

/// The projects of the board: its repos, each with the tasks `list` shows for it.
/// Tasks come from `herdr-projects list --json`; their tags (`PLN-1`) from the
/// plain `list` that prints the same tasks in the same order.
pub fn assemble(
    mut tasks: Vec<Task>,
    repos: &BTreeMap<String, String>,
    keys: &BTreeMap<String, String>,
) -> Vec<Project> {
    let mut by_repo: BTreeMap<String, Vec<Task>> = BTreeMap::new();
    let mut tag_keys: BTreeMap<String, String> = BTreeMap::new();
    for t in tasks.drain(..) {
        if let Some((key, _)) = t.tag.rsplit_once('-') {
            tag_keys.insert(t.repo.clone(), key.to_string());
        }
        by_repo.entry(t.repo.clone()).or_default().push(t);
    }
    let mut slugs: Vec<String> = repos.keys().chain(by_repo.keys()).cloned().collect();
    slugs.sort();
    slugs.dedup();
    slugs
        .into_iter()
        .map(|slug| {
            let key = tag_keys
                .get(&slug)
                .or_else(|| keys.get(&slug))
                .map(String::as_str)
                .unwrap_or("");
            let path = repos.get(&slug).map(String::as_str).unwrap_or("");
            let tasks = by_repo.remove(&slug).unwrap_or_default();
            Project::build(&slug, key, path, tasks)
        })
        .collect()
}

pub async fn read_projects(app: &Shared) -> Result<Vec<Project>, String> {
    let timeout = Duration::from_secs(20);
    let (json, plain) = tokio::join!(
        app.board(&["list", "--json"], timeout),
        app.board(&["list"], timeout)
    );
    let json = json?;
    if !json.ok() {
        return Err(json.error());
    }
    let mut tasks = board::parse_list(&json.stdout)?;
    if let Ok(plain) = plain {
        if plain.ok() {
            board::apply_tags(&mut tasks, &board::plain_tags(&plain.stdout));
        }
    }
    let dir = app.config.data_dir.clone();
    let app2 = app.clone();
    // File reads are quick but synchronous: off the runtime's workers.
    let (projects, spaces) = tokio::task::spawn_blocking(move || {
        let repos = repos::read_repos(&dir);
        let keys = repos::read_keys(&dir);
        let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let times = app2.times.lock().unwrap().times(&dir, &refs);
        for t in &mut tasks {
            t.at = times.get(&t.id).copied().unwrap_or(0);
        }
        (assemble(tasks, &repos, &keys), repos::read_spaces(&dir))
    })
    .await
    .map_err(|e| e.to_string())?;
    *app.spaces.write().unwrap() = spaces;
    Ok(projects)
}

pub async fn refresh_once(app: &Shared) {
    {
        let mut s = app.snapshot.write().unwrap();
        s.loading = true;
    }
    let result = read_projects(app).await;
    {
        let mut s = app.snapshot.write().unwrap();
        s.loading = false;
        match result {
            Ok(projects) => {
                *s = Snapshot {
                    projects,
                    updated: now_ms(),
                    loading: false,
                    error: None,
                };
            }
            Err(e) => s.error = Some(e),
        }
    }
    sync_workspace(app);
    app.emit_snapshot();
    // Projects that appeared since the last read get their group guess.
    crate::suggest::tick(app);
}

pub async fn refresh_loop(app: Shared) {
    loop {
        refresh_once(&app).await;
        let _ = tokio::time::timeout(
            Duration::from_secs(REFRESH_SECS),
            app.refresh_now.notified(),
        )
        .await;
    }
}

/// The project a workspace (id and label) names, among the current projects.
fn workspace_slug(app: &Shared, id: Option<&str>, label: Option<&str>) -> Option<String> {
    let label = label?;
    let projects = app.snapshot.read().unwrap().projects.clone();
    let spaces = app.spaces.read().unwrap().clone();
    project_for_workspace(&projects, &spaces, id.unwrap_or(""), label)
}

/// Re-matches the known workspace against the (new) project list.
fn sync_workspace(app: &Shared) {
    let (id, label) = {
        let ws = app.workspace.read().unwrap();
        (ws.id.clone(), ws.label.clone())
    };
    let slug = workspace_slug(app, id.as_deref(), label.as_deref());
    app.workspace.write().unwrap().slug = slug;
}

pub async fn workspace_loop(app: Shared) {
    let argv = runner::argv([app.config.herdr.as_str(), "workspace", "list"]);
    loop {
        if let Ok(out) = runner::run(&argv, None, Duration::from_secs(5)).await {
            if out.ok() {
                let (id, label) = match workspace::focused(&out.stdout) {
                    Some((id, label)) => (Some(id), Some(label)),
                    None => (None, None),
                };
                let changed = {
                    let slug = workspace_slug(&app, id.as_deref(), label.as_deref());
                    let mut ws = app.workspace.write().unwrap();
                    let same = ws.label.as_deref().map(str::to_lowercase)
                        == label.as_deref().map(str::to_lowercase)
                        && ws.id == id;
                    if same {
                        false
                    } else {
                        *ws = Workspace {
                            id,
                            label,
                            slug,
                            seq: ws.seq + 1,
                        };
                        true
                    }
                };
                if changed {
                    let ws = app.workspace.read().unwrap().clone();
                    app.emit("workspace", &ws);
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(WORKSPACE_SECS)).await;
    }
}

pub async fn theme_loop(app: Shared) {
    let files = theme::watched_files();
    let stamp = || {
        files
            .iter()
            .map(|f| crate::config::file_mtime(f))
            .collect::<Vec<_>>()
    };
    let mut last = stamp();
    loop {
        tokio::time::sleep(Duration::from_secs(THEME_SECS)).await;
        let now = stamp();
        if now != last {
            last = now;
            let report = tokio::task::spawn_blocking(theme::load)
                .await
                .unwrap_or_else(|_| theme::load());
            let changed = {
                let mut t = app.theme.write().unwrap();
                let changed = *t != report;
                *t = report;
                changed
            };
            if changed {
                let t = app.theme.read().unwrap().clone();
                app.emit("theme", &t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::parse_list;

    #[test]
    fn projects_are_the_repos_of_the_board_with_their_tasks() {
        let mut tasks = parse_list(
            r#"[{"id":"alpha/1","state":"done"},{"id":"alpha/2","state":"blocked","kind":"question"},{"id":"remote/7","state":"idle"}]"#,
        )
        .unwrap();
        board::apply_tags(&mut tasks, &["ALP-1".into(), "ALP-2".into(), "#7".into()]);
        let repos = BTreeMap::from([
            ("alpha".to_string(), "/p/alpha".to_string()),
            ("beta".to_string(), "/p/beta".to_string()),
        ]);
        let keys = BTreeMap::from([
            ("beta".to_string(), "BET".to_string()),
            ("alpha".to_string(), "OLD".to_string()),
        ]);
        let v = assemble(tasks, &repos, &keys);
        let names: Vec<&str> = v.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(names, ["alpha", "beta", "remote"], "a repo with tasks that the board does not list is still shown");
        assert_eq!(v[0].key, "ALP", "the key the board prints wins over the record of old keys");
        assert_eq!((v[0].path.as_str(), v[0].counts.done, v[0].counts.needs), ("/p/alpha", 1, 1));
        assert_eq!((v[1].key.as_str(), v[1].tasks.len()), ("BET", 0));
        assert_eq!((v[2].key.as_str(), v[2].path.as_str(), v[2].counts.todo), ("", "", 1));
    }
}
