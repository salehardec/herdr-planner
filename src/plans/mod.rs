//! Subscription limits, read by the planner itself (PLN-84): the same sources and the same arithmetic as herdr-gpui's
//! `herdr-usage` crate, written here so that the planner depends on neither its crate, its program nor its files.
//!
//! A provider is signed in when its own agent's sign-in file is on this machine (Claude Code, the Codex CLI, the Kimi Code CLI)
//! or a key is in the planner's config or environment (DeepSeek). The planner asks the provider's service with it, through
//! `curl` with the secret on its standard input, and keeps only the numbers: the sign-in never goes into the snapshot, the
//! log, the page or the gate. Readings are cached: every ten minutes, half as often after a rejection, an hour later after a
//! rate limit, and by hand no more than once in ten seconds.
//!
//! `usage.rs` merges these readings over Omarchy's records, which stay the fallback (and the only source for the programs
//! this module does not read).

pub mod http;
pub mod model;
pub mod providers;
pub mod routes;

use self::http::{Fail, Res, Secret};
use self::model::Report;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokio::sync::Notify;

/// The services rate limit these endpoints, and the numbers move slowly.
pub const REFRESH_MS: u64 = 10 * 60_000;
pub const RATE_LIMITED_MS: u64 = 30 * 60_000;
/// A service that could not be reached is asked again sooner than a full refresh.
pub const ERROR_BACKOFF_MS: u64 = 5 * 60_000;
/// A sign-in the service rejected is asked again half as often after each rejection, up to this: a dead token left behind
/// would otherwise be sent every refresh for as long as the planner runs, which services can take for abuse.
pub const REJECTED_LIMIT_MS: u64 = 6 * 60 * 60_000;
/// A click cannot queue reads back to back.
pub const MANUAL_SPACING_MS: u64 = 10_000;
/// A provider with no sign-in is looked for again this often (a file read, no network), and an expired sign-in too: the
/// agent renews it when it runs, and the next look finds it.
pub const LOOK_MS: u64 = 30_000;

/// The providers, in the order their tabs are listed (the registry's order in herdr-gpui).
pub const IDS: [&str; 4] = ["codex", "claude", "kimi", "deepseek"];

pub fn name_of(id: &str) -> &'static str {
    match id {
        "codex" => "Codex",
        "claude" => "Claude",
        "kimi" => "Kimi Code",
        "deepseek" => "DeepSeek",
        _ => "",
    }
}

/// The account's own usage page and the service's status page, as the card's two links.
pub fn links_of(id: &str) -> (&'static str, &'static str) {
    match id {
        "codex" => ("https://chatgpt.com/codex/settings/usage", "https://status.openai.com"),
        "claude" => ("https://claude.ai/settings/usage", "https://status.anthropic.com"),
        "kimi" => ("https://www.kimi.com/code/console", ""),
        "deepseek" => ("https://platform.deepseek.com/usage", "https://status.deepseek.com"),
        _ => ("", ""),
    }
}

/// Where the sign-ins are, and what the planner's own config says about them.
pub struct Sources {
    pub home: PathBuf,
    pub claude_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub opencode_auth: PathBuf,
    pub kimi_home: PathBuf,
    /// pi's `auth.json` (`$PI_CODING_AGENT_DIR`, else `~/.pi/agent`): the first place the keys and tokens are looked for. Only ever read.
    pub pi_auth: PathBuf,
    /// omp's credential table (`$OMP_CODING_AGENT_DIR`, else `~/.omp/agent`, `agent.db`): opened read-only.
    pub omp_db: PathBuf,
    /// Which agents the crew runs each provider through (read from the board's settings, see `routes`).
    pub routes: Vec<routes::Route>,
    pub hidden: Vec<String>,
    settings: toml::Table,
    env: BTreeMap<String, String>,
}

impl Sources {
    pub fn from_env() -> Self {
        let home = crate::config::home();
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let env = std::env::vars()
            .filter(|(k, v)| !v.trim().is_empty() && ["ANTHROPIC_", "CLAUDE_CODE_", "DEEPSEEK_", "KIMI_"].iter().any(|p| k.starts_with(p)))
            .collect();
        let mut s = Self {
            claude_dir: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            codex_dir: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            opencode_auth: var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")).join("opencode/auth.json"),
            kimi_home: var("KIMI_CODE_HOME").unwrap_or_else(|| home.join(".kimi-code")),
            pi_auth: var("PI_CODING_AGENT_DIR").unwrap_or_else(|| home.join(".pi/agent")).join("auth.json"),
            omp_db: var("OMP_CODING_AGENT_DIR").unwrap_or_else(|| home.join(".omp/agent")).join("agent.db"),
            routes: Vec::new(),
            home,
            hidden: Vec::new(),
            settings: toml::Table::new(),
            env,
        };
        s.read_settings(&crate::config::config_home().join("herdr-planner/config.toml"));
        s
    }

