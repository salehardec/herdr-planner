//! «Уборка диска» of a shipyard: what the docker daemon of the machine holds that nobody uses — images without
//! containers, build cache, stopped containers of finished tasks, buildx builders and their state, unused volumes.
//!
//! The planner counts first (one fixed read-only ssh script), shows «было → стало» with a tick per kind and per item,
//! and runs only the ticked kinds: `docker image prune`, `docker builder prune`, `docker buildx rm`, and `docker rm` /
//! `docker volume rm` by exact id and name. A running container, a running builder and anything under the runner's
//! protected masks (volumes) are never in the list as removable; the human's ticks are checked again against a fresh
//! count just before the first command.

use crate::config::now_ms;
use crate::runners::{self, parse_bytes, Class, Container, Runner, TaskIndex};
use crate::runners_api::{safe_id, safe_volume};
use crate::ssh::{self, MachineRef};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The count walks every image, container and volume: 12 s on a cold disk.
pub const MEASURE_WAIT: Duration = Duration::from_secs(120);
/// A prune of a few hundred GB takes minutes.
pub const STEP_WAIT: Duration = Duration::from_secs(900);
/// How long a counted card may be run.
pub const PLAN_TTL_MS: u64 = 10 * 60 * 1000;
/// The volumes listed for ticking; the rest are only counted.
const MAX_VOLUMES: usize = 200;

