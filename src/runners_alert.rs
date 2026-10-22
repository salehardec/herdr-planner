//! Alarms of the shipyards (PLN-42): little disk, a yard that does not answer, forgotten containers, a task's
//! container that fell or keeps restarting.
//!
//! The alarms are derived from the last read of a yard (nothing new is asked of the machine) and shown in «Ждёт тебя»
//! and in the yard's focus. The Coordinator hears about them once: a key that has been told about is kept in the
//! database and is not told again until that alarm goes away and comes back — a change of state.

use crate::app::{self, Shared};
use crate::runners::{Class, Container, RunnerState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::Duration;

/// The defaults: a yard with less than a tenth of its disk free, or one that has not answered for ten minutes.
pub const DEFAULT_DISK_FREE_PCT: u64 = 10;
pub const DEFAULT_OFFLINE_MIN: u64 = 10;
/// A threshold beyond these is a mistake in the file, not a wish.
pub const MAX_DISK_FREE_PCT: u64 = 99;
pub const MAX_OFFLINE_MIN: u64 = 24 * 60;

/// When a yard is worth a word, as the human set it for that yard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertRules {
    /// Alarms when the free share of the docker disk is below this, in percent (0 — never).
    #[serde(default = "default_disk_free_pct")]
    pub disk_free_pct: u64,
    /// Alarms when the yard has not answered for this many minutes (0 — at once).
    #[serde(default = "default_offline_min")]
    pub offline_min: u64,
}

fn default_disk_free_pct() -> u64 {
    DEFAULT_DISK_FREE_PCT
}

fn default_offline_min() -> u64 {
    DEFAULT_OFFLINE_MIN
}

impl Default for AlertRules {
    fn default() -> Self {
        Self {
            disk_free_pct: DEFAULT_DISK_FREE_PCT,
            offline_min: DEFAULT_OFFLINE_MIN,
        }
    }
}

