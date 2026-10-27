//! Reads the agent usage records Omarchy already maintains.
//!
//! The planner never probes a provider itself. Omarchy's
//! `omarchy-agent-usage-update` writes one record per agent under
//! `~/.local/state/omarchy/agents/usage/`, and the Claude collector keeps the
//! last successful limits probe in
//! `~/.cache/omarchy/agent-usage/claude-limits.json` together with
//! `fetchedAtMs`. A record may carry `limits[]`, a prepaid `balance`, both, or
//! neither; when neither is there the dashboard says «нет данных» instead of
//! inventing a number.

use crate::config::{now_ms, state_home};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Usage older than this is shown, but marked as stale.
pub const STALE_AFTER_MS: u64 = 30 * 60 * 1000;
/// A window at or above this share of its allowance used gets a warning.
pub const WARN_AT: f64 = 0.80;
/// A window at or above this share is critical.
pub const CRIT_AT: f64 = 0.95;

/// The subscriptions the dashboard reports, in display order. `claude` is the
/// only agent whose collector provides subscription limits today; `omp` and
/// `pi` are API-key agents and their records carry stats only.
pub const AGENTS: [(&str, &str); 3] = [("claude", "Claude"), ("omp", "omp"), ("pi", "pi")];

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Usage {
    /// When the files were read (unix ms).
    pub at: u64,
    pub stale_after_secs: u64,
    pub agents: Vec<AgentUsage>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct AgentUsage {
    pub id: String,
    pub name: String,
    /// Whether Omarchy wrote a record at all.
    pub present: bool,
    pub ready: bool,
    pub tier: String,
    /// The collector's own status line, e.g. `API keys · no subscription`.
    pub status: String,
    /// When the numbers were fetched; falls back to the record file's mtime.
    pub updated_at_ms: u64,
    pub stale: bool,
    pub limits: Vec<LimitWindow>,
    pub balance: Option<Balance>,
    /// Why there is nothing to show, when `limits` and `balance` are empty.
    pub note: String,
    /// What the collector says to do to sign in (`authHelpText`), when the agent is not ready.
    pub auth_help: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LimitWindow {
    pub label: String,
    /// Share used, 0..1.
    pub percent: f64,
    /// Raw `resetsAt` from the collector.
    pub resets_at: String,
    /// Parsed reset time (unix ms), 0 when absent or unparsable.
    pub resets_at_ms: u64,
    /// `ok`, `warn` or `crit`.
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Balance {
    pub remaining: f64,
    pub funded: f64,
    pub spent: f64,
    pub currency: String,
    pub estimated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Alert {
    pub level: String,
    pub stale: bool,
    pub worst: Option<WorstLimit>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorstLimit {
    pub agent: String,
    pub label: String,
    pub percent: f64,
    pub remaining_percent: u32,
    pub resets_at_ms: u64,
    pub updated_at_ms: u64,
    pub stale: bool,
}

pub fn read() -> Usage {
    read_at(
        now_ms(),
        &state_home().join("omarchy/agents/usage"),
        &cache_home().join("omarchy/agent-usage"),
    )
}

/// One program's record by its id, for any program Omarchy writes one for (the crew screen): the same reading as
/// the dashboard's, with the program's own name when the record has none.
pub fn agent_by_id(id: &str) -> AgentUsage {
    agent_at(
        now_ms(),
        id,
        &state_home().join("omarchy/agents/usage"),
        &cache_home().join("omarchy/agent-usage"),
    )
}

pub fn agent_at(now: u64, id: &str, state_dir: &Path, cache_dir: &Path) -> AgentUsage {
    let path = state_dir.join(format!("{id}.json"));
    let record = read_json(&path);
    let cache = if id == "claude" {
        read_json(&cache_dir.join("claude-limits.json"))
    } else {
        None
    };
    agent_usage(id, id, record.as_ref(), file_mtime_ms(&path), cache.as_ref(), now)
}

/// The model names a program's record says it was used with (`modelUsage`, `todayTokensByModel`), as the program
/// wrote them: `deepseek-v4-flash`, `anthropic/claude-opus-5`. Empty without a record.
pub fn seen_models(id: &str) -> Vec<String> {
    seen_models_at(&state_home().join("omarchy/agents/usage"), id)
}

pub fn seen_models_at(state_dir: &Path, id: &str) -> Vec<String> {
    let Some(record) = read_json(&state_dir.join(format!("{id}.json"))) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for key in ["todayTokensByModel", "modelUsage"] {
        if let Some(map) = record.get(key).and_then(Value::as_object) {
            for name in map.keys() {
                let name = name.trim();
                if !name.is_empty() && name.len() <= 80 && !name.contains(char::is_whitespace) && !out.iter().any(|n| n == name) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

fn cache_home() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::home().join(".cache"))
}

pub fn read_at(now: u64, state_dir: &Path, cache_dir: &Path) -> Usage {
    let claude_cache = read_json(&cache_dir.join("claude-limits.json"));
    let agents = AGENTS
        .iter()
        .map(|(id, fallback_name)| {
            let path = state_dir.join(format!("{id}.json"));
            let record = read_json(&path);
            let mtime = file_mtime_ms(&path);
            let cache = if *id == "claude" {
                claude_cache.as_ref()
            } else {
                None
            };
            agent_usage(id, fallback_name, record.as_ref(), mtime, cache, now)
        })
        .collect();
    Usage {
        at: now,
        stale_after_secs: STALE_AFTER_MS / 1000,
        agents,
    }
}

fn agent_usage(
    id: &str,
    fallback_name: &str,
    record: Option<&Value>,
    mtime: Option<u64>,
    claude_cache: Option<&Value>,
    now: u64,
) -> AgentUsage {
    let Some(record) = record else {
        return AgentUsage {
            id: id.into(),
            name: fallback_name.into(),
            present: false,
            note: "Omarchy не пишет запись для этого агента".into(),
            ..AgentUsage::default()
        };
    };

    let name = text(record.get("name")).unwrap_or_else(|| fallback_name.to_string());
    let ready = record
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tier = text(record.get("tierLabel")).unwrap_or_default();
    let status = text(record.get("usageStatusText")).unwrap_or_default();

    let mut updated_at_ms = parse_time_ms(record.get("updatedAt")).unwrap_or(0);
    if updated_at_ms == 0 {
        updated_at_ms = mtime.unwrap_or(0);
    }

    let mut limits = parse_limits(record);
    if let Some(cache) = claude_cache {
        let cached = parse_limits(cache);
        if limits.is_empty() && !cached.is_empty() {
            limits = cached;
        }
        // The probe cache is the only file that records when a provider
        // actually answered; the record itself is rewritten after every scan
        // even when its limits came from that cache.
        if let Some(fetched) = parse_time_ms(cache.get("fetchedAtMs")) {
            if fetched > 0 {
                updated_at_ms = fetched;
            }
        }
    }

    let stale = updated_at_ms > 0 && now.saturating_sub(updated_at_ms) > STALE_AFTER_MS;
    let balance = parse_balance(record);
    let note = if limits.is_empty() && balance.is_none() {
        if !status.is_empty() {
            format!("нет данных: {status}")
        } else if ready {
            "нет данных: Omarchy не получает лимиты этого агента".into()
        } else {
            "нет данных: агент не сообщает лимиты".into()
        }
    } else {
        String::new()
    };

    AgentUsage {
        id: id.into(),
        name,
        present: true,
        ready,
        tier,
        status,
        updated_at_ms,
        stale,
        limits,
        balance,
        note,
        auth_help: text(record.get("authHelpText")).unwrap_or_default(),
    }
}

fn parse_limits(value: &Value) -> Vec<LimitWindow> {
    let Some(entries) = value.get("limits").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let label = text(entry.get("title"))
                .filter(|s| !s.is_empty())
                .or_else(|| text(entry.get("label")))
                .filter(|s| !s.is_empty())?;
            let percent = percent_value(entry.get("percent"))?;
            let resets_at = text(entry.get("resetsAt")).unwrap_or_default();
            let resets_at_ms = parse_time_ms(entry.get("resetsAt")).unwrap_or(0);
            Some(LimitWindow {
                label,
                percent,
                resets_at,
                resets_at_ms,
                severity: severity(percent).to_string(),
            })
        })
        .collect()
}

/// Collectors write a fraction (0.21), but an older payload may carry a
/// percentage (21 or "21 %"); both mean the same window.
fn percent_value(value: Option<&Value>) -> Option<f64> {
    let n = match value? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().trim_end_matches('%').trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let fraction = if n > 1.0 { n / 100.0 } else { n };
    Some(fraction.clamp(0.0, 1.0))
}

pub fn severity(percent: f64) -> &'static str {
    if percent >= CRIT_AT {
        "crit"
    } else if percent >= WARN_AT {
        "warn"
    } else {
        "ok"
    }
}

fn parse_balance(value: &Value) -> Option<Balance> {
    let raw = value.get("balance")?;
    if !raw.is_object() {
        return None;
    }
    let remaining = number_value(raw.get("remaining"))?;
    if !remaining.is_finite() || remaining < 0.0 {
        return None;
    }
    Some(Balance {
        remaining,
        funded: number_value(raw.get("funded"))
            .filter(|n| n.is_finite() && *n > 0.0)
            .unwrap_or(0.0),
        spent: number_value(raw.get("spent"))
            .filter(|n| n.is_finite())
            .unwrap_or(0.0)
            .max(0.0),
        currency: text(raw.get("currency")).unwrap_or_else(|| "USD".into()),
        estimated: raw
            .get("estimated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn number_value(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn read_json(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn file_mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// The worst limit the Omarchy panel should flag. A window whose reset time
/// has already passed is ignored: its percentage describes an allowance that
/// is no longer being spent.
pub fn alert(usage: &Usage) -> Alert {
    alert_at(usage, now_ms())
}

pub fn alert_at(usage: &Usage, now: u64) -> Alert {
    let mut worst: Option<(&AgentUsage, &LimitWindow)> = None;
    for agent in &usage.agents {
        for window in &agent.limits {
            if window.resets_at_ms > 0 && window.resets_at_ms <= now {
                continue;
            }
            if worst.map_or(true, |(_, current)| window.percent > current.percent) {
                worst = Some((agent, window));
            }
        }
    }

    let level = worst
        .map(|(_, window)| window.severity.clone())
        .unwrap_or_else(|| "ok".into());
    Alert {
        stale: usage
            .agents
            .iter()
            .any(|agent| !agent.limits.is_empty() && agent.stale),
        level,
        worst: worst.map(|(agent, window)| WorstLimit {
            agent: agent.name.clone(),
            label: window.label.clone(),
            percent: window.percent,
            remaining_percent: ((1.0 - window.percent) * 100.0).round() as u32,
            resets_at_ms: window.resets_at_ms,
            updated_at_ms: agent.updated_at_ms,
            stale: agent.stale,
        }),
    }
}

/// Parses the timestamps collectors use: unix seconds or milliseconds, and
/// RFC 3339 with `Z` or a numeric offset.
pub fn parse_time_ms(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(n) => epoch_number_to_ms(n.as_f64()?),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            match s.parse::<f64>() {
                Ok(n) => epoch_number_to_ms(n),
                Err(_) => parse_iso_ms(s),
            }
        }
        _ => None,
    }
}

fn epoch_number_to_ms(n: f64) -> Option<u64> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    let ms = if n >= 100_000_000_000.0 {
        n
    } else {
        n * 1000.0
    };
    if ms < 0.0 {
        return None;
    }
    Some(ms.round() as u64)
}

fn parse_iso_ms(raw: &str) -> Option<u64> {
    let (date, remainder) = raw.split_once('T').or_else(|| raw.split_once(' '))?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }

    let mut clock = remainder;
    let mut offset_seconds = 0i64;
    if clock.ends_with('Z') || clock.ends_with('z') {
        clock = &clock[..clock.len() - 1];
    } else if let Some(pos) = clock.rfind(|c: char| c == '+' || c == '-') {
        if pos > 0 {
            let sign = if clock.as_bytes()[pos] == b'-' { -1 } else { 1 };
            let zone = &clock[pos + 1..];
            let (hours, minutes) = zone.split_once(':').unwrap_or((zone, "0"));
            let hours: i64 = hours.parse().ok()?;
            let minutes: i64 = minutes.parse().ok()?;
            offset_seconds = sign * (hours * 3600 + minutes * 60);
            clock = &clock[..pos];
        }
    }

    let mut parts = clock.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let second_part = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (seconds, fraction) = match second_part.split_once('.') {
        Some((secs, fraction)) => (secs, fraction),
        None => (second_part, ""),
    };
    let seconds: i64 = seconds.parse().ok()?;
    let millis = fraction_millis(fraction)?;

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hours)
        || !(0..=59).contains(&minutes)
        || !(0..=60).contains(&seconds)
    {
        return None;
    }

    let epoch_seconds =
        days_from_civil(year, month, day) * 86_400 + hours * 3_600 + minutes * 60 + seconds
            - offset_seconds;
    if epoch_seconds < 0 {
        return None;
    }
    Some(epoch_seconds as u64 * 1000 + millis as u64)
}

fn fraction_millis(fraction: &str) -> Option<i64> {
    if fraction.is_empty() {
        return Some(0);
    }
    let digits: String = fraction.chars().take(3).collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut padded = digits;
    while padded.len() < 3 {
        padded.push('0');
    }
    padded.parse().ok()
}

/// Days since 1970-01-01 (Howard Hinnant's civil-date algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = include_str!("../tests/fixtures/agent-usage/claude.json");
    const CLAUDE_LIMITS: &str = include_str!("../tests/fixtures/agent-usage/claude-limits.json");
    const OMP: &str = include_str!("../tests/fixtures/agent-usage/omp.json");
    const PI: &str = include_str!("../tests/fixtures/agent-usage/pi.json");
    const NOW: u64 = 1_792_413_620_000; // 2026-10-19T12:40:20Z

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn fixture_agent(id: &str, record: &str) -> AgentUsage {
        let name = AGENTS.iter().find(|(a, _)| *a == id).unwrap().1;
        let value = json(record);
        agent_usage(id, name, Some(&value), Some(NOW), None, NOW)
    }

    #[test]
    fn parses_claude_limits_tier_and_status() {
        let agent = fixture_agent("claude", CLAUDE);
        assert!(agent.present);
        assert_eq!(agent.name, "Claude Code");
        assert_eq!(agent.tier, "Max 5x");
        assert_eq!(agent.limits.len(), 3);
        assert_eq!(agent.limits[0].label, "Session (5-hour)");
        assert!((agent.limits[0].percent - 0.21).abs() < 1e-9);
        assert_eq!(agent.limits[2].label, "Fable Weekly");
        assert!((agent.limits[2].percent - 0.97).abs() < 1e-9);
        assert_eq!(agent.limits[2].severity, "crit");
        assert_eq!(agent.note, "");
        assert!(!agent.stale);
    }

    #[test]
    fn parses_reset_times_to_the_epoch() {
        let value = json(CLAUDE);
        let at = parse_time_ms(
            value
                .get("limits")
                .and_then(|l| l.get(1))
                .and_then(|l| l.get("resetsAt")),
        );
        assert_eq!(at, Some(1_792_472_399_677));
        assert_eq!(
            parse_time_ms(Some(&json("\"2026-10-19T17:40:09+05:00\""))),
            Some(1_792_413_609_000)
        );
        assert_eq!(
            parse_time_ms(Some(&json("\"1970-01-01T00:00:00Z\""))),
            Some(0)
        );
        assert_eq!(
            parse_time_ms(Some(&json("1792413609000"))),
            Some(1_792_413_609_000)
        );
        assert_eq!(
            parse_time_ms(Some(&json("1792413609"))),
            Some(1_792_413_609_000)
        );
        assert_eq!(parse_time_ms(Some(&json("\"not a time\""))), None);
    }

    #[test]
    fn percentages_become_fractions_and_are_clamped() {
        assert!((percent_value(Some(&json("0.21"))).unwrap() - 0.21).abs() < 1e-9);
        assert!((percent_value(Some(&json("21"))).unwrap() - 0.21).abs() < 1e-9);
        assert!((percent_value(Some(&json("\"21 %\""))).unwrap() - 0.21).abs() < 1e-9);
        assert!((percent_value(Some(&json("140"))).unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(percent_value(Some(&json("-1"))), None);
        assert_eq!(percent_value(Some(&json("\"n/a\""))), None);
    }

    #[test]
    fn severity_uses_the_80_and_95_percent_thresholds() {
        assert_eq!(severity(0.79), "ok");
        assert_eq!(severity(0.80), "warn");
        assert_eq!(severity(0.949), "warn");
        assert_eq!(severity(0.95), "crit");
        assert_eq!(severity(1.0), "crit");
    }

    #[test]
    fn claude_probe_cache_supplies_the_real_fetch_time() {
        let record = json(CLAUDE);
        let cache = json(CLAUDE_LIMITS);
        let agent = agent_usage(
            "claude",
            "Claude",
            Some(&record),
            Some(NOW),
            Some(&cache),
            NOW,
        );
        // 12:40:15Z is later than the record's own 12:40:09Z.
        assert_eq!(agent.updated_at_ms, 1_792_413_615_000);
        assert_eq!(agent.limits.len(), 3);
    }

    #[test]
    fn the_cache_also_stands_in_when_a_record_has_no_limits() {
        let mut record = json(CLAUDE);
        record["limits"] = json("[]");
        let cache = json(CLAUDE_LIMITS);
        let agent = agent_usage(
            "claude",
            "Claude",
            Some(&record),
            Some(NOW),
            Some(&cache),
            NOW,
        );
        assert_eq!(agent.limits.len(), 3);
        assert_eq!(agent.note, "");
    }

    #[test]
    fn omp_and_pi_say_no_data_instead_of_inventing_limits() {
        for (id, record) in [("omp", OMP), ("pi", PI)] {
            let agent = fixture_agent(id, record);
            assert!(agent.present);
            assert!(agent.limits.is_empty());
            assert!(agent.balance.is_none());
            assert_eq!(agent.status, "API keys · no subscription");
            assert!(agent.note.contains("нет данных"));
            assert!(agent.note.contains("API keys · no subscription"));
        }
    }

    #[test]
    fn a_missing_record_is_reported_as_no_data() {
        let agent = agent_usage("pi", "pi", None, None, None, NOW);
        assert!(!agent.present);
        assert!(agent.limits.is_empty());
        assert!(agent.note.contains("не пишет запись"));
    }

    #[test]
    fn old_records_are_marked_stale() {
        let record = json(CLAUDE);
        let cache = json(CLAUDE_LIMITS);
        let much_later = NOW + STALE_AFTER_MS + 1;
        let agent = agent_usage(
            "claude",
            "Claude",
            Some(&record),
            None,
            Some(&cache),
            much_later,
        );
        assert!(agent.stale);
    }

    #[test]
    fn read_at_reads_the_state_and_cache_directories() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-planner-usage-{}-{}",
            std::process::id(),
            NOW
        ));
        let state = dir.join("state");
        let cache = dir.join("cache");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::write(state.join("claude.json"), CLAUDE).unwrap();
        fs::write(state.join("omp.json"), OMP).unwrap();
        fs::write(state.join("pi.json"), PI).unwrap();
        fs::write(cache.join("claude-limits.json"), CLAUDE_LIMITS).unwrap();

        let usage = read_at(NOW, &state, &cache);
        assert_eq!(usage.stale_after_secs, 1800);
        assert_eq!(usage.agents.len(), 3);
        assert_eq!(usage.agents[0].limits.len(), 3);
        assert_eq!(usage.agents[1].id, "omp");
        assert!(usage.agents[1].limits.is_empty());
        assert_eq!(usage.agents[2].id, "pi");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn alert_picks_the_worst_live_window() {
        let usage = read_at(NOW, Path::new("/nonexistent"), Path::new("/nonexistent"));
        let alert = alert_at(&usage, NOW);
        assert_eq!(alert.level, "ok");
        assert!(alert.worst.is_none());

        let record = json(CLAUDE);
        let cache = json(CLAUDE_LIMITS);
        let claude = agent_usage("claude", "Claude", Some(&record), None, Some(&cache), NOW);
        let usage = Usage {
            at: NOW,
            stale_after_secs: 1800,
            agents: vec![claude],
        };
        let alert = alert_at(&usage, NOW);
        assert_eq!(alert.level, "crit");
        let worst = alert.worst.unwrap();
        assert_eq!(worst.agent, "Claude Code");
        assert_eq!(worst.label, "Fable Weekly");
        assert_eq!(worst.remaining_percent, 3);
        assert!(!alert.stale);

        // After every window resets, the stale 97% no longer counts.
        let after_reset = 1_792_472_400_000;
        let alert = alert_at(&usage, after_reset);
        assert_eq!(alert.level, "ok");
        assert!(alert.worst.is_none());
    }

    #[test]
    fn balance_is_read_when_a_record_has_one() {
        let record = json(
            r#"{"id":"pi","name":"Pi","ready":true,"balance":{"remaining":12.5,"funded":20,"spent":7.5,"currency":"USD","estimated":true}}"#,
        );
        let agent = agent_usage("pi", "pi", Some(&record), Some(NOW), None, NOW);
        let balance = agent.balance.unwrap();
        assert_eq!(balance.remaining, 12.5);
        assert_eq!(balance.funded, 20.0);
        assert_eq!(balance.spent, 7.5);
        assert!(balance.estimated);
        assert_eq!(agent.note, "");
    }
}
