//! Spares for the roles the board has no fallback for (PLN-89): the Coordinator, the planner (`plan.agent`) and the
//! light tasks. The board knows only `worker.fallback` and `reviewer.fallback`, so the planner keeps these chains
//! itself, in `crew-failover.json` of its state folder, and does the switching:
//!
//! - **Coordinator**: when its program cannot be given work (out of allowance, signed out, not installed), the next
//!   usable item of its chain is set with `set coordinator.agent X --main-coordinator` and `coordinator new`.
//! - **Planner**: the same with `set plan.agent X`.
//! - **Light tasks**: nothing runs in the background; a task made from the «Лёгкая» preset starts on the first usable
//!   item of [preset, chain…], and the items after it go to the task as its own `worker.fallback`.
//!
//! Every switch is written to the journal and shown as a toast. A switch is remembered (`switched`), and when the
//! agent it left is usable again the planner only *offers* to go back; it never goes back by itself.
//!
//! A role switches by itself only with its «переключать само» mark on (it is on for every chain the human sets) and
//! only when it has a chain. The decisions are pure functions below (unit-tested); `tick` is the loop's step.

use crate::app::{self, Shared};
use crate::config::{now_ms, AppConfig};
use crate::crew::{local_minute, program_of};
use crate::server::{error, same_origin};
use crate::station::{self, find, Fail, BOARD_WAIT};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// The roles whose spares the planner keeps. `light` is never switched in the background.
pub const ROLES: &[&str] = &["coordinator", "plan", "light"];

/// The roles the loop switches (the light tasks choose at the start of a task).
const SWITCHED_ROLES: &[&str] = &["coordinator", "plan"];

/// A failed switch is not tried again for this long.
const RETRY_MS: u64 = 10 * 60 * 1000;

fn yes() -> bool {
    true
}

/// A role's chain of spares and its «переключать само» mark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spare {
    /// `agent, program/model, …`, as a fallback is written; empty: no chain.
    #[serde(default)]
    pub chain: String,
    #[serde(default = "yes")]
    pub auto: bool,
}

impl Default for Spare {
    fn default() -> Self {
        Self { chain: String::new(), auto: true }
    }
}

/// A role that now runs on a spare, and what it left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Switched {
    /// The agent the role had before the first switch: where «вернуть» goes.
    pub home: String,
    /// The spare it runs on now.
    pub to: String,
    pub at: u64,
    /// Why it left: `limited`, `signed_out`, `missing`.
    pub kind: String,
    /// The board's words for when the allowance is back (`17:00`), or empty.
    #[serde(default)]
    pub until: String,
    /// The offer to go back has been made (a toast, once).
    #[serde(default)]
    pub offered: bool,
    /// The Coordinator had its own `coordinator.agent` in its layer before the first switch (`home` is that value).
    /// Otherwise `home` is the human's own value and a return clears the layer instead of pinning `home` into it.
    #[serde(default)]
    pub pinned_home: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub roles: BTreeMap<String, Spare>,
    #[serde(default)]
    pub switched: BTreeMap<String, Switched>,
}

impl Store {
    pub fn spare(&self, role: &str) -> Spare {
        self.roles.get(role).cloned().unwrap_or_default()
    }

    /// Whether the loop has anything to look at: a chain that may switch, or a switch that waits to be taken back.
    fn watching(&self) -> bool {
        SWITCHED_ROLES.iter().any(|r| {
            let s = self.spare(r);
            (s.auto && !items_of(&s.chain).is_empty()) || self.switched.contains_key(*r)
        })
    }
}

/* ───────── what is not available ───────── */

/// A program that cannot be given work now, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Gone {
    /// `limited`, `signed_out` or `missing`.
    pub kind: String,
    /// The board's words for when the allowance is back (`17:00`), or a clock time from Omarchy's window; may be empty.
    pub until: String,
}

/// The programs that are not available, by name. A program that is not in it can be given work.
pub type Down = BTreeMap<String, Gone>;

/// `17:20` for a moment in ms (the machine's own clock).
pub fn clock(ms: u64) -> String {
    let m = local_minute(ms);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/* ───────── the decisions (pure) ───────── */

/// A chain as its items: `none` and empty places are no items.
pub fn items_of(chain: &str) -> Vec<String> {
    chain
        .split(',')
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty() && i != "none")
        .collect()
}

