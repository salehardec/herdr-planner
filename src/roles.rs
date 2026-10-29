//! The crew's own roles (PLN-90): «Дизайнер», «Ревьюер безопасности», «Документатор» — a name, an agent, an effort, the
//! spares (fallback) and the instructions, which the human adds to the crew themselves.
//!
//! The board has no roles of its own: its list of settings is fixed (`worker.*`, `reviewer.*`, …). So a role is the
//! planner's thing, kept in `crew-roles.json` in its state folder, and it only reaches the board as the board's own
//! commands when it is put on a task:
//!
//! * a worker role → `set worker.agent|effort|fallback|instructions … --task <id>` (or `--worker.*` on `add`);
//! * a reviewer role «instead of the reviewer» → the same with `reviewer.*`;
//! * a reviewer role «in addition, after the task» → a research task of its own:
//!   `add … --research --after <id> --delegate --worker.* …`, whose report holds the findings.
//!
//! Which role a task gets is chosen when it is added or started, or comes from the rules («for the repository X», «if
//! the title or the branch is like …», the way the board's automations take `--if`). `assigned` keeps where each role
//! was put, so the task's row and the crew window say where it came from.
//!
//! Everything here is pure or a small file; `roles_api.rs` runs the commands.

use crate::config::{now_ms, AppConfig};
use crate::crew;
use crate::station::{agent_program, find, Setting};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_ROLES: usize = 40;
pub const MAX_RULES: usize = 60;
const MAX_NAME: usize = 40;
const MAX_INSTRUCTIONS: usize = 4000;
const MAX_PATTERN: usize = 80;
/// The assignments kept: the oldest go first.
const MAX_ASSIGNED: usize = 2000;

/// The preset key of a custom role (`role:r18f…`): a preset holds the roles by these names next to the built-in ones.
pub const PRESET_PREFIX: &str = "role:";

pub fn preset_key(id: &str) -> String {
    format!("{PRESET_PREFIX}{id}")
}

pub fn role_of_key(key: &str) -> Option<&str> {
    key.strip_prefix(PRESET_PREFIX).filter(|id| valid_id(id))
}

/// A role id the planner made (`r` and hex) or a suggested one it was given.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/* ───────── the records ───────── */

/// One role of the human's own. An empty agent, effort or fallback means «the task keeps what it has»: nothing is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomRole {
    pub id: String,
    pub name: String,
    /// `worker` (does the task) or `reviewer` (checks it).
    pub kind: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub fallback: String,
    /// The role's focus, given to the agent as `worker.instructions` or `reviewer.instructions`.
    #[serde(default)]
    pub instructions: String,
    /// How a reviewer role goes on a task when the choice does not say: `instead` of the board's reviewer, or `after`
    /// the task as a research task of its own. A worker role is always `worker`.
    #[serde(default)]
    pub how: String,
    /// The suggested role this one was made from (`security`), or empty.
    #[serde(default)]
    pub suggested: String,
}

/// «If the task is like this, give it that role». Every condition that is filled must hold; the first rule that
/// holds wins. A pattern is words that must be in the text, or, with `*`, a mask for the whole text (`feature/KIC-*`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub role: String,
    /// The repository's name; empty: any.
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub branch: String,
    /// How the role goes on the task; empty: the role's own.
    #[serde(default)]
    pub how: String,
}

/// Where a role was put: one record for each of a task's places (`worker`, `reviewer`, or a role that runs after it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    /// The task's id (`planner/12`).
    pub task: String,
    pub role: String,
    /// The role's name when it was put (kept when the role is deleted).
    pub name: String,
    /// `worker`, `instead`, `after` (on the task that gets the extra check) or `followup` (on the research task itself).
    pub how: String,
    /// `chosen` (the human picked it) or `rule:<id>`.
    pub by: String,
    pub at: u64,
    /// The board's settings it set on the task.
    #[serde(default)]
    pub settings: Vec<String>,
    /// `after`: the research task that does the extra check (its id).
    #[serde(default)]
    pub follow: String,
    /// `followup`: the task the check is about.
    #[serde(default)]
    pub of: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub roles: Vec<CustomRole>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub assigned: Vec<Assignment>,
}

/// What the row of a task says about its roles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RoleMark {
    pub role: String,
    pub name: String,
    pub how: String,
    /// `chosen` or `rule`.
    pub by: String,
    /// The rule's words, when `by` is `rule`.
    pub rule: String,
    /// `followup`: the task the check is about (its id).
    pub of: String,
}

/* ───────── names, shapes ───────── */

pub fn valid_name(name: &str) -> Result<String, String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        return Err("у роли должно быть имя".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("имя роли — не длиннее {MAX_NAME} знаков"));
    }
    if n.chars().any(char::is_control) {
        return Err("в имени роли не может быть управляющих знаков".into());
    }
    if n.starts_with('-') {
        // the name starts the title of a check task, which the board would take for a flag
        return Err("имя роли не может начинаться с «-»".into());
    }
    Ok(n)
}

/// A role as the page sends it, before it is kept. Everything is checked for its shape; the models and efforts the
/// programs list are checked when the role is put on a task (the lists are read then).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RoleInput {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub fallback: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub how: String,
}

