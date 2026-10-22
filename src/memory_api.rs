//! The memory panel's endpoints, the Coordinator's instructions and the suggestions made when a task is done. The
//! files and their format are in `memory`.
//!
//! * The Coordinator is told about the memory through the board's own setting `coordinator.instructions`: the planner
//!   shows the human the instructions they have and the same with a memory block added at the end (their text kept),
//!   and on their confirmation the board's `set` makes the change (the Station's endpoints). The planner writes no
//!   file of the board (its `AGENTS.md`, `CLAUDE.md`, `rules.md`): the board makes them from the setting and may
//!   rewrite them at any time.

use crate::app::{self, Shared};
use crate::board::Task;
use crate::config::now_ms;
use crate::memory::{self, Entry, Line};
use crate::memory_guard::{self, Finding};
use crate::server::{error, same_origin};
use crate::station::{self, Fail};
use crate::{taskmeta, translate};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

const SUGGEST_KEY: &str = "memory.suggest";

/// Whether the model proposes memory lines when a task is done (on until the human switches it off).
pub fn suggest_enabled(app: &Shared) -> bool {
    app.db.meta_get(SUGGEST_KEY).as_deref() != Some("off")
}

/// The board's setting that carries the human's own instructions for its Coordinator.
pub const INSTRUCTIONS: &str = "coordinator.instructions";

/// What the panel shows about the memory block in `coordinator.instructions` of this machine's board. Nothing is changed
/// here: `memory_connect` previews the change and, on the human's confirmation, makes it with the board's own `set`.
/// Files the board writes (its `AGENTS.md`, `rules.md`) are not touched.
async fn connect_state(app: &Shared, galaxy_dir: &str) -> Value {
    let block = memory::coordinator_block(galaxy_dir);
    match station::read_settings(&app.cfg, "").await {
        Ok(settings) => {
            // «none» is what an unset setting says
            let current = station::find(&settings, INSTRUCTIONS)
                .filter(|s| s.from != "built in" && s.value.trim() != "none")
                .map(|s| s.value.clone())
                .unwrap_or_default();
            json!({
                "setting": INSTRUCTIONS,
                "block": block,
                "current": current,
                "connected": memory::has_block(&current, &block),
                "stale": memory::has_any_block(&current) && !memory::has_block(&current, &block),
                "error": "",
            })
        }
        Err(f) => json!({
            "setting": INSTRUCTIONS, "block": block, "current": "", "connected": false, "stale": false,
            "error": f.message(),
        }),
    }
}

/// Run by the refresh loop now and then: the tables of contents that hand-written files left behind. (The file watcher
/// does it at once when a memory file changes.)
pub async fn keep_indexes(app: &Shared) {
    // The refresh loop runs at every poke of the board: look at the tables of contents every 20 s at most.
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = now_ms();
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < 20_000 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    reindex_stores(app).await;
}

/// The table of contents that an agent's hand-written files left behind: the Galaxy's. The file watcher calls this at once when a memory file changes; the refresh loop only now and then.
pub async fn reindex_stores(app: &Shared) {
    let cfg = app.cfg.clone();
    let _ = tokio::task::spawn_blocking(move || {
        memory::galaxy_store(&cfg).reindex();
        // A universe's index is not rewritten here: that would make the human's checkout «dirty» for the board's merges.
        // The task that sends the memory to the repository brings its lines.
    })
    .await;
}

/* ───────── the panel ───────── */

#[derive(Deserialize)]
pub struct GetQuery {
    repo: Option<String>,
}

fn entry_json(e: &Entry) -> Value {
    serde_json::to_value(e).unwrap_or(Value::Null)
}

fn bad_scope(repo: &str) -> Option<Fail> {
    (!repo.is_empty() && !taskmeta::valid_repo(repo)).then(|| Fail::new(400, "bad universe name"))
}

async fn panel_read(app: &Shared, q: GetQuery) -> Result<Value, Fail> {
    let repo = q.repo.unwrap_or_default();
    if let Some(f) = bad_scope(&repo) {
        return Err(f);
    }
    let level = if repo.is_empty() { "galaxy" } else { "universe" };
    let cfg = app.cfg.clone();
    let repo2 = repo.clone();
    let mut v = tokio::task::spawn_blocking(move || -> Result<Value, Fail> {
        if repo2.is_empty() {
            let st = memory::galaxy_store(&cfg);
            let foreign: Vec<Value> = memory::foreign_sources(&cfg)
                .into_iter()
                .map(|f| {
                    let entries: Vec<Value> = memory::read_dir_entries(&f.dir).iter().map(entry_json).collect();
                    json!({"program": f.program, "label": f.label, "dir": f.dir.display().to_string(), "entries": entries})
                })
                .collect();
            Ok(json!({
                "dir": st.dir.display().to_string(),
                "exists": st.exists(),
                "writable": true,
                "entries": st.list().iter().map(entry_json).collect::<Vec<_>>(),
                "foreign": foreign,
            }))
        } else {
            let root = memory::repo_root(&cfg, &repo2)
                .ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
            let st = memory::universe_store(&root, &repo2);
            let dirty = memory::git_dirty(&root);
            // In a git checkout the drafts are in front of the files (and not in the checkout).
            let drafts = dirty.is_some().then(|| memory::Drafts::new(&cfg, &repo2));
            // What was sent by a task and is not in the checkout yet stays on the panel, marked with the task.
            let sent = if drafts.is_some() { memory::SentLog::new(&cfg, &repo2).list(&st, now_ms()) } else { Vec::new() };
            let mut entries: Vec<Value> = match &drafts {
                Some(d) => memory::effective(&st, d)
                    .iter()
                    .map(|(e, mark)| {
                        let mut j = entry_json(e);
                        j["draft"] = json!(mark);
                        j
                    })
                    .collect(),
                None => st.list().iter().map(entry_json).collect(),
            };
            for b in &sent {
                for (id, text) in &b.files {
                    let at = entries.iter().position(|j| j["id"] == json!(id));
                    let drafted = at.is_some_and(|i| entries[i]["draft"].as_str().is_some_and(|m| !m.is_empty()));
                    match (text, at) {
                        _ if drafted => {}
                        (Some(text), at) => {
                            let mut j = entry_json(&memory::parse_entry(id, text));
                            j["draft"] = json!("sent");
                            j["task"] = json!(b.task);
                            match at {
                                Some(i) => entries[i] = j,
                                None => entries.push(j),
                            }
                        }
                        (None, Some(i)) => {
                            entries[i]["draft"] = json!("sentdel");
                            entries[i]["task"] = json!(b.task);
                        }
                        (None, None) => {}
                    }
                }
            }
            entries.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
            let git = match &dirty {
                None => json!({"checkout": false, "dirty": [], "drafts": 0, "sent": []}),
                Some(d) => json!({
                    "checkout": true,
                    "dirty": d.iter().map(|(s, f)| json!({"state": s, "file": f})).collect::<Vec<_>>(),
                    "drafts": drafts.as_ref().map(|d| d.puts().len() + d.dels().len()).unwrap_or(0),
                    "sent": sent.iter().map(|b| json!({"task": b.task, "id": b.id, "title": b.title, "files": b.files.len()})).collect::<Vec<_>>(),
                }),
            };
            Ok(json!({
                "dir": st.dir.display().to_string(),
                "exists": st.exists(),
                "writable": true,
                "entries": entries,
                "git": git,
            }))
        }
    })
    .await
    .map_err(|e| Fail::new(500, e.to_string()))??;
    if repo.is_empty() {
        v["connect"] = connect_state(app, &memory::galaxy_dir(&app.cfg).display().to_string()).await;
        v["settings"] = json!({"suggest": suggest_enabled(app)});
    } else {
        v["suggestions"] = json!(suggestions_for(app, &repo).await);
    }
    v["repo"] = json!(repo);
    v["level"] = json!(level);
    v["ok"] = json!(true);
    Ok(v)
}

