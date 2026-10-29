//! Claude plan usage, read with the sign-in of the program the crew runs Claude through (Claude Code's `.credentials.json`, or pi's `auth.json`, or omp's
//! credential table), else Claude Code's own sign-in: `.credentials.json` in its config directory (`$CLAUDE_CONFIG_DIR`,
//! else `~/.claude`). A Claude Code pointed at another endpoint, or signing in through an API key helper, is not using that
//! sign-in, so it is left alone. The token is Claude Code's own: it renews it when it runs, the planner never does.

use super::{agent_login, first_fresh, parse_json, stamp, ts_ms, Cred, Timestamp};
use crate::plans::http::{self, text_at, Fail, Request, Res};
use crate::plans::model::{title_case, Account, Kind, Report, Section, Window, SESSION, WEEK};
use crate::plans::routes::agents_for;
use crate::plans::Sources;
use serde::Deserialize;

pub const ID: &str = "claude";
const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA: &str = "oauth-2025-04-20";
/// The usage endpoint serves Claude Code's OAuth clients.
const AGENT: &str = "claude-code/2.1.0";
const API: &str = "https://api.anthropic.com";

pub async fn fetch(src: &Sources, now: u64) -> Option<Res<Report>> {
    let (cred, account) = match choose(src, now)? {
        Ok(found) => found,
        Err(fail) => return Some(Err(fail)),
    };
    let req = Request::get(ID, URL).bearer(&cred.token).header("anthropic-beta", BETA).header("User-Agent", AGENT);
    Some(stamp(http::body(req).await.and_then(|body| parse(&body, account)), &cred))
}

/// The sign-in to ask with: the one of the program the crew runs Claude through (Claude Code itself, or pi or omp when a role names them), the
/// other places an account can be, and last the planner's own environment. The first that has not expired wins; neither is renewed here: the
/// programs renew theirs when they run, and a dead one only says so.
pub fn choose(src: &Sources, now: u64) -> Option<Res<(Cred, Account)>> {
    let mut candidates = Vec::new();
    for (agent, role) in agents_for(&src.routes, ID) {
        let found = match agent {
            "claude" => claude_code(src),
            "pi" | "omp" => agent_login(src, agent, ID).map(|f| (f.cred, Account { email: f.email, plan: None })),
            _ => None,
        };
        if let Some((mut cred, account)) = found {
            cred.role = role;
            candidates.push((cred, account));
        }
    }
    if let Some((token, from)) = src.secret_from(ID, "access_token", &["CLAUDE_CODE_OAUTH_TOKEN"]) {
        candidates.push((Cred::new(token, None, from), Account::default()));
    }
    first_fresh(candidates, now)
}

/// Claude Code's own sign-in: `.credentials.json` of its config directory.
fn claude_code(src: &Sources) -> Option<(Cred, Account)> {
    let settings = http::read_json(&src.claude_dir.join("settings.json"));
    if elsewhere(src, settings.as_ref()) {
        return None;
    }
    let credentials = http::read_json(&src.claude_dir.join(".credentials.json"))?;
    let token = http::secret_at(&credentials, &["claudeAiOauth", "accessToken"])?;
    let expires_ms = credentials.pointer("/claudeAiOauth/expiresAt").and_then(|v| v.as_u64());
    let email = [src.claude_dir.join(".claude.json"), src.home.join(".claude.json")]
        .iter()
        .find_map(|p| http::read_json(p).and_then(|v| text_at(&v, &["oauthAccount", "emailAddress"])));
    let plan = plan(
        text_at(&credentials, &["claudeAiOauth", "subscriptionType"]).as_deref(),
        text_at(&credentials, &["claudeAiOauth", "rateLimitTier"]).as_deref(),
    );
    Some((Cred::new(token, expires_ms, "claude"), Account { email, plan }))
}