impl AlertRules {
    pub fn validate(&self) -> Result<(), String> {
        if self.disk_free_pct > MAX_DISK_FREE_PCT {
            return Err(format!(
                "порог свободного места: от 0 до {MAX_DISK_FREE_PCT} %, не {}",
                self.disk_free_pct
            ));
        }
        if self.offline_min > MAX_OFFLINE_MIN {
            return Err(format!(
                "порог молчания верфи: от 0 до {MAX_OFFLINE_MIN} мин, не {}",
                self.offline_min
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Offline,
    Disk,
    Crash,
    Forgotten,
}

impl Kind {
    fn rank(self) -> u8 {
        match self {
            Kind::Offline => 0,
            Kind::Disk => 1,
            Kind::Crash => 2,
            Kind::Forgotten => 3,
        }
    }
}

/// One alarm. `key` names the state it stands for: the same key while that state lasts, a new one after it went away
/// and came back.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Alert {
    pub key: String,
    pub runner: String,
    pub kind: Kind,
    /// When the state was first seen (ms).
    #[serde(default)]
    pub since_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_pct: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    /// How long the yard has been silent, in minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The task of the container (its id on the board), for a way to it from the alarm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// A container of a task that is not running: keeps restarting, is dead, or fell with a non-zero exit code. A clean
/// `Exited (0)` is a stop somebody asked for, not a fall.
fn broken(c: &Container) -> bool {
    match c.state.as_str() {
        "restarting" | "dead" => true,
        "exited" => exit_code(&c.status).is_some_and(|n| n != 0),
        _ => false,
    }
}

fn exit_code(status: &str) -> Option<i64> {
    let rest = status.split("Exited (").nth(1)?;
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

/// The alarms of one yard at `now`, from its last read. Pure: nothing is sent anywhere and nothing is stored.
///
/// A yard that does not answer gives one alarm about that and nothing else: the containers and the disk are from the
/// last good read, and acting on a stale list is worse than saying plainly that the yard is silent.
pub fn alerts_of(st: &RunnerState, now: u64) -> Vec<Alert> {
    let rules = &st.alert_rules;
    let mut out: Vec<Alert> = Vec::new();
    if !st.ok {
        // Only a silence we have seen begin: a yard restored from the cache is not live yet, and its last read may be
        // hours old without meaning that the yard is silent.
        let since = st.down_since_ms;
        if since > 0 {
            let age = now.saturating_sub(since);
            if age >= rules.offline_min.saturating_mul(60_000) {
                out.push(Alert {
                    key: format!("{}:offline", st.id),
                    runner: st.id.clone(),
                    kind: Kind::Offline,
                    since_ms: since,
                    minutes: Some(age / 60_000),
                    ..Alert::default()
                });
            }
        }
        return out;
    }
    let h = &st.host;
    if h.disk_size > 0 {
        let pct = h.disk_avail.min(h.disk_size) * 100 / h.disk_size;
        if pct < rules.disk_free_pct {
            out.push(Alert {
                key: format!("{}:disk", st.id),
                runner: st.id.clone(),
                kind: Kind::Disk,
                free_pct: Some(pct),
                free_bytes: Some(h.disk_avail),
                total_bytes: Some(h.disk_size),
                ..Alert::default()
            });
        }
    }
    for c in &st.containers {
        let tag = c.task.as_ref().map(|t| t.tag.clone()).filter(|t| !t.is_empty());
        let task = c.task.as_ref().map(|t| t.id.clone());
        match c.class {
            Class::Forgotten => out.push(Alert {
                key: format!("{}:forgotten:{}", st.id, c.id),
                runner: st.id.clone(),
                kind: Kind::Forgotten,
                name: Some(c.name.clone()),
                tag,
                task,
                ..Alert::default()
            }),
            Class::Active if broken(c) => out.push(Alert {
                key: format!("{}:crash:{}", st.id, c.id),
                runner: st.id.clone(),
                kind: Kind::Crash,
                name: Some(c.name.clone()),
                tag,
                task,
                state: Some(c.state.clone()),
                status: (!c.status.is_empty()).then(|| c.status.clone()),
                ..Alert::default()
            }),
            _ => {}
        }
    }
    out.sort_by(|a, b| {
        (a.kind.rank(), a.name.as_deref().unwrap_or(""), a.key.as_str()).cmp(&(
            b.kind.rank(),
            b.name.as_deref().unwrap_or(""),
            b.key.as_str(),
        ))
    });
    out
}

/// What the database keeps for one yard: the alarms shown last time and the keys the Coordinator has heard about.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub alerts: Vec<Alert>,
    #[serde(default)]
    pub sent: Vec<String>,
}

pub fn meta_key(id: &str) -> String {
    format!("runner_alerts:{id}")
}

fn since_of(v: u64, now: u64) -> u64 {
    if v > 0 {
        v
    } else {
        now
    }
}

/// Fits fresh alarms into the book: the same state keeps the moment it was first seen, and an alarm whose key was
/// not told yet is new. The returned book carries exactly the keys now shown — an alarm that went away is forgotten,
/// so its next appearance is news again.
pub fn reconcile(book: &Book, fresh: Vec<Alert>, now: u64) -> (Vec<Alert>, Vec<Alert>, Book) {
    let sent: BTreeSet<&str> = book.sent.iter().map(String::as_str).collect();
    let mut alerts: Vec<Alert> = Vec::with_capacity(fresh.len());
    let mut new: Vec<Alert> = Vec::new();
    for mut a in fresh {
        a.since_ms = match book.alerts.iter().find(|x| x.key == a.key) {
            Some(old) => since_of(old.since_ms, now).min(since_of(a.since_ms, now)),
            None => since_of(a.since_ms, now),
        };
        if !sent.contains(a.key.as_str()) {
            new.push(a.clone());
        }
        alerts.push(a);
    }
    let next = Book {
        sent: alerts.iter().map(|a| a.key.clone()).collect(),
        alerts: alerts.clone(),
    };
    (alerts, new, next)
}

/* ───────── the Coordinator ───────── */

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / GB)
}

/// One line in the human's words. The page has its own strings (i18n); this goes to the Coordinator as a message.
pub fn line(a: &Alert) -> String {
    match a.kind {
        Kind::Offline => format!(
            "Верфь {}: не отвечает {} мин.",
            a.runner,
            a.minutes.unwrap_or(0)
        ),
        Kind::Disk => format!(
            "Верфь {}: мало места на диске — свободно {} % ({} из {} ГБ).",
            a.runner,
            a.free_pct.unwrap_or(0),
            gb(a.free_bytes.unwrap_or(0)),
            gb(a.total_bytes.unwrap_or(0))
        ),
        Kind::Crash => {
            let name = a.name.as_deref().unwrap_or("");
            let tag = a.tag.as_deref().unwrap_or("");
            let task = if tag.is_empty() {
                String::new()
            } else {
                format!(" задачи {tag}")
            };
            let status = a
                .status
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!(" — {s}"))
                .unwrap_or_default();
            format!("Верфь {}: контейнер {name}{task} не работает{status}.", a.runner)
        }
        Kind::Forgotten => {
            let name = a.name.as_deref().unwrap_or("");
            let tag = a.tag.as_deref().unwrap_or("");
            let task = if tag.is_empty() {
                String::new()
            } else {
                format!(" задачи {tag}")
            };
            format!("Верфь {}: забыт контейнер {name}{task}.", a.runner)
        }
    }
}

/// One message for the Coordinator, one line per new alarm.
pub fn message(alerts: &[Alert]) -> String {
    let lines: Vec<String> = alerts.iter().map(line).collect();
    lines.join("\n")
}

/// Derives the alarms of every yard, keeps the book, and sends the Coordinator the new ones once.
pub async fn apply(app: &Shared, states: &mut [RunnerState], now: u64) {
    // Two refreshes can meet here (the poll and a board read): one book, one message.
    let _guard = app.runner_alerts_lock.lock().await;
    let mut new_all: Vec<Alert> = Vec::new();
    for st in states.iter_mut() {
        let current = alerts_of(st, now);
        let book: Book = app
            .db
            .meta_get(&meta_key(&st.id))
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let (alerts, new, next) = reconcile(&book, current, now);
        st.alerts = alerts;
        if let Ok(json) = serde_json::to_string(&next) {
            app.db.meta_set(&meta_key(&st.id), &json);
        }
        new_all.extend(new);
    }
    if new_all.is_empty() {
        return;
    }
    let text = message(&new_all);
    app::journal_note(app, "runner", "runner_alert", &text.replace('\n', "; ")).await;
    // The Coordinator is told in the background: a slow board command must not hold the page's snapshot.
    let cfg = app.cfg.clone();
    tokio::spawn(async move {
        if let Err(e) = app::run_board(
            &cfg,
            &["coordinator", "tell", "--", &text],
            Duration::from_secs(30),
        )
        .await
        {
            eprintln!("herdr-planner: тревоги верфей не ушли Координатору: {e}");
        }
    });
}

/* ───────── checks ───────── */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::{Container, HostState, RunnerState, TaskRef};
    use std::collections::BTreeMap;

