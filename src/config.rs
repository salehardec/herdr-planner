//! Where the planner runs and how it may be reached.
//!
//! Defaults keep everything on this machine: `127.0.0.1:7117`, no login.
//! Setting a password hash turns the login on; a non-loopback address without
//! one is refused. TLS is used when a certificate and key are configured.

use std::env;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const DEFAULT_PORT: u16 = 7117;
pub const DEFAULT_USER: &str = "herdr";

#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Address to bind (`127.0.0.1`, `0.0.0.0`, `::`, a literal IP or `localhost`).
    pub listen: String,
    pub port: u16,
    /// The data folder of the herdr-projects board (`~/.herdr-projects/data` by default).
    pub data_dir: PathBuf,
    /// Whether the folder was named explicitly; then it is passed to board commands.
    pub data_named: bool,
    /// `~/.config/herdr-planner/config.toml`.
    pub config_file: PathBuf,
    /// `~/.local/state/herdr-planner`.
    pub state_dir: PathBuf,
    /// `herdr-projects` (or a full path from `HERDR_PROJECTS_BIN`).
    pub herdr_projects: String,
    /// The herdr server's socket; its presence means herdr is running.
    pub herdr_socket: PathBuf,
    /// Login name for the dashboard.
    pub user: String,
    /// Argon2id (or bcrypt) PHC string. Empty means "local only, no login".
    pub password_hash: String,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    /// Who translates answers for the page: `claude/<model>` (default) or `omp/<model>`.
    pub translator: String,
    /// The ssh program the planner uses for the shipyards (`HERDR_PLANNER_SSH` overrides, for checks).
    pub ssh_bin: String,
    /// Where a repository added by address is cloned (`~/Projects`).
    pub projects_dir: PathBuf,
    /// Host names (`name` or `name:port`) a request may name besides this machine's own, e.g. the
    /// domain of a reverse proxy in front of the planner. Any other `Host` from this machine is refused.
    pub trusted_hosts: Vec<String>,
    /// The fixed address of a named tunnel (`planner.example.org`): a request naming it is a tunnel request.
    pub tunnel_hostname: Option<String>,
    /// Which tunnel the gate opens: a quick one (a random `*.trycloudflare.com`) or the owner's own, named one.
    pub tunnel_mode: TunnelMode,
    /// The `cloudflared` config of the owner's named tunnel (`~/.cloudflared/herdr-planner.yml`). The planner reads
    /// only the `ingress` host names from it, never the credentials it points to.
    pub tunnel_config: Option<PathBuf>,
    /// The `cloudflared` program (a name or a path), when it is not in the usual places.
    pub tunnel_bin: Option<String>,
}

/// `tunnel.mode` in `config.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelMode {
    /// `cloudflared tunnel --url …`: no domain, a new random address at every start.
    #[default]
    Quick,
    /// `cloudflared --config <file> tunnel run`: a tunnel made earlier with `cloudflared tunnel create`, a fixed address.
    Named,
}

impl TunnelMode {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "quick" => Some(Self::Quick),
            "named" => Some(Self::Named),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Named => "named",
        }
    }
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