/// Whether Claude Code works without its OAuth sign-in: `ANTHROPIC_BASE_URL` (from the environment or the `env` of its
/// settings) names another endpoint, or an `apiKeyHelper` supplies its key. An OAuth token still in the file is then a
/// leftover, possibly revoked.
fn elsewhere(src: &Sources, settings: Option<&serde_json::Value>) -> bool {
    let base = src
        .env("ANTHROPIC_BASE_URL")
        .or_else(|| settings.and_then(|s| text_at(s, &["env", "ANTHROPIC_BASE_URL"])));
    base.is_some_and(|url| !anthropic(&url)) || settings.and_then(|s| text_at(s, &["apiKeyHelper"])).is_some()
}

fn anthropic(url: &str) -> bool {
    let url = url.trim().trim_end_matches('/');
    url == API || url.strip_prefix(API).is_some_and(|rest| rest.starts_with('/'))
}

/// `max` on tier `default_claude_max_20x` is `Max 20x`.
pub fn plan(subscription: Option<&str>, tier: Option<&str>) -> Option<String> {
    let name = title_case(subscription?);
    let multiple = tier.and_then(|t| t.rsplit('_').next()).filter(|last| {
        last.strip_suffix('x').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    });
    (!name.is_empty()).then(|| match multiple {
        Some(m) => format!("{name} {m}"),
        None => name,
    })
}

pub fn parse(body: &str, account: Account) -> Res<Report> {
    let usage: Usage = parse_json(body)?;
    let mut windows: Vec<Window> = usage
        .limits
        .unwrap_or_default()
        .into_iter()
        .filter_map(|limit| {
            let kind = match limit.kind.as_str() {
                "session" => Kind::Session,
                "weekly_all" => Kind::Weekly,
                "weekly_scoped" => Kind::Named(
                    limit.scope?.model?.display_name.filter(|n| !n.trim().is_empty())?,
                ),
                _ => return None,
            };
            let length = if kind == Kind::Session { SESSION } else { WEEK };
            Some(Window::new(kind, limit.percent?, ts_ms(&limit.resets_at), Some(length)))
        })
        .collect();
    if windows.is_empty() {
        windows = [(Kind::Session, usage.five_hour, SESSION), (Kind::Weekly, usage.seven_day, WEEK)]
            .into_iter()
            .filter_map(|(kind, w, length)| {
                let w = w?;
                Some(Window::new(kind, w.utilization?, ts_ms(&w.resets_at), Some(length)))
            })
            .collect();
    }
    let shares: Vec<(String, f64)> = usage
        .seven_day_breakdown
        .map(|b| b.rows)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| Some((row.display_name?, row.percent.filter(|p| *p > 0.)?)))
        .collect();
    let spend = usage.spend.map(|spend| Section::Facts {
        title: "Extra usage".into(),
        facts: if spend.enabled.unwrap_or(false) {
            let used = spend.used.as_ref().and_then(Money::text);
            let limit = spend.limit.as_ref().and_then(Money::text);
            vec![(
                "This month".into(),
                match (used, limit) {
                    (Some(u), Some(l)) => format!("{u} of {l}"),
                    (Some(u), None) => u,
                    _ => "On".into(),
                },
            )]
        } else {
            vec![("Status".into(), "Off".into())]
        },
    });
    let mut report = Report::new(ID, account, windows);
    if !shares.is_empty() {
        report.sections.push(Section::Shares { title: "This week by surface".into(), shares });
    }
    report.sections.extend(spend);
    if !report.has_numbers() {
        return Err(Fail::NoPlan);
    }
    Ok(report)
}

#[derive(Deserialize)]
struct Usage {
    limits: Option<Vec<Limit>>,
    five_hour: Option<FixedWindow>,
    seven_day: Option<FixedWindow>,
    seven_day_breakdown: Option<Breakdown>,
    spend: Option<Spend>,
}

#[derive(Deserialize)]
struct Limit {
    kind: String,
    percent: Option<f64>,
    resets_at: Option<Timestamp>,
    scope: Option<Scope>,
}

#[derive(Deserialize)]
struct Scope {
    model: Option<Model>,
}

