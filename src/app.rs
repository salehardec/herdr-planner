//! The planner's one process: the board snapshot the page reads, the journal
//! of what changed, the Coordinator's standing line and the details cache.

use crate::board::{self, Details, Link, Project, Status, Summary, Task};
use crate::config::{now_ms, AppConfig};
use crate::coordinator::{self, CoordinatorView};
use crate::theme::{self, Theme};
use crate::usage::{self, Usage};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinSet;
use tokio::time::timeout;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const JOURNAL_MAX: usize = 80;

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
}

/// One item of the God's own list: an idea that is not on the board yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    pub at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub version: String,
    pub at: u64,
    pub seq: u64,
    pub board: BoardStatus,
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
    #[serde(skip)]
    pub busy_until: u64,
}

#[derive(Debug, Default)]
pub struct Journal {
    seq: u64,
    entries: VecDeque<JournalEntry>,
    prev: HashMap<String, (Status, String)>,
    primed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SavedJournal {
    seq: u64,
    entries: Vec<JournalEntry>,
}

#[derive(Debug, Default)]
pub struct AfterCache {
    /// The snapshot sequence the cached map belongs to.
    pub at_seq: u64,
    /// Task id → tags it waits for, from `herdr-projects after <id>`.
    pub map: BTreeMap<String, Vec<String>>,
}

pub struct App {
    pub cfg: AppConfig,
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
}

pub type Shared = Arc<App>;

impl App {
    pub fn new(cfg: AppConfig) -> Shared {
        let theme = theme::load();
        let usage = usage::read();
        let snapshot = Snapshot {
            version: VERSION.to_string(),
            at: 0,
            seq: 0,
            board: BoardStatus {
                ok: false,
                error: String::new(),
                stale_at: 0,
            },
            tasks: Vec::new(),
            archived: Vec::new(),
            projects: Vec::new(),
            summary: Summary::default(),
            links: Vec::new(),
            limits: usage,
            todo: load_todo(&cfg),
            journal: Vec::new(),
            coordinator: CoordinatorState {
                view: CoordinatorView::default(),
                source: "herdr-projects coordinator".into(),
            },
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
            },
        };
        let chat = load_chat(&cfg);
        let journal = load_journal(&cfg);
        let auth = crate::auth::Auth::new(
            cfg.auth_required(),
            cfg.user.clone(),
            cfg.password_hash.clone(),
        );
        Arc::new(Self {
            cfg,
            snapshot: RwLock::new(snapshot),
            auth,
            chat: Mutex::new(chat),
            journal: Mutex::new(journal),
            details: Mutex::new(HashMap::new()),
            refresh_lock: Mutex::new(()),
            action_lock: Mutex::new(()),
            after: Mutex::new(AfterCache::default()),
            force_after: AtomicBool::new(false),
        })
    }
}

fn state_json_path(cfg: &AppConfig, name: &str) -> std::path::PathBuf {
    cfg.state_dir.join(name)
}

