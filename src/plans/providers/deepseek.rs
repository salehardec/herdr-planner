//! DeepSeek credit balance. The sign-in is an API key: pi's (`deepseek` in `~/.pi/agent/auth.json`), else (`[usage.deepseek] api_key` in the planner's config.toml, or
//! `DEEPSEEK_API_KEY`/`DEEPSEEK_KEY` in the planner's environment) read against the public balance endpoint, else a DeepSeek
//! Platform session token (`platform_token`, `DEEPSEEK_PLATFORM_TOKEN`) read against the dashboard's user summary.
//! Not ported: the Platform's per-key cost history, and importing the token from a browser.

use super::{agent_login, number, parse_json, stamp, Cred};
use crate::plans::http::{self, Fail, Request, Res};
use crate::plans::model::{Account, Balance, Report, Section};
use crate::plans::routes::agents_for;
use crate::plans::Sources;
use serde::Deserialize;

pub const ID: &str = "deepseek";
const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";
const SUMMARY_URL: &str = "https://platform.deepseek.com/api/v0/users/get_user_summary";

pub async fn fetch(src: &Sources, _now: u64) -> Option<Res<Report>> {
    if let Some(cred) = api_key(src) {
        let req = Request::get(ID, BALANCE_URL).bearer(&cred.token).header("Accept", "application/json");
        return Some(stamp(http::body(req).await.and_then(|body| parse_balance(&body)), &cred));
    }
    let (stored, from) = src.secret_from(ID, "platform_token", &["DEEPSEEK_PLATFORM_TOKEN", "DEEPSEEK_USER_TOKEN"])?;
    let cred = Cred::new(stored.clone(), None, from);
    // localStorage keeps the token as `{"value":"…"}`; a bare token is used as is.
    let token = stored
        .json_field("value")
        .unwrap_or(stored);
    let req = Request::get(ID, SUMMARY_URL).bearer(&token).header("Accept", "application/json").header("x-client-platform", "web");
    Some(stamp(http::body(req).await.and_then(|body| parse_summary(&body)), &cred))
}

/// The API key, best first: that of the program the crew runs DeepSeek through (pi's `auth.json`, omp's credential table, OpenCode), then the other
/// places a key can be, then the planner's own config, and last the planner's environment.
pub fn api_key(src: &Sources) -> Option<Cred> {
    for (agent, role) in agents_for(&src.routes, ID) {
        if let Some(mut found) = agent_login(src, agent, ID) {
            found.cred.role = role;
            return Some(found.cred);
        }
    }
    src.secret_from(ID, "api_key", &["DEEPSEEK_API_KEY", "DEEPSEEK_KEY"]).map(|(token, from)| Cred::new(token, None, from))
}

/// The public API's answer: one entry per currency the account holds.
pub fn parse_balance(body: &str) -> Res<Report> {
    let answer: BalanceAnswer = parse_json(body)?;
    let wallets: Vec<Wallet> = answer
        .balance_infos
        .into_iter()
        .map(|i| {
            Ok(Wallet {
                currency: i.currency,
                total: i.total_balance.ok_or(Fail::Parse)?,
                granted: i.granted_balance.ok_or(Fail::Parse)?,
                paid: i.topped_up_balance.ok_or(Fail::Parse)?,
            })
        })
        .collect::<Res<_>>()?;
    Ok(report(choose(wallets), answer.is_available))
}

/// The Platform dashboard's answer: paid (`normal`) and granted (`bonus`) wallets, summed per currency.
pub fn parse_summary(body: &str) -> Res<Report> {
    let answer: SummaryAnswer = parse_json(body)?;
    check_code(answer.code)?;
    let data = answer.data.ok_or(Fail::Parse)?;
    check_code(data.biz_code)?;
    let summary = data.biz_data.ok_or(Fail::Parse)?;
    let mut wallets: Vec<Wallet> = Vec::new();
    let entries = summary
        .normal_wallets
        .into_iter()
        .map(|w| (w, false))
        .chain(summary.bonus_wallets.into_iter().map(|w| (w, true)));
    for (wallet, granted) in entries {
        let amount = wallet.balance.ok_or(Fail::Parse)?;
        let index = match wallets.iter().position(|k| k.currency == wallet.currency) {
            Some(i) => i,
            None => {
                wallets.push(Wallet { currency: wallet.currency, total: 0., granted: 0., paid: 0. });
                wallets.len() - 1
            }
        };
        let entry = &mut wallets[index];
        entry.total += amount;
        if granted {
            entry.granted += amount;
        } else {
            entry.paid += amount;
        }
    }
    wallets.sort_by(|a, b| a.currency.cmp(&b.currency));
    let wallet = choose(wallets);
    let available = wallet.as_ref().is_some_and(|w| w.total > 0.);
    Ok(report(wallet, available))
}

