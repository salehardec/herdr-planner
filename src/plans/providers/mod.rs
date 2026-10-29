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
