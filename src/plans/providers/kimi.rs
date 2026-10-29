//! Kimi Code (Kimi For Coding) quotas: the 5-hour rate limit, the weekly request quota, and the monthly Total usage pool, in
//! CodexBar's order (as herdr-gpui reads them):
//!
//! 0. pi's Kimi sign-in (`kimi-coding` in `~/.pi/agent/auth.json`), while it has not expired;
//! 1. a Kimi Code API key (`[usage.kimi] api_key`, `KIMI_CODE_API_KEY`), sent to `GET {api}/coding/v1/usages`;
//! 2. the Kimi Code CLI's own sign-in, `$KIMI_CODE_HOME/credentials/kimi-code.json` (default `~/.kimi-code`), while its access
//!    token is fresh (it lives 15 minutes and only the CLI renews it), with the CLI's device headers (China region only, as the
//!    file does not say which host issued it);
//! 3. the web session token, the `kimi-auth` cookie's value (`[usage.kimi] token`, `KIMI_AUTH_TOKEN`), sent to the console's
//!    `GetUsages` call.
//!
//! With a web token, the membership pool (`GetSubscriptionStats`) and the plan title (`GetSubscription`) are added; failures
//! there never fail the report. Not ported: reading the cookie from a browser, and several labelled web accounts.

use super::{number, parse_json, pi_oauth, ts_ms, Timestamp};
use crate::plans::http::{self, Fail, Request, Res, Secret};
use crate::plans::model::{group, Account, Kind, Report, Section, Window, MIN, MONTH, SESSION, WEEK};
use crate::plans::Sources;
use serde::Deserialize;
use serde_json::Value;

pub const ID: &str = "kimi";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
/// The CLI's token is used while it has this much left.
const FRESH_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    China,
    International,
}

impl Region {
    pub fn from(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("international" | "global" | "kimi.ai") => Self::International,
            _ => Self::China,
        }
    }
    fn api(self) -> &'static str {
        match self {
            Self::China => "https://api.kimi.com",
            Self::International => "https://api.kimi.ai",
        }
    }
    fn web(self) -> &'static str {
        match self {
            Self::China => "https://www.kimi.com",
            Self::International => "https://www.kimi.ai",
        }
    }
}

pub async fn fetch(src: &Sources, now: u64) -> Option<Res<Report>> {
    let region = Region::from(src.text(ID, "region").as_deref());
    let web = src.secret(ID, "token", &["KIMI_AUTH_TOKEN"]);
    // pi's Kimi sign-in (`kimi-coding` in its `auth.json`) is asked with first; pi renews it when it runs, the planner never does
    let pi = pi_oauth(src, "kimi-coding");
    let pi_dead = pi.as_ref().is_some_and(|c| c.stale(now));
    if let Some(cred) = pi.filter(|c| !c.stale(now)) {
        let req = Request::get(ID, usages_url(region.api())).bearer(&cred.token);
        return Some(code(req, web.as_ref(), region).await);
    }
    if let Some(key) = src.secret(ID, "api_key", &["KIMI_CODE_API_KEY"]) {
        let req = Request::get(ID, usages_url(region.api())).bearer(&key);
        return Some(code(req, web.as_ref(), region).await);
    }
    if region == Region::China {
        if let Some(file) = http::read_json(&src.kimi_home.join("credentials/kimi-code.json")) {
            let fresh = file
                .get("expires_at")
                .and_then(Value::as_f64)
                .filter(|e| *e > 0.)
                .is_some_and(|e| (e * 1000.) as u64 > now + FRESH_MS);
            match http::secret_at(&file, &["access_token"]).filter(|_| fresh) {
                Some(token) => {
                    let device = std::fs::read_to_string(src.kimi_home.join("device_id"))
                        .map(|id| ascii(id.trim()))
                        .ok()
                        .filter(|id| !id.is_empty());
                    let mut req = Request::get(ID, usages_url(region.api()))
                        .bearer(&token)
                        .header("User-Agent", concat!("herdr-planner/", env!("CARGO_PKG_VERSION")))
                        .header("X-Msh-Platform", "kimi_code_cli")
                        .header("X-Msh-Version", env!("CARGO_PKG_VERSION"))
                        .header("X-Msh-Device-Model", "Linux");
                    if let Some(device) = device {
                        req = req.header("X-Msh-Device-Id", &device);
                    }
                    return Some(code(req, web.as_ref(), region).await);
                }
                // The CLI renews its token when it runs; the planner never rewrites its files.
                None if web.is_none() => return Some(Err(Fail::Expired)),
                None => {}
            }
        }
    }
    let Some(token) = web else { return pi_dead.then_some(Err(Fail::Expired)) };
    Some(web_usage(&token, region).await)
}

