//! The phone's API: what the projection keeps, what the action route lets through, the long-poll, and the routes as a
//! device meets them through an open gate.

use super::*;
use crate::board::Status;
use crate::config::AppConfig;
use crate::devices::{Who, COOKIE};
use crate::runners::RunnerState;
use crate::runners_alert::{Alert, Kind};
use crate::testutil::{test_cfg, write_exec};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A `herdr-projects` that writes down the commands it is given (the reads of a refresh are not written) and
/// answers `details` with a task that has places in it.
fn fake_board(dir: &Path) -> (AppConfig, PathBuf) {
    let log = dir.join("board.log");
    let script = dir.join("herdr-projects");
    let details = r#"{"task":{"id":"alpha/1","title":"Сделать","stage":"working","msg":"идёт","description":"текст","session":"SECRET-SESSION","worktree":"/home/SECRET-TREE"},"failure":{"title":"упало","detail":"log","worktree":"/home/SECRET-TREE"},"review":{"pass":true,"findings":[{"severity":"minor","file":"src/a.rs","text":"x"}]},"reviews":[{"worktree":"/home/SECRET-TREE","round":1}],"agent":"claude","worktree":"/home/SECRET-TREE","branch":"work/1","after":{"waitsFor":[]},"limits":{},"plan":{"text":"1. a\n2. b","by":"claude/opus"},"report":{"url":"http://127.0.0.1:34925/alpha/1/review.html"},"pr":"https://example.org/pr/1","attempts":[{"letter":"A","state":"done","worktree":"/home/SECRET-TREE/a"}],"comparison":null,"picked":null}"#;
    write_exec(
        &script,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  list|show) exit 0 ;;\n  coordinator) [ \"$2\" = tell ] || exit 0 ;;\n  details) cat <<'EOF'\n{details}\nEOF\n    exit 0 ;;\nesac\necho \"$@\" >> {}\n",
            log.display()
        ),
    );
    (AppConfig { herdr_projects: script.display().to_string(), ..test_cfg(dir) }, log)
}