/// `GET /api/memory?repo=`: the entries of a level, with the Coordinator's wiring (Galaxy) or git state and
/// suggestions (universe).
pub async fn memory_get(State(app): State<Shared>, Query(q): Query<GetQuery>) -> Response {
    match panel_read(&app, q).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

/* ───────── changes: «было → стало», the personal-data check, the human's «да» ───────── */

/// What the human confirms with: nothing (a preview is made), or `confirm` with the token of the preview they saw and,
/// when the text holds personal data or secrets, their explicit word about it.
#[derive(Deserialize, Default)]
pub struct Gate {
    confirm: Option<bool>,
    /// The token of the preview: a hash of «было → стало» and the findings. A write with another token (the file
    /// changed meanwhile, the text was edited) is refused, so what is written is what was shown.
    token: Option<String>,
    /// «Да, записать» when something personal was found (a repository that is not public).
    accept_risk: Option<bool>,
    /// The stronger word for a public repository: by default nothing with findings is written there.
    accept_public: Option<bool>,
}

/// Whether a universe's repository is public: a text with personal data is stricter there.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
    /// No remote: the repository is only on this machine.
    Local,
    /// A remote the hosting could not be asked about (no login, another hosting): treated as public.
    Unknown,
}

impl Visibility {
    pub fn strict(self) -> bool {
        matches!(self, Visibility::Public | Visibility::Unknown)
    }
}

type VisibilityCache = std::sync::Mutex<HashMap<String, (std::time::Instant, Visibility)>>;

/// Asks the hosting once in ten minutes (`gh repo view`, `glab api`: the same as when a universe is added).
async fn visibility_of(app: &Shared, root: &str) -> Visibility {
    static CACHE: std::sync::OnceLock<VisibilityCache> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().ok().and_then(|c| c.get(root).copied()) {
        if at.elapsed() < Duration::from_secs(600) {
            return v;
        }
    }
    let v = match crate::universe::inspect(&app.cfg, root).await {
        Ok(i) if i.origin.is_none() => Visibility::Local,
        Ok(i) => match i.private {
            Some(true) => Visibility::Private,
            Some(false) => Visibility::Public,
            None => Visibility::Unknown,
        },
        Err(_) => Visibility::Unknown,
    };
    if let Ok(mut c) = cache.lock() {
        c.insert(root.to_string(), (std::time::Instant::now(), v));
    }
    v
}

/// One file's change.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    /// `add`, `change` or `delete`.
    pub op: &'static str,
    pub id: String,
    /// Where the file is: a path for the Galaxy, `docs/memory/<id>.md` for a universe.
    pub file: String,
    pub before: Option<String>,
    pub after: Option<String>,
    #[serde(skip)]
    entry: Option<Entry>,
}

#[derive(Debug, Serialize)]
struct FindingOut {
    file: String,
    #[serde(flatten)]
    f: Finding,
}

/// A change as the human will see it, before anything is written.
struct Plan {
    repo: String,
    changes: Vec<Change>,
    findings: Vec<FindingOut>,
    visibility: Option<Visibility>,
    /// The entries go to the drafts (outside the checkout), not to files.
    draft: bool,
    /// The same entries with what was found taken out, to offer instead.
    redacted: Vec<Value>,
}

/// `none`: nothing found. `confirm`: something found, a «да» is needed. `block`: public repository, forbidden unless
/// the human says it explicitly.
fn risk_of(findings: &[FindingOut], vis: Option<Visibility>) -> &'static str {
    if findings.is_empty() {
        "none"
    } else if vis.is_none_or(Visibility::strict) {
        "block"
    } else {
        "confirm"
    }
}

impl Plan {
    fn risk(&self) -> &'static str {
        risk_of(&self.findings, self.visibility)
    }

    fn token(&self) -> String {
        token_of(&self.changes, &self.findings)
    }

    fn preview(&self) -> Value {
        let level = if self.repo.is_empty() { "galaxy" } else { "universe" };
        json!({
            "ok": true, "preview": true, "token": self.token(), "level": level, "repo": self.repo,
            "draft": self.draft, "changes": self.changes, "findings": self.findings, "risk": self.risk(),
            "visibility": self.visibility, "redacted": self.redacted,
        })
    }
}

