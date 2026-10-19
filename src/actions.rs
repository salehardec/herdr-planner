//! What the page does with the board: the commands for one task (`approve`,
//! `review`, `retry`, `delegate`, `stop`, `tell`), a task's details, and
//! «показать все проекты» (clears the agent view of the herdr sidebar).
//!
//! The planner decides nothing about the board: it offers a command only for the
//! stages in `Task::actions`, passes the task's own id (one the board listed,
//! never text from the page) and shows the board's answer.

use crate::app::Shared;
use crate::board::{self, Task};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

/// A refusal with the HTTP status it is answered with.
pub struct Failure(pub u16, pub String);

#[derive(Debug, Deserialize)]
pub struct TaskReq {
    pub id: String,
    pub kind: String,
    /// `tell`: the words for the agent.
    #[serde(default)]
    pub text: String,
}

/// The longest message `tell` sends.
pub const TELL_MAX: usize = 4000;

/// The arguments of the board command for `req` on `task`; a refusal when the
/// command does not fit the stage the task is in.
pub fn board_args(task: &Task, req: &TaskReq) -> Result<Vec<String>, Failure> {
    let kind = req.kind.as_str();
    let known = ["approve", "anyway", "review", "retry", "delegate", "stop", "tell"];
    if !known.contains(&kind) {
        return Err(Failure(400, "неизвестное действие".into()));
    }
    if !task.actions.contains(&kind) {
        return Err(Failure(
            409,
            format!("{}: «{}» сейчас так нельзя ({})", task.tag, kind_word(kind), task.label),
        ));
    }
    // «Всё равно» is the board's `approve --anyway`: also when the review failed or the merge cannot go through.
    let mut args = vec![if kind == "anyway" { "approve" } else { kind }.to_string(), task.id.clone()];
    match kind {
        "anyway" => args.push("--anyway".into()),
        "tell" => {
            let text = req.text.trim();
            if text.is_empty() {
                return Err(Failure(400, "нет текста".into()));
            }
            if text.chars().count() > TELL_MAX {
                return Err(Failure(400, "слишком длинное сообщение".into()));
            }
            // Words that start with `--` are text, not options.
            args.push("--".into());
            args.push(text.to_string());
        }
        _ => {}
    }
    Ok(args)
}

pub fn kind_word(kind: &str) -> &'static str {
    match kind {
        "approve" => "одобрить",
        "anyway" => "одобрить всё равно",
        "review" => "отправить ревью агенту",
        "retry" => "повторить",
        "delegate" => "запустить",
        "stop" => "остановить",
        "tell" => "ответить",
        _ => "действие",
    }
}

fn find_task(app: &Shared, id: &str) -> Result<Task, Failure> {
    if !board::valid_id(id) {
        return Err(Failure(400, "нет задачи".into()));
    }
    app.snapshot
        .read()
        .unwrap()
        .projects
        .iter()
        .flat_map(|p| p.tasks.iter())
        .find(|t| t.id == id)
        .cloned()
        .ok_or_else(|| Failure(404, format!("{id}: такой задачи нет на доске")))
}

/// Runs the command; the board's answer comes back as `message`.
pub async fn run_task_action(app: &Shared, req: &TaskReq) -> Result<Value, Failure> {
    let task = find_task(app, &req.id)?;
    let args = board_args(&task, req)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = app
        .board(&refs, Duration::from_secs(60))
        .await
        .map_err(|e| Failure(502, e))?;
    if !out.ok() {
        return Err(Failure(502, out.error()));
    }
    app.refresh_now.notify_one();
    Ok(json!({ "ok": true, "message": out.stdout.trim() }))
}

/// Cuts long strings of a decoded answer, so a huge plan or report does not travel whole.
fn shorten(v: &Value, max: usize) -> Value {
    match v {
        Value::String(s) if s.chars().count() > max => {
            Value::String(s.chars().take(max).collect::<String>() + "…")
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| shorten(x, max)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), shorten(x, max))).collect()),
        other => other.clone(),
    }
}