fn ascii(value: &str) -> String {
    value.chars().filter(|c| (' '..='~').contains(c)).collect()
}

/// `…/coding/v1/usages`, whether the base ends at the host, `coding`, or `coding/v1`.
fn usages_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/coding/v1") {
        format!("{base}/usages")
    } else if base.ends_with("/coding") {
        format!("{base}/v1/usages")
    } else {
        format!("{base}/coding/v1/usages")
    }
}

async fn code(req: Request, web: Option<&Secret>, region: Region) -> Res<Report> {
    let body = http::body(req.header("Accept", "application/json")).await?;
    let mut usage = parse_code(&body)?;
    if let Some(token) = web {
        enrich(token, region, &mut usage).await;
    }
    Ok(usage.report())
}

async fn web_usage(token: &Secret, region: Region) -> Res<Report> {
    let body = http::body(
        web_request(region, "kimi.gateway.billing.v1.BillingService/GetUsages", token).json(r#"{"scope":["FEATURE_CODING"]}"#),
    )
    .await?;
    let mut usage = parse_web(&body)?;
    enrich(token, region, &mut usage).await;
    Ok(usage.report())
}

/// The membership pool and plan title, best effort: the quota stands without them.
async fn enrich(token: &Secret, region: Region, usage: &mut Usage) {
    let call = |method: &'static str| async move {
        http::body(web_request(region, method, token).json("{}")).await.ok()
    };
    if let Some(stats) = call("kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats").await.and_then(|b| parse_stats(&b)) {
        usage.monthly = usage.monthly.take().or(stats.monthly);
        usage.code_weekly = stats.code_weekly;
    }
    if usage.plan.is_none() {
        usage.plan = call("kimi.gateway.membership.v2.MembershipService/GetSubscription").await.and_then(|b| parse_plan(&b));
    }
}

fn web_request(region: Region, method: &str, token: &Secret) -> Request {
    let origin = region.web();
    Request::post(ID, format!("{origin}/apiv2/{method}"))
        .bearer(token)
        .secret_header("Cookie", "kimi-auth=", token)
        .header("Origin", origin)
        .header("Referer", &format!("{origin}/code/console"))
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("User-Agent", USER_AGENT)
        .header("connect-protocol-version", "1")
        .header("x-language", "en-US")
        .header("x-msh-platform", "web")
}

/// What the Code API or the console reports, before it becomes a report.
#[derive(Debug, Default)]
pub struct Usage {
    weekly: Option<Window>,
    session: Option<Window>,
    monthly: Option<Window>,
    code_weekly: Option<Window>,
    weekly_requests: Option<(f64, f64)>,
    session_requests: Option<(f64, f64)>,
    plan: Option<String>,
}

fn total_usage() -> Kind {
    Kind::Named("Total usage".into())
}

impl Usage {
    pub fn report(self) -> Report {
        let distinct = self.code_weekly.filter(|code| {
            !self.weekly.as_ref().is_some_and(|weekly| {
                weekly.length_ms.is_some()
                    && (weekly.used - code.used).abs() <= 1.
                    && weekly.resets_at_ms.zip(code.resets_at_ms).is_some_and(|(a, b)| a.abs_diff(b) <= 5 * MIN)
            })
        });
        let requests = |(used, limit): (f64, f64)| format!("{} of {}", group(used.round() as i64), group(limit.round() as i64));
        let facts: Vec<(String, String)> = [
            ("Weekly requests", self.weekly_requests.map(requests)),
            ("Rate limit requests", self.session_requests.map(requests)),
        ]
        .into_iter()
        .filter_map(|(label, value)| Some((label.to_owned(), value?)))
        .collect();
        let windows = [self.session, self.weekly, self.monthly].into_iter().flatten().collect();
        let mut report = Report::new(ID, Account { email: None, plan: self.plan }, windows);
        report.sections.extend(distinct.map(Section::Limit));
        if !facts.is_empty() {
            report.sections.push(Section::Facts { title: "Requests".into(), facts });
        }
        report
    }
}

