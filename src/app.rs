//! Shared server state: the latest snapshot of projects, the saved state,
//! the theme, and the event bus the page listens to.

use crate::config::AppConfig;
use crate::model::{Groups, Project};
use crate::sources::repos::TimeCache;
use crate::store::State;
use crate::suggest::GroupRun;
use crate::theme::ThemeReport;
use serde::Serialize;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Notify};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often the board is re-read.
pub const REFRESH_SECS: u64 = 20;
/// How often the focused herdr workspace is asked for.
pub const WORKSPACE_SECS: u64 = 3;
/// How often the theme files are checked for changes.
pub const THEME_SECS: u64 = 2;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The projects as last read from the board, with its error when the read failed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Snapshot {
    pub projects: Vec<Project>,
    pub updated: u64,
    pub loading: bool,
    pub error: Option<String>,
}

/// The active herdr workspace: its id, its label and the project it names.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Workspace {
    pub id: Option<String>,
    pub label: Option<String>,
    pub slug: Option<String>,
    /// Counts up on every change of label, so a page can tell a new switch from a refresh.
    pub seq: u64,
}

/// A project the bar popup asked the page to show.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Show {
    pub slug: String,
    /// When it was asked for, unix milliseconds: a page shows each request once.
    pub at: u64,
}

/// How long a request to show a project waits for a page that is still opening.
pub const SHOW_SECS: u64 = 20;

/// What `GET /api/snapshot` returns and what the `snapshot` event carries.
#[derive(Debug, Clone, Serialize)]
pub struct Full {
    pub version: &'static str,
    /// The board's data folder.
    pub data: String,
    pub projects: Vec<Project>,
    pub updated: u64,
    pub now: u64,
    pub loading: bool,
    pub error: Option<String>,
    pub groups: Groups,
    pub settings: crate::store::Settings,
    /// What the settings screen is built from.
    pub setting_defs: &'static [crate::store::SettingDef],
    /// The group suggestion: running, and why it failed.
    pub group_suggest: GroupSuggest,
    pub workspace: Workspace,
    pub imported_from: Option<String>,
    /// The project the bar popup asked for in the last seconds.
    pub show: Option<Show>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GroupSuggest {
    pub busy: bool,
    pub error: Option<String>,
}

/// One event on the bus, serialized for SSE.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: &'static str,
    pub data: String,
}

pub struct App {
    pub config: AppConfig,
    pub snapshot: RwLock<Snapshot>,
    pub state: Mutex<State>,
    pub theme: RwLock<ThemeReport>,
    pub workspace: RwLock<Workspace>,
    pub events: broadcast::Sender<Event>,
    /// Woken by `POST /api/refresh`.
    pub refresh_now: Notify,
    /// The group suggestion of the cheap agent.
    pub group_run: Mutex<GroupRun>,
    /// When the board's tasks came to their stage (read from the task records).
    pub times: Mutex<TimeCache>,
    /// The herdr space of each repo's folder, from the board's data folder: (folder, workspace id).
    pub spaces: RwLock<Vec<(String, String)>>,
    /// The last request to show a project (see `Show`).
    pub show: Mutex<Option<Show>>,
}

pub type Shared = Arc<App>;

impl App {
    pub fn new(config: AppConfig, state: State, theme: ThemeReport) -> Shared {
        let (events, _) = broadcast::channel(64);
        Arc::new(App {
            config,
            snapshot: RwLock::new(Snapshot {
                loading: true,
                ..Default::default()
            }),
            state: Mutex::new(state),
            theme: RwLock::new(theme),
            workspace: RwLock::new(Workspace::default()),
            events,
            refresh_now: Notify::new(),
            group_run: Mutex::new(GroupRun::default()),
            times: Mutex::new(TimeCache::default()),
            spaces: RwLock::new(Vec::new()),
            show: Mutex::new(None),
        })
    }

    pub fn full(&self) -> Full {
        let s = self.snapshot.read().unwrap().clone();
        let st = self.state.lock().unwrap();
        Full {
            version: VERSION,
            data: self.config.data_dir.display().to_string(),
            projects: s.projects,
            updated: s.updated,
            now: now_ms(),
            loading: s.loading,
            error: s.error,
            groups: st.groups.clone(),
            settings: st.settings.clone(),
            setting_defs: &crate::store::SETTING_DEFS,
            group_suggest: {
                let run = self.group_run.lock().unwrap();
                GroupSuggest {
                    busy: run.busy,
                    error: run.error.clone(),
                }
            },
            workspace: self.workspace.read().unwrap().clone(),
            imported_from: st.imported_from.clone(),
            show: self
                .show
                .lock()
                .unwrap()
                .clone()
                .filter(|s| now_ms().saturating_sub(s.at) < SHOW_SECS * 1000),
        }
    }

    /// Runs a command of the board (`herdr-projects ARGS`) in its data folder, when one was named.
    pub async fn board(
        &self,
        args: &[&str],
        timeout: std::time::Duration,
    ) -> Result<crate::sources::runner::Output, String> {
        let mut parts = vec![self.config.herdr_projects.as_str()];
        parts.extend_from_slice(args);
        let argv = crate::sources::runner::argv(parts);
        let env: Vec<(String, String)> = if self.config.data_named {
            vec![(
                "HERDR_PROJECTS_DATA_DIR".into(),
                self.config.data_dir.display().to_string(),
            )]
        } else {
            Vec::new()
        };
        crate::sources::runner::run_env(&argv, None, timeout, None, &env).await
    }

    pub fn emit<T: Serialize>(&self, kind: &'static str, data: &T) {
        let data = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
        let _ = self.events.send(Event { kind, data });
    }

    pub fn emit_snapshot(&self) {
        let full = self.full();
        self.emit("snapshot", &full);
    }

    /// Saves the state file; a failure is logged, the in-memory state stays.
    pub fn save_state(&self) {
        let st = self.state.lock().unwrap();
        if let Err(e) = st.save(&self.config.state_file) {
            eprintln!(
                "herdr-planner: не сохранилось состояние {}: {}",
                self.config.state_file.display(),
                e
            );
        }
    }
}
