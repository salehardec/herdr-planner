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

/// A token or key found on this machine, and when it stops working (ms), when that is known.
pub struct Cred {
    pub token: super::http::Secret,
    pub expires_ms: Option<u64>,
}

impl Cred {
    pub fn stale(&self, now: u64) -> bool {
        self.expires_ms.is_some_and(|e| e > 0 && e <= now) || (self.expires_ms.is_none() && jwt_exp_ms(&self.token).is_some_and(|e| e <= now))
    }
}

/// The OAuth sign-in pi keeps for `name` in its `auth.json` (`{"<name>": {"type": "oauth", "access": …, "expires": ms}}`). Read only:
/// pi renews its own tokens when it runs, and the planner never writes the file or spends the refresh token.
pub fn pi_oauth(src: &super::Sources, name: &str) -> Option<Cred> {
    let auth = super::http::read_json(&src.pi_auth)?;
    let entry = auth.get(name)?;
    if super::http::text_at(entry, &["type"]).as_deref().is_some_and(|t| t != "oauth") {
        return None;
    }
    Some(Cred { token: super::http::secret_at(entry, &["access"])?, expires_ms: entry.get("expires").and_then(|v| v.as_u64()) })
}

/// The API key pi keeps for `name` (`{"<name>": {"type": "api_key", "key": …}}`).
pub fn pi_key(src: &super::Sources, name: &str) -> Option<super::http::Secret> {
    let auth = super::http::read_json(&src.pi_auth)?;
    let entry = auth.get(name)?;
    if super::http::text_at(entry, &["type"]).as_deref().is_some_and(|t| t != "api_key") {
        return None;
    }
    super::http::secret_at(entry, &["key"])
}

/// The first of the candidates (best first) that still works; when there are some and all have expired, why there is nothing.
pub fn first_fresh<T>(candidates: Vec<(Cred, T)>, now: u64) -> Option<super::http::Res<(Cred, T)>> {
    if candidates.is_empty() {
        return None;
    }
    Some(candidates.into_iter().find(|(c, _)| !c.stale(now)).ok_or(super::http::Fail::Expired))
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
        let c = |ms: Option<u64>| Cred { token: crate::plans::http::Secret::new("t").unwrap(), expires_ms: ms };
        assert!(first_fresh::<u8>(vec![], 10).is_none());
        let got = first_fresh(vec![(c(Some(5)), 1), (c(Some(50)), 2), (c(None), 3)], 10).unwrap().unwrap();
        assert_eq!(got.1, 2);
        assert_eq!(first_fresh(vec![(c(Some(5)), 1)], 10).unwrap().err(), Some(crate::plans::http::Fail::Expired));
        assert_eq!(first_fresh(vec![(c(None), 7)], 10).unwrap().unwrap().1, 7, "no expiry known: tried");
    }
}
