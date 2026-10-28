//! The herdr-projects board, as `herdr-projects list --all --json` and plain
//! `list --all` print it, and the eight statuses the dashboard shows.
//!
//! The board owns the real stage of a task. This module only turns its words
//! (`state`, `stage`, `kind`, `review`) into the dashboard's status, which is
//! the same for a card, a badge, a thread and a test.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Waiting,
    Failed,
    Ready,
    Plan,
    Review,
    Working,
    Queued,
    Done,
}

impl Status {
    pub fn key(self) -> &'static str {
        match self {
            Status::Waiting => "waiting",
            Status::Failed => "failed",
            Status::Ready => "ready",
            Status::Plan => "plan",
            Status::Review => "review",
            Status::Working => "working",
            Status::Queued => "queued",
            Status::Done => "done",
        }
    }

    /// The badge's words, exactly as the mockup has them.
    pub fn label(self) -> &'static str {
        match self {
            Status::Waiting => "Ждёт тебя",
            Status::Failed => "Упала",
            Status::Ready => "К одобрению",
            Status::Plan => "План готов",
            Status::Review => "На ревью",
            Status::Working => "В работе",
            Status::Queued => "В очереди",
            Status::Done => "Сделано",
        }
    }

    pub fn urgency(self) -> u8 {
        match self {
            Status::Waiting => 0,
            Status::Failed => 1,
            Status::Ready => 2,
            Status::Plan => 3,
            Status::Review => 4,
            Status::Working => 5,
            Status::Queued => 6,
            Status::Done => 7,
        }
    }

    pub fn needs_you(self) -> bool {
        matches!(
            self,
            Status::Waiting | Status::Failed | Status::Ready | Status::Plan
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ReviewBrief {
    /// The board's gate said yes; none when a review from before the gate gave no verdict.
    pub pass: Option<bool>,
    pub meets: Option<bool>,
    /// `1 critical, 2 major`.
    pub tally: String,
    pub text: String,
    pub checks: Vec<String>,
    pub by: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Task {
    pub id: String,
    pub repo: String,
    pub n: u64,
    /// `PLN-3`, from plain `list`.
    pub tag: String,
    pub title: String,
    pub state: String,
    pub kind: String,
    pub stage: String,
    /// The board's current line (its `msg`).
    pub msg: String,
    pub app: String,
    pub progress: Option<u32>,
    pub eta: Option<u32>,
    pub pr: String,
    pub review: Option<ReviewBrief>,
    pub round: u32,
    pub queue: Option<u32>,
    pub status: Status,
    /// The dashboard's badge: «Ждёт тебя», «К одобрению», …
    pub label: String,
    /// The board's own words: «ждёт ответа», «в очереди, №2».
    pub board_label: String,
    /// Finished work whose review failed (the board may still let it through with `--anyway`).
    pub review_failed: bool,
    /// Finished work the board will not merge by a plain yes.
    pub merge_blocked: bool,
    pub archived: bool,
    /// The board commands the page may offer.
    pub actions: Vec<&'static str>,
    /// From the task's own record (`taskmeta`): when it was made (ms), its text,
    /// the agent and model, the session, the checkout and when the agent left.
    pub created: u64,
    pub description: String,
    pub worker_agent: String,
    pub model: String,
    pub session: String,
    pub worktree: String,
    pub exited_at: u64,
    /// When the task's work left the machine for good (ms), 0 while it may still be there: the agent exited, or the
    /// task is archived, or it is done and the board saw its pane gone (`exitedAt` alone is set only when the pane is
    /// alive and the agent is gone, which is the rarer case on a real board).
    #[serde(skip)]
    pub finished_ms: u64,
    /// The project's key (`PLN`).
    pub key: String,
    /// Sub-agents of the task's agent, newest first.
    pub subagents: Vec<crate::subagents::Subagent>,
    /// Why there is no sub-agent data, when that is the honest answer.
    pub subagents_note: String,
    /// Who is on the task now (the reviewer under review, the working agent while it works); see `seat`.
    pub seat: Option<crate::seat::Seat>,
}

impl Default for Status {
    fn default() -> Self {
        Status::Queued
    }
}

pub fn repo_of(id: &str) -> &str {
    id.rsplit_once('/').map_or(id, |(repo, _)| repo)
}

pub fn number_of(id: &str) -> u64 {
    id.rsplit_once('/')
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(0)
}

/// A board task id the planner may pass on to a command: no flags, no spaces.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id.len() <= 200
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | ':' | '@'))
}

/// A task tag the page may put in `data-*` and a URL (`PLN-3`); ids are separate.
pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 40
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn num(v: &Value, key: &str) -> Option<u32> {
    v.get(key)
        .and_then(Value::as_f64)
        .filter(|n| *n >= 0.0)
        .map(|n| n.round() as u32)
}

