//! Where the planner runs and keeps its files: port, projects root, XDG paths.

use std::env;
use std::path::{Path, PathBuf};

/// The port the server listens on when nothing else says otherwise.
pub const DEFAULT_PORT: u16 = 7117;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub port: u16,
    /// Projects root of herdr-projects (`~/.herdr-projects` by default).
    pub root: PathBuf,
    /// `~/.local/state/herdr-planner/state.json`: groups, settings, answers.
    pub state_file: PathBuf,
    /// `~/.config/herdr-planner/config.toml`: `port = N`, `root = "…"`.
    pub config_file: PathBuf,
    /// The `herdr-projects` executable; a full path when `HERDR_PROJECTS_BIN` names one.
    pub herdr_projects: String,
}

pub fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

pub fn state_home() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state"))
}

/// The optional `config.toml` of the planner.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct FileConfig {
    port: Option<u16>,
    root: Option<String>,
}

fn parse_file(text: &str) -> FileConfig {
    toml::from_str(text).unwrap_or_default()
}

/// Expands a leading `~/`.
pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

impl AppConfig {
    /// Precedence: command-line `--port`/`--root`, then `HERDR_PLANNER_PORT` /
    /// `HERDR_PROJECTS_ROOT`, then the config file, then defaults.
    pub fn load(args: &[String]) -> Self {
        let config_file = config_home().join("herdr-planner/config.toml");
        let file = std::fs::read_to_string(&config_file)
            .map(|t| parse_file(&t))
            .unwrap_or_default();
        let mut port = file
            .port
            .or_else(|| {
                env::var("HERDR_PLANNER_PORT")
                    .ok()
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(DEFAULT_PORT);
        let mut root = env::var("HERDR_PROJECTS_ROOT")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| expand_home(&v))
            .or_else(|| file.root.as_deref().map(expand_home))
            .unwrap_or_else(|| home().join(".herdr-projects"));
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--port" => {
                    if let Some(v) = it.next().and_then(|v| v.parse().ok()) {
                        port = v;
                    }
                }
                "--root" => {
                    if let Some(v) = it.next() {
                        root = expand_home(v);
                    }
                }
                _ => {}
            }
        }
        Self {
            port,
            root,
            state_file: state_home().join("herdr-planner/state.json"),
            config_file,
            herdr_projects: env::var("HERDR_PROJECTS_BIN")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "herdr-projects".to_string()),
        }
    }

    pub fn project_dir(&self, slug: &str) -> PathBuf {
        self.root.join(slug)
    }
}

pub fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_and_root() {
        let f = parse_file("port = 9000\nroot = \"~/p\"\n");
        assert_eq!(f.port, Some(9000));
        assert_eq!(f.root.as_deref(), Some("~/p"));
        assert_eq!(expand_home("~/p"), home().join("p"));
    }

    #[test]
    fn garbage_config_is_default() {
        let f = parse_file("port = \"abc\"");
        assert_eq!(f.port, None);
    }
}
