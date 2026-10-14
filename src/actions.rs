//! What the card does with a task. TASKS.md is never edited here: «Решить»
//! and «Не решать» go to the project's coordinator as one sentence
//! (`herdr-projects coordinator prompt`), the coordinator being the only
//! writer of TASKS.md. «Задать вопрос» goes to the cheap read-only agent and
//! its answers are kept in the saved state.

use crate::agent::{self, Kind};
use crate::app::Shared;
use crate::cards::{argv_projects, valid_slug};
use crate::sources::project_md;
use crate::sources::runner;
use crate::sources::tasks::{self, Task};
use crate::store::{mem_key, QaEntry};
use serde_json::{json, Value};
use std::time::Duration;

const PROMPT_TIMEOUT: Duration = Duration::from_secs(15);
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_secs(2);

/// What a request failed with: an HTTP status and the text the page shows.
pub type Failure = (u16, String);

fn fail<T>(status: u16, text: impl Into<String>) -> Result<T, Failure> {
    Err((status, text.into()))
}

// ---------------------------------------------------------------------------
// Texts

/// Earlier answered questions, oldest first (`list` holds the newest first).
pub fn qa_lines(list: &[QaEntry], bytes: usize) -> Vec<String> {
    list.iter()
        .rev()
        .filter_map(|e| {
            let a = e.a.as_ref()?;
            let a = a.split_whitespace().collect::<Vec<_>>().join(" ");
            Some(format!("В: {} — О: {}", e.q, tasks::clip(&a, bytes)))
        })
        .collect()
}

/// «Решить»: the sentence the coordinator gets.
pub fn solve_text(task: &str, profile: Option<&str>, qa: &[String]) -> String {
    let mut text = format!("(из Планировщика проектов) Реши задачу из TASKS.md: «{task}». ");
    match profile.filter(|p| !p.is_empty()) {
        Some(p) => text.push_str(&format!(
            "Делегируй её треду с профилем {p} и отметь в TASKS.md."
        )),
        None => {
            text.push_str("Делегируй её треду (или сделай сам, если она твоя) и отметь в TASKS.md.")
        }
    }
    if !qa.is_empty() {
        text.push_str(&format!(
            " Учти уточнения пользователя (вопрос — ответ): {}",
            qa.join(" | ")
        ));
    }
    text
}

/// «Не решать» / «Убрать»: the coordinator is asked to remove the line. A
/// freshness verdict (word, reason) goes along when that is where it came from.
pub fn remove_text(task: &str, verdict: Option<(&str, &str)>, qa: &[String]) -> String {
    let mut text = format!(
        "(из Планировщика проектов) Пользователь отказался от задачи «{task}»: удали её из TASKS.md."
    );
    if let Some((word, why)) = verdict {
        text.push_str(&format!(" Проверка актуальности отметила её как «{word}»"));
        if !why.is_empty() {
            text.push_str(&format!(": {why}"));
        }
        text.push('.');
    }
    if !qa.is_empty() {
        text.push_str(&format!(
            " Уточнения по задаче (вопрос — ответ): {}",
            qa.join(" | ")
        ));
    }
    text
}

const ASK_SYSTEM: &str = "Ты отвечаешь на короткий вопрос пользователя о задаче проекта herdr-projects из Планировщика проектов. \
Ты НЕ координатор: ничего не меняй и не запускай, только читай файлы проекта \
(PROJECT.md, TASKS.md, threads/*.md, library/) и его репозитория и отвечай. Ответ по-русски, кратко (до ~8 строк), markdown можно.";

/// The question to the cheap agent: the task, its notes, the repositories,
/// what was asked before.
pub fn ask_prompt(
    slug: &str,
    task: &Task,
    repos: &[crate::model::Repo],
    earlier: &[String],
    question: &str,
) -> String {
    let mut prompt = format!("Проект: {slug}. Задача из TASKS.md: «{}».", task.full);
    if !task.notes.is_empty() {
        prompt.push_str(&format!("\nЗаметки к задаче в TASKS.md:\n{}", task.notes));
    }
    if repos.len() == 1 && repos[0].machine.is_empty() {
        prompt.push_str(&format!("\nРепозиторий проекта: {}", repos[0].path));
    } else if !repos.is_empty() {
        prompt.push_str("\nРепозитории проекта:");
        for r in repos {
            if r.machine.is_empty() {
                prompt.push_str(&format!("\n- {}", r.path));
            } else {
                prompt.push_str(&format!(
                    "\n- {} (на машине {}, здесь недоступен)",
                    r.path, r.machine
                ));
            }
        }
    }
    if !earlier.is_empty() {
        prompt.push_str(&format!("\nРанее спрашивали:\n{}", earlier.join("\n")));
    }
    prompt.push_str(&format!("\nВопрос: {question}"));
    prompt
}

