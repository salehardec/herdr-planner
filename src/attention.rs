//! What really waits on the user. herdr-projects files a thread under
//! «Waiting on you» for anything from a permission prompt to an agent that
//! finished and left `Waiting for you` as its last report, so its count is
//! not a signal by itself. Here every project's attention is rebuilt:
//!
//! 1. the coordinator reported `Waiting for you` (and is not working since);
//! 2. a thread sits on a prompt (`blocked`);
//! 3. a thread is broken: failed start, a brief that never arrived, a launch
//!    stuck on a dialog, a pane that vanished without a report;
//! 4. a thread asks and has not finished (its own report says `Waiting for
//!    you` and herdr does not call it `done`).
//!
//! A `done` thread with `Waiting for you` has finished: it goes to review.
//!
//! Review (👁) is a finished thread whose report nobody has looked at, and
//! whose pull or merge request is not merged or closed, and whose report is not
//! older than the cut-off (`review_stale` days). A thread whose request is
//! merged or closed «can be closed» (quiet); one with an old report is
//! «forgotten» (quiet, grey on the page); neither counts in the bar or the popup.
//!
//! The one place this is decided is [`apply`]; the page, `/api/summary`,
//! `/api/overview` and the bar plugin only show its result.

use crate::model::{Project, Wait};
use crate::mrstate::MrState;
use crate::sources::threads::Thread;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// The activity an agent reports when it stops to ask the user.
pub const WAITING: &str = "Waiting for you";

/// What a thread that herdr-projects put under «Waiting on you» really is.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Not in that group: nothing to decide.
    Untouched,
    /// The user is needed.
    Wait(Wait),
    /// Finished work to look at, not to answer; the time it arrived (ms).
    Review(u64),
    /// Its pull/merge request is merged or closed: the thread can be closed.
    CanClose,
    /// Its report is older than the cut-off: forgotten.
    Forgotten,
    /// The report was looked at (acknowledged).
    Seen,
}

/// What the verdicts depend on besides the thread itself.
pub struct Env<'a> {
    /// Now, unix milliseconds.
    pub now: u64,
    /// The review cut-off in milliseconds; 0 = off.
    pub stale_ms: u64,
    /// The state of the merge request in each thread's report, by thread id.
    pub mr: &'a HashMap<String, MrState>,
}

fn is_waiting_group(group: &str) -> bool {
    matches!(group, "Waiting on you" | "Needs you")
}

/// The thread's agent state without the ` , on MACHINE` tail remote threads add.
fn state_of(note: &str) -> &str {
    note.split_once(", on ").map_or(note, |(s, _)| s).trim()
}

/// `2026-10-14T12:38:16Z` as unix milliseconds; 0 when it does not parse.
pub fn iso_ms(s: &str) -> u64 {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') {
        return 0;
    }
    let num = |a: usize, z: usize| s.get(a..z).and_then(|t| t.parse::<i64>().ok());
    let (Some(y), Some(m), Some(d), Some(h), Some(mi), Some(se)) = (
        num(0, 4),
        num(5, 7),
        num(8, 10),
        num(11, 13),
        num(14, 16),
        num(17, 19),
    ) else {
        return 0;
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return 0;
    }
    // Days since 1970-01-01 (Howard Hinnant's civil-date algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + se;
    if secs < 0 {
        0
    } else {
        secs as u64 * 1000
    }
}

fn newest(times: &[&str]) -> u64 {
    times.iter().map(|t| iso_ms(t)).max().unwrap_or(0)
}

/// When a finished thread's result arrived: the later of its last report
/// change and its last state change.
pub fn review_since(t: &Thread) -> u64 {
    let since = newest(&[&t.last_report_change, &t.last_state_change]);
    if since > 0 {
        since
    } else {
        newest(&[&t.updated])
    }
}

