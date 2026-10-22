//! The ship («Бортовой компьютер»): the planner's own settings, the same wherever it flies. The folder new
//! universes are cloned into (live), the login and the address it listens on.
//! Settings go to the planner's config file.

use crate::app::{self, Shared};
use crate::config::{self, expand_home, listen_is_loopback, AppConfig};
use crate::server::{client_is_local, error, same_origin};
use crate::station::Fail;
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 4096;
const MAX_PATH: usize = 1000;

/// `GET /api/ship`: what the ship is set to. `can_edit` says whether this client may change the login and address.
pub async fn ship_get(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let cfg = &app.cfg;
    Json(json!({
        "ok": true,
        "projects_dir": cfg.projects_dir_now().display().to_string(),
        "listen": cfg.listen,
        "port": cfg.port,
        "user": cfg.user,
        "auth_required": cfg.auth_required(),
        "tls": cfg.tls().is_some(),
        "config_file": cfg.config_file.display().to_string(),
        "can_edit": client_is_local(addr, &headers),
    }))
    .into_response()
}

/// The settings a change may name. Passwords are never logged or echoed: a hash is written, nothing else is kept.
#[derive(Deserialize)]
pub struct ShipBody {
    confirm: Option<bool>,
    projects_dir: Option<String>,
    user: Option<String>,
    listen: Option<String>,
    password: Option<PasswordChange>,
}

#[derive(Deserialize)]
pub struct PasswordChange {
    old: Option<String>,
    new: Option<String>,
}

/// `POST /api/ship`: changes the given settings (any subset) and says which need a restart.
pub async fn ship_post(
    State(app): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<ShipBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(
            axum::http::StatusCode::FORBIDDEN,
            "cross-origin request refused",
        );
    }
    match apply(&app, client_is_local(addr, &headers), body).await {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

async fn apply(app: &Shared, local: bool, body: ShipBody) -> Result<Value, Fail> {
    if body.confirm != Some(true) {
        return Err(Fail::new(400, "нужно подтверждение: было → стало"));
    }
    if body.projects_dir.is_none()
        && body.user.is_none()
        && body.listen.is_none()
        && body.password.is_none()
    {
        return Err(Fail::new(400, "нечего менять"));
    }
    if (body.user.is_some() || body.listen.is_some()) && !local {
        return Err(Fail::new(
            403,
            "вход и адрес меняют только за компьютером с доской",
        ));
    }
    let cfg = &app.cfg;

    // Everything is checked first; nothing is written unless all of it is right.
    let dir = body
        .projects_dir
        .as_deref()
        .map(projects_dir_of)
        .transpose()?;
    let user = body.user.as_deref().map(valid_user).transpose()?;
    let listen = body.listen.as_deref().map(valid_listen).transpose()?;
    let hash = body
        .password
        .as_ref()
        .map(|p| new_hash(cfg, p))
        .transpose()?;
    let effective_listen = listen.clone().unwrap_or_else(|| cfg.listen.clone());
    let has_login = hash.is_some() || cfg.auth_required();
    if needs_password(&effective_listen, has_login) {
        return Err(Fail::new(
            400,
            format!(
                "адрес «{effective_listen}» доступен с других машин: сначала задайте пароль входа"
            ),
        ));
    }

    let mut changed = Vec::new();
    let mut values: Vec<(&str, toml::Value)> = Vec::new();
    let mut restart = false;
    if let Some(dir) = &dir {
        std::fs::create_dir_all(dir)
            .map_err(|e| Fail::new(400, format!("папку {} не создать: {e}", dir.display())))?;
        changed.push(json!({
            "name": "projects_dir",
            "was": cfg.projects_dir_now().display().to_string(),
            "now": dir.display().to_string(),
        }));
        values.push((
            "projects_dir",
            toml::Value::String(dir.display().to_string()),
        ));
    }
    if let Some(user) = &user {
        changed.push(json!({"name": "user", "was": cfg.user, "now": user}));
        values.push(("user", toml::Value::String(user.clone())));
        restart = true;
    }
    if let Some(listen) = &listen {
        changed.push(json!({"name": "listen", "was": cfg.listen, "now": listen}));
        values.push(("listen", toml::Value::String(listen.clone())));
        restart = true;
    }
    if let Some(hash) = &hash {
        // The password is kept as its hash only; the old and new words are not in the answer or the log.
        changed.push(json!({
            "name": "password",
            "was": if cfg.auth_required() { "задан" } else { "не задан" },
            "now": "задан",
        }));
        values.push(("password_hash", toml::Value::String(hash.clone())));
        restart = true;
    }
    config::set_values(&cfg.config_file, values)
        .map_err(|e| Fail::new(500, format!("настройки не записаны: {e}")))?;

    if let Some(dir) = dir {
        config::set_live_projects_dir(dir.clone());
        app.snapshot.write().await.server.projects_dir = dir.display().to_string();
        app::publish(app).await;
    }
    Ok(json!({"ok": true, "changed": changed, "restart": restart}))
}

/// A folder for new universes: absolute once `~/` is expanded. It is created when the change is made.
pub fn projects_dir_of(raw: &str) -> Result<PathBuf, Fail> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > MAX_PATH || raw.contains('\0') {
        return Err(Fail::new(400, "папка: нужен путь"));
    }
    let dir = expand_home(raw);
    if !dir.is_absolute() {
        return Err(Fail::new(400, "папка: нужен полный путь или путь от ~/"));
    }
    Ok(dir)
}