fn is_archived(v: &Value) -> bool {
    v.get("archivedAt").map(|x| !x.is_null()).unwrap_or(false)
}

fn tagged(rv: &Value) -> bool {
    rv.get("findings").is_some_and(Value::is_array)
        && rv.get("meets").is_some_and(Value::is_boolean)
}

fn tally(rv: &Value) -> String {
    let mut counts = [0u32; 4];
    for f in rv
        .get("findings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if f.get("preexisting").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let at = match s(f, "severity") {
            "critical" => 0,
            "major" => 1,
            "minor" => 2,
            "trivial" => 3,
            _ => 1,
        };
        counts[at] += 1;
    }
    ["critical", "major", "minor", "trivial"]
        .iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(w, n)| format!("{n} {w}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn review_brief(rv: &Value) -> Option<ReviewBrief> {
    if !rv.is_object() {
        return None;
    }
    let text = s(rv, "text").to_string();
    let checks = rv
        .get("checks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect();
    let by = s(rv, "by").to_string();
    if tagged(rv) {
        return Some(ReviewBrief {
            pass: rv.get("pass").and_then(Value::as_bool),
            meets: rv.get("meets").and_then(Value::as_bool),
            tally: tally(rv),
            text,
            checks,
            by,
        });
    }
    let recommend = rv.get("recommend").and_then(Value::as_bool);
    let score = rv.get("score").and_then(Value::as_i64);
    Some(ReviewBrief {
        pass: recommend,
        meets: recommend,
        tally: score.map(|n| format!("{n}/5")).unwrap_or_default(),
        text,
        checks,
        by,
    })
}

pub fn says_merge_blocked(msg: &str) -> bool {
    let m = msg.to_lowercase();
    [
        "checks fail",
        "checks did not pass",
        "merge fail",
        "conflict",
        "behind",
        "cannot merge",
        "could not merge",
        "did not go through",
        "not up to date",
    ]
    .iter()
    .any(|w| m.contains(w))
}

/// The board's own, more specific words for a task.
pub fn board_label(t: &Task) -> String {
    let state = t.state.as_str();
    let stage = t.stage.as_str();
    if state == "done" {
        return "сделана".into();
    }
    if state == "error" || stage == "failed" || stage == "nosummary" {
        return if stage == "nosummary" {
            "остановилась без итога".into()
        } else {
            "упала".into()
        };
    }
    if stage == "pick" {
        return "выбери вариант".into();
    }
    if stage == "stopped" {
        return if state == "blocked" {
            "остановлена на лимите".into()
        } else {
            "остановлена".into()
        };
    }
    if state == "blocked" {
        if stage == "reviewneeds" {
            return "ревьюеру нужен ты".into();
        }
        if stage == "plan" {
            return "план готов".into();
        }
        if stage == "approval" {
            if t.review_failed {
                return "ревью не прошло".into();
            }
            if t.merge_blocked {
                return "нельзя влить".into();
            }
            if t.kind == "permission" {
                return "готова к одобрению".into();
            }
        }
        return match t.kind.as_str() {
            "question" => "ждёт ответа".into(),
            "permission" => "ждёт разрешения".into(),
            "auth" => "нужен вход".into(),
            _ => "ждёт тебя".into(),
        };
    }
    if state == "working" {
        return match stage {
            "queued" => match t.queue {
                Some(n) if n > 0 => format!("в очереди, №{n}"),
                _ => "в очереди".into(),
            },
            "after" => "ждёт другие задачи".into(),
            "reviewing" => "на ревью".into(),
            "starting" => "запускается".into(),
            "setup" => "готовит окружение".into(),
            "checking" => "проверка".into(),
            "pushing" => "отправляет".into(),
            "merging" => "вливается".into(),
            _ => "в работе".into(),
        };
    }
    "не начата".into()
}

/// The dashboard status for a parsed task.
pub fn classify(t: &Task) -> Status {
    let state = t.state.as_str();
    let stage = t.stage.as_str();
    if state == "done" {
        return Status::Done;
    }
    if state == "error" || stage == "failed" || stage == "nosummary" {
        return Status::Failed;
    }
    if stage == "pick" {
        return Status::Waiting;
    }
    if stage == "stopped" {
        return if state == "blocked" {
            Status::Waiting
        } else {
            Status::Queued
        };
    }
    if state == "blocked" {
        if stage == "reviewneeds" {
            return Status::Waiting;
        }
        if stage == "plan" {
            return Status::Plan;
        }
        if stage == "approval" {
            if t.review_failed || t.merge_blocked {
                return Status::Waiting;
            }
            if t.kind == "permission" {
                return Status::Ready;
            }
        }
        return Status::Waiting;
    }
    if state == "working" {
        return match stage {
            "queued" | "after" => Status::Queued,
            "reviewing" => Status::Review,
            _ => Status::Working,
        };
    }
    // `idle`, `todo`, a stop by hand, or no state at all: nobody is on it.
    Status::Queued
}

fn actions_for(t: &Task) -> Vec<&'static str> {
    match t.status {
        Status::Waiting if t.stage == "pick" => vec![],
        Status::Waiting if t.stage == "stopped" => vec!["retry", "tell"],
        Status::Waiting if t.review_failed => vec!["review", "anyway", "tell"],
        Status::Waiting if t.merge_blocked => vec!["anyway", "review", "tell"],
        Status::Waiting => vec!["answer", "tell", "stop"],
        Status::Failed => vec!["retry", "tell", "stop"],
        Status::Ready => vec!["approve", "anyway", "review", "tell"],
        Status::Plan => vec!["approve", "tell"],
        Status::Review | Status::Working => vec!["tell", "stop"],
        Status::Queued => vec!["delegate", "tell"],
        Status::Done => vec!["archive"],
    }
}