/// DeepSeek reports an expired Platform session as code 40002 or 40003.
fn check_code(code: Option<i64>) -> Res<()> {
    match code {
        None | Some(0) => Ok(()),
        Some(40002 | 40003) => Err(Fail::Rejected),
        Some(_) => Err(Fail::Parse),
    }
}

/// USD when it is funded, else any funded currency, else USD, else the first: an empty USD row must not hide a positive CNY balance.
fn choose(wallets: Vec<Wallet>) -> Option<Wallet> {
    let pick = wallets
        .iter()
        .position(|w| w.currency == "USD" && w.total > 0.)
        .or_else(|| wallets.iter().position(|w| w.total > 0.))
        .or_else(|| wallets.iter().position(|w| w.currency == "USD"))
        .unwrap_or(0);
    wallets.into_iter().nth(pick)
}

fn money(amount: f64, currency: &str) -> String {
    if currency == "USD" { format!("${amount:.2}") } else { format!("{amount:.2} {currency}") }
}

fn report(wallet: Option<Wallet>, available: bool) -> Report {
    let wallet = wallet.unwrap_or(Wallet { currency: "USD".into(), total: 0., granted: 0., paid: 0. });
    let currency = wallet.currency.trim().to_uppercase();
    let mut facts = vec![
        ("Paid".to_string(), money(wallet.paid, &currency)),
        ("Granted".to_string(), money(wallet.granted, &currency)),
    ];
    if wallet.total <= 0. {
        facts.push(("Status".into(), "Add credits at platform.deepseek.com".into()));
    } else if !available {
        facts.push(("Status".into(), "Balance unavailable for API calls".into()));
    }
    let mut report = Report::new(ID, Account::default(), Vec::new());
    report.balances.push(Balance { amount: wallet.total, currency, total: None });
    report.sections.push(Section::Facts { title: "Credits".into(), facts });
    report
}

struct Wallet {
    currency: String,
    total: f64,
    granted: f64,
    paid: f64,
}

#[derive(Deserialize)]
struct BalanceAnswer {
    #[serde(default)]
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Deserialize)]
struct BalanceInfo {
    currency: String,
    #[serde(default, deserialize_with = "number")]
    total_balance: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    granted_balance: Option<f64>,
    #[serde(default, deserialize_with = "number")]
    topped_up_balance: Option<f64>,
}

#[derive(Deserialize)]
struct SummaryAnswer {
    code: Option<i64>,
    data: Option<SummaryData>,
}

#[derive(Deserialize)]
struct SummaryData {
    biz_code: Option<i64>,
    biz_data: Option<Summary>,
}

#[derive(Deserialize)]
struct Summary {
    #[serde(default)]
    normal_wallets: Vec<PlatformWallet>,
    #[serde(default)]
    bonus_wallets: Vec<PlatformWallet>,
}

