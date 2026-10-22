//! The planner's SQLite database: `<state>/planner.sqlite`.
//!
//! Everything the planner keeps across restarts lives here: the God's list (todo),
//! the journal, the Coordinator's chat (its state and an index of its transcript
//! turns), the status history, the search index and translations. One connection behind a mutex; every method is short and blocking, so
//! async callers wrap them in `spawn_blocking`.
//!
//! Full-text search uses FTS5 with the `trigram` tokenizer: it finds substrings of
//! 3+ characters in any script (Cyrillic included) and ignores case. Shorter terms
//! cannot be looked up in a trigram index, so they fall back to `LIKE`.

use crate::app::{ChatMessage, ChatState, JournalEntry, TodoItem};
use crate::attachments::Attachment;
use crate::search::highlight;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Schema migrations in order; `PRAGMA user_version` counts how many have run.
const MIGRATIONS: &[&str] = &[
    include_str!("db/migrations/001_init.sql"),
    include_str!("db/migrations/002_todo_files.sql"),
    include_str!("db/migrations/003_memory_suggest.sql"),
    include_str!("db/migrations/004_no_far_galaxies.sql"),
    include_str!("db/migrations/005_runners.sql"),
    include_str!("db/migrations/006_runner_hist.sql"),
];

/// Entries the journal hands out; older ones stay in the table up to [`JOURNAL_KEEP`].
const JOURNAL_LIVE: usize = 80;
const JOURNAL_KEEP: usize = 500;
#[allow(dead_code)]
const TRANSLATIONS_KEEP: usize = 500;

/// A searchable thing: a planet, a task, a universe, a satellite, ... The lead builds
/// these from the board snapshot; `key` is unique across the whole index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchDoc {
    pub key: String,
    pub kind: String,
    pub id: String,
    pub path: String,
    pub title: String,
    pub text: String,
    pub status: String,
    pub at: u64,
}

/// A suggestion for a universe's memory, made when a task was done.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SuggestRow {
    pub task: String,
    pub repo: String,
    pub tag: String,
    /// JSON list of `{kind, text}`.
    pub lines: String,
    pub state: String,
    pub at: u64,
}

/// A document found by [`Db::search_docs_match`]. `rank` is FTS5 `bm25()`: lower is
/// better (more negative). Matches found by `LIKE` have rank 0.
#[derive(Debug, Clone, PartialEq)]
pub struct DocHit {
    pub doc: SearchDoc,
    pub rank: f64,
}

/// One transcript turn as stored in the index. `n` is its position over all transcripts.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatTurnRow {
    pub n: usize,
    pub at: u64,
    pub from: String,
    pub q: String,
    pub a: String,
}

/// A turn found by [`Db::chat_search`]. The snippets are HTML: the text is escaped
/// and the matched words are wrapped in `<mark>`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatHit {
    pub n: usize,
    pub at: u64,
    pub from: String,
    /// The question as written (plain text).
    pub q: String,
    pub q_snip: String,
    pub a_snip: String,
    /// FTS5 `bm25()` (lower is better), 0 for `LIKE` matches.
    pub rank: f64,
}

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