fn said(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

fn task_named(id: &str, status: Status, actions: Vec<&'static str>) -> Task {
    let (repo, n) = id.split_once('/').unwrap();
    Task {
        id: id.into(),
        repo: repo.into(),
        n: n.parse().unwrap(),
        tag: format!("#{n}"),
        title: "Сделать".into(),
        status,
        stage: "working".into(),
        actions,
        session: "SECRET-SESSION".into(),
        worktree: "/home/SECRET-TREE".into(),
        description: "текст задачи".into(),
        subagents: vec![crate::subagents::Subagent { result: "SECRET-SUBAGENT".into(), ..Default::default() }],
        subagents_note: "SECRET-NOTE".into(),
        ..Default::default()
    }
}

/// A snapshot with a sentinel in every place the phone must not see, and the usual things in the places it must.
async fn loaded_app(dir: &Path) -> Shared {
    let (cfg, _) = fake_board(dir);
    let app = app::App::new(AppConfig { password_hash: "$argon2id$x".into(), user: "SECRET-USER".into(), ..cfg });
    {
        let mut s = app.snapshot.write().await;
        s.tasks = vec![task_named("alpha/1", Status::Plan, vec!["approve", "anyway", "stop", "answer", "tell", "yazi"])];
        s.archived = vec![task_named("alpha/9", Status::Done, vec!["archive"])];
        s.projects = vec![Project {
            name: "alpha".into(),
            key: "ALP".into(),
            root: "/home/SECRET-ROOT".into(),
            folder: "/home/SECRET-FOLDER".into(),
            origin: "git@SECRET-ORIGIN".into(),
            lag: Some(crate::board::ProjectLag { origin: "SECRET-LAG".into(), main: "main".into(), behind: 3, ..Default::default() }),
            ..Default::default()
        }];
        s.todo = vec![app::TodoItem { id: "t1".into(), text: "SECRET-TODO".into(), ..Default::default() }];
        s.journal = vec![app::JournalEntry { n: 1, t: 1, id: "alpha/1".into(), s: "plan".into(), text: "SECRET-JOURNAL".into() }];
        s.runners = vec![
            RunnerState {
                id: "yard".into(),
                target: "SECRET-TARGET".into(),
                ports_host: "SECRET-PORTS".into(),
                note: "SECRET-RUNNER-NOTE".into(),
                ok: true,
                alerts: vec![Alert { key: "yard:disk".into(), runner: "yard".into(), kind: Kind::Disk, free_pct: Some(4), ..Default::default() }],
                ..Default::default()
            },
            RunnerState { id: "quiet".into(), target: "SECRET-QUIET".into(), ok: true, ..Default::default() },
        ];
        s.runner_picks.insert("alpha/1".into(), crate::runners::Pick { runner: "SECRET-PICK".into(), ..Default::default() });
        s.herdr.socket = "/run/SECRET-SOCKET".into();
        s.server.projects_dir = "/home/SECRET-PROJECTS".into();
        s.server.attachments_dir = "/home/SECRET-ATTACH".into();
        s.server.listen = "SECRET-LISTEN".into();
        s.theme.source = "/home/SECRET-THEME".into();
        s.theme.error = "SECRET-THEME-ERROR".into();
        s.coordinator.source = "SECRET-SOURCE".into();
        s.coordinator.view.error = "/usr/bin/SECRET-BIN: not found".into();
        s.coordinator.view.reply = "Готово, жду тебя".into();
        s.coordinator.can_compact = true;
        s.coordinator.subagents = vec![crate::subagents::Subagent { result: "SECRET-MOON".into(), ..Default::default() }];
        s.coordinator.subagents_note = "SECRET-MOON-NOTE".into();
        s.coordinator.history = vec![app::ChatMessage {
            text: "привет".into(),
            at: 5,
            attachments: vec![crate::attachments::Attachment { id: "SECRET-FILE.png".into(), name: "SECRET-NAME".into(), mime: "image/png".into(), size: 1 }],
        }];
        s.board = BoardStatus { ok: false, error: "herdr-projects: /home/SECRET-BIN failed".into(), stale_at: 7 };
    }
    app
}

#[tokio::test]
async fn the_projection_leaves_out_what_the_phone_has_no_use_for() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path()).await;
    let cur = app.mobile.current(&app).await;
    assert!(!cur.body.contains("SECRET"), "{}", cur.body.split("SECRET").nth(1).map(|t| format!("SECRET{}", &t[..t.len().min(40)])).unwrap_or_default());
    let v: Value = serde_json::from_str(&cur.body).unwrap();
    // …and what the page draws from is there
    assert_eq!(v["tasks"][0]["id"], "alpha/1");
    assert_eq!(v["tasks"][0]["title"], "Сделать");
    assert_eq!(v["tasks"][0]["description"], "текст задачи");
    assert_eq!(v["tasks"][0]["tag"], "#1");
    assert_eq!(v["host"], json!(app::hostname()));
    assert_eq!(v["coordinator"]["reply"], "Готово, жду тебя");
    assert_eq!(v["coordinator"]["history"][0]["text"], "привет");
    assert_eq!(v["coordinator"]["history"][0]["attachments"], json!([]));
    assert_eq!(v["projects"][0]["name"], "alpha");
    assert_eq!(v["projects"][0]["key"], "ALP");
    assert_eq!(v["runners"].as_array().unwrap().len(), 1, "only a yard with an alarm");
    assert_eq!(v["runners"][0]["id"], "yard");
    assert_eq!(v["runners"][0]["alerts"][0]["key"], "yard:disk");
    assert_eq!(v["board"]["ok"], false);
    assert_eq!(v["coordinator"]["can_compact"], false, "no compacting from a phone");
    assert_eq!(v["coordinator"]["error"], "no answer from the Coordinator", "the board's own words name its programs");
    // the lists the phone has no screen for are there, empty, so the page's store reads the same shape
    for k in ["archived", "todo", "journal", "memory_suggestions"] {
        assert_eq!(v[k], json!([]), "{k}");
    }
    for k in ["runner_picks", "runner_repo_picks"] {
        assert_eq!(v[k], json!({}), "{k}");
    }
    assert_eq!(v["auth"], json!({"required": false, "user": ""}));
}