/// A finished thread: is it still review? See the module doc.
fn review_verdict(t: &Thread, env: &Env) -> Verdict {
    let pr_over =
        t.pr_state.eq_ignore_ascii_case("merged") || t.pr_state.eq_ignore_ascii_case("closed");
    if pr_over || env.mr.get(&t.id).is_some_and(|s| s.done()) {
        return Verdict::CanClose;
    }
    let since = review_since(t);
    if env.stale_ms > 0 && since > 0 && env.now.saturating_sub(since) > env.stale_ms {
        return Verdict::Forgotten;
    }
    if !t.report_hash.is_empty() && t.acked_report_hash == t.report_hash {
        return Verdict::Seen;
    }
    Verdict::Review(since)
}

fn wait(kind: &str, t: &Thread, text: String, since: u64) -> Verdict {
    Verdict::Wait(Wait {
        kind: kind.into(),
        thread: t.id.clone(),
        text,
        since,
    })
}

/// Decides one thread. Only threads herdr-projects filed under «Waiting on
/// you» or «Ready for review» are judged; every other thread is [`Verdict::Untouched`].
pub fn judge(t: &Thread, env: &Env) -> Verdict {
    if t.closed() {
        return Verdict::Untouched;
    }
    if t.group == "Ready for review" {
        return review_verdict(t, env);
    }
    if !is_waiting_group(&t.group) {
        return Verdict::Untouched;
    }
    let id = &t.id;
    let state = state_of(&t.note);
    // When it last changed: the state, else the record.
    let moved = newest(&[&t.last_state_change, &t.updated]);
    if t.failed() {
        return wait(
            "failed",
            t,
            format!("сбой запуска {id}"),
            newest(&[&t.updated, &t.created]),
        );
    }
    if t.status == "starting" {
        return wait(
            "stuck",
            t,
            format!("запуск {id} завис"),
            newest(&[&t.created]),
        );
    }
    if t.brief_stuck {
        return wait("stuck", t, format!("бриф не дошёл до {id}"), moved);
    }
    if state == "blocked" {
        return wait("blocked", t, format!("разрешение или вопрос в {id}"), moved);
    }
    if t.prompt_pending {
        return wait("stuck", t, format!("запуск {id} стоит на диалоге"), moved);
    }
    if state == "pane closed" {
        return wait("lost", t, format!("панель {id} закрыта без отчёта"), moved);
    }
    if t.activity == WAITING {
        // Finished (herdr says `done`): a result to look at. The agent only
        // forgot to change its last report.
        if t.last_state == "done" || state == "done" {
            return review_verdict(t, env);
        }
        return wait("asking", t, format!("вопрос в {id}"), review_since(t));
    }
    // herdr-projects waits on it for a reason this reading does not know:
    // visible as a review, never a bell.
    review_verdict(t, env)
}

/// One pane's self-report, as `herdr-projects report` writes it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub socket: String,
    pub pane_id: String,
    pub activity: String,
    pub reported_at: i64,
}

/// Every record under `<root>/.progress/`.
pub fn read_progress(root: &Path) -> Vec<Progress> {
    let Ok(rd) = std::fs::read_dir(root.join(".progress")) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str::<Progress>(&s).ok())
        .collect()
}

/// Whether the herdr-projects ticker (which keeps the coordinators' state
/// and prunes the records of closed panes) is running.
pub fn ticker_alive(root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join(".ticker.lock")) else {
        return false;
    };
    let pid = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v["pid"].as_u64());
    pid.is_some_and(|p| Path::new(&format!("/proc/{p}")).exists())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LivePane {
    pane_id: String,
    agent_status: String,
    agent: String,
    agent_session: String,
}

/// A coordinator whose own report says `Waiting for you`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// When it reported, unix milliseconds.
    pub since: u64,
    pub agent: String,
    /// The session the report was made in: its transcript says what was asked.
    pub session: String,
    /// The folder the coordinator works in.
    pub cwd: String,
}