fn load_todo(cfg: &AppConfig) -> Vec<TodoItem> {
    std::fs::read_to_string(state_json_path(cfg, "todo.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_todo(cfg: &AppConfig, todo: &[TodoItem]) {
    if let Ok(s) = serde_json::to_string(todo) {
        write_state(state_json_path(cfg, "todo.json"), &s);
    }
}

fn load_chat(cfg: &AppConfig) -> ChatState {
    std::fs::read_to_string(state_json_path(cfg, "chat.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_chat(cfg: &AppConfig, chat: &ChatState) {
    if let Ok(s) = serde_json::to_string(chat) {
        write_state(state_json_path(cfg, "chat.json"), &s);
    }
}

/// State files hold task text and the last chat message: keep them for this user only.
fn write_state(path: std::path::PathBuf, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    if std::fs::write(&path, text).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

fn load_journal(cfg: &AppConfig) -> Journal {
    let saved: SavedJournal = std::fs::read_to_string(state_json_path(cfg, "journal.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Journal {
        seq: saved.seq,
        entries: saved.entries.into_iter().collect(),
        prev: HashMap::new(),
        primed: false,
    }
}

fn save_journal(cfg: &AppConfig, j: &Journal) {
    let _ = std::fs::create_dir_all(&cfg.state_dir);
    let saved = SavedJournal {
        seq: j.seq,
        entries: j.entries.iter().cloned().collect(),
    };
    if let Ok(s) = serde_json::to_string(&saved) {
        write_state(state_json_path(cfg, "journal.json"), &s);
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

/// Reads the board, the Coordinator and the theme; safe to call at any time.
/// Failed board reads keep the previous tasks and mark them stale.
pub async fn refresh(app: &Shared) {
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
        save_chat(cfg, &chat);
    }
    *app.chat.lock().await = chat;

    let seq = app.snapshot.read().await.seq + 1;
    let read = match &json {
        Ok(text) => board::parse_list(text),
        Err(e) => Err(e.clone()),
    };

    // The waits are read outside the snapshot lock: each is a board process,
    // and a slow one must not keep the page from reading the snapshot.
    let (active, archived, links) = match read {
        Ok(mut tasks) => {
            if let Ok(plain) = &plain {
                let tags = board::plain_tags(plain);
                board::apply_tags(&mut tasks, &tags);
            }
            let (archived, active): (Vec<Task>, Vec<Task>) =
                tasks.into_iter().partition(|t| t.archived);
            let after = poll_after(app, seq, &active).await;
            let mut all = active.clone();
            all.extend(archived.iter().cloned());
            let links = board::links_from(&after, &all);
            (Ok(active), archived, links)
        }
        Err(e) => (Err(e), Vec::new(), Vec::new()),
    };

    let mut snap = app.snapshot.write().await;
    snap.at = now;
    snap.seq += 1;
    snap.theme = theme;
    snap.limits = usage;
    snap.herdr.running = cfg.herdr_socket.exists();
    snap.coordinator = CoordinatorState {
        view: coord_view,
        source: "herdr-projects coordinator · последние строки окна Координатора".into(),
    };

    match active {
        Ok(active) => {
            let projects = board::projects(&active);
            let summary = board::summary(&active, &archived, &projects);
            let mut journal = app.journal.lock().await;
            update_journal(&mut journal, &active, now);
            save_journal(cfg, &journal);
            let entries: Vec<JournalEntry> = journal.entries.iter().cloned().collect();
            snap.journal = entries;
            snap.tasks = active;
            snap.archived = archived;
            snap.projects = projects;
            snap.summary = summary;
            snap.links = links;
            snap.board = BoardStatus {
                ok: true,
                error: String::new(),
                stale_at: 0,
            };
        }
        Err(e) => stale(&mut snap, e, now),
    }
}

/// Reads `herdr-projects after <id>` for every active, unfinished task. Six
/// processes at a time; if there are more than 24 tasks the walk runs every
/// third refresh unless an `after`/`delegate` action asked for it right away.
/// A failed read keeps what was known for that task.
async fn poll_after(app: &Shared, seq: u64, tasks: &[Task]) -> BTreeMap<String, Vec<String>> {
    let ids: Vec<String> = tasks
        .iter()
        .filter(|t| !t.archived && t.status != Status::Done)
        .map(|t| t.id.clone())
        .collect();
    let mut cache = app.after.lock().await;
    let force = app.force_after.swap(false, Ordering::SeqCst);
    let every = if ids.len() > 24 { 3 } else { 1 };
    if !force && cache.at_seq != 0 && seq.saturating_sub(cache.at_seq) < every {
        return cache.map.clone();
    }
    let mut next = cache.map.clone();
    next.retain(|id, _| ids.iter().any(|x| x == id));
    for chunk in ids.chunks(6) {
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
    cache.at_seq = seq;
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