/// Expands a leading `~/`.
pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct FileConfig {
    listen: Option<String>,
    port: Option<u16>,
    data: Option<String>,
    user: Option<String>,
    password_hash: Option<String>,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    translator: Option<String>,
    projects_dir: Option<String>,
    trusted_hosts: Vec<String>,
    tunnel: TunnelFile,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct TunnelFile {
    hostname: Option<String>,
    mode: Option<String>,
    config: Option<String>,
    bin: Option<String>,
}

fn parse_file(text: &str) -> FileConfig {
    toml::from_str(text).unwrap_or_default()
}

impl AppConfig {
    pub fn load(args: &[String]) -> Self {
        let config_file = config_home().join("herdr-planner/config.toml");
        let file = fs::read_to_string(&config_file)
            .map(|t| parse_file(&t))
            .unwrap_or_default();
        let mut listen = file
            .listen
            .or_else(|| {
                env::var("HERDR_PLANNER_LISTEN")
                    .ok()
                    .filter(|v| !v.is_empty())
            })
            .unwrap_or_else(|| "127.0.0.1".to_string());
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
                "--listen" => {
                    if let Some(v) = it.next() {
                        listen = v.clone();
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
        let tls_cert = file.tls_cert.map(|v| expand_home(&v));
        let tls_key = file.tls_key.map(|v| expand_home(&v));
        Self {
            listen,
            port,
            data_named: data.is_some(),
            data_dir: data.unwrap_or_else(|| home().join(".herdr-projects/data")),
            config_file,
            state_dir: state_home().join("herdr-planner"),
            herdr_projects: env::var("HERDR_PROJECTS_BIN")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "herdr-projects".to_string()),
            herdr_socket: env::var_os("HERDR_SOCKET_PATH")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| config_home().join("herdr/herdr.sock")),
            user: file
                .user
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_USER.to_string()),
            password_hash: file.password_hash.unwrap_or_default(),
            tls_cert,
            tls_key,
            translator: file
                .translator
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "claude/claude-haiku-5-5".to_string()),
            ssh_bin: env::var("HERDR_PLANNER_SSH")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "ssh".to_string()),
            projects_dir: file
                .projects_dir
                .filter(|v| !v.trim().is_empty())
                .map(|v| expand_home(&v))
                .unwrap_or_else(|| home().join("Projects")),
            trusted_hosts: file
                .trusted_hosts
                .iter()
                .filter_map(|h| clean_host(h))
                .collect(),
            tunnel_hostname: file.tunnel.hostname.as_deref().and_then(clean_host),
            tunnel_mode: file.tunnel.mode.as_deref().and_then(TunnelMode::parse).unwrap_or_default(),
            tunnel_config: file.tunnel.config.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(expand_home),
            tunnel_bin: file.tunnel.bin.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()),
        }
    }

    pub fn auth_required(&self) -> bool {
        !self.password_hash.trim().is_empty()
    }

    pub fn tls(&self) -> Option<(&Path, &Path)> {
        match (&self.tls_cert, &self.tls_key) {
            (Some(c), Some(k)) => Some((c.as_path(), k.as_path())),
            _ => None,
        }
    }

    /// True when the bound address cannot be reached from another machine.
    pub fn is_loopback(&self) -> bool {
        listen_is_loopback(&self.listen)
    }

    pub fn reports_dir(&self) -> PathBuf {
        self.data_dir
            .parent()
            .map(|p| p.join("reports"))
            .unwrap_or_else(|| home().join(".herdr-projects/reports"))
    }

    /// Images sent to the Coordinator: the planner's own state folder, never
    /// the repository or the board's data folder.
    pub fn attachments_dir(&self) -> PathBuf {
        self.state_dir.join("attachments")
    }

    /// The shipyards («верфи»): one file next to the board's own settings, the same path on every
    /// machine. `HERDR_PLANNER_RUNNERS` points the checks at another one.
    /// The board's own settings (`settings/config.json`): the level files the CLI writes. The planner reads a scope's
    /// own instructions here, not from `show`, whose value is already joined with the broader places (PLN-45).
    pub fn settings_file(&self) -> PathBuf {
        if let Ok(v) = env::var("HERDR_PROJECTS_CONFIG_DIR") {
            if !v.trim().is_empty() {
                return expand_home(v.trim()).join("config.json");
            }
        }
        self.data_dir
            .parent()
            .map(|p| p.join("settings/config.json"))
            .unwrap_or_else(|| home().join(".herdr-projects/settings/config.json"))
    }

    pub fn runners_file(&self) -> PathBuf {
        if let Ok(v) = env::var("HERDR_PLANNER_RUNNERS") {
            if !v.trim().is_empty() {
                return expand_home(v.trim());
            }
        }
        self.data_dir
            .parent()
            .map(|p| p.join("settings/runners.json"))
            .unwrap_or_else(|| home().join(".herdr-projects/settings/runners.json"))
    }

    /// The folder new universes are cloned into now: the one the ship set last, else the configured one.
    pub fn projects_dir_now(&self) -> PathBuf {
        LIVE_PROJECTS_DIR
            .read()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(|| self.projects_dir.clone())
    }
}

/// The folder for new universes as changed on the ship, live (the config file has it too).
static LIVE_PROJECTS_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn set_live_projects_dir(dir: PathBuf) {
    if let Ok(mut g) = LIVE_PROJECTS_DIR.write() {
        *g = Some(dir);
    }
}

/// The host name of a tunnel as the settings and the gate write it (see `clean_host`).
pub fn tunnel_host(text: &str) -> Option<String> {
    clean_host(text)
}

