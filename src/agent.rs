//! The cheap agent: one place that knows how to run a one-shot, read-only
//! model. Questions about a task, the backlog freshness check and the group
//! suggestion (stage 3) all go through it. Stage 2 uses it for questions.

// The freshness check and the group suggestion (stage 3) use the rest.
#![allow(dead_code)]

use crate::sources::runner::{self, Output};
use crate::sources::tasks;
use std::time::Duration;

pub const HAIKU_MODEL: &str = "claude-haiku-5-5";
pub const DEEPSEEK_MODEL: &str = "deepseek/deepseek-v4-flash";
pub const TIMEOUT: Duration = Duration::from_secs(270);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Haiku,
    Deepseek,
}

impl Kind {
    pub fn from_setting(value: &str) -> Kind {
        if value == "deepseek" {
            Kind::Deepseek
        } else {
            Kind::Haiku
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Kind::Haiku => "Haiku 5.5",
            Kind::Deepseek => "DeepSeek Flash",
        }
    }
}

/// One request: a system prompt, the prompt itself, the folder to run in and
/// extra folders the agent may read.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub system: String,
    pub prompt: String,
    pub cwd: String,
    pub dirs: Vec<String>,
}

/// argv (and the text for stdin) of the agent `kind` for one request. Both
/// variants can only read: Read/Grep/Glob for claude, read/grep for omp.
/// claude gets no settings sources and no hooks (the user's own hooks would
/// talk to herdr), no session file, and the prompt on stdin (the tool lists
/// are variadic and would swallow a positional prompt).
pub fn command(kind: Kind, req: &Request) -> (Vec<String>, Option<String>) {
    match kind {
        Kind::Haiku => {
            let mut argv = runner::argv([
                "claude",
                "-p",
                "--model",
                HAIKU_MODEL,
                "--tools",
                "Read,Grep,Glob",
                "--allowedTools",
                "Read,Grep,Glob",
                "--disallowedTools",
                "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch",
                "--permission-mode",
                "dontAsk",
                "--no-session-persistence",
                "--disable-slash-commands",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--settings",
                r#"{"disableAllHooks":true}"#,
                "--append-system-prompt",
                req.system.as_str(),
            ]);
            if !req.dirs.is_empty() {
                argv.push("--add-dir".into());
                argv.extend(req.dirs.iter().cloned());
            }
            (argv, Some(req.prompt.clone()))
        }
        Kind::Deepseek => {
            let mut argv = runner::argv([
                "omp",
                "-p",
                "--model",
                DEEPSEEK_MODEL,
                "--tools",
                "read,grep",
                "--no-session",
                "--no-title",
                "--no-skills",
                "--no-extensions",
                "--max-time",
                "4m",
                "--cwd",
                req.cwd.as_str(),
                "--append-system-prompt",
                req.system.as_str(),
            ]);
            argv.push(req.prompt.clone());
            (argv, None)
        }
    }
}

/// Runs the request and returns the model's text, or the error text.
pub async fn run(kind: Kind, req: &Request) -> Result<String, String> {
    let (argv, stdin) = command(kind, req);
    let cwd = (!req.cwd.is_empty()).then_some(req.cwd.as_str());
    let out: Output = runner::run_in(&argv, stdin.as_deref(), TIMEOUT, cwd).await?;
    let text = out.stdout.trim().trim_start_matches("Working...").trim();
    if out.ok() && !text.is_empty() {
        Ok(text.to_string())
    } else if out.ok() {
        Err("агент ответил пустотой".into())
    } else {
        Err(tasks::clip(
            &out.error().split_whitespace().collect::<Vec<_>>().join(" "),
            300,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haiku_command_is_read_only_and_hookless() {
        let req = Request {
            system: "sys".into(),
            prompt: "p".into(),
            cwd: "/p".into(),
            dirs: vec!["/r".into(), "/s".into()],
        };
        let (argv, stdin) = command(Kind::Haiku, &req);
        assert_eq!(argv[0], "claude");
        assert!(argv.windows(2).any(|w| w == ["--model", HAIKU_MODEL]));
        assert!(argv.windows(2).any(|w| w == ["--setting-sources", ""]));
        assert!(argv.windows(2).any(|w| w == ["--tools", "Read,Grep,Glob"]));
        assert!(argv
            .windows(2)
            .any(|w| w == ["--permission-mode", "dontAsk"]));
        assert!(argv
            .windows(2)
            .any(|w| w == ["--append-system-prompt", "sys"]));
        assert!(argv.iter().any(|a| a.contains("disableAllHooks")));
        assert!(argv.iter().any(|a| a.starts_with("Bash,Edit,Write")));
        assert_eq!(&argv[argv.len() - 3..], ["--add-dir", "/r", "/s"]);
        // The prompt goes through stdin, never as an argument.
        assert!(!argv.iter().any(|a| a == "p"));
        assert_eq!(stdin.as_deref(), Some("p"));
    }

    #[test]
    fn deepseek_uses_omp() {
        let req = Request {
            prompt: "question".into(),
            cwd: "/p".into(),
            ..Default::default()
        };
        let (argv, stdin) = command(Kind::Deepseek, &req);
        assert_eq!(argv[0], "omp");
        assert!(argv.windows(2).any(|w| w == ["--tools", "read,grep"]));
        assert!(argv.windows(2).any(|w| w == ["--cwd", "/p"]));
        assert_eq!(argv.last().unwrap(), "question");
        assert_eq!(stdin, None);
        assert_eq!(Kind::from_setting("deepseek"), Kind::Deepseek);
        assert_eq!(Kind::from_setting("x"), Kind::Haiku);
    }
}
