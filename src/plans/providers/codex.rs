//! Codex plan usage, read with the Codex CLI's ChatGPT sign-in (`$CODEX_HOME/auth.json`, default `~/.codex`), falling back to
//! OpenCode's OpenAI OAuth sign-in. API keys do not provide plan usage. The CLI renews its token when it runs.

use super::{jwt_exp_ms, number, parse_json, ts_ms, Timestamp};
use crate::plans::http::{self, text_at, Fail, Request, Res, Secret};
use crate::plans::model::{title_case, Account, Kind, Report, Section, Window, SESSION, WEEK};
use crate::plans::Sources;
use serde::Deserialize;

pub const ID: &str = "codex";
const URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const AGENT: &str = "codex-cli";
/// A window is known by its length within this much; which slot it arrives in only decides when the length is missing.
const SLACK_MS: u64 = 60_000;

pub async fn fetch(src: &Sources, now: u64) -> Option<Res<Report>> {
    let (token, account_id) = codex_auth(src).or_else(|| opencode_auth(src))?;
    if jwt_exp_ms(&token).is_some_and(|exp| exp <= now) {
        return Some(Err(Fail::Expired));
    }
    let mut req = Request::get(ID, URL)
        .bearer(&token)
        .header("User-Agent", AGENT)
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "Codex Desktop");
    if let Some(account) = account_id {
        req = req.secret_header("ChatGPT-Account-Id", "", &account);
    }
    Some(http::body(req).await.and_then(|body| parse(&body)))
}

fn codex_auth(src: &Sources) -> Option<(Secret, Option<Secret>)> {
    let auth = http::read_json(&src.codex_dir.join("auth.json"))?;
    let token = http::secret_at(&auth, &["tokens", "access_token"])?;
    Some((token, http::secret_at(&auth, &["tokens", "account_id"])))
}

/// OpenCode renews and saves this token itself; each poll reads the current file.
fn opencode_auth(src: &Sources) -> Option<(Secret, Option<Secret>)> {
    let auth = http::read_json(&src.opencode_auth)?;
    if text_at(&auth, &["openai", "type"]).as_deref() != Some("oauth") {
        return None;
    }
    Some((http::secret_at(&auth, &["openai", "access"])?, http::secret_at(&auth, &["openai", "accountId"])))
}

pub fn parse(body: &str) -> Res<Report> {
    let usage: Usage = parse_json(body)?;
    let windows = usage.rate_limit.map(Limits::windows).unwrap_or_default();
    let review = usage.code_review_rate_limit.map(Limits::windows).unwrap_or_default().into_iter().next().map(|w| {
        Section::Limit(Window { kind: Kind::Named("Code review".into()), ..w })
    });
    let resets = usage.rate_limit_reset_credits.map(|c| Section::Facts {
        title: "Limit reset credits".into(),
        facts: vec![("Available".into(), c.available_count.to_string())],
    });
    let credits = usage.credits.map(|c| Section::Facts {
        title: "Credits".into(),
        facts: vec![(
            "Balance".into(),
            if c.unlimited.unwrap_or(false) { "Unlimited".into() } else { c.balance.unwrap_or_else(|| "0".into()) },
        )],
    });
    let account = Account {
        email: usage.email.filter(|e| !e.trim().is_empty()),
        plan: usage.plan_type.map(|p| title_case(&p)).filter(|p| !p.is_empty()),
    };
    let mut report = Report::new(ID, account, windows);
    report.sections.extend(review.into_iter().chain(resets).chain(credits));
    if !report.has_numbers() && report.sections.is_empty() {
        return Err(Fail::NoPlan);
    }
    Ok(report)
}

#[derive(Deserialize)]
struct Usage {
    email: Option<String>,
    plan_type: Option<String>,
    rate_limit: Option<Limits>,
    code_review_rate_limit: Option<Limits>,
    rate_limit_reset_credits: Option<ResetCredits>,
    credits: Option<Credits>,
}

