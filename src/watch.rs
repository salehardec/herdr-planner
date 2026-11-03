//! File watching (notify): the board changed, read it again. The page is told
//! over the event stream; nothing polls every few seconds any more.
//!
//! Watched: the tasks' own records (`task.json`, `closing.md`), the Coordinator's
//! state, the board's repo lists, the Coordinator's transcript, the limits records
//! and the theme. A change only pokes the refresh loop (which merges bursts).
//!
//! The Coordinator's transcript folder is not fixed: a first session or a change of
//! program makes it after the planner has started. The plan of watch folders is
//! therefore built again after each event, and the thread that owns the watcher adds
//! the folders that have appeared (the callback cannot: it is inside the debouncer).
//! A new conversation folder is indexed at once — its first records may have been
//! written before its watch existed.

use crate::app::Shared;
use crate::{config, history, theme};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Keeps the watcher alive; dropping it stops the thread that owns it.
pub struct Guard {
    stop: Arc<AtomicBool>,
    wake: mpsc::Sender<()>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.wake.send(());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Board,
    Chat,
    Limits,
    Theme,
    /// A memory entry (the Galaxy's folder, or a universe's `docs/memory`).
    Memory,
}

/// What a changed path means, or nothing (noise such as logs and locks). `chat_dirs` are the folders
/// with the Coordinator's transcripts: they are not always inside one root (see `history::watch_plan`).
pub fn classify(path: &Path, chat_dirs: &[PathBuf], theme_dir: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?;
    if chat_dirs.iter().any(|d| path.starts_with(d)) {
        return (name.ends_with(".jsonl")).then_some(Kind::Chat);
    }
    if path.starts_with(theme_dir) {
        return Some(Kind::Theme);
    }
    if is_memory_file(path) {
        return Some(Kind::Memory);
    }
    if matches!(name, "task.json" | "closing.md" | "state.json" | "repos.json" | "repo-keys.json" | "board.json") {
        return Some(Kind::Board);
    }
    if name.ends_with(".json") && (path.to_string_lossy().contains("agent-usage") || path.to_string_lossy().contains("agents/usage")) {
        return Some(Kind::Limits);
    }
    None
}

/// `…/memory/galaxy/x.md` or `…/docs/memory/x.md`.
fn is_memory_file(path: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return false;
    }
    let dir = |p: Option<&Path>| p.and_then(|p| p.file_name()).and_then(|n| n.to_str()).map(String::from);
    let parent = path.parent();
    let (a, b) = (dir(parent), dir(parent.and_then(Path::parent)));
    matches!((b.as_deref(), a.as_deref()), (Some("memory"), Some("galaxy")) | (Some("docs"), Some("memory")))
}