/// The first item of the chain that can be given work: its program is not down, and it is not `current`.
pub fn next_in_chain(current: &str, items: &[String], down: &Down) -> Option<String> {
    items
        .iter()
        .find(|i| i.as_str() != current && !down.contains_key(program_of(i)))
        .cloned()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Leave things as they are.
    Nothing,
    /// The role's agent is down: run it on `to`. `home` is where a return goes; `from_program` is what is down.
    Switch { to: String, home: String, from_program: String, gone: Gone },
    /// The role's agent is down and the chain has nobody usable.
    NoSpare { program: String, gone: Gone },
    /// The agent the role left is usable again: offer to go back (once).
    OfferBack,
    /// The agent was changed by hand since the switch: forget the switch.
    Forget,
}

/// What to do for a role with the spares `spare`, `current` agent, the switch on record, and the programs that are down.
pub fn decide(spare: &Spare, current: &str, switched: Option<&Switched>, down: &Down) -> Decision {
    let may_switch = spare.auto && !items_of(&spare.chain).is_empty();
    let cur_program = program_of(current);
    let cur_down = (!cur_program.is_empty() && cur_program != "none").then(|| down.get(cur_program)).flatten();
    if let Some(sw) = switched {
        if current != sw.to {
            return Decision::Forget;
        }
        if cur_down.is_none() {
            let home_back = !down.contains_key(program_of(&sw.home));
            return if home_back && !sw.offered { Decision::OfferBack } else { Decision::Nothing };
        }
    }
    let Some(gone) = cur_down else { return Decision::Nothing };
    if !may_switch {
        return Decision::Nothing;
    }
    match next_in_chain(current, &items_of(&spare.chain), down) {
        Some(to) => Decision::Switch {
            to,
            home: switched.map(|s| s.home.clone()).unwrap_or_else(|| current.to_string()),
            from_program: cur_program.to_string(),
            gone: gone.clone(),
        },
        None => Decision::NoSpare { program: cur_program.to_string(), gone: gone.clone() },
    }
}

/// The board commands (after `herdr-projects`) that put `agent` on a role, in the order they run.
pub fn commands(role: &str, agent: &str) -> Vec<Vec<String>> {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match role {
        "coordinator" => vec![v(&["set", "coordinator.agent", agent, "--main-coordinator"]), v(&["coordinator", "new"])],
        "plan" => vec![v(&["set", "plan.agent", agent])],
        _ => Vec::new(),
    }
}

/// The commands of a return to `home`. The switch pinned `coordinator.agent` in the Coordinator's own layer; unless
/// that layer held `home` before, the return clears it (`unset … --main-coordinator`), so that the Coordinator follows
/// the human's own value again and a later choice in the Crew window is not shadowed.
pub fn return_commands(role: &str, home: &str, pinned_home: bool) -> Vec<Vec<String>> {
    if role == "coordinator" && !pinned_home {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        return vec![v(&["unset", "coordinator.agent", "--main-coordinator"]), v(&["coordinator", "new"])];
    }
    commands(role, home)
}

/// «Оставить так»: the spare stays, and the Coordinator's agent moves from its own layer into the human's own, so
/// that the Crew window shows what runs (no new Coordinator: it already runs on the spare). Nothing for the plan.
pub fn keep_commands(role: &str, to: &str, pinned_home: bool) -> Vec<Vec<String>> {
    if role == "coordinator" && !pinned_home {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        return vec![v(&["set", "coordinator.agent", to]), v(&["unset", "coordinator.agent", "--main-coordinator"])];
    }
    Vec::new()
}

/// The agent a light task starts on, and the spares it carries as its own `worker.fallback`.
#[derive(Debug, Clone, PartialEq)]
pub struct LightPick {
    pub agent: String,
    /// `a, b` (what is left of the chain after `agent`), or empty.
    pub fallback: String,
}

/// The first usable agent of [preset, chain…] and the rest of the chain after it; with every one down, the preset and
/// the whole chain. `None`: nothing to change (no chain, or the switching is off).
pub fn pick_light(light: &str, spare: &Spare, down: &Down) -> Option<LightPick> {
    let items = items_of(&spare.chain);
    if !spare.auto || items.is_empty() || light.is_empty() {
        return None;
    }
    let mut all = vec![light.to_string()];
    all.extend(items.into_iter().filter(|i| i != light));
    // Nobody usable: the task starts on the preset all the same and carries the whole chain, for when one comes back.
    let at = all.iter().position(|a| !down.contains_key(program_of(a))).unwrap_or(0);
    Some(LightPick { agent: all[at].clone(), fallback: all[at + 1..].join(", ") })
}