/// A live coordinator pane of the project that reported `Waiting for you`
/// and is not working since. The report alone does not say there is a question
/// (see `coordask`). The newest one when there are several panes.
pub fn coordinator_candidate(dir: &Path, progress: &[Progress]) -> Option<Candidate> {
    let read = |name: &str| std::fs::read_to_string(dir.join(".state").join(name)).ok();
    let live: Vec<LivePane> = read("coordinators.json")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let record: serde_json::Value = read("coordinator.json")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let socket = record["socket"].as_str().unwrap_or("").to_string();
    let cwd = record["cwd"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| dir.display().to_string());
    live.iter()
        .filter(|p| p.agent_status != "working")
        .filter_map(|p| {
            let r = progress.iter().find(|r| {
                r.pane_id == p.pane_id
                    && (socket.is_empty() || r.socket == socket)
                    && r.reported_at > 0
                    && r.activity == WAITING
            })?;
            Some(Candidate {
                since: r.reported_at as u64 * 1000,
                agent: p.agent.clone(),
                session: p.agent_session.clone(),
                cwd: cwd.clone(),
            })
        })
        .max_by_key(|c| c.since)
}

/// What is known about the coordinator's wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coordinator {
    /// Not waiting, or the last answer asks nothing.
    No,
    /// An open question: the wait, its text the line that says what it asks.
    Asks(Wait),
    /// Its status says it waits but the question was not recognized (or is still being checked): quiet.
    Unclear(String),
}

/// Fixes the project's counts. `threads` is `thread list` when it could be
/// read, `None` when it could not (herdr-projects' own counts then stay,
/// reason unknown). Sets `counts.needs` to the real waits and `counts.review`
/// to the threads that are really to review, counts the quiet ones apart
/// (`can_close`, `forgotten`), and records the waits and the time of the newest
/// event that needs a look.
pub fn apply(p: &mut Project, coordinator: Coordinator, threads: Option<&[Thread]>, env: &Env) {
    let mut waits: Vec<Wait> = Vec::new();
    p.coordinator_note.clear();
    match coordinator {
        Coordinator::No => {}
        Coordinator::Asks(w) => waits.push(w),
        Coordinator::Unclear(note) => p.coordinator_note = note,
    }
    let mut since = waits.iter().map(|w| w.since).max().unwrap_or(0);
    p.can_close = 0;
    p.forgotten = 0;
    match threads {
        Some(list) => {
            let mut review = 0;
            for t in list {
                match judge(t, env) {
                    Verdict::Untouched | Verdict::Seen => {}
                    Verdict::Wait(w) => {
                        since = since.max(w.since);
                        waits.push(w);
                    }
                    Verdict::Review(s) => {
                        since = since.max(s);
                        review += 1;
                    }
                    Verdict::CanClose => p.can_close += 1,
                    Verdict::Forgotten => p.forgotten += 1,
                }
            }
            p.counts.review = review;
        }
        None => {
            for _ in 0..p.counts.needs {
                waits.push(Wait {
                    kind: "unknown".into(),
                    thread: String::new(),
                    text: "треды ждут (причина не прочитана)".into(),
                    since: 0,
                });
            }
        }
    }
    p.counts.needs = waits.len() as u32;
    p.waits = waits;
    p.attention_since = if p.counts.needs > 0 || p.counts.review > 0 {
        since
    } else {
        0
    };
}