/// Adds the folders of `plan` that are not watched yet. Returns the new conversation folders: their
/// first records may have been written before their watch, so the caller indexes them at once.
fn add_watches(debouncer: &mut Debouncer<RecommendedWatcher>, plan: &[history::Watch], known: &mut HashSet<PathBuf>) -> Vec<PathBuf> {
    let mut fresh = Vec::new();
    for w in plan {
        if known.contains(&w.path) {
            continue;
        }
        let mode = if w.deep { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        // A folder that is gone by now is tried again on a later event: it is not marked as watched.
        if debouncer.watcher().watch(&w.path, mode).is_err() {
            continue;
        }
        known.insert(w.path.clone());
        if w.chat {
            fresh.push(w.path.clone());
        }
    }
    fresh
}

/// The conversation folders of the plan, for [`classify`].
fn chat_dirs(plan: &[history::Watch]) -> Vec<PathBuf> {
    plan.iter().filter(|w| w.chat && w.path.is_dir()).map(|w| w.path.clone()).collect()
}

/// Starts the watcher; the returned guard must be kept alive. Folders that do not exist (yet) are
/// watched through the closest folder above them, and the folders made later are added as they appear.
pub fn start(app: Shared) -> Option<Guard> {
    let cfg = app.cfg.clone();
    let theme_dir = theme::theme_dir();
    let handle = tokio::runtime::Handle::current();
    let handle2 = handle.clone();
    let app2 = app.clone();
    let theme2 = theme_dir.clone();

    // The folders the callback treats as the Coordinator's conversation: the thread below keeps this
    // list up to date, the callback reads it for every event.
    let plan = history::watch_plan(&cfg);
    let chat = Arc::new(Mutex::new(chat_dirs(&plan)));
    let chat_now = chat.clone();

    // The callback runs inside the debouncer and cannot reach its watcher: it asks the thread that owns
    // the debouncer to build the plan again. There is an ask for every batch — a folder may have
    // appeared since the last one, and nothing else would say so.
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let wake = wake_tx.clone();
    let mut debouncer = new_debouncer(Duration::from_millis(300), move |res: DebounceEventResult| {
        let _ = wake.send(());
        let Ok(events) = res else { return };
        let dirs = chat_now.lock().map(|d| d.clone()).unwrap_or_default();
        let mut board = false;
        let mut chat = false;
        let mut memory = false;
        for e in events {
            match classify(&e.path, &dirs, &theme2) {
                Some(Kind::Memory) => {
                    memory = true;
                    board = true;
                }
                Some(Kind::Chat) => {
                    chat = true;
                    board = true;
                }
                Some(_) => board = true,
                None => {}
            }
        }
        if chat {
            app2.bus.send("chat", "1".into());
            let app3 = app2.clone();
            handle.spawn(async move { crate::app::index_chat(&app3).await });
        }
        if memory {
            // a file put in by hand (or by an agent): its table of contents and the search index follow at once
            let app3 = app2.clone();
            handle.spawn(async move {
                crate::memory_api::reindex_stores(&app3).await;
                crate::app::index_snapshot(&app3).await;
            });
        }
        if board {
            app2.poke.notify_one();
        }
    })
    .ok()?;
    let mut known = HashSet::new();
    add_watches(&mut debouncer, &plan, &mut known);
    let mut watch = |path: PathBuf, mode: RecursiveMode| {
        if path.exists() {
            let _ = debouncer.watcher().watch(&path, mode);
        }
    };
    watch(cfg.data_dir.join("tasks"), RecursiveMode::Recursive);
    watch(cfg.data_dir.join("coordinator/default"), RecursiveMode::NonRecursive);
    watch(cfg.data_dir.clone(), RecursiveMode::NonRecursive);
    watch(crate::memory::galaxy_dir(&cfg), RecursiveMode::NonRecursive);
    for r in crate::memory::universes(&cfg) {
        watch(std::path::Path::new(&r.root).join(crate::memory::UNIVERSE_DIR), RecursiveMode::NonRecursive);
    }
    watch(theme_dir, RecursiveMode::NonRecursive);
    let state = config::state_home().join("omarchy/agents/usage");
    watch(state, RecursiveMode::NonRecursive);
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| config::home().join(".cache"))
        .join("omarchy/agent-usage");
    watch(cache, RecursiveMode::NonRecursive);

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let cfg2 = cfg.clone();
    let app3 = app.clone();
    let chat3 = chat;
    std::thread::spawn(move || {
        // The debouncer lives here: `watch()` is called between events, never from its callback.
        let mut debouncer = debouncer;
        while !stop2.load(Ordering::Relaxed) {
            if wake_rx.recv().is_err() {
                return;
            }
            if stop2.load(Ordering::Relaxed) {
                return;
            }
            let plan = history::watch_plan(&cfg2);
            if let Ok(mut dirs) = chat3.lock() {
                *dirs = chat_dirs(&plan);
            }
            let fresh = add_watches(&mut debouncer, &plan, &mut known);
            if !fresh.is_empty() {
                // A conversation folder appeared (a first session, another program): what is already in
                // it may be newer than the last index, and its watch waits for the writes going on now.
                app3.bus.send("chat", "1".into());
                let app4 = app3.clone();
                handle2.spawn(async move { crate::app::index_chat(&app4).await });
            }
        }
    });
    Some(Guard { stop, wake: wake_tx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_meaningful_files_count() {
        let chat = vec![PathBuf::from("/c/chat")];
        let theme = Path::new("/t");
        assert_eq!(classify(Path::new("/d/tasks/a/1/task.json"), &chat, theme), Some(Kind::Board));
        assert_eq!(classify(Path::new("/d/tasks/a/1/closing.md"), &chat, theme), Some(Kind::Board));
        assert_eq!(classify(Path::new("/d/tasks/a/1/runner.log"), &chat, theme), None);
        assert_eq!(classify(Path::new("/d/keeper.log"), &chat, theme), None);
        assert_eq!(classify(Path::new("/d/repos.json"), &chat, theme), Some(Kind::Board));
        assert_eq!(classify(Path::new("/c/chat/s.jsonl"), &chat, theme), Some(Kind::Chat));
        assert_eq!(classify(Path::new("/c/chat/notes.txt"), &chat, theme), None);
        assert_eq!(classify(Path::new("/t/colors.toml"), &chat, theme), Some(Kind::Theme));
        assert_eq!(classify(Path::new("/h/.cache/omarchy/agent-usage/codex-limits.json"), &chat, theme), Some(Kind::Limits));
        assert_eq!(classify(Path::new("/h/.herdr-projects/memory/galaxy/gitlab.md"), &chat, theme), Some(Kind::Memory));
        assert_eq!(classify(Path::new("/p/repo/docs/memory/decision.md"), &chat, theme), Some(Kind::Memory));
        assert_eq!(classify(Path::new("/p/repo/docs/other/decision.md"), &chat, theme), None);
        assert_eq!(classify(Path::new("/p/repo/docs/memory/x.txt"), &chat, theme), None);
        assert_eq!(classify(Path::new("/p/repo/AGENTS.md"), &chat, theme), None);
        assert_eq!(classify(Path::new("/d/coordinator/default/chat/AGENTS.md"), &chat, theme), None, "the board's own files are not ours to watch");
        let two = vec![PathBuf::from("/one"), PathBuf::from("/two")];
        assert_eq!(classify(Path::new("/two/s.jsonl"), &two, theme), Some(Kind::Chat), "every folder of the program counts");
    }

    #[test]
    fn a_folder_that_appears_later_is_watched_and_counted_as_a_conversation() {
        let dir = std::env::temp_dir().join(format!("hp-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut debouncer = new_debouncer(Duration::from_millis(50), |_res: DebounceEventResult| {}).ok().unwrap();
        let mut known = HashSet::new();
        let plan = vec![history::Watch { path: dir.clone(), deep: false, chat: true }];
        assert_eq!(add_watches(&mut debouncer, &plan, &mut known), vec![dir.clone()], "its first records are indexed at once");
        assert!(add_watches(&mut debouncer, &plan, &mut known).is_empty(), "the same folder is watched once");

        let later = dir.join("later");
        let plan = vec![history::Watch { path: later.clone(), deep: false, chat: true }];
        assert!(add_watches(&mut debouncer, &plan, &mut known).is_empty(), "a missing folder is not watched, nor remembered");
        std::fs::create_dir_all(&later).unwrap();
        assert_eq!(add_watches(&mut debouncer, &plan, &mut known), vec![later.clone()], "made later, it is watched and indexed");
        assert_eq!(chat_dirs(&plan), vec![later.clone()], "and the callback treats it as a conversation folder");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
