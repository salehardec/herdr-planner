//! The crew's own roles over HTTP (PLN-90) and the places a role is put on a task: the page's role editor, the preview and
//! the apply of a role on an existing task, and the two hooks the task actions use (a new task from the to-do list, the
//! start of a task). The records and the commands they make are in `roles.rs`; here the board is asked.
//!
//! A role reaches the board only as its own commands, after a «было → стало» card the human confirms. The rules apply
//! when a task is added or started, as the human wrote them in the crew window.

use crate::app::{self, Shared};
use crate::board::{self, Status, Task};
use crate::crew;
use crate::roles::{self, Assignment, CustomRole, Target, Way};
use crate::server::{error, same_origin};
use crate::station::{board_run, check_agent, check_effort, models_of, parse_settings, Fail, BOARD_WAIT};
use crate::taskmeta;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// One role is put on a task at a time: two sets of commands would interleave.
static PUT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn live(t: &Task) -> bool {
    !t.archived && t.status != Status::Done
}

fn respond(r: Result<Value, Fail>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

/// A task by its id (`planner/12`) or its tag (`PLN-12`), as the page's last snapshot has it, with the branch from its record.
async fn target_of(app: &Shared, id: &str) -> Option<(Target, Task)> {
    let task = app.snapshot.read().await.tasks.iter().find(|t| t.id == id || t.tag == id).cloned()?;
    let branch = taskmeta::read_task(&app.cfg.data_dir, &task.repo, task.n).map(|m| m.branch).unwrap_or_default();
    let target = Target {
        id: task.id.clone(),
        tag: task.tag.clone(),
        title: task.title.clone(),
        repo: task.repo.clone(),
        branch,
        description: task.description.clone(),
        pr: task.pr.clone(),
    };
    Some((target, task))
}

/* ───────── the window's data ───────── */

fn sets_json(role: &CustomRole) -> Value {
    let one = |way: Way| -> Vec<Value> {
        roles::settings_of(role, way).into_iter().map(|(setting, value)| json!({"setting": setting, "value": value})).collect()
    };
    if role.kind == "worker" {
        json!({"worker": one(Way::Worker)})
    } else {
        json!({"instead": one(Way::Instead), "after": one(Way::After)})
    }
}

async fn roles_read(app: &Shared) -> Value {
    let store = roles::read_store(&app.cfg);
    let presets = crew::read_store(&app.cfg);
    let tasks: Vec<Task> = app.snapshot.read().await.tasks.clone();
    let lookup = |id: &str| -> Option<(String, String, String)> {
        tasks.iter().find(|t| t.id == id && live(t)).map(|t| (t.tag.clone(), t.title.clone(), t.status.key().to_string()))
    };
    let list: Vec<Value> = store
        .roles
        .iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("sets".into(), sets_json(r));
                o.insert("usage".into(), json!(roles::usage_of(&store, &presets, &r.id, &lookup)));
            }
            v
        })
        .collect();
    let rules: Vec<Value> = store
        .rules
        .iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("words".into(), json!(r.words()));
            }
            v
        })
        .collect();
    let suggested: Vec<Value> = roles::SUGGESTED
        .iter()
        .filter(|s| !store.roles.iter().any(|r| r.suggested == s.key))
        .map(|s| json!({"key": s.key, "name": s.name, "kind": s.kind, "how": s.how, "instructions": s.instructions}))
        .collect();
    json!({"ok": true, "roles": list, "rules": rules, "suggested": suggested, "repos": taskmeta::repos(&app.cfg.data_dir).into_iter().map(|r| r.name).collect::<Vec<_>>()})
}

/// `GET /api/crew/roles`: the human's own roles with what each sets on the board and where it stands now, the rules,
/// and the suggested roles that are not on.
pub async fn roles_get(State(app): State<Shared>) -> Response {
    Json(roles_read(&app).await).into_response()
}

#[derive(Deserialize)]
pub struct RolesBody {
    /// `add`, `update`, `delete`, `suggest`, `rule_add`, `rule_update`, `rule_delete`, `rule_up`.
    op: String,
    id: Option<String>,
    role: Option<roles::RoleInput>,
    rule: Option<roles::RuleInput>,
    /// The suggested role's key, for `suggest`.
    key: Option<String>,
    /// `delete` of a role that is in use goes through only with this.
    force: Option<bool>,
}

fn store_fail(e: String) -> Fail {
    Fail::new(422, e)
}

