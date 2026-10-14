//! Background work: the project list every 20 s (or on demand), the focused
//! herdr workspace every 3 s, the theme files every 2 s. Each loop publishes
//! on the event bus; nothing here blocks a request.

use crate::app::{now_ms, Shared, Snapshot, Workspace, REFRESH_SECS, THEME_SECS, WORKSPACE_SECS};
use crate::model::{project_for_label, Project};
use crate::sources::{list, project_md, runner, workspace};
use crate::theme;
use std::path::Path;
use std::time::Duration;

/// Items in `inbox/` other than `done`.
pub fn inbox_count(dir: &Path) -> u32 {
    std::fs::read_dir(dir.join("inbox"))
        .map(|rd| rd.flatten().filter(|e| e.file_name() != "done").count() as u32)
        .unwrap_or(0)
}

/// The highest thread number a project holds: its recency signal.
pub fn newest_thread(dir: &Path) -> u32 {
    std::fs::read_dir(dir.join("threads"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let digits: String = name
                        .strip_prefix("t-")?
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    digits.parse::<u32>().ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// Fills a project from its folder: PROJECT.md, inbox, threads.
pub fn enrich(p: &mut Project, dir: &Path) {
    if let Ok(text) = std::fs::read_to_string(dir.join("PROJECT.md")) {
        let md = project_md::parse(&text);
        if let Some(n) = md.name {
            p.name = n;
        }
        p.goal = md.goal;
        p.repos = md.repos;
    }
    p.inbox = inbox_count(dir);
    p.activity = newest_thread(dir);
}

pub async fn read_projects(app: &Shared) -> Result<Vec<Project>, String> {
    let argv = runner::argv([
        app.config.herdr_projects.as_str(),
        "--root",
        &app.config.root.display().to_string(),
        "list",
        "--all",
    ]);
    let out = runner::run(&argv, None, Duration::from_secs(20)).await?;
    if !out.ok() {
        return Err(out.error());
    }
    let mut projects = list::parse_list(&out.stdout);
    let root = app.config.root.clone();
    // Folder reads are quick but synchronous: off the runtime's workers.
    projects = tokio::task::spawn_blocking(move || {
        for p in &mut projects {
            enrich(p, &root.join(&p.slug));
        }
        projects
    })
    .await
    .map_err(|e| e.to_string())?;
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

/// Re-matches the known label against the (new) project list.
fn sync_workspace(app: &Shared) {
    let projects = app.snapshot.read().unwrap().projects.clone();
    let mut ws = app.workspace.write().unwrap();
    let slug = ws
        .label
        .as_deref()
        .and_then(|l| project_for_label(&projects, l));
    ws.slug = slug;
}

pub async fn workspace_loop(app: Shared) {
    let argv = runner::argv([app.config.herdr.as_str(), "workspace", "list"]);
    loop {
        if let Ok(out) = runner::run(&argv, None, Duration::from_secs(5)).await {
            if out.ok() {
                let label = workspace::focused_label(&out.stdout);
                let changed = {
                    let projects = app.snapshot.read().unwrap().projects.clone();
                    let mut ws = app.workspace.write().unwrap();
                    let same = ws.label.as_deref().map(str::to_lowercase)
                        == label.as_deref().map(str::to_lowercase);
                    if same {
                        false
                    } else {
                        let slug = label
                            .as_deref()
                            .and_then(|l| project_for_label(&projects, l));
                        *ws = Workspace {
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

    #[test]
    fn reads_a_project_folder() {
        let dir = std::env::temp_dir().join(format!("herdr-planner-proj-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("inbox/done")).unwrap();
        std::fs::create_dir_all(dir.join("threads")).unwrap();
        std::fs::write(dir.join("inbox/one.md"), "").unwrap();
        std::fs::write(dir.join("threads/t-0003.md"), "").unwrap();
        std::fs::write(dir.join("threads/t-0012.md"), "").unwrap();
        std::fs::write(dir.join("threads/notes"), "").unwrap();
        std::fs::write(
            dir.join("PROJECT.md"),
            "+++\nname = \"Имя\"\ngoal = \"Цель\"\n[[repos]]\npath = \"/r\"\n+++\n",
        )
        .unwrap();
        let mut p = Project {
            slug: "x".into(),
            name: "x".into(),
            ..Default::default()
        };
        enrich(&mut p, &dir);
        assert_eq!(p.name, "Имя");
        assert_eq!(p.goal.as_deref(), Some("Цель"));
        assert_eq!(p.inbox, 1);
        assert_eq!(p.activity, 12);
        assert_eq!(p.repos.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