fn token_of(changes: &[Change], findings: &[FindingOut]) -> String {
    use sha2::{Digest, Sha256};
    let body = serde_json::to_string(&(changes, findings)).unwrap_or_default();
    let sum = Sha256::digest(body.as_bytes());
    sum.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

fn change_of(id: &str, file: String, before: Option<&Entry>, after: Option<Entry>) -> Change {
    Change {
        op: match (before, &after) {
            (None, _) => "add",
            (Some(_), Some(_)) => "change",
            (Some(_), None) => "delete",
        },
        id: id.to_string(),
        file,
        before: before.map(memory::render_entry),
        after: after.as_ref().map(memory::render_entry),
        entry: after,
    }
}

/// What the personal-data check looks for besides the patterns: this person's own terms, the names of machines they know
/// (ssh aliases, `/etc/hosts`), and the names of the board's shipyards (`runners.json`: the label and the host of each).
fn guard_terms(cfg: &crate::config::AppConfig) -> Vec<String> {
    let mut own = memory_guard::own_terms();
    if let Ok(file) = crate::runners::load(&cfg.runners_file()) {
        let names = file.runners.iter().flat_map(|r| {
            let host = crate::ssh::split_target(&r.target).dest;
            let host = host.rsplit('@').next().unwrap_or("").trim_matches(['[', ']']).to_string();
            [r.id.clone(), host]
        });
        for t in memory_guard::host_terms(names, 3) {
            if !own.iter().any(|o| o.eq_ignore_ascii_case(&t)) {
                own.push(t);
            }
        }
    }
    own
}

/// What the check finds in the entries that will be written; for a universe only (the Galaxy's memory is local).
fn check_entries(changes: &[Change], own: &[String]) -> (Vec<FindingOut>, Vec<Value>) {
    let mut findings = Vec::new();
    let mut redacted = Vec::new();
    for c in changes {
        let Some(e) = &c.entry else {
            redacted.push(Value::Null);
            continue;
        };
        let before = findings.len();
        for (field, text) in [("title", &e.title), ("description", &e.description), ("body", &e.body)] {
            for f in memory_guard::scan(text, field, own) {
                // the summary is cut from the body when the human wrote none: one hit, listed once
                if !findings.iter().any(|o: &FindingOut| o.file == c.file && o.f.kind == f.kind && o.f.text == f.text) {
                    findings.push(FindingOut { file: c.file.clone(), f });
                }
            }
        }
        if findings.len() > before {
            redacted.push(json!({
                "title": memory_guard::redact(&e.title, own),
                "description": memory_guard::redact(&e.description, own),
                "body": memory_guard::redact(&e.body, own),
            }));
        } else {
            redacted.push(Value::Null);
        }
    }
    (findings, redacted)
}

/// A level of memory on this machine: the files, and (a universe in a git checkout) the drafts in front of them.
struct Local {
    store: memory::Store,
    drafts: Option<memory::Drafts>,
    root: Option<String>,
    /// The ids a board task is about to bring (sent, not merged yet): a new entry does not take them.
    sent: Vec<String>,
}

impl Local {
    fn open(cfg: &crate::config::AppConfig, repo: &str) -> Result<Local, Fail> {
        if repo.is_empty() {
            return Ok(Local { store: memory::galaxy_store(cfg), drafts: None, root: None, sent: Vec::new() });
        }
        let root = memory::repo_root(cfg, repo).ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
        let drafts = memory::git_dirty(&root).is_some().then(|| memory::Drafts::new(cfg, repo));
        let store = memory::universe_store(&root, repo);
        let sent = if drafts.is_some() {
            memory::SentLog::new(cfg, repo).list(&store, now_ms()).into_iter().flat_map(|b| b.files.into_iter().map(|(id, _)| id)).collect()
        } else {
            Vec::new()
        };
        Ok(Local { store, drafts, root: Some(root), sent })
    }

    fn get(&self, id: &str) -> Option<Entry> {
        match &self.drafts {
            Some(d) => memory::effective_get(&self.store, d, id).filter(|(_, m)| *m != "deleted").map(|(e, _)| e),
            None => self.store.get(id),
        }
    }

    fn taken(&self, id: &str) -> bool {
        self.sent.iter().any(|s| s == id) || self.store.dir.join(format!("{id}.md")).exists() || self.drafts.as_ref().is_some_and(|d| d.puts().iter().any(|e| e.id == id))
    }

    fn file(&self, id: &str, repo: &str) -> String {
        if repo.is_empty() {
            self.store.dir.join(format!("{id}.md")).display().to_string()
        } else {
            format!("{}/{id}.md", memory::UNIVERSE_DIR)
        }
    }
}

enum Req {
    Save(SaveBody),
    Delete { repo: String, id: String },
    Lines { repo: String, task: String, lines: Vec<Line> },
}

/// Works out what a request would change, without writing anything.
async fn plan(app: &Shared, req: &Req) -> Result<Plan, Fail> {
    let repo = match req {
        Req::Save(b) => b.repo.clone(),
        Req::Delete { repo, .. } | Req::Lines { repo, .. } => repo.clone(),
    };
    if let Some(f) = bad_scope(&repo) {
        return Err(f);
    }
    match req {
        Req::Save(b) if !b.id.is_empty() && !memory::valid_stem(&b.id) => return Err(Fail::new(400, "недопустимое имя записи")),
        Req::Delete { id, .. } if !memory::valid_stem(id) => return Err(Fail::new(400, "недопустимое имя записи")),
        _ => {}
    }
    let default_kind = if repo.is_empty() { "rule" } else { "decision" };
    let (cfg, repo2, req2) = (app.cfg.clone(), repo.clone(), req_clone(req));
    let (changes, draft, root) = tokio::task::spawn_blocking(move || -> Result<(Vec<Change>, bool, Option<String>), Fail> {
        let lv = Local::open(&cfg, &repo2)?;
        let now = now_ms();
        let mut out: Vec<Change> = Vec::new();
        match &req2 {
            Req::Save(b) => {
                let existing = if b.id.is_empty() { None } else { lv.get(&b.id) };
                let mut entry = memory::complete(merged(existing.clone(), b), default_kind, now).map_err(|e| Fail::new(422, e))?;
                memory::assign_id(&mut entry, |c| lv.taken(c)).map_err(|e| Fail::new(400, e))?;
                let file = lv.file(&entry.id, &repo2);
                out.push(change_of(&entry.id.clone(), file, existing.as_ref(), Some(entry)));
            }
            Req::Delete { id, .. } => {
                let existing = lv.get(id).ok_or_else(|| Fail::new(404, "такой записи нет"))?;
                out.push(change_of(id, lv.file(id, &repo2), Some(&existing), None));
            }
            Req::Lines { task, lines, .. } => {
                let mut taken: Vec<String> = Vec::new();
                for l in lines {
                    let e = Entry { kind: l.kind.clone(), body: l.text.clone(), source: format!("task {task}"), ..Entry::default() };
                    let mut e = memory::complete(e, "decision", now).map_err(|er| Fail::new(422, er))?;
                    memory::assign_id(&mut e, |c| lv.taken(c) || taken.iter().any(|t| t == c)).map_err(|er| Fail::new(400, er))?;
                    taken.push(e.id.clone());
                    let (id, file) = (e.id.clone(), lv.file(&e.id, &repo2));
                    out.push(change_of(&id, file, None, Some(e)));
                }
            }
        }
        Ok((out, lv.drafts.is_some(), lv.root.clone()))
    })
    .await
    .map_err(|e| Fail::new(500, e.to_string()))??;
    if changes.iter().all(|c| c.op == "change" && c.before == c.after) {
        return Err(Fail::new(422, "ничего не изменится: запись такая же"));
    }
    // The check is for what travels with git: a universe's memory (the Galaxy's stays on this machine).
    let (findings, redacted, visibility) = if repo.is_empty() {
        (Vec::new(), Vec::new(), None)
    } else {
        let (findings, redacted) = check_entries(&changes, &guard_terms(&app.cfg));
        let vis = match (&root, findings.is_empty()) {
            (Some(r), false) => Some(visibility_of(app, r).await),
            _ => None,
        };
        (findings, redacted, vis)
    };
    Ok(Plan { repo, changes, findings, visibility, draft, redacted })
}

fn req_clone(r: &Req) -> Req {
    match r {
        Req::Save(b) => Req::Save(b.clone()),
        Req::Delete { repo, id } => Req::Delete { repo: repo.clone(), id: id.clone() },
        Req::Lines { repo, task, lines } => Req::Lines { repo: repo.clone(), task: task.clone(), lines: lines.clone() },
    }
}

/// What the human's word allows for a text with findings: nothing in a public (or unknown) repository unless they say it
/// explicitly, an ordinary «да» elsewhere.
fn risk_allowed(risk: &str, gate: &Gate) -> Result<(), Fail> {
    match risk {
        "block" if gate.accept_public != Some(true) => Err(Fail::new(
            403,
            "в тексте найдены персональные данные или секреты, а репозиторий публичный (или это неизвестно): по умолчанию в него не пишем",
        )),
        "confirm" if gate.accept_risk != Some(true) && gate.accept_public != Some(true) => Err(Fail::new(
            422,
            "в тексте найдены персональные данные или секреты: нужно явное «да» (или обезличьте текст)",
        )),
        _ => Ok(()),
    }
}

/// What the human's word allows: refuses a write that the preview did not show, or that holds personal data without
/// their word for it.
fn allowed(plan: &Plan, gate: &Gate) -> Result<(), Fail> {
    if gate.token.as_deref() != Some(plan.token().as_str()) {
        return Err(Fail::new(409, "запись изменилась с тех пор, как вы смотрели «было → стало»: посмотрите ещё раз"));
    }
    risk_allowed(plan.risk(), gate)
}

/// Writes a plan: files for the Galaxy (and a folder that is not in git), drafts for a universe in a git checkout.
async fn execute(app: &Shared, plan: &Plan) -> Result<Value, Fail> {
    let entries: Vec<(String, Option<Entry>)> = plan.changes.iter().map(|c| (c.id.clone(), c.entry.clone())).collect();
    let (cfg, repo) = (app.cfg.clone(), plan.repo.clone());
    let saved = tokio::task::spawn_blocking(move || -> Result<Vec<Entry>, Fail> {
        let lv = Local::open(&cfg, &repo)?;
        let mut saved = Vec::new();
        for (id, entry) in entries {
            match (entry, &lv.drafts) {
                (Some(e), Some(d)) => {
                    d.put(&e).map_err(|er| Fail::new(500, er))?;
                    saved.push(e);
                }
                (Some(e), None) => saved.push(lv.store.save(e).map_err(|er| Fail::new(500, er))?),
                (None, Some(d)) => d.delete(&id, lv.store.get(&id).is_some()).map_err(|er| Fail::new(500, er))?,
                (None, None) => {
                    lv.store.delete(&id).map_err(|er| Fail::new(500, er))?;
                }
            }
        }
        Ok(saved)
    })
    .await
    .map_err(|e| Fail::new(500, e.to_string()))??;
    app::index_snapshot(app).await;
    Ok(json!({
        "ok": true, "draft": plan.draft, "saved": saved.iter().map(entry_json).collect::<Vec<_>>(),
        "entry": saved.first().map(entry_json), "removed": plan.changes.iter().any(|c| c.op == "delete"),
    }))
}

/// A request without `confirm` is a preview; with `confirm` and the preview's token it is written.
async fn run(app: &Shared, req: Req, gate: &Gate) -> Result<Value, Fail> {
    let p = plan(app, &req).await?;
    if gate.confirm != Some(true) {
        return Ok(p.preview());
    }
    allowed(&p, gate)?;
    execute(app, &p).await
}

fn respond(r: Result<Value, Fail>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[derive(Deserialize, Clone)]
pub struct SaveBody {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
pub struct SaveReq {
    #[serde(flatten)]
    body: SaveBody,
    #[serde(flatten)]
    gate: Gate,
}

fn merged(existing: Option<Entry>, b: &SaveBody) -> Entry {
    // An edit keeps what the form does not carry: the date it was added, where it came from.
    let (added, source) = existing
        .as_ref()
        .map(|e| (e.added.clone(), e.source.clone()))
        .unwrap_or_default();
    Entry {
        id: b.id.clone(),
        title: b.title.clone(),
        description: b.description.clone(),
        kind: b.kind.clone(),
        body: b.body.clone(),
        added,
        source: if b.source.is_empty() { source } else { b.source.clone() },
        at: 0,
    }
}

/// `POST /api/memory/save`: a new entry (no `id`) or an edit. Without `confirm` it answers with what would change
/// («было → стало», the level and file, what the personal-data check found); with `confirm` and that answer's `token`
/// it writes. A universe in a git checkout gets a draft outside the checkout; it goes to the repository by a board task (`/api/memory/send`).
pub async fn memory_save(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<SaveReq>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    respond(run(&app, Req::Save(body.body), &body.gate).await)
}

#[derive(Deserialize)]
pub struct DeleteBody {
    #[serde(default)]
    repo: String,
    id: String,
    #[serde(flatten)]
    gate: Gate,
}

/// `POST /api/memory/delete`: removes an entry, with the same preview and confirmation as a save.
pub async fn memory_delete(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<DeleteBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    respond(run(&app, Req::Delete { repo: body.repo, id: body.id }, &body.gate).await)
}

#[derive(Deserialize)]
pub struct DiscardBody {
    repo: String,
    id: String,
    #[serde(flatten)]
    gate: Gate,
}

/// What throwing a draft away would change: the draft goes back to what git has (or disappears).
fn discard_plan(cfg: &crate::config::AppConfig, repo: &str, id: &str) -> Result<Vec<Change>, Fail> {
    let root = memory::repo_root(cfg, repo).ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
    let drafts = memory::Drafts::new(cfg, repo);
    let committed = memory::universe_store(&root, repo).get(id);
    let file = format!("{}/{id}.md", memory::UNIVERSE_DIR);
    if let Some(d) = drafts.puts().into_iter().find(|e| e.id == id) {
        return Ok(vec![Change {
            op: if committed.is_some() { "change" } else { "delete" },
            id: id.to_string(),
            file,
            before: Some(memory::render_entry(&d)),
            after: committed.as_ref().map(memory::render_entry),
            entry: None,
        }]);
    }
    if drafts.dels().iter().any(|d| d == id) {
        return Ok(vec![Change {
            op: "change",
            id: id.to_string(),
            file,
            before: Some("(запись помечена на удаление)".to_string()),
            after: committed.as_ref().map(memory::render_entry),
            entry: None,
        }]);
    }
    Err(Fail::new(404, "черновика этой записи нет"))
}

/// `POST /api/memory/discard`: drops a draft of a universe's entry (what git has stays). A change like any other: without
/// `confirm` the answer is «было → стало» (the draft → what git has), with `confirm` and its token the draft goes.
pub async fn memory_discard(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<DiscardBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if !taskmeta::valid_repo(&body.repo) || !memory::valid_stem(&body.id) {
        return error(StatusCode::BAD_REQUEST, "bad name");
    }
    let (cfg, repo, id) = (app.cfg.clone(), body.repo.clone(), body.id.clone());
    let result = tokio::task::spawn_blocking(move || -> Result<Value, Fail> {
        let changes = discard_plan(&cfg, &repo, &id)?;
        let token = token_of(&changes, &[]);
        if body.gate.confirm != Some(true) {
            return Ok(json!({
                "ok": true, "preview": true, "token": token, "level": "universe", "repo": repo, "draft": true,
                "discard": true, "changes": changes, "findings": [], "risk": "none", "visibility": null, "redacted": [],
            }));
        }
        if body.gate.token.as_deref() != Some(token.as_str()) {
            return Err(Fail::new(409, "черновик изменился с тех пор, как вы смотрели: посмотрите ещё раз"));
        }
        memory::Drafts::new(&cfg, &repo).discard(&id);
        Ok(json!({"ok": true}))
    })
    .await
    .map_err(|e| Fail::new(500, e.to_string()))
    .and_then(|r| r);
    respond(result)
}

#[derive(Deserialize)]
pub struct SendBody {
    repo: String,
    #[serde(flatten)]
    gate: Gate,
}

/// What goes to the repository with a send: the drafts, as files (`add`, `change`) and removals (`delete`). Nothing is
/// written; the checkout is not even looked at (a file the human edited there by hand is theirs).
fn send_plan(cfg: &crate::config::AppConfig, repo: &str) -> Result<(String, Vec<Change>), Fail> {
    let root = memory::repo_root(cfg, repo).ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
    memory::git_dirty(&root).ok_or_else(|| Fail::new(422, "это не git-репозиторий"))?;
    let drafts = memory::Drafts::new(cfg, repo);
    let mut changes: Vec<Change> = Vec::new();
    let rel = |id: &str| format!("{}/{id}.md", memory::UNIVERSE_DIR);
    for e in drafts.puts() {
        let before = memory::git_head_text(&root, &rel(&e.id));
        changes.push(Change {
            op: if before.is_some() { "change" } else { "add" },
            id: e.id.clone(),
            file: rel(&e.id),
            before,
            after: Some(memory::render_entry(&e)),
            entry: Some(e),
        });
    }
    for id in drafts.dels() {
        let before = memory::git_head_text(&root, &rel(&id));
        changes.push(Change { op: "delete", id: id.clone(), file: rel(&id), before, after: None, entry: None });
    }
    if changes.is_empty() {
        return Err(Fail::new(422, "нечего отправлять: черновиков нет"));
    }
    Ok((root, changes))
}

/// What the check finds in the files that go to the repository, as a whole (header and body).
fn scan_files(changes: &[Change], own: &[String]) -> Vec<FindingOut> {
    let mut findings = Vec::new();
    for c in changes {
        if let Some(after) = &c.after {
            for f in memory_guard::scan(after, "text", own) {
                // the summary is cut from the body when the human wrote none: one hit, listed once
                if !findings.iter().any(|o: &FindingOut| o.file == c.file && o.f.kind == f.kind && o.f.text == f.text) {
                    findings.push(FindingOut { file: c.file.clone(), f });
                }
            }
        }
    }
    findings
}

/// A Markdown fence that the text cannot close early.
fn fenced(text: &str, lang: &str) -> String {
    let mut run = 0;
    let mut longest = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}{lang}\n{}\n{fence}", text.trim_end_matches('\n'))
}

/// The board task for a send: its title, and a description with the texts of the files and the lines of `MEMORY.md` as
/// they are to be written, so the agent has nothing to invent.
fn task_text(repo: &str, changes: &[Change]) -> (String, String) {
    let titles: Vec<String> = changes
        .iter()
        .filter_map(|c| c.entry.as_ref().map(|e| e.title.clone()).or_else(|| c.before.as_deref().map(|b| memory::parse_entry(&c.id, b).title)))
        .collect();
    let title = format!("Запомнить в docs/memory: {}", memory::cut(&titles.join("; "), 100));
    let index = format!("{}/{}", memory::UNIVERSE_DIR, memory::INDEX);
    let mut d = format!(
        "Записать в память вселенной «{repo}» (папка `{}/`) то, что человек подтвердил в планировщике («было → стало»). \
Тексты готовы: ничего не придумывай и не переписывай, только положи их в репозиторий.\n\n## 1. Файлы\n",
        memory::UNIVERSE_DIR
    );
    for c in changes {
        match (c.op, &c.after) {
            ("delete", _) | (_, None) => d.push_str(&format!("\n### Удалить `{}`\n", c.file)),
            (op, Some(after)) => {
                let what = if op == "add" { "новый файл" } else { "заменить файл целиком" };
                d.push_str(&format!("\n### `{}` — {what}\n\n{}\n", c.file, fenced(after, "markdown")));
            }
        }
    }
    d.push_str(&format!(
        "\n## 2. Оглавление `{index}`\nПо одной строке на запись, по порядку id. Нет файла — создай его с заголовком `# Память вселенной {repo}` и этими строками.\n"
    ));
    for c in changes {
        match (c.op, &c.entry) {
            ("add", Some(e)) => d.push_str(&format!("\nДобавить строку:\n\n{}\n", fenced(&memory::index_line(e), ""))),
            ("change", Some(e)) => {
                d.push_str(&format!("\nЗаменить строку со ссылкой `({}.md)`:\n\n{}\n", c.id, fenced(&memory::index_line(e), "")))
            }
            _ => d.push_str(&format!("\nУбрать строку со ссылкой `({}.md)`.\n", c.id.replace(' ', "%20"))),
        }
    }
    d.push_str("\n## 3. Дальше\nБольше в репозитории ничего не меняй. Коммит и слияние — как у любой задачи этого репозитория.\n");
    (title, d)
}

/// The most that goes into one task description: it travels as one command-line argument.
const MAX_TASK_TEXT: usize = 100_000;

/// `POST /api/memory/send`: hands a universe's drafts to a board task («Запомнить в docs/memory: …»). The task has its own
/// working copy and is merged like any other: the planner never writes or commits into the human's checkout. The same
/// preview, check and confirmation as a save: the text goes into git through the task, so it is looked at once more.
pub async fn memory_send(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<SendBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        if !taskmeta::valid_repo(&body.repo) {
            return Err(Fail::new(400, "bad universe name"));
        }
        let (cfg, repo) = (app.cfg.clone(), body.repo.clone());
        let (root, changes) = tokio::task::spawn_blocking(move || send_plan(&cfg, &repo))
            .await
            .map_err(|e| Fail::new(500, e.to_string()))??;
        // what goes into git is checked as a whole, header and body
        let findings = scan_files(&changes, &guard_terms(&app.cfg));
        let vis = if findings.is_empty() { None } else { Some(visibility_of(&app, &root).await) };
        let token = token_of(&changes, &findings);
        let risk = risk_of(&findings, vis);
        if body.gate.confirm != Some(true) {
            return Ok(json!({
                "ok": true, "preview": true, "token": token, "level": "universe", "repo": body.repo, "send": true,
                "changes": changes, "findings": findings, "risk": risk, "visibility": vis, "redacted": [],
            }));
        }
        if body.gate.token.as_deref() != Some(token.as_str()) {
            return Err(Fail::new(409, "память изменилась с тех пор, как вы смотрели список: посмотрите ещё раз"));
        }
        risk_allowed(risk, &body.gate)?;
        let (title, desc) = task_text(&body.repo, &changes);
        if desc.len() > MAX_TASK_TEXT {
            return Err(Fail::new(422, "черновиков слишком много для одной задачи: отправьте часть, остальное потом"));
        }
        let out = app::run_board(&app.cfg, &["add", &title, "--repo", &body.repo, "--desc", &desc], Duration::from_secs(60))
            .await
            .map_err(|e| Fail::new(502, e))?;
        // The board says `<repo>/<n>: added…` (the fixture's older words: `PLN-35: added`): the id, and the tag from the board.
        let said = out.lines().find_map(|l| l.split_once(':').map(|(t, _)| t.trim().to_string())).unwrap_or_default();
        let id = if said.contains('/') {
            Some(said.clone())
        } else {
            said.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok()).map(|n| format!("{}/{n}", body.repo))
        };
        app::refresh(&app).await;
        let tag = match &id {
            Some(id) => app.snapshot.read().await.tasks.iter().find(|t| &t.id == id).map(|t| t.tag.clone()).filter(|t| !t.is_empty()),
            None => None,
        }
        .unwrap_or_else(|| said.clone());
        // Only what this send took leaves the drafts (one saved meanwhile stays), and the panel keeps it, with the task,
        // until the checkout has it.
        let files: Vec<(String, Option<String>)> = changes.iter().map(|c| (c.id.clone(), c.after.clone())).collect();
        memory::Drafts::new(&app.cfg, &body.repo).drop_sent(&files);
        let batch = memory::SentBatch { task: tag.clone(), id: id.clone(), title: title.clone(), at: now_ms(), files };
        let _ = memory::SentLog::new(&app.cfg, &body.repo).add(&batch);
        Ok(json!({"ok": true, "task": tag, "id": id, "title": title, "files": changes.len()}))
    }
    .await;
    respond(result)
}