/// `POST /api/crew/roles`: add, change, rename (an `update` with the new name) and delete the roles and the rules. The board is
/// not asked: a role only holds values; it reaches the board when it is put on a task.
pub async fn roles_post(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<RolesBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let id_of = || body.id.clone().ok_or_else(|| Fail::new(400, "нужен id"));
    let role_of = || body.role.clone().ok_or_else(|| Fail::new(400, "нужна роль"));
    let rule_of = || body.rule.clone().ok_or_else(|| Fail::new(400, "нужно правило"));
    let result: Result<Value, Fail> = async {
        let id = match body.op.as_str() {
            "add" => {
                let input = role_of()?;
                roles::edit_store(&app.cfg, |s| roles::add_role(s, &input)).map_err(store_fail)?
            }
            "update" => {
                let (id, input) = (id_of()?, role_of()?);
                roles::edit_store(&app.cfg, |s| roles::update_role(s, &id, &input)).map_err(store_fail)?;
                id
            }
            "suggest" => {
                let key = body.key.clone().unwrap_or_default();
                roles::edit_store(&app.cfg, |s| roles::enable_suggested(s, &key)).map_err(store_fail)?
            }
            "delete" => return delete_role(&app, &id_of()?, body.force == Some(true)).await,
            "rule_add" => {
                let input = rule_of()?;
                roles::edit_store(&app.cfg, |s| roles::add_rule(s, &input)).map_err(store_fail)?
            }
            "rule_update" => {
                let (id, input) = (id_of()?, rule_of()?);
                roles::edit_store(&app.cfg, |s| roles::update_rule(s, &id, &input)).map_err(store_fail)?;
                id
            }
            "rule_delete" => {
                let id = id_of()?;
                roles::edit_store(&app.cfg, |s| roles::delete_rule(s, &id)).map_err(store_fail)?;
                id
            }
            "rule_up" => {
                let id = id_of()?;
                roles::edit_store(&app.cfg, |s| {
                    let at = s.rules.iter().position(|r| r.id == id).ok_or("такого правила нет")?;
                    if at > 0 {
                        s.rules.swap(at, at - 1);
                    }
                    Ok(())
                })
                .map_err(store_fail)?;
                id
            }
            _ => return Err(Fail::new(400, "неизвестное действие с ролью")),
        };
        app::refresh(&app).await;
        Ok(json!({"ok": true, "id": id}))
    }
    .await;
    respond(result)
}

/// A role that is used (a live task has it, a rule gives it, a preset holds it) is deleted only on a second, explicit
/// word: the answer says what uses it. Its rules and its place in the presets go with it; the tasks keep what the board
/// was told and their rows keep its name.
async fn delete_role(app: &Shared, id: &str, force: bool) -> Result<Value, Fail> {
    let store = roles::read_store(&app.cfg);
    if !store.roles.iter().any(|r| r.id == id) {
        return Err(Fail::new(404, "такой роли нет"));
    }
    let tasks: Vec<Task> = app.snapshot.read().await.tasks.clone();
    let lookup = |t: &str| -> Option<(String, String, String)> {
        tasks.iter().find(|x| x.id == t && live(x)).map(|x| (x.tag.clone(), x.title.clone(), x.status.key().to_string()))
    };
    let usage = roles::usage_of(&store, &crew::read_store(&app.cfg), id, &lookup);
    if !usage.is_empty() && !force {
        let name = store.roles.iter().find(|r| r.id == id).map(|r| r.name.clone()).unwrap_or_default();
        return Err(Fail::in_use(format!("роль «{name}» используется: подтвердите удаление"), json!(usage)));
    }
    let key = roles::preset_key(id);
    roles::edit_store(&app.cfg, |s| roles::delete_role(s, id).map(|_| ())).map_err(store_fail)?;
    crew::edit_store(&app.cfg, |s| {
        for p in s.presets.iter_mut() {
            p.roles.remove(&key);
        }
        Ok(())
    })
    .map_err(|e| Fail::new(500, e))?;
    app::refresh(app).await;
    Ok(json!({"ok": true, "id": id, "usage": usage}))
}

/* ───────── putting a role on a task ───────── */

/// Which role goes on a task, as an action says it: not said (the rules decide), `none`, or a role.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Rules,
    Nothing,
    Role(String),
}

pub fn pick_of(role: Option<&str>) -> Pick {
    match role.map(str::trim) {
        None => Pick::Rules,
        Some("" | "none") => Pick::Nothing,
        Some("auto") => Pick::Rules,
        Some(id) => Pick::Role(id.to_string()),
    }
}

/// The role chosen for a task, the way it goes on, and where the choice came from.
#[derive(Debug, Clone)]
pub struct Chosen {
    pub role: CustomRole,
    pub way: Way,
    pub by: String,
}

fn choose(store: &roles::Store, pick: &Pick, how: &str, repo: &str, title: &str, branch: &str) -> Result<Option<Chosen>, String> {
    match pick {
        Pick::Nothing => Ok(None),
        Pick::Role(id) => {
            let role = store.roles.iter().find(|r| &r.id == id).ok_or("такой роли нет")?;
            Ok(Some(Chosen { way: roles::way_of(role, how)?, role: role.clone(), by: "chosen".into() }))
        }
        Pick::Rules => Ok(roles::rule_for(store, repo, title, branch).and_then(|rule| {
            let role = store.roles.iter().find(|r| r.id == rule.role)?;
            // a rule's way is checked when the rule is made; a role changed since may no longer have it: no role then
            let way = roles::way_of(role, if how.is_empty() { &rule.how } else { how }).ok()?;
            Some(Chosen { role: role.clone(), way, by: format!("rule:{}", rule.id) })
        })),
    }
}

