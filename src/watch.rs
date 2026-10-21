//! File watching (notify): the board changed, read it again. The page is told
//! over the event stream; nothing polls every few seconds any more.
//!
//! Watched: the tasks' own records (`task.json`, `closing.md`), the Coordinator's
//! state, the board's repo lists, the Coordinator's transcript, the limits records
//! and the theme. A change only pokes the refresh loop (which merges bursts).

use crate::app::Shared;
use crate::{config, history, theme};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub type Guard = Debouncer<RecommendedWatcher>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Board,
    Chat,
    Limits,
    Theme,
}

/// What a changed path means, or nothing (noise such as logs and locks).
pub fn classify(path: &Path, chat_dir: &Path, theme_dir: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?;
    if path.starts_with(chat_dir) {
        return (name.ends_with(".jsonl")).then_some(Kind::Chat);
    }
    if path.starts_with(theme_dir) {
        return Some(Kind::Theme);
    }
    if matches!(name, "task.json" | "closing.md" | "state.json" | "repos.json" | "repo-keys.json" | "board.json") {
        return Some(Kind::Board);
    }
    if name.ends_with(".json") && (path.to_string_lossy().contains("agent-usage") || path.to_string_lossy().contains("agents/usage")) {
        return Some(Kind::Limits);
    }
    None
}

/// Starts the watcher; the returned guard must be kept alive. Folders that do not exist (yet) are skipped.
pub fn start(app: Shared) -> Option<Guard> {
    let cfg = app.cfg.clone();
    let chat_dir = history::transcript_dir(&cfg);
    let theme_dir = theme::theme_dir();
    let handle = tokio::runtime::Handle::current();
    let app2 = app.clone();
    let (chat2, theme2) = (chat_dir.clone(), theme_dir.clone());
    let mut debouncer = new_debouncer(Duration::from_millis(300), move |res: DebounceEventResult| {
        let Ok(events) = res else { return };
        let mut board = false;
        let mut chat = false;
        for e in events {
            match classify(&e.path, &chat2, &theme2) {
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
        if board {
            app2.poke.notify_one();
        }
    })
    .ok()?;
    let mut watch = |path: PathBuf, mode: RecursiveMode| {
        if path.exists() {
            let _ = debouncer.watcher().watch(&path, mode);
        }
    };
    watch(cfg.data_dir.join("tasks"), RecursiveMode::Recursive);
    watch(cfg.data_dir.join("coordinator/default"), RecursiveMode::NonRecursive);
    watch(cfg.data_dir.clone(), RecursiveMode::NonRecursive);
    watch(chat_dir, RecursiveMode::NonRecursive);
    watch(theme_dir, RecursiveMode::NonRecursive);
    let state = config::state_home().join("omarchy/agents/usage");
    watch(state, RecursiveMode::NonRecursive);
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| config::home().join(".cache"))
        .join("omarchy/agent-usage");
    watch(cache, RecursiveMode::NonRecursive);
    Some(debouncer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_meaningful_files_count() {
        let chat = Path::new("/c/chat");
        let theme = Path::new("/t");
        assert_eq!(classify(Path::new("/d/tasks/a/1/task.json"), chat, theme), Some(Kind::Board));
        assert_eq!(classify(Path::new("/d/tasks/a/1/closing.md"), chat, theme), Some(Kind::Board));
        assert_eq!(classify(Path::new("/d/tasks/a/1/runner.log"), chat, theme), None);
        assert_eq!(classify(Path::new("/d/keeper.log"), chat, theme), None);
        assert_eq!(classify(Path::new("/d/repos.json"), chat, theme), Some(Kind::Board));
        assert_eq!(classify(Path::new("/c/chat/s.jsonl"), chat, theme), Some(Kind::Chat));
        assert_eq!(classify(Path::new("/c/chat/notes.txt"), chat, theme), None);
        assert_eq!(classify(Path::new("/t/colors.toml"), chat, theme), Some(Kind::Theme));
        assert_eq!(classify(Path::new("/h/.cache/omarchy/agent-usage/claude-limits.json"), chat, theme), Some(Kind::Limits));
    }
}