#[derive(Deserialize)]
pub struct ForgetBody {
    repo: String,
    task: String,
}

/// `POST /api/memory/sent/forget`: drops the panel's note of a send (the task was dropped, or the memory came another way).
/// It only forgets what the planner kept; no memory file is touched, so there is no card.
pub async fn memory_sent_forget(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<ForgetBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if !taskmeta::valid_repo(&body.repo) {
        return error(StatusCode::BAD_REQUEST, "bad universe name");
    }
    let gone = memory::SentLog::new(&app.cfg, &body.repo).forget(&body.task);
    Json(json!({"ok": true, "forgotten": gone})).into_response()
}

#[derive(Deserialize)]
pub struct SettingsBody {
    suggest: bool,
}

/// `POST /api/memory/settings`: whether the model proposes memory lines when a task is done.
pub async fn memory_settings(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<SettingsBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    app.db.meta_set(SUGGEST_KEY, if body.suggest { "on" } else { "off" });
    Json(json!({"ok": true, "suggest": body.suggest})).into_response()
}

#[derive(Deserialize)]
pub struct ConnectBody {
    on: bool,
    /// The instructions the human saw on the card. Absent: only a preview.
    expect: Option<String>,
    confirm: Option<bool>,
}

/// The instructions of the board as they are now (empty when the setting is not set).
async fn current_instructions(app: &Shared) -> Result<String, Fail> {
    let settings = station::read_settings(&app.cfg, "").await.map_err(Fail::refused)?;
    Ok(station::find(&settings, INSTRUCTIONS)
        .filter(|s| s.from != "built in" && s.value.trim() != "none")
        .map(|s| s.value.clone())
        .unwrap_or_default())
}

