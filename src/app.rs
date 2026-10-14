//! Shared server state: the latest snapshot of projects, the saved state,
//! the theme, and the event bus the page listens to.

use crate::cards::Cache;
use crate::config::AppConfig;
use crate::coordask::AskCache;
use crate::fresh::FreshRun;
use crate::model::{Groups, Project};
use crate::mrstate::MrCache;
use crate::store::State;
use crate::suggest::GroupRun;
use crate::theme::ThemeReport;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Notify, Semaphore};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often the project list is re-read (as in the plugin).
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

/// The project list as last read, with its error when the read failed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Snapshot {
    pub projects: Vec<Project>,
    pub updated: u64,
    pub loading: bool,
    pub error: Option<String>,
}

/// The active herdr workspace: its label and the project it names.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Workspace {
    pub label: Option<String>,
    pub slug: Option<String>,
    /// Counts up on every change of label, so a page can tell a new switch from a refresh.
    pub seq: u64,
}

/// What `GET /api/snapshot` returns and what the `snapshot` event carries.
#[derive(Debug, Clone, Serialize)]
pub struct Full {
    pub version: &'static str,
    pub root: String,
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
    /// What the project card reads in the background (git, profiles, trailers).
    pub cache: Mutex<Cache>,
    /// Projects whose TASKS.md was read since the start (see `State::settle`).
    pub tasks_read: Mutex<HashSet<String>>,
    /// The backlog freshness check of each project the card was opened on.
    pub fresh_run: Mutex<HashMap<String, FreshRun>>,
    /// The group suggestion of the cheap agent.
    pub group_run: Mutex<GroupRun>,
    /// What each waiting coordinator asks, by project (see `coordask`).
    pub asks: Mutex<AskCache>,
    /// The state of the merge requests in finished threads' reports (see `mrstate`).
    pub mrs: Mutex<MrCache>,
    /// At most this many `git` scripts run at once …
    pub git_gate: Semaphore,
    /// … and at most three of them read finished threads' repositories.
    pub trailer_gate: Semaphore,
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
            cache: Mutex::new(Cache::default()),
            tasks_read: Mutex::new(HashSet::new()),
            fresh_run: Mutex::new(HashMap::new()),
            group_run: Mutex::new(GroupRun::default()),
            asks: Mutex::new(AskCache::default()),
            mrs: Mutex::new(MrCache::default()),
            git_gate: Semaphore::new(6),
            trailer_gate: Semaphore::new(3),
        })
    }

    pub fn full(&self) -> Full {
        let s = self.snapshot.read().unwrap().clone();
        let st = self.state.lock().unwrap();
        Full {
            version: VERSION,
            root: self.config.root.display().to_string(),
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
        }
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
