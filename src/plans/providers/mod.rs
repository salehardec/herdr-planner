//! One module per provider: where its sign-in lives on this machine, what to ask, and how the answer becomes a `Report`.
//! Each follows the same provider of herdr-gpui's `herdr-usage` crate (which ports CodexBar), so the same account reads
//! the same numbers. A provider that finds no sign-in answers `None` and is left out of the bar.

pub mod claude;
pub mod codex;
pub mod deepseek;
pub mod kimi;

use super::http::{Fail, Res};
use serde::de::DeserializeOwned;
use serde::Deserialize;

pub fn parse_json<T: DeserializeOwned>(body: &str) -> Res<T> {
    serde_json::from_str(body).map_err(|_| Fail::Parse)
}

/// Seconds, milliseconds, or RFC 3339, as services variously send times.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Timestamp {
    Number(f64),
    Text(String),
}

impl Timestamp {
    pub fn ms(&self) -> Option<u64> {
        let v = match self {
            Self::Number(n) => serde_json::Number::from_f64(*n).map(serde_json::Value::Number)?,
            Self::Text(s) => serde_json::Value::String(s.clone()),
        };
        crate::usage::parse_time_ms(Some(&v))
    }
}

pub fn ts_ms(t: &Option<Timestamp>) -> Option<u64> {
    t.as_ref().and_then(Timestamp::ms)
}

/// A number a service may send as a JSON number or as a string.
pub fn number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Loose {
        Number(f64),
        Text(String),
        Null,
    }
    Ok(match Option::<Loose>::deserialize(d)? {
        Some(Loose::Number(n)) if n.is_finite() => Some(n),
        Some(Loose::Text(t)) => t.trim().parse().ok().filter(|n: &f64| n.is_finite()),
        _ => None,
    })
}

/// When a JWT (the middle of the token) says it stops working, in ms. A token that is no JWT says nothing.
pub fn jwt_exp_ms(token: &super::http::Secret) -> Option<u64> {
    use base64::Engine;
    let text = token.reveal_for_claims();
    let payload = text.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp")?.as_f64().filter(|e| *e > 0.).map(|e| (e * 1000.) as u64)
}

/// A token or key found on this machine: which program's file it came from (`pi`, `omp`, `claude`, `codex`, `kimi`, `opencode`, or the planner's
/// own `config` and `env`), the crew role that runs the provider through that program (empty when none does), and when it stops working (ms),
/// when that is known.
pub struct Cred {
    pub token: super::http::Secret,
    pub expires_ms: Option<u64>,
    pub from: &'static str,
    pub role: String,
}

impl Cred {
    pub fn new(token: super::http::Secret, expires_ms: Option<u64>, from: &'static str) -> Self {
        Self { token, expires_ms, from, role: String::new() }
    }

    pub fn stale(&self, now: u64) -> bool {
        self.expires_ms.is_some_and(|e| e > 0 && e <= now) || (self.expires_ms.is_none() && jwt_exp_ms(&self.token).is_some_and(|e| e <= now))
    }
}

/// Says where a reading's sign-in came from: on the report, for the card.
pub fn stamp(report: super::http::Res<super::model::Report>, cred: &Cred) -> super::http::Res<super::model::Report> {
    report.map(|mut r| {
        r.from = cred.from.to_string();
        r.role = cred.role.clone();
        r
    })
}

/// What an agent keeps for a provider: the sign-in, and the account when it says whose it is.
pub struct Found {
    pub cred: Cred,
    pub email: Option<String>,
    pub account_id: Option<String>,
}

/// The name an agent keeps a provider's account under.
fn kept_as(agent: &str, provider: &str) -> Option<&'static str> {
    Some(match (agent, provider) {
        ("pi", "deepseek") | ("omp", "deepseek") => "deepseek",
        ("pi", "kimi") => "kimi-coding",
        ("omp", "kimi") => "kimi-code",
        ("pi", "codex") => "openai",
        ("omp", "codex") => "openai-codex",
        ("pi", "claude") | ("omp", "claude") => "anthropic",
        _ => return None,
    })
}