/// `GET /coding/v1/usages`: ratio pools take precedence over the older request counts for the windows they report.
pub fn parse_code(body: &str) -> Res<Usage> {
    let response: CodeResponse = parse_json(body)?;
    let pools = response.usages.unwrap_or_default();
    let limit = response.limits.as_ref().and_then(|l| l.first());
    let session_minutes = limit.map_or(Some(300), |l| l.window.as_ref().and_then(RateWindow::minutes));
    let has_monthly = pools.limit_month_total.is_some();
    let weekly_counts = response.usage.as_ref().and_then(Detail::counts);

    let weekly = resolve(pools.limit_7d.as_ref(), response.usage.as_ref(), WEEK, Some(7 * 24 * 60), has_monthly, weekly_counts)
        .or_else(|| response.usage.as_ref().and_then(|d| d.window(Kind::Weekly, Some(WEEK))));
    let session = resolve(pools.limit_5h.as_ref(), limit.map(|l| &l.detail), SESSION, session_minutes, has_monthly, weekly_counts)
        .or_else(|| {
            let length = session_minutes.map(|m| u64::from(m) * MIN);
            limit.and_then(|l| l.detail.window(Kind::Session, length))
        });
    let monthly = pools.limit_month_total.as_ref().and_then(|p| p.window(total_usage(), MONTH));
    if weekly.is_none() && session.is_none() && monthly.is_none() {
        return Err(Fail::Parse);
    }
    Ok(Usage {
        weekly,
        session,
        monthly,
        code_weekly: None,
        weekly_requests: weekly_counts.filter(|c| c.2).map(|(u, l, _)| (u, l)),
        session_requests: limit.and_then(|l| l.detail.counts()).filter(|c| c.2).map(|(u, l, _)| (u, l)),
        plan: plan_name(response.user.as_ref(), response.version.as_ref()),
    })
}

/// The console's `GetUsages` answer.
pub fn parse_web(body: &str) -> Res<Usage> {
    let response: WebResponse = parse_json(body)?;
    let coding = response.usages.into_iter().find(|u| u.scope == "FEATURE_CODING").ok_or(Fail::Parse)?;
    let limit = coding.limits.as_ref().and_then(|l| l.first());
    let length = limit.map_or(Some(SESSION), |l| l.window.as_ref().and_then(RateWindow::minutes).map(|m| u64::from(m) * MIN));
    let counts = |d: &Detail| d.counts().filter(|c| c.2).map(|(u, l, _)| (u, l));
    Ok(Usage {
        weekly: coding.detail.window(Kind::Weekly, Some(WEEK)),
        session: limit.and_then(|l| l.detail.window(Kind::Session, length)),
        monthly: None,
        code_weekly: None,
        weekly_requests: counts(&coding.detail),
        session_requests: limit.and_then(|l| counts(&l.detail)),
        plan: None,
    })
}

pub struct Stats {
    monthly: Option<Window>,
    code_weekly: Option<Window>,
}

pub fn parse_stats(body: &str) -> Option<Stats> {
    let stats: StatsResponse = serde_json::from_str(body).ok()?;
    let monthly = stats.subscription_balance.and_then(|b| {
        let feature_ok = b.feature.as_deref().is_none_or(|f| f == "FEATURE_OMNI");
        let type_ok = b.kind.as_deref().is_none_or(|k| k == "SUBSCRIPTION");
        let ratio = b.amount_used_ratio.filter(|r| r.is_finite())?;
        (feature_ok && type_ok).then(|| Window::new(total_usage(), ratio * 100., ts_ms(&b.expire_time), Some(MONTH)))
    });
    let code_weekly = stats.ratelimit_code_7d.and_then(|l| {
        if l.enabled == Some(false) {
            return None;
        }
        let ratio = l.ratio.filter(|r| r.is_finite())?;
        Some(Window::new(Kind::Named("Code 7-day".into()), ratio * 100., ts_ms(&l.reset_time), Some(WEEK)))
    });
    Some(Stats { monthly, code_weekly })
}

