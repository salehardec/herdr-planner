//! What a shipyard did (PLN-46): its load over the last hours and the day, and what the containers of every task ate.
//!
//! The history is thin on purpose. One row per minute for the last [`FINE_KEEP_SEC`], folded into [`COARSE_STEP`]-wide
//! rows (the mean of the minutes) for the day, and forgotten after [`KEEP_SEC`]: a yard watched for a week keeps under
//! five hundred rows. The three numbers are percentages of the machine, so a sparkline has no units of its own and a
//! hole (the yard did not answer) stays a hole.
//!
//! The task usage is integrated from the probes: docker's `CPUPerc` is an instant value, so two reads give a
//! trapezoid over the time between them, and a gap longer than [`USAGE_GAP_MS`] counts nothing (the yard was silent and
//! what happened on it is unknown). Memory is the sum of the memory of the task's running containers at one read; the
//! peak is the highest such sum. Both come from the container's `herdr.task` label, not from its name, exactly as the
//! task's containers are counted elsewhere.

use crate::db::Db;
use crate::runners::RunnerState;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

/// A minute-wide sample of the load.
pub const FINE_STEP: u64 = 60;
/// The width of a row the minutes are folded into.
pub const COARSE_STEP: u64 = 10 * 60;
/// Minutes are kept this long, then folded.
pub const FINE_KEEP_SEC: u64 = 6 * 3600;
/// Nothing older than this is kept.
pub const KEEP_SEC: u64 = 26 * 3600;
/// The longest silence that still counts as time a container ran; a longer gap is unknown and is not counted.
pub const USAGE_GAP_MS: u64 = 15 * 60 * 1000;
/// A task's usage is kept this long after the last read that saw it.
pub const USAGE_KEEP_MS: u64 = 30 * 24 * 3600 * 1000;
/// The label that says which task a container serves.
pub const LABEL: &str = "herdr.task";

/// One sample of a yard's load, in percent. `None` — the value could not be read (or the yard was silent then).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Point {
    /// Unix seconds of the sample's start.
    pub at: u64,
    pub cpu: Option<f64>,
    pub mem: Option<f64>,
    pub disk: Option<f64>,
}

/// What the containers of one task ate on a yard.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UseRow {
    /// The `herdr.task` label as the containers carry it (upper case).
    pub tag: String,
    /// The board task the label names; empty when this board does not know it.
    pub task: String,
    /// CPU time in core-seconds, summed over the reads.
    pub cpu_sec: f64,
    /// The highest sum of the memory of the task's running containers seen at one read, in bytes.
    pub mem_peak: u64,
    /// The first read that saw the task (the page says «since»).
    pub first_ms: u64,
}

/// A yard's history as the page reads it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct YardHistory {
    pub points: Vec<Point>,
    pub tasks: Vec<UseRow>,
}

/// What one read says about one task.
#[derive(Debug, Clone, Default, PartialEq)]
struct Delta {
    tag: String,
    task: String,
    cpu_sec: f64,
    mem_peak: u64,
}

/// A share in percent; `None` when the machine did not say, so the page draws a hole instead of a false zero.
pub fn percent(used: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| used as f64 / total as f64 * 100.0)
}

/// The load as a share of the cores (`load1 / nproc`).
pub fn load_percent(load1: f64, cpus: u64) -> Option<f64> {
    (cpus > 0 && load1.is_finite()).then(|| load1 / cpus as f64 * 100.0)
}

fn tag_of(c: &crate::runners::Container) -> Option<String> {
    let v = c.labels.get(LABEL)?.trim();
    (!v.is_empty()).then(|| v.to_uppercase())
}

fn task_of(c: &crate::runners::Container) -> String {
    c.task.as_ref().map(|t| t.id.clone()).unwrap_or_default()
}

/// Cores in use: docker prints `CPUPerc` in percent of one core, so 250 % is two and a half cores.
fn cores(c: &crate::runners::Container) -> Option<f64> {
    c.cpu_pct.map(|p| (p / 100.0).max(0.0))
}