fn clean_fields(input: &RoleInput) -> Result<CustomRole, String> {
    let name = valid_name(&input.name)?;
    let kind = input.kind.trim().to_string();
    if kind != "worker" && kind != "reviewer" {
        return Err("роль — исполнитель (worker) или ревьюер (reviewer)".into());
    }
    let agent = input.agent.trim().to_string();
    if !agent.is_empty() {
        agent_program(&agent)?;
    }
    let effort = input.effort.trim().to_string();
    if effort.len() > 24 || effort.starts_with('-') || !effort.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("«{effort}»: так усилие не называется"));
    }
    let fallback = input.fallback.trim().to_string();
    if !fallback.is_empty() {
        crew::check_fallback(&fallback, &|_| Vec::new())?;
    }
    let instructions = input.instructions.trim().to_string();
    if instructions.chars().count() > MAX_INSTRUCTIONS {
        return Err(format!("инструкции длиннее {MAX_INSTRUCTIONS} знаков"));
    }
    if instructions.contains('\0') {
        return Err("в инструкциях не может быть нулевого байта".into());
    }
    if instructions.starts_with('-') {
        return Err("инструкции не могут начинаться с «-»: доска приняла бы их за ключ".into());
    }
    let how = if kind == "worker" {
        "worker".to_string()
    } else {
        match input.how.trim() {
            "" | "instead" => "instead".to_string(),
            "after" => "after".to_string(),
            other => return Err(format!("«{other}»: ревьюер идёт «вместо» ревьюера доски или «после» задачи")),
        }
    };
    Ok(CustomRole { id: String::new(), name, kind, agent, effort, fallback, instructions, how, suggested: String::new() })
}

fn name_taken(store: &Store, name: &str, except: &str) -> bool {
    store.roles.iter().any(|r| r.id != except && r.name.to_lowercase() == name.to_lowercase())
}

fn fresh_id(taken: &dyn Fn(&str) -> bool) -> String {
    let mut n = now_ms();
    loop {
        let id = format!("r{n:x}");
        if !taken(&id) {
            return id;
        }
        n += 1;
    }
}

pub fn add_role(store: &mut Store, input: &RoleInput) -> Result<String, String> {
    let mut role = clean_fields(input)?;
    if store.roles.len() >= MAX_ROLES {
        return Err(format!("ролей уже {MAX_ROLES}: удалите ненужные"));
    }
    if name_taken(store, &role.name, "") {
        return Err(format!("роль «{}» уже есть", role.name));
    }
    role.id = fresh_id(&|id| store.roles.iter().any(|r| r.id == id));
    let id = role.id.clone();
    store.roles.push(role);
    Ok(id)
}

pub fn update_role(store: &mut Store, id: &str, input: &RoleInput) -> Result<(), String> {
    let mut role = clean_fields(input)?;
    if name_taken(store, &role.name, id) {
        return Err(format!("роль «{}» уже есть", role.name));
    }
    let old = store.roles.iter_mut().find(|r| r.id == id).ok_or("такой роли нет")?;
    if old.kind != role.kind && store.assigned.iter().any(|a| a.role == id) {
        // the kind decides which settings the role sets: a role that is on tasks keeps the one it has
        return Err("роль уже стоит на задачах: вид (исполнитель или ревьюер) не меняется. Заведите другую роль".into());
    }
    role.id = id.to_string();
    role.suggested = old.suggested.clone();
    *old = role;
    Ok(())
}

/* ───────── the suggested roles ───────── */

/// A role the planner offers; it is not on until the human adds it.
pub struct Suggested {
    pub key: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub how: &'static str,
    pub instructions: &'static str,
}

pub const SUGGESTED: &[Suggested] = &[Suggested {
    key: "security",
    name: "Ревьюер безопасности",
    kind: "reviewer",
    how: "after",
    instructions: "Ты проверяешь изменения только с точки зрения безопасности. Не правь код и не обсуждай стиль. \
Ищи: инъекции (SQL, команды оболочки, шаблоны, пути), обход авторизации и аутентификации, непроверенные входные данные, \
утечки секретов (ключи, токены, пароли в коде, логах и истории git), небезопасную работу с файлами и временными \
файлами, SSRF и открытые редиректы, XSS и CSRF, слабую криптографию, небезопасные зависимости и права доступа. \
Для каждой находки назови файл и строку, как её воспроизвести и чем исправить, и отметь серьёзность: critical, major, \
minor или trivial. Если уязвимостей нет, так и скажи и перечисли, что проверил.",
}];

pub fn suggested_role(key: &str) -> Option<&'static Suggested> {
    SUGGESTED.iter().find(|s| s.key == key)
}

/// Adds a suggested role (it becomes the human's own and can be changed or deleted like any other).
pub fn enable_suggested(store: &mut Store, key: &str) -> Result<String, String> {
    let s = suggested_role(key).ok_or("такой предложенной роли нет")?;
    if store.roles.iter().any(|r| r.suggested == key) {
        return Err(format!("роль «{}» уже включена", s.name));
    }
    let mut id = add_role(
        store,
        &RoleInput {
            name: s.name.into(),
            kind: s.kind.into(),
            how: s.how.into(),
            instructions: s.instructions.into(),
            ..RoleInput::default()
        },
    )?;
    if let Some(r) = store.roles.iter_mut().find(|r| r.id == id) {
        r.suggested = key.to_string();
        id = r.id.clone();
    }
    Ok(id)
}

/* ───────── the rules ───────── */

/// Whether `text` fits `pattern`: no `*` means «contains» (any case); with `*` it is a mask for the whole text.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.trim().to_lowercase(), text.to_lowercase());
    if p.is_empty() {
        return true;
    }
    if !p.contains('*') {
        return t.contains(&p);
    }
    let parts: Vec<&str> = p.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !t.starts_with(first) || !t[first.len()..].ends_with(last) || t.len() < first.len() + last.len() {
        return false;
    }
    let mut at = first.len();
    let end = t.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        match t[at..end].find(mid) {
            Some(i) => at += i + mid.len(),
            None => return false,
        }
    }
    true
}

impl Rule {
    pub fn matches(&self, repo: &str, title: &str, branch: &str) -> bool {
        (self.repo.is_empty() || self.repo == repo)
            && (self.title.is_empty() || glob_match(&self.title, title))
            && (self.branch.is_empty() || glob_match(&self.branch, branch))
    }