/// The models, efforts and spares a role names against what the programs list. An empty list checks nothing.
async fn check_role(app: &Shared, role: &CustomRole) -> Vec<String> {
    let mut problems = Vec::new();
    let mut lists: std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> = Default::default();
    let mut programs: Vec<String> = Vec::new();
    if !role.agent.is_empty() {
        programs.push(crew::program_of(&role.agent).to_string());
    }
    programs.extend(crew::chain_programs(&role.fallback));
    for p in programs {
        if crate::models::valid_program(&p) && !lists.contains_key(&p) {
            let m = models_of(app, &p).await;
            lists.insert(p, (m.names, m.effort));
        }
    }
    let empty = (Vec::new(), Vec::new());
    let of = |p: &str| lists.get(p).unwrap_or(&empty).clone();
    if !role.agent.is_empty() {
        let p = crew::program_of(&role.agent).to_string();
        if let Err(e) = check_agent(&role.agent, &of(&p).0) {
            problems.push(e);
        }
        if !role.effort.is_empty() {
            if let Err(e) = check_effort(&role.effort, &of(&p).1, &p) {
                problems.push(e);
            }
        }
    }
    if !role.fallback.is_empty() {
        if let Err(e) = crew::check_fallback(&role.fallback, &|p: &str| of(p).0) {
            problems.push(e);
        }
    }
    problems
}

#[derive(Deserialize)]
pub struct PutBody {
    /// The role's id.
    role: String,
    /// The task's id (`planner/12`) or tag.
    task: String,
    /// `worker`, `instead` or `after`; empty: the role's own.
    how: Option<String>,
    confirm: Option<bool>,
}

/// The commands that put a chosen role on an existing task, each with what it changes.
async fn commands_for(app: &Shared, chosen: &Chosen, target: &Target) -> Result<Vec<roles::Cmd>, Fail> {
    match chosen.way {
        Way::After => Ok(vec![roles::research_add(&chosen.role, target)]),
        way => {
            let out = board_run(&app.cfg, &["show".to_string(), target.id.clone(), "--json".to_string()], BOARD_WAIT).await;
            let current = if out.ok() { parse_settings(&out.stdout).unwrap_or_default() } else { Vec::new() };
            Ok(roles::set_commands(&chosen.role, way, target, &current))
        }
    }
}

fn command_row(c: &roles::Cmd) -> Value {
    let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("args");
        o.insert("command".into(), json!(c.text()));
    }
    v
}

/// `POST /api/crew/role/preview`: what putting the role on the task does, as the board's commands, with was → now.
pub async fn role_preview(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<PutBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        let store = roles::read_store(&app.cfg);
        let (target, task) = target_of(&app, &body.task).await.ok_or_else(|| Fail::new(404, "такой задачи нет"))?;
        let chosen = choose(&store, &Pick::Role(body.role.clone()), body.how.as_deref().unwrap_or(""), &target.repo, &target.title, &target.branch)
            .map_err(|e| Fail::new(422, e))?
            .ok_or_else(|| Fail::new(400, "нужна роль"))?;
        let mut cmds = commands_for(&app, &chosen, &target).await?;
        let problems = check_role(&app, &chosen.role).await;
        if let Some(first) = cmds.first_mut() {
            first.error = problems.join("; ");
        }
        let duplicate = chosen.way == Way::After && already_after(&store, &target.id, &chosen.role.id);
        let started = matches!(task.status, Status::Working | Status::Review);
        Ok::<Value, Fail>(json!({
            "ok": true,
            "role": {"id": chosen.role.id, "name": chosen.role.name, "kind": chosen.role.kind},
            "how": chosen.way.key(),
            "task": {"id": target.id, "tag": target.tag, "title": target.title, "repo": target.repo, "status": task.status.key()},
            "steps": cmds.iter().map(command_row).collect::<Vec<_>>(),
            "valid": problems.is_empty() && !duplicate,
            "duplicate": duplicate,
            "started": started,
            "nothing": cmds.is_empty(),
        }))
    }
    .await;
    respond(result)
}

fn already_after(store: &roles::Store, task: &str, role: &str) -> bool {
    store.assigned.iter().any(|a| a.task == task && a.role == role && a.how == "after")
}

/// The new task's id from the board's `<repo>/<n>: added…` (the fixture's older words: `PLN-35: added`).
pub fn added_id(out: &str, repo: &str) -> Option<String> {
    let said = out.lines().find_map(|l| l.split_once(':').map(|(t, _)| t.trim().to_string()))?;
    if said.contains('/') && board::valid_id(&said) {
        Some(said)
    } else {
        said.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok()).map(|n| format!("{repo}/{n}"))
    }
}