/// What the containers of every task ate since the moment `from_ms` (the end of the previous count, never older than
/// the read before): CPU by a trapezoid over the gap, memory as the sum at this read. A task whose containers are gone
/// gets the last CPU it was seen using for the whole gap (that is the only honest guess: the read cannot know when it
/// stopped); a gap over [`USAGE_GAP_MS`] counts no CPU at all. A container the previous read did not have is charged
/// from its own `created` on, not from that read.
fn usage_of(prev: Option<&RunnerState>, fresh: &RunnerState, from_ms: u64) -> Vec<Delta> {
    let mut out: BTreeMap<String, Delta> = BTreeMap::new();
    let running: Vec<&crate::runners::Container> =
        fresh.containers.iter().filter(|c| c.state == "running").collect();
    for c in &running {
        let Some(tag) = tag_of(c) else { continue };
        let d = out.entry(tag.clone()).or_insert_with(|| Delta {
            tag,
            task: task_of(c),
            ..Delta::default()
        });
        if d.task.is_empty() {
            d.task = task_of(c);
        }
        d.mem_peak += c.mem_bytes.unwrap_or(0);
    }
    let gap = fresh.fetched_at_ms.saturating_sub(from_ms);
    if from_ms == 0 || gap == 0 || gap > USAGE_GAP_MS {
        return out.into_values().collect();
    }
    let dt = gap as f64 / 1000.0;
    // A failed read keeps the last good containers and its moment (`keep_last`): `ok` says nothing about them.
    let before: BTreeMap<&str, &crate::runners::Container> = prev
        .map(|p| {
            p.containers
                .iter()
                .filter(|c| c.state == "running")
                .map(|c| (c.id.as_str(), c))
                .collect()
        })
        .unwrap_or_default();
    for c in &running {
        let previous = before.get(c.id.as_str()).copied();
        let used = match (previous.and_then(cores), cores(c)) {
            (Some(a), Some(b)) => (a + b) / 2.0,
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => 0.0,
        };
        if used <= 0.0 {
            continue;
        }
        // Between the previous read and this one, the container may have been born.
        let since = match (previous, c.created_ms) {
            (None, created) if created > 0 => {
                fresh.fetched_at_ms.saturating_sub(created).min(gap) as f64 / 1000.0
            }
            _ => dt,
        };
        if let Some(tag) = tag_of(c) {
            let d = out.entry(tag.clone()).or_insert_with(|| Delta {
                tag,
                task: task_of(c),
                ..Delta::default()
            });
            d.cpu_sec += used * since;
        }
    }
    let seen: HashSet<&str> = fresh.containers.iter().map(|c| c.id.as_str()).collect();
    for c in prev
        .map(|p| p.containers.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|c| c.state == "running" && !seen.contains(c.id.as_str()))
    {
        let (Some(tag), Some(used)) = (tag_of(c), cores(c)) else { continue };
        let d = out.entry(tag.clone()).or_insert_with(|| Delta {
            tag,
            task: task_of(c),
            ..Delta::default()
        });
        d.cpu_sec += used * dt;
    }
    out.into_values().collect()
}