#[derive(Deserialize)]
struct Model {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct FixedWindow {
    utilization: Option<f64>,
    resets_at: Option<Timestamp>,
}

#[derive(Deserialize)]
struct Breakdown {
    rows: Vec<BreakdownRow>,
}

#[derive(Deserialize)]
struct BreakdownRow {
    display_name: Option<String>,
    percent: Option<f64>,
}

#[derive(Deserialize)]
struct Spend {
    enabled: Option<bool>,
    used: Option<Money>,
    limit: Option<Money>,
}

#[derive(Deserialize)]
struct Money {
    amount_minor: i64,
    currency: String,
    exponent: u32,
}

impl Money {
    /// `12.34 EUR`, in the currency's own minor units.
    fn text(&self) -> Option<String> {
        let exp = self.exponent.min(6);
        let scale = 10u64.checked_pow(exp)?;
        let sign = if self.amount_minor < 0 { "-" } else { "" };
        let amount = self.amount_minor.unsigned_abs();
        let currency = self.currency.trim();
        Some(if exp == 0 {
            format!("{sign}{amount} {currency}")
        } else {
            format!("{sign}{}.{:0width$} {currency}", amount / scale, amount % scale, width = exp as usize)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plans::model::{DAY, HOUR, MIN};

    /// A saved answer of the usage endpoint (the shape the screenshot's account gets), the times relative to `NOW`.
    fn answer() -> String {
        let at = |ms: u64| crate::plans::tests::iso(ms);
        format!(
            r#"{{"limits":[
              {{"kind":"session","percent":7,"resets_at":"{s}"}},
              {{"kind":"weekly_all","percent":12,"resets_at":"{w}"}},
              {{"kind":"weekly_scoped","percent":5,"resets_at":"{w}","scope":{{"model":{{"display_name":"Fable"}}}}}},
              {{"kind":"weekly_scoped","percent":9,"resets_at":"{w}","scope":{{"model":{{"display_name":" "}}}}}},
              {{"kind":"monthly_credits","percent":1}}],
             "seven_day_breakdown":{{"rows":[{{"display_name":"Claude Code","percent":100}},{{"display_name":"Cowork","percent":0}}]}},
             "spend":{{"enabled":false}}}}"#,
            s = at(crate::plans::tests::NOW + HOUR + 33 * MIN),
            w = at(crate::plans::tests::NOW + 5 * DAY + HOUR),
        )
    }

    #[test]
    fn reads_the_windows_the_surfaces_and_the_extra_usage() {
        let r = parse(&answer(), Account { email: Some("a@b.c".into()), plan: plan(Some("max"), Some("default_claude_max_5x")) }).unwrap();
        assert_eq!(r.account.plan.as_deref(), Some("Max 5x"));
        let kinds: Vec<_> = r.windows.iter().map(|w| (w.kind.id(), w.used)).collect();
        assert_eq!(kinds, [("session", 7.), ("weekly", 12.), ("named", 5.)], "unknown kinds and nameless models are dropped");
        assert_eq!(r.windows[2].kind, Kind::Named("Fable".into()));
        assert_eq!(r.windows[0].length_ms, Some(SESSION));
        assert_eq!(r.windows[2].length_ms, Some(WEEK));
        assert_eq!(r.windows[0].resets_at_ms, Some(crate::plans::tests::NOW + HOUR + 33 * MIN));
        assert_eq!(
            r.sections,
            vec![
                Section::Shares { title: "This week by surface".into(), shares: vec![("Claude Code".into(), 100.)] },
                Section::Facts { title: "Extra usage".into(), facts: vec![("Status".into(), "Off".into())] },
            ]
        );
    }

    #[test]
    fn falls_back_to_the_fixed_windows_of_the_older_shape() {
        let body = r#"{"five_hour":{"utilization":21.5,"resets_at":"2026-10-29T10:00:00Z"},"seven_day":{"utilization":3,"resets_at":null}}"#;
        let r = parse(body, Account::default()).unwrap();
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.windows[0].kind, Kind::Session);
        assert_eq!(r.windows[0].used, 21.5);
        assert_eq!(r.windows[1].resets_at_ms, None);
    }

    #[test]
    fn extra_usage_that_is_on_shows_the_month() {
        let body = r#"{"five_hour":{"utilization":1},"spend":{"enabled":true,"used":{"amount_minor":1234,"currency":"USD","exponent":2},"limit":{"amount_minor":5000,"currency":"USD","exponent":2}}}"#;
        let r = parse(body, Account::default()).unwrap();
        assert_eq!(
            r.sections,
            vec![Section::Facts { title: "Extra usage".into(), facts: vec![("This month".into(), "12.34 USD of 50.00 USD".into())] }]
        );
    }