#[allow(dead_code)]
impl Db {
    /// Opens (or creates) the database file. The folder becomes 0700, the file 0600.
    pub fn open(path: &Path) -> Result<Db, String> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            restrict(dir, 0o700);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Created before SQLite sees it, so it is never world-readable.
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(path);
        }
        let conn = Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let db = Self::init(conn)?;
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_owned();
            p.push(suffix);
            restrict(Path::new(&p), 0o600);
        }
        Ok(db)
    }

    /// A database in memory, for tests and checks.
    pub fn memory() -> Result<Db, String> {
        let conn = Connection::open_in_memory().map_err(db_err)?;
        Self::init(conn)
    }

    fn init(mut conn: Connection) -> Result<Db, String> {
        conn.busy_timeout(Duration::from_secs(5)).map_err(db_err)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )
        .map_err(db_err)?;
        migrate(&mut conn)?;
        Ok(Db(Arc::new(Mutex::new(conn))))
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs one short operation. A failed statement is logged and gives the default
    /// value: the planner keeps serving what it can.
    fn with<T: Default>(
        &self,
        what: &str,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> T {
        let mut guard = self.conn();
        match f(&mut guard) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("planner db: {what}: {e}");
                T::default()
            }
        }
    }

    // ---- todo (the God's list) ----

    pub fn todo_list(&self) -> Vec<TodoItem> {
        self.with("todo_list", |c| {
            let mut st =
                c.prepare("SELECT id, text, at, to_, files FROM todo ORDER BY ord, rowid")?;
            let rows = st.query_map([], |r| {
                Ok(TodoItem {
                    id: r.get(0)?,
                    text: r.get(1)?,
                    at: r.get::<_, i64>(2)? as u64,
                    to: r.get(3)?,
                    attachments: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                })
            })?;
            rows.collect()
        })
    }

    /// Appends an item at the end. Its id is `t<ms>-<n>`.
    pub fn todo_add(&self, text: &str, at: u64) -> TodoItem {
        self.with("todo_add", |c| {
            let tx = c.transaction()?;
            let ord: i64 =
                tx.query_row("SELECT COALESCE(MAX(ord), -1) + 1 FROM todo", [], |r| {
                    r.get(0)
                })?;
            let ms = now_ms();
            let mut n = 0u32;
            let id = loop {
                let cand = format!("t{ms}-{n}");
                let taken: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM todo WHERE id = ?1)",
                    [&cand],
                    |r| r.get(0),
                )?;
                if !taken {
                    break cand;
                }
                n += 1;
            };
            tx.execute(
                "INSERT INTO todo (id, text, at, ord, to_) VALUES (?1, ?2, ?3, ?4, '')",
                params![id, text, at as i64, ord],
            )?;
            tx.commit()?;
            Ok(TodoItem {
                id,
                text: text.to_string(),
                at,
                to: String::new(),
                attachments: Vec::new(),
            })
        })
    }

    /// Replaces the files of the item; `false` when there is no such item.
    pub fn todo_files(&self, id: &str, files: &[Attachment]) -> bool {
        let json = serde_json::to_string(files).unwrap_or_else(|_| "[]".into());
        self.with("todo_files", |c| {
            let n = c.execute("UPDATE todo SET files = ?2 WHERE id = ?1", params![id, json])?;
            Ok(n > 0)
        })
    }

    /// Changes the text; `false` when there is no such item.
    pub fn todo_update(&self, id: &str, text: &str) -> bool {
        self.with("todo_update", |c| {
            let n = c.execute("UPDATE todo SET text = ?2 WHERE id = ?1", params![id, text])?;
            Ok(n > 0)
        })
    }

    pub fn todo_remove(&self, id: &str) -> bool {
        self.with("todo_remove", |c| {
            let n = c.execute("DELETE FROM todo WHERE id = ?1", [id])?;
            Ok(n > 0)
        })
    }

    /// Puts the given items first, in the given order. Unknown ids are ignored; the
    /// items not listed keep their relative order after them.
    pub fn todo_reorder(&self, ids: &[String]) {
        self.with("todo_reorder", |c| {
            let tx = c.transaction()?;
            let current: Vec<String> = {
                let mut st = tx.prepare("SELECT id FROM todo ORDER BY ord, rowid")?;
                let rows = st.query_map([], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let known: HashSet<&str> = current.iter().map(String::as_str).collect();
            let mut placed: HashSet<&str> = HashSet::new();
            let mut order: Vec<&str> = Vec::with_capacity(current.len());
            for id in ids {
                if known.contains(id.as_str()) && placed.insert(id.as_str()) {
                    order.push(id.as_str());
                }
            }
            for id in &current {
                if placed.insert(id.as_str()) {
                    order.push(id.as_str());
                }
            }
            for (i, id) in order.iter().enumerate() {
                tx.execute(
                    "UPDATE todo SET ord = ?1 WHERE id = ?2",
                    params![i as i64, id],
                )?;
            }
            tx.commit()
        })
    }

    /// Addresses the item: `""` (nobody), `"quasar"` or a task id. `false` when there is no such item.
    pub fn todo_assign(&self, id: &str, to: &str) -> bool {
        self.with("todo_assign", |c| {
            let n = c.execute("UPDATE todo SET to_ = ?2 WHERE id = ?1", params![id, to])?;
            Ok(n > 0)
        })
    }

    // ---- journal ----

    /// The sequence number and the newest entries (at most 80), newest first.
    pub fn journal_load(&self) -> (u64, Vec<JournalEntry>) {
        self.with("journal_load", |c| {
            let seq = journal_seq_in(c)?;
            let mut st =
                c.prepare("SELECT n, t, id, s, text FROM journal ORDER BY n DESC LIMIT ?1")?;
            let rows = st.query_map([JOURNAL_LIVE as i64], journal_row)?;
            Ok((seq, rows.collect::<rusqlite::Result<Vec<_>>>()?))
        })
    }

    /// Stores entries (by their `n`) and keeps the newest 500 rows.
    pub fn journal_append(&self, entries: &[JournalEntry]) {
        self.with("journal_append", |c| {
            let tx = c.transaction()?;
            {
                let mut ins = tx.prepare(
                    "INSERT OR REPLACE INTO journal (n, t, id, s, text) VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for e in entries {
                    ins.execute(params![e.n as i64, e.t as i64, e.id, e.s, e.text])?;
                }
            }
            trim_journal(&tx)?;
            tx.commit()
        })
    }

    // ---- chat (the former chat.json) ----

    pub fn chat_load(&self) -> ChatState {
        self.with("chat_load", |c| {
            let row: Option<(String, u64, String, String)> = c
                .query_row(
                    "SELECT last_sent, sent_at, last_reply, history FROM chat_state WHERE id = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            Ok(match row {
                None => ChatState::default(),
                Some((last_sent, sent_at, last_reply, history)) => ChatState {
                    last_sent,
                    sent_at,
                    last_reply,
                    history: parse_history(&history),
                    ..Default::default()
                },
            })
        })
    }

    /// Replaces the stored chat state. The caller caps the history.
    pub fn chat_save(&self, chat: &ChatState) {
        self.with("chat_save", |c| {
            let history = serde_json::to_string(&chat.history).unwrap_or_else(|_| "[]".into());
            c.execute(
                "INSERT INTO chat_state (id, last_sent, sent_at, last_reply, history) \
                 VALUES (1, ?1, ?2, ?3, ?4) \
                 ON CONFLICT(id) DO UPDATE SET last_sent = excluded.last_sent, \
                 sent_at = excluded.sent_at, last_reply = excluded.last_reply, \
                 history = excluded.history",
                params![
                    chat.last_sent,
                    chat.sent_at as i64,
                    chat.last_reply,
                    history
                ],
            )?;
            Ok(())
        })
    }

    // ---- status history ----

    /// Records the task's status, but only when it differs from the last recorded one.
    pub fn status_note(&self, task: &str, status: &str, at: u64) {
        self.with("status_note", |c| {
            let last: Option<String> = c
                .query_row(
                    "SELECT status FROM status_history WHERE task = ?1 ORDER BY id DESC LIMIT 1",
                    [task],
                    |r| r.get(0),
                )
                .optional()?;
            if last.as_deref() != Some(status) {
                c.execute(
                    "INSERT INTO status_history (task, status, at) VALUES (?1, ?2, ?3)",
                    params![task, status, at as i64],
                )?;
            }
            Ok(())
        })
    }

    /// The task's current status and when it began.
    pub fn status_since(&self, task: &str) -> Option<(String, u64)> {
        self.with("status_since", |c| {
            c.query_row(
                "SELECT status, at FROM status_history WHERE task = ?1 ORDER BY id DESC LIMIT 1",
                [task],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)),
            )
            .optional()
        })
    }

    // ---- translations ----

    pub fn translation_get(&self, hash: &str) -> Option<String> {
        self.with("translation_get", |c| {
            c.query_row(
                "SELECT text FROM translations WHERE hash = ?1",
                [hash],
                |r| r.get(0),
            )
            .optional()
        })
    }

    /// Stores a translation; keeps the newest 500.
    pub fn translation_put(&self, hash: &str, text: &str, at: u64) {
        self.with("translation_put", |c| {
            c.execute(
                "INSERT INTO translations (hash, text, at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(hash) DO UPDATE SET text = excluded.text, at = excluded.at",
                params![hash, text, at as i64],
            )?;
            c.execute(
                "DELETE FROM translations WHERE hash NOT IN \
                 (SELECT hash FROM translations ORDER BY at DESC LIMIT ?1)",
                [TRANSLATIONS_KEEP as i64],
            )?;
            Ok(())
        })
    }

    // ---- meta ----

    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.with("meta_get", |c| {
            c.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .optional()
        })
    }

    pub fn meta_set(&self, key: &str, value: &str) {
        self.with("meta_set", |c| {
            c.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }

    // ---- shipyards (верфи) ----

    /// Stores what a runner's probe said (`json`), or why it could not be reached. The moment the CPU time of its
    /// tasks was counted last stays (see [`Db::runner_usage_take`]).
    pub fn runner_put(&self, id: &str, json: &str, ok: bool, why: &str) {
        self.with("runner_put", |c| {
            c.execute(
                "INSERT INTO runners_cache (id, json, fetched_at, ok, why) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(id) DO UPDATE SET json = excluded.json, fetched_at = excluded.fetched_at, \
                   ok = excluded.ok, why = excluded.why",
                params![id, json, now_ms() as i64, ok, why],
            )?;
            Ok(())
        })
    }

    /// Claims the CPU time of a yard's tasks up to `at_ms` and returns the end of the previous claim (0 — none). One
    /// interval is counted once however many reads see it: a poll, a cleanup and its re-read all come here.
    pub fn runner_usage_take(&self, id: &str, at_ms: u64) -> u64 {
        self.with("runner_usage_take", |c| {
            let tx = c.transaction()?;
            let prev: Option<i64> = tx
                .query_row("SELECT usage_at FROM runners_cache WHERE id = ?1", [id], |r| r.get(0))
                .optional()?;
            tx.execute(
                "INSERT INTO runners_cache (id, usage_at) VALUES (?1, ?2) \
                 ON CONFLICT(id) DO UPDATE SET usage_at = excluded.usage_at",
                params![id, at_ms as i64],
            )?;
            tx.commit()?;
            Ok(prev.unwrap_or(0).max(0) as u64)
        })
    }

    /// Forgets a runner that was removed from the list.
    pub fn runner_remove(&self, id: &str) {
        self.with("runner_remove", |c| {
            c.execute("DELETE FROM runners_cache WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// `(id, json, fetched_at, ok, why)` for every runner, by id.
    pub fn runner_all(&self) -> Vec<(String, String, u64, bool, String)> {
        self.with("runner_all", |c| {
            let mut st = c.prepare(
                "SELECT id, json, fetched_at, ok, why FROM runners_cache ORDER BY id",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? as u64,
                    r.get(3)?,
                    r.get(4)?,
                ))
            })?;
            rows.collect()
        })
    }

    // ---- what the shipyards did (PLN-46) ----

    /// A sample of a yard's load: `step` is the row width in seconds, `at` its start; `None` — not known.
    pub fn hist_put(&self, runner: &str, step: u64, at: u64, cpu: Option<f64>, mem: Option<f64>, disk: Option<f64>) {
        self.with("hist_put", |c| {
            c.execute(
                "INSERT OR REPLACE INTO runner_hist (runner, step, at, cpu, mem, disk) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![runner, step as i64, at as i64, cpu, mem, disk],
            )?;
            Ok(())
        })
    }

    /// `(step, at, cpu, mem, disk)` of a yard, oldest first, from `since` on.
    pub fn hist_rows(&self, runner: &str, since: u64) -> Vec<(u64, u64, Option<f64>, Option<f64>, Option<f64>)> {
        self.with("hist_rows", |c| {
            let mut st = c.prepare(
                "SELECT step, at, cpu, mem, disk FROM runner_hist WHERE runner = ?1 AND at >= ?2 ORDER BY at",
            )?;
            let rows = st.query_map(params![runner, since as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            })?;
            rows.collect()
        })
    }

    /// A hole of a minute: the yard did not answer then, so the line breaks instead of joining the reads around it.
    /// A known sample of the same minute is never replaced by it.
    pub fn hist_hole(&self, runner: &str, step: u64, at: u64) {
        self.with("hist_hole", |c| {
            c.execute(
                "INSERT OR IGNORE INTO runner_hist (runner, step, at, cpu, mem, disk) VALUES (?1, ?2, ?3, NULL, NULL, NULL)",
                params![runner, step as i64, at as i64],
            )?;
            Ok(())
        })
    }

    /// Narrows the minute samples older than `cut` into `step`-wide rows (the mean of the minutes) and drops them; then
    /// forgets the samples older than `keep`. `cut` must be a multiple of `step`: a wide row is folded once, whole.
    pub fn hist_fold(&self, runner: &str, cut: u64, step: u64, keep: u64) {
        self.with("hist_fold", |c| {
            let tx = c.transaction()?;
            tx.execute(
                "INSERT OR REPLACE INTO runner_hist (runner, step, at, cpu, mem, disk) \
                 SELECT ?1, ?3, (at / ?3) * ?3, avg(cpu), avg(mem), avg(disk) \
                 FROM runner_hist WHERE runner = ?1 AND step = 60 AND at < ?2 GROUP BY (at / ?3) * ?3",
                params![runner, cut as i64, step as i64],
            )?;
            tx.execute(
                "DELETE FROM runner_hist WHERE runner = ?1 AND step = 60 AND at < ?2",
                params![runner, cut as i64],
            )?;
            tx.execute(
                "DELETE FROM runner_hist WHERE runner = ?1 AND at < ?2",
                params![runner, keep as i64],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Every yard that has a history row, so a yard taken out of the list can take its rows with it.
    pub fn hist_runners(&self) -> Vec<String> {
        self.with("hist_runners", |c| {
            let mut st = c.prepare("SELECT runner FROM runner_hist UNION SELECT runner FROM task_usage")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// Forgets a yard's whole history (the yard is gone from the list).
    pub fn hist_remove(&self, runner: &str) {
        self.with("hist_remove", |c| {
            c.execute("DELETE FROM runner_hist WHERE runner = ?1", [runner])?;
            Ok(())
        })
    }

    /// Adds what the containers of `tag` ate at one read: CPU seconds, and the memory they held at once. The first
    /// and the last moments are kept; a peak is the highest one seen.
    pub fn usage_add(&self, runner: &str, tag: &str, task: &str, cpu_sec: f64, mem: u64, at_ms: u64) {
        self.with("usage_add", |c| {
            c.execute(
                "INSERT INTO task_usage (runner, tag, task, cpu_sec, mem_peak, first_ms, last_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) \
                 ON CONFLICT(runner, tag) DO UPDATE SET \
                   task = CASE WHEN excluded.task <> '' THEN excluded.task ELSE task END, \
                   cpu_sec = cpu_sec + excluded.cpu_sec, \
                   mem_peak = MAX(mem_peak, excluded.mem_peak), \
                   first_ms = MIN(CASE WHEN first_ms = 0 THEN excluded.first_ms ELSE first_ms END, excluded.first_ms), \
                   last_ms = MAX(last_ms, excluded.last_ms)",
                params![runner, tag, task, cpu_sec, mem as i64, at_ms as i64],
            )?;
            Ok(())
        })
    }

    /// `(tag, task, cpu_sec, mem_peak, first_ms, last_ms)` of a yard's tasks, the most CPU-hungry first.
    pub fn usage_rows(&self, runner: &str) -> Vec<(String, String, f64, u64, u64, u64)> {
        self.with("usage_rows", |c| {
            let mut st = c.prepare(
                "SELECT tag, task, cpu_sec, mem_peak, first_ms, last_ms FROM task_usage WHERE runner = ?1 \
                 ORDER BY cpu_sec DESC, tag",
            )?;
            let rows = st.query_map([runner], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i64>(3)? as u64,
                    r.get::<_, i64>(4)? as u64,
                    r.get::<_, i64>(5)? as u64,
                ))
            })?;
            rows.collect()
        })
    }

    /// Forgets a yard's task usage (the yard is gone from the list).
    pub fn usage_remove(&self, runner: &str) {
        self.with("usage_remove", |c| {
            c.execute("DELETE FROM task_usage WHERE runner = ?1", [runner])?;
            Ok(())
        })
    }

    /// Forgets the task rows nobody has seen since `before_ms` (a finished task's usage is kept a while, not for ever).
    pub fn usage_prune(&self, before_ms: u64) {
        self.with("usage_prune", |c| {
            c.execute("DELETE FROM task_usage WHERE last_ms < ?1", [before_ms as i64])?;
            Ok(())
        })
    }

    // ---- memory suggestions ----

    /// Reserves the suggestion of `task`; false when it was made (or is being made) already.
    pub fn suggest_claim(&self, task: &str, repo: &str, tag: &str, at: u64) -> bool {
        self.with("suggest_claim", |c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO memory_suggest (task, repo, tag, state, at) VALUES (?1, ?2, ?3, 'working', ?4)",
                params![task, repo, tag, at as i64],
            )?;
            Ok(n > 0)
        })
    }

    pub fn suggest_finish(&self, task: &str, state: &str, lines: &str) {
        self.with("suggest_finish", |c| {
            c.execute(
                "UPDATE memory_suggest SET state = ?2, lines = ?3 WHERE task = ?1",
                params![task, state, lines],
            )?;
            Ok(())
        })
    }

    /// Takes a waiting suggestion for saving: true for the one caller that finds it `pending` (a second, concurrent
    /// accept finds `saving` and gets false).
    pub fn suggest_take(&self, task: &str) -> bool {
        self.with("suggest_take", |c| {
            Ok(c.execute(
                "UPDATE memory_suggest SET state = 'saving' WHERE task = ?1 AND state = 'pending'",
                [task],
            )? > 0)
        })
    }

    /// After a restart: a suggestion that was being made is dropped (it can be made again), one that was being saved
    /// waits again.
    pub fn suggest_recover(&self) {
        self.with("suggest_recover", |c| {
            c.execute("DELETE FROM memory_suggest WHERE state = 'working'", [])?;
            c.execute("UPDATE memory_suggest SET state = 'pending' WHERE state = 'saving'", [])?;
            Ok(())
        })
    }

    pub fn suggest_set_state(&self, task: &str, state: &str) -> bool {
        self.with("suggest_set_state", |c| {
            Ok(c.execute(
                "UPDATE memory_suggest SET state = ?2 WHERE task = ?1",
                params![task, state],
            )? > 0)
        })
    }

    pub fn suggest_get(&self, task: &str) -> Option<SuggestRow> {
        self.with("suggest_get", |c| {
            c.query_row(
                "SELECT task, repo, tag, lines, state, at FROM memory_suggest WHERE task = ?1",
                [task],
                suggest_row,
            )
            .optional()
        })
    }

    /// The suggestions that wait for the human, oldest first.
    pub fn suggest_pending(&self) -> Vec<SuggestRow> {
        self.with("suggest_pending", |c| {
            let mut st = c.prepare(
                "SELECT task, repo, tag, lines, state, at FROM memory_suggest WHERE state = 'pending' ORDER BY at, task",
            )?;
            let rows = st.query_map([], suggest_row)?;
            rows.collect()
        })
    }

    // ---- search documents ----

    /// Makes the index equal to `docs`: documents that are no longer present are removed,
    /// the others are inserted or updated.
    pub fn search_docs_replace(&self, docs: &[SearchDoc]) {
        self.with("search_docs_replace", |c| {
            let tx = c.transaction()?;
            let wanted: HashSet<&str> = docs.iter().map(|d| d.key.as_str()).collect();
            let present: Vec<String> = {
                let mut st = tx.prepare("SELECT key FROM search_docs")?;
                let rows = st.query_map([], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for key in present.iter().filter(|k| !wanted.contains(k.as_str())) {
                tx.execute("DELETE FROM search_docs WHERE key = ?1", [key])?;
            }
            {
                let mut up = tx.prepare(
                    "INSERT INTO search_docs (key, kind, id, path, title, text, status, at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                     ON CONFLICT(key) DO UPDATE SET kind = excluded.kind, id = excluded.id, \
                     path = excluded.path, title = excluded.title, \
                     text = excluded.text, status = excluded.status, at = excluded.at",
                )?;
                for d in docs {
                    up.execute(params![
                        d.key, d.kind, d.id, d.path, d.title, d.text, d.status, d.at as i64
                    ])?;
                }
            }
            tx.commit()
        })
    }

    /// Documents matching every term of `q` (title or text), best first.
    pub fn search_docs_match(&self, q: &str, limit: usize) -> Vec<DocHit> {
        let terms = query_terms(q);
        if terms.is_empty() {
            return Vec::new();
        }
        let limit = limit.clamp(1, 500) as i64;
        self.with("search_docs_match", |c| {
            let (sql, args) = build_match(
                &terms,
                "search_docs",
                "search_fts",
                "rid",
                &["d.title", "d.text"],
                "d",
                limit,
            );
            let mut st = c.prepare(&sql)?;
            let rows = st.query_map(params_from_iter(args.iter()), |r| {
                Ok(DocHit {
                    doc: SearchDoc {
                        key: r.get(0)?,
                        kind: r.get(1)?,
                        id: r.get(2)?,
                        path: r.get(3)?,
                        title: r.get(4)?,
                        text: r.get(5)?,
                        status: r.get(6)?,
                        at: r.get::<_, i64>(7)? as u64,
                    },
                    rank: r.get(8)?,
                })
            })?;
            rows.collect()
        })
    }

    // ---- chat transcript index ----

    /// Stores turns by `n`, replacing the rows with the same numbers.
    pub fn chat_turns_upsert(&self, rows: &[ChatTurnRow]) {
        self.with("chat_turns_upsert", |c| upsert_turns(c, rows));
    }

    /// Removes every turn numbered `from` or later and stores `rows` in one transaction.
    pub fn chat_turns_replace_from(&self, from: usize, rows: &[ChatTurnRow]) {
        self.with("chat_turns_replace_from", |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM chat_turns WHERE n >= ?1", [from as i64])?;
            upsert_turns(&tx, rows)?;
            tx.commit()
        })
    }

    /// `(n, fingerprint)` of every indexed turn, by `n`. See [`chat_row_hash`].
    pub fn chat_turn_hashes(&self) -> Vec<(usize, String)> {
        self.with("chat_turn_hashes", |c| {
            let mut st = c.prepare("SELECT n, h FROM chat_turns ORDER BY n")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)? as usize, r.get(1)?)))?;
            rows.collect()
        })
    }

    pub fn chat_turns_max_n(&self) -> Option<usize> {
        self.with("chat_turns_max_n", |c| {
            let n: Option<i64> = c.query_row("SELECT MAX(n) FROM chat_turns", [], |r| r.get(0))?;
            Ok(n.map(|n| n as usize))
        })
    }

    /// Turns whose question or answer match every term of `q`, best first.
    pub fn chat_search(&self, q: &str, limit: usize) -> Vec<ChatHit> {
        let terms = query_terms(q);
        if terms.is_empty() {
            return Vec::new();
        }
        let limit = limit.clamp(1, 500) as i64;
        self.with("chat_search", |c| {
            let (sql, args) = build_match(
                &terms,
                "chat_turns",
                "chat_fts",
                "n",
                &["t.q", "t.a"],
                "t",
                limit,
            );
            let mut st = c.prepare(&sql)?;
            let rows = st.query_map(params_from_iter(args.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)? as usize,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, f64>(5)?,
                ))
            })?;
            let mut hits = Vec::new();
            for row in rows {
                let (n, at, from, q, a, rank) = row?;
                hits.push(ChatHit {
                    n,
                    at,
                    from,
                    q_snip: highlight(&q, &terms, 160),
                    a_snip: highlight(&a, &terms, 240),
                    q,
                    rank,
                });
            }
            Ok(hits)
        })
    }

    // ---- legacy import ----

    /// Imports `todo.json`, `journal.json` and `chat.json` from the state folder, once.
    /// Each imported file is renamed to `<name>.imported`; a file that is not valid
    /// JSON is renamed to `<name>.bad` and skipped. Returns one line per file acted on.
    pub fn import_legacy(&self, state_dir: &Path) -> Result<Vec<String>, String> {
        let mut c = self.conn();
        let mut log = Vec::new();
        import_todo(&mut c, state_dir, &mut log)?;
        import_journal(&mut c, state_dir, &mut log)?;
        import_chat(&mut c, state_dir, &mut log)?;
        Ok(log)
    }
}