// ---------------------------------------------------------------------------
// The coordinator

/// herdr-projects says a project is not up with "...; run `open SLUG` first"
/// or "...; `open SLUG` starts one".
pub fn needs_open(err: &str) -> bool {
    err.contains("`open ")
}

/// Sends one sentence to the project's coordinator.
async fn prompt_coordinator(app: &Shared, slug: &str, text: &str) -> Result<(), String> {
    let argv = argv_projects(app, &["coordinator", "prompt", slug, "--text-file", "-"]);
    match runner::run(&argv, Some(text), PROMPT_TIMEOUT).await {
        Ok(out) if out.ok() => Ok(()),
        Ok(out) => {
            let err = if out.stderr.trim().is_empty() {
                out.stdout.trim().to_string()
            } else {
                out.stderr.trim().to_string()
            };
            Err(if err.is_empty() { out.error() } else { err })
        }
        Err(e) => Err(e),
    }
}

/// «Решить» on a project that is not open in herdr: the first prompt fails
/// with a message naming `open`, so the project is opened (as «Открыть» does)
/// and the prompt is retried every two seconds until a coordinator takes it
/// or the wait runs out. `stage` is told when the step changes.
pub async fn send(
    app: &Shared,
    slug: &str,
    text: &str,
    stage: impl Fn(&str),
) -> Result<(), String> {
    match prompt_coordinator(app, slug, text).await {
        Ok(()) => return Ok(()),
        Err(e) if !needs_open(&e) => return Err(e),
        Err(_) => {}
    }
    stage("open");
    let argv = argv_projects(app, &["open", slug]);
    match runner::run(&argv, None, OPEN_TIMEOUT).await {
        Ok(out) if out.ok() => {}
        Ok(out) => {
            let why = out.error();
            return Err(format!("проект не открылся: {}", tasks::clip(&why, 200)));
        }
        Err(e) => return Err(format!("проект не открылся: {e}")),
    }
    let deadline = tokio::time::Instant::now() + app.config.open_wait;
    loop {
        match prompt_coordinator(app, slug, text).await {
            Ok(()) => return Ok(()),
            Err(e) if !needs_open(&e) => return Err(e),
            Err(_) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "координатор не запустился за {} с",
                app.config.open_wait.as_secs()
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

// ---------------------------------------------------------------------------
// Checks shared by the actions

/// The open task `text` of `slug`, as TASKS.md has it now.
async fn open_task(app: &Shared, slug: &str, text: &str) -> Result<Task, Failure> {
    if !valid_slug(slug) {
        return fail(400, "нет проекта");
    }
    let dir = app.config.project_dir(slug);
    if !dir.is_dir() {
        return fail(404, "такого проекта нет");
    }
    if text.trim().is_empty() {
        return fail(400, "нет задачи");
    }
    let Ok(content) = tokio::fs::read_to_string(dir.join("TASKS.md")).await else {
        return fail(404, "TASKS.md не читается");
    };
    match tasks::parse_tasks(&content)
        .into_iter()
        .find(|t| t.full == text && !t.done)
    {
        Some(t) => Ok(t),
        None => fail(404, "такой открытой задачи нет в TASKS.md"),
    }
}

/// Marks a task as being worked on; a second request for it is refused.
fn begin(app: &Shared, key: &str) -> Result<(), Failure> {
    let mut c = app.cache.lock().unwrap();
    if !c.running.insert(key.to_string()) {
        return fail(409, "по этой задаче уже идёт запрос");
    }
    Ok(())
}

fn end(app: &Shared, key: &str) {
    app.cache.lock().unwrap().running.remove(key);
}

fn qa_of(app: &Shared, slug: &str, task: &str) -> Vec<String> {
    let st = app.state.lock().unwrap();
    st.qa
        .get(&mem_key(slug, task))
        .map(|l| qa_lines(l, 1200))
        .unwrap_or_default()
}

fn mark_sent(app: &Shared, slug: &str, task: &str, mark: Value) {
    app.state
        .lock()
        .unwrap()
        .sent
        .insert(mem_key(slug, task), mark);
    app.save_state();
}

// ---------------------------------------------------------------------------
// The three actions

/// «Решить». Returns when the coordinator took the task; the mark «отправлено
/// координатору» is set by then, so a page that was closed meanwhile loses nothing.
pub async fn solve(
    app: &Shared,
    slug: &str,
    task: &str,
    profile: Option<&str>,
) -> Result<(), Failure> {
    let t = open_task(app, slug, task).await?;
    if let Some(p) = profile.filter(|p| !p.is_empty()) {
        if !crate::sources::profiles::valid_profile(p) {
            return fail(400, "неверное имя профиля");
        }
    }
    let key = format!("{slug}\n{}", t.full);
    begin(app, &key)?;
    let text = solve_text(&t.full, profile, &qa_of(app, slug, &t.full));
    let notify = |stage: &str| {
        app.emit(
            "solve",
            &json!({ "slug": slug, "task": t.full, "stage": stage }),
        )
    };
    let result = send(app, slug, &text, notify).await;
    end(app, &key);
    match result {
        Ok(()) => {
            mark_sent(app, slug, &t.full, Value::Bool(true));
            app.emit_snapshot();
            Ok(())
        }
        Err(e) => fail(502, e),
    }
}

/// «Не решать» (the page has asked «Убрать из бэклога?» already): the
/// coordinator is asked to remove the line. A task a thread works on stays.
pub async fn remove(
    app: &Shared,
    slug: &str,
    task: &str,
    with_verdict: bool,
) -> Result<(), Failure> {
    let t = open_task(app, slug, task).await?;
    if t.thread_led {
        return fail(409, "задачу ведёт тред, из Планировщика её не убрать");
    }
    let verdict = if with_verdict {
        crate::fresh::view(app, slug, &t.full).0
    } else {
        None
    };
    let key = format!("{slug}\n{}", t.full);
    begin(app, &key)?;
    let word = verdict.as_ref().map(|v| crate::fresh::verdict_word(&v.v));
    let text = remove_text(
        &t.full,
        verdict.as_ref().zip(word).map(|(v, w)| (w, v.why.as_str())),
        &qa_of(app, slug, &t.full),
    );
    let result = prompt_coordinator(app, slug, &text).await;
    end(app, &key);
    match result {
        Ok(()) => {
            mark_sent(app, slug, &t.full, Value::String("remove".into()));
            app.emit_snapshot();
            Ok(())
        }
        Err(e) => fail(502, e),
    }
}

/// «Задать вопрос»: the entry is saved as pending at once, the answer
/// arrives in the background and a `card` event tells the page.
pub async fn ask(app: &Shared, slug: &str, task: &str, question: &str) -> Result<(), Failure> {
    let t = open_task(app, slug, task).await?;
    let q = question.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        return fail(400, "пустой вопрос");
    }
    if q.chars().count() > 2000 {
        return fail(400, "вопрос длиннее 2000 знаков");
    }
    let dir = app.config.project_dir(slug);
    let repos = tokio::fs::read_to_string(dir.join("PROJECT.md"))
        .await
        .map(|c| project_md::parse(&c).repos)
        .unwrap_or_default();
    let (kind, earlier) = {
        let mut st = app.state.lock().unwrap();
        let kind = Kind::from_setting(&st.settings.agent);
        let list = st.qa_list(slug, &t.full);
        let earlier = qa_lines(list, 800);
        list.insert(
            0,
            QaEntry {
                q: q.clone(),
                pending: true,
                ..Default::default()
            },
        );
        (kind, earlier)
    };
    app.save_state();
    let req = agent::Request {
        system: ASK_SYSTEM.into(),
        prompt: ask_prompt(slug, &t, &repos, &earlier, &q),
        cwd: dir.display().to_string(),
        dirs: repos
            .iter()
            .filter(|r| r.machine.is_empty() && std::path::Path::new(&r.path).is_dir())
            .map(|r| r.path.clone())
            .collect(),
    };
    let (app2, slug2, full) = (app.clone(), slug.to_string(), t.full.clone());
    tokio::spawn(async move {
        let result = agent::run(kind, &req).await;
        {
            let mut st = app2.state.lock().unwrap();
            let list = st.qa_list(&slug2, &full);
            if let Some(e) = list.iter_mut().find(|e| e.pending && e.q == q) {
                e.pending = false;
                match result {
                    Ok(text) => e.a = Some(text),
                    Err(err) => e.error = Some(err),
                }
            }
        }
        app2.save_state();
        app2.emit("card", &json!({ "slug": slug2 }));
    });
    app.emit("card", &json!({ "slug": slug }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Repo;

    fn entry(q: &str, a: Option<&str>) -> QaEntry {
        QaEntry {
            q: q.into(),
            a: a.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn solve_sentence() {
        let plain = solve_text("Этап 2", None, &[]);
        assert_eq!(
            plain,
            "(из Планировщика проектов) Реши задачу из TASKS.md: «Этап 2». Делегируй её треду (или сделай сам, если она твоя) и отметь в TASKS.md."
        );
        let with = solve_text("Этап 2", Some("claude-sonnet"), &["В: а? — О: б".into()]);
        assert!(with.contains("Делегируй её треду с профилем claude-sonnet и отметь в TASKS.md."));
        assert!(with.ends_with("Учти уточнения пользователя (вопрос — ответ): В: а? — О: б"));
        assert!(!solve_text("x", Some(""), &[]).contains("профилем"));
    }

    #[test]
    fn remove_sentence() {
        let t = remove_text("Старое", None, &[]);
        assert_eq!(
            t,
            "(из Планировщика проектов) Пользователь отказался от задачи «Старое»: удали её из TASKS.md."
        );
        assert!(
            remove_text("Старое", None, &["В: q — О: a".into()]).contains("Уточнения по задаче")
        );
        let v = remove_text(
            "Старое",
            Some(("⊘ сделано", "есть в src/a.rs")),
            &["В: q — О: a".into()],
        );
        assert!(v.contains("Проверка актуальности отметила её как «⊘ сделано»: есть в src/a.rs. Уточнения по задаче"));
        assert!(remove_text("Старое", Some(("◐ частично", "")), &[]).ends_with("как «◐ частично»."));
    }

    #[test]
    fn answered_questions_oldest_first() {
        let list = vec![
            entry("новый?", Some("да\nточно")),
            entry("без ответа", None),
            entry("старый?", Some("нет")),
        ];
        assert_eq!(
            qa_lines(&list, 800),
            ["В: старый? — О: нет", "В: новый? — О: да точно"]
        );
        let long = vec![entry("q", Some(&"я".repeat(50)))];
        assert!(qa_lines(&long, 10)[0].ends_with('…'));
    }

    #[test]
    fn ask_prompt_lists_repos_notes_and_history() {
        let task = Task {
            idx: 1,
            done: false,
            full: "Сделать X".into(),
            owner: None,
            thread: None,
            text: "Сделать X".into(),
            notes: "Детали: так и так.".into(),
            thread_led: false,
        };
        let one = ask_prompt(
            "proj",
            &task,
            &[Repo {
                path: "/r".into(),
                machine: String::new(),
            }],
            &[],
            "Что нужно?",
        );
        assert!(one.starts_with("Проект: proj. Задача из TASKS.md: «Сделать X»."));
        assert!(one.contains("Заметки к задаче в TASKS.md:\nДетали: так и так."));
        assert!(one.contains("Репозиторий проекта: /r"));
        assert!(one.ends_with("Вопрос: Что нужно?"));
        let many = ask_prompt(
            "proj",
            &task,
            &[
                Repo {
                    path: "/a".into(),
                    machine: String::new(),
                },
                Repo {
                    path: "/b".into(),
                    machine: "mac".into(),
                },
            ],
            &["В: q — О: a".into()],
            "?",
        );
        assert!(many.contains("Репозитории проекта:\n- /a\n- /b (на машине mac, здесь недоступен)"));
        assert!(many.contains("Ранее спрашивали:\nВ: q — О: a"));
    }

    /// An App whose `herdr-projects` is a script that logs the prompts it gets
    /// and refuses them until `open` has run (unless `never_opens`).
    struct Fake {
        dir: std::path::PathBuf,
        app: Shared,
    }

    fn fake(name: &str, never_opens: bool, wait_secs: u64) -> Fake {
        let dir = std::env::temp_dir().join(format!("hp-actions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("root/alpha")).unwrap();
        let script = format!(
            "#!/bin/sh\nshift 2\ncase \"$1\" in\n coordinator)\n  if [ -f '{d}/opened' ]; then cat >> '{d}/prompts.log'; echo >> '{d}/prompts.log'; exit 0; fi\n  cat >/dev/null; echo \"no coordinator of alpha can take a prompt right now; \\`open alpha\\` starts one\" >&2; exit 1;;\n open) echo \"$@\" >> '{d}/opens.log'; {open} exit 0;;\nesac\nexit 2\n",
            d = dir.display(),
            open = if never_opens { String::new() } else { format!("touch '{}/opened';", dir.display()) },
        );
        let bin = dir.join("hp");
        std::fs::write(&bin, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = crate::config::AppConfig {
            port: 0,
            root: dir.join("root"),
            state_file: dir.join("state.json"),
            config_file: dir.join("config.toml"),
            herdr_projects: bin.display().to_string(),
            herdr: "herdr".into(),
            open_wait: Duration::from_secs(wait_secs),
            fresh_debounce: Duration::from_secs(60),
        };
        let app = crate::app::App::new(cfg, crate::store::State::default(), crate::theme::load());
        Fake { dir, app }
    }

    #[tokio::test]
    async fn send_opens_a_project_that_is_not_open_and_retries() {
        let f = fake("open", false, 20);
        let stages = std::sync::Mutex::new(Vec::<String>::new());
        send(&f.app, "alpha", "сделай это", |s| {
            stages.lock().unwrap().push(s.into())
        })
        .await
        .unwrap();
        assert_eq!(*stages.lock().unwrap(), ["open"]);
        let prompts = std::fs::read_to_string(f.dir.join("prompts.log")).unwrap();
        assert_eq!(prompts.trim(), "сделай это");
        let opens = std::fs::read_to_string(f.dir.join("opens.log")).unwrap();
        assert!(opens.trim().ends_with("open alpha"), "{opens}");
        // An open project takes the prompt at once, no `open`.
        let stages2 = std::sync::Mutex::new(Vec::<String>::new());
        send(&f.app, "alpha", "ещё", |s| {
            stages2.lock().unwrap().push(s.into())
        })
        .await
        .unwrap();
        assert!(stages2.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[tokio::test]
    async fn send_gives_up_when_the_coordinator_never_starts() {
        let f = fake("never", true, 1);
        let err = send(&f.app, "alpha", "x", |_| {}).await.unwrap_err();
        assert_eq!(err, "координатор не запустился за 1 с");
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[tokio::test]
    async fn solve_marks_the_task_and_refuses_unknown_ones() {
        let f = fake("solve", false, 20);
        std::fs::write(
            f.dir.join("root/alpha/TASKS.md"),
            "- [ ] Первая\n- [ ] Вторая · t-0003\n- [x] Готовая\n",
        )
        .unwrap();
        solve(&f.app, "alpha", "Первая", Some("claude-sonnet"))
            .await
            .unwrap();
        let prompts = std::fs::read_to_string(f.dir.join("prompts.log")).unwrap();
        assert!(prompts.contains("Реши задачу из TASKS.md: «Первая»"));
        assert!(prompts.contains("с профилем claude-sonnet"));
        assert_eq!(
            f.app.state.lock().unwrap().sent.get("alpha\nПервая"),
            Some(&Value::Bool(true))
        );
        // Wrong input: no such task, a done task, a bad profile, a bad project.
        assert_eq!(
            solve(&f.app, "alpha", "Нет такой", None)
                .await
                .unwrap_err()
                .0,
            404
        );
        assert_eq!(
            solve(&f.app, "alpha", "Готовая", None).await.unwrap_err().0,
            404
        );
        assert_eq!(
            solve(&f.app, "alpha", "Первая", Some("-x y"))
                .await
                .unwrap_err()
                .0,
            400
        );
        assert_eq!(
            solve(&f.app, "../etc", "Первая", None).await.unwrap_err().0,
            400
        );
        assert_eq!(
            solve(&f.app, "nope", "Первая", None).await.unwrap_err().0,
            404
        );
        // A task a thread works on is not given up from the card.
        assert_eq!(
            remove(&f.app, "alpha", "Вторая · t-0003", false)
                .await
                .unwrap_err()
                .0,
            409
        );
        remove(&f.app, "alpha", "Первая", false).await.unwrap();
        assert_eq!(
            f.app.state.lock().unwrap().sent.get("alpha\nПервая"),
            Some(&Value::String("remove".into()))
        );
        // Questions: empty and too long are refused before anything starts.
        assert_eq!(
            ask(&f.app, "alpha", "Первая", "  ").await.unwrap_err().0,
            400
        );
        assert_eq!(
            ask(&f.app, "alpha", "Первая", &"я".repeat(2001))
                .await
                .unwrap_err()
                .0,
            400
        );
        assert!(f.app.state.lock().unwrap().qa.is_empty());
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn open_errors_are_recognised() {
        assert!(needs_open(
            "coordinator for x is not running; run `open x` first"
        ));
        assert!(needs_open("no coordinator pane; `open x` starts one"));
        assert!(!needs_open("unknown project x"));
    }
}
