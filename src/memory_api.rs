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

/// The tables of contents that an agent's hand-written files left behind: the Galaxy's, and every universe's that has
/// one. The file watcher calls this at once when a memory file changes; the refresh loop only now and then.
pub async fn reindex_stores(app: &Shared) {
    let cfg = app.cfg.clone();
    let _ = tokio::task::spawn_blocking(move || {
        memory::galaxy_store(&cfg).reindex();
        for r in memory::universes(&cfg) {
            if taskmeta::valid_repo(&r.name) {
                memory::universe_store(&r.root, &r.name).reindex_existing();
            }
        }
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
                    let git = match memory::git_dirty(&root) {
                        None => json!({"checkout": false, "dirty": []}),
                        Some(d) => json!({
                            "checkout": true,
                            "dirty": d.iter().map(|(s, f)| json!({"state": s, "file": f})).collect::<Vec<_>>(),
                        }),
                    };
                    Ok(json!({
                        "dir": st.dir.display().to_string(),
                        "exists": st.exists(),
                        "writable": true,
                        "entries": st.list().iter().map(entry_json).collect::<Vec<_>>(),
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

#[derive(Deserialize)]
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

async fn save_entry(app: &Shared, b: SaveBody) -> Result<Value, Fail> {
    if let Some(f) = bad_scope(&b.galaxy, &b.repo) {
        return Err(f);
    }
    if !b.id.is_empty() && !memory::valid_stem(&b.id) {
        return Err(Fail::new(400, "недопустимое имя записи"));
    }
    let default_kind = if b.repo.is_empty() { "rule" } else { "decision" };
    match place_of(&b.galaxy).await? {
        Place::Home => {
            let cfg = app.cfg.clone();
            let saved = tokio::task::spawn_blocking(move || -> Result<Entry, Fail> {
                let st = if b.repo.is_empty() {
                    memory::galaxy_store(&cfg)
                } else {
                    let root = memory::repo_root(&cfg, &b.repo)
                        .ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
                    memory::universe_store(&root, &b.repo)
                };
                let existing = if b.id.is_empty() { None } else { st.get(&b.id) };
                let entry = memory::complete(merged(existing, &b), default_kind, now_ms()).map_err(|e| Fail::new(422, e))?;
                st.save(entry).map_err(|e| Fail::new(500, e))
            })
            .await
            .map_err(|e| Fail::new(500, e.to_string()))??;
            app::index_snapshot(app).await;
            Ok(json!({"ok": true, "entry": entry_json(&saved)}))
        }
        Place::Far(m) => {
            if !b.repo.is_empty() {
                return Err(Fail::new(
                    403,
                    "память вселенной живёт в git: запишите её в своей галактике, а в далёкой она появится после pull",
                ));
            }
            let mut all = far_entries(app, &m, FAR_GALAXY_DIR).await?;
            let existing = all.iter().find(|e| e.id == b.id).cloned();
            // a new entry has a slug for a name; one that is there keeps its name
            if !b.id.is_empty() && existing.is_none() && !memory::valid_id(&b.id) {
                return Err(Fail::new(400, "недопустимое имя записи"));
            }
            let mut entry = memory::complete(merged(existing, &b), default_kind, now_ms()).map_err(|e| Fail::new(422, e))?;
            if entry.id.is_empty() {
                entry.id = memory::unique_id(&memory::slugify(&entry.title), |c| all.iter().any(|e| e.id == c));
            }
            all.retain(|e| e.id != entry.id);
            all.push(entry.clone());
            let index = memory::render_index("Память Галактики", &all);
            let writes = vec![
                (format!("{}.md", entry.id), memory::render_entry(&entry)),
                (memory::INDEX.to_string(), index),
            ];
            ssh_run(app, &m, &write_script(FAR_GALAXY_DIR, &writes, &[])).await?;
            remember_far(app, &m.label, all).await;
            Ok(json!({"ok": true, "entry": entry_json(&entry)}))
        }
    }
}

async fn remember_far(app: &Shared, label: &str, galaxy: Vec<Entry>) {
    {
        let mut all = app.far_memory.lock().await;
        all.entry(label.to_string()).or_default().galaxy = galaxy;
    }
    app::index_snapshot(app).await;
}

/// `POST /api/memory/save`: a new entry (no `id`) or an edit.
pub async fn memory_save(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<SaveBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    match save_entry(&app, body).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[derive(Deserialize)]
pub struct DeleteBody {
    #[serde(default)]
    galaxy: String,
    #[serde(default)]
    repo: String,
    id: String,
    confirm: Option<bool>,
}

async fn delete_entry(app: &Shared, b: DeleteBody) -> Result<Value, Fail> {
    if b.confirm != Some(true) {
        return Err(Fail::new(400, "нужно подтверждение"));
    }
    if let Some(f) = bad_scope(&b.galaxy, &b.repo) {
        return Err(f);
    }
    if !memory::valid_stem(&b.id) {
        return Err(Fail::new(400, "недопустимое имя записи"));
    }
    match place_of(&b.galaxy).await? {
        Place::Home => {
            let cfg = app.cfg.clone();
            let (repo, id) = (b.repo.clone(), b.id.clone());
            let gone = tokio::task::spawn_blocking(move || -> Result<bool, Fail> {
                let st = if repo.is_empty() {
                    memory::galaxy_store(&cfg)
                } else {
                    let root = memory::repo_root(&cfg, &repo)
                        .ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
                    memory::universe_store(&root, &repo)
                };
                st.delete(&id).map_err(|e| Fail::new(500, e))
            })
            .await
            .map_err(|e| Fail::new(500, e.to_string()))??;
            app::index_snapshot(app).await;
            Ok(json!({"ok": true, "removed": gone}))
        }
        Place::Far(m) => {
            if !b.repo.is_empty() {
                return Err(Fail::new(403, "память вселенной живёт в git: удалите запись в своей галактике"));
            }
            let mut all = far_entries(app, &m, FAR_GALAXY_DIR).await?;
            let had = all.iter().any(|e| e.id == b.id);
            all.retain(|e| e.id != b.id);
            let writes = vec![(memory::INDEX.to_string(), memory::render_index("Память Галактики", &all))];
            ssh_run(app, &m, &write_script(FAR_GALAXY_DIR, &writes, &[format!("{}.md", b.id)])).await?;
            remember_far(app, &m.label, all).await;
            Ok(json!({"ok": true, "removed": had}))
        }
    }
}

/// `POST /api/memory/delete`: removes an entry's file (with `confirm: true`).
pub async fn memory_delete(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<DeleteBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    match delete_entry(&app, body).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[derive(Deserialize)]
pub struct CommitBody {
    repo: String,
    confirm: Option<bool>,
}

/// `POST /api/memory/commit`: commits the changed files of a universe's `docs/memory` (and only them) in its checkout.
pub async fn memory_commit(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<CommitBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        if body.confirm != Some(true) {
            return Err(Fail::new(400, "нужно подтверждение"));
        }
        if !taskmeta::valid_repo(&body.repo) {
            return Err(Fail::new(400, "bad universe name"));
        }
        let cfg = app.cfg.clone();
        let repo = body.repo.clone();
        tokio::task::spawn_blocking(move || -> Result<Value, Fail> {
            let root = memory::repo_root(&cfg, &repo)
                .ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
            let dirty = memory::git_dirty(&root).ok_or_else(|| Fail::new(422, "это не git-репозиторий"))?;
            if dirty.is_empty() {
                return Err(Fail::new(422, "в docs/memory нечего коммитить"));
            }
            let titles: Vec<String> = memory::universe_store(&root, &repo)
                .list()
                .into_iter()
                .filter(|e| dirty.iter().any(|(_, f)| *f == format!("{}/{}.md", memory::UNIVERSE_DIR, e.id)))
                .map(|e| e.title)
                .collect();
            let message = if titles.is_empty() {
                "Память вселенной".to_string()
            } else {
                format!("Память вселенной: {}", memory::cut(&titles.join("; "), 100))
            };
            let hash = memory::git_commit(&root, &message).map_err(|e| Fail::new(422, e))?;
            Ok(json!({"ok": true, "commit": hash, "message": message, "files": dirty.len()}))
        })
        .await
        .map_err(|e| Fail::new(500, e.to_string()))?
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
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

/// Writes the lines into the universe's memory, all or none: every line is checked before the first is written.
async fn keep_lines(app: &Shared, repo: &str, task: &str, lines: Vec<Line>) -> Result<Vec<Entry>, Fail> {
    let (cfg, repo, task) = (app.cfg.clone(), repo.to_string(), task.to_string());
    let saved = tokio::task::spawn_blocking(move || -> Result<Vec<Entry>, Fail> {
        let root = memory::repo_root(&cfg, &repo).ok_or_else(|| Fail::new(404, "у этой вселенной нет папки на этой машине"))?;
        let st = memory::universe_store(&root, &repo);
        let now = now_ms();
        let entries = lines
            .iter()
            .map(|l| {
                let e = Entry { kind: l.kind.clone(), body: l.text.clone(), source: format!("task {task}"), ..Entry::default() };
                memory::complete(e, "decision", now).map_err(|er| Fail::new(422, er))
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.into_iter().map(|e| st.save(e).map_err(|er| Fail::new(500, er))).collect()
    })
    .await
    .map_err(|e| Fail::new(500, e.to_string()))??;
    app::index_snapshot(app).await;
    Ok(saved)
}

/// `POST /api/memory/suggestion`: keeps the suggested lines in the universe's memory, or drops them.
pub async fn memory_suggestion(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<ResolveBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
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
                // One accept at a time: a second, concurrent one finds the suggestion taken.
                if !app.db.suggest_take(&row.task) {
                    return Err(Fail::new(409, "это предложение уже обрабатывается"));
                }
                match keep_lines(&app, &row.repo, &row.task, lines).await {
                    Ok(saved) => {
                        app.db.suggest_set_state(&row.task, "done");
                        sync_snapshot(&app).await;
                        Ok(json!({"ok": true, "saved": saved.iter().map(entry_json).collect::<Vec<_>>()}))
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
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
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
}