pub fn parse_plan(body: &str) -> Option<String> {
    let response: Value = serde_json::from_str(body).ok()?;
    let subscription = response.get("subscription")?;
    let active = subscription.get("active").and_then(Value::as_bool) == Some(true)
        && subscription.get("status").and_then(Value::as_str) == Some("SUBSCRIPTION_STATUS_ACTIVE");
    let title = subscription.get("goods")?.get("title")?.as_str()?.trim();
    (active && !title.is_empty()).then(|| title.to_owned())
}

/// A zero ratio pool is a placeholder when the counts of the same window say it has been used (the console lags behind them).
fn resolve(
    pool: Option<&Pool>,
    detail: Option<&Detail>,
    length: u64,
    count_minutes: Option<u32>,
    has_monthly: bool,
    weekly_counts: Option<(f64, f64, bool)>,
) -> Option<Window> {
    let kind = if length == WEEK { Kind::Weekly } else { Kind::Session };
    let window = pool?.window(kind, length)?;
    let placeholder = window.used == 0.
        && !has_monthly
        && weekly_counts.is_some_and(|c| c.2)
        && count_minutes.map(u64::from) == Some(length / MIN)
        && detail.is_some_and(|d| {
            let counts = d.counts().filter(|(used, _, reliable)| *reliable && *used > 0.);
            let reset = ts_ms(&d.reset);
            counts.is_some() && reset.zip(window.resets_at_ms).is_some_and(|(a, b)| a.abs_diff(b) <= 2_000)
        });
    (!placeholder).then_some(window)
}

fn plan_name(user: Option<&Value>, version: Option<&Value>) -> Option<String> {
    let level = user?.get("membership")?.get("level")?.as_str()?.trim();
    if level.is_empty() || level == "LEVEL_UNSPECIFIED" {
        return None;
    }
    let v1 = match version {
        None | Some(Value::Null) => true,
        Some(Value::String(v)) => v == "GOODS_VERSION_V1",
        Some(_) => false,
    };
    let name = match level {
        _ if !v1 => level,
        "LEVEL_FREE" => "Adagio",
        "LEVEL_TRIAL" => "Andante",
        "LEVEL_BASIC" => "Moderato",
        "LEVEL_INTERMEDIATE" => "Allegretto",
        "LEVEL_ADVANCED" => "Allegro",
        other => other,
    };
    Some(name.to_owned())
}

#[derive(Deserialize)]
struct CodeResponse {
    usage: Option<Detail>,
    usages: Option<Pools>,
    limits: Option<Vec<RateLimit>>,
    user: Option<Value>,
    version: Option<Value>,
}

#[derive(Default, Deserialize)]
struct Pools {
    limit_5h: Option<Pool>,
    limit_7d: Option<Pool>,
    limit_month_total: Option<Pool>,
}

#[derive(Deserialize)]
struct Pool {
    #[serde(default, deserialize_with = "number")]
    used_ratio: Option<f64>,
    reset_time: Option<Timestamp>,
}

impl Pool {
    fn window(&self, kind: Kind, length: u64) -> Option<Window> {
        let ratio = self.used_ratio.filter(|r| r.is_finite() && *r >= 0.)?;
        Some(Window::new(kind, ratio.min(1.) * 100., ts_ms(&self.reset_time), Some(length)))
    }
}

#[derive(Deserialize)]
struct Detail {
    #[serde(default, deserialize_with = "number")]
    limit: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    used: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    remaining: Option<f64>,
    #[serde(rename = "resetTime", alias = "resetAt", alias = "reset_time", alias = "reset_at")]
    reset: Option<Timestamp>,
}

impl Detail {
    /// Used, limit, and whether the used count is a reading and not a guess.
    fn counts(&self) -> Option<(f64, f64, bool)> {
        let limit = self.limit.filter(|l| l.is_finite() && *l > 0.)?;
        if let Some(used) = self.used.filter(|u| u.is_finite() && *u >= 0.) {
            return Some((used, limit, true));
        }
        if let Some(remaining) = self.remaining.filter(|r| (0. ..=limit).contains(r)) {
            return Some((limit - remaining, limit, true));
        }
        Some((0., limit, false))
    }

    fn window(&self, kind: Kind, length: Option<u64>) -> Option<Window> {
        let (used, limit, reliable) = self.counts()?;
        Some(Window::new(kind, used / limit * 100., ts_ms(&self.reset), length.filter(|_| reliable)))
    }
}

