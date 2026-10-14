//! GitLab merge requests of finished threads. herdr-projects watches only GitHub
//! pull requests (`pr` / `pr_state`), so a merged MR leaves its thread «in
//! review» for ever. The MR link is in the thread's report (`MR: https://…/-/merge_requests/N`,
//! `threads/<id>.md`); its state is read with `glab api` (read-only), kept
//! for a few minutes and asked only for threads that are in review. Without
//! `glab`, a login or the network the state is unknown, and unknown counts as not merged.

use crate::app::{now_ms, Shared};
use crate::sources::runner;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MrState {
    Open,
    Merged,
    Closed,
    Unknown,
}

impl MrState {
    /// Nothing is left to review in it.
    pub fn done(self) -> bool {
        matches!(self, MrState::Merged | MrState::Closed)
    }
}

/// One merge request: where it lives and which one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MrRef {
    pub host: String,
    /// `group/project`, as in the link.
    pub path: String,
    pub iid: u64,
    pub url: String,
}

/// The first merge request link in a report. A line that starts with `MR:` or `PR:` wins
/// over a link in the middle of a sentence.
pub fn find_mr(report: &str) -> Option<MrRef> {
    let parse = |word: &str| -> Option<MrRef> {
        let w = word.trim_matches(|c: char| {
            matches!(
                c,
                '(' | ')' | '<' | '>' | ',' | ';' | '.' | '`' | '*' | '"' | '\''
            )
        });
        let rest = w
            .strip_prefix("https://")
            .or_else(|| w.strip_prefix("http://"))?;
        let (host, tail) = rest.split_once('/')?;
        let (path, after) = tail.split_once("/-/merge_requests/")?;
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        let iid = digits.parse().ok()?;
        (!host.is_empty() && !path.is_empty()).then(|| MrRef {
            host: host.to_string(),
            path: path.to_string(),
            iid,
            url: format!("https://{host}/{path}/-/merge_requests/{iid}"),
        })
    };
    let first = report.lines().take(40).find_map(|l| {
        let l = l.trim_start();
        (l.starts_with("MR:") || l.starts_with("PR:"))
            .then(|| l.split_whitespace().find_map(parse))
            .flatten()
    });
    first.or_else(|| report.split_whitespace().take(4000).find_map(parse))
}

/// `group/project` as one path segment for the API.
pub fn encode_path(path: &str) -> String {
    path.replace('%', "%25").replace('/', "%2F")
}

pub fn parse_state(json: &str) -> MrState {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    match v["state"].as_str() {
        Some("merged") => MrState::Merged,
        Some("closed") => MrState::Closed,
        Some("opened") | Some("locked") => MrState::Open,
        _ => MrState::Unknown,
    }
}

/// What the last asks said, and when.
#[derive(Debug, Default)]
pub struct MrCache {
    seen: HashMap<String, (MrState, u64)>,
    running: std::collections::HashSet<String>,
}

/// How long an answer that may still change stays. A merged or closed MR does not change.
const OPEN_TTL_MS: u64 = 5 * 60_000;
const UNKNOWN_TTL_MS: u64 = 10 * 60_000;

impl MrCache {
    /// The cached state when it is still good; `None` when it has to be asked.
    fn fresh(&self, url: &str, now: u64) -> Option<MrState> {
        let (state, at) = self.seen.get(url)?;
        let ttl = match state {
            MrState::Merged | MrState::Closed => u64::MAX,
            MrState::Open => OPEN_TTL_MS,
            MrState::Unknown => UNKNOWN_TTL_MS,
        };
        (now.saturating_sub(*at) < ttl).then_some(*state)
    }

    #[cfg(test)]
    pub fn put(&mut self, url: &str, state: MrState, at: u64) {
        self.seen.insert(url.to_string(), (state, at));
    }
}

fn glab_bin() -> String {
    std::env::var("HERDR_PLANNER_GLAB").unwrap_or_else(|_| "glab".into())
}

async fn fetch(mr: &MrRef) -> MrState {
    let argv = vec![
        glab_bin(),
        "api".into(),
        "--hostname".into(),
        mr.host.clone(),
        format!(
            "projects/{}/merge_requests/{}",
            encode_path(&mr.path),
            mr.iid
        ),
    ];
    match runner::run(&argv, None, Duration::from_secs(20)).await {
        Ok(out) if out.ok() => parse_state(&out.stdout),
        _ => MrState::Unknown,
    }
}

