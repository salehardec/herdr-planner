//! Codex plan usage, read with pi's ChatGPT sign-in (`~/.pi/agent/auth.json`), else the Codex CLI's ChatGPT sign-in (`$CODEX_HOME/auth.json`, default `~/.codex`), falling back to
//! OpenCode's OpenAI OAuth sign-in. API keys do not provide plan usage. The CLI renews its token when it runs.

use super::{agent_login, first_fresh, jwt_claim, number, parse_json, stamp, ts_ms, Cred, Timestamp};
use crate::plans::http::{self, text_at, Fail, Request, Res, Secret};
use crate::plans::model::{title_case, Account, Kind, Report, Section, Window, SESSION, WEEK};
use crate::plans::routes::agents_for;
use crate::plans::Sources;
use serde::Deserialize;

pub const ID: &str = "codex";
const URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const AGENT: &str = "codex-cli";
/// A window is known by its length within this much; which slot it arrives in only decides when the length is missing.
const SLACK_MS: u64 = 60_000;

pub async fn fetch(src: &Sources, now: u64) -> Option<Res<Report>> {
    let (cred, account_id) = match choose(src, now)? {
        Ok(found) => found,
        Err(fail) => return Some(Err(fail)),
    };
    let mut req = Request::get(ID, URL)
        .bearer(&cred.token)
        .header("User-Agent", AGENT)
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "Codex Desktop");
    if let Some(account) = account_id {
        req = req.secret_header("ChatGPT-Account-Id", "", &account);
    }
    Some(stamp(http::body(req).await.and_then(|body| parse(&body)), &cred))
}

/// The sign-in to ask with, best first: that of the program the crew runs Codex's account through (the Codex CLI, or pi or omp when a role names
/// them), then the other places an account can be (the Codex CLI, pi, omp, OpenCode). The first that has not expired wins; when all have, that
/// is the reason. None of them is renewed here.
pub fn choose(src: &Sources, now: u64) -> Option<Res<(Cred, Option<Secret>)>> {
    let mut candidates = Vec::new();
    for (agent, role) in agents_for(&src.routes, ID) {
        let found = match agent {
            "codex" => codex_auth(src),
            "opencode" => opencode_auth(src),
            "pi" | "omp" => agent_login(src, agent, ID).map(|f| {
                // the account id is kept beside the token, or is in it
                let account = f.account_id.and_then(Secret::new).or_else(|| jwt_claim(&f.cred.token, &["https://api.openai.com/auth", "chatgpt_account_id"]).and_then(Secret::new));
                (f.cred, account)
            }),
            _ => None,
        };
        if let Some((mut cred, account)) = found {
            cred.role = role;
            candidates.push((cred, account));
        }
    }
    first_fresh(candidates, now)
}

fn codex_auth(src: &Sources) -> Option<(Cred, Option<Secret>)> {
    let auth = http::read_json(&src.codex_dir.join("auth.json"))?;
    let token = http::secret_at(&auth, &["tokens", "access_token"])?;
    Some((Cred::new(token, None, "codex"), http::secret_at(&auth, &["tokens", "account_id"])))
}

/// OpenCode renews and saves this token itself; each poll reads the current file.
fn opencode_auth(src: &Sources) -> Option<(Cred, Option<Secret>)> {
    let auth = http::read_json(&src.opencode_auth)?;
    if text_at(&auth, &["openai", "type"]).as_deref() != Some("oauth") {
        return None;
    }
    let token = http::secret_at(&auth, &["openai", "access"])?;
    Some((Cred::new(token, auth.pointer("/openai/expires").and_then(|v| v.as_u64()), "opencode"), http::secret_at(&auth, &["openai", "accountId"])))
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

    #[test]
    fn the_sign_in_is_of_the_program_the_crew_runs_the_account_through() {
        use crate::plans::providers::tests::{jwt, pi_file};
        use crate::plans::routes::routes_of;
        let tmp = tempfile::tempdir().unwrap();
        let mut src = Sources::for_tests(tmp.path());
        let now = 1_792_000_000_000u64;
        assert!(choose(&src, now).is_none());
        std::fs::create_dir_all(&src.codex_dir).unwrap();
        let cli = jwt(&serde_json::json!({"exp": 4102444800u64}));
        std::fs::write(src.codex_dir.join("auth.json"), serde_json::json!({"tokens": {"access_token": cli, "account_id": "cli-acct"}}).to_string()).unwrap();
        let (cred, account) = choose(&src, now).unwrap().unwrap();
        assert!(account.is_some() && !cred.stale(now) && cred.from == "codex");
        // pi has the ChatGPT sign-in too, but no role runs Codex through pi: the CLI's is first
        let access = jwt(&serde_json::json!({"exp": 4102444800u64, "https://api.openai.com/auth": {"chatgpt_account_id": "pi-acct"}}));
        pi_file(tmp.path(), &serde_json::json!({"openai": {"type": "oauth", "access": access, "refresh": "r", "expires": 4102444800000u64}}));
        assert_eq!(choose(&src, now).unwrap().unwrap().0.from, "codex");
        // a role runs it through pi: pi's is first, with the account id that is in the token, and the role is kept
        src.routes = routes_of(&[("reviewer.agent".to_string(), "pi/openai/gpt-5".to_string())]);
        let (pi_cred, pi_account) = choose(&src, now).unwrap().unwrap();
        assert_eq!((pi_cred.from, pi_cred.role.as_str()), ("pi", "reviewer"));
        assert_ne!(pi_cred.token, cred.token, "pi's token, not the CLI's");
        assert!(pi_account.is_some());
        // pi's has expired, the CLI's has not: Codex is run through pi, so the reason is said and the CLI's other account is not shown instead
        pi_file(tmp.path(), &serde_json::json!({"openai": {"type": "oauth", "access": access, "expires": 5u64}}));
        assert_eq!(choose(&src, now).unwrap().err(), Some(Fail::Expired));
        // with no role naming pi, the CLI's is used
        src.routes = Vec::new();
        assert_eq!(choose(&src, now).unwrap().unwrap().0.token, cred.token);
        // the CLI's token is a JWT that expired: nothing works, and that is said
        let old = jwt(&serde_json::json!({"exp": 1000u64}));
        std::fs::write(src.codex_dir.join("auth.json"), serde_json::json!({"tokens": {"access_token": old}}).to_string()).unwrap();
        assert_eq!(choose(&src, now).unwrap().err(), Some(Fail::Expired));
    }
}
