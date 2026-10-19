//! Tasks of the herdr-projects board, as `herdr-projects list --json` prints
//! them, and where each one stands: the bucket the planner files it under.
//!
//! The board decides the stage of a task; the planner only reads it. The
//! rules below follow the board's own grouping (`needs you`, `ready to
//! approve`, `working`, `to do`, `done`), refined with the stages the planner
//! wants apart: a task that failed, one under review, one queued.

use serde::Serialize;
use serde_json::Value;

/// Where a task stands, in the order it needs the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bucket {
    /// A question, a permission, a login, a plan, a choice, a failed review.
    Needs,
    /// Finished work that passed its review and waits for the user's yes.
    Ready,
    /// Went wrong: an error, a stop without a summary.
    Failed,
    /// The reviewer is on it.
    Review,
    /// The agent (or the board's own step) is on it.
    Working,
    /// Waits for a free slot or for other tasks (`--after`).
    Queued,
    /// Nobody has started it.
    #[default]
    Todo,
    /// Finished.
    Done,
}


/// What the current review said, in short.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ReviewBrief {
    /// Passed the board's gate; none for a review from before the gate that gave no verdict.
    pub pass: Option<bool>,
    /// `1 critical, 1 major`; empty when it found nothing.
    pub tally: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Task {
    /// `repo/number`, the board's own name for it.
    pub id: String,
    pub repo: String,
    pub n: u64,
    /// `PLN-1`: the repo's key and the number; `#1` for a repo without a key.
    pub tag: String,
    pub title: String,
    pub state: String,
    pub kind: String,
    pub stage: String,
    /// What the task says now (the board's `msg`).
    pub msg: String,
    /// The program that works on it.
    pub app: String,
    pub progress: Option<u32>,
    pub eta: Option<u32>,
    pub pr: String,
    pub research: bool,
    /// Its place in the queue for a free slot.
    pub queue: Option<u32>,
    pub review: Option<ReviewBrief>,
    pub round: u32,
    pub bucket: Bucket,
    /// Where it stands, in words: «план готов», «ждёт ответа», «на ревью».
    pub label: String,
    /// The finished work failed its review: it may still be approved, anyway.
    pub review_failed: bool,
    /// Finished work the board will not merge by a plain yes: its pull request's checks fail, the merge
    /// failed or the branch is behind main. The list does not say which; the board's own words do.
    pub merge_blocked: bool,
    /// When it came to this stage, unix milliseconds; 0 when not known.
    pub at: u64,
    /// The board commands the page may offer for it.
    pub actions: Vec<&'static str>,
}

/// The repo of a task id: everything before the number (`herdr-planner/1`).
pub fn repo_of(id: &str) -> &str {
    id.rsplit_once('/').map_or(id, |(repo, _)| repo)
}

/// The number of a task id, 0 when there is none.
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

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn num(v: &Value, key: &str) -> Option<u32> {
    v.get(key)
        .and_then(Value::as_f64)
        .filter(|n| *n >= 0.0)
        .map(|n| n.round() as u32)
}

/// A review is tagged when it lists findings and says whether the change does what was asked.
fn tagged(rv: &Value) -> bool {
    rv.get("findings").is_some_and(Value::is_array) && rv.get("meets").is_some_and(Value::is_boolean)
}

