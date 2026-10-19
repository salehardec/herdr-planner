//! Where the planner runs and keeps its files: port, the board's data folder, XDG paths.

use std::env;
use std::path::{Path, PathBuf};

/// The port the server listens on when nothing else says otherwise.
pub const DEFAULT_PORT: u16 = 7117;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub port: u16,
    /// The data folder of the herdr-projects board (`~/.herdr-projects/data` by default):
    /// its repos, keys and tasks. Passed on to the board's commands only when it was named explicitly.
    pub data_dir: PathBuf,
    /// Whether the folder was named (`--data`, `HERDR_PROJECTS_DATA_DIR`, `data = "…"`), not the default.
    pub data_named: bool,
    /// `~/.local/state/herdr-planner/state.json`: groups and settings.
    pub state_file: PathBuf,
    /// `~/.config/herdr-planner/config.toml`: `port = N`, `data = "…"`.
    pub config_file: PathBuf,
    /// The `herdr-projects` executable; a full path when `HERDR_PROJECTS_BIN` names one.
    pub herdr_projects: String,
    /// The `herdr` executable (`HERDR_BIN`): the focused workspace.
    pub herdr: String,
    /// The socket of the herdr server (`HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock`).
    pub herdr_socket: PathBuf,
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
    data: Option<String>,
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
    /// Precedence: command-line `--port`/`--data`, then `HERDR_PLANNER_PORT` /
    /// `HERDR_PROJECTS_DATA_DIR`, then the config file, then defaults.
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
        let mut data = env::var("HERDR_PROJECTS_DATA_DIR")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| expand_home(&v))
            .or_else(|| file.data.as_deref().map(expand_home));
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--port" => {
                    if let Some(v) = it.next().and_then(|v| v.parse().ok()) {
                        port = v;
                    }
                }
                "--data" => {
                    if let Some(v) = it.next() {
                        data = Some(expand_home(v));
                    }
                }
                _ => {}
            }
        }
        Self {
            port,
            data_named: data.is_some(),
            data_dir: data.unwrap_or_else(|| home().join(".herdr-projects/data")),
            state_file: state_home().join("herdr-planner/state.json"),
            config_file,
            herdr_projects: env::var("HERDR_PROJECTS_BIN")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "herdr-projects".to_string()),
            herdr: env::var("HERDR_BIN")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "herdr".to_string()),
            herdr_socket: env::var_os("HERDR_SOCKET_PATH")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| config_home().join("herdr/herdr.sock")),
        }
    }
}

pub fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_and_data() {
        let f = parse_file("port = 9000\ndata = \"~/p\"\nroot = \"~/old\"\n");
        assert_eq!(f.port, Some(9000));
        assert_eq!(f.data.as_deref(), Some("~/p"));
        assert_eq!(expand_home("~/p"), home().join("p"));
    }

    #[test]
    fn garbage_config_is_default() {
        let f = parse_file("port = \"abc\"");
        assert_eq!(f.port, None);
    }
}