/// One row of `list --json`; none for something that is not a task or is an attempt.
pub fn parse_task(v: &Value) -> Option<Task> {
    let id = s(v, "id");
    if id.is_empty() || !id.contains('/') || !valid_id(id) {
        return None;
    }
    let stage = s(v, "stage");
    if stage == "tried" {
        return None;
    }
    let review = v.get("review").and_then(review_brief);
    let mut t = Task {
        id: id.to_string(),
        repo: repo_of(id).to_string(),
        n: number_of(id),
        tag: format!("#{}", number_of(id)),
        title: s(v, "title").to_string(),
        state: s(v, "state").to_string(),
        kind: s(v, "kind").to_string(),
        stage: stage.to_string(),
        msg: s(v, "msg").to_string(),
        app: s(v, "app").to_string(),
        progress: num(v, "progress").map(|n| n.min(100)),
        eta: num(v, "eta"),
        pr: match v.get("pr") {
            Some(Value::String(x)) => x.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        },
        queue: num(v, "queue"),
        round: num(v, "reviewRound").unwrap_or(0),
        archived: is_archived(v),
        review,
        ..Task::default()
    };
    t.review_failed = t.stage == "approval"
        && t.review
            .as_ref()
            .is_some_and(|r| r.pass == Some(false) || r.meets == Some(false));
    t.merge_blocked = t.stage == "approval" && !t.review_failed && says_merge_blocked(&t.msg);
    t.status = classify(&t);
    t.label = t.status.label().to_string();
    t.board_label = board_label(&t);
    t.actions = actions_for(&t);
    Some(t)
}

/// Parses `list --all --json`; a non-list or broken JSON is an error.
pub fn parse_list(stdout: &str) -> Result<Vec<Task>, String> {
    let v: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("herdr-projects list --all --json: {e}"))?;
    let Some(rows) = v.as_array() else {
        return Err("herdr-projects list --all --json: not a list".into());
    };
    Ok(rows.iter().filter_map(parse_task).collect())
}

/// The first word of each line of plain `list --all`: the task's tag (`PLN-1`).
pub fn plain_tags(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| {
            let first = l.split_whitespace().next()?;
            if first == "(archived)" {
                None
            } else {
                Some(first.to_string())
            }
        })
        .collect()
}

/// Gives each task its tag from plain `list --all`, which prints the same rows in the same order.
pub fn apply_tags(tasks: &mut [Task], tags: &[String]) {
    if tags.len() != tasks.len() {
        return;
    }
    for (t, tag) in tasks.iter_mut().zip(tags) {
        if valid_tag(tag) && tag.rsplit(['-', '#']).next() == Some(t.n.to_string().as_str()) {
            t.tag = tag.clone();
        }
    }
}

/// One `after` dependency: `to` waits for `from`. A `far` link names an archived task
/// (or a tag this board cannot resolve) in `from`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Link {
    pub from: String,
    pub to: String,
    /// `waiting` (the task is not done yet), `done` (it is), `far` (an archived task /
    /// an unknown tag).
    pub kind: String,
    /// How to name `from` in the world: a tag for an archived task, the reference as written for an unknown one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