/// `POST /api/memory/connect`: puts the memory block into the board's `coordinator.instructions`, or takes it out.
/// Without `confirm` it only says what would change («было → стало»). With `confirm` and `expect` (what the card
/// showed) it reads the instructions again first: if they are not the ones the human saw — an edit in the Station, the
/// CLI or another tab — nothing is sent (409), so a newer text is never overwritten; otherwise the board's own `set`
/// (or `unset`) is run with the value built from those fresh instructions. The human's own text is kept as it is.
pub async fn memory_connect(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<ConnectBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        let block = memory::coordinator_block(&memory::galaxy_dir(&app.cfg).display().to_string());
        let current = current_instructions(&app).await?;
        let value = memory::connect_value(&current, &block, body.on).map_err(|e| Fail::new(422, e))?;
        if body.confirm != Some(true) {
            return Ok(json!({"ok": true, "name": INSTRUCTIONS, "current": current, "now": value}));
        }
        let Some(expect) = body.expect.as_deref() else {
            return Err(Fail::new(400, "нужно показать «было → стало»: передайте expect"));
        };
        if expect != current {
            return Err(Fail::new(
                409,
                "Инструкции Координатора изменились после того, как вы открыли карточку. Ничего не отправлено: откройте карточку заново.",
            ));
        }
        let args: Vec<String> = match &value {
            Some(v) => vec!["set".into(), INSTRUCTIONS.into(), v.clone()],
            None => vec!["unset".into(), INSTRUCTIONS.into()],
        };
        let out = station::board_run(&app.cfg, &args, station::BOARD_WAIT).await;
        if !out.ok() {
            return Err(Fail::new(422, out.words()));
        }
        app::refresh(&app).await;
        Ok(json!({"ok": true, "name": INSTRUCTIONS, "was": current, "now": value}))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

/* ───────── suggestions after a task is done ───────── */

#[derive(Debug, Clone, Serialize)]
pub struct SuggestOut {
    pub task: String,
    pub repo: String,
    pub tag: String,
    pub title: String,
    pub lines: Vec<Line>,
    pub at: u64,
}

pub(crate) fn suggest_out(row: &crate::db::SuggestRow, title: String) -> SuggestOut {
    SuggestOut {
        task: row.task.clone(),
        repo: row.repo.clone(),
        tag: row.tag.clone(),
        title,
        lines: serde_json::from_str(&row.lines).unwrap_or_default(),
        at: row.at,
    }
}

async fn title_of(app: &Shared, task: &str) -> String {
    let s = app.snapshot.read().await;
    s.tasks
        .iter()
        .chain(s.archived.iter())
        .find(|t| t.id == task)
        .map(|t| t.title.clone())
        .unwrap_or_default()
}

/// The suggestions waiting for the human; with `repo`, only that universe's.
pub async fn suggestions_for(app: &Shared, repo: &str) -> Vec<SuggestOut> {
    let db = app.db.clone();
    let rows = tokio::task::spawn_blocking(move || db.suggest_pending()).await.unwrap_or_default();
    let mut out = Vec::new();
    for r in rows.iter().filter(|r| repo.is_empty() || r.repo == repo) {
        out.push(suggest_out(r, title_of(app, &r.task).await));
    }
    out
}

/// Puts the waiting suggestions into the snapshot and tells the page.
pub async fn sync_snapshot(app: &Shared) {
    let all = suggestions_for(app, "").await;
    app.snapshot.write().await.memory_suggestions = all;
    app::publish(app).await;
}

/// A task became done: ask the model once for one or two lines worth keeping in its universe's memory. The human
/// decides; nothing is written here.
pub async fn suggest_for(app: &Shared, t: &Task) {
    if !suggest_enabled(app) || memory::repo_root(&app.cfg, &t.repo).is_none() {
        return;
    }
    let (db, id, repo, tag) = (app.db.clone(), t.id.clone(), t.repo.clone(), t.tag.clone());
    let claimed = tokio::task::spawn_blocking(move || db.suggest_claim(&id, &repo, &tag, now_ms()))
        .await
        .unwrap_or(false);
    if !claimed {
        return;
    }
    // The board may write the closing note a moment after the task is done: look three times.
    let mut closing = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
        let (cfg, repo, n) = (app.cfg.clone(), t.repo.clone(), t.n);
        closing = tokio::task::spawn_blocking(move || taskmeta::read_closing(&cfg.data_dir, &repo, n, 8000))
            .await
            .ok()
            .flatten()
            .map(|c| c.text)
            .unwrap_or_default();
        if !closing.trim().is_empty() {
            break;
        }
    }
    if closing.trim().is_empty() {
        app.db.suggest_finish(&t.id, "none", "[]");
        return;
    }
    let input = format!("Задача: {} {}\n\nИтог:\n{}", t.tag, t.title, closing);
    match translate::ask(&app.cfg, memory::SUGGEST_PROMPT, &input).await {
        Ok(reply) => {
            let lines = memory::parse_suggestion(&reply);
            if lines.is_empty() {
                app.db.suggest_finish(&t.id, "none", "[]");
            } else {
                app.db
                    .suggest_finish(&t.id, "pending", &serde_json::to_string(&lines).unwrap_or_else(|_| "[]".into()));
                sync_snapshot(app).await;
            }
        }
        Err(e) => {
            eprintln!("herdr-planner: memory suggestion for {}: {e}", t.id);
            app.db.suggest_finish(&t.id, "failed", "[]");
        }
    }
}