/// What the page shows of `herdr-projects details --json`: the stage, the message, the failure,
/// the review with its findings, the plan, the branch and pull request, the limits.
pub fn trim_details(d: &Value) -> Value {
    let get = |k: &str| d.get(k).cloned().unwrap_or(Value::Null);
    let review = d.get("review").filter(|r| r.is_object()).map(|rv| {
        let brief = board::review_brief(rv);
        let findings: Vec<Value> = rv
            .get("findings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|f| f.is_object())
            .map(|f| {
                json!({
                    "id": f.get("id"), "severity": f.get("severity"), "subject": f.get("subject"),
                    "where": f.get("where"), "proof": f.get("proof"), "preexisting": f.get("preexisting"),
                })
            })
            .collect();
        json!({
            "pass": brief.as_ref().and_then(|b| b.pass),
            "tally": brief.map(|b| b.tally).unwrap_or_default(),
            "findings": findings,
            "summary": rv.get("summary"),
        })
    });
    shorten(
        &json!({
            "task": get("task"),
            "failure": get("failure"),
            "review": review,
            "rounds": d.get("reviews").and_then(Value::as_array).map_or(0, Vec::len),
            "agent": get("agent"),
            "research": get("research"),
            "report": get("report"),
            "plan": get("plan"),
            "pr": get("pr"),
            "worktree": get("worktree"),
            "branch": get("branch"),
            "limits": get("limits"),
            "queue": get("queue"),
            "after": get("after"),
        }),
        6000,
    )
}

/// `herdr-projects details <id> --json`, trimmed.
pub async fn details(app: &Shared, id: &str) -> Result<Value, Failure> {
    let task = find_task(app, id)?;
    let out = app
        .board(&["details", &task.id, "--json"], Duration::from_secs(20))
        .await
        .map_err(|e| Failure(502, e))?;
    if !out.ok() {
        return Err(Failure(502, out.error()));
    }
    let d: Value = serde_json::from_str(out.stdout.trim())
        .map_err(|_| Failure(502, "доска ответила не JSON".into()))?;
    let mut t = trim_details(&d);
    t["tag"] = json!(task.tag);
    t["actions"] = json!(task.actions);
    Ok(t)
}

/// One request to the herdr server's socket: a JSON line out, a JSON line back.
pub fn socket_request(socket: &Path, method: &str, params: &Value) -> Result<Value, String> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|e| format!("herdr не отвечает ({}): {e}", socket.display()))?;
    let timeout = Some(Duration::from_secs(5));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let line = json!({ "id": "herdr-planner", "method": method, "params": params });
    stream
        .write_all(format!("{line}\n").as_bytes())
        .map_err(|e| format!("herdr: {e}"))?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| format!("herdr: {e}"))?;
    let v: Value = serde_json::from_str(reply.trim()).map_err(|_| "herdr ответил не JSON".to_string())?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| err.to_string());
        return Err(format!("herdr: {msg}"));
    }
    Ok(v)
}