/// `herdr-projects after ID` prints `ID: waits for TAG (done), TAG`, or
/// `ID: is to wait for TAG`, or `ID: waits for nothing`. Returns the tags it
/// names, without the `(done)` markers.
pub fn parse_after(stdout: &str) -> Vec<String> {
    let line = stdout.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut rest = if let Some(at) = line.find(" is to wait for ") {
        &line[at + " is to wait for ".len()..]
    } else if let Some(at) = line.find(" waits for ") {
        &line[at + " waits for ".len()..]
    } else {
        return Vec::new();
    };
    for cut in [
        "; it starts by itself",
        ", from when it is delegated",
        ", so it starts now",
        ". It waits for you:",
        " and starts by itself",
    ] {
        if let Some(at) = rest.find(cut) {
            rest = &rest[..at];
        }
    }
    let rest = rest.trim().trim_end_matches('.');
    if rest.is_empty() || rest == "nothing" || rest.starts_with("nothing ") {
        return Vec::new();
    }
    rest.split(',')
        .map(|item| {
            item.trim()
                .strip_suffix(" (done)")
                .unwrap_or(item.trim())
                .trim()
                .to_string()
        })
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// A task id, or a reference the board resolves on its own (`vm-str:ELA-3`, set with its CLI), that `after`
/// may be given. Tags are not accepted here: the gesture and the API speak ids.
pub fn valid_ref(r: &str) -> bool {
    if valid_id(r) && r.contains('/') {
        return true;
    }
    let Some((label, rest)) = r.split_once(':') else {
        return false;
    };
    !label.is_empty()
        && label.len() <= 64
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !rest.is_empty()
        && rest.len() <= 200
        && rest
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '@' | '#'))
}

/// A task's attempts letter, as `herdr-projects pick` takes it.
pub fn valid_letter(letter: &str) -> bool {
    letter.len() == 1 && letter.chars().all(|c| c.is_ascii_alphabetic())
}

/// Turns what `after ID` said into the world's links. `tasks` may hold the
/// active and archived rows: an archived predecessor is still a done one.
pub fn links_from(after: &BTreeMap<String, Vec<String>>, tasks: &[Task]) -> Vec<Link> {
    let by_tag: BTreeMap<&str, &Task> = tasks
        .iter()
        .filter(|t| valid_tag(&t.tag))
        .map(|t| (t.tag.as_str(), t))
        .collect();
    let mut out: Vec<Link> = Vec::new();
    for (to, tags) in after {
        for tag in tags {
            let (from, kind, label) = match by_tag.get(tag.as_str()) {
                Some(t) if !t.archived => (
                    t.id.clone(),
                    if t.status == Status::Done {
                        "done"
                    } else {
                        "waiting"
                    },
                    t.tag.clone(),
                ),
                // An archived task is still a real id the board accepts: draw it as
                // a far/star node, but keep its id so later link edits can resend it.
                Some(t) => (t.id.clone(), "far", t.tag.clone()),
                None => (tag.clone(), "far", tag.clone()),
            };
            let link = Link {
                from,
                to: to.clone(),
                kind: kind.to_string(),
                label,
            };
            if !out.contains(&link) {
                out.push(link);
            }
        }
    }
    out
}

/// How far a clone's main branch is from its origin.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProjectLag {
    pub origin: String,
    pub main: String,
    pub behind: u32,
    pub ahead: u32,
    pub known: bool,
    pub as_of_ms: u64,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Project {
    pub name: String,
    pub key: String,
    pub active: u32,
    pub done: u32,
    pub total: u32,
    pub statuses: BTreeMap<String, u32>,
    /// No task is being worked on: the board is quiet here.
    pub quiet: bool,
    /// The repository's folder, and the folder it is in (the arm of the Galaxy).
    pub root: String,
    pub folder: String,
    pub origin: String,
    pub lag: Option<ProjectLag>,
    /// A project of the board with no task at all (new, or every task archived).
    pub empty: bool,
}