#[derive(Deserialize)]
struct RateLimit {
    window: Option<RateWindow>,
    detail: Detail,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateWindow {
    duration: i64,
    time_unit: String,
}

impl RateWindow {
    fn minutes(&self) -> Option<u32> {
        let unit = match self.time_unit.as_str() {
            "TIME_UNIT_MINUTE" => 1,
            "TIME_UNIT_HOUR" => 60,
            "TIME_UNIT_DAY" => 24 * 60,
            _ => return None,
        };
        u32::try_from(self.duration).ok().filter(|d| *d > 0)?.checked_mul(unit)
    }
}

#[derive(Deserialize)]
struct WebResponse {
    usages: Vec<WebUsage>,
}

#[derive(Deserialize)]
struct WebUsage {
    scope: String,
    detail: Detail,
    limits: Option<Vec<RateLimit>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatsResponse {
    subscription_balance: Option<SubscriptionBalance>,
    #[serde(rename = "ratelimitCode7d")]
    ratelimit_code_7d: Option<CodeLimit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscriptionBalance {
    feature: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "number")]
    amount_used_ratio: Option<f64>,
    expire_time: Option<Timestamp>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeLimit {
    #[serde(default, deserialize_with = "number")]
    ratio: Option<f64>,
    enabled: Option<bool>,
    reset_time: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> u64 {
        Timestamp::Text(text.into()).ms().unwrap()
    }

    const COUNTS: &str = r#"{
      "usage":{"limit":"2048","used":"214","remaining":"1834","resetTime":"2026-01-09T15:23:13.716839300Z"},
      "limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
        "detail":{"limit":"200","used":"139","remaining":"61","resetTime":"2026-01-06T13:33:02.717479433Z"}}],
      "user":{"membership":{"level":"LEVEL_BASIC"}}
    }"#;

    #[test]
    fn reads_request_counts() {
        let report = parse_code(COUNTS).unwrap().report();
        assert_eq!(report.account.plan.as_deref(), Some("Moderato"));
        let session = &report.windows[0];
        assert_eq!(session.kind, Kind::Session);
        assert!((session.used - 69.5).abs() < 1e-4);
        assert_eq!(session.length_ms, Some(SESSION));
        let weekly = &report.windows[1];
        assert_eq!(weekly.kind, Kind::Weekly);
        assert!((weekly.used - 10.449_219).abs() < 1e-4);
        assert!(weekly.resets_at_ms.is_some());
        assert_eq!(
            report.sections,
            vec![Section::Facts {
                title: "Requests".into(),
                facts: vec![
                    ("Weekly requests".into(), "214 of 2,048".into()),
                    ("Rate limit requests".into(), "139 of 200".into()),
                ],
            }]
        );
    }

    /// The shape the screenshot's account gets: the ratio pools, the session and the monthly total.
    #[test]
    fn ratio_pools_take_precedence_and_the_monthly_total_is_named() {
        let body = r#"{
          "limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
            "detail":{"limit":"100","used":"25","remaining":"75"}}],
          "usages":{
            "limit_5h":{"used_ratio":0.35,"reset_time":"2026-09-16T20:15:44Z"},
            "limit_month_total":{"used_ratio":0.21,"reset_time":"2026-10-17T00:00:00Z"},
            "limit_month_code":{"used_ratio":0,"reset_time":"2026-10-17T00:00:00Z"}
          }
        }"#;
        let report = parse_code(body).unwrap().report();
        assert_eq!(report.account.plan, None);
        assert_eq!(report.windows.len(), 2);
        let session = &report.windows[0];
        assert_eq!(session.kind, Kind::Session);
        assert!((session.used - 35.).abs() < 1e-9);
        assert_eq!(session.resets_at_ms, Some(at("2026-09-16T20:15:44Z")));
        let monthly = &report.windows[1];
        assert_eq!(monthly.kind, Kind::Named("Total usage".into()));
        assert!((monthly.used - 21.).abs() < 1e-9);
        assert_eq!(monthly.length_ms, Some(MONTH));
    }

    #[test]
    fn a_zero_ratio_placeholder_yields_to_matching_counts() {
        let body = r#"{
          "usage":{"limit":"100","used":"10","resetTime":"2026-01-09T00:00:00Z"},
          "limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
            "detail":{"limit":"200","used":"50","resetTime":"2026-01-06T13:33:02Z"}}],
          "usages":{"limit_5h":{"used_ratio":0,"reset_time":"2026-01-06T13:33:02Z"},
                    "limit_7d":{"used_ratio":0,"reset_time":"2026-01-09T00:00:00Z"}}
        }"#;
        let report = parse_code(body).unwrap().report();
        assert!((report.windows[0].used - 25.).abs() < 1e-9, "the counts, not the zero placeholder: {:?}", report.windows);
        assert!((report.windows[1].used - 10.).abs() < 1e-9);
    }

    #[test]
    fn the_console_answer_and_the_membership_extras() {
        let web = r#"{"usages":[{"scope":"FEATURE_CODING","detail":{"limit":"100","used":"8","resetTime":"2026-01-09T00:00:00Z"},
          "limits":[{"window":{"duration":5,"timeUnit":"TIME_UNIT_HOUR"},"detail":{"limit":"50","used":"5","resetTime":"2026-01-06T13:00:00Z"}}]}]}"#;
        let u = parse_web(web).unwrap();
        let r = u.report();
        assert_eq!(r.windows.len(), 2);
        assert_eq!(r.windows[0].kind, Kind::Session);
        assert_eq!(r.windows[0].length_ms, Some(SESSION));
        assert!((r.windows[0].used - 10.).abs() < 1e-9);
        assert!((r.windows[1].used - 8.).abs() < 1e-9);

        let stats = parse_stats(
            r#"{"subscriptionBalance":{"feature":"FEATURE_OMNI","type":"SUBSCRIPTION","amountUsedRatio":0.2404,"expireTime":"2026-11-28T00:00:00Z"},
                "ratelimitCode7d":{"ratio":0.5,"enabled":true,"resetTime":"2026-01-12T00:00:00Z"}}"#,
        )
        .unwrap();
        assert!((stats.monthly.unwrap().used - 24.04).abs() < 1e-9);
        assert_eq!(stats.code_weekly.unwrap().kind, Kind::Named("Code 7-day".into()));
        assert_eq!(parse_stats(r#"{"ratelimitCode7d":{"ratio":0.5,"enabled":false}}"#).unwrap().code_weekly.map(|w| w.used), None);
        assert_eq!(
            parse_plan(r#"{"subscription":{"active":true,"status":"SUBSCRIPTION_STATUS_ACTIVE","goods":{"title":"Kimi Plus"}}}"#).as_deref(),
            Some("Kimi Plus")
        );
        assert_eq!(parse_plan(r#"{"subscription":{"active":false,"goods":{"title":"Kimi Plus"}}}"#), None);
    }

    #[test]
    fn a_distinct_code_weekly_window_is_kept_and_a_twin_of_the_weekly_one_dropped() {
        let mk = |code_used: f64, code_reset: &str| {
            let mut u = parse_web(
                r#"{"usages":[{"scope":"FEATURE_CODING","detail":{"limit":"100","used":"50","resetTime":"2026-01-09T00:00:00Z"}}]}"#,
            )
            .unwrap();
            u.code_weekly = Some(Window::new(Kind::Named("Code 7-day".into()), code_used, Some(at(code_reset)), Some(WEEK)));
            u.report().sections.iter().filter(|s| matches!(s, Section::Limit(_))).count()
        };
        assert_eq!(mk(50.5, "2026-01-09T00:02:00Z"), 0, "the same window twice");
        assert_eq!(mk(70., "2026-01-09T00:00:00Z"), 1);
        assert_eq!(mk(50., "2026-01-10T00:00:00Z"), 1);
    }

    #[test]
    fn nothing_usable_and_odd_regions_and_urls() {
        assert!(matches!(parse_code("{}"), Err(Fail::Parse)));
        assert!(matches!(parse_code("[]"), Err(Fail::Parse)));
        assert!(matches!(parse_web(r#"{"usages":[{"scope":"OTHER","detail":{}}]}"#), Err(Fail::Parse)));
        assert_eq!(Region::from(Some(" International ")), Region::International);
        assert_eq!(Region::from(None), Region::China);
        assert_eq!(usages_url("https://api.kimi.com/"), "https://api.kimi.com/coding/v1/usages");
        assert_eq!(usages_url("https://x.example/coding"), "https://x.example/coding/v1/usages");
        assert_eq!(usages_url("https://x.example/coding/v1"), "https://x.example/coding/v1/usages");
        assert_eq!(ascii("ab\u{e9}-1\n"), "ab-1");
    }
}