    /// The rule in words, for the row and the window: «репозиторий planner · название: *безопас* · ветка: feature/*».
    pub fn words(&self) -> String {
        let mut v = Vec::new();
        if !self.repo.is_empty() {
            v.push(format!("репозиторий {}", self.repo));
        }
        if !self.title.is_empty() {
            v.push(format!("название: {}", self.title));
        }
        if !self.branch.is_empty() {
            v.push(format!("ветка: {}", self.branch));
        }
        v.join(" · ")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RuleInput {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub how: String,
}

fn clean_rule(store: &Store, input: &RuleInput) -> Result<Rule, String> {
    let role = store.roles.iter().find(|r| r.id == input.role).ok_or("для правила нужна существующая роль")?;
    let repo = input.repo.trim().to_string();
    if !repo.is_empty() && !crate::taskmeta::valid_repo(&repo) {
        return Err("недопустимое имя вселенной".into());
    }
    let pattern = |label: &str, v: &str| -> Result<String, String> {
        let v = v.trim().to_string();
        if v.chars().count() > MAX_PATTERN || v.chars().any(char::is_control) {
            return Err(format!("{label}: до {MAX_PATTERN} знаков, без управляющих"));
        }
        Ok(v)
    };
    let (title, branch) = (pattern("название", &input.title)?, pattern("ветка", &input.branch)?);
    if repo.is_empty() && title.is_empty() && branch.is_empty() {
        return Err("правило без условий подошло бы всем задачам: назовите репозиторий, слова в названии или ветку".into());
    }
    let how = input.how.trim().to_string();
    if !how.is_empty() {
        way_of(role, &how)?;
    }
    Ok(Rule { id: String::new(), role: role.id.clone(), repo, title, branch, how })
}

pub fn add_rule(store: &mut Store, input: &RuleInput) -> Result<String, String> {
    let mut rule = clean_rule(store, input)?;
    if store.rules.len() >= MAX_RULES {
        return Err(format!("правил уже {MAX_RULES}: удалите ненужные"));
    }
    rule.id = fresh_id(&|id| store.rules.iter().any(|r| r.id == id));
    let id = rule.id.clone();
    store.rules.push(rule);
    Ok(id)
}

pub fn update_rule(store: &mut Store, id: &str, input: &RuleInput) -> Result<(), String> {
    let mut rule = clean_rule(store, input)?;
    let old = store.rules.iter_mut().find(|r| r.id == id).ok_or("такого правила нет")?;
    rule.id = id.to_string();
    *old = rule;
    Ok(())
}

pub fn delete_rule(store: &mut Store, id: &str) -> Result<(), String> {
    let before = store.rules.len();
    store.rules.retain(|r| r.id != id);
    if store.rules.len() == before {
        return Err("такого правила нет".into());
    }
    Ok(())
}

/// The first rule that fits a task, in the order they were made.
pub fn rule_for<'a>(store: &'a Store, repo: &str, title: &str, branch: &str) -> Option<&'a Rule> {
    store
        .rules
        .iter()
        .filter(|r| store.roles.iter().any(|x| x.id == r.role))
        .find(|r| r.matches(repo, title, branch))
}

/* ───────── how a role goes on a task ───────── */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// A worker role: `worker.*` of the task.
    Worker,
    /// A reviewer role in place of the board's reviewer: `reviewer.*` of the task.
    Instead,
    /// A reviewer role as a research task of its own, after the task.
    After,
}

impl Way {
    pub fn key(self) -> &'static str {
        match self {
            Way::Worker => "worker",
            Way::Instead => "instead",
            Way::After => "after",
        }
    }
}

/// The way a role goes on a task: `how` (the choice's or the rule's), else the role's own. A worker role has one way.
pub fn way_of(role: &CustomRole, how: &str) -> Result<Way, String> {
    match (role.kind.as_str(), how.trim()) {
        ("worker", "" | "worker") => Ok(Way::Worker),
        ("worker", _) => Err(format!("«{}» — исполнитель: ставится на задачу, а не «вместо» или «после» ревьюера", role.name)),
        ("reviewer", "instead") => Ok(Way::Instead),
        ("reviewer", "after") => Ok(Way::After),
        ("reviewer", "") => Ok(if role.how == "after" { Way::After } else { Way::Instead }),
        ("reviewer", other) => Err(format!("«{other}»: ревьюер идёт «вместо» ревьюера доски (instead) или «после» задачи (after)")),
        _ => Err("у роли нет вида".into()),
    }
}

/// The board setting names a role sets and their values, in the order they run: agent, effort, fallback, instructions.
/// A field the role leaves empty is not there. `after` roles are the research task's own `worker.*`.
pub fn settings_of(role: &CustomRole, way: Way) -> Vec<(String, String)> {
    let prefix = if way == Way::Instead { "reviewer" } else { "worker" };
    [("agent", &role.agent), ("effort", &role.effort), ("fallback", &role.fallback), ("instructions", &role.instructions)]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(f, v)| (format!("{prefix}.{f}"), v.clone()))
        .collect()
}

/// One board command of putting a role on a task, with what it changes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cmd {
    /// `set`, or `add` (the research task of an «after» role).
    pub kind: &'static str,
    pub setting: String,
    pub was: String,
    pub was_from: String,
    pub now: String,
    pub error: String,
    /// The arguments after `herdr-projects`.
    pub args: Vec<String>,
}

impl Cmd {
    pub fn text(&self) -> String {
        let quoted: Vec<String> = self
            .args
            .iter()
            .map(|a| if a.is_empty() || a.chars().any(|c| c.is_whitespace() || c == '"') || a.chars().count() > 60 { format!("\"{}\"", clip_one_line(a, 60).replace('"', "'")) } else { a.clone() })
            .collect();
        format!("herdr-projects {}", quoted.join(" "))
    }
}