/// A host name as written in the settings, lower case: `https://Planner.example.org/` and
/// `planner.example.org` are the same entry; an empty one is none.
fn clean_host(text: &str) -> Option<String> {
    let t = text.trim().to_lowercase();
    let t = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(&t);
    let t = t.split('/').next().unwrap_or("").trim_end_matches('.');
    (!t.is_empty()).then(|| t.to_string())
}

pub fn listen_is_loopback(listen: &str) -> bool {
    let l = listen.trim();
    l == "localhost"
        || l.parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Rewrites `config.toml`, keeping unrelated keys, with the login and listen address.
/// The file is written with mode 600: it holds a password hash.
pub fn set_auth(
    config_file: &Path,
    user: &str,
    password_hash: &str,
    listen: Option<&str>,
) -> Result<(), String> {
    edit_config(config_file, |doc| {
        doc.insert("user".into(), toml::Value::String(user.to_string()));
        doc.insert(
            "password_hash".into(),
            toml::Value::String(password_hash.to_string()),
        );
        if let Some(l) = listen {
            doc.insert("listen".into(), toml::Value::String(l.to_string()));
        }
    })
}

fn absolute_path(path: &str) -> String {
    let path = expand_home(path);
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    };
    path.canonicalize().unwrap_or(path).display().to_string()
}

pub fn set_tls(config_file: &Path, cert: &str, key: &str) -> Result<(), String> {
    // The service has its own working directory (systemd starts it in $HOME),
    // so a relative path from this shell would not resolve later.
    let cert = absolute_path(cert);
    let key = absolute_path(key);
    edit_config(config_file, |doc| {
        doc.insert("tls_cert".into(), toml::Value::String(cert));
        doc.insert("tls_key".into(), toml::Value::String(key));
    })
}

fn edit_config(
    config_file: &Path,
    edit: impl FnOnce(&mut toml::map::Map<String, toml::Value>),
) -> Result<(), String> {
    let text = fs::read_to_string(config_file).unwrap_or_default();
    let mut doc: toml::Value = if text.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(&text).map_err(|e| format!("{}: {e}", config_file.display()))?
    };
    let table = doc
        .as_table_mut()
        .ok_or_else(|| format!("{}: not a TOML table", config_file.display()))?;
    edit(table);
    if let Some(parent) = config_file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let out = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    let tmp = config_file.with_extension("toml.new");
    fs::write(&tmp, out).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, config_file).map_err(|e| e.to_string())?;
    Ok(())
}

/// Writes top-level keys of `config.toml` and keeps the rest of the file (mode 600, as `set_auth`).
pub fn set_values(config_file: &Path, values: Vec<(&str, toml::Value)>) -> Result<(), String> {
    edit_config(config_file, |doc| {
        for (key, value) in values {
            doc.insert(key.to_string(), value);
        }
    })
}

/// Writes the `[tunnel]` table of `config.toml` (mode, the cloudflared config of a named tunnel, its address) and keeps
/// the table's other keys and the rest of the file. An empty `config`/`hostname` removes the key.
pub fn set_tunnel(config_file: &Path, mode: TunnelMode, config: &str, hostname: &str) -> Result<(), String> {
    edit_config(config_file, |doc| {
        let mut table = match doc.remove("tunnel") {
            Some(toml::Value::Table(t)) => t,
            _ => toml::map::Map::new(),
        };
        table.insert("mode".into(), toml::Value::String(mode.as_str().into()));
        for (key, value) in [("config", config.trim()), ("hostname", hostname.trim())] {
            if value.is_empty() {
                table.remove(key);
            } else {
                table.insert(key.into(), toml::Value::String(value.into()));
            }
        }
        doc.insert("tunnel".into(), toml::Value::Table(table));
    })
}