    /// `[usage]` of the planner's config.toml: `hide = ["kimi"]` and one table per provider with its keys.
    fn read_settings(&mut self, path: &Path) {
        let Some(table) = std::fs::read_to_string(path).ok().and_then(|t| t.parse::<toml::Table>().ok()) else { return };
        let Some(usage) = table.get("usage").and_then(|v| v.as_table()) else { return };
        self.hidden = usage
            .get("hide")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        self.settings = usage.clone();
    }

    pub fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    }

    pub fn text(&self, provider: &str, name: &str) -> Option<String> {
        self.settings.get(provider)?.as_table()?.get(name)?.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
    }

    /// A key from the planner's config, else from the environment, and which of the two it was.
    pub fn secret_from(&self, provider: &str, name: &str, env: &[&str]) -> Option<(Secret, &'static str)> {
        if let Some(s) = self.text(provider, name).and_then(Secret::new) {
            return Some((s, "config"));
        }
        env.iter().find_map(|e| self.env(e)).and_then(Secret::new).map(|s| (s, "env"))
    }

    /// A key from the planner's config, else from the environment.
    #[cfg(test)]
    pub fn secret(&self, provider: &str, name: &str, env: &[&str]) -> Option<Secret> {
        self.text(provider, name).or_else(|| env.iter().find_map(|e| self.env(e))).and_then(Secret::new)
    }

    #[cfg(test)]
    pub fn for_tests(root: &Path) -> Self {
        Self {
            home: root.to_path_buf(),
            claude_dir: root.join(".claude"),
            codex_dir: root.join(".codex"),
            opencode_auth: root.join(".local/share/opencode/auth.json"),
            kimi_home: root.join(".kimi-code"),
            pi_auth: root.join(".pi/agent/auth.json"),
            omp_db: root.join(".omp/agent/agent.db"),
            routes: Vec::new(),
            hidden: Vec::new(),
            settings: toml::Table::new(),
            env: BTreeMap::new(),
        }
    }
    #[cfg(test)]
    pub fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.into(), value.into());
        self
    }
    #[cfg(test)]
    pub fn with_settings(mut self, toml: &str) -> Self {
        self.settings = toml.parse().unwrap();
        self
    }
}

/// Reads one provider: `None` when there is no sign-in for it (it is left out), else the report or the reason there is none.
pub async fn fetch(id: &str, src: &Sources, now: u64) -> Option<Res<Report>> {
    if src.hidden.iter().any(|h| h == id) {
        return None;
    }
    let mut got = match id {
        "claude" => providers::claude::fetch(src, now).await,
        "codex" => providers::codex::fetch(src, now).await,
        "kimi" => providers::kimi::fetch(src, now).await,
        "deepseek" => providers::deepseek::fetch(src, now).await,
        _ => None,
    };
    if let Some(Ok(report)) = &mut got {
        // the other programs the crew runs this provider through: the card says the numbers are of the one that was asked, and names them
        report.also = routes::agents_for(&src.routes, id)
            .into_iter()
            .filter(|(agent, role)| !role.is_empty() && *agent != report.from)
            .map(|(agent, _)| agent.to_string())
            .collect();
    }
    got
}

/* ───────── the cache and its clock ───────── */

/// One provider's last answer: the last report that came (kept, marked stale, when later reads fail), and why the latest read failed.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub id: &'static str,
    pub report: Option<Report>,
    /// When `report` was read.
    pub fetched_at_ms: u64,
    pub error: Option<Fail>,
}

#[derive(Debug, Default)]
struct Entry {
    reading: Option<Reading>,
    due_ms: u64,
    /// Rejections in a row.
    strikes: u32,
}

#[derive(Debug, Default)]
pub struct Store {
    entries: BTreeMap<&'static str, Entry>,
    manual_ms: u64,
    routes: Vec<routes::Route>,
    routes_at_ms: u64,
}