/// «Показать все проекты»: the sidebar of herdr lists every agent again (its agent
/// view is cleared). The board has no command for it, so this is a request to herdr itself.
pub async fn unfocus(app: &Shared) -> Result<(), String> {
    let socket = app.config.herdr_socket.clone();
    tokio::task::spawn_blocking(move || socket_request(&socket, "agent.view.clear", &json!({})).map(|_| ()))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(v: Value) -> Task {
        board::parse_task(&v).unwrap()
    }

    fn req(kind: &str) -> TaskReq {
        TaskReq {
            id: "p/1".into(),
            kind: kind.into(),
            text: String::new(),
        }
    }

    fn ready() -> Task {
        task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","review":{"findings":[],"meets":true,"pass":true}}))
    }

    #[test]
    fn commands_for_a_stage() {
        let t = ready();
        assert_eq!(board_args(&t, &req("approve")).ok().unwrap(), ["approve", "p/1"]);
        assert_eq!(board_args(&t, &req("review")).ok().unwrap(), ["review", "p/1"]);
        let working = task(json!({"id":"p/1","state":"working","stage":"working"}));
        assert_eq!(board_args(&working, &req("stop")).ok().unwrap(), ["stop", "p/1"]);
        let failed = task(json!({"id":"p/1","state":"error"}));
        assert_eq!(board_args(&failed, &req("retry")).ok().unwrap(), ["retry", "p/1"]);
        let todo = task(json!({"id":"p/1","state":"idle"}));
        assert_eq!(board_args(&todo, &req("delegate")).ok().unwrap(), ["delegate", "p/1"]);
    }

    #[test]
    fn a_command_that_does_not_fit_the_stage_is_refused() {
        let working = task(json!({"id":"p/1","state":"working","stage":"working"}));
        for kind in ["approve", "anyway", "review", "retry", "delegate"] {
            let e = board_args(&working, &req(kind)).expect_err(kind);
            assert_eq!(e.0, 409, "{kind}");
        }
        let done = task(json!({"id":"p/1","state":"done"}));
        assert!(board_args(&done, &req("stop")).is_err());
        assert_eq!(board_args(&done, &req("rm")).err().unwrap().0, 400);
        assert_eq!(board_args(&done, &req("--now")).err().unwrap().0, 400);
    }

    #[test]
    fn approving_work_that_cannot_be_merged_by_a_plain_yes_is_anyway() {
        let bad = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","review":{"findings":[{"severity":"major"}],"meets":true,"pass":false}}));
        // After a failed review a plain `approve` is not offered at all.
        assert_eq!(board_args(&bad, &req("approve")).err().unwrap().0, 409);
        assert_eq!(board_args(&bad, &req("anyway")).ok().unwrap(), ["approve", "p/1", "--anyway"]);
        assert_eq!(board_args(&bad, &req("review")).ok().unwrap(), ["review", "p/1"]);
        // Checks that fail or a merge that cannot go through: only «anyway» (and the reply).
        let checks = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"approval","msg":"GitHub checks failed"}));
        assert_eq!(board_args(&checks, &req("approve")).err().unwrap().0, 409);
        assert_eq!(board_args(&checks, &req("anyway")).ok().unwrap(), ["approve", "p/1", "--anyway"]);
        // Work that looks ready can be approved anyway, too: the list does not say whether its checks pass.
        assert_eq!(board_args(&ready(), &req("approve")).ok().unwrap(), ["approve", "p/1"]);
        assert_eq!(board_args(&ready(), &req("anyway")).ok().unwrap(), ["approve", "p/1", "--anyway"]);
        // Nowhere else.
        let working = task(json!({"id":"p/1","state":"working","stage":"working"}));
        assert_eq!(board_args(&working, &req("anyway")).err().unwrap().0, 409);
        // A task a limit stopped is continued with `retry`.
        let limit = task(json!({"id":"p/1","state":"blocked","stage":"stopped"}));
        assert_eq!(board_args(&limit, &req("retry")).ok().unwrap(), ["retry", "p/1"]);
    }

    #[test]
    fn tell_sends_the_words_after_a_double_dash() {
        let t = task(json!({"id":"p/1","state":"blocked","kind":"question","stage":"working"}));
        let mut r = req("tell");
        r.text = "  --now да, делай  ".into();
        assert_eq!(board_args(&t, &r).ok().unwrap(), ["tell", "p/1", "--", "--now да, делай"]);
        r.text = "   ".into();
        assert_eq!(board_args(&t, &r).err().unwrap().0, 400);
        r.text = "я".repeat(TELL_MAX + 1);
        assert_eq!(board_args(&t, &r).err().unwrap().0, 400);
        // The plan: yes (`approve`) or words (`tell`).
        let plan = task(json!({"id":"p/1","state":"blocked","kind":"permission","stage":"plan"}));
        assert!(board_args(&plan, &req("approve")).is_ok());
        let pick = task(json!({"id":"p/1","state":"blocked","stage":"pick"}));
        assert!(pick.actions.is_empty());
    }

    #[test]
    fn details_are_trimmed_to_what_the_page_shows() {
        let d = json!({
            "task":{"id":"p/1","title":"T","stage":"approval","msg":"","description":"d"},
            "failure":{"title":"Could not open the pull request","detail":"x","next":"y","at":1,"sentAt":2},
            "review":{"findings":[{"id":"F1","severity":"minor","subject":"s","where":"a.rs:1","proof":"","preexisting":false,"secret":"no"}],"meets":true,"pass":true,"summary":"ok"},
            "reviews":[{},{}],
            "agent":"claude","research":false,"report":null,"plan":null,
            "pr":"https://github.com/o/r/pull/3","worktree":"/w","branch":"work/1",
            "limits":{"time":{"limit":"none","used":"19s"}},
            "handovers":[{"big":"x"}]
        });
        let t = trim_details(&d);
        assert_eq!(t["task"]["title"], "T");
        assert_eq!(t["failure"]["title"], "Could not open the pull request");
        assert_eq!(t["review"]["tally"], "1 minor");
        assert_eq!(t["review"]["pass"], true);
        assert_eq!(t["review"]["findings"][0]["where"], "a.rs:1");
        assert!(t["review"]["findings"][0].get("secret").is_none());
        assert_eq!(t["rounds"], 2);
        assert_eq!(t["branch"], "work/1");
        assert!(t.get("handovers").is_none());
        // A long plan is cut.
        let long = "я".repeat(7000);
        let t = trim_details(&json!({"plan":{"text":long}}));
        assert_eq!(t["plan"]["text"].as_str().unwrap().chars().count(), 6001);
        assert_eq!(trim_details(&json!({}))["review"], Value::Null);
    }

    #[test]
    fn the_herdr_socket_gets_one_line_and_answers_one() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("herdr-planner-sock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("h.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (n, conn) in listener.incoming().take(2).enumerate() {
                let mut conn = conn.unwrap();
                let mut line = String::new();
                BufReader::new(conn.try_clone().unwrap()).read_line(&mut line).unwrap();
                seen.push(line);
                let reply = if n == 0 {
                    r#"{"id":"herdr-planner","result":{"type":"agent_view","active":false}}"#
                } else {
                    r#"{"id":"herdr-planner","error":{"code":"bad","message":"nope"}}"#
                };
                conn.write_all(format!("{reply}\n").as_bytes()).unwrap();
            }
            seen
        });
        let ok = socket_request(&sock, "agent.view.clear", &json!({})).unwrap();
        assert_eq!(ok["result"]["active"], false);
        assert_eq!(socket_request(&sock, "agent.view.clear", &json!({})).unwrap_err(), "herdr: nope");
        let seen = server.join().unwrap();
        let sent: Value = serde_json::from_str(seen[0].trim()).unwrap();
        assert_eq!((sent["method"].as_str(), sent["params"].clone()), (Some("agent.view.clear"), json!({})));
        assert!(socket_request(&dir.join("none.sock"), "ping", &json!({})).unwrap_err().starts_with("herdr не отвечает"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
