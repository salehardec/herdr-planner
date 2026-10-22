//! Auto-cleaning of a shipyard: the containers, networks and volumes of finished tasks (the `herdr.task` label) go by
//! themselves once the task has been finished for the set time. Off by default; the switch is the consent.
//!
//! It touches only what the manual «убрать забытое» would offer without a tick: a container the board's classes call
//! forgotten (never active, unsure or foreign), with the label, and whose task left long enough ago. Networks and
//! volumes go when they carry the label of such a task, or belong to such a container's compose project.

use crate::app::Shared;
use crate::config::now_ms;
use crate::runners::{self, Class, Net, Runner, RunnerState, TaskIndex, Vol};
use crate::runners_api::{safe_id, safe_volume};
use crate::ssh::{self, MachineRef};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;

pub const DEFAULT_HOURS: u64 = 6;
pub const MAX_HOURS: u64 = 24 * 90;
const AUTO_WAIT: Duration = Duration::from_secs(90);
/// The same plan that failed is not tried again for this long.
const RETRY_MS: u64 = 30 * 60 * 1000;
/// A read older than this is not a ground for removing anything.
const FRESH_MS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoClean {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_hours")]
    pub hours: u64,
}

fn default_hours() -> u64 {
    DEFAULT_HOURS
}

impl Default for AutoClean {
    fn default() -> Self {
        Self { enabled: false, hours: DEFAULT_HOURS }
    }
}

/// What the auto-clean has done on one shipyard, since it was first switched on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AutoStats {
    #[serde(default)]
    pub containers: u64,
    #[serde(default)]
    pub networks: u64,
    #[serde(default)]
    pub volumes: u64,
    /// How many times it removed anything.
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub last_ms: u64,
    #[serde(default)]
    pub last_line: String,
}

pub fn meta_key(id: &str) -> String {
    format!("runner_auto:{id}")
}