/* ───────── the words ───────── */

pub fn role_title(role: &str) -> &'static str {
    match role {
        "coordinator" => "Координатор",
        "plan" => "Планировщик",
        _ => "Лёгкие задачи",
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Why a program is down, in words: «лимит Claude до 17:00».
pub fn why_words(program: &str, g: &Gone) -> String {
    let name = cap(program);
    match g.kind.as_str() {
        "limited" if g.until.is_empty() => format!("лимит {name}"),
        "limited" => format!("лимит {name} до {}", g.until),
        "signed_out" => format!("{name}: вход не выполнен"),
        _ => format!("{name} не установлен"),
    }
}

/// «Координатор: claude → codex/gpt-6-astra, лимит Claude до 17:00».
pub fn switch_line(role: &str, from: &str, to: &str, program: &str, g: &Gone) -> String {
    format!("{}: {} → {}, {}", role_title(role), from, to, why_words(program, g))
}

/* ───────── the file ───────── */

static FILE_LOCK: Mutex<()> = Mutex::new(());

fn file_of(cfg: &AppConfig) -> PathBuf {
    cfg.state_dir.join("crew-failover.json")
}

fn read_at(path: &std::path::Path) -> Store {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Store>(&t).ok())
        .unwrap_or_default()
}

pub fn read_store(cfg: &AppConfig) -> Store {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read_at(&file_of(cfg))
}

/// Reads the file, lets `f` change it, writes it back (a temporary file, then a rename) — all under one lock.
pub fn edit_store<T>(cfg: &AppConfig, f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = file_of(cfg);
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

/* ───────── what the page shows ───────── */

/// The spares of the three roles for the station: the chain, the mark, the switch on record, whether the agent it
/// left is back, and (light) the agent a task would start on now.
pub fn page_json(cfg: &AppConfig, light: &str, down: &Down) -> Value {
    let store = read_store(cfg);
    let mut out = serde_json::Map::new();
    for role in ROLES {
        let spare = store.spare(role);
        let mut o = json!({"chain": spare.chain, "auto": spare.auto, "switched": Value::Null});
        if let Some(sw) = store.switched.get(*role) {
            let home_program = program_of(&sw.home);
            o["switched"] = json!({
                "home": sw.home, "to": sw.to, "at": sw.at, "kind": sw.kind, "until": sw.until,
                "back_ready": !down.contains_key(home_program),
                "words": why_words(home_program, &Gone { kind: sw.kind.clone(), until: sw.until.clone() }),
            });
        }
        if *role == "light" {
            o["use"] = json!(pick_light(light, &spare, down).filter(|p| p.agent != light).map(|p| p.agent).unwrap_or_default());
        }
        out.insert(role.to_string(), o);
    }
    Value::Object(out)
}

/* ───────── the loop ───────── */

/// The programs installed here (a test says its own).
fn installed() -> Vec<String> {
    #[cfg(test)]
    return ["claude", "codex", "omp"].map(String::from).to_vec();
    #[cfg(not(test))]
    station::available()
}

/// The agent a role has now, and whether it stands in the Coordinator's own layer. The Coordinator's own layer is read
/// with `show --main-coordinator` (that is where `set coordinator.agent … --main-coordinator` writes; a plain `show`
/// does not see it); the plan's with a plain `show`.
async fn current_agent(app: &Shared, role: &str) -> Result<(String, bool), Fail> {
    let settings = if role == "coordinator" {
        station::read_coordinator_settings(&app.cfg).await
    } else {
        station::read_settings(&app.cfg, "").await
    }
    .map_err(Fail::refused)?;
    let it = find(&settings, &format!("{role}.agent"));
    Ok((it.map(|s| s.value.clone()).unwrap_or_default(), it.is_some_and(|s| s.place.contains("coord") || s.from.contains("coord"))))
}

/// The last failed switch of each role (ms), and the dead ends already told (`role|program`): the loop runs every
/// minute and does not say or try the same thing again. Keyed by the state folder as well as the role.
static FAILED: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());
static TOLD: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