/// A login name: letters, digits and `_ . @ -`, not starting with `-`.
pub fn valid_user(user: &str) -> Result<String, Fail> {
    let ok = !user.is_empty()
        && user.len() <= 64
        && !user.starts_with('-')
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '-'));
    if ok {
        Ok(user.to_string())
    } else {
        Err(Fail::new(400, "логин: буквы, цифры и _ . @ -"))
    }
}

/// An address to listen on: an IP, `localhost`, or a host name.
pub fn valid_listen(listen: &str) -> Result<String, Fail> {
    let l = listen.trim();
    if l.parse::<IpAddr>().is_ok() || l == "localhost" {
        return Ok(l.to_string());
    }
    let labels_ok = !l.is_empty()
        && l.len() <= 253
        && l.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if labels_ok {
        Ok(l.to_string())
    } else {
        Err(Fail::new(400, "адрес: IP-адрес, localhost или имя машины"))
    }
}

/// A non-loopback address is served only with a login: the planner refuses to start without one.
pub fn needs_password(listen: &str, has_login: bool) -> bool {
    !listen_is_loopback(listen) && !has_login
}

/// The new password's hash, after the old one is checked when a login is on. The words are never kept.
fn new_hash(cfg: &AppConfig, p: &PasswordChange) -> Result<String, Fail> {
    let new = p.new.as_deref().unwrap_or("");
    if new.chars().count() < MIN_PASSWORD {
        return Err(Fail::new(
            400,
            format!("новый пароль: не меньше {MIN_PASSWORD} знаков"),
        ));
    }
    if new.len() > MAX_PASSWORD {
        return Err(Fail::new(400, "новый пароль слишком длинный"));
    }
    if cfg.auth_required() {
        let old = p.old.as_deref().unwrap_or("");
        if !config::verify_password(&cfg.password_hash, old) {
            return Err(Fail::new(401, "старый пароль не подходит"));
        }
    }
    config::hash_password(new).map_err(|_| Fail::new(500, "пароль не удалось сохранить"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_must_be_a_full_path() {
        assert!(projects_dir_of("/srv/projects").is_ok());
        assert!(projects_dir_of("~/Work").unwrap().is_absolute());
        assert!(projects_dir_of("Projects").is_err());
        assert!(projects_dir_of("  ").is_err());
        assert!(projects_dir_of("/a\0b").is_err());
    }

    #[test]
    fn a_login_name_is_plain() {
        assert_eq!(valid_user("me@host.example").unwrap(), "me@host.example");
        assert!(valid_user("-root").is_err());
        assert!(valid_user("a b").is_err());
        assert!(valid_user("").is_err());
        assert!(valid_user(&"u".repeat(65)).is_err());
    }

    #[test]
    fn an_address_is_an_ip_a_localhost_or_a_host_name() {
        assert_eq!(valid_listen(" 0.0.0.0 ").unwrap(), "0.0.0.0");
        assert_eq!(valid_listen("::1").unwrap(), "::1");
        assert_eq!(valid_listen("localhost").unwrap(), "localhost");
        assert_eq!(valid_listen("planner.lan").unwrap(), "planner.lan");
        assert!(valid_listen("[::1]").is_err());
        assert!(valid_listen("-x").is_err());
        assert!(valid_listen("a..b").is_err());
        assert!(valid_listen("bad host").is_err());
        assert!(valid_listen("").is_err());
    }

    #[test]
    fn a_public_address_needs_a_login() {
        assert!(needs_password("0.0.0.0", false));
        assert!(!needs_password("0.0.0.0", true));
        assert!(!needs_password("127.0.0.1", false));
    }

    #[test]
    fn a_password_is_at_least_eight_letters_and_never_stored_as_typed() {
        let cfg = crate::testutil::test_cfg(std::path::Path::new("/nonexistent-ship-test"));
        let short = PasswordChange {
            old: None,
            new: Some("1234567".into()),
        };
        assert!(new_hash(&cfg, &short).is_err());
        let ok = PasswordChange {
            old: None,
            new: Some("correct horse".into()),
        };
        let hash = new_hash(&cfg, &ok).unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(!hash.contains("correct horse"));
    }

    #[test]
    fn the_ship_writes_its_settings_and_keeps_the_rest_of_the_file() {
        let dir = std::env::temp_dir().join(format!("hp-ship-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        std::fs::write(&file, "port = 7333\n").unwrap();
        config::set_values(
            &file,
            vec![
                ("projects_dir", toml::Value::String("/srv/projects".into())),
                ("user", toml::Value::String("me".into())),
            ],
        )
        .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("port = 7333"));
        assert!(text.contains("projects_dir = \"/srv/projects\""));
        assert!(text.contains("user = \"me\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