/// `1 critical, 2 major`: the findings of the task's own, the worst first.
fn tally(rv: &Value) -> String {
    let mut counts = [0u32; 4];
    for f in rv.get("findings").and_then(Value::as_array).into_iter().flatten() {
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

/// The verdict and tally of a review object (the board's `review`).
pub fn review_brief(rv: &Value) -> Option<ReviewBrief> {
    if !rv.is_object() {
        return None;
    }
    if tagged(rv) {
        let pass = rv.get("pass").and_then(Value::as_bool);
        return Some(ReviewBrief {
            pass,
            tally: tally(rv),
        });
    }
    // A review from before the gate: a score and a merge advice.
    let recommend = rv.get("recommend").and_then(Value::as_bool);
    let score = rv.get("score").and_then(Value::as_i64);
    Some(ReviewBrief {
        pass: recommend,
        tally: score.map(|n| format!("{n}/5")).unwrap_or_default(),
    })
}

/// Whether what a finished task says is the board's word for a merge that cannot go through
/// (failing checks, a failed merge, a branch behind main). The list gives only the message.
pub fn says_merge_blocked(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["checks fail", "checks did not pass", "merge fail", "conflict", "behind", "cannot merge", "could not merge", "did not go through", "not up to date"]
        .iter()
        .any(|w| m.contains(w))
}

/// The stage of a task and the words for it. `state` is the board's own
/// (`idle`, `working`, `blocked`, `done`, `error`), `stage` its position on the way.
pub fn classify(t: &Task) -> (Bucket, String) {
    let state = t.state.as_str();
    let stage = t.stage.as_str();
    if state == "done" {
        return (Bucket::Done, "сделана".into());
    }
    if state == "error" || stage == "failed" || stage == "nosummary" {
        let word = if stage == "nosummary" {
            "остановилась без итога"
        } else {
            "упала"
        };
        return (Bucket::Failed, word.into());
    }
    if stage == "pick" {
        return (Bucket::Needs, "выбери вариант".into());
    }
    // Stopped by hand it is a task nobody works on; stopped at a limit it waits for the user to go on.
    if stage == "stopped" {
        return if state == "blocked" {
            (Bucket::Needs, "остановлена на лимите".into())
        } else {
            (Bucket::Todo, "остановлена".into())
        };
    }
    if state == "blocked" {
        if stage == "reviewneeds" {
            return (Bucket::Needs, "ревьюеру нужен ты".into());
        }
        if stage == "plan" {
            return (Bucket::Needs, "план готов".into());
        }
        if stage == "approval" {
            if t.review_failed {
                return (Bucket::Needs, "ревью не прошло".into());
            }
            if t.merge_blocked {
                return (Bucket::Needs, "нельзя влить".into());
            }
            if t.kind == "permission" {
                return (Bucket::Ready, "готова к одобрению".into());
            }
        }
        let word = match t.kind.as_str() {
            "question" => "ждёт ответа",
            "permission" => "ждёт разрешения",
            "auth" => "нужен вход",
            _ => "ждёт тебя",
        };
        return (Bucket::Needs, word.into());
    }
    if state == "working" {
        return match stage {
            "queued" => (
                Bucket::Queued,
                match t.queue {
                    Some(n) if n > 0 => format!("в очереди, №{n}"),
                    _ => "в очереди".into(),
                },
            ),
            "after" => (Bucket::Queued, "ждёт другие задачи".into()),
            "reviewing" => (Bucket::Review, "на ревью".into()),
            "starting" => (Bucket::Working, "запускается".into()),
            "setup" => (Bucket::Working, "готовит окружение".into()),
            "checking" => (Bucket::Working, "проверка".into()),
            "pushing" => (Bucket::Working, "отправляет".into()),
            "merging" => (Bucket::Working, "вливается".into()),
            _ => (Bucket::Working, "в работе".into()),
        };
    }
    (Bucket::Todo, "не начата".into())
}

/// The board commands the page offers for a task in this bucket.
pub fn actions_for(t: &Task) -> Vec<&'static str> {
    match t.bucket {
        Bucket::Needs if t.stage == "plan" => vec!["approve", "tell"],
        Bucket::Needs if t.review_failed => vec!["review", "anyway", "tell"],
        Bucket::Needs if t.merge_blocked && t.review.is_some() => vec!["anyway", "review", "tell"],
        Bucket::Needs if t.merge_blocked => vec!["anyway", "tell"],
        Bucket::Needs if t.stage == "pick" => vec![],
        Bucket::Needs if t.stage == "stopped" => vec!["retry", "tell"],
        Bucket::Needs => vec!["tell", "stop"],
        // The list does not show a failing pull request or a merge that cannot go through: «всё равно» is always at hand.
        Bucket::Ready if t.review.is_some() => vec!["approve", "anyway", "review", "tell"],
        Bucket::Ready => vec!["approve", "anyway", "tell"],
        Bucket::Failed => vec!["retry", "tell", "stop"],
        Bucket::Review | Bucket::Working | Bucket::Queued => vec!["tell", "stop"],
        Bucket::Todo => vec!["delegate"],
        Bucket::Done => vec![],
    }
}

/// One row of `list --json`; none for something that is not a task.
pub fn parse_task(v: &Value) -> Option<Task> {
    let id = s(v, "id");
    if id.is_empty() || !id.contains('/') {
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
        stage: s(v, "stage").to_string(),
        msg: s(v, "msg").to_string(),
        app: s(v, "app").to_string(),
        progress: num(v, "progress").map(|n| n.min(100)),
        eta: num(v, "eta"),
        pr: match v.get("pr") {
            Some(Value::String(x)) => x.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        },
        research: v.get("research").and_then(Value::as_bool) == Some(true),
        queue: num(v, "queue"),
        round: num(v, "reviewRound").unwrap_or(0),
        ..Task::default()
    };
    // The board holds work that failed its review under `needs you`, not `ready`.
    t.review_failed = t.stage == "approval" && review.as_ref().is_some_and(|r| r.pass == Some(false));
    t.merge_blocked = t.stage == "approval" && !t.review_failed && says_merge_blocked(&t.msg);
    t.review = review;
    // An attempt of a task stays with its task.
    if t.stage == "tried" {
        return None;
    }
    let (bucket, label) = classify(&t);
    t.bucket = bucket;
    t.label = label;
    t.actions = actions_for(&t);
    Some(t)
}

/// The tasks of `list --json`. A reply that is not a list is an error; a row that is not a task is skipped.
pub fn parse_list(stdout: &str) -> Result<Vec<Task>, String> {
    let v: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("herdr-projects list --json ответил не JSON: {e}"))?;
    let Some(rows) = v.as_array() else {
        return Err("herdr-projects list --json ответил не списком".into());
    };
    Ok(rows.iter().filter_map(parse_task).collect())
}

