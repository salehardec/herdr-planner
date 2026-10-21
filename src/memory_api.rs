//! The memory panel's endpoints, the Coordinator's instructions, the far machines' memory and the suggestions made
//! when a task is done. The files and their format are in `memory`.
//!
//! * A far galaxy's memory is read and written over the board's ssh (a login shell, plain POSIX `sh` scripts); its
//!   universes' memory is only read — it lives in git, and a far checkout is not the planner's to change.
//! * The Coordinator is told about the memory through the board's own setting `coordinator.instructions`: the planner
//!   shows the human the instructions they have and the same with a memory block added at the end (their text kept),
//!   and on their confirmation the board's `set` makes the change (the Station's endpoints). The planner writes no
//!   file of the board (its `AGENTS.md`, `CLAUDE.md`, `rules.md`): the board makes them from the setting and may
//!   rewrite them at any time.

use crate::app::{self, Shared};
use crate::board::Task;
use crate::config::now_ms;
use crate::machines::{self, MachineRef};
use crate::memory::{self, Entry, Line};
use crate::memory_guard::{self, Finding};
use crate::server::{clip, error, same_origin};
use crate::station::{self, galaxy_of, place_of, Fail, Place};
use crate::{taskmeta, translate};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

const FAR_WAIT: Duration = Duration::from_secs(25);
/// The Galaxy memory on a far machine (the same place as at home, under that machine's home folder).
pub const FAR_GALAXY_DIR: &str = "~/.herdr-projects/memory/galaxy";
const SUGGEST_KEY: &str = "memory.suggest";

/// The memory of the far galaxies as the planner last read it (for the search; the panel reads fresh).
#[derive(Debug, Clone, Default)]
pub struct FarMemory {
    pub galaxy: Vec<Entry>,
    pub universes: HashMap<String, Vec<Entry>>,
}

/* ───────── scripts for a far machine ───────── */

/// A path for the far shell: `~/x` is under that machine's `$HOME`.
pub fn sh_path(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"{}", machines::sh_quote(&format!("/{rest}"))),
        None => machines::sh_quote(p),
    }
}

/// One line of `sh` that prints the `*.md` files of the folders: `\036DIR <n>` then, per file, `\037FILE <name>` and
/// its first 64 KB.
pub fn read_script(dirs: &[String]) -> String {
    let list: Vec<String> = dirs.iter().map(|d| sh_path(d)).collect();
    format!(
        r#"HP_MEMORY=1; i=0; for d in {}; do i=$((i+1)); printf '\036DIR %s\n' "$i"; [ -d "$d" ] || continue; for f in "$d"/*.md; do [ -f "$f" ] || continue; printf '\037FILE %s\n' "${{f##*/}}"; head -c 65536 "$f"; printf '\n'; done; done"#,
        list.join(" ")
    )
}

/// `read_script`'s output: for each folder (in the order asked) its entries.
pub fn parse_read(stdout: &str, n: usize) -> Vec<Vec<Entry>> {
    let mut out = vec![Vec::new(); n];
    for chunk in stdout.split('\u{1e}').skip(1) {
        let (head, rest) = chunk.split_once('\n').unwrap_or((chunk, ""));
        let Some(idx) = head
            .strip_prefix("DIR ")
            .and_then(|i| i.trim().parse::<usize>().ok())
            .filter(|i| (1..=n).contains(i))
        else {
            continue;
        };
        for file in rest.split('\u{1f}').skip(1) {
            let (h, content) = file.split_once('\n').unwrap_or((file, ""));
            let Some(name) = h.strip_prefix("FILE ").map(str::trim) else { continue };
            let Some(stem) = name.strip_suffix(".md") else { continue };
            if name.eq_ignore_ascii_case(memory::INDEX) || name.eq_ignore_ascii_case("README.md") || stem.is_empty() {
                continue;
            }
            let content = content.strip_suffix('\n').unwrap_or(content);
            out[idx - 1].push(memory::parse_entry(stem, content));
        }
        out[idx - 1].sort_by(|a, b| a.id.cmp(&b.id));
    }
    out
}