    fn state() -> RunnerState {
        RunnerState {
            id: "vm-r".into(),
            ok: true,
            fetched_at_ms: 1_000,
            host: HostState {
                disk_size: 500_000_000_000,
                disk_avail: 300_000_000_000,
                ..HostState::default()
            },
            alert_rules: AlertRules::default(),
            ..RunnerState::default()
        }
    }

    fn container(id: &str, name: &str, class: Class, st: &str) -> Container {
        Container {
            id: id.into(),
            name: name.into(),
            state: st.into(),
            status: match st {
                "running" => "Up 2 hours".into(),
                "exited" => "Exited (137) 2 minutes ago".into(),
                "restarting" => "Restarting (1) 2 seconds ago".into(),
                _ => String::new(),
            },
            class,
            labels: BTreeMap::new(),
            ..Container::default()
        }
    }

    #[test]
    fn a_healthy_yard_has_no_alarms() {
        let st = state();
        assert!(alerts_of(&st, 2_000).is_empty(), "60 % free, nothing forgotten");
    }

    #[test]
    fn the_disk_threshold_is_the_human_s_one() {
        let mut st = state();
        st.host.disk_size = 100_000_000_000;
        st.host.disk_avail = 9_000_000_000; // 9 %
        let list = alerts_of(&st, 2_000);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, Kind::Disk);
        assert_eq!(list[0].free_pct, Some(9));
        assert_eq!(list[0].key, "vm-r:disk");
        // exactly at the threshold is fine: «меньше 10 %»
        st.host.disk_avail = 10_000_000_000;
        assert!(alerts_of(&st, 2_000).is_empty());
        // the human's own threshold
        st.alert_rules.disk_free_pct = 20;
        assert_eq!(alerts_of(&st, 2_000).len(), 1);
        // 0 turns the disk alarm off
        st.alert_rules.disk_free_pct = 0;
        st.host.disk_avail = 1;
        assert!(alerts_of(&st, 2_000).is_empty());
    }

    #[test]
    fn a_silent_yard_alarms_only_after_the_minutes_the_human_set() {
        let mut st = state();
        st.ok = false;
        st.fetched_at_ms = 10_000_000;
        st.down_since_ms = 10_000_000 - 9 * 60_000; // nine minutes ago
        let now = 10_000_000;
        assert!(alerts_of(&st, now).is_empty(), "9 minutes, the default is 10");
        st.alert_rules.offline_min = 5;
        let list = alerts_of(&st, now);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, Kind::Offline);
        assert_eq!(list[0].minutes, Some(9));
        assert_eq!(list[0].since_ms, 10_000_000 - 9 * 60_000);
        st.alert_rules.offline_min = 0;
        assert_eq!(alerts_of(&st, 10_000_001).len(), 1, "0 means at once");
        // a silent yard says nothing about containers from the last good read
        st.host.disk_size = 100;
        st.host.disk_avail = 1;
        st.containers = vec![container("a", "web", Class::Forgotten, "exited")];
        let again = alerts_of(&st, now);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].kind, Kind::Offline);
    }

    #[test]
    fn a_fallen_container_of_a_live_task_alarms_and_a_stopped_one_of_a_done_task_is_forgotten() {
        let mut st = state();
        st.containers = vec![
            container("a", "pg-a", Class::Active, "running"),
            container("b", "pg-b", Class::Active, "restarting"),
            container("c", "pg-c", Class::Active, "exited"),
            container("d", "pg-d", Class::Forgotten, "exited"),
            container("e", "x-unsure", Class::Unsure, "exited"),
            container("f", "neighbor", Class::Foreign, "exited"),
            container("g", "paused-one", Class::Active, "paused"),
            container("h", "stopped-by-hand", Class::Active, "exited"),
        ];
        st.containers[7].status = "Exited (0) 1 second ago".into();
        st.containers[3].task = Some(TaskRef {
            id: "alpha/1".into(),
            tag: "ALP-1".into(),
            ..TaskRef::default()
        });
        st.containers[1].task = Some(TaskRef {
            id: "alpha/2".into(),
            tag: "ALP-2".into(),
            ..TaskRef::default()
        });
        let list = alerts_of(&st, 2_000);
        let kinds: Vec<(Kind, &str)> = list.iter().map(|a| (a.kind, a.key.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (Kind::Crash, "vm-r:crash:b"),
                (Kind::Crash, "vm-r:crash:c"),
                (Kind::Forgotten, "vm-r:forgotten:d"),
            ],
            "running, paused, cleanly stopped, unsure and foreign containers are not alarms"
        );
        assert_eq!(list[0].tag.as_deref(), Some("ALP-2"));
        assert_eq!(list[1].status.as_deref(), Some("Exited (137) 2 minutes ago"));
    }

    #[test]
    fn a_yard_restored_from_the_cache_is_not_called_silent() {
        // The planner starts with the last reads marked «not live» until a probe answers: their age is not a silence.
        let mut st = state();
        st.ok = false;
        st.fetched_at_ms = 1_000; // an hour ago, say
        st.down_since_ms = 0;
        assert!(alerts_of(&st, 3_600_000).is_empty());
    }

    #[test]
    fn a_told_alarm_is_not_told_again_until_it_goes_and_comes_back() {
        let mut st = state();
        st.host.disk_size = 100;
        st.host.disk_avail = 5;
        let fresh = alerts_of(&st, 1_000);
        assert_eq!(fresh.len(), 1);
        let (alerts, new, book) = reconcile(&Book::default(), fresh, 1_000);
        assert_eq!(alerts[0].since_ms, 1_000);
        assert_eq!(new.len(), 1, "the first sight is news");
        assert_eq!(book.sent, vec!["vm-r:disk".to_string()]);
        // the same state later: still shown, not news, the first sight kept
        let again = alerts_of(&st, 90_000);
        let (alerts2, new2, book2) = reconcile(&book, again, 90_000);
        assert_eq!(alerts2[0].since_ms, 1_000);
        assert!(new2.is_empty());
        assert_eq!(book2.sent, book.sent);
        // the alarm goes away: it is forgotten, and so is the mark
        let (none, new3, book3) = reconcile(&book2, Vec::new(), 100_000);
        assert!(none.is_empty() && new3.is_empty());
        assert!(book3.sent.is_empty() && book3.alerts.is_empty());
        // and comes back: news again
        let (_, new4, _) = reconcile(&book3, alerts_of(&st, 200_000), 200_000);
        assert_eq!(new4.len(), 1);
        assert_eq!(new4[0].since_ms, 200_000);
    }

    #[test]
    fn the_message_to_the_coordinator_names_each_alarm() {
        let alerts = vec![
            Alert {
                key: "vm-r:offline".into(),
                runner: "vm-r".into(),
                kind: Kind::Offline,
                minutes: Some(12),
                ..Alert::default()
            },
            Alert {
                key: "vm-r:disk".into(),
                runner: "vm-r".into(),
                kind: Kind::Disk,
                free_pct: Some(8),
                free_bytes: Some(40 * 1024 * 1024 * 1024),
                total_bytes: Some(500 * 1024 * 1024 * 1024),
                ..Alert::default()
            },
            Alert {
                key: "vm-r:crash:b".into(),
                runner: "vm-r".into(),
                kind: Kind::Crash,
                name: Some("pg-ALP-2".into()),
                tag: Some("ALP-2".into()),
                status: Some("Restarting (1) 2 seconds ago".into()),
                ..Alert::default()
            },
            Alert {
                key: "vm-r:forgotten:d".into(),
                runner: "vm-r".into(),
                kind: Kind::Forgotten,
                name: Some("web-ALP-1".into()),
                tag: Some("ALP-1".into()),
                ..Alert::default()
            },
        ];
        let text = message(&alerts);
        assert!(text.contains("Верфь vm-r: не отвечает 12 мин."), "{text}");
        assert!(text.contains("свободно 8 % (40.0 из 500.0 ГБ)"), "{text}");
        assert!(text.contains("контейнер pg-ALP-2 задачи ALP-2 не работает — Restarting"), "{text}");
        assert!(text.contains("забыт контейнер web-ALP-1 задачи ALP-1"), "{text}");
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn the_rules_refuse_what_no_yard_would_want() {
        assert!(AlertRules::default().validate().is_ok());
        assert!(AlertRules { disk_free_pct: 0, offline_min: 0 }.validate().is_ok());
        assert!(AlertRules { disk_free_pct: 100, ..AlertRules::default() }.validate().is_err());
        assert!(AlertRules { offline_min: 24 * 60 + 1, ..AlertRules::default() }.validate().is_err());
    }
}