/// The same decision on the card: a thread judged finished is shown as
/// «ревью», not «ждёт тебя»; one whose request is merged «можно закрыть»; an old one «забыт».
pub fn normalize(t: &mut Thread, env: &Env) {
    match judge(t, env) {
        Verdict::Review(_) | Verdict::Seen => t.group = "Ready for review".into(),
        Verdict::CanClose => t.group = "Can close".into(),
        Verdict::Forgotten => t.group = "Forgotten".into(),
        Verdict::Wait(_) | Verdict::Untouched => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_791_990_000_000; // 2026-10-14 ~15:40 UTC

    fn env(mr: &HashMap<String, MrState>, stale_days: u64) -> Env<'_> {
        Env {
            now: NOW,
            stale_ms: stale_days * 86_400_000,
            mr,
        }
    }

    fn th(id: &str, group: &str, note: &str) -> Thread {
        Thread {
            id: id.into(),
            group: group.into(),
            note: note.into(),
            status: "open".into(),
            launched_at: "2026-10-14T10:00:00Z".into(),
            ..Default::default()
        }
    }

    /// A finished thread whose report arrived `h` hours before NOW.
    fn finished(id: &str, group: &str, h: u64) -> Thread {
        let mut t = th(id, group, "done");
        t.last_state = "done".into();
        t.report_hash = "h1".into();
        let at = iso(NOW - h * 3_600_000);
        t.last_report_change = at.clone();
        t.last_state_change = at;
        t
    }

    fn iso(ms: u64) -> String {
        // The inverse of `iso_ms` for the tests: days since the epoch to a civil date.
        let secs = (ms / 1000) as i64;
        let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    }

    fn kind(v: Verdict) -> String {
        match v {
            Verdict::Wait(w) => w.kind,
            Verdict::Review(_) => "review".into(),
            Verdict::CanClose => "can_close".into(),
            Verdict::Forgotten => "forgotten".into(),
            Verdict::Seen => "seen".into(),
            Verdict::Untouched => "untouched".into(),
        }
    }

    #[test]
    fn parses_iso_times() {
        assert_eq!(iso_ms("1970-01-01T00:00:01Z"), 1000);
        assert_eq!(iso_ms("2026-10-14T12:38:16Z"), 1_791_981_496_000);
        assert_eq!(iso_ms("2024-02-29T00:00:00Z"), 1_709_164_800_000);
        assert_eq!(iso_ms(""), 0);
        assert_eq!(iso_ms("nonsense text here!!"), 0);
        assert_eq!(iso_ms(&iso(NOW)), NOW / 1000 * 1000);
    }

    #[test]
    fn a_done_thread_that_left_waiting_is_a_review() {
        let none = HashMap::new();
        let mut t = finished("t-0231", "Waiting on you", 1);
        t.activity = WAITING.into();
        assert!(matches!(judge(&t, &env(&none, 3)), Verdict::Review(_)));
        normalize(&mut t, &env(&none, 3));
        assert_eq!(t.group, "Ready for review");
    }

    #[test]
    fn an_unfinished_thread_that_asks_is_a_wait() {
        let none = HashMap::new();
        let mut t = th("t-0007", "Waiting on you", "idle");
        t.last_state = "idle".into();
        t.activity = WAITING.into();
        let Verdict::Wait(w) = judge(&t, &env(&none, 3)) else {
            panic!("a question is a wait")
        };
        assert_eq!(
            (w.kind.as_str(), w.text.as_str()),
            ("asking", "вопрос в t-0007")
        );
    }

    #[test]
    fn prompts_and_breakage_wait() {
        let none = HashMap::new();
        let e = env(&none, 3);
        let blocked = th("t-1", "Waiting on you", "blocked");
        assert_eq!(kind(judge(&blocked, &e)), "blocked");
        let remote = th("t-1", "Waiting on you", "blocked, on box");
        assert_eq!(kind(judge(&remote, &e)), "blocked");
        let mut failed = th("t-2", "Waiting on you", "failed: x");
        failed.status = "failed".into();
        assert_eq!(kind(judge(&failed, &e)), "failed");
        let mut no_launch = th("t-3", "Waiting on you", "no agent");
        no_launch.error = "boom".into();
        no_launch.launched_at.clear();
        assert_eq!(kind(judge(&no_launch, &e)), "failed");
        let mut stuck = th("t-4", "Waiting on you", "idle");
        stuck.brief_stuck = true;
        assert_eq!(kind(judge(&stuck, &e)), "stuck");
        let mut dialog = th("t-5", "Waiting on you", "unknown");
        dialog.prompt_pending = true;
        assert_eq!(kind(judge(&dialog, &e)), "stuck");
        let mut starting = th("t-6", "Waiting on you", "no agent");
        starting.status = "starting".into();
        assert_eq!(kind(judge(&starting, &e)), "stuck");
        let lost = th("t-7", "Waiting on you", "pane closed");
        assert_eq!(kind(judge(&lost, &e)), "lost");
    }

    #[test]
    fn other_groups_are_left_alone_and_unknown_reasons_never_ring() {
        let none = HashMap::new();
        let e = env(&none, 3);
        assert_eq!(
            kind(judge(&th("t-1", "Working", "blocked"), &e)),
            "untouched"
        );
        let mut closed = th("t-1", "Resolved", "manual");
        closed.status = "resolved".into();
        assert_eq!(kind(judge(&closed, &e)), "untouched");
        // A reason herdr-projects waits for that is not known: a review at most.
        assert_eq!(
            kind(judge(&finished("t-1", "Waiting on you", 1), &e)),
            "review"
        );
    }

    #[test]
    fn review_means_a_fresh_unread_report_with_no_merged_request() {
        let mut mr = HashMap::new();
        let e = env(&mr, 3);
        // Fresh and unread: review.
        assert_eq!(
            kind(judge(&finished("t-1", "Ready for review", 5), &e)),
            "review"
        );
        // Looked at (acked): not review.
        let mut seen = finished("t-2", "Ready for review", 5);
        seen.acked_report_hash = "h1".into();
        assert_eq!(kind(judge(&seen, &e)), "seen");
        // A new report after the ack is review again.
        seen.report_hash = "h2".into();
        assert_eq!(kind(judge(&seen, &e)), "review");
        // Older than the cut-off: forgotten; with the cut-off off it is review.
        let old = finished("t-3", "Ready for review", 24 * 4);
        assert_eq!(kind(judge(&old, &e)), "forgotten");
        assert_eq!(kind(judge(&old, &env(&mr, 0))), "review");
        // A merged or closed GitHub pull request: can be closed, old or not.
        let mut gh = finished("t-4", "Ready for review", 5);
        gh.pr_state = "MERGED".into();
        assert_eq!(kind(judge(&gh, &e)), "can_close");
        gh.pr_state = "OPEN".into();
        assert_eq!(kind(judge(&gh, &e)), "review");
        // A merged GitLab MR from the report, even on an old thread; an open one is no reason.
        mr.insert("t-5".to_string(), MrState::Merged);
        mr.insert("t-6".to_string(), MrState::Open);
        mr.insert("t-7".to_string(), MrState::Unknown);
        let e = env(&mr, 3);
        assert_eq!(
            kind(judge(&finished("t-5", "Ready for review", 24 * 9), &e)),
            "can_close"
        );
        assert_eq!(
            kind(judge(&finished("t-6", "Ready for review", 5), &e)),
            "review"
        );
        // glab unavailable: not merged, and the age cut-off still works.
        assert_eq!(
            kind(judge(&finished("t-7", "Ready for review", 5), &e)),
            "review"
        );
        assert_eq!(
            kind(judge(&finished("t-7", "Ready for review", 24 * 9), &e)),
            "forgotten"
        );
        // On the card they are shown for what they are.
        let mut t = finished("t-5", "Ready for review", 24 * 9);
        normalize(&mut t, &e);
        assert_eq!(t.group, "Can close");
        let mut t = finished("t-8", "Ready for review", 24 * 9);
        normalize(&mut t, &e);
        assert_eq!(t.group, "Forgotten");
    }

    fn proj(needs: u32, review: u32) -> Project {
        let mut p = Project {
            slug: "x".into(),
            ..Default::default()
        };
        p.counts.needs = needs;
        p.counts.review = review;
        p
    }

    fn asks(since: u64) -> Coordinator {
        Coordinator::Asks(Wait {
            kind: "coordinator".into(),
            thread: String::new(),
            text: "Запускать тред?".into(),
            since,
        })
    }

    #[test]
    fn apply_rebuilds_counts_and_the_newest_event() {
        let none = HashMap::new();
        let e = env(&none, 3);
        let mut done = finished("t-1", "Waiting on you", 2);
        done.activity = WAITING.into();
        let review = finished("t-2", "Ready for review", 3);
        let mut blocked = th("t-3", "Waiting on you", "blocked");
        blocked.last_state_change = iso(NOW - 5 * 3_600_000);
        let working = th("t-4", "Working", "working");
        let old = finished("t-5", "Ready for review", 24 * 6);
        let mut merged = finished("t-6", "Ready for review", 1);
        merged.pr_state = "merged".into();

        let mut p = proj(2, 5);
        apply(
            &mut p,
            Coordinator::No,
            Some(&[done.clone(), review, blocked, working, old, merged]),
            &e,
        );
        assert_eq!((p.counts.needs, p.counts.review), (1, 2));
        assert_eq!((p.can_close, p.forgotten), (1, 1));
        assert_eq!(p.waits.len(), 1);
        assert_eq!(p.waits[0].thread, "t-3");
        assert_eq!(p.attention_since, iso_ms(&iso(NOW - 2 * 3_600_000)));

        // The coordinator's question adds one wait and may be the newest event.
        let mut p = proj(1, 0);
        apply(&mut p, asks(NOW), Some(&[done.clone()]), &e);
        assert_eq!((p.counts.needs, p.counts.review), (1, 1));
        assert_eq!(p.waits[0].kind, "coordinator");
        assert_eq!(p.attention_since, NOW);

        // A coordinator whose status waits but who asks nothing: a quiet note, no bell.
        let mut p = proj(0, 0);
        apply(
            &mut p,
            Coordinator::Unclear("координатор: статус ждёт, вопрос не распознан".into()),
            Some(&[]),
            &e,
        );
        assert_eq!((p.counts.needs, p.attention_since), (0, 0));
        assert_eq!(
            p.coordinator_note,
            "координатор: статус ждёт, вопрос не распознан"
        );
        apply(&mut p, Coordinator::No, Some(&[]), &e);
        assert_eq!(p.coordinator_note, "");

        // Threads unreadable: herdr-projects' counts stay, reason unknown.
        let mut p = proj(2, 3);
        apply(&mut p, asks(NOW), None, &e);
        assert_eq!((p.counts.needs, p.counts.review), (3, 3));
        assert_eq!(p.waits.last().map(|w| w.kind.as_str()), Some("unknown"));
    }

    #[test]
    fn the_coordinator_is_a_candidate_only_when_live_reported_and_not_working() {
        let dir = std::env::temp_dir().join(format!("herdr-planner-att-{}", std::process::id()));
        let state = dir.join(".state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("coordinator.json"),
            r#"{"socket":"/s","cwd":"/work"}"#,
        )
        .unwrap();
        let rec = |pane: &str, activity: &str, at: i64, socket: &str| Progress {
            socket: socket.into(),
            pane_id: pane.into(),
            activity: activity.into(),
            reported_at: at,
        };
        let live = |status: &str| {
            std::fs::write(
                state.join("coordinators.json"),
                format!(r#"[{{"pane_id":"w1:p1","agent_status":"{status}","agent":"claude","agent_session":"sid"}}]"#),
            )
            .unwrap();
        };
        live("idle");
        let waiting = [rec("w1:p1", WAITING, 100, "/s")];
        let c = coordinator_candidate(&dir, &waiting).unwrap();
        assert_eq!(
            (
                c.since,
                c.agent.as_str(),
                c.session.as_str(),
                c.cwd.as_str()
            ),
            (100_000, "claude", "sid", "/work")
        );
        // Not the coordinator's pane, another session's socket, answered, other activity.
        assert!(coordinator_candidate(&dir, &[rec("w9:p1", WAITING, 100, "/s")]).is_none());
        assert!(coordinator_candidate(&dir, &[rec("w1:p1", WAITING, 100, "/other")]).is_none());
        assert!(coordinator_candidate(&dir, &[rec("w1:p1", WAITING, 0, "/s")]).is_none());
        assert!(coordinator_candidate(&dir, &[rec("w1:p1", "Done", 100, "/s")]).is_none());
        // Working since: the question was answered.
        live("working");
        assert!(coordinator_candidate(&dir, &waiting).is_none());
        // Pane gone.
        std::fs::write(state.join("coordinators.json"), "[]").unwrap();
        assert!(coordinator_candidate(&dir, &waiting).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