#[derive(Deserialize)]
struct Limits {
    primary_window: Option<LimitWindow>,
    secondary_window: Option<LimitWindow>,
}

impl Limits {
    fn windows(self) -> Vec<Window> {
        [(Kind::Session, self.primary_window), (Kind::Weekly, self.secondary_window)]
            .into_iter()
            .filter_map(|(slot, w)| {
                let w = w?;
                let length = w.limit_window_seconds.map(|s| s.saturating_mul(1000));
                let kind = match length {
                    Some(l) if l.abs_diff(SESSION) <= SLACK_MS => Kind::Session,
                    Some(l) if l.abs_diff(WEEK) <= SLACK_MS => Kind::Weekly,
                    _ => slot,
                };
                Some(Window::new(kind, w.used_percent?, ts_ms(&w.reset_at), length))
            })
            .collect()
    }
}

#[derive(Deserialize)]
struct LimitWindow {
    #[serde(default, deserialize_with = "number")]
    used_percent: Option<f64>,
    limit_window_seconds: Option<u64>,
    reset_at: Option<Timestamp>,
}

#[derive(Deserialize)]
struct ResetCredits {
    available_count: u32,
}

#[derive(Deserialize)]
struct Credits {
    unlimited: Option<bool>,
    balance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_two_windows_by_their_length() {
        let body = r#"{"email":"me@x.y","plan_type":"plus",
          "rate_limit":{"primary_window":{"used_percent":0,"limit_window_seconds":18000,"reset_at":1792000000},
                        "secondary_window":{"used_percent":12.5,"limit_window_seconds":604800,"reset_at":"2026-11-05T04:42:15+00:00"}},
          "code_review_rate_limit":{"primary_window":{"used_percent":3,"limit_window_seconds":604800,"reset_at":1792100000}},
          "rate_limit_reset_credits":{"available_count":2},
          "credits":{"unlimited":false,"balance":"7.50"}}"#;
        let r = parse(body).unwrap();
        assert_eq!(r.account, Account { email: Some("me@x.y".into()), plan: Some("Plus".into()) });
        assert_eq!(r.windows.len(), 2);
        assert_eq!((r.windows[0].kind.clone(), r.windows[0].used, r.windows[0].length_ms), (Kind::Session, 0., Some(SESSION)));
        assert_eq!((r.windows[1].kind.clone(), r.windows[1].used, r.windows[1].length_ms), (Kind::Weekly, 12.5, Some(WEEK)));
        assert_eq!(r.windows[0].resets_at_ms, Some(1_792_000_000_000));
        assert_eq!(r.windows[1].resets_at_ms, Some(1_793_853_735_000));
        assert!(matches!(&r.sections[0], Section::Limit(w) if w.kind == Kind::Named("Code review".into()) && w.used == 3.));
        assert_eq!(r.sections[1], Section::Facts { title: "Limit reset credits".into(), facts: vec![("Available".into(), "2".into())] });
        assert_eq!(r.sections[2], Section::Facts { title: "Credits".into(), facts: vec![("Balance".into(), "7.50".into())] });
    }

    #[test]
    fn a_window_of_an_unfamiliar_length_keeps_its_slot_and_a_string_percent_is_read() {
        let body = r#"{"rate_limit":{"primary_window":{"used_percent":"40","limit_window_seconds":3600,"reset_at":1792000000}}}"#;
        let r = parse(body).unwrap();
        assert_eq!(r.windows.len(), 1);
        assert_eq!(r.windows[0].kind, Kind::Session);
        assert_eq!(r.windows[0].used, 40.);
        assert_eq!(r.windows[0].length_ms, Some(3_600_000));
    }

    #[test]
    fn nothing_to_show_is_a_failure_not_an_empty_card() {
        assert_eq!(parse(r#"{"email":"a@b.c"}"#), Err(Fail::NoPlan));
        assert_eq!(parse("nope"), Err(Fail::Parse));
    }
}