#[tokio::test]
async fn a_task_keeps_the_actions_the_phone_has() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path()).await;
    let snap = app.snapshot.read().await.clone();
    let p = project(&snap);
    // approve, tell and answer stay; anyway, stop and the folder do not
    assert_eq!(p.tasks[0].actions, vec!["approve", "answer", "tell"]);
    let t = task_named("alpha/2", Status::Failed, vec!["retry", "tell", "stop"]);
    assert_eq!(super::task(&t).actions, vec!["retry", "tell"]);
    assert_eq!(super::task(&task_named("alpha/3", Status::Review, vec!["review", "anyway"])).actions, Vec::<&str>::new());
}

#[tokio::test]
async fn the_projections_number_moves_with_what_the_phone_draws_and_not_with_the_clock() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path()).await;
    let first = app.mobile.current(&app).await;
    // a refresh that found nothing new: the snapshot's own counter and time move, the projection's number does not
    {
        let mut s = app.snapshot.write().await;
        s.seq += 5;
        s.at += 60_000;
    }
    let again = app.mobile.current(&app).await;
    assert_eq!(first.seq, again.seq);
    let v: Value = serde_json::from_str(&again.body).unwrap();
    assert_eq!(v["seq"], json!(again.seq), "the page is told the projection's number");
    // the limits are read again at every refresh and stamped with the time of the read
    app.snapshot.write().await.limits.at += 1;
    assert_eq!(app.mobile.current(&app).await.seq, first.seq);
    // something the phone shows changed
    app.snapshot.write().await.tasks[0].msg = "ждёт тебя".into();
    let changed = app.mobile.current(&app).await;
    assert_eq!(changed.seq, first.seq + 1);
    // something it does not show changed
    app.snapshot.write().await.tasks[0].worktree = "/elsewhere".into();
    app.snapshot.write().await.todo.clear();
    assert_eq!(app.mobile.current(&app).await.seq, changed.seq);
    // the Coordinator's transcript grew: the page must read the history again
    app.bus.send("chat", "1".into());
    let grown = app.mobile.current(&app).await;
    assert_eq!(grown.seq, changed.seq + 1);
    assert_eq!(serde_json::from_str::<Value>(&grown.body).unwrap()["chat_rev"], 1);
}

#[tokio::test]
async fn the_number_starts_from_the_clock_so_a_restart_is_noticed() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path()).await;
    let seq = app.mobile.current(&app).await.seq;
    let secs = now_ms() / 1000;
    assert!(seq >= secs - 5 && seq <= secs + 5, "{seq} against {secs}");
    // a phone that kept a small number from an older planner is told at once
    assert!(poll_for(&app, Some(7), Duration::from_secs(5)).await.status().is_success());
}

#[tokio::test]
async fn the_poll_answers_at_once_for_another_number_and_waits_for_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path()).await;
    let seq = app.mobile.current(&app).await.seq;
    // no number, or another one: the projection now
    for q in [None, Some(0), Some(seq + 1)] {
        let res = poll_for(&app, q, Duration::from_secs(5)).await;
        assert_eq!(res.status(), StatusCode::OK, "{q:?}");
    }
    // the same number: nothing new, after the wait
    let t = Instant::now();
    let res = poll_for(&app, Some(seq), Duration::from_millis(150)).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(t.elapsed() >= Duration::from_millis(140));
    // pulses that change nothing the phone shows do not end the wait
    let app2 = app.clone();
    let noise = tokio::spawn(async move {
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            app2.bus.send("load", "{}".into());
            app::publish(&app2).await;
        }
    });
    let res = poll_for(&app, Some(seq), Duration::from_millis(250)).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    noise.await.unwrap();
}