pub fn stats_of(app: &Shared, id: &str) -> AutoStats {
    app.db
        .meta_get(&meta_key(id))
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoPlan {
    pub containers: Vec<(String, String)>,
    pub networks: Vec<String>,
    pub volumes: Vec<String>,
    pub tags: BTreeSet<String>,
}

impl AutoPlan {
    pub fn is_empty(&self) -> bool {
        self.containers.is_empty() && self.networks.is_empty() && self.volumes.is_empty()
    }

    fn signature(&self) -> String {
        let mut parts: Vec<&str> = self.containers.iter().map(|(id, _)| id.as_str()).collect();
        parts.extend(self.networks.iter().map(String::as_str));
        parts.extend(self.volumes.iter().map(String::as_str));
        parts.sort_unstable();
        parts.join(",")
    }
}

fn due(finished_ms: u64, now: u64, hours: u64) -> bool {
    finished_ms > 0 && now.saturating_sub(finished_ms) >= hours.saturating_mul(3_600_000)
}

fn builtin_network(name: &str) -> bool {
    matches!(name, "bridge" | "host" | "none")
}

/// A unit that carries the label of a task that has not finished, or one that finished less than `hours` ago, stays.
fn label_blocks(labels: &std::collections::BTreeMap<String, String>, index: &TaskIndex, now: u64, hours: u64) -> bool {
    match labels.get("herdr.task") {
        Some(v) => !index.finished_label(v).is_some_and(|f| due(f, now, hours)),
        None => false,
    }
}

/// What the auto-clean would remove from the last read of a shipyard. Pure: nothing is sent anywhere.
pub fn plan_of(state: &RunnerState, index: &TaskIndex, hours: u64, now: u64) -> AutoPlan {
    let mut plan = AutoPlan::default();
    let mut ids: Vec<String> = Vec::new();
    for c in &state.containers {
        if c.class != Class::Forgotten || !c.labels.contains_key("herdr.task") || !safe_id(&c.id) {
            continue;
        }
        let Some(t) = &c.task else { continue };
        if !due(t.finished_ms, now, hours) {
            continue;
        }
        ids.push(c.id.clone());
        plan.tags.insert(t.tag.to_uppercase());
        plan.containers.push((c.id.clone(), c.name.clone()));
    }
    let mut nets: BTreeSet<String> = BTreeSet::new();
    let mut vols: BTreeSet<String> = BTreeSet::new();
    // The units that belong to those containers (their compose project, their tag in the name)…
    if !ids.is_empty() {
        let attached = runners::plan_of(state, Some(&ids));
        for n in &attached.networks {
            if let Some(net) = state.networks.iter().find(|x| x.id == n.id) {
                nets.insert(net.name.clone());
            }
        }
        for v in &attached.volumes {
            vols.insert(v.name.clone());
        }
    }
    // …and the ones that carry the label of a finished task themselves, even when no container is left.
    for n in &state.networks {
        if n.labels.get("herdr.task").is_some_and(|v| index.finished_label(v).is_some_and(|f| due(f, now, hours))) {
            nets.insert(n.name.clone());
        }
    }
    for v in &state.volumes {
        if v.labels.get("herdr.task").is_some_and(|l| index.finished_label(l).is_some_and(|f| due(f, now, hours))) {
            vols.insert(v.name.clone());
        }
    }
    let net_of = |name: &str| -> Option<&Net> { state.networks.iter().find(|n| n.name == name) };
    let vol_of = |name: &str| -> Option<&Vol> { state.volumes.iter().find(|v| v.name == name) };
    plan.networks = nets
        .into_iter()
        .filter(|name| !builtin_network(name) && safe_volume(name))
        .filter(|name| {
            net_of(name).is_some_and(|n| !runners::name_protected(&n.name, &n.labels, &state.protected) && !label_blocks(&n.labels, index, now, hours))
        })
        .collect();
    plan.volumes = vols
        .into_iter()
        .filter(|name| safe_volume(name))
        .filter(|name| {
            vol_of(name).is_some_and(|v| !runners::name_protected(&v.name, &v.labels, &state.protected) && !label_blocks(&v.labels, index, now, hours))
        })
        .collect();
    plan
}

fn machine_of(r: &Runner) -> MachineRef {
    MachineRef { label: r.id.clone(), target: r.target.clone(), session: String::new() }
}

async fn ran(app: &Shared, m: &MachineRef, cmd: &str) -> Result<(), String> {
    match ssh::ssh_ended(&app.cfg, m, cmd, AUTO_WAIT).await {
        Ok(e) if !e.timed_out && e.error.is_empty() && e.code == Some(0) => Ok(()),
        Ok(e) => Err(if e.stderr.trim().is_empty() { e.stdout.trim().to_string() } else { e.stderr.trim().to_string() }),
        Err(e) => Err(e),
    }
}

/// Reads the shipyard again after a removal and stores the read; the numbers of `rebuild_runners` follow it.
async fn reread(app: &Shared, r: &Runner, prev: &RunnerState) -> RunnerState {
    let fresh = runners::probe(&app.cfg, r).await;
    let kept = runners::keep_last(Some(prev), fresh);
    if let Ok(json) = serde_json::to_string(&kept) {
        app.db.runner_put(&kept.id, &json, kept.ok, kept.why.as_ref().map(|w| w.line.as_str()).unwrap_or(""));
    }
    let mut list = app.runners.lock().await;
    list.retain(|s| s.id != kept.id);
    list.push(kept.clone());
    list.sort_by(|a, b| a.id.cmp(&b.id));
    kept
}

/// One pass over one shipyard. Returns how many containers, networks and volumes went.
pub async fn run_one(app: &Shared, r: &Runner) -> (usize, usize, usize) {
    let now = now_ms();
    let (state, index, board_ok) = {
        let snap = app.snapshot.read().await;
        let repos = crate::taskmeta::repos(&app.cfg.data_dir);
        let index = TaskIndex::build(&snap.tasks, &snap.archived, &repos);
        (snap.runners.iter().find(|s| s.id == r.id).cloned(), index, snap.board.ok)
    };
    let Some(state) = state else { return (0, 0, 0) };
    // No guessing from an old read, or from a board that could not be read (its tasks would look finished or gone).
    if !state.ok || !board_ok || now.saturating_sub(state.fetched_at_ms) > FRESH_MS {
        return (0, 0, 0);
    }
    let plan = plan_of(&state, &index, r.auto.hours.clamp(1, MAX_HOURS), now);
    if plan.is_empty() {
        return (0, 0, 0);
    }
    {
        let mut tried = app.runner_auto_try.lock().await;
        let sig = plan.signature();
        if tried.get(&r.id).is_some_and(|(s, at)| *s == sig && now.saturating_sub(*at) < RETRY_MS) {
            return (0, 0, 0);
        }
        tried.insert(r.id.clone(), (sig, now));
    }
    let m = machine_of(r);
    let mut errors: Vec<String> = Vec::new();
    if !plan.containers.is_empty() {
        let ids: Vec<&str> = plan.containers.iter().map(|(id, _)| id.as_str()).collect();
        if let Err(e) = ran(app, &m, &format!("docker rm -f -v {}", ids.join(" "))).await {
            errors.push(e);
        }
    }
    for chunk in plan.networks.chunks(40) {
        if let Err(e) = ran(app, &m, &format!("docker network rm {}", chunk.join(" "))).await {
            errors.push(e);
        }
    }
    for chunk in plan.volumes.chunks(40) {
        if let Err(e) = ran(app, &m, &format!("docker volume rm {}", chunk.join(" "))).await {
            errors.push(e);
        }
    }
    let after = reread(app, r, &state).await;
    let gone = |before: usize, still: usize| before.saturating_sub(still);
    let (c, n, v) = if after.ok {
        let still_c = plan.containers.iter().filter(|(id, _)| after.containers.iter().any(|x| x.id == *id)).count();
        let still_n = plan.networks.iter().filter(|name| after.networks.iter().any(|x| &x.name == *name)).count();
        let still_v = plan.volumes.iter().filter(|name| after.volumes.iter().any(|x| &x.name == *name)).count();
        (gone(plan.containers.len(), still_c), gone(plan.networks.len(), still_n), gone(plan.volumes.len(), still_v))
    } else if errors.is_empty() {
        (plan.containers.len(), plan.networks.len(), plan.volumes.len())
    } else {
        (0, 0, 0)
    };
    let tags = plan.tags.iter().cloned().collect::<Vec<_>>().join(", ");
    if c + n + v > 0 {
        let line = format!("контейнеров {c}, сетей {n}, томов {v} (задачи {tags})");
        let mut stats = stats_of(app, &r.id);
        stats.containers += c as u64;
        stats.networks += n as u64;
        stats.volumes += v as u64;
        stats.runs += 1;
        stats.last_ms = now_ms();
        stats.last_line = line.clone();
        if let Ok(json) = serde_json::to_string(&stats) {
            app.db.meta_set(&meta_key(&r.id), &json);
        }
        crate::app::journal_note(app, &format!("runner/{}", r.id), "runner", &format!("убрано автоматически на верфи {}: {line}", r.id)).await;
        app.bus.send(
            "toast",
            json!({"key": "runner.autoToast", "vars": {"id": r.id, "n": c + n + v}, "icon": "done"}).to_string(),
        );
    }
    if !errors.is_empty() {
        let first = errors.first().cloned().unwrap_or_default();
        crate::app::journal_note(
            app,
            &format!("runner/{}", r.id),
            "runner",
            &format!("автоуборка на верфи {} не всё убрала: {}", r.id, first.lines().next().unwrap_or("")),
        )
        .await;
    }
    crate::app::rebuild_runners(app).await;
    crate::app::publish(app).await;
    (c, n, v)
}

/// The poll loop's step: every shipyard whose switch is on.
pub async fn run_all(app: &Shared) {
    let Ok(file) = runners::load(&app.cfg.runners_file()) else { return };
    for r in file.runners.iter().filter(|r| r.auto.enabled) {
        run_one(app, r).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Status, Task};
    use crate::runners::{Container, TaskRef};
    use std::collections::BTreeMap;

    const HOUR: u64 = 3_600_000;

    fn task(id: &str, tag: &str, status: Status, archived: bool, exited_at: u64) -> Task {
        Task {
            id: id.into(),
            repo: id.split('/').next().unwrap_or("").into(),
            n: id.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(0),
            tag: tag.into(),
            key: tag.split('-').next().unwrap_or("").into(),
            status,
            archived,
            finished_ms: exited_at,
            title: tag.into(),
            ..Task::default()
        }
    }

    fn labels(l: &[(&str, &str)]) -> BTreeMap<String, String> {
        l.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    fn cont(id: char, name: &str, l: &[(&str, &str)], class: Class, tag: &str, finished: u64) -> Container {
        Container {
            id: id.to_string().repeat(64),
            name: name.into(),
            labels: labels(l),
            class,
            task: (!tag.is_empty()).then(|| TaskRef { tag: tag.into(), finished_ms: finished, ..TaskRef::default() }),
            ..Container::default()
        }
    }

    fn index(tasks: &[Task]) -> TaskIndex {
        let repo = crate::taskmeta::RepoInfo { name: "alpha".into(), key: "ALP".into(), keys: vec!["ALP".into()], root: String::new() };
        TaskIndex::build(tasks, &[], &[repo])
    }

    #[test]
    fn the_default_is_off_and_six_hours() {
        let a = AutoClean::default();
        assert!(!a.enabled);
        assert_eq!(a.hours, 6);
        let parsed: AutoClean = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, a);
    }

    #[test]
    fn only_a_labelled_forgotten_container_that_waited_long_enough_goes() {
        let now = 100 * HOUR;
        let mut st = RunnerState::default();
        st.id = "vm-r".into();
        st.containers = vec![
            cont('a', "web-ALP-1", &[("herdr.task", "ALP-1")], Class::Forgotten, "ALP-1", now - 7 * HOUR),
            cont('b', "web-ALP-2", &[("herdr.task", "ALP-2")], Class::Forgotten, "ALP-2", now - 2 * HOUR),
            // A name alone (no label) is never enough for the automatic pass.
            cont('c', "web-ALP-3", &[], Class::Forgotten, "ALP-3", now - 9 * HOUR),
            cont('d', "x-ALP-99", &[("herdr.task", "ALP-99")], Class::Unsure, "", 0),
            cont('e', "neighbor", &[], Class::Foreign, "", 0),
            cont('f', "pg-ALP-4", &[("herdr.task", "ALP-4")], Class::Active, "ALP-4", 0),
        ];
        let plan = plan_of(&st, &index(&[]), 6, now);
        assert_eq!(plan.containers.len(), 1);
        assert_eq!(plan.containers[0].1, "web-ALP-1");
        assert_eq!(plan_of(&st, &index(&[]), 1, now).containers.len(), 2);
        assert!(plan_of(&st, &index(&[]), 24, now).is_empty());
    }

    #[test]
    fn units_go_with_the_label_or_the_compose_project_and_never_the_foreign_or_the_builtin() {
        let now = 100 * HOUR;
        let tasks = vec![
            task("alpha/1", "ALP-1", Status::Done, false, now - 7 * HOUR),
            task("alpha/2", "ALP-2", Status::Done, false, now - 2 * HOUR),
            task("alpha/3", "ALP-3", Status::Working, false, 0),
        ];
        let idx = index(&tasks);
        let mut st = RunnerState::default();
        st.id = "vm-r".into();
        st.protected = vec!["neighbor".into()];
        let mut web = cont('a', "web-ALP-1", &[("herdr.task", "ALP-1"), ("com.docker.compose.project", "work-1")], Class::Forgotten, "ALP-1", now - 7 * HOUR);
        web.mounts = "work-1_data:/data".into();
        st.containers = vec![web];
        let net = |id: &str, name: &str, l: &[(&str, &str)]| Net { id: id.repeat(64), name: name.into(), driver: "bridge".into(), labels: labels(l) };
        st.networks = vec![
            net("1", "work-1_default", &[]),
            net("2", "lonely", &[("herdr.task", "ALP-1")]),
            net("3", "young", &[("herdr.task", "ALP-2")]),
            net("4", "busy", &[("herdr.task", "ALP-3")]),
            net("5", "bridge", &[]),
            net("6", "neighbor_default", &[("herdr.task", "ALP-1"), ("com.docker.compose.project", "neighbor")]),
            net("7", "other_default", &[]),
        ];
        let vol = |name: &str, l: &[(&str, &str)]| Vol { name: name.into(), driver: "local".into(), labels: labels(l) };
        st.volumes = vec![
            vol("work-1_data", &[]),
            vol("vol-lonely", &[("herdr.task", "ALP-1")]),
            vol("vol-young", &[("herdr.task", "ALP-2")]),
            vol("vol-busy", &[("herdr.task", "ALP-3")]),
            vol("vol-nameless", &[]),
        ];
        let plan = plan_of(&st, &idx, 6, now);
        assert_eq!(plan.networks, vec!["lonely", "work-1_default"]);
        assert_eq!(plan.volumes, vec!["vol-lonely", "work-1_data"]);
        assert_eq!(plan.containers.len(), 1);
    }

    #[test]
    fn a_label_naming_a_task_not_on_the_board_is_not_removed() {
        let now = 100 * HOUR;
        let mut st = RunnerState::default();
        st.id = "vm-r".into();
        st.volumes = vec![Vol { name: "vol-ghost".into(), driver: "local".into(), labels: labels(&[("herdr.task", "ALP-77")]) }];
        assert!(plan_of(&st, &index(&[]), 6, now).is_empty());
    }

    #[test]
    fn a_task_whose_agent_has_not_left_is_not_finished() {
        let now = 100 * HOUR;
        let idx = index(&[task("alpha/1", "ALP-1", Status::Done, false, 0)]);
        assert_eq!(idx.finished_label("ALP-1"), None);
        let idx = index(&[task("alpha/1", "ALP-1", Status::Done, false, now - 3 * HOUR)]);
        assert_eq!(idx.finished_label("ALP-1"), Some(now - 3 * HOUR));
    }

    #[test]
    fn the_signature_is_the_same_for_the_same_plan() {
        let mut a = AutoPlan::default();
        a.containers = vec![("x".into(), "n".into())];
        a.volumes = vec!["v".into()];
        let mut b = a.clone();
        b.containers = vec![("x".into(), "other name".into())];
        assert_eq!(a.signature(), b.signature());
        b.volumes.push("w".into());
        assert_ne!(a.signature(), b.signature());
    }
}