fn clip_one_line(text: &str, max: usize) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        format!("{}…", one.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}

/// What the planner knows of the task a role is put on.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// `planner/12`
    pub id: String,
    /// `PLN-12`
    pub tag: String,
    pub title: String,
    pub repo: String,
    pub branch: String,
    pub description: String,
    pub pr: String,
}

impl Target {
    pub fn name(&self) -> &str {
        if self.tag.is_empty() { &self.id } else { &self.tag }
    }
}

/// The `set … --task` commands that put a worker or an «instead» role on an existing task. `current` is what `show <id>`
/// says now; a value the task already has of its own is not set again.
pub fn set_commands(role: &CustomRole, way: Way, target: &Target, current: &[Setting]) -> Vec<Cmd> {
    settings_of(role, way)
        .into_iter()
        .filter_map(|(name, value)| {
            let cur = find(current, &name);
            let own_task = cur.is_some_and(|s| s.own && s.place == "task");
            // instructions add up through the levels: the joined text is not the task's own, so it is compared only when the task's own is the whole
            if own_task && cur.is_some_and(|s| s.value == value) {
                return None;
            }
            Some(Cmd {
                kind: "set",
                was: cur.map(|s| s.value.clone()).unwrap_or_default(),
                was_from: cur.map(|s| s.from.clone()).unwrap_or_default(),
                now: value.clone(),
                error: String::new(),
                args: vec!["set".into(), name.clone(), value, "--task".into(), target.id.clone()],
                setting: name,
            })
        })
        .collect()
}

/// The `--worker.agent X …` flags of `add` (a new task that has a worker or an «instead» role from its first minute).
pub fn add_flags(role: &CustomRole, way: Way) -> Vec<String> {
    settings_of(role, way).into_iter().flat_map(|(name, value)| [format!("--{name}"), value]).collect()
}

/// The research task of an «after» role: a title, a description, and the board's `add` for it.
pub fn research_add(role: &CustomRole, target: &Target) -> Cmd {
    let title = clip_one_line(&format!("{}: {} {}", role.name, target.name(), target.title), 60);
    let mut desc = format!("Проверка задачи {} «{}» (вселенная {}).\n", target.name(), target.title, target.repo);
    if !target.branch.is_empty() {
        desc.push_str(&format!("Ветка задачи: {}.\n", target.branch));
    }
    if !target.pr.is_empty() {
        desc.push_str(&format!("Pull request: {}.\n", target.pr));
    }
    if !target.description.trim().is_empty() {
        desc.push_str(&format!("\nЧто просили в задаче:\n{}\n", clip_one_line(&target.description, 1500)));
    }
    desc.push_str(&format!(
        "\nТы — роль «{}». Ничего не меняй в репозитории: итог — отчёт с находками (critical, major, minor, trivial), для каждой файл и строка.",
        role.name
    ));
    if !role.instructions.is_empty() {
        desc.push_str(&format!("\n\nФокус роли:\n{}", role.instructions));
    }
    let mut args: Vec<String> = vec![
        "add".into(),
        title,
        "--repo".into(),
        target.repo.clone(),
        "--desc".into(),
        desc,
        "--research".into(),
        "--after".into(),
        target.id.clone(),
        "--delegate".into(),
    ];
    args.extend(add_flags(role, Way::After));
    Cmd { kind: "add", setting: "research".into(), was: String::new(), was_from: String::new(), now: role.name.clone(), error: String::new(), args }
}

/* ───────── what the roles do to the board ───────── */

/// The place a role takes on a task: one record per place.
pub fn slot_of(a: &Assignment) -> String {
    match a.how.as_str() {
        "worker" => "worker".into(),
        "instead" => "reviewer".into(),
        "after" => format!("after:{}", a.role),
        _ => "followup".into(),
    }
}

/// Records where a role was put (replacing the role that held the same place on the task).
pub fn record(store: &mut Store, a: Assignment) {
    let slot = slot_of(&a);
    store.assigned.retain(|x| !(x.task == a.task && slot_of(x) == slot));
    store.assigned.push(a);
    if store.assigned.len() > MAX_ASSIGNED {
        let drop = store.assigned.len() - MAX_ASSIGNED;
        store.assigned.drain(..drop);
    }
}

/// The marks of each task's row, by task id. A name follows the role when it was renamed.
pub fn marks_of(store: &Store) -> HashMap<String, Vec<RoleMark>> {
    let mut out: HashMap<String, Vec<RoleMark>> = HashMap::new();
    for a in &store.assigned {
        let name = store.roles.iter().find(|r| r.id == a.role).map(|r| r.name.clone()).unwrap_or_else(|| a.name.clone());
        let rule = a
            .by
            .strip_prefix("rule:")
            .and_then(|id| store.rules.iter().find(|r| r.id == id))
            .map(Rule::words)
            .unwrap_or_default();
        out.entry(a.task.clone()).or_default().push(RoleMark {
            role: a.role.clone(),
            name,
            how: a.how.clone(),
            by: if a.by.starts_with("rule:") { "rule".into() } else { "chosen".into() },
            rule,
            of: a.of.clone(),
        });
    }
    out
}

/// What uses a role: the live tasks it stands on, the rules that give it, the presets that hold it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Usage {
    pub tasks: Vec<UsedTask>,
    pub rules: usize,
    pub presets: Vec<String>,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty() && self.rules == 0 && self.presets.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsedTask {
    pub id: String,
    pub tag: String,
    pub title: String,
    pub status: String,
    pub how: String,
}

/// What a live task looks like to the usage list; `None` for a task that is done, archived or gone.
pub type LiveTask<'a> = &'a dyn Fn(&str) -> Option<(String, String, String)>;