/// The first word of each line of plain `list`, which is the task's tag (`PLN-1`),
/// in the order of `list --json`.
pub fn plain_tags(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(String::from)
        .collect()
}

/// Gives each task its tag from plain `list`, whose lines come in the same order.
/// A tag whose number is not the task's is not believed.
pub fn apply_tags(tasks: &mut [Task], tags: &[String]) {
    if tags.len() != tasks.len() {
        return;
    }
    for (t, tag) in tasks.iter_mut().zip(tags) {
        // `PLN-1` or `#1`: it ends with the task's own number.
        if tag.len() > 1 && tag.rsplit(['-', '#']).next() == Some(t.n.to_string().as_str()) {
            t.tag = tag.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(v: Value) -> Task {
        parse_task(&v).expect("a task")
    }

    #[test]
    fn id_parts() {
        assert_eq!(repo_of("herdr-planner/1"), "herdr-planner");
        assert_eq!(number_of("herdr-planner/12"), 12);
        assert_eq!(number_of("x"), 0);
        assert!(valid_id("apicache/2"));
        assert!(valid_id("vm-str:ELA/3"));
        assert!(!valid_id("--now"));
        assert!(!valid_id("a b"));
        assert!(!valid_id(""));
    }

    #[test]
    fn working_and_review() {
        let t = task(json!({"id":"p/1","state":"working","stage":"working","title":"T","progress":4,"app":"claude","msg":"Reading"}));
        assert_eq!((t.bucket, t.label.as_str()), (Bucket::Working, "в работе"));
        assert_eq!(t.progress, Some(4));
        assert_eq!(t.actions, vec!["tell", "stop"]);
        let t = task(json!({"id":"p/2","state":"working","stage":"reviewing","reviewRound":1}));
        assert_eq!(t.bucket, Bucket::Review);
        assert_eq!(t.round, 1);
        let t = task(json!({"id":"p/3","state":"working","stage":"merging"}));
        assert_eq!((t.bucket, t.label.as_str()), (Bucket::Working, "вливается"));
    }

    #[test]
    fn queued_and_after() {
        let t = task(json!({"id":"p/1","state":"working","stage":"queued","queue":2}));
        assert_eq!((t.bucket, t.label.as_str()), (Bucket::Queued, "в очереди, №2"));
        let t = task(json!({"id":"p/1","state":"working","stage":"after","msg":"queued after PLN-3"}));
        assert_eq!((t.bucket, t.label.as_str()), (Bucket::Queued, "ждёт другие задачи"));
    }

    #[test]
    fn what_waits_for_the_user() {
        let q = task(json!({"id":"p/1","state":"blocked","kind":"question","stage":"working"}));
        assert_eq!((q.bucket, q.label.as_str()), (Bucket::Needs, "ждёт ответа"));
        let perm = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"working"}));
        assert_eq!((perm.bucket, perm.label.as_str()), (Bucket::Needs, "ждёт разрешения"));
        let auth = task(json!({"id":"p/1","state":"blocked","kind":"auth","stage":"working"}));
        assert_eq!(auth.label, "нужен вход");
        let plan = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"plan"}));
        assert_eq!((plan.bucket, plan.label.as_str()), (Bucket::Needs, "план готов"));
        assert_eq!(plan.actions, vec!["approve", "tell"]);
        let pick = task(json!({"id":"p/1","state":"blocked","stage":"pick"}));
        assert_eq!((pick.bucket, pick.label.as_str()), (Bucket::Needs, "выбери вариант"));
        assert!(pick.actions.is_empty());
    }

    #[test]
    fn ready_to_approve_and_a_failed_review() {
        let ok = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval",
            "review":{"findings":[{"severity":"minor","subject":"x"}],"meets":true,"pass":true}}));
        assert_eq!((ok.bucket, ok.label.as_str()), (Bucket::Ready, "готова к одобрению"));
        assert_eq!(ok.review.as_ref().unwrap().tally, "1 minor");
        assert_eq!(ok.actions, vec!["approve", "anyway", "review", "tell"]);
        let none = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval"}));
        assert_eq!(none.bucket, Bucket::Ready);
        assert_eq!(none.actions, vec!["approve", "anyway", "tell"]);
        let bad = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval",
            "review":{"findings":[{"severity":"critical"},{"severity":"major"},{"severity":"major","preexisting":true}],"meets":false,"pass":false}}));
        assert_eq!((bad.bucket, bad.label.as_str()), (Bucket::Needs, "ревью не прошло"));
        assert!(bad.review_failed);
        assert_eq!(bad.review.as_ref().unwrap().tally, "1 critical, 1 major");
        assert_eq!(bad.actions, vec!["review", "anyway", "tell"]);
        // A review from before the gate.
        let old = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","review":{"score":2,"recommend":false}}));
        assert!(old.review_failed);
        assert_eq!(old.review.unwrap().tally, "2/5");
    }

    #[test]
    fn work_the_board_will_not_merge_by_a_plain_yes() {
        let checks = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","msg":"GitHub checks failed","review":{"findings":[],"meets":true,"pass":true}}));
        assert_eq!((checks.bucket, checks.label.as_str()), (Bucket::Needs, "нельзя влить"));
        assert!(checks.merge_blocked && !checks.review_failed);
        assert_eq!(checks.actions, vec!["anyway", "review", "tell"]);
        let board_words = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","msg":"The pull request's checks fail"}));
        assert_eq!((board_words.bucket, board_words.actions.clone()), (Bucket::Needs, vec!["anyway", "tell"]));
        let behind = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","msg":"Behind main: could not bring it up to date"}));
        assert_eq!(behind.bucket, Bucket::Needs);
        // A plain message stays ready, and then «всё равно» is offered next to «одобрить».
        let plain = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","msg":"Pass · 1 minor"}));
        assert_eq!(plain.bucket, Bucket::Ready);
        assert!(plain.actions.contains(&"anyway"));
        // The words only count at the approval stage.
        assert!(!task(json!({"id":"p/1","state":"working","stage":"working","msg":"fixing the merge conflict"})).merge_blocked);
    }

    #[test]
    fn stopped_tasks_and_the_reviewer_asking() {
        let by_hand = task(json!({"id":"p/1","state":"idle","stage":"stopped","msg":"Stopped"}));
        assert_eq!((by_hand.bucket, by_hand.label.as_str(), by_hand.actions.clone()), (Bucket::Todo, "остановлена", vec!["delegate"]));
        let limit = task(json!({"id":"p/1","state":"blocked","stage":"stopped","msg":"Stopped at a limit"}));
        assert_eq!((limit.bucket, limit.label.as_str(), limit.actions.clone()), (Bucket::Needs, "остановлена на лимите", vec!["retry", "tell"]));
        let rn = task(json!({"id":"p/1","state":"blocked","stage":"reviewneeds"}));
        assert_eq!((rn.bucket, rn.label.as_str()), (Bucket::Needs, "ревьюеру нужен ты"));
    }

    #[test]
    fn failed_todo_done() {
        let e = task(json!({"id":"p/1","state":"error","stage":"working"}));
        assert_eq!((e.bucket, e.label.as_str()), (Bucket::Failed, "упала"));
        assert_eq!(e.actions, vec!["retry", "tell", "stop"]);
        let n = task(json!({"id":"p/1","state":"blocked","stage":"nosummary"}));
        assert_eq!((n.bucket, n.label.as_str()), (Bucket::Failed, "остановилась без итога"));
        let todo = task(json!({"id":"p/1","state":"idle","stage":"todo"}));
        assert_eq!((todo.bucket, todo.actions.clone()), (Bucket::Todo, vec!["delegate"]));
        let done = task(json!({"id":"p/1","state":"done","stage":"done","pr":"https://github.com/o/r/pull/7"}));
        assert_eq!((done.bucket, done.pr.as_str()), (Bucket::Done, "https://github.com/o/r/pull/7"));
        assert!(done.actions.is_empty());
        // No state at all is a task nobody started.
        assert_eq!(task(json!({"id":"p/9"})).bucket, Bucket::Todo);
    }

    #[test]
    fn parses_a_list_and_skips_what_is_not_a_task() {
        let list = parse_list(r#"[{"id":"a/1","state":"done"},{"nope":1},{"id":"a/2","state":"idle","stage":"tried"},{"id":"b/3","state":"idle"}]"#).unwrap();
        assert_eq!(list.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["a/1", "b/3"]);
        assert!(parse_list("not json").is_err());
        assert!(parse_list("{}").is_err());
        assert!(parse_list("[]").unwrap().is_empty());
    }

    #[test]
    fn tags_come_from_plain_list_in_the_same_order() {
        let mut tasks = parse_list(r#"[{"id":"a/1","state":"done"},{"id":"b/12","state":"idle"}]"#).unwrap();
        assert_eq!(tasks[0].tag, "#1");
        let tags = plain_tags("PLN-1   done  [x]  Title\n#12      idle  ...\n");
        apply_tags(&mut tasks, &tags);
        assert_eq!(tasks[0].tag, "PLN-1");
        assert_eq!(tasks[1].tag, "#12");
        // A different number, or a different count, is not believed.
        let mut tasks = parse_list(r#"[{"id":"a/1","state":"done"}]"#).unwrap();
        apply_tags(&mut tasks, &["PLN-7".to_string()]);
        assert_eq!(tasks[0].tag, "#1");
        apply_tags(&mut tasks, &["PLN-1".to_string(), "X-2".to_string()]);
        assert_eq!(tasks[0].tag, "#1");
    }
}