impl Store {
    /// The providers to read now: never read, or due.
    pub fn due(&self, now: u64) -> Vec<&'static str> {
        IDS.iter().copied().filter(|id| self.entries.get(id).is_none_or(|e| e.due_ms <= now)).collect()
    }

    /// When the soonest one is due (for the worker's sleep).
    pub fn next_due(&self) -> u64 {
        IDS.iter().map(|id| self.entries.get(id).map_or(0, |e| e.due_ms)).min().unwrap_or(0)
    }

    pub fn readings(&self) -> Vec<Reading> {
        IDS.iter().filter_map(|id| self.entries.get(id)?.reading.clone()).collect()
    }

    /// What one read gave. Returns whether what the page shows changed.
    pub fn apply(&mut self, id: &'static str, now: u64, got: Option<Res<Report>>) -> bool {
        let entry = self.entries.entry(id).or_default();
        let before = entry.reading.clone();
        match got {
            None => {
                entry.reading = None;
                entry.strikes = 0;
                entry.due_ms = now + LOOK_MS;
            }
            Some(Ok(report)) => {
                entry.reading = Some(Reading { id, report: Some(report), fetched_at_ms: now, error: None });
                entry.strikes = 0;
                entry.due_ms = now + REFRESH_MS;
            }
            Some(Err(fail)) => {
                let kept = entry.reading.as_ref().and_then(|r| r.report.clone().map(|rep| (rep, r.fetched_at_ms)));
                entry.due_ms = now
                    + match fail {
                        Fail::Expired | Fail::NoLogin => LOOK_MS,
                        Fail::RateLimited => RATE_LIMITED_MS,
                        // Only an answer proves the sign-in works: an outage between rejections keeps the wait growing.
                        Fail::Rejected => REFRESH_MS.saturating_mul(1 << entry.strikes.min(6)).min(REJECTED_LIMIT_MS),
                        _ => ERROR_BACKOFF_MS,
                    };
                if fail == Fail::Rejected {
                    entry.strikes = entry.strikes.saturating_add(1);
                }
                let (report, fetched_at_ms) = kept.map_or((None, 0), |(r, at)| (Some(r), at));
                entry.reading = Some(Reading { id, report, fetched_at_ms, error: Some(fail) });
            }
        }
        entry.reading != before
    }

    /// The Refresh button: everything is due now and rejections are forgotten, but not twice in ten seconds.
    pub fn refresh_now(&mut self, now: u64) -> bool {
        if self.manual_ms != 0 && now.saturating_sub(self.manual_ms) < MANUAL_SPACING_MS {
            return false;
        }
        self.manual_ms = now;
        self.routes_at_ms = 0;
        for e in self.entries.values_mut() {
            e.due_ms = now;
            e.strikes = 0;
        }
        for id in IDS {
            self.entries.entry(id).or_default().due_ms = now;
        }
        true
    }
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

fn wake() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}

/// How long the crew's settings stay as read: the planner asks the board for them no more often than this.
const ROUTES_EVERY_MS: u64 = 5 * 60_000;

/// The crew's settings, read again when they are old: which agent each provider runs through. A board that does not answer leaves what was known.
async fn refresh_routes(app: &crate::app::Shared, now: u64) -> Vec<routes::Route> {
    let (old, at) = store().lock().map(|s| (s.routes.clone(), s.routes_at_ms)).unwrap_or_default();
    if at != 0 && now.saturating_sub(at) < ROUTES_EVERY_MS {
        return old;
    }
    let read = crate::station::read_settings(&app.cfg, "").await;
    let routes = match read {
        Ok(settings) => routes::routes_of(&settings.into_iter().map(|s| (s.name, s.value)).collect::<Vec<_>>()),
        Err(_) => old,
    };
    if let Ok(mut s) = store().lock() {
        s.routes = routes.clone();
        s.routes_at_ms = now;
    }
    routes
}

/// What the readings are now, for the snapshot.
pub fn readings() -> Vec<Reading> {
    store().lock().map(|s| s.readings()).unwrap_or_default()
}

/// The providers with no sign-in here (and not hidden by the config): the page names them and what to do. Before the first look
/// at them (the first seconds) the list is empty rather than a guess.
pub fn unconnected(readings: &[Reading]) -> Vec<String> {
    let looked = store().lock().map(|s| IDS.iter().all(|id| s.entries.contains_key(id))).unwrap_or(false);
    if !looked {
        return Vec::new();
    }
    let hidden = Sources::from_env().hidden;
    IDS.iter().filter(|id| !readings.iter().any(|r| r.id == **id) && !hidden.iter().any(|h| h == *id)).map(|id| id.to_string()).collect()
}