    #[test]
    fn an_answer_without_windows_or_not_json_is_a_failure_not_zeros() {
        assert_eq!(parse("{}", Account::default()), Err(Fail::NoPlan));
        assert_eq!(parse("<html>", Account::default()), Err(Fail::Parse));
    }

    #[test]
    fn plan_names() {
        assert_eq!(plan(Some("max"), Some("default_claude_max_20x")).as_deref(), Some("Max 20x"));
        assert_eq!(plan(Some("pro"), Some("default")).as_deref(), Some("Pro"));
        assert_eq!(plan(Some("pro"), None).as_deref(), Some("Pro"));
        assert_eq!(plan(None, Some("x_5x")), None);
    }

    #[test]
    fn another_endpoint_or_a_key_helper_means_the_sign_in_is_not_in_use() {
        assert!(anthropic("https://api.anthropic.com/"));
        assert!(anthropic("https://api.anthropic.com/v1"));
        assert!(!anthropic("https://api.anthropic.com.evil.example"));
        assert!(!anthropic("https://proxy.example"));
    }

    #[test]
    fn the_sign_in_is_of_the_program_the_crew_runs_claude_through_and_a_dead_one_is_the_reason() {
        use crate::plans::providers::tests::pi_file;
        use crate::plans::routes::routes_of;
        let tmp = tempfile::tempdir().unwrap();
        let mut src = Sources::for_tests(tmp.path());
        assert!(choose(&src, NOW0).is_none(), "no sign-in anywhere: left out");
        std::fs::create_dir_all(&src.claude_dir).unwrap();
        std::fs::write(
            src.claude_dir.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"cc-token","expiresAt":4102444800000,"subscriptionType":"max","rateLimitTier":"default_claude_max_5x"}}"#,
        )
        .unwrap();
        std::fs::write(src.claude_dir.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"me@x.y"}}"#).unwrap();
        let (cred, account) = choose(&src, NOW0).unwrap().unwrap();
        assert_eq!((account.email.as_deref(), account.plan.as_deref(), cred.from), (Some("me@x.y"), Some("Max 5x"), "claude"));
        assert_eq!(cred.expires_ms, Some(4102444800000));
        // pi has a Claude sign-in too, but the crew does not run Claude through pi: Claude Code's own is first, pi's is only a place to look after it
        pi_file(tmp.path(), &serde_json::json!({"anthropic": {"type": "oauth", "access": "pi-token", "refresh": "r", "expires": 4102444800000u64}}));
        assert_eq!(choose(&src, NOW0).unwrap().unwrap().0.from, "claude");
        // the crew runs it through pi (a role says so): pi's is first, it names no account, and the role is kept
        src.routes = routes_of(&[("worker.agent".to_string(), "pi/anthropic/claude-sonnet".to_string())]);
        let (cred, account) = choose(&src, NOW0).unwrap().unwrap();
        assert_eq!((cred.from, cred.role.as_str(), account), ("pi", "worker", Account::default()));
        // pi's has expired: Claude Code's is the one asked with
        pi_file(tmp.path(), &serde_json::json!({"anthropic": {"type": "oauth", "access": "pi-token", "expires": 5u64}}));
        assert_eq!(choose(&src, NOW0).unwrap().unwrap().0.from, "claude");
        // both have expired: the reason is said
        std::fs::write(src.claude_dir.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"cc-token","expiresAt":5}}"#).unwrap();
        assert_eq!(choose(&src, NOW0).unwrap().err(), Some(Fail::Expired));
        // the environment is the last resort
        let tmp2 = tempfile::tempdir().unwrap();
        let env = Sources::for_tests(tmp2.path()).with_env("CLAUDE_CODE_OAUTH_TOKEN", "env-token");
        assert_eq!(choose(&env, NOW0).unwrap().unwrap().0.from, "env");
    }

    const NOW0: u64 = 1_792_000_000_000;
}