pub fn usage_of(store: &Store, presets: &crew::Store, role: &str, live: LiveTask) -> Usage {
    let mut tasks = Vec::new();
    for a in store.assigned.iter().filter(|a| a.role == role && a.how != "followup") {
        if let Some((tag, title, status)) = live(&a.task) {
            tasks.push(UsedTask { id: a.task.clone(), tag, title, status, how: a.how.clone() });
        }
    }
    let key = preset_key(role);
    let has = |p: &crew::Preset| p.roles.get(&key).is_some_and(|s| !s.agent.is_empty() || !s.effort.is_empty() || !s.fallback.is_empty());
    Usage {
        tasks,
        rules: store.rules.iter().filter(|r| r.role == role).count(),
        presets: presets.presets.iter().filter(|p| has(p)).map(|p| p.name.clone()).collect(),
    }
}

/// Takes a role away together with its rules. The tasks it stands on keep what the board was told, and their rows keep
/// its name. The presets are cleaned by the caller (they live in another file).
pub fn delete_role(store: &mut Store, id: &str) -> Result<CustomRole, String> {
    let at = store.roles.iter().position(|r| r.id == id).ok_or("такой роли нет")?;
    store.rules.retain(|r| r.role != id);
    Ok(store.roles.remove(at))
}

/* ───────── roles inside presets ───────── */

/// The steps that bring the roles to what a preset says: the planner's own, no board command. A preset may hold
/// roles that are gone now: those are left out.
pub fn preset_steps(preset: &crew::Preset, roles: &[CustomRole], repo: &str) -> Vec<crew::Step> {
    let mut steps = Vec::new();
    if !repo.is_empty() {
        return steps;
    }
    for r in roles {
        let Some(set) = preset.roles.get(&preset_key(&r.id)) else { continue };
        for (field, was, want) in [("agent", &r.agent, &set.agent), ("effort", &r.effort, &set.effort), ("fallback", &r.fallback, &set.fallback)] {
            if was == want {
                continue;
            }
            steps.push(crew::Step {
                role: "custom",
                field: match field {
                    "agent" => "agent",
                    "effort" => "effort",
                    _ => "fallback",
                },
                setting: preset_key(&r.id),
                was: was.clone(),
                was_from: String::new(),
                now: Some(want.clone()).filter(|v| !v.is_empty()),
                error: String::new(),
                custom: r.id.clone(),
                label: r.name.clone(),
            });
        }
    }
    steps
}

/// The roles as a preset holds them.
pub fn capture(roles: &[CustomRole]) -> BTreeMap<String, crew::RoleSet> {
    roles
        .iter()
        .map(|r| (preset_key(&r.id), crew::RoleSet { agent: r.agent.clone(), effort: r.effort.clone(), fallback: r.fallback.clone() }))
        .collect()
}

/// Puts a preset's value for a role into the role (agent, effort, fallback; the instructions stay).
pub fn apply_step(store: &mut Store, id: &str, field: &str, now: Option<&str>) -> Result<(), String> {
    let role = store.roles.iter_mut().find(|r| r.id == id).ok_or("такой роли уже нет")?;
    let v = now.unwrap_or("").to_string();
    match field {
        "agent" => role.agent = v,
        "effort" => role.effort = v,
        "fallback" => role.fallback = v,
        _ => return Err("у роли нет такого поля".into()),
    }
    Ok(())
}

/* ───────── the file ───────── */

static FILE_LOCK: Mutex<()> = Mutex::new(());

fn roles_file(cfg: &AppConfig) -> PathBuf {
    cfg.state_dir.join("crew-roles.json")
}

fn read_at(path: &Path) -> Store {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Store>(&t).ok()).unwrap_or_default()
}

pub fn read_store(cfg: &AppConfig) -> Store {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read_at(&roles_file(cfg))
}