/// One line of `sh` that writes files into a folder (each by a temporary name, then moved) and removes others.
pub fn write_script(dir: &str, writes: &[(String, String)], removes: &[String]) -> String {
    let mut s = format!(
        "HP_MEMORY=1; b() {{ base64 -d 2>/dev/null || base64 -D; }}; d={}; mkdir -p \"$d\"",
        sh_path(dir)
    );
    for (name, text) in writes {
        let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let n = machines::sh_quote(name);
        let tmp = machines::sh_quote(&format!("{name}.new"));
        s.push_str(&format!(
            " && printf %s '{b64}' | b > \"$d\"/{tmp} && mv \"$d\"/{tmp} \"$d\"/{n}"
        ));
    }
    for name in removes {
        s.push_str(&format!(" && rm -f \"$d\"/{}", machines::sh_quote(name)));
    }
    s
}

/// Whether the model proposes memory lines when a task is done (on until the human switches it off).
pub fn suggest_enabled(app: &Shared) -> bool {
    app.db.meta_get(SUGGEST_KEY).as_deref() != Some("off")
}

/// The board's setting that carries the human's own instructions for its Coordinator.
pub const INSTRUCTIONS: &str = "coordinator.instructions";

/// What the panel shows about the memory block in `coordinator.instructions` of a galaxy's board. Nothing is changed
/// here: `memory_connect` previews the change and, on the human's confirmation, makes it with the board's own `set`.
/// Files the board writes (its `AGENTS.md`, `rules.md`) are not touched.
async fn connect_state(app: &Shared, place: &Place, galaxy_dir: &str) -> Value {
    let block = memory::coordinator_block(galaxy_dir);
    match station::read_settings(&app.cfg, place, "").await {
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
        // It is rebuilt when the memory is committed.
    })
    .await;
}

/* ───────── ssh ───────── */

async fn ssh_run(app: &Shared, m: &MachineRef, script: &str) -> Result<String, Fail> {
    let end = machines::ssh_ended(&app.cfg, m, script, FAR_WAIT)
        .await
        .map_err(|e| Fail::new(502, e))?;
    if end.timed_out || !end.error.is_empty() || end.code == Some(255) {
        return Err(Fail::unreachable(machines::why_not(&end, m)));
    }
    if end.code != Some(0) {
        let words = if end.stderr.trim().is_empty() { end.stdout.trim() } else { end.stderr.trim() };
        return Err(Fail::new(502, clip(format!("на машине {}: {words}", m.label))));
    }
    Ok(end.stdout)
}

async fn far_entries(app: &Shared, m: &MachineRef, dir: &str) -> Result<Vec<Entry>, Fail> {
    let out = ssh_run(app, m, &read_script(&[dir.to_string()])).await?;
    Ok(parse_read(&out, 1).remove(0))
}

/// Reads a far galaxy's memory (its Galaxy folder and every universe's `docs/memory`) for the search.
pub async fn refresh_far(app: &Shared, label: &str) {
    let Some(g) = galaxy_of(app, label).await else { return };
    if !g.ok {
        return;
    }
    let Ok(Place::Far(m)) = place_of(label).await else { return };
    let mut dirs = vec![FAR_GALAXY_DIR.to_string()];
    let names: Vec<&str> = g.projects.iter().filter(|p| !p.folder.is_empty()).map(|p| p.name.as_str()).collect();
    for n in &names {
        let folder = g.projects.iter().find(|p| p.name == *n).map(|p| p.folder.as_str()).unwrap_or("");
        dirs.push(format!("{}/{}", folder.trim_end_matches('/'), memory::UNIVERSE_DIR));
    }
    let Ok(out) = ssh_run(app, &m, &read_script(&dirs)).await else { return };
    let mut got = parse_read(&out, dirs.len());
    let galaxy = got.remove(0);
    let universes = names.iter().map(|n| n.to_string()).zip(got).collect();
    app.far_memory.lock().await.insert(label.to_string(), FarMemory { galaxy, universes });
}