/// Stores one read of a yard: its load, the usage of every task, the fold and the pruning. `at_ms` is the moment of
/// the read (or of the silence, when the yard did not answer): a failed read leaves a hole in the line, so a silence
/// is never drawn as a straight line across it. The CPU interval up to `at_ms` is claimed once — a poll, a cleanup and
/// its own re-read can all see the same stretch of time, and it is counted for the first of them only.
pub fn record(db: &Db, prev: Option<&RunnerState>, state: &RunnerState, at_ms: u64) {
    let at = at_ms / 1000;
    if state.ok {
        let host = &state.host;
        db.hist_put(
            &state.id,
            FINE_STEP,
            at - at % FINE_STEP,
            load_percent(host.load1, host.cpus),
            percent(host.mem_total.saturating_sub(host.mem_avail), host.mem_total),
            percent(host.disk_size.saturating_sub(host.disk_avail), host.disk_size),
        );
        let counted = db.runner_usage_take(&state.id, at_ms);
        let from = counted.max(prev.map(|p| p.fetched_at_ms).unwrap_or(0));
        for d in usage_of(prev, state, from) {
            db.usage_add(&state.id, &d.tag, &d.task, d.cpu_sec, d.mem_peak, at_ms);
        }
    } else {
        db.hist_hole(&state.id, FINE_STEP, at - at % FINE_STEP);
    }
    // Whole coarse rows only: a row is folded once, from all of its minutes.
    let cut = (at.saturating_sub(FINE_KEEP_SEC) / COARSE_STEP) * COARSE_STEP;
    db.hist_fold(&state.id, cut, COARSE_STEP, at.saturating_sub(KEEP_SEC));
    db.usage_prune(at_ms.saturating_sub(USAGE_KEEP_MS));
}

fn round1(v: Option<f64>) -> Option<f64> {
    v.map(|x| (x * 10.0).round() / 10.0)
}