/// Read-only: docker's own totals, the verbose listing (images with their container counts, containers, volumes with
/// their links and sizes) and the buildx builders. A login banner may come before the marker.
pub const MEASURE_SCRIPT: &str = r##"echo herdr-planner-disk 1
echo '== df';     docker system df --format '{{json .}}'
echo '== dfv';    docker system df -v --format '{{json .}}'; echo
echo '== buildx'; docker buildx ls 2>/dev/null"##;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DfRow {
    pub total: u64,
    pub active: u64,
    pub bytes: u64,
    pub reclaimable: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageRow {
    pub name: String,
    pub containers: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VolRow {
    pub name: String,
    pub links: u64,
    pub bytes: u64,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Builder {
    pub name: String,
    pub driver: String,
    /// (node, status) as `docker buildx ls` prints them.
    pub nodes: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Raw {
    pub df: BTreeMap<String, DfRow>,
    pub images: Vec<ImageRow>,
    pub containers: Vec<Container>,
    pub volumes: Vec<VolRow>,
    pub builders: Vec<Builder>,
}

fn s_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn n_of(v: &Value, key: &str) -> u64 {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn sections(out: &str) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cur: Option<String> = None;
    for line in out.lines() {
        if let Some(name) = line.strip_prefix("== ") {
            cur = Some(name.trim().to_string());
            map.entry(name.trim().to_string()).or_default();
        } else if let Some(c) = &cur {
            map.get_mut(c).map(|v| v.push(line.to_string()));
        }
    }
    map
}

/// `docker buildx ls` (the text table of every version): a builder line starts at the left edge, its nodes are
/// indented. `default *` and the `docker` driver are the daemon itself and never a builder to remove.
pub fn parse_builders(lines: &[String]) -> Vec<Builder> {
    let mut list: Vec<Builder> = Vec::new();
    for line in lines {
        if line.trim().is_empty() || line.starts_with("NAME/NODE") {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if line.starts_with(char::is_whitespace) {
            if let (Some(b), Some(node)) = (list.last_mut(), tokens.first()) {
                // `ERROR: …` and the like are not nodes.
                if node.ends_with(':') || node.starts_with("ERROR") {
                    continue;
                }
                let status = tokens.get(2).copied().unwrap_or("").to_string();
                b.nodes.push(((*node).to_string(), status));
            }
        } else if let Some(name) = tokens.first() {
            let rest = &tokens[1..];
            let rest: Vec<&str> = rest.iter().copied().skip_while(|t| *t == "*").collect();
            list.push(Builder {
                name: (*name).to_string(),
                driver: rest.first().copied().unwrap_or("").to_string(),
                nodes: Vec::new(),
            });
        }
    }
    list
}

pub fn parse_raw(out: &str) -> Result<Raw, String> {
    if !out.lines().any(|l| l.contains("herdr-planner-disk 1")) {
        return Err("машина ответила не тем, что ждёт планировщик: нет отметки подсчёта".into());
    }
    let secs = sections(out);
    let mut raw = Raw::default();
    for line in secs.get("df").into_iter().flatten().filter(|l| l.trim().starts_with('{')) {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            raw.df.insert(
                s_of(&v, "Type"),
                DfRow {
                    total: n_of(&v, "TotalCount"),
                    active: n_of(&v, "Active"),
                    bytes: parse_bytes(&s_of(&v, "Size")).unwrap_or(0),
                    reclaimable: parse_bytes(&s_of(&v, "Reclaimable")).unwrap_or(0),
                },
            );
        }
    }
    let text = secs.get("dfv").map(|v| v.join("\n")).unwrap_or_default();
    let v: Value = serde_json::from_str(text.trim())
        .map_err(|_| "docker system df -v не дал разбираемого ответа".to_string())?;
    for i in v.get("Images").and_then(Value::as_array).into_iter().flatten() {
        raw.images.push(ImageRow {
            name: format!("{}:{}", s_of(i, "Repository"), s_of(i, "Tag")),
            containers: n_of(i, "Containers"),
        });
    }
    for c in v.get("Containers").and_then(Value::as_array).into_iter().flatten() {
        raw.containers.push(runners::container_of(c));
    }
    for vol in v.get("Volumes").and_then(Value::as_array).into_iter().flatten() {
        raw.volumes.push(VolRow {
            name: s_of(vol, "Name"),
            links: n_of(vol, "Links"),
            bytes: parse_bytes(&s_of(vol, "Size")).unwrap_or(0),
            labels: runners::labels_of(&s_of(vol, "Labels")),
        });
    }
    raw.builders = parse_builders(secs.get("buildx").map(Vec::as_slice).unwrap_or(&[]));
    Ok(raw)
}

/* ───────── the card ───────── */

/// One removable (or shown-but-locked) thing. `kind` and `locked` are codes the page puts into words.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Item {
    /// What the page sends back: a container id, `b:<builder>`, `s:<volume>` or `v:<volume>`.
    pub key: String,
    pub name: String,
    pub bytes: u64,
    /// `container`, `unsure`, `builder`, `state`, `volume`.
    pub kind: String,
    /// The task tag of a container, the compose project of a volume.
    pub sub: String,
    pub on: bool,
    /// Empty when the item can go; `running` for a builder that is up.
    pub locked: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Images {
    pub unused: u64,
    pub total: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Cache {
    pub total: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Part {
    pub items: Vec<Item>,
    /// Not in the list, and not touched: stopped containers that are not forgotten ones; volumes under a mask or of a
    /// task in work.
    pub kept: u64,
    /// Items counted but cut from the list.
    pub more: u64,
    /// Bytes of the whole kind, listed or not.
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    pub ok: bool,
    pub runner: String,
    pub at_ms: u64,
    pub token: String,
    pub expires_ms: u64,
    pub disk_size: u64,
    pub disk_avail: u64,
    pub images: Images,
    pub cache: Cache,
    pub stopped: Part,
    pub builders: Part,
    pub volumes: Part,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub report: Report,
    pub at: Instant,
}

fn state_volume(node: &str) -> String {
    format!("buildx_buildkit_{node}_state")
}

fn is_state_volume(name: &str) -> bool {
    name.starts_with("buildx_buildkit_") && name.ends_with("_state") && name.len() > "buildx_buildkit__state".len()
}

fn safe_builder(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "default"
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Counts the card from one read. Pure: `now` and the host's disk come in as arguments.
pub fn report_of(
    raw: &Raw,
    runner: &str,
    protected: &[String],
    index: &TaskIndex,
    disk: (u64, u64),
    now: u64,
) -> Report {
    let mut containers = raw.containers.clone();
    runners::classify(&mut containers, protected, index);
    let mut rep = Report {
        ok: true,
        runner: runner.to_string(),
        at_ms: now,
        disk_size: disk.0,
        disk_avail: disk.1,
        ..Report::default()
    };
    let df = |name: &str| raw.df.get(name).cloned().unwrap_or_default();
    // Images with no container, running or not: `docker image prune -a` takes exactly these. The size is docker's own
    // «reclaimable», which counts the layers shared with images that stay once.
    let unused = raw.images.iter().filter(|i| i.containers == 0).count() as u64;
    rep.images = Images { unused, total: raw.images.len() as u64, bytes: if unused > 0 { df("Images").reclaimable } else { 0 } };
    let cache = df("Build Cache");
    rep.cache = Cache { total: cache.total, bytes: cache.reclaimable };

    // Stopped containers: only the forgotten ones are removable; an unsure one is in the list, unticked.
    for c in &containers {
        if matches!(c.state.as_str(), "running" | "paused" | "restarting") {
            continue;
        }
        match c.class {
            Class::Forgotten | Class::Unsure => {
                let unsure = c.class == Class::Unsure;
                if !safe_id(&c.id) {
                    continue;
                }
                rep.stopped.bytes += c.size_bytes;
                rep.stopped.items.push(Item {
                    key: c.id.clone(),
                    name: c.name.clone(),
                    bytes: c.size_bytes,
                    kind: if unsure { "unsure" } else { "container" }.into(),
                    sub: c.task.as_ref().map(|t| t.tag.clone()).unwrap_or_default(),
                    on: !unsure,
                    locked: String::new(),
                });
            }
            Class::Active | Class::Foreign => rep.stopped.kept += 1,
        }
    }

    // Builders: a listed builder that is not running, and the state volumes nobody owns any more.
    let mut owned: BTreeSet<String> = BTreeSet::new();
    for b in raw.builders.iter().filter(|b| b.driver != "docker" && !b.driver.is_empty()) {
        let vols: Vec<String> = b.nodes.iter().map(|(n, _)| state_volume(n)).collect();
        owned.extend(vols.iter().cloned());
        let bytes: u64 = raw.volumes.iter().filter(|v| vols.contains(&v.name)).map(|v| v.bytes).sum();
        let running = b.nodes.iter().any(|(_, s)| s == "running");
        if !safe_builder(&b.name) {
            continue;
        }
        rep.builders.bytes += bytes;
        rep.builders.items.push(Item {
            key: format!("b:{}", b.name),
            name: b.name.clone(),
            bytes,
            kind: "builder".into(),
            sub: b.driver.clone(),
            on: !running,
            locked: if running { "running".into() } else { String::new() },
        });
    }
    for v in &raw.volumes {
        if is_state_volume(&v.name) && v.links == 0 && !owned.contains(&v.name) && safe_volume(&v.name) {
            rep.builders.bytes += v.bytes;
            rep.builders.items.push(Item {
                key: format!("s:{}", v.name),
                name: v.name.clone(),
                bytes: v.bytes,
                kind: "state".into(),
                sub: String::new(),
                on: true,
                locked: String::new(),
            });
        }
    }
    rep.builders.items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));

    // Unused volumes: nobody uses them, but they may be the data of a neighbour that is down. Under a mask or named
    // after a task in work they are only counted.
    let mut tags: BTreeSet<String> = BTreeSet::new();
    let mut projects: BTreeSet<String> = BTreeSet::new();
    for c in containers.iter().filter(|c| c.class == Class::Active) {
        if let Some(t) = &c.task {
            tags.insert(t.tag.to_uppercase());
        }
        if let Some(p) = c.labels.get("com.docker.compose.project") {
            projects.insert(p.clone());
        }
    }
    let mut vols: Vec<&VolRow> = Vec::new();
    for v in raw.volumes.iter().filter(|v| v.links == 0 && !is_state_volume(&v.name)) {
        // In work: named after, or labelled with, a live task or its compose project (a label naming no finished task is
        // kept too: the planner cannot tell whose it is).
        let in_work = runners::volume_matches(&v.name, &tags, &projects)
            || v.labels.get("com.docker.compose.project").is_some_and(|p| projects.contains(p))
            || v.labels.get("herdr.task").is_some_and(|l| index.finished_label(l).is_none());
        if runners::name_protected(&v.name, &v.labels, protected) || in_work || !safe_volume(&v.name) {
            rep.volumes.kept += 1;
        } else {
            vols.push(v);
        }
    }
    vols.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(&b.name)));
    rep.volumes.bytes = vols.iter().map(|v| v.bytes).sum();
    rep.volumes.more = vols.len().saturating_sub(MAX_VOLUMES) as u64;
    rep.volumes.items = vols
        .into_iter()
        .take(MAX_VOLUMES)
        .map(|v| Item {
            key: format!("v:{}", v.name),
            name: v.name.clone(),
            bytes: v.bytes,
            kind: "volume".into(),
            sub: v.labels.get("com.docker.compose.project").cloned().unwrap_or_default(),
            on: false,
            locked: String::new(),
        })
        .collect();
    rep
}

/* ───────── running it ───────── */

fn machine_of(r: &Runner) -> MachineRef {
    MachineRef { label: r.id.clone(), target: r.target.clone(), session: String::new() }
}

pub async fn measure(
    cfg: &crate::config::AppConfig,
    r: &Runner,
    index: &TaskIndex,
    disk: (u64, u64),
) -> Result<Report, String> {
    let m = machine_of(r);
    let end = ssh::ssh_ended(cfg, &m, MEASURE_SCRIPT, MEASURE_WAIT).await?;
    if end.timed_out {
        return Err("верфь не посчитала место за две минуты".into());
    }
    if !end.error.is_empty() || end.code != Some(0) {
        let line = end.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        return Err(if line.is_empty() { "верфь не ответила на подсчёт места".into() } else { line });
    }
    let raw = parse_raw(&end.stdout)?;
    Ok(report_of(&raw, &r.id, &r.protected, index, disk, now_ms()))
}

/// What the human ticked in the card.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub images: bool,
    pub cache: bool,
    pub stopped: Vec<String>,
    pub builders: Vec<String>,
    pub volumes: Vec<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        !self.images && !self.cache && self.stopped.is_empty() && self.builders.is_empty() && self.volumes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cmd {
    /// `stopped`, `builders`, `volumes`, `images`, `cache`.
    pub kind: &'static str,
    pub line: String,
    pub count: usize,
}

/// The commands for a selection, after checking each tick against `fresh` (a count made just now): it must be an item
/// of the fresh card and not locked. The order frees what the next step needs: containers, builders, volumes, then the
/// images those containers held, then the cache.
pub fn commands(fresh: &Report, sel: &Selection) -> Result<Vec<Cmd>, String> {
    let has = |part: &Part, key: &str| part.items.iter().any(|i| i.key == key && i.locked.is_empty());
    for k in &sel.stopped {
        if !has(&fresh.stopped, k) {
            return Err("контейнер больше не в списке уборки: посчитайте место заново".into());
        }
    }
    for k in &sel.builders {
        if !has(&fresh.builders, k) {
            return Err("сборщик больше не в списке уборки или запущен: посчитайте место заново".into());
        }
    }
    for k in &sel.volumes {
        if !has(&fresh.volumes, k) {
            return Err("том больше не в списке уборки: посчитайте место заново".into());
        }
    }
    let mut cmds: Vec<Cmd> = Vec::new();
    if !sel.stopped.is_empty() {
        for chunk in sel.stopped.chunks(50) {
            // No `-f`: a container that has started since is refused, not killed.
            cmds.push(Cmd { kind: "stopped", line: format!("docker rm -v {}", chunk.join(" ")), count: chunk.len() });
        }
    }
    let mut vols: Vec<&str> = Vec::new();
    for k in &sel.builders {
        if let Some(name) = k.strip_prefix("b:") {
            if !safe_builder(name) {
                return Err("недопустимое имя сборщика".into());
            }
            cmds.push(Cmd { kind: "builders", line: format!("docker buildx rm {name}"), count: 1 });
        } else if let Some(name) = k.strip_prefix("s:") {
            vols.push(name);
        } else {
            return Err("недопустимый сборщик".into());
        }
    }
    let mut kind_of_vols: Vec<(&'static str, &str)> = vols.iter().map(|v| ("builders", *v)).collect();
    for k in &sel.volumes {
        match k.strip_prefix("v:") {
            Some(name) => kind_of_vols.push(("volumes", name)),
            None => return Err("недопустимый том".into()),
        }
    }
    for kind in ["builders", "volumes"] {
        let names: Vec<&str> = kind_of_vols.iter().filter(|(k, _)| *k == kind).map(|(_, n)| *n).collect();
        if names.iter().any(|n| !safe_volume(n)) {
            return Err("недопустимое имя тома".into());
        }
        for chunk in names.chunks(40) {
            cmds.push(Cmd { kind, line: format!("docker volume rm {}", chunk.join(" ")), count: chunk.len() });
        }
    }
    if sel.images {
        cmds.push(Cmd { kind: "images", line: "docker image prune -a -f".into(), count: 1 });
    }
    if sel.cache {
        cmds.push(Cmd { kind: "cache", line: "docker builder prune -a -f".into(), count: 1 });
    }
    Ok(cmds)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Step {
    pub kind: String,
    pub ok: bool,
    /// How many things docker said it removed (its output lines), for the id and name commands.
    pub count: usize,
    pub error: String,
    /// `Total reclaimed space: 1.2GB`, when docker says it.
    pub reclaimed: String,
}

pub async fn execute(cfg: &crate::config::AppConfig, r: &Runner, cmds: &[Cmd]) -> Vec<Step> {
    let m = machine_of(r);
    let mut steps = Vec::new();
    for cmd in cmds {
        let mut step = Step { kind: cmd.kind.to_string(), ..Step::default() };
        match ssh::ssh_ended(cfg, &m, &cmd.line, STEP_WAIT).await {
            Ok(e) => {
                step.ok = !e.timed_out && e.error.is_empty() && e.code == Some(0);
                step.reclaimed = e
                    .stdout
                    .lines()
                    .find(|l| l.to_ascii_lowercase().contains("reclaimed space"))
                    .map(|l| l.trim().to_string())
                    .unwrap_or_default();
                step.count = e
                    .stdout
                    .lines()
                    .filter(|l| !l.trim().is_empty() && !l.contains(':') && !l.starts_with("Deleted"))
                    .count();
                if !step.ok {
                    step.error = if e.timed_out {
                        "не дождались ответа".into()
                    } else if e.stderr.trim().is_empty() {
                        e.stdout.trim().to_string()
                    } else {
                        e.stderr.trim().to_string()
                    };
                }
            }
            Err(e) => step.error = e,
        }
        steps.push(step);
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Status, Task};

    const BUILDX: &str = "NAME/NODE DRIVER/ENDPOINT STATUS  BUILDKIT PLATFORMS\n\
        default * docker                           \n\
        \x20 default default         running v0.23.1  linux/amd64, linux/386\n\
        old-ci    docker-container                 \n\
        \x20 old-ci0  unix:///var/run/docker.sock inactive\n\
        live      docker-container                 \n\
        \x20 live0   unix:///var/run/docker.sock running v0.12.0 linux/amd64\n";

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    fn out() -> String {
        let id = |c: char| c.to_string().repeat(64);
        let dfv = serde_json::json!({
            "Images": [
                {"Repository": "postgres", "Tag": "16", "Containers": "0", "Size": "294MB"},
                {"Repository": "nginx", "Tag": "1", "Containers": "2", "Size": "190MB"},
            ],
            "Containers": [
                {"ID": id('b'), "Names": "web-ALP-1", "Image": "nginx:1", "State": "exited", "Status": "Exited (0) 2 hours ago", "Labels": "herdr.task=ALP-1", "Size": "12MB"},
                {"ID": id('c'), "Names": "x-ALP-99", "Image": "alpine", "State": "exited", "Status": "Exited", "Labels": "herdr.task=ALP-99", "Size": "4MB"},
                {"ID": id('d'), "Names": "neighbor-db-1", "Image": "postgres:15", "State": "exited", "Status": "Exited", "Labels": "com.docker.compose.project=neighbor", "Size": "1.2GB"},
                {"ID": id('e'), "Names": "pg-ALP-3", "Image": "postgres:16", "State": "running", "Status": "Up", "Labels": "herdr.task=ALP-3", "Size": "28MB"},
                {"ID": id('f'), "Names": "pg-ALP-4", "Image": "postgres:16", "State": "exited", "Status": "Exited", "Labels": "herdr.task=ALP-4,com.docker.compose.project=work-4", "Size": "2MB"},
            ],
            "Volumes": [
                {"Name": "buildx_buildkit_old-ci0_state", "Links": "0", "Size": "2.5GB", "Labels": ""},
                {"Name": "buildx_buildkit_gone0_state", "Links": "0", "Size": "29GB", "Labels": ""},
                {"Name": "buildx_buildkit_live0_state", "Links": "1", "Size": "1GB", "Labels": ""},
                {"Name": "chromadb_data", "Links": "0", "Size": "3GB", "Labels": "com.docker.compose.project=chromadb"},
                {"Name": "supabase_db", "Links": "0", "Size": "500MB", "Labels": "com.docker.compose.project=supabase"},
                {"Name": "work-4_data", "Links": "0", "Size": "10MB", "Labels": "com.docker.compose.project=work-4"},
                {"Name": "inuse", "Links": "2", "Size": "1GB", "Labels": ""},
            ],
        });
        format!(
            "Welcome\nherdr-planner-disk 1\n== df\n\
             {{\"Type\":\"Images\",\"TotalCount\":\"19\",\"Active\":\"14\",\"Size\":\"16.28GB\",\"Reclaimable\":\"10.24GB (62%)\"}}\n\
             {{\"Type\":\"Build Cache\",\"TotalCount\":\"7\",\"Active\":\"0\",\"Size\":\"4GB\",\"Reclaimable\":\"3.5GB\"}}\n\
             == dfv\n{dfv}\n== buildx\n{BUILDX}"
        )
    }

    fn index() -> TaskIndex {
        let t = |id: &str, tag: &str, status: Status, exited: u64| Task {
            id: id.into(),
            repo: "alpha".into(),
            n: id.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(0),
            tag: tag.into(),
            key: "ALP".into(),
            status,
            exited_at: exited,
            title: tag.into(),
            ..Task::default()
        };
        let repo = crate::taskmeta::RepoInfo { name: "alpha".into(), key: "ALP".into(), keys: vec!["ALP".into()], root: String::new() };
        TaskIndex::build(
            &[t("alpha/1", "ALP-1", Status::Done, 5), t("alpha/3", "ALP-3", Status::Working, 0), t("alpha/4", "ALP-4", Status::Working, 0)],
            &[],
            &[repo],
        )
    }

    fn report() -> Report {
        let raw = parse_raw(&out()).unwrap();
        report_of(&raw, "vm-r", &["supabase".into()], &index(), (1000, 400), 42)
    }

    #[test]
    fn the_builders_table_is_read_for_every_buildx_version() {
        let b = parse_builders(&lines(BUILDX));
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].name, "default");
        assert_eq!(b[0].driver, "docker");
        assert_eq!(b[1].name, "old-ci");
        assert_eq!(b[1].driver, "docker-container");
        assert_eq!(b[1].nodes, vec![("old-ci0".to_string(), "inactive".to_string())]);
        assert_eq!(b[2].nodes[0].1, "running");
    }

    #[test]
    fn a_reply_without_the_marker_is_refused() {
        assert!(parse_raw("nothing").is_err());
        assert!(parse_raw("herdr-planner-disk 1\n== dfv\nnot json").is_err());
    }

    #[test]
    fn the_card_counts_each_kind() {
        let r = report();
        assert_eq!((r.images.unused, r.images.total), (1, 2));
        assert_eq!(r.images.bytes, 10_240_000_000);
        assert_eq!((r.cache.total, r.cache.bytes), (7, 3_500_000_000));
        // Stopped: web-ALP-1 forgotten (ticked), x-ALP-99 unsure (unticked); neighbor and the task in work are kept.
        let keys: Vec<(&str, bool)> = r.stopped.items.iter().map(|i| (i.name.as_str(), i.on)).collect();
        assert_eq!(keys, vec![("web-ALP-1", true), ("x-ALP-99", false)]);
        assert_eq!(r.stopped.kept, 2);
        assert_eq!(r.disk_avail, 400);
    }

    #[test]
    fn a_builder_that_runs_is_locked_and_an_orphan_state_is_offered() {
        let r = report();
        let by = |k: &str| r.builders.items.iter().find(|i| i.key == k).unwrap();
        assert_eq!(by("b:old-ci").bytes, 2_500_000_000);
        assert!(by("b:old-ci").on && by("b:old-ci").locked.is_empty());
        assert_eq!(by("b:live").locked, "running");
        assert!(!by("b:live").on);
        assert!(r.builders.items.iter().all(|i| i.key != "b:default"));
        assert_eq!(by("s:buildx_buildkit_gone0_state").kind, "state");
        // A state volume of a builder in use (links) or of a listed builder is not a second item.
        assert!(r.builders.items.iter().all(|i| i.key != "s:buildx_buildkit_live0_state" && i.key != "s:buildx_buildkit_old-ci0_state"));
        assert_eq!(r.builders.items[0].key, "s:buildx_buildkit_gone0_state");
    }

    #[test]
    fn volumes_are_unticked_and_a_mask_or_a_task_in_work_keeps_them() {
        let r = report();
        let names: Vec<&str> = r.volumes.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["chromadb_data"]);
        assert!(r.volumes.items.iter().all(|i| !i.on));
        // supabase (mask) and work-4 (the project of ALP-4, in work) are only counted.
        assert_eq!(r.volumes.kept, 2);
        assert_eq!(r.volumes.bytes, 3_000_000_000);
    }

    #[test]
    fn only_the_ticked_kinds_become_commands() {
        let r = report();
        let sel = Selection {
            images: true,
            cache: false,
            stopped: vec!["b".repeat(64)],
            builders: vec!["b:old-ci".into(), "s:buildx_buildkit_gone0_state".into()],
            volumes: vec![],
        };
        let lines: Vec<String> = commands(&r, &sel).unwrap().into_iter().map(|c| c.line).collect();
        assert_eq!(
            lines,
            vec![
                format!("docker rm -v {}", "b".repeat(64)),
                "docker buildx rm old-ci".to_string(),
                "docker volume rm buildx_buildkit_gone0_state".to_string(),
                "docker image prune -a -f".to_string(),
            ]
        );
        assert!(commands(&r, &Selection::default()).unwrap().is_empty());
    }

    #[test]
    fn a_tick_that_is_not_in_the_fresh_card_or_is_locked_refuses_everything() {
        let r = report();
        let bad = |sel: Selection| commands(&r, &sel).is_err();
        // A running builder, a foreign stopped container, a masked volume, an invented name.
        assert!(bad(Selection { builders: vec!["b:live".into()], ..Selection::default() }));
        assert!(bad(Selection { stopped: vec!["d".repeat(64)], ..Selection::default() }));
        assert!(bad(Selection { volumes: vec!["v:supabase_db".into()], ..Selection::default() }));
        assert!(bad(Selection { volumes: vec!["v:chromadb_data; rm -rf /".into()], ..Selection::default() }));
        assert!(bad(Selection { builders: vec!["b:default".into()], ..Selection::default() }));
        // A tick that is fine does not pass with a bad one.
        assert!(bad(Selection { volumes: vec!["v:chromadb_data".into(), "v:inuse".into()], ..Selection::default() }));
        assert!(!bad(Selection { volumes: vec!["v:chromadb_data".into()], ..Selection::default() }));
    }

    #[test]
    fn long_lists_are_cut_into_commands_of_a_few_names() {
        let mut r = report();
        r.volumes.items = (0..95)
            .map(|i| Item { key: format!("v:vol{i}"), name: format!("vol{i}"), kind: "volume".into(), ..Item::default() })
            .collect();
        let sel = Selection { volumes: r.volumes.items.iter().map(|i| i.key.clone()).collect(), ..Selection::default() };
        let cmds = commands(&r, &sel).unwrap();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds.iter().map(|c| c.count).sum::<usize>(), 95);
    }
}
