//! The models a program takes, for the task form's pop-up (plan §4.10): `herdr-projects models <program>`.
//!
//! Answers are cached for ten minutes ("fresh"). A stale answer is still served at once while a new one is read in
//! the background, so the pop-up never waits for the board after the first read. The first read of a program shows
//! `loading: true`, and the pop-up then takes free text.

use crate::app::run_board;
use crate::config::AppConfig;
use moka::sync::Cache;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long an answer counts as fresh.
const FRESH: Duration = Duration::from_secs(600);
/// After a failed read, the next try waits this long (the last good answer stays).
const RETRY: Duration = Duration::from_secs(60);
/// The board's own limit: `models claude` takes about 42 s here, with two 30 s timeouts inside.
const WAIT: Duration = Duration::from_secs(90);
/// How long a stale answer may still be served before it is dropped.
const KEEP: Duration = Duration::from_secs(3600);
const MAX_NAMES: usize = 300;

/// What the pop-up gets: the names it may offer, the board's explanation, and whether they are still coming.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Models {
    pub names: Vec<String>,
    /// The effort levels the program takes (`worker.effort`), from the same answer.
    pub effort: Vec<String>,
    pub note: String,
    pub loading: bool,
}

#[derive(Debug, Clone)]
struct Entry {
    names: Vec<String>,
    effort: Vec<String>,
    note: String,
    fresh_until: Instant,
}

fn cache() -> &'static Cache<String, Entry> {
    static CACHE: OnceLock<Cache<String, Entry>> = OnceLock::new();
    CACHE.get_or_init(|| Cache::builder().max_capacity(64).time_to_live(KEEP).build())
}

