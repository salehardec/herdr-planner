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