#[derive(Deserialize)]
pub struct ResolveBody {
    task: String,
    /// `accept` or `dismiss`.
    action: String,
    /// The lines as the human left them (edited, some removed); none sent: the suggested ones. Sent and all blank:
    /// nothing to keep.
    #[serde(default)]
    lines: Option<Vec<Line>>,
}

#[derive(Deserialize)]
pub struct ResolveReq {
    #[serde(flatten)]
    body: ResolveBody,
    #[serde(flatten)]
    gate: Gate,
}

/// `POST /api/memory/suggestion`: keeps the suggested lines in the universe's memory, or drops them. Keeping is a
/// change like any other: without `confirm` the answer is «было → стало» and the check; with it the lines go to the
/// universe's drafts (or its files, in a folder that is not in git), all or none.
pub async fn memory_suggestion(State(app): State<Shared>, headers: HeaderMap, Json(req): Json<ResolveReq>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let (body, gate) = (req.body, req.gate);
    let result = async {
        let row = app
            .db
            .suggest_get(&body.task)
            .filter(|r| r.state == "pending")
            .ok_or_else(|| Fail::new(404, "такого предложения нет"))?;
        match body.action.as_str() {
            "dismiss" => {
                app.db.suggest_set_state(&row.task, "dismissed");
                sync_snapshot(&app).await;
                Ok(json!({"ok": true, "saved": []}))
            }
            "accept" => {
                let lines: Vec<Line> = body
                    .lines
                    .clone()
                    .unwrap_or_else(|| serde_json::from_str(&row.lines).unwrap_or_default())
                    .into_iter()
                    .filter(|l| !l.text.trim().is_empty())
                    .collect();
                if lines.is_empty() {
                    return Err(Fail::new(422, "нет строк для записи: «Не нужно» отклонит предложение"));
                }
                let req = Req::Lines { repo: row.repo.clone(), task: row.task.clone(), lines };
                if gate.confirm != Some(true) {
                    return run(&app, req, &gate).await;
                }
                // One accept at a time: a second, concurrent one finds the suggestion taken.
                if !app.db.suggest_take(&row.task) {
                    return Err(Fail::new(409, "это предложение уже обрабатывается"));
                }
                match run(&app, req, &gate).await {
                    Ok(v) => {
                        app.db.suggest_set_state(&row.task, "done");
                        sync_snapshot(&app).await;
                        Ok(v)
                    }
                    Err(f) => {
                        app.db.suggest_set_state(&row.task, "pending");
                        Err(f)
                    }
                }
            }
            _ => Err(Fail::new(400, "accept или dismiss")),
        }
    }
    .await;
    respond(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_with(body: &str, vis: Option<Visibility>) -> Plan {
        let mut e = memory::complete(Entry { title: "Т".into(), body: body.into(), ..Entry::default() }, "decision", 0).unwrap();
        e.id = "t".into();
        let changes = vec![change_of("t", "docs/memory/t.md".into(), None, Some(e))];
        let (findings, redacted) = check_entries(&changes, &memory_guard::own_terms());
        Plan { repo: "demo".into(), changes, findings, visibility: vis, draft: true, redacted }
    }

    fn gate(token: &str, risk: bool, public: bool) -> Gate {
        Gate { confirm: Some(true), token: Some(token.into()), accept_risk: Some(risk).filter(|r| *r), accept_public: Some(public).filter(|p| *p) }
    }

    fn add_change(id: &str, title: &str, body: &str) -> Change {
        let mut e = memory::complete(Entry { title: title.into(), body: body.into(), ..Entry::default() }, "convention", 0).unwrap();
        e.id = id.into();
        change_of(id, format!("docs/memory/{id}.md"), None, Some(e))
    }

    #[test]
    fn a_send_makes_a_task_with_the_ready_files_and_index_lines() {
        let before = Entry { id: "old".into(), title: "Старое".into(), body: "было".into(), kind: "pitfall".into(), ..Entry::default() };
        let gone = change_of("gone", "docs/memory/gone.md".into(), Some(&Entry { id: "gone".into(), title: "Лишнее".into(), ..Entry::default() }), None);
        let mut edited = add_change("old", "Старое", "стало ``` вот так");
        edited.before = Some(memory::render_entry(&before));
        edited.op = "change";
        let changes = vec![add_change("squash", "Слияние squash", "Сливаем squash."), edited, gone];
        let (title, desc) = task_text("demo", &changes);
        assert!(title.starts_with("Запомнить в docs/memory: Слияние squash; Старое"), "{title}");
        assert!(desc.contains("### `docs/memory/squash.md` — новый файл") && desc.contains("Сливаем squash."), "{desc}");
        assert!(desc.contains("type: convention") && desc.contains("name: squash"), "the whole file is there, header included: {desc}");
        assert!(desc.contains("### `docs/memory/old.md` — заменить файл целиком"), "{desc}");
        assert!(desc.contains("````markdown"), "a fence longer than the backticks in the text: {desc}");
        assert!(desc.contains("### Удалить `docs/memory/gone.md`"), "{desc}");
        assert!(desc.contains("docs/memory/MEMORY.md") && desc.contains("- [Слияние squash](squash.md)"), "the index lines: {desc}");
        assert!(desc.contains("Убрать строку со ссылкой `(gone.md)`") && desc.contains("Заменить строку со ссылкой `(old.md)`"), "{desc}");
        assert!(desc.contains("# Память вселенной demo"), "{desc}");
    }

    #[test]
    fn a_send_is_checked_for_machine_names_and_a_public_repository_forbids_it() {
        let changes = vec![add_change("stand", "Стенд", "Стенд поднят на app-semantics, ключ на salehost.")];
        let own: Vec<String> = ["host:app-semantics", "host:salehost", "host:dokploymain"].iter().map(|s| s.to_string()).collect();
        let found = scan_files(&changes, &own);
        let hosts: Vec<&str> = found.iter().filter(|f| f.f.kind == "host").map(|f| f.f.text.as_str()).collect();
        assert_eq!(hosts, ["app-semantics", "salehost"], "{found:?}");
        assert!(scan_files(&changes, &[]).is_empty(), "without the machine names in the list it is a plain word");
        // no word of the human: not for a public or unknown repository, an ordinary «да» only for a private one
        for vis in [Some(Visibility::Public), Some(Visibility::Unknown), None] {
            let risk = risk_of(&found, vis);
            assert_eq!(risk, "block", "{vis:?}");
            assert_eq!(format!("{:?}", risk_allowed(risk, &Gate::default()).unwrap_err()).contains("403"), true);
            assert!(risk_allowed(risk, &gate("t", true, false)).is_err(), "the ordinary «да» is not enough for {vis:?}");
            assert!(risk_allowed(risk, &gate("t", false, true)).is_ok());
        }
        let private = risk_of(&found, Some(Visibility::Private));
        assert_eq!(private, "confirm");
        assert!(risk_allowed(private, &Gate::default()).is_err() && risk_allowed(private, &gate("t", true, false)).is_ok());
        assert!(risk_allowed(risk_of(&[], Some(Visibility::Public)), &Gate::default()).is_ok(), "clean text needs no word");
    }

    #[test]
    fn the_names_of_the_boards_runners_are_looked_for_too() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::testutil::test_cfg(dir.path());
        std::fs::create_dir_all(cfg.runners_file().parent().unwrap()).unwrap();
        std::fs::write(
            cfg.runners_file(),
            serde_json::json!({"version": 1, "runners": [{"id": "vm-r", "target": "root@yard-host.lan:2222"}, {"id": "ip-one", "target": "10.9.8.7"}]}).to_string(),
        )
        .unwrap();
        let terms = guard_terms(&cfg);
        for want in ["host:vm-r", "host:yard-host.lan", "host:ip-one"] {
            assert!(terms.contains(&want.to_string()), "{want} in {terms:?}");
        }
        assert!(!terms.iter().any(|t| t.contains("10.9.8.7")), "an address is found as an address: {terms:?}");
        let changes = vec![add_change("yard", "Верфь", "Контейнеры гоняем на верфи vm-r.")];
        let found = scan_files(&changes, &terms);
        assert!(found.iter().any(|f| f.f.kind == "host" && f.f.text == "vm-r"), "{found:?}");
        assert_eq!(risk_of(&found, Some(Visibility::Public)), "block");
        // no runners file: nothing more to look for
        std::fs::remove_file(cfg.runners_file()).unwrap();
        assert!(!guard_terms(&cfg).contains(&"host:vm-r".to_string()));
    }

    #[test]
    fn a_send_does_not_look_at_or_write_into_the_checkout() {
        let (proj, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = proj.path().to_str().unwrap().to_string();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false", "-C", &root])
                .args(args)
                .output()
                .unwrap();
            assert!(o.status.success(), "git {args:?}");
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        git(&["init", "--quiet", "-b", "main"]);
        std::fs::write(proj.path().join("a.txt"), "a").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "--quiet", "-m", "first"]);
        let mut cfg = crate::testutil::test_cfg(state.path());
        cfg.data_dir = state.path().join("data");
        std::fs::create_dir_all(&cfg.data_dir).unwrap();
        std::fs::write(cfg.data_dir.join("repos.json"), serde_json::json!({"demo": root}).to_string()).unwrap();
        let drafts = memory::Drafts::new(&cfg, "demo");
        assert!(send_plan(&cfg, "demo").is_err(), "nothing drafted, nothing to send");
        let mut e = memory::complete(Entry { title: "Раз".into(), body: "текст".into(), ..Entry::default() }, "decision", 0).unwrap();
        e.id = "raz".into();
        drafts.put(&e).unwrap();
        // a file the human edited by hand in the checkout is theirs: not a part of the send
        std::fs::create_dir_all(proj.path().join("docs/memory")).unwrap();
        std::fs::write(proj.path().join("docs/memory/mine.md"), "моё\n").unwrap();
        let status = git(&["status", "--porcelain", "--untracked-files=all"]);
        let head = git(&["rev-parse", "HEAD"]);
        let (_, changes) = send_plan(&cfg, "demo").unwrap();
        assert_eq!(changes.iter().map(|c| c.file.as_str()).collect::<Vec<_>>(), ["docs/memory/raz.md"]);
        assert_eq!(git(&["status", "--porcelain", "--untracked-files=all"]), status, "the checkout is as it was");
        assert_eq!(git(&["rev-parse", "HEAD"]), head, "and has no new commit");
        assert!(!proj.path().join("docs/memory/raz.md").exists());
    }

    #[test]
    fn a_change_shows_was_and_now_with_the_level_and_file() {
        let before = Entry { id: "t".into(), title: "Т".into(), body: "старое".into(), kind: "decision".into(), ..Entry::default() };
        let after = Entry { body: "новое".into(), ..before.clone() };
        let c = change_of("t", "docs/memory/t.md".into(), Some(&before), Some(after));
        assert_eq!(c.op, "change");
        assert!(c.before.as_deref().unwrap().contains("старое") && c.after.as_deref().unwrap().contains("новое"));
        assert_eq!(change_of("t", "f".into(), None, Some(before.clone())).op, "add");
        let d = change_of("t", "f".into(), Some(&before), None);
        assert_eq!((d.op, d.after.is_none()), ("delete", true));
    }

    #[test]
    fn clean_text_needs_only_the_token_of_the_preview() {
        let p = plan_with("Решили сливать squash.", None);
        assert_eq!(p.risk(), "none");
        assert!(allowed(&p, &gate(&p.token(), false, false)).is_ok());
        let err = allowed(&p, &gate("stale", false, false)).unwrap_err();
        assert!(format!("{err:?}").len() > 0);
        let other = plan_with("Решили сливать merge.", None);
        assert_ne!(p.token(), other.token(), "another text is another token: what is written is what was shown");
        assert!(allowed(&p, &Gate::default()).is_err(), "no token, no write");
    }

    #[test]
    fn personal_data_in_a_private_repository_needs_an_explicit_yes() {
        let p = plan_with("Сервер 203.0.113.7 за VPN", Some(Visibility::Private));
        assert_eq!(p.risk(), "confirm");
        assert!(p.findings.iter().any(|f| f.f.kind == "ip"));
        assert!(p.redacted[0]["body"].as_str().unwrap_or("").contains("<ip>"), "an anonymised text is offered");
        assert!(allowed(&p, &gate(&p.token(), false, false)).is_err(), "without the «да» nothing is written");
        assert!(allowed(&p, &gate(&p.token(), true, false)).is_ok());
        let local = plan_with("Сервер 203.0.113.7", Some(Visibility::Local));
        assert_eq!(local.risk(), "confirm", "a repository only on this machine is not public");
    }

    #[test]
    fn in_a_public_or_unknown_repository_findings_are_forbidden_by_default() {
        for vis in [Some(Visibility::Public), Some(Visibility::Unknown), None] {
            let p = plan_with("пишите ivan@corp-mail.example.org", vis);
            assert_eq!(p.risk(), "block", "{vis:?}");
            assert!(allowed(&p, &gate(&p.token(), true, false)).is_err(), "the ordinary «да» is not enough: {vis:?}");
            assert!(allowed(&p, &gate(&p.token(), false, true)).is_ok(), "only the explicit word for a public repository");
        }
        // clean text is no business of the check
        assert_eq!(plan_with("Решили.", Some(Visibility::Public)).risk(), "none");
    }

    #[test]
    fn the_check_looks_at_title_description_and_body_and_names_the_file() {
        let mut e = Entry { id: "t".into(), title: "Хост db01.office.local".into(), description: "см. /home/ivan/x".into(), body: "token=abc123XYZ789".into(), kind: "decision".into(), ..Entry::default() };
        e.added = "2026-10-21".into();
        let (f, r) = check_entries(&[change_of("t", "docs/memory/t.md".into(), None, Some(e))], &[]);
        let got: Vec<(&str, &str)> = f.iter().map(|x| (x.f.field.as_str(), x.f.kind)).collect();
        assert_eq!(got, [("title", "host"), ("description", "path"), ("body", "secret")]);
        assert!(f.iter().all(|x| x.file == "docs/memory/t.md"));
        assert!(f[2].f.text.ends_with('…'), "a secret is not echoed");
        assert!(!r[0].is_null());
    }
}