fn memo(app: &Shared, role: &str) -> String {
    format!("{}|{role}", app.cfg.state_dir.display())
}

async fn say(app: &Shared, line: &str, toast: Value) {
    app::journal_note(app, "crew", "crew", line).await;
    app.bus.send("toast", toast.to_string());
}

/// Runs the commands that put `agent` on `role`. The first refusal stops the run: the error says which command it was
/// (0 is the first; after a later one the setting has already changed).
async fn run_commands(app: &Shared, list: Vec<Vec<String>>) -> Result<(), (usize, String)> {
    for (i, args) in list.into_iter().enumerate() {
        let out = station::board_run(&app.cfg, &args, BOARD_WAIT).await;
        if !out.ok() {
            return Err((i, format!("herdr-projects {}: {}", args.join(" "), out.words())));
        }
    }
    Ok(())
}

async fn do_switch(app: &Shared, role: &str, to: &str, home: &str, pinned_home: bool, from_program: &str, from_agent: &str, gone: &Gone) {
    let now = now_ms();
    {
        let failed = FAILED.lock().unwrap_or_else(|e| e.into_inner());
        if failed.get(&memo(app, role)).is_some_and(|at| now.saturating_sub(*at) < RETRY_MS) {
            return;
        }
    }
    let _one_at_a_time = station::APPLY.lock().await;
    let mut half_done = false;
    if let Err((at, why)) = run_commands(app, commands(role, to)).await {
        FAILED.lock().unwrap_or_else(|e| e.into_inner()).insert(memo(app, role), now);
        app::journal_note(app, "crew", "crew", &format!("{}: не удалось перейти на {to}: {why}", role_title(role))).await;
        // The agent is already set when only a later command failed: the switch is kept, so that it can be taken back.
        half_done = at > 0;
        if !half_done {
            app::refresh(app).await;
            return;
        }
    }
    if !half_done {
        FAILED.lock().unwrap_or_else(|e| e.into_inner()).remove(&memo(app, role));
    }
    TOLD.lock().unwrap_or_else(|e| e.into_inner()).remove(&memo(app, role));
    let sw = Switched {
        home: home.to_string(),
        to: to.to_string(),
        at: now,
        kind: gone.kind.clone(),
        until: gone.until.clone(),
        offered: false,
        pinned_home,
    };
    let _ = edit_store(&app.cfg, |s| {
        s.switched.insert(role.to_string(), sw);
        Ok(())
    });
    let line = switch_line(role, from_agent, to, from_program, gone);
    say(
        app,
        &line,
        json!({
            "key": format!("crew.fail.{role}.{}", if gone.kind == "limited" && !gone.until.is_empty() { "limited" } else { "down" }),
            "vars": {"from": from_agent, "to": to, "prog": from_program, "until": gone.until},
            "icon": "failed",
        }),
    )
    .await;
    app::refresh(app).await;
}

async fn step(app: &Shared, role: &str, store: &Store, down: &Down) {
    let Ok((current, own_layer)) = current_agent(app, role).await else { return };
    let sw = store.switched.get(role);
    match decide(&store.spare(role), &current, sw, down) {
        Decision::Nothing => {
            TOLD.lock().unwrap_or_else(|e| e.into_inner()).remove(&memo(app, role));
        }
        Decision::Forget => {
            let _ = edit_store(&app.cfg, |s| {
                s.switched.remove(role);
                Ok(())
            });
        }
        Decision::OfferBack => {
            let Some(sw) = sw else { return };
            let _ = edit_store(&app.cfg, |s| {
                if let Some(x) = s.switched.get_mut(role) {
                    x.offered = true;
                }
                Ok(())
            });
            let line = format!(
                "{}: {} снова доступен — можно вернуть с {} (кнопка «Вернуть» в окне «Экипаж»)",
                role_title(role),
                sw.home,
                sw.to
            );
            say(
                app,
                &line,
                json!({"key": format!("crew.back.{role}"), "vars": {"home": sw.home, "to": sw.to}, "icon": "done"}),
            )
            .await;
        }
        Decision::Switch { to, home, from_program, gone } => {
            let pinned_home = sw.map(|s| s.pinned_home).unwrap_or(own_layer);
            do_switch(app, role, &to, &home, pinned_home, &from_program, &current, &gone).await;
        }
        Decision::NoSpare { program, gone } => {
            let key = format!("{program}|{}", gone.kind);
            let told = TOLD.lock().unwrap_or_else(|e| e.into_inner()).insert(memo(app, role), key.clone());
            if told.as_deref() != Some(&key) {
                let line = format!("{}: {}, а запасных, которые можно запустить, в цепочке нет", role_title(role), why_words(&program, &gone));
                app::journal_note(app, "crew", "crew", &line).await;
            }
        }
    }
}