/// All the connected far galaxies, one after the other.
pub async fn refresh_all_far(app: &Shared) {
    let labels: Vec<String> = app.galaxies.lock().await.iter().filter(|g| g.ok).map(|g| g.label.clone()).collect();
    for l in &labels {
        refresh_far(app, l).await;
    }
    app.far_memory.lock().await.retain(|k, _| labels.contains(k));
    app::index_snapshot(app).await;
}

/* ───────── the panel ───────── */

#[derive(Deserialize)]
pub struct GetQuery {
    galaxy: Option<String>,
    repo: Option<String>,
}

fn entry_json(e: &Entry) -> Value {
    serde_json::to_value(e).unwrap_or(Value::Null)
}

fn bad_scope(galaxy: &str, repo: &str) -> Option<Fail> {
    if !repo.is_empty() && !taskmeta::valid_repo(repo) {
        return Some(Fail::new(400, "bad universe name"));
    }
    if !galaxy.is_empty() && !machines::valid_label(galaxy) {
        return Some(Fail::new(400, "bad machine label"));
    }
    None
}

async fn panel_read(app: &Shared, q: GetQuery) -> Result<Value, Fail> {
    let galaxy = q.galaxy.unwrap_or_default();
    let repo = q.repo.unwrap_or_default();
    if let Some(f) = bad_scope(&galaxy, &repo) {
        return Err(f);
    }
    let place = place_of(&galaxy).await?;
    let level = if repo.is_empty() { "galaxy" } else { "universe" };
    match place {
        Place::Home => {
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
                    let entries: Vec<Value> = match &drafts {
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
                    let git = match &dirty {
                        None => json!({"checkout": false, "dirty": [], "drafts": 0}),
                        Some(d) => json!({
                            "checkout": true,
                            "dirty": d.iter().map(|(s, f)| json!({"state": s, "file": f})).collect::<Vec<_>>(),
                            "drafts": drafts.as_ref().map(|d| d.puts().len() + d.dels().len()).unwrap_or(0),
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
                v["connect"] = connect_state(app, &Place::Home, &memory::galaxy_dir(&app.cfg).display().to_string()).await;
                v["settings"] = json!({"suggest": suggest_enabled(app)});
            } else {
                v["suggestions"] = json!(suggestions_for(app, &repo).await);
            }
            v["galaxy"] = json!(galaxy);
            v["repo"] = json!(repo);
            v["level"] = json!(level);
            v["ok"] = json!(true);
            Ok(v)
        }
        Place::Far(m) => {
            let (dir, writable) = if repo.is_empty() {
                (FAR_GALAXY_DIR.to_string(), true)
            } else {
                let g = galaxy_of(app, &galaxy).await.ok_or_else(|| Fail::new(404, "такой галактики нет"))?;
                let p = g
                    .projects
                    .iter()
                    .find(|p| p.name == repo && !p.folder.is_empty())
                    .ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на той машине"))?;
                (format!("{}/{}", p.folder.trim_end_matches('/'), memory::UNIVERSE_DIR), false)
            };
            let entries = far_entries(app, &m, &dir).await?;
            let mut v = json!({
                "ok": true, "galaxy": galaxy, "repo": repo, "level": level,
                "dir": dir, "exists": !entries.is_empty(), "writable": writable,
                "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
                "note": if writable { "" } else { "Память вселенной живёт в git: здесь она только читается, а меняется в своей галактике и приезжает через git." },
            });
            if repo.is_empty() {
                v["connect"] = connect_state(app, &Place::Far(m.clone()), FAR_GALAXY_DIR).await;
            }
            Ok(v)
        }
    }
}

/// `GET /api/memory?galaxy=&repo=`: the entries of a level, with the Coordinator's wiring (Galaxy) or git state and
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
    galaxy: String,
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
            "ok": true, "preview": true, "token": self.token(), "level": level, "galaxy": self.galaxy, "repo": self.repo,
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

/// What the check finds in the entries that will be written; for a universe only (the Galaxy's memory is local).
fn check_entries(changes: &[Change]) -> (Vec<FindingOut>, Vec<Value>) {
    let own = memory_guard::own_terms();
    let mut findings = Vec::new();
    let mut redacted = Vec::new();
    for c in changes {
        let Some(e) = &c.entry else {
            redacted.push(Value::Null);
            continue;
        };
        let before = findings.len();
        for (field, text) in [("title", &e.title), ("description", &e.description), ("body", &e.body)] {
            for f in memory_guard::scan(text, field, &own) {
                // the summary is cut from the body when the human wrote none: one hit, listed once
                if !findings.iter().any(|o: &FindingOut| o.file == c.file && o.f.kind == f.kind && o.f.text == f.text) {
                    findings.push(FindingOut { file: c.file.clone(), f });
                }
            }
        }
        if findings.len() > before {
            redacted.push(json!({
                "title": memory_guard::redact(&e.title, &own),
                "description": memory_guard::redact(&e.description, &own),
                "body": memory_guard::redact(&e.body, &own),
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
}

impl Local {
    fn open(cfg: &crate::config::AppConfig, repo: &str) -> Result<Local, Fail> {
        if repo.is_empty() {
            return Ok(Local { store: memory::galaxy_store(cfg), drafts: None, root: None });
        }
        let root = memory::repo_root(cfg, repo).ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
        let drafts = memory::git_dirty(&root).is_some().then(|| memory::Drafts::new(cfg, repo));
        Ok(Local { store: memory::universe_store(&root, repo), drafts, root: Some(root) })
    }

    fn get(&self, id: &str) -> Option<Entry> {
        match &self.drafts {
            Some(d) => memory::effective_get(&self.store, d, id).filter(|(_, m)| *m != "deleted").map(|(e, _)| e),
            None => self.store.get(id),
        }
    }

    fn taken(&self, id: &str) -> bool {
        self.store.dir.join(format!("{id}.md")).exists() || self.drafts.as_ref().is_some_and(|d| d.puts().iter().any(|e| e.id == id))
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
    Delete { galaxy: String, repo: String, id: String },
    Lines { repo: String, task: String, lines: Vec<Line> },
}

fn far_universe_refused(repo: &str, what: &str) -> Option<Fail> {
    (!repo.is_empty()).then(|| Fail::new(403, format!("память вселенной живёт в git: {what}")))
}

/// Works out what a request would change, without writing anything.
async fn plan(app: &Shared, req: &Req) -> Result<Plan, Fail> {
    let (galaxy, repo) = match req {
        Req::Save(b) => (b.galaxy.clone(), b.repo.clone()),
        Req::Delete { galaxy, repo, .. } => (galaxy.clone(), repo.clone()),
        Req::Lines { repo, .. } => (String::new(), repo.clone()),
    };
    if let Some(f) = bad_scope(&galaxy, &repo) {
        return Err(f);
    }
    match req {
        Req::Save(b) if !b.id.is_empty() && !memory::valid_stem(&b.id) => return Err(Fail::new(400, "недопустимое имя записи")),
        Req::Delete { id, .. } if !memory::valid_stem(id) => return Err(Fail::new(400, "недопустимое имя записи")),
        _ => {}
    }
    let default_kind = if repo.is_empty() { "rule" } else { "decision" };
    let place = place_of(&galaxy).await?;
    let mut draft = false;
    let mut root = None;
    let changes: Vec<Change> = match place {
        Place::Home => {
            let (cfg, repo2, req2) = (app.cfg.clone(), repo.clone(), req_clone(req));
            let (changes, drafted, r) = tokio::task::spawn_blocking(move || -> Result<(Vec<Change>, bool, Option<String>), Fail> {
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
            draft = drafted;
            root = r;
            changes
        }
        Place::Far(m) => {
            if let Some(f) = far_universe_refused(&repo, "запишите её в своей галактике, а в далёкой она появится после pull") {
                return Err(f);
            }
            let all = far_entries(app, &m, FAR_GALAXY_DIR).await?;
            match req {
                Req::Save(b) => {
                    let existing = all.iter().find(|e| e.id == b.id).cloned();
                    // a new entry has a slug for a name; one that is there keeps its name
                    if !b.id.is_empty() && existing.is_none() && !memory::valid_id(&b.id) {
                        return Err(Fail::new(400, "недопустимое имя записи"));
                    }
                    let mut entry = memory::complete(merged(existing.clone(), b), default_kind, now_ms()).map_err(|e| Fail::new(422, e))?;
                    memory::assign_id(&mut entry, |c| all.iter().any(|e| e.id == c)).map_err(|e| Fail::new(400, e))?;
                    vec![change_of(&entry.id, format!("{FAR_GALAXY_DIR}/{}.md", entry.id), existing.as_ref(), Some(entry.clone()))]
                }
                Req::Delete { id, .. } => {
                    let existing = all.iter().find(|e| e.id == *id).ok_or_else(|| Fail::new(404, "такой записи нет"))?;
                    vec![change_of(id, format!("{FAR_GALAXY_DIR}/{id}.md"), Some(existing), None)]
                }
                Req::Lines { .. } => return Err(Fail::new(403, "память вселенной живёт в git")),
            }
        }
    };
    if changes.iter().all(|c| c.op == "change" && c.before == c.after) {
        return Err(Fail::new(422, "ничего не изменится: запись такая же"));
    }
    // The check is for what travels with git: a universe's memory (the Galaxy's stays on this machine).
    let (findings, redacted, visibility) = if repo.is_empty() {
        (Vec::new(), Vec::new(), None)
    } else {
        let (findings, redacted) = check_entries(&changes);
        let vis = match (&root, findings.is_empty()) {
            (Some(r), false) => Some(visibility_of(app, r).await),
            _ => None,
        };
        (findings, redacted, vis)
    };
    Ok(Plan { galaxy, repo, changes, findings, visibility, draft, redacted })
}

fn req_clone(r: &Req) -> Req {
    match r {
        Req::Save(b) => Req::Save(b.clone()),
        Req::Delete { galaxy, repo, id } => Req::Delete { galaxy: galaxy.clone(), repo: repo.clone(), id: id.clone() },
        Req::Lines { repo, task, lines } => Req::Lines { repo: repo.clone(), task: task.clone(), lines: lines.clone() },
    }
}

/// What the human's word allows: refuses a write that the preview did not show, or that holds personal data without
/// their word for it.
fn allowed(plan: &Plan, gate: &Gate) -> Result<(), Fail> {
    if gate.token.as_deref() != Some(plan.token().as_str()) {
        return Err(Fail::new(409, "запись изменилась с тех пор, как вы смотрели «было → стало»: посмотрите ещё раз"));
    }
    match plan.risk() {
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

/// Writes a plan: files for the Galaxy (and a folder that is not in git), drafts for a universe in a git checkout.
async fn execute(app: &Shared, plan: &Plan) -> Result<Value, Fail> {
    let entries: Vec<(String, Option<Entry>)> = plan.changes.iter().map(|c| (c.id.clone(), c.entry.clone())).collect();
    let saved: Vec<Entry>;
    match place_of(&plan.galaxy).await? {
        Place::Home => {
            let (cfg, repo) = (app.cfg.clone(), plan.repo.clone());
            saved = tokio::task::spawn_blocking(move || -> Result<Vec<Entry>, Fail> {
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
        }
        Place::Far(m) => {
            let mut all = far_entries(app, &m, FAR_GALAXY_DIR).await?;
            let mut writes = Vec::new();
            let mut removes = Vec::new();
            let mut done = Vec::new();
            for (id, entry) in entries {
                all.retain(|e| e.id != id);
                match entry {
                    Some(e) => {
                        writes.push((format!("{}.md", e.id), memory::render_entry(&e)));
                        all.push(e.clone());
                        done.push(e);
                    }
                    None => removes.push(format!("{id}.md")),
                }
            }
            writes.push((memory::INDEX.to_string(), memory::render_index("Память Галактики", &all)));
            ssh_run(app, &m, &write_script(FAR_GALAXY_DIR, &writes, &removes)).await?;
            remember_far(app, &m.label, all).await;
            saved = done;
        }
    }
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

async fn remember_far(app: &Shared, label: &str, galaxy: Vec<Entry>) {
    {
        let mut all = app.far_memory.lock().await;
        all.entry(label.to_string()).or_default().galaxy = galaxy;
    }
    app::index_snapshot(app).await;
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
    galaxy: String,
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
/// it writes. A universe in a git checkout gets a draft outside the checkout; its commit is `/api/memory/commit`.
pub async fn memory_save(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<SaveReq>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    respond(run(&app, Req::Save(body.body), &body.gate).await)
}

#[derive(Deserialize)]
pub struct DeleteBody {
    #[serde(default)]
    galaxy: String,
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
    respond(run(&app, Req::Delete { galaxy: body.galaxy, repo: body.repo, id: body.id }, &body.gate).await)
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
                "ok": true, "preview": true, "token": token, "level": "universe", "galaxy": "", "repo": repo, "draft": true,
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
pub struct CommitBody {
    repo: String,
    #[serde(flatten)]
    gate: Gate,
}

fn read_text(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(200_000).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// What a commit of a universe's memory would put into git: the drafts, and whatever else in `docs/memory` is uncommitted
/// in the checkout (the human's or an agent's own files). Nothing is written.
fn commit_plan(cfg: &crate::config::AppConfig, repo: &str) -> Result<(String, Vec<Change>), Fail> {
    let root = memory::repo_root(cfg, repo).ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
    let dirty = memory::git_dirty(&root).ok_or_else(|| Fail::new(422, "это не git-репозиторий"))?;
    let drafts = memory::Drafts::new(cfg, repo);
    let conflicts = memory::draft_conflicts(&root, &drafts);
    if !conflicts.is_empty() {
        return Err(Fail::new(
            409,
            format!("в чекауте есть незакоммиченные правки тех же файлов: {}. Закоммитьте или отмените их, потом коммитьте память.", conflicts.join(", ")),
        ));
    }
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
    let index = format!("{}/{}", memory::UNIVERSE_DIR, memory::INDEX);
    for (state, file) in dirty {
        if file == index || changes.iter().any(|c| c.file == file) {
            continue;
        }
        let id = file.rsplit('/').next().unwrap_or(&file).trim_end_matches(".md").to_string();
        let before = memory::git_head_text(&root, &file);
        let after = read_text(&std::path::Path::new(&root).join(&file));
        let op = if state.contains('D') || after.is_none() { "delete" } else if before.is_some() { "change" } else { "add" };
        changes.push(Change { op, id, file, before, after: if op == "delete" { None } else { after }, entry: None });
    }
    if changes.is_empty() {
        return Err(Fail::new(422, "в docs/memory нечего коммитить"));
    }
    Ok((root, changes))
}

/// `POST /api/memory/commit`: puts a universe's drafts into `docs/memory/` and commits them (and only that folder). The
/// same preview, check and confirmation as a save: the text goes into git here, so it is looked at once more.
pub async fn memory_commit(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<CommitBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        if !taskmeta::valid_repo(&body.repo) {
            return Err(Fail::new(400, "bad universe name"));
        }
        let (cfg, repo) = (app.cfg.clone(), body.repo.clone());
        let (root, changes) = tokio::task::spawn_blocking(move || commit_plan(&cfg, &repo))
            .await
            .map_err(|e| Fail::new(500, e.to_string()))??;
        // what goes into git is checked as a whole, header and body
        let own = memory_guard::own_terms();
        let mut findings = Vec::new();
        for c in &changes {
            if let Some(after) = &c.after {
                for f in memory_guard::scan(after, "text", &own) {
                    findings.push(FindingOut { file: c.file.clone(), f });
                }
            }
        }
        let vis = if findings.is_empty() { None } else { Some(visibility_of(&app, &root).await) };
        let token = token_of(&changes, &findings);
        let risk = risk_of(&findings, vis);
        if body.gate.confirm != Some(true) {
            return Ok(json!({
                "ok": true, "preview": true, "token": token, "level": "universe", "repo": body.repo, "commit": true,
                "changes": changes, "findings": findings, "risk": risk, "visibility": vis, "redacted": [],
            }));
        }
        if body.gate.token.as_deref() != Some(token.as_str()) {
            return Err(Fail::new(409, "память изменилась с тех пор, как вы смотрели список: посмотрите ещё раз"));
        }
        match risk {
            "block" if body.gate.accept_public != Some(true) => {
                return Err(Fail::new(403, "в файлах найдены персональные данные или секреты, а репозиторий публичный (или это неизвестно): по умолчанию в него не коммитим"));
            }
            "confirm" if body.gate.accept_risk != Some(true) && body.gate.accept_public != Some(true) => {
                return Err(Fail::new(422, "в файлах найдены персональные данные или секреты: нужно явное «да»"));
            }
            _ => {}
        }
        let titles: Vec<String> = changes
            .iter()
            .filter_map(|c| c.entry.as_ref().map(|e| e.title.clone()).or_else(|| c.before.as_deref().map(|b| memory::parse_entry(&c.id, b).title)))
            .collect();
        let message = if titles.is_empty() {
            "Память вселенной".to_string()
        } else {
            format!("Память вселенной: {}", memory::cut(&titles.join("; "), 100))
        };
        let (cfg, repo, msg) = (app.cfg.clone(), body.repo.clone(), message.clone());
        let n = changes.len();
        let hash = tokio::task::spawn_blocking(move || {
            let drafts = memory::Drafts::new(&cfg, &repo);
            memory::commit_drafts(&root, &repo, &drafts, &msg)
        })
        .await
        .map_err(|e| Fail::new(500, e.to_string()))?
        .map_err(|e| Fail::new(422, e))?;
        app::index_snapshot(&app).await;
        Ok(json!({"ok": true, "commit": hash, "message": message, "files": n}))
    }
    .await;
    respond(result)
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
    #[serde(default)]
    galaxy: String,
    on: bool,
    /// The instructions the human saw on the card. Absent: only a preview.
    expect: Option<String>,
    confirm: Option<bool>,
}

/// The instructions of the galaxy's board as they are now (empty when the setting is not set).
async fn current_instructions(app: &Shared, place: &Place) -> Result<String, Fail> {
    let settings = station::read_settings(&app.cfg, place, "").await.map_err(Fail::refused)?;
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
        if let Some(f) = bad_scope(&body.galaxy, "") {
            return Err(f);
        }
        let place = place_of(&body.galaxy).await?;
        let dir = match &place {
            Place::Home => memory::galaxy_dir(&app.cfg).display().to_string(),
            Place::Far(_) => FAR_GALAXY_DIR.to_string(),
        };
        let block = memory::coordinator_block(&dir);
        let current = current_instructions(&app, &place).await?;
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
        let out = station::board_run(&app.cfg, &place, &args, station::BOARD_WAIT).await?;
        if !out.ok() {
            return Err(Fail::new(422, out.words()));
        }
        match &place {
            Place::Home => app::refresh(&app).await,
            Place::Far(m) => {
                let _ = crate::api::ask_galaxy(&app, &m.label).await;
            }
        }
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

    #[test]
    fn far_paths_are_under_that_machines_home() {
        assert_eq!(sh_path("~/.herdr-projects/memory/galaxy"), "\"$HOME\"/.herdr-projects/memory/galaxy");
        assert_eq!(sh_path("/srv/my projects/a"), "'/srv/my projects/a'");
    }

    #[test]
    fn a_far_read_comes_back_as_entries_in_the_order_asked() {
        let one = memory::render_entry(&Entry {
            id: "a".into(),
            title: "Раз".into(),
            description: "один".into(),
            kind: "rule".into(),
            body: "тело".into(),
            ..Entry::default()
        });
        let out = format!(
            "Welcome to vm-a\n\u{1e}DIR 1\n\u{1f}FILE a.md\n{one}\n\u{1f}FILE MEMORY.md\n# index\n\n\u{1e}DIR 2\n\u{1e}DIR 3\n\u{1f}FILE z.md\nбез шапки\n\n"
        );
        let got = parse_read(&out, 3);
        assert_eq!(got[0].len(), 1, "the index is not an entry");
        assert_eq!(got[0][0].title, "Раз");
        assert_eq!(got[0][0].body, "тело");
        assert!(got[1].is_empty());
        assert_eq!(got[2][0].id, "z");
        assert_eq!(parse_read("no records at all", 2), vec![Vec::<Entry>::new(), vec![]]);
        assert_eq!(parse_read("\u{1e}DIR 9\n", 2), vec![Vec::<Entry>::new(), vec![]], "a folder number out of range is ignored");
    }

    #[test]
    fn a_far_write_is_one_line_and_hides_the_text_in_base64() {
        let s = write_script(
            FAR_GALAXY_DIR,
            &[("a.md".into(), "привет'; rm -rf /; echo '".into())],
            &["old.md".into()],
        );
        assert!(!s.contains('\n'), "one line, so the far login shell takes it as one command");
        assert!(!s.contains("rm -rf /"), "the text never reaches the shell as text");
        assert!(s.starts_with("HP_MEMORY=1;"));
        assert!(s.contains("mkdir -p \"$d\"") && s.contains("rm -f \"$d\"/old.md"));
        assert!(s.contains("mv \"$d\"/a.md.new \"$d\"/a.md"));
        let r = read_script(&[FAR_GALAXY_DIR.into(), "/srv/p q/docs/memory".into()]);
        assert!(!r.contains('\n') && r.contains("'/srv/p q/docs/memory'"));
    }

    #[test]
    fn the_far_script_really_runs_in_sh() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_str().unwrap().to_string();
        let run = |script: &str| {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .env("HOME", &home)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let e = Entry { id: "a".into(), title: "Заголовок: с двоеточием".into(), description: "d".into(), kind: "rule".into(), body: "Строка 1\nСтрока 2 'кавычки'".into(), added: "2026-10-21".into(), ..Entry::default() };
        run(&write_script(
            FAR_GALAXY_DIR,
            &[("a.md".into(), memory::render_entry(&e)), (memory::INDEX.into(), memory::render_index("П", std::slice::from_ref(&e)))],
            &[],
        ));
        let got = parse_read(&run(&read_script(&[FAR_GALAXY_DIR.into(), "/nonexistent/x".into()])), 2);
        assert_eq!(got[0].len(), 1);
        assert_eq!(got[0][0].title, e.title);
        assert_eq!(got[0][0].body, e.body);
        assert!(got[1].is_empty());
        run(&write_script(FAR_GALAXY_DIR, &[], &["a.md".into()]));
        assert!(parse_read(&run(&read_script(&[FAR_GALAXY_DIR.into()])), 1)[0].is_empty());
    }

    fn plan_with(body: &str, vis: Option<Visibility>) -> Plan {
        let mut e = memory::complete(Entry { title: "Т".into(), body: body.into(), ..Entry::default() }, "decision", 0).unwrap();
        e.id = "t".into();
        let changes = vec![change_of("t", "docs/memory/t.md".into(), None, Some(e))];
        let (findings, redacted) = check_entries(&changes);
        Plan { galaxy: String::new(), repo: "demo".into(), changes, findings, visibility: vis, draft: true, redacted }
    }

    fn gate(token: &str, risk: bool, public: bool) -> Gate {
        Gate { confirm: Some(true), token: Some(token.into()), accept_risk: Some(risk).filter(|r| *r), accept_public: Some(public).filter(|p| *p) }
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
        let (f, r) = check_entries(&[change_of("t", "docs/memory/t.md".into(), None, Some(e))]);
        let got: Vec<(&str, &str)> = f.iter().map(|x| (x.f.field.as_str(), x.f.kind)).collect();
        assert_eq!(got, [("title", "host"), ("description", "path"), ("body", "secret")]);
        assert!(f.iter().all(|x| x.file == "docs/memory/t.md"));
        assert!(f[2].f.text.ends_with('…'), "a secret is not echoed");
        assert!(!r[0].is_null());
    }
}