/// Projects of the visible tasks, with a quiet project for a repo whose every task is done.
pub fn projects(tasks: &[Task]) -> Vec<Project> {
    let mut map: BTreeMap<String, Project> = BTreeMap::new();
    for t in tasks {
        let p = map.entry(t.repo.clone()).or_insert_with(|| Project {
            name: t.repo.clone(),
            ..Project::default()
        });
        p.total += 1;
        *p.statuses.entry(t.status.key().to_string()).or_insert(0) += 1;
        if t.status == Status::Done {
            p.done += 1;
        } else {
            p.active += 1;
        }
        if p.key.is_empty() && valid_tag(&t.tag) && t.tag.contains('-') {
            p.key = t.tag.split('-').next().unwrap_or("").to_string();
        }
    }
    for p in map.values_mut() {
        if p.key.is_empty() {
            p.key = p
                .name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(3)
                .collect::<String>()
                .to_ascii_uppercase();
        }
        p.quiet = p.active == 0;
    }
    map.into_values().collect()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Summary {
    pub projects: u32,
    pub active_projects: u32,
    pub quiet_projects: u32,
    pub tasks: u32,
    pub done: u32,
    pub archived: u32,
    pub attention: u32,
    pub waiting: u32,
    pub failed: u32,
    pub ready: u32,
    pub plan: u32,
    pub review: u32,
    pub working: u32,
    pub queued: u32,
}

pub fn summary(tasks: &[Task], archived: &[Task], projects: &[Project]) -> Summary {
    let mut s = Summary {
        archived: archived.len() as u32,
        ..Summary::default()
    };
    for t in tasks {
        s.tasks += 1;
        match t.status {
            Status::Waiting => s.waiting += 1,
            Status::Failed => s.failed += 1,
            Status::Ready => s.ready += 1,
            Status::Plan => s.plan += 1,
            Status::Review => s.review += 1,
            Status::Working => s.working += 1,
            Status::Queued => s.queued += 1,
            Status::Done => s.done += 1,
        }
    }
    s.attention = s.waiting + s.failed + s.ready + s.plan;
    s.projects = projects.len() as u32;
    s.active_projects = projects.iter().filter(|p| !p.quiet).count() as u32;
    s.quiet_projects = projects.iter().filter(|p| p.quiet).count() as u32;
    s
}

/// `worker.agent` from `herdr-projects show --task ID`: `omp/deepseek/deepseek-v4-flash`.
pub fn show_agent(stdout: &str) -> String {
    for line in stdout.lines() {
        let mut it = line.split_whitespace();
        if it.next() == Some("worker.agent") || line.contains("worker.agent") {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if let Some(i) = fields.iter().position(|f| *f == "worker.agent") {
                return fields.get(i + 1).copied().unwrap_or("").to_string();
            }
        }
    }
    String::new()
}

/// The body of `herdr-projects details ID`, with the few derived paths the page needs.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Details {
    pub id: String,
    pub task: Value,
    pub failure: Value,
    pub review: Value,
    pub reviews: Value,
    pub agent: String,
    pub agent_model: String,
    pub worktree: String,
    pub branch: String,
    pub after: Value,
    pub limits: Value,
    pub plan: Value,
    pub report: Value,
    pub pr: Value,
    /// Attempts of a `pick` task, with what each one came to.
    pub attempts: Value,
    pub comparison: Value,
    pub picked: Value,
    pub reports_dir: String,
}