/// The key or OAuth sign-in `agent` (pi or omp) holds for `provider`, read and never written: pi's `auth.json`, omp's credential table.
pub fn agent_login(src: &super::Sources, agent: &'static str, provider: &str) -> Option<Found> {
    let name = kept_as(agent, provider)?;
    match agent {
        "pi" => {
            let auth = super::http::read_json(&src.pi_auth)?;
            found_in(auth.get(name)?, "pi")
        }
        "omp" => found_in(&omp_entry(&src.omp_db, name)?, "omp"),
        _ => None,
    }
}

/// An entry as pi and omp both keep it: `{"type": "api_key", "key": …}` or `{"type": "oauth", "access": …, "expires": ms, …}`.
fn found_in(entry: &serde_json::Value, from: &'static str) -> Option<Found> {
    let kind = super::http::text_at(entry, &["type"]);
    let (token, expires_ms) = match kind.as_deref() {
        Some("api_key") => (super::http::secret_at(entry, &["key"])?, None),
        Some("oauth") | None => (
            super::http::secret_at(entry, &["access"]).or_else(|| super::http::secret_at(entry, &["key"]))?,
            entry.get("expires").and_then(|v| v.as_u64()),
        ),
        Some(_) => return None,
    };
    Some(Found {
        cred: Cred::new(token, expires_ms, from),
        email: super::http::text_at(entry, &["email"]),
        account_id: super::http::text_at(entry, &["accountId"]),
    })
}

/// One row of omp's credential table (`agent.db`, `auth_credentials`), as the JSON it keeps, with its type: opened read-only, the newest row
/// that is not disabled. omp renews its own tokens; nothing is written.
fn omp_entry(db: &std::path::Path, provider: &str) -> Option<serde_json::Value> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    conn.busy_timeout(std::time::Duration::from_millis(500)).ok()?;
    let (kind, data): (String, String) = conn
        .query_row(
            "SELECT credential_type, data FROM auth_credentials WHERE provider = ?1 AND disabled_cause IS NULL ORDER BY updated_at DESC, id DESC LIMIT 1",
            [provider],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    let mut v: serde_json::Value = serde_json::from_str(&data).ok()?;
    v.as_object_mut()?.insert("type".into(), kind.into());
    Some(v)
}

/// The OAuth sign-in pi keeps for `name`. Kept for the readers of pi's file that want it by name.
#[cfg(test)]
pub fn pi_oauth(src: &super::Sources, name: &str) -> Option<Cred> {
    let auth = super::http::read_json(&src.pi_auth)?;
    let entry = auth.get(name)?;
    if super::http::text_at(entry, &["type"]).as_deref().is_some_and(|t| t != "oauth") {
        return None;
    }
    found_in(entry, "pi").map(|f| f.cred)
}

#[cfg(test)]
pub fn pi_key(src: &super::Sources, name: &str) -> Option<super::http::Secret> {
    let auth = super::http::read_json(&src.pi_auth)?;
    let entry = auth.get(name)?;
    if super::http::text_at(entry, &["type"]).as_deref().is_some_and(|t| t != "api_key") {
        return None;
    }
    found_in(entry, "pi").map(|f| f.cred.token)
}

/// The sign-in to ask with, from the candidates (best first). When the crew runs the provider through some of the programs (their candidates
/// carry a role), only those count: the first of them that has not expired, and when all have, that is the reason. Falling back to another
/// program's account would show somebody else's limits as if they were the ones the board spends. Without any routed candidate the first that
/// still works wins, and when all have expired, that is the reason.
pub fn first_fresh<T>(candidates: Vec<(Cred, T)>, now: u64) -> Option<super::http::Res<(Cred, T)>> {
    if candidates.is_empty() {
        return None;
    }
    let routed = candidates.iter().any(|(c, _)| !c.role.is_empty());
    Some(candidates.into_iter().filter(|(c, _)| !routed || !c.role.is_empty()).find(|(c, _)| !c.stale(now)).ok_or(super::http::Fail::Expired))
}