/// The state of `mr` as far as it is known now. A stale or missing answer starts
/// a background ask (one per link) that wakes the refresh when it is back.
pub fn state(app: &Shared, mr: &MrRef) -> MrState {
    let now = now_ms();
    let known = {
        let mut c = app.mrs.lock().unwrap();
        match c.fresh(&mr.url, now) {
            Some(s) => return s,
            None => {
                if !c.running.insert(mr.url.clone()) {
                    return c.seen.get(&mr.url).map_or(MrState::Unknown, |(s, _)| *s);
                }
                c.seen.get(&mr.url).map_or(MrState::Unknown, |(s, _)| *s)
            }
        }
    };
    let (app, mr) = (app.clone(), mr.clone());
    tokio::spawn(async move {
        let _slot = app.git_gate.acquire().await;
        let state = fetch(&mr).await;
        {
            let mut c = app.mrs.lock().unwrap();
            c.running.remove(&mr.url);
            c.seen.insert(mr.url.clone(), (state, now_ms()));
        }
        app.refresh_now.notify_one();
    });
    known
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_merge_request_in_a_report() {
        let r = "PR: https://gitlab.example.com/jdoe/apicache/-/merge_requests/131\n\n## Report\n\nMR: https://gitlab.example.com/jdoe/apicache/-/merge_requests/131\nPipeline: https://gitlab.example.com/jdoe/apicache/-/pipelines/25234\n";
        let mr = find_mr(r).unwrap();
        assert_eq!(
            (mr.host.as_str(), mr.path.as_str(), mr.iid),
            ("gitlab.example.com", "jdoe/apicache", 131)
        );
        assert_eq!(
            mr.url,
            "https://gitlab.example.com/jdoe/apicache/-/merge_requests/131"
        );
        // In a sentence, in brackets, with a trailing dot; the labelled line wins.
        let r = "Сделан MR5 (https://g.example/a/b/-/merge_requests/6).\nMR: https://g.example/a/c/-/merge_requests/9\n";
        assert_eq!(find_mr(r).unwrap().iid, 9);
        let r = "Сделан MR5 (https://g.example/a/b/-/merge_requests/6).";
        assert_eq!(find_mr(r).unwrap().path, "a/b");
        // A GitHub pull request or nothing is not a merge request.
        assert_eq!(find_mr("PR: https://github.com/o/r/pull/7\n"), None);
        assert_eq!(find_mr(""), None);
    }

    #[test]
    fn reads_the_state_and_encodes_the_project() {
        assert_eq!(
            parse_state(r#"{"state":"merged","merged_at":"x"}"#),
            MrState::Merged
        );
        assert_eq!(parse_state(r#"{"state":"closed"}"#), MrState::Closed);
        assert_eq!(parse_state(r#"{"state":"opened"}"#), MrState::Open);
        assert_eq!(
            parse_state(r#"{"message":"404 Not Found"}"#),
            MrState::Unknown
        );
        assert_eq!(parse_state("not json"), MrState::Unknown);
        assert_eq!(
            encode_path("jdoe/gigabit_server"),
            "jdoe%2Fgigabit_server"
        );
        assert!(MrState::Merged.done() && MrState::Closed.done());
        assert!(!MrState::Open.done() && !MrState::Unknown.done());
    }

    #[test]
    fn answers_live_for_minutes_and_a_merge_for_good() {
        let mut c = MrCache::default();
        c.put("a", MrState::Open, 1000);
        c.put("b", MrState::Merged, 1000);
        c.put("c", MrState::Unknown, 1000);
        assert_eq!(c.fresh("a", 1000 + 4 * 60_000), Some(MrState::Open));
        assert_eq!(c.fresh("a", 1000 + 6 * 60_000), None);
        assert_eq!(c.fresh("b", 1000 + 99 * 3_600_000), Some(MrState::Merged));
        assert_eq!(c.fresh("c", 1000 + 11 * 60_000), None);
        assert_eq!(c.fresh("none", 1000), None);
    }
}