/// Runs the commands that put a chosen role on an existing task and records where it was put. A refusal stops the run
/// and says what was done. For an «after» role the research task is added and its id kept with both tasks.
async fn put(app: &Shared, chosen: &Chosen, target: &Target) -> Result<Value, Fail> {
    let _one_at_a_time = PUT.lock().await;
    let store = roles::read_store(&app.cfg);
    if chosen.way == Way::After && already_after(&store, &target.id, &chosen.role.id) {
        return Err(Fail::new(409, format!("«{}» уже запущена после {}", chosen.role.name, target.name())));
    }
    let problems = check_role(app, &chosen.role).await;
    if !problems.is_empty() {
        return Err(Fail::new(422, problems.join("; ")));
    }
    let cmds = commands_for(app, chosen, target).await?;
    let mut ran: Vec<String> = Vec::new();
    let mut follow = String::new();
    for c in &cmds {
        let out = board_run(&app.cfg, &c.args, BOARD_WAIT).await;
        if !out.ok() {
            app::refresh(app).await;
            let before = if ran.is_empty() { "Ничего не изменилось.".to_string() } else { format!("Уже выполнено: {}.", ran.join(", ")) };
            return Err(Fail::new(422, format!("{} не принята доской: {}. {before}", c.setting, out.words())));
        }
        if c.kind == "add" {
            follow = added_id(&out.stdout, &target.repo).unwrap_or_default();
        }
        ran.push(c.setting.clone());
    }
    note_put(app, chosen, target, &ran, &follow)?;
    app::refresh(app).await;
    Ok(json!({"ok": true, "role": chosen.role.name, "how": chosen.way.key(), "ran": ran, "follow": follow}))
}

fn note_put(app: &Shared, chosen: &Chosen, target: &Target, ran: &[String], follow: &str) -> Result<(), Fail> {
    roles::edit_store(&app.cfg, |s| {
        let base = Assignment {
            task: target.id.clone(),
            role: chosen.role.id.clone(),
            name: chosen.role.name.clone(),
            how: chosen.way.key().to_string(),
            by: chosen.by.clone(),
            at: crate::config::now_ms(),
            ..Assignment::default()
        };
        if chosen.way == Way::After {
            roles::record(s, Assignment { follow: follow.to_string(), ..base.clone() });
            if !follow.is_empty() {
                roles::record(s, Assignment { task: follow.to_string(), how: "followup".into(), of: target.id.clone(), ..base });
            }
        } else {
            roles::record(s, Assignment { settings: ran.to_vec(), ..base });
        }
        Ok(())
    })
    .map_err(|e| Fail::new(500, e))
}

/// `POST /api/crew/role/apply`: the commands of the preview, after the same checks, once the human has confirmed.
pub async fn role_apply(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<PutBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        if body.confirm != Some(true) {
            return Err(Fail::new(400, "нужно подтверждение: было → стало"));
        }
        let store = roles::read_store(&app.cfg);
        let (target, _) = target_of(&app, &body.task).await.ok_or_else(|| Fail::new(404, "такой задачи нет"))?;
        let chosen = choose(&store, &Pick::Role(body.role.clone()), body.how.as_deref().unwrap_or(""), &target.repo, &target.title, &target.branch)
            .map_err(|e| Fail::new(422, e))?
            .ok_or_else(|| Fail::new(400, "нужна роль"))?;
        put(&app, &chosen, &target).await
    }
    .await;
    respond(result)
}

#[derive(Deserialize)]
pub struct MatchBody {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    branch: String,
}

/// `POST /api/crew/role/match`: which role the rules would give a task with this repository, title and branch.
pub async fn role_match(State(app): State<Shared>, headers: HeaderMap, Json(b): Json<MatchBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let store = roles::read_store(&app.cfg);
    let found = choose(&store, &Pick::Rules, "", &b.repo, &b.title, &b.branch).ok().flatten();
    Json(match found {
        Some(c) => {
            let rule = c.by.strip_prefix("rule:").and_then(|id| store.rules.iter().find(|r| r.id == id)).map(|r| r.words()).unwrap_or_default();
            json!({"ok": true, "role": {"id": c.role.id, "name": c.role.name, "kind": c.role.kind}, "how": c.way.key(), "rule": rule})
        }
        None => json!({"ok": true, "role": null}),
    })
    .into_response()
}

/* ───────── the hooks of the task actions ───────── */

/// For a task about to be added from the to-do list: the role it gets (the one picked, or the rules' for its repository
/// and title), as flags for `add`. A role that goes «after» adds nothing here: its research task follows the add.
pub async fn plan_new(app: &Shared, pick: &Pick, how: &str, repo: &str, title: &str) -> Result<Option<(Chosen, Vec<String>)>, String> {
    let store = roles::read_store(&app.cfg);
    let Some(chosen) = choose(&store, pick, how, repo, title, "")? else { return Ok(None) };
    let problems = check_role(app, &chosen.role).await;
    if !problems.is_empty() {
        return Err(format!("роль «{}»: {}", chosen.role.name, problems.join("; ")));
    }
    let flags = if chosen.way == Way::After { Vec::new() } else { roles::add_flags(&chosen.role, chosen.way) };
    Ok(Some((chosen, flags)))
}