#[derive(Deserialize)]
struct PlatformWallet {
    #[serde(default, deserialize_with = "number")]
    balance: Option<f64>,
    currency: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_balance_prefers_funded_usd_and_reads_the_amount_as_text() {
        let body = r#"{"is_available":true,"balance_infos":[
          {"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"},
          {"currency":"USD","total_balance":"11.92","granted_balance":"0.00","topped_up_balance":"11.92"}]}"#;
        let r = parse_balance(body).unwrap();
        assert_eq!(r.balances, vec![Balance { amount: 11.92, currency: "USD".into(), total: None }]);
        assert_eq!(
            r.sections,
            vec![Section::Facts { title: "Credits".into(), facts: vec![("Paid".into(), "$11.92".into()), ("Granted".into(), "$0.00".into())] }]
        );
        assert!(r.windows.is_empty(), "a balance has no windows: the bar shows the money");
    }

    #[test]
    fn an_empty_usd_row_does_not_hide_a_funded_cny_balance() {
        let body = r#"{"is_available":true,"balance_infos":[
          {"currency":"USD","total_balance":0,"granted_balance":0,"topped_up_balance":0},
          {"currency":"CNY","total_balance":5,"granted_balance":0,"topped_up_balance":5}]}"#;
        let r = parse_balance(body).unwrap();
        assert_eq!(r.balances[0].currency, "CNY");
        assert_eq!(r.balances[0].amount, 5.);
    }

    #[test]
    fn an_unavailable_or_empty_balance_says_so() {
        let body = r#"{"is_available":false,"balance_infos":[{"currency":"USD","total_balance":"2","granted_balance":"2","topped_up_balance":"0"}]}"#;
        let r = parse_balance(body).unwrap();
        assert!(matches!(&r.sections[0], Section::Facts { facts, .. } if facts.last().unwrap() == &("Status".to_string(), "Balance unavailable for API calls".to_string())));
        let r = parse_balance(r#"{"is_available":false,"balance_infos":[]}"#).unwrap();
        assert_eq!(r.balances[0].amount, 0.);
        assert!(matches!(&r.sections[0], Section::Facts { facts, .. } if facts.last().unwrap().1.contains("Add credits")));
    }

    #[test]
    fn the_platform_summary_adds_paid_and_granted_wallets() {
        let body = r#"{"code":0,"data":{"biz_code":0,"biz_data":{
          "normal_wallets":[{"balance":"3.00","currency":"USD"}],
          "bonus_wallets":[{"balance":"1.25","currency":"USD"}]}}}"#;
        let r = parse_summary(body).unwrap();
        assert_eq!(r.balances[0].amount, 4.25);
        assert!(matches!(&r.sections[0], Section::Facts { facts, .. } if facts[0].1 == "$3.00" && facts[1].1 == "$1.25"));
    }

    #[test]
    fn an_expired_platform_session_is_a_rejection_and_a_wrong_shape_a_parse_failure() {
        assert_eq!(parse_summary(r#"{"code":40003}"#), Err(Fail::Rejected));
        assert_eq!(parse_summary(r#"{"code":0,"data":{"biz_code":7}}"#), Err(Fail::Parse));
        assert_eq!(parse_summary(r#"{"code":0}"#), Err(Fail::Parse));
        assert_eq!(parse_balance(r#"{"balance_infos":[{"currency":"USD"}]}"#), Err(Fail::Parse));
        assert_eq!(parse_balance("x"), Err(Fail::Parse));
    }

    #[test]
    fn the_key_is_of_the_program_the_crew_runs_deepseek_through_then_the_config_then_the_environment() {
        use crate::plans::providers::tests::pi_file;
        use crate::plans::routes::routes_of;
        let tmp = tempfile::tempdir().unwrap();
        let bare = Sources::for_tests(tmp.path());
        assert!(api_key(&bare).is_none(), "no key anywhere: the provider is not connected, and the page says how to connect it");
        let env = Sources::for_tests(tmp.path()).with_env("DEEPSEEK_API_KEY", "from-env");
        assert_eq!(api_key(&env).unwrap().from, "env");
        let cfg = Sources::for_tests(tmp.path()).with_env("DEEPSEEK_API_KEY", "from-env").with_settings("[deepseek]\napi_key = \"from-config\"\n");
        assert_eq!(api_key(&cfg).unwrap().from, "config", "the planner's config comes before the environment");
        pi_file(tmp.path(), &serde_json::json!({"deepseek": {"type": "api_key", "key": "from-pi"}}));
        let c = api_key(&cfg).unwrap();
        assert_eq!((c.from, c.role.as_str()), ("pi", ""), "pi's key comes before the config's and the environment's, and no role names pi yet");
        // omp holds a key too, and the crew runs DeepSeek through omp: omp's is the one, with its role
        let mut both = Sources::for_tests(tmp.path());
        crate::plans::providers::tests::omp_db(&both.omp_db, &[("deepseek", "api_key", r#"{"key":"from-omp","source":"login"}"#, None)]);
        both.routes = routes_of(&[("worker.agent".to_string(), "omp/deepseek/deepseek-v4-pro".to_string())]);
        let c = api_key(&both).unwrap();
        assert_eq!((c.from, c.role.as_str()), ("omp", "worker"));
        // …and through pi: pi's
        both.routes = routes_of(&[("worker.agent".to_string(), "pi/deepseek/deepseek-v4-pro".to_string())]);
        assert_eq!(api_key(&both).unwrap().from, "pi");
    }

    #[tokio::test]
    async fn without_a_key_the_provider_gives_nothing_and_is_listed_as_not_connected() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(fetch(&Sources::for_tests(tmp.path()), 0).await.is_none());
    }
}