// ---- helpers ----

fn db_err(e: rusqlite::Error) -> String {
    format!("database: {e}")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict(_: &Path, _: u32) {}

/// Fingerprint of a turn's content, stored next to it so a sync can tell which turns changed.
pub fn chat_row_hash(at: u64, from: &str, q: &str, a: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!("{at}\n{from}\n{}\n", q.len()).as_bytes());
    h.update(q.as_bytes());
    h.update(b"\n");
    h.update(a.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn migrate(conn: &mut Connection) -> Result<(), String> {
    let current: usize = conn
        .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .map_err(db_err)? as usize;
    if current > MIGRATIONS.len() {
        return Err(format!(
            "the planner database is from a newer version (schema {current})"
        ));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute_batch(sql)
            .map_err(|e| format!("migration {}: {e}", i + 1))?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
    }
    Ok(())
}

fn suggest_row(r: &Row) -> rusqlite::Result<SuggestRow> {
    Ok(SuggestRow {
        task: r.get(0)?,
        repo: r.get(1)?,
        tag: r.get(2)?,
        lines: r.get(3)?,
        state: r.get(4)?,
        at: r.get::<_, i64>(5)? as u64,
    })
}

fn journal_row(r: &Row) -> rusqlite::Result<JournalEntry> {
    Ok(JournalEntry {
        n: r.get::<_, i64>(0)? as u64,
        t: r.get::<_, i64>(1)? as u64,
        id: r.get(2)?,
        s: r.get(3)?,
        text: r.get(4)?,
    })
}

/// The larger of the stored sequence and the newest entry's number.
fn journal_seq_in(c: &Connection) -> rusqlite::Result<u64> {
    let stored: Option<u64> = c
        .query_row(
            "SELECT value FROM meta WHERE key = 'journal_seq'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| s.parse().ok());
    let newest: Option<i64> = c.query_row("SELECT MAX(n) FROM journal", [], |r| r.get(0))?;
    Ok(stored.unwrap_or(0).max(newest.unwrap_or(0) as u64))
}

fn trim_journal(c: &Connection) -> rusqlite::Result<()> {
    let seq = journal_seq_in(c)?;
    c.execute(
        "INSERT INTO meta (key, value) VALUES ('journal_seq', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [seq.to_string()],
    )?;
    c.execute(
        "DELETE FROM journal WHERE n NOT IN (SELECT n FROM journal ORDER BY n DESC LIMIT ?1)",
        [JOURNAL_KEEP as i64],
    )?;
    Ok(())
}

/// A stored history is a JSON list; a damaged one is dropped rather than failing the chat.
fn parse_history(text: &str) -> Vec<ChatMessage> {
    serde_json::from_str(text).unwrap_or_else(|e| {
        eprintln!("planner db: chat history is not readable, starting it empty: {e}");
        Vec::new()
    })
}

fn upsert_turns(c: &Connection, rows: &[ChatTurnRow]) -> rusqlite::Result<()> {
    let mut up = c.prepare(
        "INSERT INTO chat_turns (n, at, from_, q, a, h) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(n) DO UPDATE SET at = excluded.at, from_ = excluded.from_, \
         q = excluded.q, a = excluded.a, h = excluded.h",
    )?;
    for r in rows {
        let h = chat_row_hash(r.at, &r.from, &r.q, &r.a);
        up.execute(params![r.n as i64, r.at as i64, r.from, r.q, r.a, h])?;
    }
    Ok(())
}

/// Words of the query, at most 12 of them from the first 200 characters.
pub fn query_terms(q: &str) -> Vec<String> {
    q.chars()
        .take(200)
        .collect::<String>()
        .split_whitespace()
        .take(12)
        .map(str::to_string)
        .collect()
}

/// Escapes a term for `LIKE ... ESCAPE '\'`.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// The term in each case the characters can take (`ок` → `ок`, `ОК`, `Ок`, `оК`).
/// Only used for terms of 1–2 characters, so there are at most four.
fn case_variants(term: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for ch in term.chars() {
        let mut forms = vec![ch];
        for f in [
            ch.to_lowercase().next().unwrap_or(ch),
            ch.to_uppercase().next().unwrap_or(ch),
        ] {
            if !forms.contains(&f) {
                forms.push(f);
            }
        }
        out = out
            .iter()
            .flat_map(|p| forms.iter().map(move |f| format!("{p}{f}")))
            .collect();
    }
    out
}

/// A FTS5 phrase for each term of 3+ characters, joined with AND. The text is quoted,
/// so operators and punctuation in it (`"`, `*`, `(`, `OR`, `NEAR`) match literally.
fn fts_expression(terms: &[String]) -> Option<String> {
    let parts: Vec<String> = terms
        .iter()
        .filter(|t| t.chars().count() >= 3)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// One `LIKE` condition per short term (1–2 characters): the term must appear in one of `cols`.
fn like_conditions(terms: &[String], cols: &[&str], args: &mut Vec<Value>) -> Vec<String> {
    terms
        .iter()
        .map(|t| {
            let mut parts = Vec::new();
            for v in case_variants(t) {
                args.push(Value::Text(format!("%{}%", escape_like(&v))));
                let idx = args.len();
                for col in cols {
                    parts.push(format!("{col} LIKE ?{idx} ESCAPE '\\'"));
                }
            }
            format!("({})", parts.join(" OR "))
        })
        .collect()
}

/// The SQL for a search over `table` (with FTS table `fts` keyed by `rowid_col`).
///
/// Terms of 3+ characters go through the trigram index (`MATCH`); the others through
/// `LIKE`. Output columns, in order: the table's own columns (`d.*` for docs, `t.*`
/// for chat turns) and the rank. Rows are ordered by rank (or by recency when no
/// term reached the index), and limited to `limit`.
fn build_match(
    terms: &[String],
    table: &str,
    fts: &str,
    rowid_col: &str,
    like_cols: &[&str],
    alias: &str,
    limit: i64,
) -> (String, Vec<Value>) {
    let mut args: Vec<Value> = Vec::new();
    let mut conds: Vec<String> = Vec::new();
    let shorts: Vec<String> = terms
        .iter()
        .filter(|t| t.chars().count() < 3)
        .cloned()
        .collect();
    let (from, rank, order) = match fts_expression(terms) {
        Some(expr) => {
            args.push(Value::Text(expr));
            conds.push(format!("{fts} MATCH ?1"));
            (
                format!("{fts} JOIN {table} {alias} ON {alias}.{rowid_col} = {fts}.rowid"),
                format!("bm25({fts}, 4.0, 1.0)"),
                format!("score, {alias}.at DESC"),
            )
        }
        None => (
            format!("{table} {alias}"),
            "0.0".to_string(),
            format!("{alias}.at DESC, {alias}.rowid DESC"),
        ),
    };
    conds.extend(like_conditions(&shorts, like_cols, &mut args));
    let select = if table == "search_docs" {
        "d.key, d.kind, d.id, d.path, d.title, d.text, d.status, d.at"
    } else {
        "t.n, t.at, t.from_, t.q, t.a"
    };
    args.push(Value::Integer(limit));
    let sql = format!(
        "SELECT {select}, {rank} AS score FROM {from} WHERE {} ORDER BY {order} LIMIT ?{}",
        conds.join(" AND "),
        args.len()
    );
    (sql, args)
}

// ---- legacy import internals ----

fn legacy_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    p.is_file().then_some(p)
}

/// Renames a file aside (`<name>.imported` or `<name>.bad`).
fn retire(path: &Path, suffix: &str) -> Result<(), String> {
    let mut to = path.as_os_str().to_owned();
    to.push(suffix);
    std::fs::rename(path, &to).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads a legacy file. `Ok(None)` when absent; a file that cannot be read as
/// JSON is renamed to `.bad` and reported as `Ok(None)` too.
fn read_legacy<T: serde::de::DeserializeOwned>(
    dir: &Path,
    name: &str,
    log: &mut Vec<String>,
) -> Option<T> {
    let path = legacy_path(dir, name)?;
    let parsed = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<T>(&t).map_err(|e| e.to_string()));
    match parsed {
        Ok(v) => Some(v),
        Err(e) => {
            match retire(&path, ".bad") {
                Ok(()) => log.push(format!("{name}: not readable ({e}), kept as {name}.bad")),
                Err(r) => log.push(format!("{name}: not readable ({e}); {r}")),
            }
            None
        }
    }
}

fn finish_legacy(dir: &Path, name: &str, log: &mut Vec<String>, what: &str) {
    match retire(&dir.join(name), ".imported") {
        Ok(()) => log.push(format!("{name}: {what}, renamed to {name}.imported")),
        Err(e) => log.push(format!("{name}: {what}; could not rename ({e})")),
    }
}

fn import_todo(c: &mut Connection, dir: &Path, log: &mut Vec<String>) -> Result<(), String> {
    const NAME: &str = "todo.json";
    if legacy_path(dir, NAME).is_none() {
        return Ok(());
    }
    let Some(items) = read_legacy::<Vec<TodoItem>>(dir, NAME, log) else {
        return Ok(());
    };
    let tx = c.transaction().map_err(db_err)?;
    let base: i64 = tx
        .query_row("SELECT COALESCE(MAX(ord), -1) + 1 FROM todo", [], |r| {
            r.get(0)
        })
        .map_err(db_err)?;
    let mut added = 0;
    for (i, it) in items.iter().enumerate() {
        added += tx
            .execute(
                "INSERT OR IGNORE INTO todo (id, text, at, ord, to_) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![it.id, it.text, it.at as i64, base + i as i64, it.to],
            )
            .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    finish_legacy(dir, NAME, log, &format!("{added} items imported"));
    Ok(())
}

/// The shape journal.json was written in.
#[derive(Deserialize, Default)]
struct LegacyJournal {
    #[serde(default)]
    seq: u64,
    #[serde(default)]
    entries: Vec<JournalEntry>,
}

fn import_journal(c: &mut Connection, dir: &Path, log: &mut Vec<String>) -> Result<(), String> {
    const NAME: &str = "journal.json";
    if legacy_path(dir, NAME).is_none() {
        return Ok(());
    }
    let Some(saved) = read_legacy::<LegacyJournal>(dir, NAME, log) else {
        return Ok(());
    };
    let tx = c.transaction().map_err(db_err)?;
    let mut added = 0;
    for e in &saved.entries {
        added += tx
            .execute(
                "INSERT OR IGNORE INTO journal (n, t, id, s, text) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![e.n as i64, e.t as i64, e.id, e.s, e.text],
            )
            .map_err(db_err)?;
    }
    let seq = journal_seq_in(&tx).map_err(db_err)?.max(saved.seq);
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('journal_seq', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [seq.to_string()],
    )
    .map_err(db_err)?;
    trim_journal(&tx).map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    finish_legacy(dir, NAME, log, &format!("{added} entries imported"));
    Ok(())
}

fn import_chat(c: &mut Connection, dir: &Path, log: &mut Vec<String>) -> Result<(), String> {
    const NAME: &str = "chat.json";
    if legacy_path(dir, NAME).is_none() {
        return Ok(());
    }
    let Some(chat) = read_legacy::<ChatState>(dir, NAME, log) else {
        return Ok(());
    };
    let existing: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_state WHERE id = 1)",
            [],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if existing {
        finish_legacy(dir, NAME, log, "kept the chat already in the database");
        return Ok(());
    }
    let history = serde_json::to_string(&chat.history).unwrap_or_else(|_| "[]".into());
    c.execute(
        "INSERT INTO chat_state (id, last_sent, sent_at, last_reply, history) \
         VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            chat.last_sent,
            chat.sent_at as i64,
            chat.last_reply,
            history
        ],
    )
    .map_err(db_err)?;
    finish_legacy(dir, NAME, log, "chat imported");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u64, text: &str) -> JournalEntry {
        JournalEntry {
            n,
            t: 1000 + n,
            id: format!("ALP-{n}"),
            s: "done".into(),
            text: text.into(),
        }
    }

    fn doc(key: &str, title: &str, text: &str) -> SearchDoc {
        SearchDoc {
            key: key.into(),
            kind: "planet".into(),
            id: key.into(),
            path: "alpha › ALP".into(),
            title: title.into(),
            text: text.into(),
            status: "идёт".into(),
            at: 42,
        }
    }

    fn turn(n: usize, q: &str, a: &str) -> ChatTurnRow {
        ChatTurnRow {
            n,
            at: 1_000 + n as u64,
            from: "human".into(),
            q: q.into(),
            a: a.into(),
        }
    }

    fn titles(hits: &[DocHit]) -> Vec<&str> {
        hits.iter().map(|h| h.doc.title.as_str()).collect()
    }

    #[test]
    fn trigram_finds_russian_substrings_in_any_case() {
        let db = Db::memory().unwrap();
        db.search_docs_replace(
            &[
                doc("a", "Удалить Задачу", "Текст про базу"),
                doc("b", "Другое", ""),
            ],
        );
        db.chat_turns_upsert(&[turn(0, "Удали все Задачи", "Удалил шесть задач.")]);
        assert_eq!(
            titles(&db.search_docs_match("задач", 10)),
            vec!["Удалить Задачу"]
        );
        assert_eq!(
            db.search_docs_match("ЗАДАЧ", 10).len(),
            1,
            "case does not matter"
        );
        assert_eq!(db.chat_search("задач", 10).len(), 1);
        assert_eq!(
            db.chat_search("шесть задач", 10).len(),
            1,
            "terms are ANDed"
        );
        assert!(db.chat_search("шесть базу", 10).is_empty());
    }

    #[test]
    fn short_terms_fall_back_to_like_in_any_case() {
        let db = Db::memory().unwrap();
        db.search_docs_replace(
            &[
                doc("a", "Яблоко", "OK готово"),
                doc("b", "Груша", ""),
            ],
        );
        assert_eq!(
            titles(&db.search_docs_match("яб", 10)),
            vec!["Яблоко"],
            "2 chars: LIKE"
        );
        assert_eq!(titles(&db.search_docs_match("яБ", 10)), vec!["Яблоко"]);
        assert_eq!(db.search_docs_match("ok", 10).len(), 1);
        assert_eq!(db.search_docs_match("Ok", 10).len(), 1);
        assert!(db.search_docs_match("м", 10).is_empty());
        assert_eq!(
            db.search_docs_match("ко", 10).len(),
            1,
            "matches inside text"
        );
        // A mixed query: the long term goes through the index, the short one through LIKE.
        assert_eq!(
            titles(&db.search_docs_match("яблоко g", 10)),
            vec![] as Vec<&str>
        );
        assert_eq!(
            titles(&db.search_docs_match("яблоко о", 10)),
            vec!["Яблоко"]
        );
    }

    #[test]
    fn search_text_is_literal_not_fts_syntax() {
        let db = Db::memory().unwrap();
        db.search_docs_replace(
            &[doc("a", "задача OR NEAR(x) * ^", "a \"quoted\" word")],
        );
        db.chat_turns_upsert(&[turn(0, "что (с) задачей?", "ответ")]);
        for q in [
            "\"",
            "\"\"",
            "*",
            "***",
            "))",
            "(",
            "OR",
            "AND",
            "NOT",
            "NEAR(",
            "NEAR(задач",
            "a\" OR \"b",
            "задач\"*",
            "^",
            ":",
            "-",
            "%",
            "_",
            "\\",
            "задач)",
            "задач NOT",
            "  ",
            "\u{0}",
            "\"quoted\"",
            "x".repeat(500).as_str(),
        ] {
            let _ = db.search_docs_match(q, 10);
            let _ = db.chat_search(q, 10);
        }
        assert_eq!(
            db.search_docs_match("\"", 10).len(),
            1,
            "a quote is an ordinary character"
        );
        assert!(db.search_docs_match("***", 10).is_empty());
        assert_eq!(
            db.search_docs_match("OR", 10).len(),
            1,
            "OR is an ordinary word"
        );
        assert_eq!(db.search_docs_match("\"quoted\"", 10).len(), 1);
        assert_eq!(db.search_docs_match("NEAR(x)", 10).len(), 1);
        assert_eq!(db.chat_search("(с)", 10).len(), 1);
        assert!(db.chat_search("задач) OR zzzz", 10).is_empty());
        assert_eq!(db.chat_search("%", 10).len(), 0);
        assert_eq!(
            db.search_docs_match("%", 10).len(),
            0,
            "LIKE wildcards are escaped"
        );
        assert!(db.chat_search("  ", 10).is_empty());
    }

    #[test]
    fn a_todo_item_keeps_its_files() {
        let db = Db::memory().unwrap();
        let a = db.todo_add("со скриншотом", 1);
        assert!(db.todo_list()[0].attachments.is_empty());
        let file = Attachment {
            id: "0123456789abcdef0123456789abcdef.png".into(),
            name: "shot.png".into(),
            mime: "image/png".into(),
            size: 8,
        };
        assert!(db.todo_files(&a.id, std::slice::from_ref(&file)));
        assert_eq!(db.todo_list()[0].attachments, vec![file]);
        assert!(!db.todo_files("missing", &[]));
        assert!(db.todo_files(&a.id, &[]));
        assert!(db.todo_list()[0].attachments.is_empty());
    }

    #[test]
    fn migrations_run_once_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("planner.sqlite");
        {
            let db = Db::open(&path).unwrap();
            db.todo_add("раз", 1);
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.todo_list().len(), 1, "data survives a reopen");
        let v: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        let mode: String = db
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(m, 0o600);
            let d = std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(d, 0o700);
        }
    }

    #[test]
    fn memory_suggestions_are_made_once_and_wait_for_the_human() {
        let db = Db::memory().unwrap();
        assert!(db.suggest_claim("a/1", "a", "A-1", 10));
        assert!(!db.suggest_claim("a/1", "a", "A-1", 11), "once per task, even while it is being made");
        assert!(db.suggest_pending().is_empty(), "a suggestion being made is not shown yet");
        db.suggest_finish("a/1", "pending", r#"[{"kind":"decision","text":"x"}]"#);
        assert!(db.suggest_claim("b/2", "b", "B-2", 20));
        db.suggest_finish("b/2", "none", "[]");
        let waiting = db.suggest_pending();
        assert_eq!(waiting.len(), 1);
        assert_eq!((waiting[0].task.as_str(), waiting[0].repo.as_str(), waiting[0].tag.as_str()), ("a/1", "a", "A-1"));
        assert!(waiting[0].lines.contains("decision"));
        assert!(db.suggest_take("a/1"), "the first accept takes it");
        assert!(!db.suggest_take("a/1"), "a second, concurrent one does not");
        assert!(db.suggest_pending().is_empty(), "a suggestion being saved is not offered again");
        db.suggest_set_state("a/1", "pending");
        assert_eq!(db.suggest_pending().len(), 1, "a failed save puts it back");
        assert!(db.suggest_claim("c/3", "c", "C-3", 40));
        assert!(db.suggest_take("a/1"));
        db.suggest_recover();
        assert!(db.suggest_get("c/3").is_none(), "after a restart one that was being made is made again later");
        assert_eq!(db.suggest_get("a/1").unwrap().state, "pending", "one that was being saved waits again");
        assert!(db.suggest_set_state("a/1", "done"));
        assert!(db.suggest_pending().is_empty());
        assert_eq!(db.suggest_get("a/1").unwrap().state, "done");
        assert!(!db.suggest_claim("a/1", "a", "A-1", 30), "a kept suggestion is not made again");
        assert!(!db.suggest_set_state("zzz", "done"));
        assert!(db.suggest_get("zzz").is_none());
    }

    #[test]
    fn an_old_database_gets_the_suggestion_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sqlite");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.execute_batch("PRAGMA user_version = 1").unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert!(db.suggest_claim("a/1", "a", "A-1", 1), "migration 2 ran on the old file");
    }

    #[test]
    fn a_database_from_before_the_far_galaxies_went_forgets_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sqlite");
        {
            let c = Connection::open(&path).unwrap();
            for sql in &MIGRATIONS[..3] {
                c.execute_batch(sql).unwrap();
            }
            c.execute_batch(
                "INSERT INTO galaxies (label, json, fetched_at, ok) VALUES ('vm-b', '{}', 1, 1);
                 INSERT INTO gitlag (galaxy, repo, json, at) VALUES ('', 'r', '{}', 1);
                 INSERT INTO search_docs (key, kind, id, galaxy, title) VALUES ('p::a', 'planet', 'a', '', 'Свой мост');
                 INSERT INTO search_docs (key, kind, id, galaxy, title) VALUES ('p:vm-b:a', 'planet', 'a', 'vm-b', 'Чужой мост');
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let tables: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('galaxies', 'gitlag')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "the cache of the other machines is dropped");
        let hits = db.search_docs_match("мост", 10);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].doc.title, "Свой мост");
        db.conn()
            .execute(
                "INSERT INTO search_fts(search_fts, rank) VALUES ('integrity-check', 1)",
                [],
            )
            .expect("the FTS index matches its table");
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sqlite");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch("PRAGMA user_version = 99").unwrap();
        }
        assert!(Db::open(&path).is_err());
    }

    #[test]
    fn todo_order_add_update_remove_reorder_assign() {
        let db = Db::memory().unwrap();
        let a = db.todo_add("первая", 1);
        let b = db.todo_add("вторая", 2);
        let c = db.todo_add("третья", 3);
        assert!(a.id.starts_with('t') && a.id != b.id && b.id != c.id);
        assert_eq!(a.to, "");
        let ids = |db: &Db| db.todo_list().into_iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids(&db), vec![a.id.clone(), b.id.clone(), c.id.clone()]);

        db.todo_reorder(&[c.id.clone(), "nope".into(), a.id.clone()]);
        assert_eq!(
            ids(&db),
            vec![c.id.clone(), a.id.clone(), b.id.clone()],
            "unlisted keep their order after"
        );

        assert!(db.todo_assign(&b.id, "quasar"));
        assert!(db.todo_assign(&a.id, "APC-7"));
        assert!(!db.todo_assign("missing", "quasar"));
        let list = db.todo_list();
        assert_eq!(list[2].to, "quasar");
        assert_eq!(list[1].to, "APC-7");

        assert!(db.todo_update(&a.id, "новый текст"));
        assert_eq!(db.todo_list()[1].text, "новый текст");
        assert!(!db.todo_update("missing", "x"));

        assert!(db.todo_remove(&c.id));
        assert!(!db.todo_remove(&c.id));
        assert_eq!(ids(&db), vec![a.id.clone(), b.id.clone()]);
        let d = db.todo_add("четвёртая", 4);
        assert_eq!(ids(&db).last(), Some(&d.id), "a new item goes to the end");
    }

    #[test]
    fn journal_keeps_500_and_serves_80_newest() {
        let db = Db::memory().unwrap();
        let all: Vec<JournalEntry> = (1..=600).map(|n| entry(n, "x")).collect();
        db.journal_append(&all[..300]);
        db.journal_append(&all[300..]);
        let (seq, newest) = db.journal_load();
        assert_eq!(seq, 600);
        assert_eq!(newest.len(), JOURNAL_LIVE);
        assert_eq!(newest[0].n, 600, "newest first");
        assert_eq!(newest.last().unwrap().n, 521);
        let rows: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM journal", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, JOURNAL_KEEP as i64);
        let oldest: i64 = db
            .conn()
            .query_row("SELECT MIN(n) FROM journal", [], |r| r.get(0))
            .unwrap();
        assert_eq!(oldest, 101);
    }

    #[test]
    fn chat_state_roundtrip_with_history() {
        let db = Db::memory().unwrap();
        assert_eq!(db.chat_load(), ChatState::default());
        let chat = ChatState {
            last_sent: "привет".into(),
            sent_at: 5,
            last_reply: "здравствуй".into(),
            history: vec![ChatMessage {
                text: "привет".into(),
                at: 5,
                attachments: vec![crate::attachments::Attachment {
                    id: "0".repeat(32) + ".png",
                    name: "shot.png".into(),
                    mime: "image/png".into(),
                    size: 9,
                }],
            }],
            busy_until: 0,
        };
        db.chat_save(&chat);
        assert_eq!(db.chat_load(), chat);
    }

    #[test]
    fn status_note_writes_only_changes() {
        let db = Db::memory().unwrap();
        assert_eq!(db.status_since("ALP-1"), None);
        db.status_note("ALP-1", "идёт", 10);
        db.status_note("ALP-1", "идёт", 20);
        db.status_note("ALP-2", "идёт", 25);
        assert_eq!(db.status_since("ALP-1"), Some(("идёт".into(), 10)));
        db.status_note("ALP-1", "готово", 30);
        db.status_note("ALP-1", "идёт", 40);
        assert_eq!(db.status_since("ALP-1"), Some(("идёт".into(), 40)));
        let rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM status_history WHERE task = 'ALP-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 3);
    }

    #[test]
    fn translations_and_meta() {
        let db = Db::memory().unwrap();
        assert_eq!(db.translation_get("h"), None);
        db.translation_put("h", "перевод", 1);
        db.translation_put("h", "перевод 2", 2);
        assert_eq!(db.translation_get("h").as_deref(), Some("перевод 2"));
        for i in 0..505 {
            db.translation_put(&format!("k{i}"), "t", 100 + i);
        }
        let n: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM translations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, TRANSLATIONS_KEEP as i64);
        assert_eq!(db.translation_get("h"), None, "the oldest went");

        assert_eq!(db.meta_get("k"), None);
        db.meta_set("k", "1");
        db.meta_set("k", "2");
        assert_eq!(db.meta_get("k").as_deref(), Some("2"));
    }

    #[test]
    fn search_docs_replace_removes_gone_docs_and_keeps_fts_in_step() {
        let db = Db::memory().unwrap();
        db.search_docs_replace(&[
            doc("p1", "Мост", "старый текст"),
            doc("p2", "Дорога", ""),
            doc("local", "Мост локальный", ""),
        ]);
        assert_eq!(db.search_docs_match("мост", 10).len(), 2);

        // The board changed: p2 is gone, p1 has a new text.
        db.search_docs_replace(&[doc("p1", "Мост", "новый текст"), doc("local", "Мост локальный", "")]);
        assert!(db.search_docs_match("дорога", 10).is_empty());
        assert!(
            db.search_docs_match("старый", 10).is_empty(),
            "updated text is no longer indexed"
        );
        assert_eq!(db.search_docs_match("новый", 10).len(), 1);
        assert_eq!(db.search_docs_match("мост", 10).len(), 2);
        db.conn()
            .execute(
                "INSERT INTO search_fts(search_fts, rank) VALUES ('integrity-check', 1)",
                [],
            )
            .expect("the FTS index matches its table");
    }

    #[test]
    fn doc_search_ranks_title_hits_first() {
        let db = Db::memory().unwrap();
        db.search_docs_replace(
            &[
                doc("body", "Другое", "здесь упомянут бюджет и бюджет"),
                doc("title", "Бюджет проекта", "нет"),
            ],
        );
        let hits = db.search_docs_match("бюджет", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].doc.key, "title");
        assert!(hits[0].rank <= hits[1].rank, "bm25: lower is better");
    }

    #[test]
    fn chat_turns_index_follows_the_numbering_and_the_fts() {
        let db = Db::memory().unwrap();
        db.chat_turns_replace_from(
            0,
            &[
                turn(0, "первый вопрос", "ответ один"),
                turn(1, "второй", ""),
            ],
        );
        assert_eq!(db.chat_turns_max_n(), Some(1));
        assert_eq!(db.chat_search("ответ", 10).len(), 1);

        // The last answer is written while the Coordinator works.
        db.chat_turns_replace_from(1, &[turn(1, "второй", "ответ два про пиццу")]);
        assert_eq!(db.chat_search("пиццу", 10).len(), 1);
        db.chat_turns_upsert(&[turn(1, "второй", "совсем другое")]);
        assert!(
            db.chat_search("пиццу", 10).is_empty(),
            "replaced text leaves the index"
        );
        assert_eq!(db.chat_search("совсем", 10).len(), 1);

        // Turns from n on are dropped (the transcript shrank).
        db.chat_turns_replace_from(0, &[turn(0, "первый вопрос", "ответ один")]);
        assert_eq!(db.chat_turn_hashes().len(), 1);
        assert_eq!(db.chat_turns_max_n(), Some(0));
        assert!(db.chat_search("совсем", 10).is_empty());
        assert_eq!(
            db.chat_turn_hashes()[0].1,
            chat_row_hash(1_000, "human", "первый вопрос", "ответ один")
        );
        db.conn()
            .execute(
                "INSERT INTO chat_fts(chat_fts, rank) VALUES ('integrity-check', 1)",
                [],
            )
            .expect("the chat FTS index matches its table");
    }

    #[test]
    fn chat_hits_carry_escaped_marked_snippets() {
        let db = Db::memory().unwrap();
        db.chat_turns_upsert(&[turn(
            0,
            "<b>что</b> & \"как\"",
            "Сделал <script>alert(1)</script> и вывел отчёт по задачам & бюджету.",
        )]);
        let hits = db.chat_search("задачам", 5);
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!(h.n, 0);
        assert_eq!(h.from, "human");
        assert!(h.a_snip.contains("<mark>задачам</mark>"), "{}", h.a_snip);
        assert!(!h.a_snip.contains("<script>"), "{}", h.a_snip);
        assert!(h.a_snip.contains("&lt;script&gt;"));
        let q = db.chat_search("как", 5);
        assert!(q[0].q_snip.contains("<mark>"), "{}", q[0].q_snip);
        assert!(q[0].q_snip.contains("&lt;b&gt;"), "{}", q[0].q_snip);
        assert!(q[0].q_snip.contains("&amp;"), "{}", q[0].q_snip);
        assert!(q[0].q_snip.contains("&quot;"), "{}", q[0].q_snip);
    }

    #[test]
    fn legacy_files_are_imported_once_and_bad_ones_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        std::fs::write(
            state.join("todo.json"),
            r#"[{"id":"t1-0","text":"первая","at":5},{"id":"t2-0","text":"вторая","at":6,"to":"quasar"}]"#,
        )
        .unwrap();
        std::fs::write(
            state.join("journal.json"),
            r#"{"seq":7,"entries":[{"n":7,"t":70,"id":"ALP-1","s":"done","text":"сделано"},{"n":6,"t":60,"id":"ALP-2","s":"idle","text":"ждёт"}]}"#,
        )
        .unwrap();
        std::fs::write(
            state.join("chat.json"),
            r#"{"last_sent":"привет","sent_at":9,"last_reply":"ок","history":[{"text":"привет","at":9}]}"#,
        )
        .unwrap();
        let db = Db::memory().unwrap();
        let log = db.import_legacy(state).unwrap();
        assert_eq!(log.len(), 3, "{log:?}");
        assert!(!state.join("todo.json").exists());
        assert!(state.join("todo.json.imported").exists());

        let todo = db.todo_list();
        assert_eq!(
            todo.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["t1-0", "t2-0"]
        );
        assert_eq!(todo[1].to, "quasar");
        let (seq, newest) = db.journal_load();
        assert_eq!(seq, 7);
        assert_eq!(newest.iter().map(|e| e.n).collect::<Vec<_>>(), vec![7, 6]);
        let chat = db.chat_load();
        assert_eq!((chat.last_sent.as_str(), chat.sent_at), ("привет", 9));
        assert_eq!(chat.history.len(), 1);

        // A second start finds nothing to do and changes nothing.
        assert!(db.import_legacy(state).unwrap().is_empty());
        assert_eq!(db.todo_list().len(), 2);
        assert_eq!(db.journal_load().1.len(), 2);
    }

    #[test]
    fn garbage_legacy_json_does_not_abort_the_import() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        std::fs::write(state.join("todo.json"), "{not json").unwrap();
        std::fs::write(state.join("journal.json"), "").unwrap();
        std::fs::write(
            state.join("chat.json"),
            r#"{"last_sent":"x","sent_at":1,"last_reply":"","history":"nope"}"#,
        )
        .unwrap();
        let db = Db::memory().unwrap();
        let log = db.import_legacy(state).unwrap();
        assert!(state.join("todo.json.bad").exists());
        assert!(state.join("journal.json.bad").exists());
        assert!(state.join("chat.json.bad").exists(), "{log:?}");
        assert!(db.todo_list().is_empty());
        assert_eq!(db.chat_load(), ChatState::default());
    }

    #[test]
    fn an_existing_chat_is_not_overwritten_by_the_legacy_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        let db = Db::memory().unwrap();
        db.chat_save(&ChatState {
            last_sent: "в базе".into(),
            ..Default::default()
        });
        std::fs::write(
            state.join("chat.json"),
            r#"{"last_sent":"в файле","sent_at":1,"last_reply":""}"#,
        )
        .unwrap();
        db.import_legacy(state).unwrap();
        assert_eq!(db.chat_load().last_sent, "в базе");
        assert!(state.join("chat.json.imported").exists());
    }

    #[test]
    fn query_helpers_escape_and_expand() {
        assert_eq!(escape_like("a%_\\b"), "a\\%\\_\\\\b");
        assert_eq!(case_variants("ок"), vec!["ок", "оК", "Ок", "ОК"]);
        assert_eq!(fts_expression(&["ab".into()]), None);
        assert_eq!(fts_expression(&["x\"y".into()]).unwrap(), "\"x\"\"y\"");
        assert_eq!(
            fts_expression(&["abc".into(), "x\"yz".into()]).unwrap(),
            "\"abc\" \"x\"\"yz\""
        );
        assert_eq!(query_terms("  a  b\tc "), vec!["a", "b", "c"]);
    }
}
