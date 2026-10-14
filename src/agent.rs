//! The cheap agent: one place that knows how to run a one-shot, read-only
//! model. Questions about a task, the backlog freshness check and the group
//! suggestion (stage 3) all go through it. Stage 1 only defines the commands.

// Reference logic: parts are used only by tests and by stages 2–4 (the page mirrors the list logic).
#![allow(dead_code)]

use crate::sources::runner::{self, Output};
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

/// argv of the agent `kind` for one request. Both variants can only read:
/// Read/Grep/Glob for claude, read/grep for omp. claude gets no settings
/// sources and no hooks (the user's own hooks would talk to herdr), no session
/// file; the prompt goes through stdin because `--tools` is variadic.
pub fn command(kind: Kind, req: &Request) -> Vec<String> {
    let mut argv = match kind {
        Kind::Haiku => runner::argv([
            "claude",
            "-p",
            "--model",
            HAIKU_MODEL,
            "--permission-mode",
            "dontAsk",
            "--setting-sources",
            "",
            "--settings",
            r#"{"disableAllHooks":true}"#,
            "--output-format",
            "text",
        ]),
        Kind::Deepseek => runner::argv(["omp", "-p", "--model", DEEPSEEK_MODEL]),
    };
    if !req.system.is_empty() {
        argv.push("--system-prompt".into());
        argv.push(req.system.clone());
    }
    for d in &req.dirs {
        argv.push("--add-dir".into());
        argv.push(d.clone());
    }
    match kind {
        Kind::Haiku => argv.extend(runner::argv(["--tools", "Read,Grep,Glob"])),
        Kind::Deepseek => argv.extend(runner::argv(["--tools", "read,grep"])),
    }
    argv
}

/// Runs the request and returns the model's text, or the error text.
pub async fn run(kind: Kind, req: &Request) -> Result<String, String> {
    let argv = command(kind, req);
    let out: Output = runner::run(&argv, Some(&req.prompt), TIMEOUT).await?;
    if out.ok() {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(out.error())
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
            dirs: vec!["/r".into()],
        };
        let argv = command(Kind::Haiku, &req);
        assert_eq!(argv[0], "claude");
        assert!(argv.windows(2).any(|w| w == ["--model", HAIKU_MODEL]));
        assert!(argv.windows(2).any(|w| w == ["--setting-sources", ""]));
        assert!(argv.iter().any(|a| a.contains("disableAllHooks")));
        assert!(argv.windows(2).any(|w| w == ["--add-dir", "/r"]));
        assert_eq!(&argv[argv.len() - 2..], ["--tools", "Read,Grep,Glob"]);
        assert!(!argv.iter().any(|a| a == "p"));
    }

    #[test]
    fn deepseek_uses_omp() {
        let argv = command(Kind::Deepseek, &Request::default());
        assert_eq!(argv[0], "omp");
        assert_eq!(&argv[argv.len() - 2..], ["--tools", "read,grep"]);
        assert_eq!(Kind::from_setting("deepseek"), Kind::Deepseek);
        assert_eq!(Kind::from_setting("x"), Kind::Haiku);
    }
}