pub fn parse_details(stdout: &str, reports_dir: &str) -> Result<Details, String> {
    let v: Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("herdr-projects details: {e}"))?;
    if !v.is_object() {
        return Err("herdr-projects details: not an object".into());
    }
    let id = v.get("task").map(|t| s(t, "id")).unwrap_or("").to_string();
    Ok(Details {
        id,
        task: v.get("task").cloned().unwrap_or(Value::Null),
        failure: v.get("failure").cloned().unwrap_or(Value::Null),
        review: v.get("review").cloned().unwrap_or(Value::Null),
        reviews: v.get("reviews").cloned().unwrap_or(Value::Null),
        agent: s(&v, "agent").to_string(),
        agent_model: String::new(),
        worktree: s(&v, "worktree").to_string(),
        branch: s(&v, "branch").to_string(),
        after: v.get("after").cloned().unwrap_or(Value::Null),
        limits: v.get("limits").cloned().unwrap_or(Value::Null),
        plan: v.get("plan").cloned().unwrap_or(Value::Null),
        report: v.get("report").cloned().unwrap_or(Value::Null),
        pr: v.get("pr").cloned().unwrap_or(Value::Null),
        attempts: v.get("attempts").cloned().unwrap_or(Value::Null),
        comparison: v.get("comparison").cloned().unwrap_or(Value::Null),
        picked: v.get("picked").cloned().unwrap_or(Value::Null),
        reports_dir: reports_dir.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(v: Value) -> Task {
        parse_task(&v).expect("a task")
    }

    #[test]
    fn statuses_are_the_mockups_eight() {
        assert_eq!(Status::Waiting.label(), "Ждёт тебя");
        assert_eq!(Status::Failed.label(), "Упала");
        assert_eq!(Status::Ready.label(), "К одобрению");
        assert_eq!(Status::Plan.label(), "План готов");
        assert_eq!(Status::Review.label(), "На ревью");
        assert_eq!(Status::Working.label(), "В работе");
        assert_eq!(Status::Queued.label(), "В очереди");
        assert_eq!(Status::Done.label(), "Сделано");
        assert_eq!(Status::Waiting.urgency(), 0);
        assert_eq!(Status::Done.urgency(), 7);
        assert!(Status::Waiting.needs_you() && Status::Plan.needs_you());
        assert!(!Status::Working.needs_you());
    }

    #[test]
    fn working_review_and_queued() {
        let t = task(
            json!({"id":"p/1","state":"working","stage":"working","title":"T","progress":4,"app":"claude","msg":"Reading"}),
        );
        assert_eq!(
            (t.status, t.board_label.as_str()),
            (Status::Working, "в работе")
        );
        assert_eq!(t.progress, Some(4));
        assert_eq!(t.label, "В работе");
        assert_eq!(t.actions, vec!["tell", "stop"]);

        let t = task(json!({"id":"p/2","state":"working","stage":"reviewing","reviewRound":1}));
        assert_eq!(t.status, Status::Review);
        assert_eq!(t.board_label, "на ревью");
        assert_eq!(t.round, 1);

        let t = task(json!({"id":"p/3","state":"working","stage":"queued","queue":2}));
        assert_eq!(
            (t.status, t.board_label.as_str()),
            (Status::Queued, "в очереди, №2")
        );
        assert_eq!(t.actions, vec!["delegate", "tell"]);

        let t =
            task(json!({"id":"p/4","state":"working","stage":"after","msg":"queued after P-3"}));
        assert_eq!(
            (t.status, t.board_label.as_str()),
            (Status::Queued, "ждёт другие задачи")
        );
    }

    #[test]
    fn what_waits_for_the_user() {
        let q = task(json!({"id":"p/1","state":"blocked","kind":"question","stage":"working"}));
        assert_eq!(
            (q.status, q.board_label.as_str()),
            (Status::Waiting, "ждёт ответа")
        );
        assert_eq!(q.actions, vec!["answer", "tell", "stop"]);

        let perm =
            task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"working"}));
        assert_eq!(perm.board_label, "ждёт разрешения");
        let auth = task(json!({"id":"p/1","state":"blocked","kind":"auth","stage":"working"}));
        assert_eq!(auth.board_label, "нужен вход");

        let pick = task(json!({"id":"p/1","state":"blocked","stage":"pick"}));
        assert_eq!(
            (pick.status, pick.board_label.as_str()),
            (Status::Waiting, "выбери вариант")
        );
        assert!(pick.actions.is_empty());

        let stopped = task(json!({"id":"p/1","state":"idle","stage":"stopped"}));
        assert_eq!(
            (stopped.status, stopped.board_label.as_str()),
            (Status::Queued, "остановлена")
        );
        let limit = task(json!({"id":"p/1","state":"blocked","stage":"stopped"}));
        assert_eq!(
            (limit.status, limit.board_label.as_str()),
            (Status::Waiting, "остановлена на лимите")
        );
        assert_eq!(limit.actions, vec!["retry", "tell"]);
    }

    #[test]
    fn plan_ready_and_failed_review() {
        let plan = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"plan"}));
        assert_eq!(
            (plan.status, plan.board_label.as_str()),
            (Status::Plan, "план готов")
        );
        assert_eq!(plan.actions, vec!["approve", "tell"]);

        let ok = task(
            json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval",
            "review":{"findings":[{"severity":"minor"}],"meets":true,"pass":true,"text":"ok"}}),
        );
        assert_eq!(
            (ok.status, ok.board_label.as_str()),
            (Status::Ready, "готова к одобрению")
        );
        assert_eq!(ok.review.as_ref().unwrap().tally, "1 minor");
        assert_eq!(ok.actions, vec!["approve", "anyway", "review", "tell"]);

        let none =
            task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval"}));
        assert_eq!(none.status, Status::Ready);

        let bad = task(
            json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval",
            "review":{"findings":[{"severity":"critical"},{"severity":"major"},{"severity":"major","preexisting":true}],"meets":false,"pass":false}}),
        );
        assert_eq!(
            (bad.status, bad.board_label.as_str()),
            (Status::Waiting, "ревью не прошло")
        );
        assert!(bad.review_failed);
        assert_eq!(bad.review.as_ref().unwrap().tally, "1 critical, 1 major");
        assert_eq!(bad.actions, vec!["review", "anyway", "tell"]);

        let old = task(
            json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","review":{"score":2,"recommend":false}}),
        );
        assert!(old.review_failed);
        assert_eq!(old.review.unwrap().tally, "2/5");
    }

    #[test]
    fn merge_blocked_words() {
        let checks = task(
            json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","msg":"GitHub checks failed","review":{"findings":[],"meets":true,"pass":true}}),
        );
        assert_eq!(
            (checks.status, checks.board_label.as_str()),
            (Status::Waiting, "нельзя влить")
        );
        assert!(checks.merge_blocked && !checks.review_failed);
        assert_eq!(checks.actions, vec!["anyway", "review", "tell"]);

        let plain = task(
            json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","msg":"Pass · 1 minor"}),
        );
        assert_eq!(plain.status, Status::Ready);
        assert!(!plain.merge_blocked);
    }

    #[test]
    fn failed_done_and_nothing_started() {
        let e = task(json!({"id":"p/1","state":"error","stage":"working"}));
        assert_eq!(
            (e.status, e.board_label.as_str()),
            (Status::Failed, "упала")
        );
        assert_eq!(e.actions, vec!["retry", "tell", "stop"]);

        let n = task(json!({"id":"p/1","state":"blocked","stage":"nosummary"}));
        assert_eq!(
            (n.status, n.board_label.as_str()),
            (Status::Failed, "остановилась без итога")
        );

        let todo = task(json!({"id":"p/1","state":"idle","stage":"todo"}));
        assert_eq!(
            (todo.status, todo.board_label.as_str()),
            (Status::Queued, "не начата")
        );
        assert_eq!(todo.actions, vec!["delegate", "tell"]);

        let done = task(
            json!({"id":"p/1","state":"done","stage":"done","pr":"https://github.com/o/r/pull/7"}),
        );
        assert_eq!(
            (done.status, done.pr.as_str()),
            (Status::Done, "https://github.com/o/r/pull/7")
        );
        assert_eq!(done.actions, vec!["archive"]);
        assert_eq!(done.label, "Сделано");
    }

    #[test]
    fn archived_rows_and_attempts_are_handled() {
        let list = parse_list(
            r#"[
            {"id":"a/1","state":"done","stage":"done"},
            {"id":"a/2","state":"done","stage":"done","archivedAt":1792407710774},
            {"id":"a/3","state":"idle","stage":"tried"},
            {"nope":1}
        ]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert!(!list[0].archived);
        assert!(list[1].archived);
        assert!(parse_list("not json").is_err());
        assert!(parse_list("{}").is_err());
        assert!(parse_list("[]").unwrap().is_empty());
    }

    #[test]
    fn tags_come_from_plain_list_in_the_same_order() {
        let mut tasks = parse_list(
            r#"[{"id":"a/1","state":"done"},{"id":"b/12","state":"idle"},{"id":"b/13","state":"done"}]"#,
        )
        .unwrap();
        let tags =
            plain_tags("PLN-1   done  [x]  A\n#12     idle  B\nSS-13   done  C  (archived)\n");
        apply_tags(&mut tasks, &tags);
        assert_eq!(tasks[0].tag, "PLN-1");
        assert_eq!(tasks[1].tag, "#12");
        assert_eq!(tasks[2].tag, "SS-13");
        // A different number, or a different count, is not believed.
        let mut one = parse_list(r#"[{"id":"a/1","state":"done"}]"#).unwrap();
        apply_tags(&mut one, &["PLN-7".to_string()]);
        assert_eq!(one[0].tag, "#1");
        apply_tags(&mut one, &["PLN-1".to_string(), "X-2".to_string()]);
        assert_eq!(one[0].tag, "#1");
    }

    #[test]
    fn projects_and_summary_split_active_done_and_quiet() {
        let mut tasks = parse_list(
            r#"[
                {"id":"alpha/1","state":"done","stage":"done"},
                {"id":"alpha/2","state":"working","stage":"working"},
                {"id":"beta/1","state":"done","stage":"done"},
                {"id":"beta/2","state":"done","stage":"done"},
                {"id":"gamma/1","state":"blocked","kind":"question","stage":"working"}
            ]"#,
        )
        .unwrap();
        apply_tags(
            &mut tasks,
            &[
                "ALP-1".into(),
                "ALP-2".into(),
                "BET-1".into(),
                "BET-2".into(),
                "GAM-1".into(),
            ],
        );
        let ps = projects(&tasks);
        assert_eq!(ps.len(), 3);
        let alpha = ps.iter().find(|p| p.name == "alpha").unwrap();
        assert_eq!(
            (alpha.key.as_str(), alpha.active, alpha.done, alpha.quiet),
            ("ALP", 1, 1, false)
        );
        let beta = ps.iter().find(|p| p.name == "beta").unwrap();
        assert_eq!((beta.active, beta.done, beta.quiet), (0, 2, true));
        let s = summary(&tasks, &tasks[..1], &ps);
        assert_eq!(s.tasks, 5);
        assert_eq!(s.done, 3);
        assert_eq!(s.waiting, 1);
        assert_eq!(s.working, 1);
        assert_eq!(s.attention, 1);
        assert_eq!(s.projects, 3);
        assert_eq!(s.active_projects, 2);
        assert_eq!(s.quiet_projects, 1);
        assert_eq!(s.archived, 1);
    }

    #[test]
    fn agent_from_show_and_details() {
        let show = "PLN-3: every setting\n* worker.agent              omp/deepseek/deepseek-v4-flash  this task\n  worker.effort high\n";
        assert_eq!(show_agent(show), "omp/deepseek/deepseek-v4-flash");
        assert_eq!(show_agent("nothing"), "");
        let d = parse_details(
            r#"{"task":{"id":"PLN-3","title":"T"},"failure":null,"review":null,"reviews":[],"agent":"omp","worktree":"/tmp/w","branch":"work/3","limits":{"tokens":{"used":1}}}"#,
            "/tmp/reports",
        )
        .unwrap();
        assert_eq!(d.task["id"], "PLN-3");
        assert_eq!(d.agent, "omp");
        assert_eq!(d.worktree, "/tmp/w");
        assert_eq!(d.reports_dir, "/tmp/reports");
    }

    #[test]
    fn after_output_becomes_tags() {
        assert_eq!(parse_after("alpha/10: waits for ALP-9"), vec!["ALP-9"]);
        assert_eq!(
            parse_after("alpha/10: waits for ALP-9 (done), ALP-3"),
            vec!["ALP-9", "ALP-3"]
        );
        assert_eq!(
            parse_after("alpha/11: is to wait for ALP-9, ALP-3"),
            vec!["ALP-9", "ALP-3"]
        );
        assert_eq!(
            parse_after("alpha/10: waits for ALP-9; it starts by itself once that is done"),
            vec!["ALP-9"]
        );
        assert_eq!(
            parse_after("alpha/10: is to wait for ALP-9, from when it is delegated"),
            vec!["ALP-9"]
        );
        assert_eq!(
            parse_after("alpha/10: waits for ALP-9. It waits for you: ALP-9 failed"),
            vec!["ALP-9"]
        );
        assert_eq!(
            parse_after("alpha/11: waits for nothing"),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_after("alpha/11: waits for nothing, so it starts now"),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_after("alpha/12: waits for vm-str:ELA-3"),
            vec!["vm-str:ELA-3"]
        );
        assert!(parse_after("").is_empty());
        assert!(parse_after("No task PLN-999").is_empty());
    }

    #[test]
    fn links_resolve_tags_and_mark_the_unknown() {
        let mut tasks = parse_list(
            r#"[
                {"id":"a/1","state":"done","stage":"done"},
                {"id":"a/2","state":"working","stage":"after","msg":"queued after A-1"},
                {"id":"a/4","state":"working","stage":"working"},
                {"id":"b/1","state":"working","stage":"working"}
            ]"#,
        )
        .unwrap();
        apply_tags(
            &mut tasks,
            &["A-1".into(), "A-2".into(), "A-4".into(), "B-1".into()],
        );
        let mut after = BTreeMap::new();
        after.insert("a/2".to_string(), vec!["A-1".to_string()]);
        after.insert(
            "a/4".to_string(),
            vec!["B-1".to_string(), "vm-str:ELA-3".to_string()],
        );
        let links = links_from(&after, &tasks);
        assert_eq!(links.len(), 3);
        assert_eq!(
            links[0],
            Link {
                from: "a/1".into(),
                to: "a/2".into(),
                kind: "done".into(),
                label: "A-1".into()
            }
        );
        assert_eq!(
            links[1],
            Link {
                from: "b/1".into(),
                to: "a/4".into(),
                kind: "waiting".into(),
                label: "B-1".into()
            }
        );
        assert_eq!(
            links[2],
            Link {
                from: "vm-str:ELA-3".into(),
                to: "a/4".into(),
                kind: "far".into(),
                label: "vm-str:ELA-3".into()
            }
        );
        // An archived predecessor is not drawn as a planet, but keeps its id so a
        // later link edit may resend it.
        let mut archived = tasks[0].clone();
        archived.archived = true;
        let one = vec![archived];
        let a = &links_from(&after, &one)[0];
        assert_eq!(
            (a.kind.as_str(), a.from.as_str(), a.label.as_str()),
            ("far", "a/1", "A-1")
        );
    }

    #[test]
    fn after_refs_and_letters_are_safe() {
        assert!(valid_ref("alpha/11"));
        assert!(valid_ref("vm-str:ELA-3"));
        assert!(valid_ref("vm-str:ela/3"));
        assert!(!valid_ref("ALP-9"));
        assert!(!valid_ref("--clear"));
        assert!(!valid_ref("vm-str:"));
        assert!(!valid_ref(":ELA-3"));
        assert!(!valid_ref("a b/1"));
        assert!(valid_letter("B"));
        assert!(!valid_letter("BB"));
        assert!(!valid_letter("1"));
        assert!(!valid_letter(""));
    }

    #[test]
    fn ids_are_safe_as_command_arguments() {
        assert!(valid_id("herdr-planner/3"));
        assert!(valid_id("vm-str:ELA/3"));
        assert!(!valid_id("--now"));
        assert!(!valid_id("a b"));
        assert!(!valid_id(""));
        assert!(valid_tag("PLN-3"));
        assert!(!valid_tag("<script>"));
    }
}