fn inflight() -> &'static Mutex<HashSet<String>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A program name the board takes as `models <program>`: `claude`, `codex`, `pi`, `omp`, `cursor-agent`, and any
/// other lowercase word of up to 21 characters (letters, digits, `-`, starting with a letter).
pub fn valid_program(p: &str) -> bool {
    let b = p.as_bytes();
    !b.is_empty()
        && b.len() <= 21
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// The board's answer: an explanatory paragraph (the first one), a blank line, then `<program>/<model>` lines.
/// Returns the names (every line that is `<program>/<one word>`) and the paragraph as the note.
pub fn parse(program: &str, stdout: &str) -> (Vec<String>, String) {
    let prefix = format!("{program}/");
    let mut note: Vec<&str> = Vec::new();
    for line in stdout.lines().map(str::trim) {
        if line.is_empty() {
            if note.is_empty() {
                continue;
            }
            break;
        }
        note.push(line);
    }
    let mut names: Vec<String> = Vec::new();
    for line in stdout.lines().map(str::trim) {
        let is_name = line.len() > prefix.len()
            && line.starts_with(&prefix)
            && !line[prefix.len()..].contains(char::is_whitespace);
        if is_name && !names.iter().any(|n| n == line) {
            names.push(line.to_string());
        }
        if names.len() >= MAX_NAMES {
            break;
        }
    }
    (names, note.join(" "))
}

/// The effort levels in the board's answer: the words of its `Effort (…): low, medium, …` line, up to the first
/// bracket or semicolon (later lines name the exceptions per model).
pub fn parse_effort(stdout: &str) -> Vec<String> {
    let Some(line) = stdout.lines().map(str::trim).find(|l| l.starts_with("Effort")) else {
        return Vec::new();
    };
    let Some((_, rest)) = line.split_once("):").or_else(|| line.split_once(':')) else {
        return Vec::new();
    };
    let list = rest.split(['(', ';', '.']).next().unwrap_or("");
    list.split(',')
        .map(str::trim)
        .filter(|w| {
            !w.is_empty()
                && w.len() <= 24
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .map(String::from)
        .collect()
}

fn refresh(cfg: AppConfig, program: String) {
    {
        let mut set = inflight().lock().unwrap_or_else(|e| e.into_inner());
        if !set.insert(program.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let entry = match run_board(&cfg, &["models", program.as_str()], WAIT).await {
            Ok(out) => {
                let (names, note) = parse(&program, &out);
                Entry {
                    names,
                    effort: parse_effort(&out),
                    note,
                    fresh_until: Instant::now() + FRESH,
                }
            }
            Err(e) => match cache().get(&program) {
                Some(old) => Entry {
                    fresh_until: Instant::now() + RETRY,
                    ..old
                },
                None => Entry {
                    names: Vec::new(),
                    effort: Vec::new(),
                    note: format!("модели не прочитаны: {e}"),
                    fresh_until: Instant::now() + RETRY,
                },
            },
        };
        cache().insert(program.clone(), entry);
        inflight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&program);
    });
}

/// The models of a program now. Returns at once: a cached answer (refreshed in the background when stale), or
/// `loading: true` while the first read runs.
pub async fn get(cfg: &AppConfig, program: &str) -> Models {
    if !valid_program(program) {
        return Models {
            note: "такой программы нет".into(),
            ..Models::default()
        };
    }
    match cache().get(&program.to_string()) {
        Some(e) => {
            if Instant::now() >= e.fresh_until {
                refresh(cfg.clone(), program.to_string());
            }
            Models {
                names: e.names,
                effort: e.effort,
                note: e.note,
                loading: false,
            }
        }
        None => {
            refresh(cfg.clone(), program.to_string());
            Models {
                loading: true,
                ..Models::default()
            }
        }
    }
}

/// Reads the models of these programs now (at start-up, and every ten minutes).
pub async fn warm(cfg: &AppConfig, programs: &[String]) {
    for p in programs {
        let _ = get(cfg, p).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// `herdr-projects models claude` as the board printed it on this machine (trimmed to its first lines).
    const CLAUDE: &str = "claude takes a short name (default, fable, opus, sonnet, haiku, opusplan), each the newest of its kind, or a full name such as claude-fable-5-1. Write one after the program, as in worker.agent claude/default:

claude/default
claude/fable
claude/opus
claude/sonnet
claude/haiku
claude/opusplan
claude/claude-fable-5-1
claude/claude-haiku-5-5

Effort (worker.effort and the like): low, medium, high, xhigh, max (from `claude --help`).
";

    /// `herdr-projects models omp` as printed on this machine: no list of names, only the note.
    const OMP: &str = "omp: any name omp takes (not checked: omp has no list of them).

Effort (worker.effort and the like): off, minimal, low, medium, high, xhigh, max, auto (from `omp --help`).
";

    #[test]
    fn the_board_answer_becomes_names_and_a_note() {
        let (names, note) = parse("claude", CLAUDE);
        assert_eq!(names.len(), 8, "{names:?}");
        assert_eq!(names[0], "claude/default");
        assert_eq!(names[7], "claude/claude-haiku-5-5");
        assert!(note.starts_with("claude takes a short name"), "{note}");
        assert!(
            !note.contains("Effort"),
            "only the first paragraph is the note: {note}"
        );
    }

    #[test]
    fn a_program_with_no_list_has_no_names_and_a_note() {
        let (names, note) = parse("omp", OMP);
        assert!(names.is_empty());
        assert_eq!(
            note,
            "omp: any name omp takes (not checked: omp has no list of them)."
        );
    }

    #[test]
    fn other_programs_lines_and_spaces_are_not_names() {
        let out =
            "note\n\ncodex/gpt-5\ncodex/gpt 5\nclaude/haiku\ncodex/gpt-5\ncodex/\n  codex/o3  \n";
        let (names, _) = parse("codex", out);
        assert_eq!(
            names,
            vec!["codex/gpt-5".to_string(), "codex/o3".to_string()]
        );
    }

    #[test]
    fn program_names_are_checked() {
        for ok in ["claude", "codex", "pi", "omp", "cursor-agent", "a1-b2"] {
            assert!(valid_program(ok), "{ok}");
        }
        for bad in [
            "",
            "Claude",
            "1abc",
            "claude;rm",
            "a b",
            "-x",
            "x/y",
            "abcdefghijklmnopqrstuvwxyz",
        ] {
            assert!(!valid_program(bad), "{bad}");
        }
    }

    fn fake_board(dir: &Path, name: &str, body: &str) -> String {
        let p = dir.join(name);
        crate::testutil::write_exec(&p, format!("#!/bin/sh\n{body}\n"));
        p.display().to_string()
    }

    async fn wait_ready(cfg: &AppConfig, program: &str) -> Models {
        for _ in 0..200 {
            let m = get(cfg, program).await;
            if !m.loading {
                return m;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{program}: still loading");
    }

    #[test]
    fn effort_is_the_first_list_of_the_effort_line() {
        assert_eq!(
            parse_effort(CLAUDE),
            vec!["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(parse_effort(OMP).len(), 8);
        let codex = "codex takes ...\n\ncodex/gpt-5.5\n\nEffort (worker.effort and the like): low, medium, high, xhigh, max, ultra (from `codex debug models`); gpt-5.5 only low, medium, high, xhigh.\n";
        assert_eq!(parse_effort(codex).last().map(String::as_str), Some("ultra"));
        assert!(parse_effort("claude/opus\n").is_empty());
    }

    #[tokio::test]
    async fn loading_turns_into_ready_and_then_comes_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let board = fake_board(
            dir.path(),
            "board.sh",
            "sleep 1\necho 'A paragraph about fakea.'\necho ''\necho 'fakea/one'\necho 'fakea/two'",
        );
        let cfg = AppConfig {
            herdr_projects: board,
            ..crate::machines::test_cfg(dir.path())
        };
        let first = get(&cfg, "fakea").await;
        assert!(first.loading, "{first:?}");
        assert!(first.names.is_empty());
        let ready = wait_ready(&cfg, "fakea").await;
        assert_eq!(
            ready.names,
            vec!["fakea/one".to_string(), "fakea/two".to_string()]
        );
        assert_eq!(ready.note, "A paragraph about fakea.");
        // Now the answer is there at once, with no loading.
        let again = get(&cfg, "fakea").await;
        assert!(!again.loading);
        assert_eq!(again.names.len(), 2);
    }

    #[tokio::test]
    async fn a_failed_read_says_so_and_keeps_the_page_working() {
        let dir = tempfile::tempdir().unwrap();
        let board = fake_board(
            dir.path(),
            "board-bad.sh",
            "echo 'no such thing' >&2\nexit 3",
        );
        let cfg = AppConfig {
            herdr_projects: board,
            ..crate::machines::test_cfg(dir.path())
        };
        let _ = get(&cfg, "fakeb").await;
        let ready = wait_ready(&cfg, "fakeb").await;
        assert!(ready.names.is_empty());
        assert!(ready.note.contains("не прочитаны"), "{ready:?}");
        assert!(ready.note.contains("no such thing"), "{ready:?}");
    }

    #[tokio::test]
    async fn an_invalid_program_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::machines::test_cfg(dir.path());
        let m = get(&cfg, "no way").await;
        assert!(!m.loading);
        assert!(m.names.is_empty());
    }
}