/// After `add` made the task: records the role, and for an «after» role adds its research task. The words say what went
/// wrong when the task is made and the role could not be put on it.
pub async fn after_add(app: &Shared, chosen: &Chosen, out: &str, repo: &str, title: &str, desc: &str) -> Option<String> {
    let Some(id) = added_id(out, repo) else {
        return Some(format!("задача добавлена, но её номер не разобран: роль «{}» не записана", chosen.role.name));
    };
    app::refresh(app).await;
    let tag = app.snapshot.read().await.tasks.iter().find(|t| t.id == id).map(|t| t.tag.clone()).unwrap_or_default();
    let target = Target { id: id.clone(), tag, title: title.to_string(), repo: repo.to_string(), description: desc.to_string(), ..Target::default() };
    if chosen.way == Way::After {
        return put(app, chosen, &target).await.err().map(|f| format!("задача добавлена, роль «{}» не запущена: {}", chosen.role.name, f.message()));
    }
    let ran: Vec<String> = roles::settings_of(&chosen.role, chosen.way).into_iter().map(|(n, _)| n).collect();
    note_put(app, chosen, &target, &ran, "").err().map(|f| f.message())
}

/// Before a task starts: puts the picked role on it, or the rules' role when none was picked and its place on the
/// task is free. A refusal of the board stops the start.
pub async fn on_start(app: &Shared, id: &str, pick: &Pick, how: &str) -> Result<(), Fail> {
    if *pick == Pick::Nothing {
        return Ok(());
    }
    let Some((target, _)) = target_of(app, id).await else { return Ok(()) };
    let store = roles::read_store(&app.cfg);
    let Some(chosen) = choose(&store, pick, how, &target.repo, &target.title, &target.branch).map_err(|e| Fail::new(422, e))? else { return Ok(()) };
    // the rules never run over a place the task already has filled (a role chosen when it was added, an earlier start)
    if *pick == Pick::Rules {
        let place = |a: &Assignment| roles::slot_of(a);
        let mine = Assignment { role: chosen.role.id.clone(), how: chosen.way.key().to_string(), ..Assignment::default() };
        if store.assigned.iter().any(|a| a.task == target.id && place(a) == place(&mine)) {
            return Ok(());
        }
    }
    put(app, &chosen, &target).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_tasks_id_is_read_from_the_boards_words() {
        assert_eq!(added_id("planner/12: added\n", "planner").as_deref(), Some("planner/12"));
        assert_eq!(added_id("PLN-35: added", "planner").as_deref(), Some("planner/35"));
        assert_eq!(added_id("nothing useful", "planner"), None);
        assert_eq!(added_id("", "planner"), None);
    }

    #[test]
    fn the_pick_of_an_action_says_rules_nothing_or_a_role() {
        assert_eq!(pick_of(None), Pick::Rules);
        assert_eq!(pick_of(Some("auto")), Pick::Rules);
        assert_eq!(pick_of(Some("none")), Pick::Nothing);
        assert_eq!(pick_of(Some("")), Pick::Nothing);
        assert_eq!(pick_of(Some("r1f")), Pick::Role("r1f".into()));
    }

    /* The board's commands a role makes, through a stand-in board that writes down what it is told. */

    use axum::body::to_bytes;
    use std::path::{Path as FsPath, PathBuf};

    const SHOW: &str = r#"{"settings":[{"name":"worker.agent","kind":"work","value":"claude/claude-opus-5-5","from":"you","place":"you","own":true,"note":""},{"name":"worker.effort","kind":"work","value":"low","from":"you","place":"you","own":true,"note":""}]}"#;

    fn stand_in(dir: &FsPath) -> (Shared, PathBuf) {
        let log = dir.join("board.log");
        let script = dir.join("board.sh");
        let body = format!(
            "#!/bin/sh\ncase \"$1\" in\n  list|coordinator) echo '[]';;\n  show) echo '{SHOW}';;\n  models) case \"$2\" in claude) printf 'claude takes a short name.\\n\\nclaude/claude-opus-5-5\\nclaude/claude-sonnet-5-5\\n\\nEffort (worker.effort and the like): low, medium, high (from x).\\n';; *) echo 'unknown' >&2; exit 2;; esac;;\n  add) echo \"$@\" | tr '\\n' ' ' >> {log}; echo >> {log}; echo 'alpha/9: added';;\n  set|unset) echo \"$@\" | tr '\\n' ' ' >> {log}; echo >> {log};;\n  *) exit 1;;\nesac\n",
            log = log.display()
        );
        crate::testutil::write_exec(&script, body);
        let cfg = crate::config::AppConfig { herdr_projects: script.display().to_string(), ..crate::testutil::test_cfg(dir) };
        (app::App::new(cfg), log)
    }

    fn commands(log: &FsPath) -> Vec<String> {
        std::fs::read_to_string(log).unwrap_or_default().lines().map(|l| l.trim_end().to_string()).collect()
    }

    async fn json_of(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn live_task(app: &Shared) {
        let mut snap = app.snapshot.write().await;
        snap.tasks = vec![Task {
            id: "alpha/1".into(),
            repo: "alpha".into(),
            n: 1,
            tag: "ALP-1".into(),
            title: "Вход по паролю".into(),
            status: Status::Working,
            ..Task::default()
        }];
    }

    fn headers() -> HeaderMap {
        HeaderMap::new()
    }

    fn designer() -> roles::RoleInput {
        roles::RoleInput {
            name: "Дизайнер".into(),
            kind: "worker".into(),
            agent: "claude/claude-sonnet-5-5".into(),
            effort: "high".into(),
            instructions: "Следи за единым видом".into(),
            ..Default::default()
        }
    }

    fn put_body(role: &str, how: &str, confirm: bool) -> PutBody {
        PutBody { role: role.into(), task: "alpha/1".into(), how: Some(how.into()), confirm: Some(confirm).filter(|c| *c) }
    }

    #[tokio::test]
    async fn a_worker_role_is_previewed_and_then_set_on_the_task_by_the_boards_own_commands() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = stand_in(dir.path());
        live_task(&app).await;
        let id = roles::edit_store(&app.cfg, |s| roles::add_role(s, &designer())).unwrap();

        let (code, prev) = json_of(role_preview(State(app.clone()), headers(), Json(put_body(&id, "", false))).await).await;
        assert_eq!(code, StatusCode::OK, "{prev}");
        let steps: Vec<String> = prev["steps"].as_array().unwrap().iter().map(|s| format!("{} {} → {}", s["setting"].as_str().unwrap(), s["was"].as_str().unwrap(), s["now"].as_str().unwrap())).collect();
        assert_eq!(
            steps,
            ["worker.agent claude/claude-opus-5-5 → claude/claude-sonnet-5-5", "worker.effort low → high", "worker.instructions  → Следи за единым видом"]
        );
        assert_eq!(prev["steps"][0]["command"], "herdr-projects set worker.agent claude/claude-sonnet-5-5 --task alpha/1");
        assert_eq!((prev["valid"].clone(), prev["started"].clone(), prev["how"].clone()), (json!(true), json!(true), json!("worker")));
        assert!(commands(&log).is_empty(), "the preview changes nothing");

        let (code, _) = json_of(role_apply(State(app.clone()), headers(), Json(put_body(&id, "", false))).await).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "not without the confirmation");
        assert!(commands(&log).is_empty());

        let (code, done) = json_of(role_apply(State(app.clone()), headers(), Json(put_body(&id, "", true))).await).await;
        assert_eq!(code, StatusCode::OK, "{done}");
        assert_eq!(
            commands(&log),
            [
                "set worker.agent claude/claude-sonnet-5-5 --task alpha/1",
                "set worker.effort high --task alpha/1",
                "set worker.instructions Следи за единым видом --task alpha/1",
            ]
        );
        let store = roles::read_store(&app.cfg);
        let marks = roles::marks_of(&store);
        assert_eq!(marks["alpha/1"][0].name, "Дизайнер");
        assert_eq!(marks["alpha/1"][0].by, "chosen");
        assert_eq!(store.assigned[0].settings, ["worker.agent", "worker.effort", "worker.instructions"]);
    }

    #[tokio::test]
    async fn a_reviewer_role_goes_instead_of_the_reviewer_or_as_a_research_task_after_the_task() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = stand_in(dir.path());
        live_task(&app).await;
        let id = roles::edit_store(&app.cfg, |s| roles::enable_suggested(s, "security")).unwrap();

        // instead: the task's reviewer.* is the role's
        let (code, done) = json_of(role_apply(State(app.clone()), headers(), Json(put_body(&id, "instead", true))).await).await;
        assert_eq!(code, StatusCode::OK, "{done}");
        let said = commands(&log);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("set reviewer.instructions Ты проверяешь изменения только с точки зрения безопасности"), "{}", said[0]);
        assert!(said[0].ends_with("--task alpha/1"));

        // after: a research task of its own, queued after the task, with the role's settings as its worker's
        live_task(&app).await;
        let (code, prev) = json_of(role_preview(State(app.clone()), headers(), Json(put_body(&id, "after", false))).await).await;
        assert_eq!(code, StatusCode::OK, "{prev}");
        assert_eq!(prev["steps"][0]["kind"], "add");
        let (code, done) = json_of(role_apply(State(app.clone()), headers(), Json(put_body(&id, "after", true))).await).await;
        assert_eq!(code, StatusCode::OK, "{done}");
        assert_eq!(done["follow"], "alpha/9");
        let said = commands(&log);
        let add = said.last().unwrap();
        for part in ["add Ревьюер безопасности: ALP-1 Вход по паролю", "--repo alpha", "--research", "--after alpha/1", "--delegate", "--worker.instructions Ты проверяешь"] {
            assert!(add.contains(part), "{part} not in {add}");
        }
        assert!(!add.contains("--reviewer."), "the check is a worker of its own");
        let store = roles::read_store(&app.cfg);
        let marks = roles::marks_of(&store);
        let on_task: Vec<&str> = marks["alpha/1"].iter().map(|m| m.how.as_str()).collect();
        assert_eq!(on_task, ["instead", "after"], "the two places of one task");
        assert_eq!((marks["alpha/9"][0].how.as_str(), marks["alpha/9"][0].of.as_str()), ("followup", "alpha/1"), "the check's own task says what it is about");

        // the same extra check is not started twice
        live_task(&app).await;
        let (code, dup) = json_of(role_apply(State(app.clone()), headers(), Json(put_body(&id, "after", true))).await).await;
        assert_eq!(code, StatusCode::CONFLICT, "{dup}");
        let (_, prev) = json_of(role_preview(State(app.clone()), headers(), Json(put_body(&id, "after", false))).await).await;
        assert_eq!((prev["duplicate"].clone(), prev["valid"].clone()), (json!(true), json!(false)));
    }

    #[tokio::test]
    async fn a_task_that_is_started_gets_the_rules_role_once_and_a_chosen_none_gets_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = stand_in(dir.path());
        let id = roles::edit_store(&app.cfg, |s| roles::add_role(s, &designer())).unwrap();
        roles::edit_store(&app.cfg, |s| roles::add_rule(s, &roles::RuleInput { role: id.clone(), repo: "alpha".into(), ..Default::default() })).unwrap();
        live_task(&app).await;
        on_start(&app, "alpha/1", &Pick::Nothing, "").await.unwrap();
        assert!(commands(&log).is_empty(), "the human said no role");
        on_start(&app, "alpha/1", &Pick::Rules, "").await.unwrap();
        assert_eq!(commands(&log).len(), 3);
        let marks = roles::marks_of(&roles::read_store(&app.cfg));
        assert_eq!((marks["alpha/1"][0].by.as_str(), marks["alpha/1"][0].rule.as_str()), ("rule", "репозиторий alpha"));
        // the place is filled: starting it again does not run the rule over it
        live_task(&app).await;
        on_start(&app, "alpha/1", &Pick::Rules, "").await.unwrap();
        assert_eq!(commands(&log).len(), 3);
        // a repository the rule does not name gets nothing
        {
            let mut snap = app.snapshot.write().await;
            snap.tasks = vec![Task { id: "beta/1".into(), repo: "beta".into(), n: 1, tag: "BET-1".into(), title: "x".into(), status: Status::Queued, ..Task::default() }];
        }
        on_start(&app, "beta/1", &Pick::Rules, "").await.unwrap();
        assert_eq!(commands(&log).len(), 3);
    }

    #[tokio::test]
    async fn a_new_task_from_the_list_gets_the_role_as_flags_of_add_and_an_after_role_follows_it() {
        let dir = tempfile::tempdir().unwrap();
        let (app, log) = stand_in(dir.path());
        let id = roles::edit_store(&app.cfg, |s| roles::add_role(s, &designer())).unwrap();
        let (chosen, flags) = plan_new(&app, &Pick::Role(id), "", "alpha", "Кнопка").await.unwrap().unwrap();
        assert_eq!(flags[..4], ["--worker.agent", "claude/claude-sonnet-5-5", "--worker.effort", "high"]);
        assert!(after_add(&app, &chosen, "alpha/9: added\n", "alpha", "Кнопка", "").await.is_none());
        let marks = roles::marks_of(&roles::read_store(&app.cfg));
        assert_eq!(marks["alpha/9"][0].name, "Дизайнер");
        assert!(commands(&log).is_empty(), "the flags already went with `add`: nothing more is set");

        let sec = roles::edit_store(&app.cfg, |s| roles::enable_suggested(s, "security")).unwrap();
        let (after, flags) = plan_new(&app, &Pick::Role(sec), "after", "alpha", "Кнопка").await.unwrap().unwrap();
        assert!(flags.is_empty(), "an «after» role adds no flags to the task itself");
        assert!(after_add(&app, &after, "alpha/9: added\n", "alpha", "Кнопка", "").await.is_none());
        assert!(commands(&log).last().unwrap().contains("--research --after alpha/9 --delegate"));
        assert!(plan_new(&app, &Pick::Role("nope".into()), "", "alpha", "x").await.is_err());
    }

    #[tokio::test]
    async fn a_role_that_is_used_is_deleted_only_on_a_second_word_and_takes_its_rules_and_presets_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _log) = stand_in(dir.path());
        live_task(&app).await;
        let id = roles::edit_store(&app.cfg, |s| roles::add_role(s, &designer())).unwrap();
        roles::edit_store(&app.cfg, |s| roles::add_rule(s, &roles::RuleInput { role: id.clone(), repo: "alpha".into(), ..Default::default() })).unwrap();
        let held = roles::capture(&roles::read_store(&app.cfg).roles);
        crew::edit_store(&app.cfg, |s| crew::add_preset(s, "Дизайн", &held)).unwrap();
        let (code, _) = json_of(role_apply(State(app.clone()), headers(), Json(put_body(&id, "", true))).await).await;
        assert_eq!(code, StatusCode::OK);
        live_task(&app).await;

        let ask = |force: bool| RolesBody { op: "delete".into(), id: Some(id.clone()), role: None, rule: None, key: None, force: Some(force) };
        let (code, said) = json_of(roles_post(State(app.clone()), headers(), Json(ask(false))).await).await;
        assert_eq!(code, StatusCode::CONFLICT, "{said}");
        assert_eq!(said["usage"]["tasks"][0]["tag"], "ALP-1");
        assert_eq!(said["usage"]["rules"], 1);
        assert_eq!(said["usage"]["presets"][0], "Дизайн");
        assert_eq!(roles::read_store(&app.cfg).roles.len(), 1, "nothing was deleted");

        let (code, done) = json_of(roles_post(State(app.clone()), headers(), Json(ask(true))).await).await;
        assert_eq!(code, StatusCode::OK, "{done}");
        let store = roles::read_store(&app.cfg);
        assert!((store.roles.is_empty(), store.rules.is_empty()) == (true, true));
        assert!(crew::read_store(&app.cfg).presets[0].roles.is_empty(), "the preset lost the role");
        assert_eq!(roles::marks_of(&store)["alpha/1"][0].name, "Дизайнер", "the task's row keeps the name");

        // an unused role goes at once
        let free = roles::edit_store(&app.cfg, |s| roles::add_role(s, &roles::RoleInput { name: "Другая".into(), ..designer() })).unwrap();
        let ask_free = RolesBody { op: "delete".into(), id: Some(free), role: None, rule: None, key: None, force: None };
        assert_eq!(json_of(roles_post(State(app.clone()), headers(), Json(ask_free)).await).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_role_window_reads_what_each_role_sets_and_where_it_stands() {
        let dir = tempfile::tempdir().unwrap();
        let (app, _log) = stand_in(dir.path());
        live_task(&app).await;
        let sec = roles::edit_store(&app.cfg, |s| roles::enable_suggested(s, "security")).unwrap();
        let v = roles_read(&app).await;
        assert_eq!(v["suggested"].as_array().unwrap().len(), 0, "the suggested role is on now");
        let r = &v["roles"][0];
        assert_eq!(r["id"], sec.as_str());
        assert_eq!(r["sets"]["instead"][0]["setting"], "reviewer.instructions");
        assert_eq!(r["sets"]["after"][0]["setting"], "worker.instructions");
        assert_eq!(r["usage"]["tasks"].as_array().unwrap().len(), 0);
        // before it is on, it is offered
        let other = tempfile::tempdir().unwrap();
        let (app2, _) = stand_in(other.path());
        let v = roles_read(&app2).await;
        assert_eq!(v["suggested"][0]["name"], "Ревьюер безопасности");
        assert_eq!(v["roles"].as_array().unwrap().len(), 0, "nothing is on by default");
    }

    fn store_with_rule() -> (roles::Store, String) {
        let mut s = roles::Store::default();
        let id = roles::add_role(
            &mut s,
            &roles::RoleInput { name: "Дизайнер".into(), kind: "worker".into(), agent: "claude/claude-sonnet-5-5".into(), ..Default::default() },
        )
        .unwrap();
        roles::add_rule(&mut s, &roles::RuleInput { role: id.clone(), repo: "web".into(), ..Default::default() }).unwrap();
        (s, id)
    }

    #[test]
    fn the_rules_choose_a_role_only_when_nothing_was_picked() {
        let (s, id) = store_with_rule();
        let c = choose(&s, &Pick::Rules, "", "web", "Кнопка", "").unwrap().unwrap();
        assert_eq!((c.role.id.as_str(), c.way, c.by.starts_with("rule:")), (id.as_str(), Way::Worker, true));
        assert!(choose(&s, &Pick::Rules, "", "planner", "Кнопка", "").unwrap().is_none());
        assert!(choose(&s, &Pick::Nothing, "", "web", "Кнопка", "").unwrap().is_none());
        let picked = choose(&s, &Pick::Role(id.clone()), "", "planner", "Кнопка", "").unwrap().unwrap();
        assert_eq!(picked.by, "chosen");
        assert!(choose(&s, &Pick::Role("nope".into()), "", "web", "", "").is_err());
        assert!(choose(&s, &Pick::Role(id), "after", "web", "", "").is_err(), "a worker has no «after»");
    }

    #[test]
    fn a_rule_whose_role_lost_its_way_gives_no_role() {
        let (mut s, id) = store_with_rule();
        let sec = roles::enable_suggested(&mut s, "security").unwrap();
        roles::add_rule(&mut s, &roles::RuleInput { role: sec.clone(), title: "безопас".into(), how: "after".into(), ..Default::default() }).unwrap();
        let c = choose(&s, &Pick::Rules, "", "planner", "О безопасности", "").unwrap().unwrap();
        assert_eq!((c.role.id, c.way), (sec, Way::After));
        // the rule of the worker role asks for a way the role never had: ignored, not a crash
        s.rules.iter_mut().find(|r| r.role == id).unwrap().how = "after".into();
        assert!(choose(&s, &Pick::Rules, "", "web", "Кнопка", "").unwrap().is_none());
    }
}