/// The Refresh button. Returns whether a read was asked for (not when the last ask was under ten seconds ago).
pub fn refresh_now() -> bool {
    let asked = store().lock().map(|mut s| s.refresh_now(crate::config::now_ms())).unwrap_or(false);
    if asked {
        wake().notify_one();
    }
    asked
}

/// The background reader: looks at what is due, reads it side by side, tells the app when the numbers changed, sleeps until
/// the next is due or the button is pressed.
pub async fn run(app: crate::app::Shared) {
    loop {
        let now = crate::config::now_ms();
        let due = store().lock().map(|s| s.due(now)).unwrap_or_default();
        let mut changed = false;
        if !due.is_empty() {
            let mut src = Sources::from_env();
            src.routes = refresh_routes(&app, now).await;
            let got = futures_util::future::join_all(due.iter().map(|id| fetch(id, &src, now))).await;
            let at = crate::config::now_ms();
            if let Ok(mut s) = store().lock() {
                for (id, got) in due.iter().zip(got) {
                    changed |= s.apply(id, at, got);
                }
            }
        }
        if changed {
            app.poke.notify_one();
        }
        let wait = store()
            .lock()
            .map(|s| s.next_due().saturating_sub(crate::config::now_ms()))
            .unwrap_or(LOOK_MS)
            .clamp(2_000, LOOK_MS);
        tokio::select! {
            _ = wake().notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_millis(wait)) => {}
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::model::*;
    use super::*;

    pub const NOW: u64 = 1_792_000_000_000;

    /// `2026-10-29T…Z` of a moment, for building saved answers relative to a clock.
    pub fn iso(ms: u64) -> String {
        let secs = (ms / 1000) as i64;
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
    }

    fn report(id: &'static str) -> Report {
        Report::new(id, Account::default(), vec![Window::new(Kind::Session, 10., Some(NOW + HOUR), Some(SESSION))])
    }

    #[test]
    fn the_test_clock_helper_round_trips_through_the_planners_own_parser() {
        for ms in [0, NOW, NOW + 33 * MIN + 7_000, 4_102_444_800_000] {
            let parsed = crate::usage::parse_time_ms(Some(&serde_json::Value::String(iso(ms)))).unwrap();
            assert_eq!(parsed, ms / 1000 * 1000, "{}", iso(ms));
        }
    }

    #[test]
    fn everything_is_due_at_first_and_a_good_read_waits_ten_minutes() {
        let mut s = Store::default();
        assert_eq!(s.due(NOW), IDS);
        assert!(s.apply("claude", NOW, Some(Ok(report("claude")))));
        assert!(!s.due(NOW + REFRESH_MS - 1).contains(&"claude"));
        assert!(s.due(NOW + REFRESH_MS).contains(&"claude"));
    }

    #[test]
    fn a_provider_with_no_sign_in_leaves_no_reading_and_is_looked_for_again_soon() {
        let mut s = Store::default();
        s.apply("kimi", NOW, None);
        assert!(s.readings().is_empty());
        assert!(!s.due(NOW + LOOK_MS - 1).contains(&"kimi"));
        assert!(s.due(NOW + LOOK_MS).contains(&"kimi"));
    }

    #[test]
    fn a_failed_read_keeps_the_last_numbers_and_says_why() {
        let mut s = Store::default();
        s.apply("codex", NOW, Some(Ok(report("codex"))));
        assert!(s.apply("codex", NOW + REFRESH_MS, Some(Err(Fail::Network))));
        let r = &s.readings()[0];
        assert!(r.report.is_some(), "the numbers it had are kept");
        assert_eq!(r.fetched_at_ms, NOW, "and they are as old as they are");
        assert_eq!(r.error, Some(Fail::Network));
        assert!(!s.due(NOW + REFRESH_MS + ERROR_BACKOFF_MS - 1).contains(&"codex"));
        assert!(s.due(NOW + REFRESH_MS + ERROR_BACKOFF_MS).contains(&"codex"));
        // and an answer clears the reason
        s.apply("codex", NOW + 2 * REFRESH_MS, Some(Ok(report("codex"))));
        assert_eq!(s.readings()[0].error, None);
    }

    #[test]
    fn a_rate_limit_waits_longer_and_a_rejection_twice_as_long_each_time_up_to_six_hours() {
        let mut s = Store::default();
        s.apply("claude", NOW, Some(Err(Fail::RateLimited)));
        assert!(s.due(NOW + RATE_LIMITED_MS).contains(&"claude") && !s.due(NOW + RATE_LIMITED_MS - 1).contains(&"claude"));
        let mut t = NOW;
        let mut waits = Vec::new();
        for _ in 0..8 {
            s.apply("claude", t, Some(Err(Fail::Rejected)));
            let due = s.entries["claude"].due_ms;
            waits.push((due - t) / MIN);
            t = due;
        }
        assert_eq!(waits, [10, 20, 40, 80, 160, 320, 360, 360]);
        // an outage in between does not reset the count; an answer does
        s.apply("claude", t, Some(Err(Fail::Network)));
        assert_eq!(s.entries["claude"].strikes, 8);
        s.apply("claude", t, Some(Ok(report("claude"))));
        assert_eq!(s.entries["claude"].strikes, 0);
    }

    #[test]
    fn an_expired_sign_in_is_looked_at_again_without_waiting_for_the_network_backoff() {
        let mut s = Store::default();
        s.apply("kimi", NOW, Some(Err(Fail::Expired)));
        assert!(s.due(NOW + LOOK_MS).contains(&"kimi"));
        assert_eq!(s.readings()[0].error, Some(Fail::Expired));
    }

    #[test]
    fn the_button_asks_for_everything_forgets_rejections_and_cannot_be_pressed_twice_in_ten_seconds() {
        let mut s = Store::default();
        s.apply("claude", NOW, Some(Err(Fail::Rejected)));
        s.apply("claude", NOW, Some(Err(Fail::Rejected)));
        assert!(s.refresh_now(NOW + 1000));
        assert_eq!(s.due(NOW + 1000), IDS);
        assert_eq!(s.entries["claude"].strikes, 0);
        assert!(!s.refresh_now(NOW + 1000 + MANUAL_SPACING_MS - 1));
        assert!(s.refresh_now(NOW + 1000 + MANUAL_SPACING_MS));
    }

    #[test]
    fn settings_come_from_the_config_then_the_environment_and_hidden_ones_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let src = Sources::for_tests(tmp.path())
            .with_env("DEEPSEEK_API_KEY", "from-env")
            .with_settings("hide = [\"kimi\"]\n[deepseek]\napi_key = \" from-config \"\n");
        assert_eq!(format!("{:?}", src.secret("deepseek", "api_key", &["DEEPSEEK_API_KEY"]).unwrap()), "Secret(…)");
        assert!(src.secret("deepseek", "api_key", &["DEEPSEEK_API_KEY"]).is_some());
        assert_eq!(src.text("deepseek", "api_key").as_deref(), Some("from-config"));
        assert!(src.secret("deepseek", "platform_token", &["NOPE"]).is_none());
        let src = Sources::for_tests(tmp.path()).with_env("DEEPSEEK_KEY", "k");
        assert!(src.secret("deepseek", "api_key", &["DEEPSEEK_API_KEY", "DEEPSEEK_KEY"]).is_some());
        assert_eq!(src.env("DEEPSEEK_KEY").as_deref(), Some("k"));
    }

    #[test]
    fn the_config_file_is_read_for_the_usage_table_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "pass = \"x\"\n[usage]\nhide = [\"kimi\", \"codex\"]\n[usage.kimi]\nregion = \"international\"\n").unwrap();
        let mut src = Sources::for_tests(tmp.path());
        src.read_settings(&path);
        assert_eq!(src.hidden, ["kimi", "codex"]);
        assert_eq!(src.text("kimi", "region").as_deref(), Some("international"));
        // a missing or a broken file changes nothing
        let mut src = Sources::for_tests(tmp.path());
        src.read_settings(&tmp.path().join("nope.toml"));
        std::fs::write(&path, "[usage\n").unwrap();
        src.read_settings(&path);
        assert!(src.hidden.is_empty());
    }

    #[test]
    fn the_providers_have_names_and_links() {
        for id in IDS {
            assert!(!name_of(id).is_empty() && !links_of(id).0.is_empty(), "{id}");
        }
    }
}