/// One look: for the Coordinator and the planner, switch what is down and offer to return what is back.
pub async fn tick_with(app: &Shared, installed: &[String]) {
    if !read_store(&app.cfg).watching() {
        return;
    }
    let Ok(down) = station::down_programs(app, installed).await else { return };
    // The store is read again after the (slow) reads, and the same state serves both roles.
    let store = read_store(&app.cfg);
    for role in SWITCHED_ROLES {
        step(app, role, &store, &down).await;
    }
}

pub async fn tick(app: &Shared) {
    tick_with(app, &installed()).await;
}

/// The background loop: a look once a minute.
pub async fn run(app: Shared) {
    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
    loop {
        tick(&app).await;
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// For a light task about to start on `agent`: when it is the «Лёгкая» preset and the role has a chain, the agent it
/// starts on and the `worker.fallback` it carries. `None` leaves the task as it was asked for.
pub async fn light_start(app: &Shared, agent: &str) -> Option<LightPick> {
    let spare = read_store(&app.cfg).spare("light");
    if !spare.auto || items_of(&spare.chain).is_empty() {
        return None;
    }
    let light = station::read_light(&app.cfg);
    if light.is_empty() || agent != light {
        return None;
    }
    let down = station::down_programs(app, &installed()).await.ok()?;
    pick_light(&light, &spare, &down)
}

/* ───────── the endpoints ───────── */

#[derive(Deserialize)]
pub struct SpareBody {
    role: String,
    /// The new chain (`none` or empty clears it).
    chain: Option<String>,
    auto: Option<bool>,
    confirm: Option<bool>,
}

fn check_role(role: &str) -> Result<(), Fail> {
    if ROLES.contains(&role) {
        Ok(())
    } else {
        Err(Fail::new(400, format!("«{role}»: у этой роли запасных планировщик не держит (Ревьюер и Тяжёлые задачи — на доске)")))
    }
}

/// `POST /api/station/failover`: the chain of a role and its «переключать само» mark.
pub async fn failover_set(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<SpareBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let result = async {
        if body.confirm != Some(true) {
            return Err(Fail::new(400, "нужно подтверждение: было → стало"));
        }
        check_role(&body.role)?;
        let chain = body.chain.as_deref().map(|c| items_of(c).join(", "));
        if let Some(c) = chain.as_deref().filter(|c| !c.is_empty()) {
            if let Err(e) = station::check_chain(&app, c).await {
                return Err(Fail::new(422, e));
            }
        }
        edit_store(&app.cfg, |s| {
            let spare = s.roles.entry(body.role.clone()).or_default();
            if let Some(c) = &chain {
                spare.chain = c.clone();
            }
            if let Some(a) = body.auto {
                spare.auto = a;
            }
            if *spare == Spare::default() {
                s.roles.remove(&body.role);
            }
            Ok(())
        })
        .map_err(|e| Fail::new(500, e))?;
        let spare = read_store(&app.cfg).spare(&body.role);
        Ok::<Value, Fail>(json!({"ok": true, "role": body.role, "chain": spare.chain, "auto": spare.auto}))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[cfg(test)]
impl SpareBody {
    pub fn test(role: &str, chain: Option<&str>, auto: Option<bool>) -> Self {
        Self { role: role.into(), chain: chain.map(String::from), auto, confirm: Some(true) }
    }
}

#[cfg(test)]
impl SpareBody {
    pub fn unconfirmed(role: &str, chain: Option<&str>) -> Self {
        Self { confirm: None, ..Self::test(role, chain, None) }
    }
}

#[cfg(test)]
impl BackBody {
    pub fn test(op: &str, role: &str) -> Self {
        Self { role: role.into(), op: op.into(), confirm: Some(true) }
    }
}

#[cfg(test)]
pub async fn back_for_test(app: &Shared, body: BackBody, installed: &[String]) -> Response {
    back_with(app, body, installed).await
}

#[derive(Deserialize)]
pub struct BackBody {
    role: String,
    /// `back` (the commands run) or `keep` (stay on the spare; the offer is forgotten).
    op: String,
    confirm: Option<bool>,
}

/// `POST /api/station/failover/back`: go back to the agent the role left, or stay where it is.
pub async fn failover_back(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<BackBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    back_with(&app, body, &installed()).await
}

/// The return, with the programs installed here given (a test names its own).
async fn back_with(app: &Shared, body: BackBody, installed: &[String]) -> Response {
    let app = app.clone();
    let result = async {
        if body.confirm != Some(true) {
            return Err(Fail::new(400, "нужно подтверждение: было → стало"));
        }
        check_role(&body.role)?;
        let role = body.role.as_str();
        if !SWITCHED_ROLES.contains(&role) {
            return Err(Fail::new(400, "у лёгких задач переключения назад нет: каждая задача выбирает сама"));
        }
        let sw = read_store(&app.cfg)
            .switched
            .get(role)
            .cloned()
            .ok_or_else(|| Fail::new(409, "эта роль не переключена"))?;
        if body.op == "keep" {
            let _one_at_a_time = station::APPLY.lock().await;
            run_commands(&app, keep_commands(role, &sw.to, sw.pinned_home)).await.map_err(|(_, e)| Fail::new(422, e))?;
            edit_store(&app.cfg, |s| {
                s.switched.remove(role);
                Ok(())
            })
            .map_err(|e| Fail::new(500, e))?;
            return Ok(json!({"ok": true, "role": role, "agent": sw.to}));
        }
        if body.op != "back" {
            return Err(Fail::new(400, "op: back или keep"));
        }
        let _one_at_a_time = station::APPLY.lock().await;
        let (current, _) = current_agent(&app, role).await?;
        if current != sw.to {
            edit_store(&app.cfg, |s| {
                s.switched.remove(role);
                Ok(())
            })
            .map_err(|e| Fail::new(500, e))?;
            return Err(Fail::new(409, format!("агент роли уже сменили вручную ({current}): возвращать нечего")));
        }
        let down = station::down_programs(&app, installed).await?;
        if let Some(g) = down.get(program_of(&sw.home)) {
            return Err(Fail::new(409, format!("{} ещё недоступен: {}", sw.home, why_words(program_of(&sw.home), g))));
        }
        run_commands(&app, return_commands(role, &sw.home, sw.pinned_home)).await.map_err(|(_, e)| Fail::new(422, e))?;
        edit_store(&app.cfg, |s| {
            s.switched.remove(role);
            Ok(())
        })
        .map_err(|e| Fail::new(500, e))?;
        app::journal_note(&app, "crew", "crew", &format!("{}: {} → {} (вернули по вашему слову)", role_title(role), sw.to, sw.home)).await;
        app::refresh(&app).await;
        Ok::<Value, Fail>(json!({"ok": true, "role": role, "agent": sw.home}))
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(f) => f.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(list: &[(&str, &str, &str)]) -> Down {
        list.iter().map(|(p, k, u)| (p.to_string(), Gone { kind: k.to_string(), until: u.to_string() })).collect()
    }

    fn spare(chain: &str) -> Spare {
        Spare { chain: chain.into(), auto: true }
    }

    fn sw(home: &str, to: &str) -> Switched {
        Switched { home: home.into(), to: to.into(), at: 1, kind: "limited".into(), until: "17:00".into(), offered: false, pinned_home: false }
    }

    #[test]
    fn the_next_spare_is_the_first_whose_program_can_be_given_work() {
        let items = items_of("claude/claude-sonnet-5-5, codex/gpt-6-astra, omp");
        assert_eq!(items.len(), 3);
        let d = down(&[("claude", "limited", "17:00")]);
        assert_eq!(next_in_chain("claude/claude-opus-5-5", &items, &d).as_deref(), Some("codex/gpt-6-astra"), "the same program is down too");
        let d = down(&[("claude", "limited", ""), ("codex", "signed_out", "")]);
        assert_eq!(next_in_chain("claude/claude-opus-5-5", &items, &d).as_deref(), Some("omp"), "unavailable ones are skipped");
        let d = down(&[("claude", "limited", ""), ("codex", "limited", ""), ("omp", "missing", "")]);
        assert_eq!(next_in_chain("claude/claude-opus-5-5", &items, &d), None);
        assert_eq!(next_in_chain("omp", &items, &down(&[])).as_deref(), Some("claude/claude-sonnet-5-5"), "never the agent it already runs");
        assert!(items_of("none").is_empty() && items_of("").is_empty());
    }

    #[test]
    fn a_role_whose_agent_is_down_switches_to_the_next_usable_spare() {
        let d = down(&[("claude", "limited", "17:00"), ("codex", "limited", "")]);
        match decide(&spare("codex/gpt-6-astra, omp/deepseek/deepseek-v4-flash"), "claude/claude-opus-5-5", None, &d) {
            Decision::Switch { to, home, from_program, gone } => {
                assert_eq!(to, "omp/deepseek/deepseek-v4-flash");
                assert_eq!(home, "claude/claude-opus-5-5");
                assert_eq!(from_program, "claude");
                assert_eq!(gone.until, "17:00");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn nothing_happens_without_a_chain_without_the_mark_or_while_the_agent_is_usable() {
        let d = down(&[("claude", "limited", "17:00")]);
        assert_eq!(decide(&spare(""), "claude/claude-opus-5-5", None, &d), Decision::Nothing, "no chain: no action");
        assert_eq!(decide(&spare("none"), "claude/claude-opus-5-5", None, &d), Decision::Nothing);
        let off = Spare { chain: "codex".into(), auto: false };
        assert_eq!(decide(&off, "claude/claude-opus-5-5", None, &d), Decision::Nothing, "the mark is off");
        assert_eq!(decide(&spare("codex"), "claude/claude-opus-5-5", None, &down(&[])), Decision::Nothing, "agent is usable");
        assert_eq!(decide(&spare("codex"), "none", None, &d), Decision::Nothing, "a role that is off stays off");
        assert_eq!(decide(&spare("codex"), "", None, &d), Decision::Nothing);
    }

    #[test]
    fn a_chain_with_nobody_usable_is_a_dead_end_not_a_switch() {
        let d = down(&[("claude", "limited", ""), ("codex", "signed_out", "")]);
        assert!(matches!(decide(&spare("codex"), "claude/claude-opus-5-5", None, &d), Decision::NoSpare { .. }));
    }

    #[test]
    fn the_return_is_offered_once_when_the_agent_it_left_is_usable_again_and_never_made_silently() {
        let s = spare("codex/gpt-6-astra");
        let still = down(&[("claude", "limited", "17:00")]);
        assert_eq!(decide(&s, "codex/gpt-6-astra", Some(&sw("claude/claude-opus-5-5", "codex/gpt-6-astra")), &still), Decision::Nothing);
        let back = down(&[]);
        assert_eq!(decide(&s, "codex/gpt-6-astra", Some(&sw("claude/claude-opus-5-5", "codex/gpt-6-astra")), &back), Decision::OfferBack);
        let mut offered = sw("claude/claude-opus-5-5", "codex/gpt-6-astra");
        offered.offered = true;
        assert_eq!(decide(&s, "codex/gpt-6-astra", Some(&offered), &back), Decision::Nothing, "told once");
    }

    #[test]
    fn a_spare_that_goes_down_passes_the_role_on_and_keeps_the_home() {
        let s = spare("codex/gpt-6-astra, omp");
        let d = down(&[("claude", "limited", ""), ("codex", "limited", "19:00")]);
        match decide(&s, "codex/gpt-6-astra", Some(&sw("claude/claude-opus-5-5", "codex/gpt-6-astra")), &d) {
            Decision::Switch { to, home, .. } => {
                assert_eq!(to, "omp");
                assert_eq!(home, "claude/claude-opus-5-5", "a return still goes to where the role began");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_hand_made_change_ends_the_switch() {
        let d = down(&[]);
        assert_eq!(decide(&spare("codex"), "omp", Some(&sw("claude/claude-opus-5-5", "codex/gpt-6-astra")), &d), Decision::Forget);
    }

    #[test]
    fn the_boards_commands_for_each_role() {
        assert_eq!(
            commands("coordinator", "codex/gpt-6-astra"),
            vec![
                vec!["set", "coordinator.agent", "codex/gpt-6-astra", "--main-coordinator"],
                vec!["coordinator", "new"],
            ]
        );
        assert_eq!(commands("plan", "codex/gpt-6-astra"), vec![vec!["set", "plan.agent", "codex/gpt-6-astra"]]);
        assert!(commands("light", "x").is_empty(), "light tasks have no setting to switch");
    }

    #[test]
    fn a_return_clears_the_coordinators_own_layer_unless_it_held_the_home_agent_before() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            return_commands("coordinator", "claude/claude-opus-5-5", false),
            vec![v(&["unset", "coordinator.agent", "--main-coordinator"]), v(&["coordinator", "new"])],
            "the human's own value shows again; nothing is pinned"
        );
        assert_eq!(
            return_commands("coordinator", "claude/claude-opus-5-5", true),
            vec![v(&["set", "coordinator.agent", "claude/claude-opus-5-5", "--main-coordinator"]), v(&["coordinator", "new"])],
            "a value the layer held before is put back"
        );
        assert_eq!(return_commands("plan", "claude/claude-opus-5-5", false), vec![v(&["set", "plan.agent", "claude/claude-opus-5-5"])]);
        assert_eq!(
            keep_commands("coordinator", "omp/x/y", false),
            vec![v(&["set", "coordinator.agent", "omp/x/y"]), v(&["unset", "coordinator.agent", "--main-coordinator"])]
        );
        assert!(keep_commands("coordinator", "omp/x/y", true).is_empty() && keep_commands("plan", "omp", false).is_empty());
    }

    #[test]
    fn a_light_task_starts_on_the_first_usable_agent_and_carries_the_rest() {
        let s = spare("omp/deepseek/deepseek-v4-flash, codex/gpt-6-astra");
        let light = "claude/claude-haiku-5-5";
        assert_eq!(
            pick_light(light, &s, &down(&[])),
            Some(LightPick { agent: light.into(), fallback: "omp/deepseek/deepseek-v4-flash, codex/gpt-6-astra".into() }),
            "usable preset: it stays first and the whole chain goes to the task as its fallback"
        );
        let d = down(&[("claude", "limited", "")]);
        assert_eq!(
            pick_light(light, &s, &d),
            Some(LightPick { agent: "omp/deepseek/deepseek-v4-flash".into(), fallback: "codex/gpt-6-astra".into() })
        );
        let d = down(&[("claude", "limited", ""), ("omp", "signed_out", "")]);
        assert_eq!(pick_light(light, &s, &d).unwrap().agent, "codex/gpt-6-astra");
        assert_eq!(pick_light(light, &s, &d).unwrap().fallback, "");
        let all = down(&[("claude", "limited", ""), ("omp", "signed_out", ""), ("codex", "missing", "")]);
        assert_eq!(
            pick_light(light, &s, &all),
            Some(LightPick { agent: light.into(), fallback: "omp/deepseek/deepseek-v4-flash, codex/gpt-6-astra".into() }),
            "nobody usable: the preset, with the whole chain for when one comes back"
        );
        assert_eq!(pick_light(light, &spare(""), &d), None, "no chain: no action");
        assert_eq!(pick_light(light, &Spare { chain: "codex".into(), auto: false }, &d), None);
    }

    #[test]
    fn the_words_of_a_switch() {
        let g = Gone { kind: "limited".into(), until: "17:00".into() };
        assert_eq!(
            switch_line("coordinator", "claude/claude-opus-5-5", "codex/gpt-6-astra", "claude", &g),
            "Координатор: claude/claude-opus-5-5 → codex/gpt-6-astra, лимит Claude до 17:00"
        );
        assert_eq!(why_words("codex", &Gone { kind: "signed_out".into(), until: String::new() }), "Codex: вход не выполнен");
    }

    #[test]
    fn the_chains_are_kept_in_a_file_and_the_mark_defaults_to_on() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::testutil::test_cfg(dir.path());
        assert!(read_store(&cfg).roles.is_empty());
        edit_store(&cfg, |s| {
            s.roles.insert("coordinator".into(), Spare { chain: "codex".into(), ..Spare::default() });
            Ok(())
        })
        .unwrap();
        let s = read_store(&cfg);
        assert!(s.spare("coordinator").auto, "on for a chain the human set");
        assert_eq!(s.spare("coordinator").chain, "codex");
        assert!(s.watching());
        assert!(!s.spare("plan").auto || s.spare("plan").chain.is_empty());
    }
}