/// A claim of the JWT a token is (the account id of a ChatGPT sign-in, say), or none when the token is no JWT.
pub fn jwt_claim(token: &super::http::Secret, path: &[&str]) -> Option<String> {
    use base64::Engine;
    let payload = token.reveal_for_claims().split('.').nth(1)?.to_string();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    path.iter().try_fold(&v, |v, k| v.get(*k))?.as_str().map(str::to_string).filter(|s| !s.is_empty())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::plans::Sources;

    pub fn jwt(claims: &serde_json::Value) -> String {
        use base64::Engine;
        let b = |v: &serde_json::Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.sig", b(&serde_json::json!({"alg": "none"})), b(claims))
    }

    /// A stand-in for omp's credential table: rows of (provider, type, data, disabled_cause).
    pub fn omp_db(path: &std::path::Path, rows: &[(&str, &str, &str, Option<&str>)]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE auth_credentials (id INTEGER PRIMARY KEY AUTOINCREMENT, provider TEXT NOT NULL, credential_type TEXT NOT NULL, data TEXT NOT NULL, disabled_cause TEXT DEFAULT NULL, identity_key TEXT DEFAULT NULL, created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        for (i, (p, t, d, off)) in rows.iter().enumerate() {
            conn.execute("INSERT INTO auth_credentials (provider, credential_type, data, disabled_cause, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)", rusqlite::params![p, t, d, off, i as i64]).unwrap();
        }
    }

    /// A stand-in for pi's `auth.json`: fake keys and tokens, the shape pi writes.
    pub fn pi_file(root: &std::path::Path, body: &serde_json::Value) {
        let p = root.join(".pi/agent/auth.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body.to_string()).unwrap();
    }

    #[test]
    fn pis_auth_file_is_read_for_keys_and_oauth_tokens_and_never_written() {
        let tmp = tempfile::tempdir().unwrap();
        let src = Sources::for_tests(tmp.path());
        assert!(pi_key(&src, "deepseek").is_none() && pi_oauth(&src, "openai").is_none(), "no file: nothing, not a failure");
        let access = jwt(&serde_json::json!({"exp": 4102444800u64, "https://api.openai.com/auth": {"chatgpt_account_id": "acct-1"}}));
        let body = serde_json::json!({
            "deepseek": {"type": "api_key", "key": "sk-fake-deepseek"},
            "kimi-coding": {"type": "oauth", "access": "kimi-access", "refresh": "kimi-refresh", "expires": 4102444800000u64},
            "openai": {"type": "oauth", "access": access, "refresh": "r", "expires": 1000u64},
            "orcarouter": {"type": "api_key", "key": "orca"},
        });
        pi_file(tmp.path(), &body);
        let before = std::fs::read(&src.pi_auth).unwrap();
        assert_eq!(format!("{:?}", pi_key(&src, "deepseek").unwrap()), "Secret(…)");
        assert!(pi_key(&src, "kimi-coding").is_none(), "an oauth entry is no API key");
        assert!(pi_oauth(&src, "deepseek").is_none(), "an API key is no oauth sign-in");
        assert!(pi_key(&src, "anthropic").is_none() && pi_oauth(&src, "anthropic").is_none());
        let kimi = pi_oauth(&src, "kimi-coding").unwrap();
        assert!(!kimi.stale(1_792_000_000_000) && kimi.stale(4_102_444_800_000));
        let openai = pi_oauth(&src, "openai").unwrap();
        assert!(openai.stale(1_792_000_000_000), "its own `expires` has passed");
        assert_eq!(jwt_claim(&openai.token, &["https://api.openai.com/auth", "chatgpt_account_id"]).as_deref(), Some("acct-1"));
        assert_eq!(jwt_claim(&kimi.token, &["x"]), None, "not a JWT");
        assert_eq!(std::fs::read(&src.pi_auth).unwrap(), before, "the file is only read");
    }

    #[test]
    fn a_broken_or_odd_pi_file_gives_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let src = Sources::for_tests(tmp.path());
        let p = tmp.path().join(".pi/agent/auth.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{ nope").unwrap();
        assert!(pi_key(&src, "deepseek").is_none());
        pi_file(tmp.path(), &serde_json::json!({"deepseek": {"type": "api_key", "key": "  "}, "openai": "x"}));
        assert!(pi_key(&src, "deepseek").is_none() && pi_oauth(&src, "openai").is_none());
    }

    #[test]
    fn the_first_working_sign_in_wins_and_when_all_have_expired_that_is_the_reason() {
        let c = |ms: Option<u64>| Cred::new(crate::plans::http::Secret::new("t").unwrap(), ms, "test");
        assert!(first_fresh::<u8>(vec![], 10).is_none());
        let got = first_fresh(vec![(c(Some(5)), 1), (c(Some(50)), 2), (c(None), 3)], 10).unwrap().unwrap();
        assert_eq!(got.1, 2);
        assert_eq!(first_fresh(vec![(c(Some(5)), 1)], 10).unwrap().err(), Some(crate::plans::http::Fail::Expired));
        assert_eq!(first_fresh(vec![(c(None), 7)], 10).unwrap().unwrap().1, 7, "no expiry known: tried");
        // a program the crew runs the provider through: its dead sign-in is the answer, not somebody else's live one
        let routed = |ms: u64| { let mut x = c(Some(ms)); x.role = "worker".into(); x };
        assert_eq!(first_fresh(vec![(routed(5), 1), (c(Some(50)), 2)], 10).unwrap().err(), Some(crate::plans::http::Fail::Expired));
        assert_eq!(first_fresh(vec![(routed(5), 1), (routed(50), 2), (c(Some(50)), 3)], 10).unwrap().unwrap().1, 2);
    }

    #[test]
    fn omps_credential_table_is_read_read_only_and_a_disabled_or_missing_row_is_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let src = Sources::for_tests(tmp.path());
        assert!(agent_login(&src, "omp", "deepseek").is_none(), "no database: nothing, not a failure");
        omp_db(
            &src.omp_db,
            &[
                ("deepseek", "api_key", r#"{"key":"old-omp-key"}"#, None),
                ("deepseek", "api_key", r#"{"key":"new-omp-key"}"#, None),
                ("anthropic", "oauth", r#"{"access":"a","refresh":"r","expires":4102444800000,"email":"me@x.y"}"#, None),
                ("openai-codex", "oauth", r#"{"access":"c","refresh":"r","expires":1000,"accountId":"acct-9"}"#, None),
                ("kimi-code", "oauth", r#"{"access":"k","refresh":"r","expires":4102444800000}"#, Some("revoked")),
            ],
        );
        let before = std::fs::read(&src.omp_db).unwrap();
        let key = agent_login(&src, "omp", "deepseek").unwrap();
        assert_eq!((key.cred.from, key.cred.expires_ms), ("omp", None));
        let claude = agent_login(&src, "omp", "claude").unwrap();
        assert_eq!(claude.email.as_deref(), Some("me@x.y"));
        assert!(!claude.cred.stale(1_792_000_000_000));
        let codex = agent_login(&src, "omp", "codex").unwrap();
        assert_eq!(codex.account_id.as_deref(), Some("acct-9"));
        assert!(codex.cred.stale(1_792_000_000_000));
        assert!(agent_login(&src, "omp", "kimi").is_none(), "a disabled credential is not used");
        assert!(agent_login(&src, "opencode", "deepseek").is_none() && agent_login(&src, "claude", "claude").is_none());
        assert_eq!(std::fs::read(&src.omp_db).unwrap(), before, "the database is only read");
    }
}
