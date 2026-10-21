//! The planner's one process: the board snapshot the page reads, the journal
//! of what changed, the Coordinator's standing line and the details cache.

use crate::board::{self, Details, Link, Project, ProjectLag, Status, Summary, Task};
use crate::config::{now_ms, AppConfig};
use crate::coordinator::{self, CoordinatorView};
use crate::db::{Db, SearchDoc};
use crate::events::Bus;
use crate::theme::{self, Theme};
use crate::usage::{self, Usage};
use crate::{gitlag, history, machines, subagents, taskmeta};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::task::JoinSet;
use tokio::time::timeout;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const JOURNAL_MAX: usize = 80;
/// How many sent Coordinator messages the page keeps with their images.
pub const CHAT_HISTORY_MAX: usize = 20;

#[derive(Debug, Clone, Serialize)]
pub struct BoardStatus {
    pub ok: bool,
    pub error: String,
    /// When the shown data was last read, if the board stopped answering.
    pub stale_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HerdrInfo {
    pub running: bool,
    pub socket: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthInfo {
    pub required: bool,
    pub user: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub listen: String,
    pub port: u16,
    pub local_only: bool,
    pub tls: bool,
    /// Where a repository added by address is cloned.
    pub projects_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub n: u64,
    pub t: u64,
    pub id: String,
    pub s: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoordinatorState {
    #[serde(flatten)]
    pub view: CoordinatorView,
    /// The tail shown is the board's last report, not a structured chat reply.
    pub source: String,
    /// Sent messages, newest last, with the images that went with them.
    pub history: Vec<ChatMessage>,
    /// Sub-agents of the Coordinator: the Quasar's moons.
    pub subagents: Vec<subagents::Subagent>,
    pub subagents_note: String,
    /// How full its context is.
    pub context: ContextOut,
    /// `/compact` can be sent: a Claude Coordinator that is not busy.
    pub can_compact: bool,
    /// Its program: `claude`, `omp`, …
    pub program: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ContextOut {
    pub used: u64,
    pub window: u64,
    pub percent: f64,
    pub known: bool,
    pub model: String,
}

/// A far galaxy as the snapshot carries it: the machine's board plus the lag of its clones.
#[derive(Debug, Clone, Serialize)]
pub struct GalaxyOut {
    #[serde(flatten)]
    pub g: machines::Galaxy,
    pub lag: BTreeMap<String, ProjectLag>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgeOut {
    pub origin: String,
    pub a: gitlag::RepoRef,
    pub b: gitlag::RepoRef,
    /// «main −2» when one of the two clones is behind its origin; empty when both are level.
    pub lag: String,
}

/// One item of the God's own list: an idea that is not on the board yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub at: u64,
    /// Whom the item is addressed to: empty, `quasar` or a task id.
    #[serde(default)]
    pub to: String,
    /// Files attached to the item (they live in the attachments folder and travel with it).
    #[serde(default)]
    pub attachments: Vec<crate::attachments::Attachment>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub version: String,
    pub at: u64,
    pub seq: u64,
    pub board: BoardStatus,
    /// This machine's name (the galaxy's label).
    pub host: String,
    pub tasks: Vec<Task>,
    pub archived: Vec<Task>,
    pub projects: Vec<Project>,
    pub summary: Summary,
    /// `after` dependencies, read from the board and cached between refreshes.
    pub links: Vec<Link>,
    /// Subscription limits and balances from Omarchy's agent usage records.
    pub limits: Usage,
    /// The God's own list: ideas to drop into a universe as a new task.
    pub todo: Vec<TodoItem>,
    pub journal: Vec<JournalEntry>,
    pub coordinator: CoordinatorState,
    /// Other machines of the board: far galaxies.
    pub galaxies: Vec<serde_json::Value>,
    /// The same project in two galaxies.
    pub bridges: Vec<BridgeOut>,
    pub theme: Theme,
    pub herdr: HerdrInfo,
    pub auth: AuthInfo,
    pub server: ServerInfo,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatState {
    pub last_sent: String,
    pub sent_at: u64,
    pub last_reply: String,
    /// Newest last; capped at [`CHAT_HISTORY_MAX`].
    #[serde(default)]
    pub history: Vec<ChatMessage>,
    #[serde(skip)]
    pub busy_until: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub text: String,
    pub at: u64,
    #[serde(default)]
    pub attachments: Vec<crate::attachments::Attachment>,
}

#[derive(Debug, Default)]
pub struct Journal {
    seq: u64,
    entries: VecDeque<JournalEntry>,
    prev: HashMap<String, (Status, String)>,
    primed: bool,
}


#[derive(Debug, Default)]
pub struct AfterCache {
    /// The snapshot sequence the cached map belongs to.
    pub at_seq: u64,
    /// Task id → tags it waits for, from `herdr-projects after <id>`.
    pub map: BTreeMap<String, Vec<String>>,
    /// Task id → the waits its record listed when `map` was read (to see a change).
    pub keys: HashMap<String, String>,
}

pub struct App {
    pub cfg: AppConfig,
    pub db: Db,
    pub bus: Bus,
    /// Something changed on disk: read the board again soon.
    pub poke: Notify,
    pub snapshot: RwLock<Snapshot>,
    pub auth: crate::auth::Auth,
    pub chat: Mutex<ChatState>,
    pub journal: Mutex<Journal>,
    pub details: Mutex<HashMap<String, (u64, Details)>>,
    pub refresh_lock: Mutex<()>,
    pub action_lock: Mutex<()>,
    pub after: Mutex<AfterCache>,
    /// `after`/`delegate` changed a wait: read the waits at the next refresh.
    pub force_after: AtomicBool,
    /// The other machines of the board, as the last poll left them.
    pub galaxies: Mutex<Vec<machines::Galaxy>>,
    /// Their last stored reads, shown until the first poll answers.
    pub cached: Mutex<Vec<serde_json::Value>>,
    /// Main-branch lag of this machine's clones, by repo name.
    pub lag_local: Mutex<BTreeMap<String, gitlag::Lag>>,
    /// The same for the other machines' clones: galaxy label → repo → lag.
    pub lag_remote: Mutex<BTreeMap<String, BTreeMap<String, gitlag::Lag>>>,
    /// Labels of the galaxies a connection is being made to.
    pub connecting: Mutex<Vec<String>>,
}

pub type Shared = Arc<App>;

/// The name of this machine as the page shows it: `HERDR_PLANNER_HOST` when set (the checks pin it), else the hostname.
pub fn hostname() -> String {
    if let Some(h) = std::env::var("HERDR_PLANNER_HOST").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return h;
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .or_else(|| std::env::var("HOST").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "localhost".to_string())
}

impl App {
    pub fn new(cfg: AppConfig) -> Shared {
        let theme = theme::load();
        let usage = usage::read();
        let _ = std::fs::create_dir_all(&cfg.state_dir);
        let db = Db::open(&cfg.state_dir.join("planner.sqlite")).unwrap_or_else(|e| {
            eprintln!("herdr-planner: database: {e}; working in memory");
            Db::memory().expect("in-memory database")
        });
        match db.import_legacy(&cfg.state_dir) {
            Ok(done) if !done.is_empty() => {
                eprintln!("herdr-planner: imported {} into the database", done.join(", "))
            }
            Ok(_) => {}
            Err(e) => eprintln!("herdr-planner: import of the old state files: {e}"),
        }
        let snapshot = Snapshot {
            version: VERSION.to_string(),
            at: 0,
            seq: 0,
            board: BoardStatus {
                ok: false,
                error: String::new(),
                stale_at: 0,
            },
            host: hostname(),
            tasks: Vec::new(),
            archived: Vec::new(),
            projects: Vec::new(),
            summary: Summary::default(),
            links: Vec::new(),
            limits: usage,
            todo: db.todo_list(),
            journal: Vec::new(),
            coordinator: CoordinatorState {
                view: CoordinatorView::default(),
                source: "herdr-projects coordinator".into(),
                history: Vec::new(),
                subagents: Vec::new(),
                subagents_note: String::new(),
                context: ContextOut::default(),
                can_compact: false,
                program: String::new(),
            },
            galaxies: Vec::new(),
            bridges: Vec::new(),
            theme,
            herdr: HerdrInfo {
                running: cfg.herdr_socket.exists(),
                socket: cfg.herdr_socket.display().to_string(),
            },
            auth: AuthInfo {
                required: cfg.auth_required(),
                user: cfg.user.clone(),
            },
            server: ServerInfo {
                listen: cfg.listen.clone(),
                port: cfg.port,
                local_only: cfg.is_loopback(),
                tls: cfg.tls().is_some(),
                projects_dir: cfg.projects_dir_now().display().to_string(),
            },
        };
        let held = held_files(&db.todo_list());
        crate::attachments::cleanup(&cfg.attachments_dir(), std::time::SystemTime::now(), &held);
        let mut chat = db.chat_load();
        if prune_missing_attachments(&cfg.attachments_dir(), &mut chat) {
            db.chat_save(&chat);
        }
        let journal = load_journal(&db);
        // The last good reads of the other machines survive a restart; they are not live until a poll says so.
        let cached: Vec<serde_json::Value> = db
            .galaxy_all()
            .into_iter()
            .filter_map(|(_, json, _, _, _)| serde_json::from_str::<serde_json::Value>(&json).ok())
            .map(|mut v| {
                v["ok"] = serde_json::Value::Bool(false);
                v
            })
            .collect();
        let auth = crate::auth::Auth::new(
            cfg.auth_required(),
            cfg.user.clone(),
            cfg.password_hash.clone(),
        );
        Arc::new(Self {
            cfg,
            db,
            bus: Bus::new(),
            poke: Notify::new(),
            snapshot: RwLock::new(snapshot),
            auth,
            chat: Mutex::new(chat),
            journal: Mutex::new(journal),
            details: Mutex::new(HashMap::new()),
            refresh_lock: Mutex::new(()),
            action_lock: Mutex::new(()),
            after: Mutex::new(AfterCache::default()),
            force_after: AtomicBool::new(false),
            galaxies: Mutex::new(Vec::new()),
            cached: Mutex::new(cached),
            lag_local: Mutex::new(BTreeMap::new()),
            lag_remote: Mutex::new(BTreeMap::new()),
            connecting: Mutex::new(Vec::new()),
        })
    }
}

pub fn save_chat(app: &App, chat: &ChatState) {
    app.db.chat_save(chat);
}

/// Drops history images whose files are gone: cleanup by age (or a manual
/// removal) must not leave broken thumbnails in the chat.
/// The ids of the files todo items still hold: the cleanup leaves them alone.
pub fn held_files(todo: &[TodoItem]) -> std::collections::HashSet<String> {
    todo.iter()
        .flat_map(|t| t.attachments.iter().map(|a| a.id.clone()))
        .collect()
}

fn prune_missing_attachments(dir: &std::path::Path, chat: &mut ChatState) -> bool {
    let mut changed = false;
    for message in &mut chat.history {
        let before = message.attachments.len();
        message.attachments.retain(|a| {
            crate::attachments::path(dir, &a.id).is_some_and(|p| p.exists())
        });
        changed |= message.attachments.len() != before;
    }
    changed
}

/// Runs after an upload's cleanup: old images may have just disappeared.
pub async fn prune_chat_attachments(app: &Shared) {
    let mut chat = app.chat.lock().await;
    if prune_missing_attachments(&app.cfg.attachments_dir(), &mut chat) {
        let saved = chat.clone();
        drop(chat);
        save_chat(app, &saved);
    }
}

fn load_journal(db: &Db) -> Journal {
    let (seq, entries) = db.journal_load();
    Journal {
        seq,
        entries: entries.into_iter().collect(),
        prev: HashMap::new(),
        primed: false,
    }
}

/// Runs a board command. The data folder is passed only when it was named, so a
/// default setup keeps using whatever `herdr-projects` would use by itself.
pub async fn run_board(cfg: &AppConfig, args: &[&str], wait: Duration) -> Result<String, String> {
    let mut cmd = Command::new(&cfg.herdr_projects);
    cmd.args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if cfg.data_named {
        cmd.env("HERDR_PROJECTS_DATA_DIR", &cfg.data_dir);
    }
    let out = match timeout(wait, cmd.output()).await {
        Err(_) => {
            return Err(format!(
                "{} {}: no answer in {}s",
                cfg.herdr_projects,
                args.join(" "),
                wait.as_secs()
            ))
        }
        Ok(Err(e)) => {
            return Err(format!("{}: {e}", cfg.herdr_projects));
        }
        Ok(Ok(o)) => o,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        let why = if stderr.is_empty() {
            stdout.trim().to_string()
        } else {
            stderr
        };
        return Err(if why.is_empty() {
            format!("herdr-projects {}: exit {}", args.join(" "), out.status)
        } else {
            why
        });
    }
    Ok(stdout)
}

/// Merges the task's own record (`task.json`) and its repo's key into the row of `list --json`.
fn apply_meta(t: &mut Task, meta: Option<&taskmeta::TaskMeta>, key: &str) {
    t.key = key.to_string();
    if let Some(m) = meta {
        t.created = m.created_ms;
        t.description = m.description.clone();
        t.worker_agent = m.worker_agent.clone();
        t.model = m.model.clone();
        t.session = m.session.clone();
        t.worktree = m.worktree.clone();
        t.exited_at = m.exited_at_ms;
        if t.app.is_empty() {
            t.app = m.app.clone();
        }
    }
}

/// Sub-agents of every active task, from the agents' own session files (blocking IO).
fn collect_subagents(tasks: &mut [Task], roots: &HashMap<String, String>, now: u64) {
    for t in tasks.iter_mut() {
        if t.status == Status::Done || t.archived || t.session.is_empty() && t.app != "pi" {
            continue;
        }
        let cwd = if t.worktree.is_empty() {
            roots.get(&t.repo).cloned().unwrap_or_default()
        } else {
            t.worktree.clone()
        };
        let agent_name = format!("sw-{}-{}", t.repo, t.n);
        let q = subagents::Query {
            app: &t.app,
            session: &t.session,
            cwd: &cwd,
            agent_name: &agent_name,
            parent_active: matches!(
                t.status,
                Status::Working | Status::Review | Status::Plan | Status::Waiting
            ) && t.exited_at == 0,
            parent_exited_at_ms: t.exited_at,
        };
        let found = subagents::for_task(&q, now);
        t.subagents = found.list;
        t.subagents_note = found.note;
    }
}

/// Projects of the visible tasks plus the board's other repositories (with no live task, or none at all).
fn build_projects(
    active: &[Task],
    repos: &[taskmeta::RepoInfo],
    lag: &BTreeMap<String, ProjectLag>,
    data_dir: &std::path::Path,
) -> Vec<Project> {
    // The Coordinator's own folders and the home folder are on the board's repo list, but they are not projects.
    let home = crate::config::home();
    let own = |root: &str| {
        let p = std::path::Path::new(root);
        p == home || p.starts_with(data_dir)
    };
    let mut projects = board::projects(active);
    let known: std::collections::HashSet<String> = projects.iter().map(|p| p.name.clone()).collect();
    for r in repos {
        if known.contains(&r.name) || r.root.is_empty() || own(&r.root) || !std::path::Path::new(&r.root).is_dir() {
            continue;
        }
        projects.push(Project {
            name: r.name.clone(),
            key: if r.key.is_empty() {
                r.name
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .take(3)
                    .collect::<String>()
                    .to_ascii_uppercase()
            } else {
                r.key.clone()
            },
            quiet: true,
            empty: true,
            ..Project::default()
        });
    }
    for p in projects.iter_mut() {
        if let Some(r) = repos.iter().find(|r| r.name == p.name) {
            p.root = r.root.clone();
            if !r.key.is_empty() {
                p.key = r.key.clone();
            }
            p.folder = std::path::Path::new(&r.root)
                .parent()
                .map(|x| x.display().to_string())
                .unwrap_or_default();
        }
        if let Some(l) = lag.get(&p.name) {
            p.origin = l.origin.clone();
            p.lag = Some(l.clone());
        }
    }
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    projects
}

fn to_project_lag(l: &gitlag::Lag) -> ProjectLag {
    ProjectLag {
        origin: l.origin.clone(),
        main: l.main.clone(),
        behind: l.behind,
        ahead: l.ahead,
        known: l.known,
        as_of_ms: l.as_of_ms,
        note: l.note.clone(),
        vs: Vec::new(),
    }
}

/// The page's view of far galaxies, the lag of every clone and the bridges between the same projects.
/// No IO: reads what the background loops stored.
pub async fn rebuild_extras(app: &Shared) {
    let live = app.galaxies.lock().await.clone();
    let cached = app.cached.lock().await.clone();
    let connecting = app.connecting.lock().await.clone();
    let local = app.lag_local.lock().await.clone();
    let remote = app.lag_remote.lock().await.clone();
    let repos = taskmeta::repos(&app.cfg.data_dir);
    let roots: HashMap<String, String> = repos.iter().map(|r| (r.name.clone(), r.root.clone())).collect();

    // Comparison of this machine's clones with the others', by origin.
    let mut vs: BTreeMap<String, Vec<board::LagVs>> = BTreeMap::new();
    let mut bridges: Vec<BridgeOut> = Vec::new();
    let mut galaxy_repos: Vec<gitlag::GalaxyRepo> = Vec::new();
    for (repo, l) in &local {
        if !l.origin.is_empty() {
            galaxy_repos.push(gitlag::GalaxyRepo {
                galaxy: String::new(),
                repo: repo.clone(),
                origin: l.origin.clone(),
            });
        }
    }
    for g in &live {
        if let Some(map) = remote.get(&g.label) {
            for (repo, l) in map {
                if !l.origin.is_empty() {
                    galaxy_repos.push(gitlag::GalaxyRepo {
                        galaxy: g.label.clone(),
                        repo: repo.clone(),
                        origin: l.origin.clone(),
                    });
                }
            }
        }
    }
    for b in gitlag::bridges(&galaxy_repos) {
        let la = lag_of(&b.a, &local, &remote);
        let lb = lag_of(&b.b, &local, &remote);
        let mut text = String::new();
        if let (Some(la), Some(lb)) = (la, lb) {
            if la.behind > 0 {
                text = format!("main −{}", la.behind);
            } else if lb.behind > 0 {
                text = format!("{}: main −{}", if b.b.galaxy.is_empty() { &b.a.galaxy } else { &b.b.galaxy }, lb.behind);
            }
            // Local clone against the other machine's clone.
            let (mine, other, other_galaxy) = if b.a.galaxy.is_empty() {
                (Some((&b.a, la)), Some(lb), b.b.galaxy.clone())
            } else if b.b.galaxy.is_empty() {
                (Some((&b.b, lb)), Some(la), b.a.galaxy.clone())
            } else {
                (None, None, String::new())
            };
            if let (Some((me, mine_lag)), Some(other_lag)) = (mine, other) {
                let root = roots.get(&me.repo).map(String::as_str);
                let (state, behind, ahead) = match gitlag::compare(mine_lag, other_lag, root) {
                    gitlag::Cmp::Same => ("same", 0, 0),
                    gitlag::Cmp::Behind(n) => ("behind", n, 0),
                    gitlag::Cmp::Ahead(n) => ("ahead", 0, n),
                    gitlag::Cmp::Diverged => ("diverged", 0, 0),
                    gitlag::Cmp::Unknown => ("unknown", 0, 0),
                };
                vs.entry(me.repo.clone()).or_default().push(board::LagVs {
                    galaxy: other_galaxy,
                    behind,
                    ahead,
                    state: state.to_string(),
                });
            }
        }
        bridges.push(BridgeOut { origin: b.origin, a: b.a, b: b.b, lag: text });
    }

    let mut out: Vec<serde_json::Value> = Vec::new();
    for g in &live {
        let mut lag: BTreeMap<String, ProjectLag> = BTreeMap::new();
        if let Some(map) = remote.get(&g.label) {
            for (repo, l) in map {
                lag.insert(repo.clone(), to_project_lag(l));
            }
        }
        let mut g2 = g.clone();
        g2.connecting = connecting.contains(&g.label);
        let mut v = serde_json::to_value(GalaxyOut { g: g2, lag }).unwrap_or(serde_json::Value::Null);
        if let Some(obj) = v.as_object_mut() {
            // The machine's own words «bridge» come from the bridges list; nothing else to add here.
            obj.entry("lag").or_insert(serde_json::json!({}));
        }
        out.push(v);
    }
    for c in cached {
        let label = c.get("label").and_then(|x| x.as_str()).unwrap_or("");
        if !live.iter().any(|g| g.label == label) {
            out.push(c);
        }
    }

    let mut lag_out: BTreeMap<String, ProjectLag> = BTreeMap::new();
    for (repo, l) in &local {
        let mut pl = to_project_lag(l);
        pl.vs = vs.remove(repo).unwrap_or_default();
        lag_out.insert(repo.clone(), pl);
    }

    let mut snap = app.snapshot.write().await;
    snap.galaxies = out;
    snap.bridges = bridges;
    for p in snap.projects.iter_mut() {
        if let Some(l) = lag_out.get(&p.name) {
            p.origin = l.origin.clone();
            p.lag = Some(l.clone());
        }
    }
}

fn lag_of<'a>(
    r: &gitlag::RepoRef,
    local: &'a BTreeMap<String, gitlag::Lag>,
    remote: &'a BTreeMap<String, BTreeMap<String, gitlag::Lag>>,
) -> Option<&'a gitlag::Lag> {
    if r.galaxy.is_empty() {
        local.get(&r.repo)
    } else {
        remote.get(&r.galaxy).and_then(|m| m.get(&r.repo))
    }
}

/// Serialises the snapshot for the page's event stream, when somebody listens.
pub async fn publish(app: &Shared) {
    if app.bus.subscribers() == 0 {
        return;
    }
    let json = {
        let snap = app.snapshot.read().await;
        serde_json::to_string(&*snap)
    };
    if let Ok(json) = json {
        app.bus.send("state", json);
    }
}

/// Reads the board, the Coordinator and the theme; safe to call at any time.
/// Failed board reads keep the previous tasks and mark them stale.
pub async fn refresh(app: &Shared) {
    refresh_inner(app).await;
    rebuild_extras(app).await;
    publish(app).await;
}

async fn refresh_inner(app: &Shared) {
    let _guard = app.refresh_lock.lock().await;
    let cfg = &app.cfg;
    let (json, plain, coord) = tokio::join!(
        run_board(cfg, &["list", "--all", "--json"], Duration::from_secs(15)),
        run_board(cfg, &["list", "--all"], Duration::from_secs(15)),
        run_board(cfg, &["coordinator"], Duration::from_secs(8)),
    );

    let now = now_ms();
    let theme = theme::load();
    let usage = usage::read();
    let chat_before = app.chat.lock().await.clone();
    let mut chat = chat_before.clone();

    // The Coordinator's standing line and the last lines of its pane.
    let mut coord_view = match &coord {
        Ok(out) => coordinator::parse(out),
        Err(e) => {
            let mut v = app.snapshot.read().await.coordinator.view.clone();
            v.error = e.clone();
            v
        }
    };
    if !coord_view.reply.is_empty() && coord_view.reply != chat.last_reply {
        chat.last_reply = coord_view.reply.clone();
        chat.busy_until = 0;
    }
    let waiting = chat.busy_until > now;
    let busy = coordinator::is_busy(&coord_view.status) || waiting;
    if !busy {
        chat.busy_until = 0;
    }
    coord_view.busy = busy;
    coord_view.last_sent = chat.last_sent.clone();
    coord_view.sent_at = chat.sent_at;
    if chat != chat_before {
        save_chat(app, &chat);
    }
    let history_msgs = chat.history.clone();
    *app.chat.lock().await = chat;

    let seq = app.snapshot.read().await.seq + 1;
    let read = match &json {
        Ok(text) => board::parse_list(text),
        Err(e) => Err(e.clone()),
    };

    let repos = taskmeta::repos(&cfg.data_dir);
    let roots: HashMap<String, String> = repos.iter().map(|r| (r.name.clone(), r.root.clone())).collect();
    let keys: HashMap<String, String> = repos.iter().map(|r| (r.name.clone(), r.key.clone())).collect();

    // The waits are read outside the snapshot lock: each is a board process,
    // and a slow one must not keep the page from reading the snapshot.
    let (active, archived, links) = match read {
        Ok(mut tasks) => {
            if let Ok(plain) = &plain {
                let tags = board::plain_tags(plain);
                board::apply_tags(&mut tasks, &tags);
            }
            // The tasks' own records, and the sub-agents of the live ones (file reads).
            let data_dir = cfg.data_dir.clone();
            let roots2 = roots.clone();
            let keys2 = keys.clone();
            let tasks = tokio::task::spawn_blocking(move || {
                let mut tasks = tasks;
                let mut metas_local: HashMap<String, taskmeta::TaskMeta> = HashMap::new();
                for t in tasks.iter_mut() {
                    let meta = taskmeta::read_task(&data_dir, &t.repo, t.n);
                    let key = keys2.get(&t.repo).cloned().unwrap_or_default();
                    apply_meta(t, meta.as_ref(), &key);
                    if let Some(m) = meta {
                        metas_local.insert(t.id.clone(), m);
                    }
                }
                collect_subagents(&mut tasks, &roots2, now_ms());
                (tasks, metas_local)
            })
            .await;
            let (tasks, metas) = match tasks {
                Ok(pair) => pair,
                Err(e) => return stale_all(app, e.to_string(), now).await,
            };
            let (archived, active): (Vec<Task>, Vec<Task>) =
                tasks.into_iter().partition(|t| t.archived);
            let after = poll_after(app, seq, &active, &metas).await;
            let mut all = active.clone();
            all.extend(archived.iter().cloned());
            let links = board::links_from(&after, &all);
            (Ok(active), archived, links)
        }
        Err(e) => (Err(e), Vec::new(), Vec::new()),
    };

    // The Quasar's own moons and how full its memory is.
    let program = history::coordinator_program(cfg);
    let (coord_subs, coord_note) = {
        let cfg2 = cfg.clone();
        let busy2 = busy;
        tokio::task::spawn_blocking(move || {
            let f = subagents::for_coordinator(&cfg2, busy2, now_ms());
            (f.list, f.note)
        })
        .await
        .unwrap_or_default()
    };
    let ctx = {
        let cfg2 = cfg.clone();
        tokio::task::spawn_blocking(move || history::coordinator_context(&cfg2))
            .await
            .ok()
            .flatten()
    };
    let context = match ctx {
        Some(c) => {
            let window = history::context_window(c.used, &c.model);
            ContextOut {
                used: c.used,
                window,
                percent: (c.used as f64 / window as f64 * 100.0).min(100.0),
                known: true,
                model: c.model,
            }
        }
        None => ContextOut::default(),
    };

    let lag_now: BTreeMap<String, ProjectLag> = app
        .lag_local
        .lock()
        .await
        .iter()
        .map(|(k, v)| (k.clone(), to_project_lag(v)))
        .collect();

    let mut snap = app.snapshot.write().await;
    snap.at = now;
    snap.seq += 1;
    snap.theme = theme;
    snap.limits = usage;
    snap.herdr.running = cfg.herdr_socket.exists();
    snap.coordinator = CoordinatorState {
        can_compact: program == "claude" && !busy && !coord_view.dialog && coord_view.error.is_empty(),
        view: coord_view,
        source: "herdr-projects coordinator · последние строки окна Координатора".into(),
        history: history_msgs,
        subagents: coord_subs,
        subagents_note: coord_note,
        context,
        program,
    };

    match active {
        Ok(active) => {
            let projects = build_projects(&active, &repos, &lag_now, &cfg.data_dir);
            let summary = board::summary(&active, &archived, &projects);
            let mut journal = app.journal.lock().await;
            let before = journal.seq;
            update_journal(&mut journal, &active, now);
            let fresh: Vec<JournalEntry> = journal
                .entries
                .iter()
                .filter(|e| e.n > before)
                .cloned()
                .collect();
            if !fresh.is_empty() {
                app.db.journal_append(&fresh);
            }
            for t in &active {
                app.db.status_note(&t.id, t.status.key(), now);
            }
            let entries: Vec<JournalEntry> = journal.entries.iter().cloned().collect();
            snap.journal = entries;
            snap.tasks = active;
            snap.archived = archived;
            snap.projects = projects;
            snap.summary = summary;
            snap.links = links;
            snap.todo = app.db.todo_list();
            snap.board = BoardStatus {
                ok: true,
                error: String::new(),
                stale_at: 0,
            };
        }
        Err(e) => stale(&mut snap, e, now),
    }
}

async fn stale_all(app: &Shared, error: String, now: u64) {
    let mut snap = app.snapshot.write().await;
    stale(&mut snap, error, now);
}

/// Reads `herdr-projects after <id>` for the active, unfinished tasks whose waits may have changed.
/// A task whose own record lists no waits needs no process at all; one whose list is the same as
/// last time keeps its answer. Without a record (a board that keeps none) the old walk applies:
/// six processes at a time, every third refresh when there are many tasks, or right away after an
/// `after`/`delegate` action. A failed read keeps what was known for that task.
async fn poll_after(
    app: &Shared,
    seq: u64,
    tasks: &[Task],
    metas: &HashMap<String, taskmeta::TaskMeta>,
) -> BTreeMap<String, Vec<String>> {
    let ids: Vec<String> = tasks
        .iter()
        .filter(|t| !t.archived && t.status != Status::Done)
        .map(|t| t.id.clone())
        .collect();
    let mut cache = app.after.lock().await;
    let force = app.force_after.swap(false, Ordering::SeqCst);
    let every = if ids.len() > 24 { 3 } else { 1 };
    let due = force || cache.at_seq == 0 || seq.saturating_sub(cache.at_seq) >= every;
    let mut next = cache.map.clone();
    next.retain(|id, _| ids.iter().any(|x| x == id));
    let mut keys = cache.keys.clone();
    keys.retain(|id, _| ids.iter().any(|x| x == id));
    let mut todo: Vec<String> = Vec::new();
    for id in &ids {
        match metas.get(id) {
            Some(m) => {
                let key = m.after.join(",");
                if m.after.is_empty() && !force {
                    next.insert(id.clone(), Vec::new());
                    keys.insert(id.clone(), key);
                } else if force || keys.get(id) != Some(&key) || !next.contains_key(id) {
                    todo.push(id.clone());
                    keys.insert(id.clone(), key);
                }
            }
            None => {
                if due {
                    todo.push(id.clone());
                }
            }
        }
    }
    for chunk in todo.chunks(6) {
        let mut set = JoinSet::new();
        for id in chunk {
            let cfg = app.cfg.clone();
            let id = id.clone();
            set.spawn(async move {
                let out = run_board(&cfg, &["after", id.as_str()], Duration::from_secs(8)).await;
                (id, out)
            });
        }
        while let Some(joined) = set.join_next().await {
            if let Ok((id, Ok(out))) = joined {
                next.insert(id, board::parse_after(&out));
            }
        }
    }
    cache.map = next.clone();
    cache.keys = keys;
    if due {
        cache.at_seq = seq;
    }
    next
}

fn stale(snap: &mut Snapshot, error: String, now: u64) {
    snap.board.ok = false;
    snap.board.error = error;
    if snap.board.stale_at == 0 {
        snap.board.stale_at = if snap.at > 0 { snap.at } else { now };
    }
}

/// Adds one line when a task's status changes; the first successful read after a
/// start only primes the comparison and adds nothing.
fn update_journal(journal: &mut Journal, tasks: &[Task], now: u64) {
    if !journal.primed {
        journal.primed = true;
        if journal.entries.is_empty() {
            let mut fresh = Vec::new();
            for t in tasks {
                journal.seq += 1;
                fresh.push(JournalEntry {
                    n: journal.seq,
                    t: now,
                    id: t.id.clone(),
                    s: t.status.key().to_string(),
                    text: if t.msg.is_empty() {
                        t.board_label.clone()
                    } else {
                        t.msg.clone()
                    },
                });
            }
            for e in fresh.into_iter().rev() {
                journal.entries.push_front(e);
            }
            trim_journal(journal);
        }
        for t in tasks {
            journal.prev.insert(t.id.clone(), (t.status, t.msg.clone()));
        }
        return;
    }
    let mut changed = false;
    for t in tasks {
        let prev = journal.prev.get(&t.id).cloned();
        if prev.as_ref().map(|(s, _)| *s) != Some(t.status) {
            journal.seq += 1;
            journal.entries.push_front(JournalEntry {
                n: journal.seq,
                t: now,
                id: t.id.clone(),
                s: t.status.key().to_string(),
                text: if t.msg.is_empty() {
                    t.board_label.clone()
                } else {
                    t.msg.clone()
                },
            });
            changed = true;
        }
        journal.prev.insert(t.id.clone(), (t.status, t.msg.clone()));
    }
    let present: std::collections::HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let gone: Vec<String> = journal
        .prev
        .keys()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect();
    for id in gone {
        journal.prev.remove(&id);
        journal.seq += 1;
        journal.entries.push_front(JournalEntry {
            n: journal.seq,
            t: now,
            id,
            s: "gone".into(),
            text: "снята с доски или убрана в архив".into(),
        });
        changed = true;
    }
    if changed {
        trim_journal(journal);
    }
}

fn trim_journal(journal: &mut Journal) {
    while journal.entries.len() > JOURNAL_MAX {
        journal.entries.pop_back();
    }
}

/// `details <id>` plus `show --task <id>`, cached for a few seconds.
pub async fn details(app: &Shared, id: &str) -> Result<Details, String> {
    if !board::valid_id(id) || !id.contains('/') {
        return Err("not a task id".into());
    }
    if let Some((at, d)) = app.details.lock().await.get(id).cloned() {
        if now_ms().saturating_sub(at) < 5000 {
            return Ok(d);
        }
    }
    let cfg = &app.cfg;
    let reports = app.cfg.reports_dir();
    let reports_dir = reports
        .join(board::repo_of(id))
        .join(board::number_of(id).to_string())
        .display()
        .to_string();
    let dargs = ["details", id];
    let sargs = ["show", "--task", id];
    let (det, show) = tokio::join!(
        run_board(cfg, &dargs, Duration::from_secs(15)),
        run_board(cfg, &sargs, Duration::from_secs(10)),
    );
    let text = det?;
    let mut d = board::parse_details(&text, &reports_dir)?;
    if let Ok(show) = show {
        d.agent_model = board::show_agent(&show);
    }
    app.details
        .lock()
        .await
        .insert(id.to_string(), (now_ms(), d.clone()));
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::parse_list;

    fn tasks() -> Vec<Task> {
        parse_list(
            r#"[{"id":"a/1","state":"working","stage":"working","msg":"one"},
                {"id":"a/2","state":"blocked","kind":"question","stage":"working","msg":"two"}]"#,
        )
        .unwrap()
    }

    #[test]
    fn journal_primes_then_logs_changes() {
        let mut j = Journal::default();
        let t = tasks();
        update_journal(&mut j, &t, 100);
        assert_eq!(j.entries.len(), 2);
        // Same data: nothing new.
        update_journal(&mut j, &t, 200);
        assert_eq!(j.entries.len(), 2);
        // One task changes state: one line, first.
        let mut t2 = t.clone();
        t2[0].state = "done".into();
        t2[0].stage = "done".into();
        t2[0].status = Status::Done;
        t2[0].msg = "Merged".into();
        update_journal(&mut j, &t2, 300);
        assert_eq!(j.entries.len(), 3);
        let first = j.entries.front().unwrap();
        assert_eq!(first.id, "a/1");
        assert_eq!(first.s, "done");
        assert_eq!(first.text, "Merged");
        assert_eq!(first.t, 300);
    }

    #[test]
    fn journal_keeps_a_bounded_history() {
        let mut j = Journal::default();
        let mut t = tasks();
        update_journal(&mut j, &t, 1);
        for i in 0..(JOURNAL_MAX * 2) {
            t[0].status = if i % 2 == 0 {
                Status::Done
            } else {
                Status::Working
            };
            update_journal(&mut j, &t, i as u64 + 2);
        }
        assert!(j.entries.len() <= JOURNAL_MAX);
    }

    #[test]
    fn chat_history_drops_images_whose_files_are_gone() {
        use crate::attachments::Attachment;
        let dir = std::env::temp_dir().join(format!(
            "herdr-chat-prune-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let present = "0123456789abcdef0123456789abcdef.png".to_string();
        let missing = "fedcba9876543210fedcba9876543210.png".to_string();
        std::fs::write(dir.join(&present), b"\x89PNG\r\n\x1a\n").unwrap();
        let mut chat = ChatState::default();
        chat.history.push(ChatMessage {
            text: "посмотри".into(),
            at: 1,
            attachments: vec![
                Attachment {
                    id: present.clone(),
                    name: "one.png".into(),
                    mime: "image/png".into(),
                    size: 8,
                },
                Attachment {
                    id: missing,
                    name: "two.png".into(),
                    mime: "image/png".into(),
                    size: 8,
                },
            ],
        });
        assert!(prune_missing_attachments(&dir, &mut chat));
        assert_eq!(chat.history[0].attachments.len(), 1);
        assert_eq!(chat.history[0].attachments[0].id, present);
        assert!(!prune_missing_attachments(&dir, &mut chat));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_reloaded_journal_does_not_log_everything_again() {
        let mut j = Journal {
            entries: VecDeque::from([JournalEntry {
                n: 7,
                t: 1,
                id: "a/1".into(),
                s: "working".into(),
                text: "old".into(),
            }]),
            seq: 7,
            ..Journal::default()
        };
        update_journal(&mut j, &tasks(), 2);
        assert_eq!(j.entries.len(), 1);
        assert_eq!(j.seq, 7);
    }
}

/// The rows of the search index for one galaxy's board (full-text part of the world search).
fn docs_for(
    galaxy: &str,
    host: &str,
    tasks: &[Task],
    projects: &[Project],
) -> Vec<SearchDoc> {
    let mut docs = Vec::new();
    for p in projects {
        docs.push(SearchDoc {
            key: format!("universe:{galaxy}:{}", p.name),
            kind: "universe".into(),
            id: p.name.clone(),
            galaxy: galaxy.to_string(),
            path: host.to_string(),
            title: format!("{} {}", p.name, p.key),
            text: format!("{} {}", p.origin, p.root),
            status: if p.quiet { "queued".into() } else { "working".into() },
            at: 0,
        });
    }
    for t in tasks {
        docs.push(SearchDoc {
            key: format!("planet:{galaxy}:{}", t.id),
            kind: "planet".into(),
            id: t.id.clone(),
            galaxy: galaxy.to_string(),
            path: format!("{host} › {} › {}", t.repo, t.tag),
            title: format!("{} {}", t.tag, t.title),
            text: format!(
                "{} {} {} {} {} {} {}",
                t.description, t.msg, t.board_label, t.app, t.worker_agent, t.model, t.label
            ),
            status: t.status.key().to_string(),
            at: t.created,
        });
        for s in &t.subagents {
            docs.push(SearchDoc {
                key: format!("moon:{galaxy}:{}#{}", t.id, s.id),
                kind: "moon".into(),
                id: format!("{}#{}", t.id, s.id),
                galaxy: galaxy.to_string(),
                path: format!("{host} › {} › {} › {}", t.repo, t.tag, s.role),
                title: format!("{} {}", s.role, s.title),
                text: format!("{} {}", s.doing, s.result),
                status: s.state.clone(),
                at: s.started_at_ms,
            });
        }
    }
    docs
}

/// Brings the search index in line with the snapshot (only what changed is written).
pub async fn index_snapshot(app: &Shared) {
    let snap = app.snapshot.read().await.clone();
    let mut sets: Vec<(String, Vec<SearchDoc>)> = Vec::new();
    let mut home = docs_for("", &snap.host, &snap.tasks, &snap.projects);
    for it in &snap.todo {
        home.push(SearchDoc {
            key: format!("todo::{}", it.id),
            kind: "todo".into(),
            id: it.id.clone(),
            galaxy: String::new(),
            path: format!("{} › {}", snap.host, "дела"),
            title: it.text.clone(),
            text: it.to.clone(),
            status: String::new(),
            at: it.at,
        });
    }
    home.push(SearchDoc {
        key: "quasar::quasar".into(),
        kind: "quasar".into(),
        id: "quasar".into(),
        galaxy: String::new(),
        path: snap.host.clone(),
        title: "Квазар Координатор Quasar Coordinator".into(),
        text: snap.coordinator.view.status_line.clone(),
        status: snap.coordinator.view.status.clone(),
        at: 0,
    });
    sets.push((String::new(), home));
    for g in app.galaxies.lock().await.iter() {
        let projects: Vec<Project> = g
            .projects
            .iter()
            .map(|p| Project { name: p.name.clone(), key: p.key.clone(), origin: p.origin.clone().unwrap_or_default(), root: p.folder.clone(), ..Project::default() })
            .collect();
        let mut docs = docs_for(&g.label, &g.label, &g.tasks, &projects);
        docs.push(SearchDoc {
            key: format!("galaxy:{}:{}", g.label, g.label),
            kind: "galaxy".into(),
            id: g.label.clone(),
            galaxy: g.label.clone(),
            path: g.target.clone(),
            title: g.label.clone(),
            text: format!("{} {}", g.target, g.host),
            status: if g.ok { "working".into() } else { "failed".into() },
            at: g.fetched_at_ms,
        });
        sets.push((g.label.clone(), docs));
    }
    let db = app.db.clone();
    let _ = tokio::task::spawn_blocking(move || {
        use std::sync::Mutex;
        static LAST: std::sync::OnceLock<Mutex<HashMap<String, Vec<SearchDoc>>>> = std::sync::OnceLock::new();
        let last = LAST.get_or_init(|| Mutex::new(HashMap::new()));
        for (galaxy, docs) in sets {
            let same = last.lock().map(|m| m.get(&galaxy) == Some(&docs)).unwrap_or(false);
            if !same {
                db.search_docs_replace(&galaxy, &docs);
                if let Ok(mut m) = last.lock() {
                    m.insert(galaxy, docs);
                }
            }
        }
    })
    .await;
}

/// Indexes the Coordinator's transcript for the chat search (heavy: the whole file is read).
pub async fn index_chat(app: &Shared) {
    let db = app.db.clone();
    let cfg = app.cfg.clone();
    let _ = tokio::task::spawn_blocking(move || history::sync_index(&db, &cfg)).await;
}

/// Reads how far the clones' main branches are from their origins (read-only git, no fetch),
/// on this machine and on the others.
pub async fn update_lag(app: &Shared) {
    let repos = taskmeta::repos(&app.cfg.data_dir);
    let folders: Vec<(String, String)> = repos
        .iter()
        .filter(|r| !r.root.is_empty())
        .map(|r| (r.name.clone(), r.root.clone()))
        .collect();
    let local = gitlag::local(&folders).await;
    for (repo, l) in &local {
        if let Ok(json) = serde_json::to_string(l) {
            app.db.gitlag_put("", repo, &json, now_ms());
        }
    }
    *app.lag_local.lock().await = local;
    let galaxies = app.galaxies.lock().await.clone();
    for g in galaxies.iter().filter(|g| g.ok) {
        let folders: Vec<(String, String)> = g
            .projects
            .iter()
            .filter(|p| !p.folder.is_empty())
            .map(|p| (p.name.clone(), p.folder.clone()))
            .collect();
        if folders.is_empty() {
            continue;
        }
        let lag = gitlag::remote(&app.cfg, &g.label, &folders).await;
        app.lag_remote.lock().await.insert(g.label.clone(), lag);
    }
    rebuild_extras(app).await;
    publish(app).await;
}