#[tokio::test]
async fn the_poll_wakes_when_the_projection_changes_even_with_no_event_stream_open() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path()).await;
    let seq = app.mobile.current(&app).await.seq;
    assert_eq!(app.bus.subscribers(), 0);
    let app2 = app.clone();
    let waiter = tokio::spawn(async move { poll_for(&app2, Some(seq), Duration::from_secs(20)).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!waiter.is_finished());
    app.snapshot.write().await.tasks[0].msg = "новое".into();
    app::publish(&app).await;
    let res = tokio::time::timeout(Duration::from_secs(5), waiter).await.expect("woke").unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 22).await.unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["tasks"][0]["msg"], "новое");
    assert_eq!(v["seq"], json!(seq + 1));
}

#[test]
fn details_keep_the_card_and_lose_the_places() {
    let dir = tempfile::tempdir().unwrap();
    let (cfg, _) = fake_board(dir.path());
    let out = std::process::Command::new(&cfg.herdr_projects).args(["details", "alpha/1"]).output().unwrap();
    let d = crate::board::parse_details(&String::from_utf8_lossy(&out.stdout), "/home/SECRET-REPORTS").unwrap();
    assert_eq!(d.worktree, "/home/SECRET-TREE");
    let v = details_of(&d);
    let text = v.to_string();
    assert!(!text.contains("SECRET") && !text.contains("127.0.0.1"), "{text}");
    assert_eq!(v["plan"]["text"], "1. a\n2. b");
    assert_eq!(v["review"]["findings"][0]["file"], "src/a.rs", "a finding names a file of the repository, which is the point");
    assert_eq!(v["task"]["title"], "Сделать");
    assert_eq!(v["pr"], "https://example.org/pr/1");
    assert_eq!(v["branch"], "work/1");
    assert_eq!(v["attempts"][0]["letter"], "A");
    assert!(v.get("worktree").is_none() && v.get("reports_dir").is_none() && v.get("report").is_none());
}

// ---- the routes, on a real socket, through an open gate ----

const ADDRESS: &str = "abc-def.trycloudflare.com";

async fn serve(app: Shared) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = crate::server::router(app);
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.ok();
    });
    addr
}

/// One request with an optional JSON body: the status, the headers' text and the body of the answer.
async fn ask(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> (u16, String, String) {
    let body = body.unwrap_or("");
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut raw)).await.unwrap().ok();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (status, head.to_string(), body.to_string())
}

struct Gate {
    _dir: tempfile::TempDir,
    app: Shared,
    addr: SocketAddr,
    log: PathBuf,
    /// The headers of a request through Cloudflare from a device that is in.
    device: Vec<(&'static str, String)>,
}

impl Gate {
    async fn open(password: bool) -> Gate {
        let dir = tempfile::tempdir().unwrap();
        let (cfg, log) = fake_board(dir.path());
        let cfg = AppConfig { password_hash: if password { "$argon2id$x".into() } else { String::new() }, tunnel_confirm_devices: false, ..cfg };
        let app = app::App::new(cfg);
        {
            let mut s = app.snapshot.write().await;
            let mut plan = task_named("alpha/1", Status::Plan, vec!["approve", "tell"]);
            plan.stage = "plan".into();
            s.tasks = vec![plan];
        }
        app.gate.set_tunnel(Some(ADDRESS));
        let now = now_ms();
        app.devices.begin_at(now);
        let key = app.devices.link_at(now).key.unwrap();
        let token = app.devices.redeem_at(now, "-", &key, Who { label: "iPhone · Safari 17".into(), country: None, touch: true }).unwrap().token;
        let addr = serve(app.clone()).await;
        let device = vec![("Host", ADDRESS.to_string()), ("Cf-Ray", "x".into()), ("Cf-Visitor", "{\"scheme\":\"https\"}".into()), ("Cookie", format!("{COOKIE}={token}"))];
        Gate { _dir: dir, app, addr, log, device }
    }