/// Reads the file, lets `f` change it, writes it back (a temporary file, then a rename) — all under one lock.
pub fn edit_store<T>(cfg: &AppConfig, f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = roles_file(cfg);
    let mut store = read_at(&path);
    let out = f(&mut store)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, text).map_err(|e| format!("не записать {}: {e}", path.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, kind: &str) -> RoleInput {
        RoleInput { name: name.into(), kind: kind.into(), ..RoleInput::default() }
    }

    fn designer() -> RoleInput {
        RoleInput {
            name: "Дизайнер".into(),
            kind: "worker".into(),
            agent: "claude/claude-sonnet-5-5".into(),
            effort: "high".into(),
            fallback: "codex, claude/claude-haiku-5-5".into(),
            instructions: "Следи за единым видом интерфейса".into(),
            how: String::new(),
        }
    }

    fn security(store: &mut Store) -> CustomRole {
        let id = enable_suggested(store, "security").unwrap();
        store.roles.iter().find(|r| r.id == id).unwrap().clone()
    }

    fn target() -> Target {
        Target {
            id: "planner/12".into(),
            tag: "PLN-12".into(),
            title: "Вход по паролю".into(),
            repo: "planner".into(),
            branch: "work/12".into(),
            description: "Сделать вход".into(),
            pr: String::new(),
        }
    }

    fn setting(name: &str, value: &str, own_task: bool) -> Setting {
        Setting {
            name: name.into(),
            kind: "work".into(),
            value: value.into(),
            from: if own_task { "this task".into() } else { "you".into() },
            place: if own_task { "task".into() } else { "you".into() },
            own: own_task,
            note: String::new(),
        }
    }

    #[test]
    fn a_role_is_added_with_a_clean_name_and_cannot_repeat_it() {
        let mut s = Store::default();
        let id = add_role(&mut s, &designer()).unwrap();
        assert!(valid_id(&id));
        assert_eq!(s.roles[0].how, "worker");
        assert!(add_role(&mut s, &RoleInput { name: " дизайнер ".into(), ..designer() }).unwrap_err().contains("уже есть"));
        assert!(add_role(&mut s, &input("  ", "worker")).is_err());
        assert!(add_role(&mut s, &input("Тест", "boss")).is_err());
        let long = RoleInput { instructions: "x".repeat(MAX_INSTRUCTIONS + 1), ..designer() };
        assert!(add_role(&mut s, &long).is_err());
        let dash = RoleInput { name: "Другая".into(), instructions: "- ключ".into(), ..designer() };
        assert!(add_role(&mut s, &dash).unwrap_err().contains("«-»"));
        // nothing that starts with «-» reaches the board as an argument: it would be read as a flag
        assert!(add_role(&mut s, &input("-флаг", "worker")).is_err());
        assert!(add_role(&mut s, &RoleInput { name: "Четвёртая".into(), effort: "--help".into(), ..designer() }).is_err());
        let bad_fallback = RoleInput { name: "Третья".into(), fallback: "codex, codex".into(), ..designer() };
        assert!(add_role(&mut s, &bad_fallback).is_err());
        assert_eq!(s.roles.len(), 1);
    }

    #[test]
    fn a_reviewer_role_goes_instead_by_default_and_a_worker_has_one_way() {
        let mut s = Store::default();
        add_role(&mut s, &input("Ревьюер", "reviewer")).unwrap();
        let r = s.roles[0].clone();
        assert_eq!(way_of(&r, "").unwrap(), Way::Instead);
        assert_eq!(way_of(&r, "after").unwrap(), Way::After);
        assert!(way_of(&r, "worker").is_err());
        let after = security(&mut s);
        assert_eq!(way_of(&after, "").unwrap(), Way::After, "the role's own way");
        assert_eq!(way_of(&after, "instead").unwrap(), Way::Instead, "the choice wins");
        add_role(&mut s, &designer()).unwrap();
        let w = s.roles.iter().find(|r| r.kind == "worker").unwrap();
        assert_eq!(way_of(w, "").unwrap(), Way::Worker);
        assert!(way_of(w, "after").is_err());
    }

    #[test]
    fn the_security_reviewer_is_suggested_and_not_on_until_it_is_added() {
        let mut s = Store::default();
        assert!(s.roles.is_empty(), "nothing is on by default");
        let r = security(&mut s);
        assert_eq!((r.name.as_str(), r.kind.as_str(), r.how.as_str()), ("Ревьюер безопасности", "reviewer", "after"));
        assert!(r.instructions.contains("инъекции") && r.instructions.contains("секрет"));
        assert!(enable_suggested(&mut s, "security").unwrap_err().contains("уже включена"));
        assert!(enable_suggested(&mut s, "nope").is_err());
        // once added it is the human's: it can be deleted and offered again
        delete_role(&mut s, &r.id).unwrap();
        assert!(enable_suggested(&mut s, "security").is_ok());
    }

    #[test]
    fn a_worker_role_becomes_set_commands_for_the_task() {
        let mut s = Store::default();
        add_role(&mut s, &designer()).unwrap();
        let cmds = set_commands(&s.roles[0], Way::Worker, &target(), &[]);
        let args: Vec<Vec<String>> = cmds.iter().map(|c| c.args.clone()).collect();
        assert_eq!(args[0], ["set", "worker.agent", "claude/claude-sonnet-5-5", "--task", "planner/12"]);
        assert_eq!(args[1], ["set", "worker.effort", "high", "--task", "planner/12"]);
        assert_eq!(args[2], ["set", "worker.fallback", "codex, claude/claude-haiku-5-5", "--task", "planner/12"]);
        assert_eq!(args[3][1], "worker.instructions");
        assert_eq!(cmds.len(), 4);
        assert!(cmds[0].text().starts_with("herdr-projects set worker.agent claude/claude-sonnet-5-5 --task planner/12"));
    }

    #[test]
    fn a_reviewer_role_instead_sets_the_reviewer_and_leaves_empty_fields_alone() {
        let mut s = Store::default();
        let r = security(&mut s);
        let cmds = set_commands(&r, Way::Instead, &target(), &[]);
        assert_eq!(cmds.len(), 1, "no agent, effort or fallback: only the focus");
        assert_eq!(cmds[0].args[..2], ["set", "reviewer.instructions"]);
        assert_eq!(cmds[0].args[3..], ["--task", "planner/12"]);
    }

    #[test]
    fn what_the_task_already_has_of_its_own_is_not_set_again() {
        let mut s = Store::default();
        add_role(&mut s, &designer()).unwrap();
        let now = [
            setting("worker.agent", "claude/claude-sonnet-5-5", true),
            setting("worker.effort", "high", false), // the human's, not the task's: it is still set on the task
            setting("worker.fallback", "none", false),
        ];
        let cmds = set_commands(&s.roles[0], Way::Worker, &target(), &now);
        let names: Vec<&str> = cmds.iter().map(|c| c.setting.as_str()).collect();
        assert_eq!(names, ["worker.effort", "worker.fallback", "worker.instructions"]);
        assert_eq!(cmds[0].was, "high");
        assert_eq!(cmds[1].was, "none");
    }

    #[test]
    fn a_reviewer_role_after_is_a_research_task_after_the_first() {
        let mut s = Store::default();
        let r = security(&mut s);
        let mut r = r;
        r.agent = "claude/claude-opus-5-5".into();
        r.effort = "high".into();
        let cmd = research_add(&r, &target());
        assert_eq!(cmd.kind, "add");
        let a = &cmd.args;
        assert_eq!(a[0], "add");
        assert!(a[1].starts_with("Ревьюер безопасности: PLN-12") && a[1].chars().count() <= 60);
        let at = |flag: &str| a.iter().position(|x| x == flag).unwrap_or_else(|| panic!("no {flag}"));
        assert_eq!(a[at("--repo") + 1], "planner");
        assert!(a.contains(&"--research".to_string()) && a.contains(&"--delegate".to_string()));
        assert_eq!(a[at("--after") + 1], "planner/12");
        assert_eq!(a[at("--worker.agent") + 1], "claude/claude-opus-5-5");
        assert_eq!(a[at("--worker.effort") + 1], "high");
        assert!(a[at("--worker.instructions") + 1].contains("инъекции"));
        let desc = &a[at("--desc") + 1];
        assert!(desc.contains("PLN-12") && desc.contains("work/12") && desc.contains("отчёт"));
        // none of the reviewer's own settings: this task is a worker of its own
        assert!(!a.iter().any(|x| x.starts_with("--reviewer.")));
    }

    #[test]
    fn a_new_task_gets_a_role_as_flags_of_add() {
        let mut s = Store::default();
        add_role(&mut s, &designer()).unwrap();
        let flags = add_flags(&s.roles[0], Way::Worker);
        assert_eq!(flags[..2], ["--worker.agent", "claude/claude-sonnet-5-5"]);
        assert_eq!(flags.len(), 8);
        let rv = security(&mut s);
        assert_eq!(add_flags(&rv, Way::Instead)[0], "--reviewer.instructions");
    }

    #[test]
    fn masks_and_words_match_the_title_and_the_branch() {
        assert!(glob_match("безопас", "Проверка безопасности входа"));
        assert!(glob_match("БЕЗОПАС", "проверка безопасности"));
        assert!(glob_match("feature/KIC-*", "feature/kic-12"));
        assert!(!glob_match("feature/KIC-*", "work/feature/KIC-12"), "a mask is for the whole text");
        assert!(glob_match("*KIC*", "work/KIC-3-x"));
        assert!(glob_match("a*b*c", "a--b--c"));
        assert!(!glob_match("a*b*c", "a--c--b"));
        assert!(!glob_match("ab*ba", "aba"), "the ends do not overlap");
        assert!(glob_match("*", "anything"));
    }

    #[test]
    fn the_first_rule_that_fits_gives_its_role_and_a_rule_needs_a_condition() {
        let mut s = Store::default();
        let designer_id = add_role(&mut s, &designer()).unwrap();
        let sec = security(&mut s);
        let by_title = add_rule(&mut s, &RuleInput { role: sec.id.clone(), title: "безопас".into(), ..RuleInput::default() }).unwrap();
        add_rule(&mut s, &RuleInput { role: designer_id.clone(), repo: "web".into(), ..RuleInput::default() }).unwrap();
        add_rule(&mut s, &RuleInput { role: designer_id.clone(), branch: "ui/*".into(), how: "".into(), ..RuleInput::default() }).unwrap();
        assert_eq!(rule_for(&s, "planner", "Проверка безопасности", "work/1").unwrap().id, by_title);
        assert_eq!(rule_for(&s, "web", "Кнопка", "work/1").unwrap().role, designer_id);
        assert_eq!(rule_for(&s, "planner", "Кнопка", "ui/button").unwrap().role, designer_id);
        assert!(rule_for(&s, "planner", "Кнопка", "work/1").is_none());
        assert!(add_rule(&mut s, &RuleInput { role: designer_id.clone(), ..RuleInput::default() }).unwrap_err().contains("без условий"));
        assert!(add_rule(&mut s, &RuleInput { role: "nope".into(), repo: "x".into(), ..RuleInput::default() }).is_err());
        // a rule may only name a way its role has
        assert!(add_rule(&mut s, &RuleInput { role: designer_id, repo: "x".into(), how: "after".into(), ..RuleInput::default() }).is_err());
        assert!(add_rule(&mut s, &RuleInput { role: sec.id, repo: "x".into(), how: "instead".into(), ..RuleInput::default() }).is_ok());
    }

    #[test]
    fn a_rule_says_in_words_what_it_looks_at() {
        let r = Rule { id: "x".into(), role: "r".into(), repo: "planner".into(), title: "*безопас*".into(), branch: "feature/*".into(), how: String::new() };
        assert_eq!(r.words(), "репозиторий planner · название: *безопас* · ветка: feature/*");
        assert!(r.matches("planner", "О безопасности", "feature/x"));
        assert!(!r.matches("web", "О безопасности", "feature/x"));
    }

    #[test]
    fn where_a_role_was_put_shows_on_the_row_and_one_place_holds_one_role() {
        let mut s = Store::default();
        let id = add_role(&mut s, &designer()).unwrap();
        let sec = security(&mut s);
        let rule = add_rule(&mut s, &RuleInput { role: id.clone(), repo: "planner".into(), ..RuleInput::default() }).unwrap();
        let put = |role: &str, name: &str, how: &str, by: &str| Assignment { task: "planner/12".into(), role: role.into(), name: name.into(), how: how.into(), by: by.into(), at: 1, ..Assignment::default() };
        record(&mut s, put(&id, "Дизайнер", "worker", &format!("rule:{rule}")));
        record(&mut s, put(&sec.id, "Ревьюер безопасности", "after", "chosen"));
        let marks = marks_of(&s);
        let m = &marks["planner/12"];
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].by.as_str(), m[0].rule.as_str()), ("rule", "репозиторий planner"));
        assert_eq!(m[1].by, "chosen");
        // another worker role takes the worker's place; the extra check stays
        record(&mut s, put("rother", "Другой", "worker", "chosen"));
        assert_eq!(marks_of(&s)["planner/12"].len(), 2);
        // a renamed role shows its new name; a deleted one keeps the name it had
        let name_of = |s: &Store| marks_of(s)["planner/12"].iter().find(|m| m.role == sec.id).unwrap().name.clone();
        s.roles.iter_mut().find(|r| r.id == sec.id).unwrap().name = "Безопасность".into();
        assert_eq!(name_of(&s), "Безопасность");
        delete_role(&mut s, &sec.id).unwrap();
        assert_eq!(name_of(&s), "Ревьюер безопасности");
    }

    #[test]
    fn a_role_in_use_shows_its_tasks_rules_and_presets_and_goes_with_its_rules() {
        let mut s = Store::default();
        let id = add_role(&mut s, &designer()).unwrap();
        add_rule(&mut s, &RuleInput { role: id.clone(), repo: "web".into(), ..RuleInput::default() }).unwrap();
        for (task, how) in [("web/1", "worker"), ("web/2", "worker"), ("web/3", "followup")] {
            record(&mut s, Assignment { task: task.into(), role: id.clone(), name: "Дизайнер".into(), how: how.into(), by: "chosen".into(), ..Assignment::default() });
        }
        let mut presets = crew::Store::default();
        crew::add_preset(&mut presets, "Дизайн", &capture(&s.roles)).unwrap();
        crew::add_preset(&mut presets, "Пустой", &Default::default()).unwrap();
        // web/1 is live, web/2 is done (not offered by `live`)
        let live = |t: &str| (t == "web/1").then(|| ("WEB-1".to_string(), "Кнопка".to_string(), "working".to_string()));
        let u = usage_of(&s, &presets, &id, &live);
        assert_eq!(u.tasks.len(), 1);
        assert_eq!(u.tasks[0].tag, "WEB-1");
        assert_eq!((u.rules, u.presets.clone()), (1, vec!["Дизайн".to_string()]));
        assert!(!u.is_empty());
        let gone = delete_role(&mut s, &id).unwrap();
        assert_eq!(gone.name, "Дизайнер");
        assert!(s.rules.is_empty(), "its rules go with it");
        assert_eq!(s.assigned.len(), 3, "the tasks keep what they were told");
        assert!(usage_of(&s, &presets, "rnone", &live).is_empty());
        assert!(delete_role(&mut s, &id).is_err());
    }

    #[test]
    fn a_role_on_tasks_keeps_its_kind() {
        let mut s = Store::default();
        let id = add_role(&mut s, &designer()).unwrap();
        record(&mut s, Assignment { task: "web/1".into(), role: id.clone(), how: "worker".into(), ..Assignment::default() });
        let as_reviewer = RoleInput { kind: "reviewer".into(), ..designer() };
        assert!(update_role(&mut s, &id, &as_reviewer).unwrap_err().contains("не меняется"));
        let renamed = RoleInput { name: "Дизайнер UI".into(), ..designer() };
        update_role(&mut s, &id, &renamed).unwrap();
        assert_eq!(s.roles[0].name, "Дизайнер UI");
    }

    #[test]
    fn a_preset_holds_the_roles_and_applying_it_changes_the_role_not_the_board() {
        let mut s = Store::default();
        let id = add_role(&mut s, &designer()).unwrap();
        let mut want = capture(&s.roles);
        want.insert(preset_key(&id), crew::RoleSet { agent: "codex/gpt-5.5".into(), effort: String::new(), fallback: "none".into() });
        let preset = crew::Preset { id: "p1".into(), name: "Дешёвый".into(), roles: want };
        let steps = preset_steps(&preset, &s.roles, "");
        let fields: Vec<(&str, &str, Option<&str>)> = steps.iter().map(|x| (x.field, x.was.as_str(), x.now.as_deref())).collect();
        assert_eq!(
            fields,
            [("agent", "claude/claude-sonnet-5-5", Some("codex/gpt-5.5")), ("effort", "high", None), ("fallback", "codex, claude/claude-haiku-5-5", Some("none"))]
        );
        assert!(steps.iter().all(|x| x.custom == id && x.label == "Дизайнер" && crew::command_of(x, "").is_none()));
        // the role is the planner's own: a repository's level has nothing to change
        assert!(preset_steps(&preset, &s.roles, "alpha").is_empty());
        for x in &steps {
            apply_step(&mut s, &x.custom, x.field, x.now.as_deref()).unwrap();
        }
        assert_eq!((s.roles[0].agent.as_str(), s.roles[0].effort.as_str(), s.roles[0].fallback.as_str()), ("codex/gpt-5.5", "", "none"));
        assert_eq!(s.roles[0].instructions, "Следи за единым видом интерфейса", "the focus is not in a preset");
        assert!(preset_steps(&preset, &s.roles, "").is_empty());
        // a role that is gone is left out
        s.roles.clear();
        assert!(preset_steps(&preset, &s.roles, "").is_empty());
        assert!(apply_step(&mut s, &id, "agent", None).is_err());
    }

    #[test]
    fn the_roles_file_is_kept_and_read_back() {
        let dir = std::env::temp_dir().join(format!("hp-roles-{}", now_ms()));
        let cfg = crate::testutil::test_cfg(&dir);
        assert!(read_store(&cfg).roles.is_empty());
        let id = edit_store(&cfg, |s| add_role(s, &designer())).unwrap();
        edit_store(&cfg, |s| add_rule(s, &RuleInput { role: id.clone(), repo: "web".into(), ..RuleInput::default() })).unwrap();
        let s = read_store(&cfg);
        assert_eq!((s.roles.len(), s.rules.len()), (1, 1));
        assert!(edit_store(&cfg, |s| add_role(s, &designer())).is_err(), "a refused change writes nothing");
        assert_eq!(read_store(&cfg).roles.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preset_keys_are_told_from_the_built_in_roles() {
        assert_eq!(role_of_key("role:r1f"), Some("r1f"));
        assert_eq!(role_of_key("worker"), None);
        assert_eq!(role_of_key("role:../x"), None);
    }
}