/// The history of the named yards: the samples (oldest first) and the usage of their tasks.
pub fn history(db: &Db, ids: &[String], now_ms: u64) -> BTreeMap<String, YardHistory> {
    let now = now_ms / 1000;
    let since = now.saturating_sub(KEEP_SEC);
    let mut out = BTreeMap::new();
    for id in ids {
        let mut by_at: BTreeMap<u64, Point> = BTreeMap::new();
        for (step, at, cpu, mem, disk) in db.hist_rows(id, since) {
            // A fine row and a folded one can share a start for a moment (the fold runs after the write): the fine
            // number wins, it is the one the page would show in the hours window.
            if step != FINE_STEP && by_at.contains_key(&at) {
                continue;
            }
            by_at.insert(
                at,
                Point { at, cpu: round1(cpu), mem: round1(mem), disk: round1(disk) },
            );
        }
        let tasks = db
            .usage_rows(id)
            .into_iter()
            .map(|(tag, task, cpu_sec, mem_peak, first_ms, _)| UseRow {
                tag,
                task,
                cpu_sec: (cpu_sec * 1000.0).round() / 1000.0,
                mem_peak,
                first_ms,
            })
            .collect();
        out.insert(
            id.clone(),
            YardHistory { points: by_at.into_values().collect(), tasks },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::{Container, HostState, RunnerState};

    fn state(ok: bool, at_ms: u64, containers: Vec<Container>) -> RunnerState {
        RunnerState {
            id: "vm-r".into(),
            ok,
            fetched_at_ms: at_ms,
            containers,
            ..RunnerState::default()
        }
    }

    fn container(id: char, name: &str, tag: &str, state: &str, cpu: f64, mem: u64) -> Container {
        Container {
            id: id.to_string().repeat(64),
            name: name.into(),
            state: state.into(),
            cpu_pct: Some(cpu),
            mem_bytes: Some(mem),
            labels: [("herdr.task".to_string(), tag.to_string())].into_iter().collect(),
            ..Container::default()
        }
    }

    fn host(load1: f64, cpus: u64, mem_total: u64, mem_avail: u64, disk: u64, avail: u64) -> RunnerState {
        RunnerState {
            id: "vm-r".into(),
            ok: true,
            fetched_at_ms: 1_000_000,
            host: HostState {
                load1,
                cpus,
                mem_total,
                mem_avail,
                disk_size: disk,
                disk_avail: avail,
                ..HostState::default()
            },
            ..RunnerState::default()
        }
    }

    #[test]
    fn the_load_is_a_share_of_the_machine() {
        assert_eq!(load_percent(1.2, 8), Some(15.0));
        assert_eq!(percent(8, 16), Some(50.0));
        assert_eq!(percent(0, 0), None, "no number of the machine, no line");
        assert_eq!(load_percent(1.0, 0), None);
        assert_eq!(load_percent(f64::NAN, 8), None);
    }

    #[test]
    fn cpu_is_a_trapezoid_between_two_reads_and_memory_is_what_runs_now() {
        let a = container('a', "pg-ALP-3", "ALP-3", "running", 100.0, 100);
        let prev = state(true, 1_000_000, vec![a.clone()]);
        let mut b = a.clone();
        b.cpu_pct = Some(300.0);
        b.mem_bytes = Some(400);
        let fresh = state(true, 1_010_000, vec![b]);
        let d = usage_of(Some(&prev), &fresh, prev.fetched_at_ms);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].tag, "ALP-3");
        assert_eq!(d[0].mem_peak, 400, "the sum at this read");
        assert!((d[0].cpu_sec - 20.0).abs() < 1e-9, "10 s at 1 … 3 cores: {}", d[0].cpu_sec);
    }

    #[test]
    fn memory_adds_up_over_the_task_containers() {
        let fresh = state(
            true,
            2_000_000,
            vec![
                container('a', "pg-ALP-3", "ALP-3", "running", 0.0, 300),
                container('b', "web-ALP-3", "alp-3", "running", 0.0, 200),
                container('c', "other", "OTHER-1", "running", 0.0, 999),
                container('d', "old-ALP-3", "ALP-3", "exited", 0.0, 100),
            ],
        );
        let d = usage_of(None, &fresh, 0);
        let row = d.iter().find(|x| x.tag == "ALP-3").unwrap();
        assert_eq!(row.mem_peak, 500, "the label is one task in any case, an exited container is not running");
        assert!(row.cpu_sec == 0.0, "one read gives no time");
    }

    #[test]
    fn a_container_gone_keeps_its_last_cpu_for_the_gap() {
        let a = container('a', "pg-ALP-3", "ALP-3", "running", 200.0, 100);
        let prev = state(true, 1_000_000, vec![a]);
        let fresh = state(true, 1_005_000, vec![]);
        let d = usage_of(Some(&prev), &fresh, prev.fetched_at_ms);
        assert_eq!(d.len(), 1);
        assert!((d[0].cpu_sec - 10.0).abs() < 1e-9, "5 s at 2 cores: {}", d[0].cpu_sec);
        assert_eq!(d[0].mem_peak, 0);
    }

    #[test]
    fn a_long_silence_counts_no_cpu() {
        let a = container('a', "pg-ALP-3", "ALP-3", "running", 100.0, 100);
        let prev = state(true, 1_000_000, vec![a.clone()]);
        let fresh = state(true, 1_000_000 + USAGE_GAP_MS + 1, vec![a]);
        assert!(usage_of(Some(&prev), &fresh, prev.fetched_at_ms).iter().all(|d| d.cpu_sec == 0.0));
        // The memory of what runs is still worth knowing.
        assert_eq!(usage_of(Some(&prev), &fresh, prev.fetched_at_ms)[0].mem_peak, 100);
    }

    #[test]
    fn a_failed_read_keeps_the_last_good_one_to_count_from() {
        // `keep_last` keeps the last good containers and their moment with ok = false: that CPU time is real, and a
        // silence longer than the gap is refused by `a_long_silence_counts_no_cpu` instead.
        let a = container('a', "pg-ALP-3", "ALP-3", "running", 100.0, 100);
        let prev = state(false, 1_000_000, vec![a.clone()]);
        let fresh = state(true, 1_010_000, vec![a]);
        assert!((usage_of(Some(&prev), &fresh, prev.fetched_at_ms)[0].cpu_sec - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_container_is_charged_from_its_created_on() {
        let prev = state(true, 1_000_000, vec![]);
        let mut c = container('a', "pg-ALP-3", "ALP-3", "running", 100.0, 10);
        // Born four minutes after the previous read, a minute before this one.
        c.created_ms = 1_240_000;
        let fresh = state(true, 1_300_000, vec![c]);
        let d = usage_of(Some(&prev), &fresh, prev.fetched_at_ms);
        assert_eq!(d.len(), 1);
        assert!((d[0].cpu_sec - 60.0).abs() < 1e-9, "one core for its 60 s, not for 300: {}", d[0].cpu_sec);
    }

    #[test]
    fn storing_a_read_keeps_the_moment_cpu_was_counted_to() {
        let db = Db::memory().unwrap();
        db.runner_usage_take("vm-r", 1_000);
        db.runner_put("vm-r", "{}", true, "");
        assert_eq!(db.runner_usage_take("vm-r", 2_000), 1_000, "a stored read must not rewind the claim");
    }

    #[test]
    fn a_silent_yard_leaves_a_hole_in_the_line() {
        let db = Db::memory().unwrap();
        let s = host(1.0, 4, 100, 40, 200, 50);
        let first = RunnerState { fetched_at_ms: 1_000_000, ..s.clone() };
        let dead = RunnerState { ok: false, fetched_at_ms: 1_120_000, ..s.clone() };
        let later = RunnerState { fetched_at_ms: 1_240_000, ..s.clone() };
        record(&db, None, &first, first.fetched_at_ms);
        record(&db, Some(&first), &dead, dead.fetched_at_ms);
        record(&db, Some(&dead), &later, later.fetched_at_ms);
        let h = history(&db, &["vm-r".into()], later.fetched_at_ms);
        let points = &h["vm-r"].points;
        assert_eq!(points.len(), 3, "{points:?}");
        assert_eq!(points[0].cpu, Some(25.0));
        assert_eq!(points[1].cpu, None, "the silence is a hole, not a line across it");
        assert_eq!(points[2].cpu, Some(25.0));
    }

    #[test]
    fn an_interval_is_counted_once_though_a_poll_and_a_cleanup_both_read() {
        let db = Db::memory().unwrap();
        let a = container('a', "pg-ALP-3", "ALP-3", "running", 100.0, 100);
        let t0 = 1_000_000u64;
        let start = state(true, t0, vec![a.clone()]);
        record(&db, None, &start, t0);
        // A poll stores a read 300 s later; the cleanup, which started from the older read, ends 60 s after it.
        let polled = state(true, t0 + 300_000, vec![a.clone()]);
        record(&db, Some(&start), &polled, polled.fetched_at_ms);
        let cleaned = state(true, t0 + 360_000, vec![a]);
        record(&db, Some(&start), &cleaned, cleaned.fetched_at_ms);
        let rows = db.usage_rows("vm-r");
        assert!((rows[0].2 - 360.0).abs() < 1e-9, "300 s and 60 s, each once: {}", rows[0].2);
    }

    #[test]
    fn the_label_is_the_task_not_the_name() {
        let c = Container {
            id: "a".repeat(64),
            name: "pg-ALP-3".into(),
            state: "running".into(),
            cpu_pct: Some(10.0),
            mem_bytes: Some(1),
            ..Container::default()
        };
        assert!(usage_of(None, &state(true, 1_000_000, vec![c]), 0).is_empty(), "no label, no row");
    }

    #[test]
    fn a_read_is_stored_thinned_and_kept_a_day() {
        let db = Db::memory().unwrap();
        let t0 = 10_000_000u64;
        let s = host(1.0, 4, 100, 40, 200, 50);
        let first = RunnerState { fetched_at_ms: t0 * 1000, ..s.clone() };
        record(&db, None, &first, first.fetched_at_ms);
        // A day later the old minute has been folded into a wide row, and both are still in the history.
        let later = RunnerState { fetched_at_ms: (t0 + 21 * 3600) * 1000, ..s.clone() };
        record(&db, Some(&first), &later, later.fetched_at_ms);
        let h = history(&db, &["vm-r".into()], (t0 + 21 * 3600) * 1000);
        let h = &h["vm-r"];
        assert_eq!(h.points.len(), 2, "a wide row of the day and a fresh minute: {:?}", h.points);
        let wide = &h.points[0];
        assert!(wide.at < t0 + 21 * 3600 - FINE_KEEP_SEC, "the old read is a row of the day now");
        assert_eq!(wide.at, t0 - t0 % COARSE_STEP);
        assert_eq!(wide.cpu, Some(25.0), "load1 1 of 4 cores");
        assert_eq!(wide.mem, Some(60.0));
        assert_eq!(wide.disk, Some(75.0));
        assert_eq!(h.points[1].at, (t0 + 21 * 3600) - (t0 + 21 * 3600) % FINE_STEP);
        // Nothing older than the day and a little survives the next fold.
        let day = RunnerState { fetched_at_ms: (t0 + 30 * 3600) * 1000, ..s.clone() };
        record(&db, Some(&later), &day, day.fetched_at_ms);
        let h = history(&db, &["vm-r".into()], (t0 + 30 * 3600) * 1000);
        assert!(h["vm-r"].points.iter().all(|p| p.at >= t0 + 30 * 3600 - KEEP_SEC), "{:?}", h["vm-r"].points);
    }

    #[test]
    fn a_fine_row_wins_the_moment_a_folded_one_shares() {
        let db = Db::memory().unwrap();
        db.hist_put("vm-r", COARSE_STEP, 600, Some(10.0), Some(20.0), Some(30.0));
        db.hist_put("vm-r", FINE_STEP, 600, Some(11.0), Some(21.0), Some(31.0));
        let h = history(&db, &["vm-r".into()], 1_000_000);
        assert_eq!(h["vm-r"].points.len(), 1);
        assert_eq!(h["vm-r"].points[0], Point { at: 600, cpu: Some(11.0), mem: Some(21.0), disk: Some(31.0) });
    }

    #[test]
    fn usage_is_summed_peaked_and_the_long_gone_is_forgotten() {
        let db = Db::memory().unwrap();
        let a = container('a', "pg-ALP-3", "ALP-3", "running", 100.0, 300);
        let mut b = a.clone();
        b.cpu_pct = Some(200.0);
        b.mem_bytes = Some(500);
        let prev = state(true, 1_000_000, vec![a]);
        let fresh = state(true, 1_010_000, vec![b]);
        let later = state(true, 1_020_000, vec![fresh.containers[0].clone()]);
        record(&db, Some(&prev), &fresh, fresh.fetched_at_ms);
        record(&db, Some(&fresh), &later, later.fetched_at_ms);
        let rows = db.usage_rows("vm-r");
        assert_eq!(rows.len(), 1);
        let (tag, task, cpu, peak, first, last) = &rows[0];
        assert_eq!(tag, "ALP-3");
        assert_eq!(task, "", "the label names no task of this board here");
        assert!((cpu - 35.0).abs() < 1e-9, "10 s at 1 … 2 cores and 10 s at 2: {cpu}");
        assert_eq!(*peak, 500, "the higher sum wins");
        assert_eq!(*first, 1_010_000, "the first read that wrote the row");
        assert_eq!(*last, 1_020_000);
        db.usage_prune(1_020_000 + USAGE_KEEP_MS + 1);
        assert!(db.usage_rows("vm-r").is_empty());
    }

    #[test]
    fn a_removed_yard_leaves_no_history() {
        let db = Db::memory().unwrap();
        let s = host(1.0, 4, 100, 40, 200, 50);
        record(&db, None, &s, s.fetched_at_ms);
        assert_eq!(db.hist_runners(), vec!["vm-r".to_string()]);
        db.hist_remove("vm-r");
        db.usage_remove("vm-r");
        assert!(db.hist_runners().is_empty(), "a yard with no rows is not named");
        let h = history(&db, &["vm-r".into()], 1_000_000);
        assert!(h["vm-r"].points.is_empty());
        assert!(h["vm-r"].tasks.is_empty());
    }
}