    async fn ask(&self, method: &str, path: &str, body: Option<&str>) -> (u16, String, String) {
        let h: Vec<(&str, &str)> = self.device.iter().map(|(k, v)| (*k, v.as_str())).collect();
        ask(self.addr, method, path, &h, body).await
    }
}

#[tokio::test]
async fn a_device_that_is_in_reads_the_projection() {
    let g = Gate::open(false).await;
    let (status, head, body) = g.ask("GET", "/m/api/state", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(head.to_lowercase().contains("application/json") && head.to_lowercase().contains("no-store"), "{head}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["tasks"][0]["id"], "alpha/1");
    assert!(!body.contains("SECRET"));
    // the planner's own state is another matter
    let (status, _, _) = g.ask("GET", "/api/state", None).await;
    assert_eq!(status, 403, "the light version only");
}

#[tokio::test]
async fn a_stranger_gets_nothing_from_the_phones_api() {
    let g = Gate::open(false).await;
    let none = [("Host", ADDRESS), ("Cf-Ray", "x"), ("Cf-Visitor", "{\"scheme\":\"https\"}")];
    let other = [("Host", ADDRESS), ("Cf-Ray", "x"), ("Cf-Visitor", "{\"scheme\":\"https\"}"), ("Cookie", "__Host-hp_g=00")];
    for h in [&none[..], &other[..]] {
        for (m, p) in [("GET", "/m/api/state"), ("GET", "/m/api/poll?seq=1"), ("POST", "/m/api/action"), ("POST", "/m/api/chat"), ("GET", "/m/api/session"), ("POST", "/m/api/tunnel/close"), ("POST", "/m/api/logout"), ("GET", "/m/api/details?id=alpha/1")] {
            let (status, _, body) = ask(g.addr, m, p, h, Some("{}")).await;
            assert_eq!(status, 401, "{m} {p}: {body}");
        }
    }
    assert!(said(&g.log).is_empty());
}

#[tokio::test]
async fn the_action_route_has_four_actions_and_nothing_else() {
    let g = Gate::open(false).await;
    for action in ["anyway", "stop", "archive", "restore", "fix", "finish", "review", "delegate", "after", "todo-add", "todo-create", "coordinator-new", "coordinator-compact", "keys", "yazi", "archive-done", "suggest-update", "fly", ""] {
        let body = json!({"action": action, "id": "alpha/1", "confirm": true, "keys": ["Enter"], "scope": "all", "repo": "alpha", "text": "x"}).to_string();
        let (status, _, answer) = g.ask("POST", "/m/api/action", Some(&body)).await;
        assert_eq!(status, 403, "{action}: {answer}");
    }
    assert!(said(&g.log).is_empty(), "the board was not asked: {:?}", said(&g.log));
}

#[tokio::test]
async fn an_approval_from_the_phone_names_the_phone() {
    let g = Gate::open(false).await;
    // the board's own rule stays: no confirmation, no approval
    let (status, _, body) = g.ask("POST", "/m/api/action", Some(r#"{"action":"approve","id":"alpha/1"}"#)).await;
    assert_eq!(status, 400, "{body}");
    assert!(said(&g.log).is_empty());
    let (status, _, body) = g.ask("POST", "/m/api/action", Some(r#"{"action":"approve","id":"alpha/1","confirm":true}"#)).await;
    assert_eq!(status, 200, "{body}");
    // a pick, a retry and a message
    for (b, want) in [
        (r#"{"action":"pick","id":"alpha/1","letter":"B"}"#, "pick alpha/1 B"),
        (r#"{"action":"retry","id":"alpha/1"}"#, "retry alpha/1"),
        (r#"{"action":"tell","id":"alpha/1","text":"ещё раз"}"#, "tell alpha/1 -- ещё раз"),
    ] {
        let (status, _, body) = g.ask("POST", "/m/api/action", Some(b)).await;
        assert_eq!(status, 200, "{b}: {body}");
        assert!(said(&g.log).iter().any(|l| l == want), "{want} in {:?}", said(&g.log));
    }
    // the Coordinator is told where it came from (the notice goes out in the background)
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end && !said(&g.log).iter().any(|l| l.starts_with("coordinator tell") && l.contains("(с телефона)")) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let told = said(&g.log);
    assert!(told.iter().any(|l| l.starts_with("coordinator tell") && l.contains("план одобрен (с телефона)")), "{told:?}");
    assert!(told.iter().any(|l| l.contains("approve alpha/1")), "{told:?}");
    let journal = g.app.snapshot.read().await.journal.clone();
    assert!(journal.iter().any(|e| e.text.contains("одобрено с телефона: #1")), "{journal:?}");
}

#[tokio::test]
async fn a_failed_action_leaves_no_trace_in_the_journal() {
    let g = Gate::open(false).await;
    let (status, _, _) = g.ask("POST", "/m/api/action", Some(r#"{"action":"retry","id":"-x/1"}"#)).await;
    assert_eq!(status, 400);
    assert!(g.app.snapshot.read().await.journal.is_empty());
}

#[tokio::test]
async fn the_screen_comes_without_its_colours_and_only_for_known_agents() {
    let g = Gate::open(false).await;
    let (status, _, body) = g.ask("GET", "/m/api/coordinator/screen?target=--all", None).await;
    assert_eq!(status, 400, "{body}");
    // herdr is not running here: the planner says so, in the same shape the page reads
    let (status, _, body) = g.ask("GET", "/m/api/coordinator/screen", None).await;
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["running"], false);
    assert!(v.get("ansi").is_none());
}

#[tokio::test]
async fn the_chat_takes_text_and_not_files() {
    let g = Gate::open(false).await;
    let (status, _, body) = g.ask("POST", "/m/api/chat", Some(r#"{"text":"  привет  ","attachments":["0123456789abcdef0123456789abcdef.png"]}"#)).await;
    assert_eq!(status, 200, "{body}");
    assert!(said(&g.log).iter().any(|l| l == "coordinator tell -- привет"), "{:?}", said(&g.log));
    let (status, _, _) = g.ask("POST", "/m/api/chat", Some(r#"{"text":"   "}"#)).await;
    assert_eq!(status, 400, "an empty message");
    let (status, _, _) = g.ask("POST", "/m/api/chat/attach", Some("{}")).await;
    assert_eq!(status, 404, "there is no upload on the phone's side");
    // the history comes in small bites
    let (status, _, body) = g.ask("GET", &format!("/m/api/chat/history?limit=500&q={}", "a".repeat(300)), None).await;
    assert_eq!(status, 400, "{body}");
    let (status, _, _) = g.ask("GET", "/m/api/chat/history?limit=500", None).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn the_card_of_a_task_has_no_places_in_it() {
    let g = Gate::open(false).await;
    let (status, _, body) = g.ask("GET", "/m/api/details?id=alpha/1", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(!body.contains("SECRET"), "{body}");
    assert!(body.contains("1. a"));
    let (status, _, _) = g.ask("GET", "/m/api/details?id=--all", None).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_device_knows_its_session_and_can_leave() {
    let g = Gate::open(false).await;
    let (status, _, body) = g.ask("GET", "/m/api/session", None).await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((v["kind"].as_str(), v["label"].as_str(), v["scope"].as_str()), (Some("device"), Some("iPhone · Safari 17"), Some("lite")));
    assert!(v["ends_ms"].as_u64().unwrap() > now_ms());
    assert!(v["gate_since_ms"].as_u64().is_some());
    let (status, head, _) = g.ask("POST", "/m/api/logout", Some("{}")).await;
    assert_eq!(status, 200);
    assert!(head.contains("Max-Age=0"), "{head}");
    // the cookie is no good now
    let (status, _, _) = g.ask("GET", "/m/api/state", None).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn a_cross_origin_post_is_refused() {
    let g = Gate::open(false).await;
    let mut h: Vec<(&str, &str)> = g.device.iter().map(|(k, v)| (*k, v.as_str())).collect();
    h.push(("Origin", "https://evil.example"));
    for p in ["/m/api/action", "/m/api/chat", "/m/api/logout", "/m/api/tunnel/close", "/m/api/menu/answer"] {
        let (status, _, body) = ask(g.addr, "POST", p, &h, Some(r#"{"action":"retry","id":"alpha/1","text":"x","sig":"","answer":{"kind":"cancel"}}"#)).await;
        assert_eq!(status, 403, "{p}: {body}");
    }
    assert!(said(&g.log).is_empty());
    assert!(g.app.gate.tunnel_open());
}

#[tokio::test]
async fn the_gate_can_be_closed_from_the_phone_and_the_answer_gets_out_first() {
    let g = Gate::open(false).await;
    let (status, _, body) = g.ask("POST", "/m/api/tunnel/close", Some("{}")).await;
    // there is no process behind this gate: the state is "closed", so there is nothing to close
    assert_eq!(status, 409, "{body}");
    // with a gate that is open
    g.app.tunnel.set_open_for_test(ADDRESS);
    let (status, _, body) = g.ask("POST", "/m/api/tunnel/close", Some("{}")).await;
    assert_eq!(status, 200, "{body}");
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && g.app.gate.tunnel_open() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!g.app.gate.tunnel_open());
    // every session went with it
    let (status, _, _) = g.ask("GET", "/m/api/state", None).await;
    assert_eq!(status, 503);
    let journal = g.app.snapshot.read().await.journal.clone();
    assert!(journal.iter().any(|e| e.text.contains("шлюз закрыт с телефона")), "{journal:?}");
}

#[tokio::test]
async fn from_the_computer_the_planners_own_login_decides() {
    // a password is set: the page's API wants the login, as every other route
    let g = Gate::open(true).await;
    let host = [("Host", "localhost")];
    for p in ["/m/api/state", "/m/api/session", "/m/api/poll?seq=1"] {
        let (status, _, body) = ask(g.addr, "GET", p, &host, None).await;
        assert_eq!(status, 401, "{p}: {body}");
    }
    // a device through the tunnel is judged by the gate and not by the planner's password
    let (status, _, _) = g.ask("GET", "/m/api/state", None).await;
    assert_eq!(status, 200);
    // no password: this machine reads it, and is not a device
    let g = Gate::open(false).await;
    let (status, _, body) = ask(g.addr, "GET", "/m/api/state", &host, None).await;
    assert_eq!(status, 200, "{body}");
    let (status, _, body) = ask(g.addr, "GET", "/m/api/session", &host, None).await;
    assert_eq!(status, 200);
    assert!(body.contains("\"local\""), "{body}");
    let (status, _, _) = ask(g.addr, "POST", "/m/api/logout", &host, Some("{}")).await;
    assert_eq!(status, 400, "this machine is not a device of the gate");
    // a page whose name points at this machine reaches nothing
    let (status, _, _) = ask(g.addr, "GET", "/m/api/state", &[("Host", "attacker.example")], None).await;
    assert_eq!(status, 421);
}

#[tokio::test]
async fn the_poll_does_not_keep_the_gate_alive() {
    let g = Gate::open(false).await;
    let (status, _, _) = g.ask("GET", "/m/api/poll?seq=1", None).await;
    assert_eq!(status, 200);
    // the same request on another path is use
    let before = g.app.devices.idle_closes_at().unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (status, _, _) = g.ask("GET", "/m/api/state", None).await;
    assert_eq!(status, 200);
    assert!(g.app.devices.idle_closes_at().unwrap() > before, "a read of the state is use");
    let before = g.app.devices.idle_closes_at().unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (status, _, _) = g.ask("GET", "/m/api/poll?seq=1", None).await;
    assert_eq!(status, 200);
    assert_eq!(g.app.devices.idle_closes_at().unwrap(), before, "a poll is not");
}

#[tokio::test]
async fn a_refresh_that_finds_nothing_new_does_not_end_the_poll() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path()).await;
    app::refresh(&app).await;
    let seq = app.mobile.current(&app).await.seq;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (at, limits_at) = {
        let s = app.snapshot.read().await;
        (s.at, s.limits.at)
    };
    app::refresh(&app).await;
    {
        let s = app.snapshot.read().await;
        assert!(s.at > at && s.limits.at > limits_at, "the refresh really ran: the clocks of the snapshot moved");
    }
    let res = poll_for(&app, Some(seq), Duration::from_millis(150)).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT, "nothing the phone shows has changed");
}