/// Argon2id PHC string for the password.
pub fn hash_password(password: &str) -> Result<String, String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    use argon2::Argon2;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Checks a password against an Argon2 PHC string or a bcrypt hash.
pub fn verify_password(hash: &str, password: &str) -> bool {
    let hash = hash.trim();
    if hash.starts_with("$2a$") || hash.starts_with("$2b$") || hash.starts_with("$2y$") {
        return bcrypt::verify(password, hash).unwrap_or(false);
    }
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    use argon2::Argon2;
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_and_expands_home() {
        let f = parse_file(
            "listen = \"0.0.0.0\"\nport = 9000\ndata = \"~/p\"\nuser = \"me\"\npassword_hash = \"$argon2id$x\"\ntls_cert = \"/c\"\n",
        );
        assert_eq!(f.port, Some(9000));
        assert_eq!(f.data.as_deref(), Some("~/p"));
        assert_eq!(f.user.as_deref(), Some("me"));
        assert_eq!(f.tls_cert.as_deref(), Some("/c"));
        assert_eq!(expand_home("~/p"), home().join("p"));
        assert!(f.tls_key.is_none());
    }

    #[test]
    fn trusted_hosts_and_the_tunnel_address_are_read_and_cleaned() {
        let f = parse_file(
            "trusted_hosts = [\"Planner.LAN\", \"https://box.example.org:8443/x\", \"  \", \"a.b.\"]\n[tunnel]\nhostname = \"https://Planner.Yanao.Su/\"\n",
        );
        let hosts: Vec<String> = f.trusted_hosts.iter().filter_map(|h| clean_host(h)).collect();
        assert_eq!(hosts, ["planner.lan", "box.example.org:8443", "a.b"]);
        assert_eq!(f.tunnel.hostname.as_deref().and_then(clean_host).as_deref(), Some("planner.yanao.su"));
        let none = parse_file("port = 1\n");
        assert!(none.trusted_hosts.is_empty() && none.tunnel.hostname.is_none());
    }

    #[test]
    fn the_tunnel_table_has_a_mode_a_config_and_an_address() {
        let f = parse_file("[tunnel]\nmode = \"Named\"\nconfig = \" ~/.cloudflared/herdr-planner.yml \"\nhostname = \"planner.yanao.su\"\nbin = \"\"\n");
        assert_eq!(f.tunnel.mode.as_deref().and_then(TunnelMode::parse), Some(TunnelMode::Named));
        assert_eq!(f.tunnel.config.as_deref().map(str::trim), Some("~/.cloudflared/herdr-planner.yml"));
        assert_eq!(TunnelMode::parse("anything else"), None, "an unknown mode falls back to the default, quick");
        assert_eq!(TunnelMode::default(), TunnelMode::Quick);
    }

    #[test]
    fn the_tunnel_table_is_edited_without_losing_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "port = 7333\n[tunnel]\nhostname = \"old.example.org\"\nbin = \"/opt/cloudflared\"\n").unwrap();
        set_tunnel(&path, TunnelMode::Named, "/home/me/.cloudflared/x.yml", "").unwrap();
        let f = parse_file(&fs::read_to_string(&path).unwrap());
        assert_eq!(f.port, Some(7333));
        assert_eq!(f.tunnel.mode.as_deref(), Some("named"));
        assert_eq!(f.tunnel.config.as_deref(), Some("/home/me/.cloudflared/x.yml"));
        assert_eq!(f.tunnel.hostname, None, "an empty address removes the key");
        assert_eq!(f.tunnel.bin.as_deref(), Some("/opt/cloudflared"), "other keys of the table stay");
    }

    #[test]
    fn garbage_config_is_default() {
        let f = parse_file("port = \"abc\"");
        assert_eq!(f.port, None);
    }

    #[test]
    fn loopback_addresses() {
        assert!(listen_is_loopback("127.0.0.1"));
        assert!(listen_is_loopback("::1"));
        assert!(listen_is_loopback("localhost"));
        assert!(!listen_is_loopback("0.0.0.0"));
        assert!(!listen_is_loopback("192.0.2.4"));
    }

    #[test]
    fn argon2_round_trip_and_bcrypt() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password(&h, "correct horse"));
        assert!(!verify_password(&h, "wrong"));
        let b = bcrypt::hash("hunter2", 4).unwrap();
        assert!(verify_password(&b, "hunter2"));
        assert!(!verify_password(&b, "hunter3"));
        assert!(!verify_password("not a hash", "x"));
    }

    #[test]
    fn tls_paths_are_stored_absolute() {
        let p = absolute_path("cert.pem");
        assert!(Path::new(&p).is_absolute());
        assert!(p.ends_with("cert.pem"));
    }

    #[test]
    fn edits_config_without_losing_keys() {
        let dir = std::env::temp_dir().join(format!("hp-cfg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "port = 7333\ndata = \"/tmp/board\"\n").unwrap();
        set_auth(&path, "me", "$argon2id$hash", Some("0.0.0.0")).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("port = 7333"));
        assert!(text.contains("data = \"/tmp/board\""));
        assert!(text.contains("user = \"me\""));
        assert!(text.contains("listen = \"0.0.0.0\""));
        let _ = fs::remove_dir_all(&dir);
    }
}
