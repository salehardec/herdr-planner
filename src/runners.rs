//! Shipyards («верфи»): machines that run the docker containers of the board's tasks (PLN-26).
//!
//! A runner is not a galaxy: it has no board, no herdr and no agents. The agents of this board reach it with
//! `DOCKER_HOST=ssh://…`; the planner reads its state (one fixed ssh script), matches its containers to the tasks of
//! this board and removes the forgotten ones by exact id, after a «было → стало» card.
//!
//! The source of truth is `~/.herdr-projects/settings/runners.json`; the planner keeps the generated docker rule in
//! this machine's `worker.instructions` and `reviewer.instructions`. No copy is written to another machine.

use crate::app::Shared;
use crate::board::Task;
use crate::config::{now_ms, AppConfig};
use crate::runners_auto::{AutoClean, AutoStats};
use crate::ssh::{self, CheckHint, MachineRef, Why};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

pub const FILE_VERSION: u32 = 1;
pub const PROBE_WAIT: Duration = Duration::from_secs(25);
/// How long a «было → стало» plan lives on the server.
pub const PLAN_TTL_MS: u64 = 10 * 60 * 1000;

/* ───────── the file ───────── */

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capacity {
    #[serde(default)]
    pub cpus: u64,
    #[serde(default)]
    pub mem_bytes: u64,
    #[serde(default)]
    pub disk_bytes: u64,
    #[serde(default)]
    pub gpu: Vec<Gpu>,
}

/// What a task's container may take on a yard (PLN-43): `--cpus` and `--memory` per container, and the most containers
/// one task may keep at once. Zero means «no limit of this kind»: the agents' rule leaves it out, and the Focus does not
/// mark containers that have none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Limits {
    #[serde(default)]
    pub cpus: u64,
    /// Memory per container, MiB, as `--memory` takes it (whole GiB when it divides).
    #[serde(default)]
    pub mem_mb: u64,
    /// The most containers one task may keep on the yard at once; 0 — no maximum.
    #[serde(default)]
    pub per_task: u64,
}

impl Limits {
    /// The share of a machine one container may take: a quarter of its cores (1 … 8) and an eighth of its memory
    /// (rounded to half a GiB, 512 MiB … 8 GiB). No maximum of containers per task until the human sets one.
    pub fn for_host(host: &HostState) -> Limits {
        if host.cpus == 0 && host.mem_total == 0 {
            return Limits::fallback();
        }
        let cpus = if host.cpus == 0 { 2 } else { (host.cpus / 4).clamp(1, 8) };
        let mem_mb = if host.mem_total == 0 {
            2048
        } else {
            let eighth = host.mem_total / (1024 * 1024) / 8;
            (((eighth + 256) / 512) * 512).clamp(512, 8192)
        };
        Limits { cpus, mem_mb, per_task: 0 }
    }

    /// Until the machine has been read: enough for a database and the application beside it.
    pub fn fallback() -> Limits {
        Limits { cpus: 2, mem_mb: 2048, per_task: 0 }
    }

    /// Whether the rule asks for any limit at all.
    pub fn any(&self) -> bool {
        self.cpus > 0 || self.mem_mb > 0
    }

    /// The CPU share as `docker run --cpus` has it: `4`.
    pub fn cpu_text(&self) -> String {
        self.cpus.to_string()
    }

    /// The memory as `docker run --memory` has it: `4g`, `1536m`.
    pub fn mem_text(&self) -> String {
        if self.mem_mb % 1024 == 0 {
            format!("{}g", self.mem_mb / 1024)
        } else {
            format!("{}m", self.mem_mb)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Gpu {
    #[serde(default)]
    pub index: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mem_mib: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runner {
    pub id: String,
    pub target: String,
    #[serde(default)]
    pub ports_host: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub protected: Vec<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub added_at_ms: u64,
    #[serde(default)]
    pub capacity: Option<Capacity>,
    /// Clearing the containers, networks and volumes of finished tasks by itself.
    #[serde(default)]
    pub auto: AutoClean,
    /// When a yard is worth an alarm: little disk, a long silence (PLN-42).
    #[serde(default)]
    pub alerts: crate::runners_alert::AlertRules,
    /// The defaults of a task's containers (PLN-43): absent until the machine is first read, then its share.
    #[serde(default)]
    pub limits: Option<Limits>,
}

impl Runner {
    /// The limits the rule talks about: the list file's own, else the machine's share once it has been read, else the
    /// fallback; a human who set zeros said «no limits» and gets them.
    pub fn effective_limits(&self, host: Option<&HostState>) -> Limits {
        match self.limits {
            Some(l) => l,
            None => host.map(Limits::for_host).unwrap_or_else(Limits::fallback),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunnersFile {
    #[serde(default = "file_version")]
    pub version: u32,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub runners: Vec<Runner>,
    #[serde(default)]
    pub assign: BTreeMap<String, String>,
}

fn file_version() -> u32 {
    FILE_VERSION
}

impl Default for RunnersFile {
    fn default() -> Self {
        Self {
            version: FILE_VERSION,
            updated_at_ms: 0,
            default: String::new(),
            runners: Vec::new(),
            assign: BTreeMap::new(),
        }
    }
}

impl RunnersFile {
    pub fn get(&self, id: &str) -> Option<&Runner> {
        self.runners.iter().find(|r| r.id == id)
    }

    /// The runner agents use: the named default, else the first one.
    pub fn default_runner(&self) -> Option<&Runner> {
        self.get(&self.default).or_else(|| self.runners.first())
    }

    /// Every protected mask of every runner, in a stable order.
    pub fn protected_all(&self) -> Vec<String> {
        let mut set: BTreeSet<String> = BTreeSet::new();
        for r in &self.runners {
            for m in &r.protected {
                set.insert(m.clone());
            }
        }
        set.into_iter().collect()
    }
}

/// A mask is a name with `*` (any run of characters): letters, digits, `_`, `.`, `-` and `*`.
pub fn valid_mask(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '*'))
}

pub fn validate(file: &RunnersFile) -> Result<(), String> {
    if file.version != FILE_VERSION {
        return Err(format!(
            "runners.json: версия {} — планировщик понимает только {}",
            file.version, FILE_VERSION
        ));
    }
    let mut ids = BTreeSet::new();
    for r in &file.runners {
        if !ssh::valid_label(&r.id) {
            return Err(format!("runners.json: недопустимая метка верфи: {:?}", r.id));
        }
        if !ids.insert(r.id.clone()) {
            return Err(format!("runners.json: метка верфи повторяется: {}", r.id));
        }
        if !ssh::valid_target(&r.target) {
            return Err(format!("runners.json: недопустимый адрес верфи {}: {:?}", r.id, r.target));
        }
        if !r.ports_host.is_empty() && !ssh::valid_target(&r.ports_host) {
            return Err(format!("runners.json: недопустимый адрес портов верфи {}: {:?}", r.id, r.ports_host));
        }
        for m in &r.protected {
            if !valid_mask(m) {
                return Err(format!("runners.json: маска защиты {:?} у верфи {}", m, r.id));
            }
        }
        if let Err(e) = r.alerts.validate() {
            return Err(format!("runners.json: {e} (верфь {})", r.id));
        }
        if let Some(l) = &r.limits {
            if l.cpus > 1024 {
                return Err(format!("runners.json: ЦП на контейнер у верфи {} — не больше 1024", r.id));
            }
            if l.mem_mb > 1024 * 1024 {
                return Err(format!("runners.json: память на контейнер у верфи {} — не больше 1 ТиБ", r.id));
            }
            if l.per_task > 1024 {
                return Err(format!("runners.json: контейнеров на задачу у верфи {} — не больше 1024", r.id));
            }
        }
        for t in &r.tags {
            if t.len() > 32
                || !t
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            {
                return Err(format!("runners.json: метка {:?} у верфи {}", t, r.id));
            }
        }
    }
    if !file.default.is_empty() && !ids.contains(&file.default) {
        return Err(format!(
            "runners.json: верфь по умолчанию {:?} не найдена в списке",
            file.default
        ));
    }
    for (repo, id) in &file.assign {
        if repo.is_empty() || id.is_empty() || !ids.contains(id) {
            return Err(format!("runners.json: назначение {repo:?} → {id:?} неверно"));
        }
    }
    Ok(())
}

/// The file at `path`; a missing one is an empty list.
pub fn load(path: &Path) -> Result<RunnersFile, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RunnersFile::default()),
        Err(e) => return Err(format!("не прочитать {}: {e}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(RunnersFile::default());
    }
    let file: RunnersFile =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    validate(&file)?;
    Ok(file)
}

/// Writes the file atomically (a temporary file and a rename), mode 600.
pub fn save(path: &Path, file: &RunnersFile) -> Result<(), String> {
    validate(file)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, text).map_err(|e| format!("не записать {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("не записать {}: {e}", path.display()))
}

/* ───────── the probe ───────── */

/// One ssh call, fixed text: host facts, docker's containers, their usage, networks, volumes and the GPU.
/// Nothing is changed; `nvidia-smi` is optional; the first line is the marker (a login banner may come before it).
pub const PROBE_SCRIPT: &str = r##"echo herdr-planner-runner 1
printf 'now\t%s\n' "$(date +%s)"
printf 'load\t%s\n' "$(cat /proc/loadavg)"
printf 'cpus\t%s\n' "$(nproc)"
printf 'arch\t%s\n' "$(uname -m)"
printf 'host\t%s\n' "$(hostname 2>/dev/null || uname -n)"
printf 'os\t%s\n' "$(. /etc/os-release 2>/dev/null && printf '%s' "$PRETTY_NAME")"
grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo | sed 's/^/mem\t/'
root=$(docker info --format '{{.DockerRootDir}}' 2>/dev/null || echo /var/lib/docker)
printf 'disk\t%s\t%s\n' "$root" "$(df -B1 --output=size,avail "$root" | tail -1)"
printf 'docker\t%s\n' "$(docker version --format '{{.Server.Version}} {{.Server.Arch}}' 2>/dev/null)"
printf 'compose\t%s\n' "$(docker compose version --short 2>/dev/null)"
printf 'info\t%s\n' "$(docker info --format '{{.NCPU}}|{{.MemTotal}}|{{.Containers}}|{{.ContainersRunning}}|{{.Images}}|{{.OperatingSystem}}|{{range $k,$v := .Runtimes}}{{$k}},{{end}}' 2>/dev/null)"
if command -v nvidia-smi >/dev/null; then
  nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits | sed 's/^/gpu\t/'
  for l in $(nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits | tr -d ' '); do
    printf 'gpuproc\t%s\t%s\n' "$l" "$(grep -o 'docker-[0-9a-f]*' /proc/${l%%,*}/cgroup 2>/dev/null | head -1)"
  done
fi
echo '== ps';    docker ps -a --no-trunc --format '{{json .}}'
echo '== stats'; docker stats --no-stream --format '{{json .}}'
# The limits the containers really have (PLN-43): `docker ps` and `docker stats` cannot tell a limit from the whole
# machine. One inspect for all of them, one line each; a container that vanished in between simply has no line.
echo '== lim'
ids=$(docker ps -aq 2>/dev/null)
[ -z "$ids" ] || docker inspect --format '{{.Id}}|{{.HostConfig.NanoCpus}}|{{.HostConfig.Memory}}|{{.HostConfig.CpuQuota}}|{{.HostConfig.CpuPeriod}}' $ids 2>/dev/null || true
echo '== net';   docker network ls --format '{{json .}}'
echo '== vol';   docker volume ls --format '{{json .}}'
# The build caches of the board's repositories (scripts/yard.sh labels them herdr.shared=<repo>): images first by their
# label, then the few labels that `image ls` cannot print. The marker comes before the JSON: parse_probe files the lines
# under the section they follow, and an image read under '== vol' would be a nameless volume.
echo '== img'
img_ids=$(docker image ls -q --filter label=herdr.shared 2>/dev/null)
[ -z "$img_ids" ] || docker image inspect --format '{{json .}}' $img_ids 2>/dev/null || true"##;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuState {
    pub index: String,
    pub name: String,
    pub util_pct: Option<u64>,
    pub mem_used_mib: Option<u64>,
    pub mem_total_mib: Option<u64>,
    pub temp_c: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostState {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub cpus: u64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub mem_total: u64,
    pub mem_avail: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub disk_root: String,
    pub disk_size: u64,
    pub disk_avail: u64,
    pub docker_version: String,
    pub docker_arch: String,
    pub compose_version: String,
    pub runtimes: Vec<String>,
    pub docker_cpus: u64,
    pub docker_mem: u64,
    pub docker_containers: u64,
    pub docker_running: u64,
    pub docker_images: u64,
    pub gpu: Vec<GpuState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    /// A container of a task that is still on the board.
    Active,
    /// A container of a task that is done or archived.
    Forgotten,
    /// A tag whose task is not found (removed from the board), or one tag in two tasks.
    Unsure,
    /// Not a board task, or under a protected mask: never touched.
    Foreign,
    /// A build cache of one of the board's repositories (`herdr.shared=<repo>`, scripts/yard.sh): never a task's, never
    /// forgotten, never touched from the page.
    Shared,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskRef {
    pub id: String,
    pub tag: String,
    pub status: String,
    pub title: String,
    /// When the task's agent left (ms); 0 while it has not.
    #[serde(default)]
    pub finished_ms: u64,
}

/// A service whose client command the page knows for a published port (PLN-44).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Postgres,
    Redis,
}

/// One published port of a container, out of docker's `0.0.0.0:15432->5432/tcp`: the page links a web port and
/// prints a client command for a known service. A container port with no host side (`5432/tcp`) is not published.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishedPort {
    /// The host address docker bound to (`0.0.0.0`, `::`, `127.0.0.1`); empty when docker printed none.
    pub host_ip: String,
    /// The port on the yard; `None` for a range (`8000-8010`) — no link then.
    pub host_port: Option<u16>,
    /// The port inside the container; `None` for a range.
    pub container_port: Option<u16>,
    #[serde(default)]
    pub proto: String,
    /// A known service of this port (`postgres`, `redis`): the page offers its client command instead of a link.
    #[serde(default)]
    pub service: Option<Service>,
    /// A web port of an all-interface binding: the page makes a link of it.
    #[serde(default)]
    pub http: bool,
    /// The container port is 443: the link is `https`.
    #[serde(default)]
    pub https: bool,
    /// Bound to the yard's own loopback: no link and no command from this machine, the page says why.
    #[serde(default)]
    pub loopback: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub created_ms: u64,
    pub labels: BTreeMap<String, String>,
    pub ports: String,
    /// The published ports parsed out of `ports` (PLN-44); empty for an older saved read.
    #[serde(default)]
    pub published: Vec<PublishedPort>,
    pub networks: String,
    pub mounts: String,
    pub size: String,
    pub size_bytes: u64,
    pub cpu_pct: Option<f64>,
    pub mem_bytes: Option<u64>,
    /// The memory limit the container really has (`HostConfig.Memory`); `None` — no limit at all.
    pub mem_limit: Option<u64>,
    /// The CPU limit in cores (`HostConfig.NanoCpus`, else the quota over the period); `None` — no limit.
    #[serde(default)]
    pub cpu_limit: Option<f64>,
    /// A container of a live task that has no limit the yard's rule asks for (PLN-43): a mark in the Focus.
    #[serde(default)]
    pub no_limits: bool,
    pub pids: Option<u64>,
    pub net_io: String,
    pub block_io: String,
    pub gpu_mem_mib: Option<u64>,
    pub task: Option<TaskRef>,
    pub class: Class,
}

impl Default for Class {
    fn default() -> Self {
        Class::Foreign
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Net {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Vol {
    pub name: String,
    pub driver: String,
    pub labels: BTreeMap<String, String>,
}

/// An image of the board's build caches (`herdr.shared=<repo>`), as the probe reads it: never shown as a container of a
/// task and never offered for removal; the page lists it with the cache volume under «Кэш сборок».
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub id: String,
    /// The tags docker itself prints (`first:last` if there are none).
    pub names: Vec<String>,
    pub size: Option<u64>,
    pub labels: BTreeMap<String, String>,
}

/// One thing of a board repository's build cache: a container, an image or a volume labelled `herdr.shared=<repo>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SharedItem {
    /// `container`, `image` or `volume`.
    pub kind: String,
    pub name: String,
    /// The repository of this board the cache belongs to (the label's value).
    pub repo: String,
    /// The image's size as docker reports it; a volume's size is only known from `docker system df -v` and is `None`
    /// here (the page shows «—» until the disk count is asked for).
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// A live task that keeps more containers on the yard than the rule allows (PLN-43).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverCount {
    /// The task on the board.
    pub task: String,
    pub tag: String,
    /// Containers of the task found on the yard.
    pub containers: u64,
    /// The maximum the rule sets.
    pub max: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BlockState {
    pub connected: bool,
    pub stale: bool,
    pub at: u64,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunnerState {
    pub id: String,
    pub target: String,
    pub ports_host: String,
    pub tags: Vec<String>,
    pub protected: Vec<String>,
    pub note: String,
    pub ok: bool,
    pub why: Option<Why>,
    pub fetched_at_ms: u64,
    pub host: HostState,
    pub containers: Vec<Container>,
    pub networks: Vec<Net>,
    pub volumes: Vec<Vol>,
    /// The images of the board's build caches, as read; kept for the shared list and not sent to the page.
    #[serde(default, skip_serializing)]
    pub images: Vec<ImageInfo>,
    /// The build caches of the board's repositories: never a task's, never foreign, never forgotten.
    #[serde(default)]
    pub shared: Vec<SharedItem>,
    /// The auto-clean switch of the list file and what it has done (filled when the snapshot is built).
    #[serde(default)]
    pub auto: AutoClean,
    #[serde(default)]
    pub auto_stats: AutoStats,
    /// When a yard is worth an alarm, as the list file has it now.
    #[serde(default)]
    pub alert_rules: crate::runners_alert::AlertRules,
    /// The alarms of this yard, from the last read (PLN-42).
    #[serde(default)]
    pub alerts: Vec<crate::runners_alert::Alert>,
    /// The limits of a task's containers on this yard, as the list file has them now (PLN-43).
    #[serde(default)]
    pub limits: Limits,
    /// Live tasks that keep more containers than the yard allows (PLN-43).
    #[serde(default)]
    pub over: Vec<OverCount>,
    /// When the yard stopped answering (ms); 0 while it answers.
    #[serde(default)]
    pub down_since_ms: u64,
}

impl RunnerState {
    pub fn from_runner(r: &Runner) -> Self {
        Self {
            auto: r.auto.clone(),
            alert_rules: r.alerts.clone(),
            id: r.id.clone(),
            target: r.target.clone(),
            ports_host: if r.ports_host.is_empty() {
                host_of_target(&r.target)
            } else {
                r.ports_host.clone()
            },
            tags: r.tags.clone(),
            protected: r.protected.clone(),
            note: r.note.clone(),
            limits: r.effective_limits(None),
            ..Self::default()
        }
    }
}

pub fn parse_probe(
    out: &str,
) -> Result<(HostState, Vec<Container>, Vec<Net>, Vec<Vol>, Vec<ImageInfo>), String> {
    if !out.lines().any(|l| l.contains("herdr-planner-runner 1")) {
        return Err("машина ответила не тем, что ждёт планировщик: нет отметки опроса".into());
    }
    let mut host = HostState::default();
    let mut containers: Vec<Container> = Vec::new();
    let mut stats: Vec<Value> = Vec::new();
    let mut nets: Vec<Net> = Vec::new();
    let mut vols: Vec<Vol> = Vec::new();
    let mut images: Vec<ImageInfo> = Vec::new();
    // One `== lim` line: the container, its CPU share in nano-cores, memory in bytes, quota and period in µs.
    let mut lims: Vec<(String, u64, u64, i64, u64)> = Vec::new();
    let mut section = "";
    let mut gpu_procs: Vec<(String, u64)> = Vec::new();
    let mut mem_kb: HashMap<String, u64> = HashMap::new();
    let mut now_s: u64 = 0;
    for raw in out.lines() {
        let line = raw.trim_end_matches('\r');
        if line.starts_with("==") {
            section = line.trim_start_matches('=').trim();
            continue;
        }
        if !section.is_empty() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            // The limits are `|`-separated, not JSON (one inspect line per container).
            if section == "lim" {
                let p: Vec<&str> = t.split('|').collect();
                if p.len() >= 5 && !p[0].trim().is_empty() {
                    lims.push((
                        p[0].trim().to_string(),
                        p[1].trim().parse().unwrap_or(0),
                        p[2].trim().parse().unwrap_or(0),
                        p[3].trim().parse().unwrap_or(0),
                        p[4].trim().parse().unwrap_or(0),
                    ));
                }
                continue;
            }
            let v: Value = match serde_json::from_str(t) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match section {
                "ps" => containers.push(container_of(&v)),
                "stats" => stats.push(v),
                "net" => nets.push(Net {
                    id: s_of(&v, "ID"),
                    name: s_of(&v, "Name"),
                    driver: s_of(&v, "Driver"),
                    labels: labels_of(&s_of(&v, "Labels")),
                }),
                "vol" => vols.push(Vol {
                    name: s_of(&v, "Name"),
                    driver: s_of(&v, "Driver"),
                    labels: labels_of(&s_of(&v, "Labels")),
                }),
                "img" => {
                    let labels = v
                        .get("Config")
                        .and_then(|c| c.get("Labels"))
                        .and_then(Value::as_object)
                        .map(|m| {
                            m.iter()
                                .filter_map(|(k, x)| x.as_str().map(|val| (k.clone(), val.to_string())))
                                .collect()
                        })
                        .unwrap_or_default();
                    let names = v
                        .get("RepoTags")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    images.push(ImageInfo {
                        id: s_of(&v, "Id"),
                        names,
                        size: v.get("Size").and_then(Value::as_u64),
                        labels,
                    });
                }
                _ => {}
            }
            continue;
        }
        let Some((key, value)) = line.split_once('\t') else {
            continue;
        };
        match key {
            "now" => now_s = value.trim().parse().unwrap_or(0),
            "load" => {
                let p: Vec<&str> = value.split_whitespace().collect();
                host.load1 = p.first().and_then(|x| x.parse().ok()).unwrap_or(0.0);
                host.load5 = p.get(1).and_then(|x| x.parse().ok()).unwrap_or(0.0);
                host.load15 = p.get(2).and_then(|x| x.parse().ok()).unwrap_or(0.0);
            }
            "cpus" => host.cpus = value.trim().parse().unwrap_or(0),
            "arch" => host.arch = value.trim().to_string(),
            "host" => host.hostname = value.trim().to_string(),
            "os" => host.os = value.trim().to_string(),
            "mem" => {
                if let Some((name, kb)) = value.split_once(':') {
                    if let Ok(kb) = kb.trim().trim_end_matches(" kB").trim().parse::<u64>() {
                        mem_kb.insert(name.trim().to_string(), kb * 1024);
                    }
                }
            }
            "disk" => {
                let p: Vec<&str> = value.split('\t').collect();
                host.disk_root = p.first().map(|s| s.trim().to_string()).unwrap_or_default();
                if let Some(rest) = p.get(1) {
                    let n: Vec<&str> = rest.split_whitespace().collect();
                    host.disk_size = n.first().and_then(|x| x.parse().ok()).unwrap_or(0);
                    host.disk_avail = n.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
                }
            }
            "docker" => {
                let mut p = value.split_whitespace();
                host.docker_version = p.next().unwrap_or("").to_string();
                host.docker_arch = p.next().unwrap_or("").to_string();
            }
            "compose" => host.compose_version = value.trim().to_string(),
            "info" => {
                let p: Vec<&str> = value.split('|').collect();
                host.docker_cpus = p.first().and_then(|x| x.trim().parse().ok()).unwrap_or(0);
                host.docker_mem = p.get(1).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
                host.docker_containers = p.get(2).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
                host.docker_running = p.get(3).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
                host.docker_images = p.get(4).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
                if host.os.is_empty() {
                    host.os = p.get(5).map(|s| s.trim().to_string()).unwrap_or_default();
                }
                host.runtimes = p
                    .get(6)
                    .map(|s| {
                        s.split(',')
                            .map(str::trim)
                            .filter(|x| !x.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
            }
            "gpu" => {
                let p: Vec<&str> = value.split(',').map(str::trim).collect();
                host.gpu.push(GpuState {
                    index: p.first().copied().unwrap_or("").to_string(),
                    name: p.get(1).copied().unwrap_or("").to_string(),
                    util_pct: p.get(2).and_then(|x| x.parse().ok()),
                    mem_used_mib: p.get(3).and_then(|x| x.parse().ok()),
                    mem_total_mib: p.get(4).and_then(|x| x.parse().ok()),
                    temp_c: p.get(5).and_then(|x| x.parse().ok()),
                });
            }
            "gpuproc" => {
                let p: Vec<&str> = value.split('\t').collect();
                // `pid,used` then the container's cgroup: the pid is not needed, the memory is.
                let parts: Vec<&str> = p.first().copied().unwrap_or("").split(',').collect();
                let used = parts.get(1).and_then(|x| x.trim().parse().ok());
                if let (Some(used), Some(cg)) = (used, p.get(1)) {
                    if let Some(hex) = cg.strip_prefix("docker-") {
                        gpu_procs.push((hex.to_string(), used));
                    }
                }
            }
            _ => {}
        }
    }
    host.mem_total = mem_kb.get("MemTotal").copied().unwrap_or(host.docker_mem);
    host.mem_avail = mem_kb.get("MemAvailable").copied().unwrap_or(0);
    host.swap_total = mem_kb.get("SwapTotal").copied().unwrap_or(0);
    host.swap_free = mem_kb.get("SwapFree").copied().unwrap_or(0);
    if host.hostname.is_empty() {
        host.hostname = host.os.clone();
    }
    // `docker stats` names containers by a short id: join it back.
    for s in &stats {
        let sid = s_of(s, "ID");
        let Some(c) = containers
            .iter_mut()
            .find(|c| c.id.starts_with(&sid) || sid.starts_with(&c.id[..c.id.len().min(sid.len())]))
        else {
            continue;
        };
        c.cpu_pct = s_of(s, "CPUPerc").trim_end_matches('%').trim().parse().ok();
        let (used, limit) = mem_usage(&s_of(s, "MemUsage"));
        c.mem_bytes = used;
        c.mem_limit = limit;
        c.pids = s_of(s, "PIDs").trim().parse().ok();
        c.net_io = s_of(s, "NetIO");
        c.block_io = s_of(s, "BlockIO");
    }
    // The inspect's word wins over the stats': `docker stats` shows the whole machine where there is no limit, and has
    // no word on the CPU at all.
    for (id, nano, mem, quota, period) in lims {
        let Some(c) = containers.iter_mut().find(|c| {
            c.id == id || c.id.starts_with(&id[..id.len().min(c.id.len())])
        }) else {
            continue;
        };
        c.cpu_limit = if nano > 0 {
            Some(nano as f64 / 1_000_000_000.0)
        } else if quota > 0 && period > 0 {
            Some(quota as f64 / period as f64)
        } else {
            None
        };
        c.mem_limit = (mem > 0).then_some(mem);
    }
    for c in &mut containers {
        c.gpu_mem_mib = gpu_procs
            .iter()
            .find(|(hex, _)| {
                c.id.starts_with(hex)
                    || hex.starts_with(&c.id[..c.id.len().min(hex.len())])
            })
            .map(|(_, used)| *used);
    }
    // The age is shown as «живёт N»: the docker time is compared with the machine's own clock.
    let _ = now_s;
    Ok((host, containers, nets, vols, images))
}

fn s_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub fn labels_of(s: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for part in s.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            if !k.trim().is_empty() {
                map.insert(k.trim().to_string(), v.to_string());
            }
        }
    }
    map
}

/// Ports the page makes links of: the well-known web ports of a container, when nothing says it is a database.
const WEB_PORTS: &[u16] = &[
    80, 443, 3000, 3001, 4000, 4173, 5000, 5173, 7860, 8000, 8001, 8080, 8081, 8443, 8501, 8888, 9000, 9090, 9200, 11434,
];

/// Web ports that speak TLS: the link must be `https` (Kong's 8443 is its https listener, not a plain one).
const TLS_PORTS: &[u16] = &[443, 8443];

fn port_at(s: &str) -> Option<u16> {
    s.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

/// The image's own name, without a registry, a tag or a digest (`docker.io/library/postgres:16` → `postgres`).
fn image_name(image: &str) -> String {
    let base = image.split('@').next().unwrap_or(image);
    let last = base.rsplit('/').next().unwrap_or(base);
    last.split(':').next().unwrap_or(last).to_ascii_lowercase()
}

/// The service a port speaks: by its usual port first (a custom image still runs a database on 5432), then by the
/// image's name. The page knows a client command for each of these (PLN-44).
pub fn service_of(image: &str, port: Option<u16>) -> Option<Service> {
    match port {
        Some(5432) => return Some(Service::Postgres),
        Some(6379) => return Some(Service::Redis),
        _ => {}
    }
    let name = image_name(image);
    if name.contains("postgres") || name.contains("postgis") || name.contains("timescale") {
        Some(Service::Postgres)
    } else if name.contains("redis") || name.contains("valkey") {
        Some(Service::Redis)
    } else {
        None
    }
}

fn web_image(image: &str) -> bool {
    let name = image_name(image);
    ["nginx", "httpd", "caddy", "traefik", "vite", "nextcloud"]
        .iter()
        .any(|w| name.contains(w))
}

/// The published ports out of docker's `Ports` field (`0.0.0.0:15432->5432/tcp, :::15432->5432/tcp`): a port with
/// no host side (`5432/tcp`) is not published and is left out. A port bound twice (IPv4 and IPv6 any address) is one
/// entry; a loopback binding and a public one of the same port stay apart only when nothing else links them — the
/// public one wins, because it is the one reachable from this machine.
pub fn published_ports(ports: &str, image: &str) -> Vec<PublishedPort> {
    let mut out: Vec<PublishedPort> = Vec::new();
    for part in ports.split(',') {
        let Some((left, right)) = part.trim().split_once("->") else {
            continue;
        };
        let (right, proto) = match right.split_once('/') {
            Some((r, p)) => (r.trim(), p.trim().to_ascii_lowercase()),
            None => (right.trim(), "tcp".to_string()),
        };
        let container_port = port_at(right);
        let left = left.trim();
        let (host_ip, host_port) = match left.rsplit_once(':') {
            Some((ip, p)) => (ip.trim().trim_matches(['[', ']']).to_string(), port_at(p)),
            None => (String::new(), port_at(left)),
        };
        let loopback = host_ip == "localhost"
            || host_ip == "::1"
            || host_ip.strip_prefix("127.").is_some();
        // Any binding but the yard's own loopback is reachable from this machine: the any-address, or one interface
        // (`10.31.21.29:18080->80/tcp` — the page links that very address).
        let public = !loopback;
        // A database image may publish a web port too (redis-stack's 8001 is its browser UI): that one stays a link.
        let service = match container_port {
            Some(p) if WEB_PORTS.contains(&p) => None,
            _ => service_of(image, container_port),
        };
        let http = proto == "tcp"
            && public
            && service.is_none()
            && (container_port.is_some_and(|p| WEB_PORTS.contains(&p)) || web_image(image));
        let one = PublishedPort {
            host_ip,
            host_port,
            container_port,
            proto: if proto.is_empty() { "tcp".into() } else { proto },
            service,
            http,
            https: container_port.is_some_and(|p| TLS_PORTS.contains(&p)),
            loopback,
        };
        let same = |p: &PublishedPort| {
            p.host_port == one.host_port && p.container_port == one.container_port && p.proto == one.proto
        };
        match out.iter_mut().find(|p| same(p)) {
            Some(had) if had.loopback && !one.loopback => *had = one,
            Some(_) => {}
            None => out.push(one),
        }
    }
    out
}

pub fn container_of(v: &Value) -> Container {
    let created = s_of(v, "CreatedAt");
    let size = s_of(v, "Size");
    let image = s_of(v, "Image");
    let ports = s_of(v, "Ports");
    let published = published_ports(&ports, &image);
    Container {
        id: s_of(v, "ID"),
        name: s_of(v, "Names"),
        image,
        state: s_of(v, "State"),
        status: s_of(v, "Status"),
        created_ms: parse_docker_time(&created).unwrap_or(0),
        labels: labels_of(&s_of(v, "Labels")),
        published,
        ports,
        networks: s_of(v, "Networks"),
        mounts: s_of(v, "Mounts"),
        size_bytes: parse_bytes(&size).unwrap_or(0),
        size,
        ..Container::default()
    }
}

/// `96MiB / 1GiB` → (used bytes, limit bytes).
fn mem_usage(s: &str) -> (Option<u64>, Option<u64>) {
    let mut p = s.split('/').map(str::trim);
    let a = p.next().and_then(parse_bytes);
    let b = p.next().and_then(parse_bytes);
    (a, b)
}

/// Docker's human sizes: `0B`, `12.4MiB`, `1.5GB`, `2GiB`.
pub fn parse_bytes(s: &str) -> Option<u64> {
    // `docker ps` prints `182kB (virtual 9.74GB)`: the writable layer is the first word.
    let s = s.trim().split_whitespace().next().unwrap_or("");
    if s.is_empty() || s == "0" {
        return Some(0);
    }
    let at = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let n: f64 = s[..at].parse().ok()?;
    let unit = s[at..].trim();
    let k = match unit {
        "" | "B" => 1.0,
        "kB" | "KB" => 1e3,
        "KiB" | "K" => 1024.0,
        "MB" => 1e6,
        "MiB" | "M" => 1024.0 * 1024.0,
        "GB" => 1e9,
        "GiB" | "G" => 1024.0 * 1024.0 * 1024.0,
        "TB" => 1e12,
        "TiB" | "T" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * k).round() as u64)
}

/// Docker's `2026-10-22 09:00:00 +0000 UTC` as milliseconds since the epoch.
pub fn parse_docker_time(s: &str) -> Option<u64> {
    let p: Vec<&str> = s.split_whitespace().collect();
    let d: Vec<i64> = p.first()?.split('-').filter_map(|x| x.parse().ok()).collect();
    let t: Vec<i64> = p.get(1)?.split(':').filter_map(|x| x.parse().ok()).collect();
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    let tz = p.get(2).copied().unwrap_or("+0000");
    let sign = if tz.starts_with('-') { -1 } else { 1 };
    let off_min = if tz.len() == 5 {
        tz[1..3].parse::<i64>().ok()? * 60 + tz[3..5].parse::<i64>().ok()?
    } else {
        0
    };
    let days = days_from_civil(d[0], d[1], d[2]);
    let secs = days * 86400 + t[0] * 3600 + t[1] * 60 + t[2] - sign * off_min * 60;
    (secs >= 0).then_some(secs as u64 * 1000)
}

/// Days since 1970-01-01 (Howard Hinnant's civil calendar algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The host part of an ssh target (`user@host:port` → `host`), for the ports of the containers.
pub fn host_of_target(target: &str) -> String {
    let t = target.strip_prefix("ssh://").unwrap_or(target);
    let after = t.rsplit('@').next().unwrap_or(t);
    if let Some(rest) = after.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest).to_string();
    }
    after.split(':').next().unwrap_or(after).to_string()
}

/* ───────── whose containers ───────── */

#[derive(Debug, Clone)]
struct Cand {
    id: String,
    tag: String,
    status: String,
    archived: bool,
    exited: bool,
    exited_ms: u64,
    title: String,
}

/// The tasks of this board, by tag, and the keys of its repositories (current and `had`).
#[derive(Debug, Clone, Default)]
pub struct TaskIndex {
    tags: BTreeMap<String, Vec<Cand>>,
    keys: BTreeSet<String>,
    /// Every key of a repository, so a container labelled with an older key still finds the task.
    repo_keys: BTreeMap<String, Vec<String>>,
}

impl TaskIndex {
    pub fn build(
        active: &[Task],
        archived: &[Task],
        repos: &[crate::taskmeta::RepoInfo],
    ) -> TaskIndex {
        let mut idx = TaskIndex::default();
        for r in repos {
            let mut list: Vec<String> = r.keys.iter().chain(std::iter::once(&r.key)).map(|k| k.to_uppercase()).filter(|k| !k.is_empty()).collect();
            list.sort();
            list.dedup();
            for k in &list {
                idx.keys.insert(k.clone());
            }
            idx.repo_keys.insert(r.name.clone(), list);
        }
        for t in active.iter().chain(archived.iter()) {
            idx.local_task(t);
        }
        idx
    }

    fn local_task(&mut self, t: &Task) {
        let key = tag_key(&t.tag).unwrap_or_else(|| t.key.to_uppercase());
        if key.is_empty() {
            return;
        }
        self.keys.insert(key.clone());
        let tag = if t.tag.is_empty() {
            format!("{key}-{}", t.n)
        } else {
            t.tag.to_uppercase()
        };
        let cand = Cand {
            id: t.id.clone(),
            tag: tag.clone(),
            status: t.status.key().to_string(),
            archived: t.archived,
            exited: t.finished_ms > 0,
            exited_ms: t.finished_ms,
            title: t.title.clone(),
        };
        self.tags.entry(tag).or_default().push(cand.clone());
        // A container may carry a key the board used before (`had`); the number is the same task.
        for k in self.repo_keys.get(&t.repo).cloned().unwrap_or_default() {
            let alias = format!("{k}-{}", t.n);
            self.tags.entry(alias).or_default().push(cand.clone());
        }
    }

    fn known_key(&self, key: &str) -> bool {
        self.keys.contains(&key.to_uppercase())
    }

    /// The name of a repository of this board, as a `herdr.shared` label writes it (the value may come from another
    /// machine, so the comparison does not mind case).
    pub fn repo_of(&self, value: &str) -> Option<String> {
        let v = value.trim();
        if v.is_empty() {
            return None;
        }
        self.repo_keys.keys().find(|r| r.eq_ignore_ascii_case(v)).cloned()
    }

    /// The `herdr.task` label of a network or a volume: when its one task is done or archived and its agent has left,
    /// the moment it left (ms). `None` for a label that names no such task.
    pub fn finished_label(&self, label: &str) -> Option<u64> {
        let tag = tag_from_label(label, self)?;
        let cands = self.tags.get(&tag)?;
        let mut uniq: BTreeMap<&str, &Cand> = BTreeMap::new();
        for cand in cands {
            uniq.insert(cand.id.as_str(), cand);
        }
        let list: Vec<&Cand> = uniq.into_values().collect();
        let cand = (list.len() == 1).then(|| list[0])?;
        ((cand.archived || cand.status == "done") && cand.exited).then_some(cand.exited_ms)
    }
}

fn tag_key(tag: &str) -> Option<String> {
    let (key, n) = tag.rsplit_once('-')?;
    if key.is_empty() || n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(key.to_uppercase())
}

/// A board key: 2–5 characters, the first a Latin letter, then letters or digits (`PLN`, `SSP9`).
fn valid_key(key: &str) -> bool {
    (2..=5).contains(&key.len())
        && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && key.chars().all(|c| c.is_ascii_alphanumeric())
}

fn segments(name: &str) -> Vec<&str> {
    name.split(|c| matches!(c, '-' | '_' | '.')).collect()
}

/// The first `KEY-n` in a name whose key is a key of this board.
fn tag_in_name(name: &str, index: &TaskIndex) -> Option<String> {
    let seg = segments(name);
    let mut i = 0;
    while i + 1 < seg.len() {
        let key = seg[i];
        let num = seg[i + 1];
        if valid_key(key)
            && (1..=6).contains(&num.len())
            && num.chars().all(|c| c.is_ascii_digit())
            && index.known_key(key)
        {
            return Some(format!("{}-{}", key.to_uppercase(), num));
        }
        i += 1;
    }
    None
}

fn tag_from_label(value: &str, index: &TaskIndex) -> Option<String> {
    let v = value.trim();
    let (key, n) = v.rsplit_once('-')?;
    if !valid_key(key) || !(1..=6).contains(&n.len()) || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    index
        .known_key(key)
        .then(|| format!("{}-{}", key.to_uppercase(), n))
}

/// A mask with `*`; everything else is literal.
pub fn mask_match(mask: &str, name: &str) -> bool {
    let (m, n) = (mask.as_bytes(), name.as_bytes());
    let (mut mi, mut ni) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if mi < m.len() && (m[mi] == b'?' || m[mi] == n[ni]) {
            mi += 1;
            ni += 1;
        } else if mi < m.len() && m[mi] == b'*' {
            star = Some((mi, ni));
            mi += 1;
        } else if let Some((sm, sn)) = star {
            mi = sm + 1;
            ni = sn + 1;
            star = Some((sm, sn + 1));
        } else {
            return false;
        }
    }
    while mi < m.len() && m[mi] == b'*' {
        mi += 1;
    }
    mi == m.len()
}

/// Whether a network, volume or container name, or its compose project, is under one of the runner's masks.
pub fn name_protected(name: &str, labels: &BTreeMap<String, String>, masks: &[String]) -> bool {
    let project = labels.get("com.docker.compose.project").map(String::as_str).unwrap_or("");
    masks
        .iter()
        .any(|m| mask_match(m, name) || (!project.is_empty() && mask_match(m, project)))
}

pub fn is_protected(c: &Container, masks: &[String]) -> bool {
    name_protected(&c.name, &c.labels, masks)
}

/// Fills `task` and `class` of every container. `herdr.task` is the rule; a tag in the name (its key known to
/// this board) is the fallback; compose containers are read by their project, never by their generated name.
pub fn classify(containers: &mut [Container], protected: &[String], index: &TaskIndex) {
    for c in containers.iter_mut() {
        let (class, task) = classify_one(c, protected, index);
        c.class = class;
        c.task = task;
    }
}

fn classify_one(
    c: &Container,
    protected: &[String],
    index: &TaskIndex,
) -> (Class, Option<TaskRef>) {
    // The build caches of this board's repositories (scripts/yard.sh): a label of their own, never a task's container.
    // A task's label still wins — a container that says which task it serves is classified by that.
    if !c.labels.contains_key("herdr.task")
        && c.labels.get("herdr.shared").and_then(|v| index.repo_of(v)).is_some()
    {
        return (Class::Shared, None);
    }
    if is_protected(c, protected) {
        return (Class::Foreign, None);
    }
    let project = c
        .labels
        .get("com.docker.compose.project")
        .map(String::as_str)
        .unwrap_or("");
    let from_label = c.labels.get("herdr.task").map(|v| v.as_str());
    let tag = match from_label {
        Some(v) => match tag_from_label(v, index) {
            Some(t) => Some(t),
            None => return (Class::Foreign, None),
        },
        None if !project.is_empty() => {
            tag_in_name(project, index).or_else(|| tag_in_name(&c.name, index))
        }
        None => tag_in_name(&c.name, index),
    };
    let Some(tag) = tag else {
        return (Class::Foreign, None);
    };
    let Some(cands) = index.tags.get(&tag) else {
        return (Class::Unsure, None);
    };
    let mut uniq: BTreeMap<&str, &Cand> = BTreeMap::new();
    for cand in cands {
        uniq.insert(cand.id.as_str(), cand);
    }
    let list: Vec<&Cand> = uniq.into_values().collect();
    // One tag in two tasks of this board (an old key and a new one, or a reused number): not sure enough to touch.
    let Some(cand) = (list.len() == 1).then(|| list[0]) else {
        return (Class::Unsure, None);
    };
    let done = (cand.archived || cand.status == "done") && cand.exited;
    let class = if done { Class::Forgotten } else { Class::Active };
    (
        class,
        Some(TaskRef {
            id: cand.id.clone(),
            tag: cand.tag.clone(),
            status: cand.status.clone(),
            title: cand.title.clone(),
            finished_ms: cand.exited_ms,
        }),
    )
}

/// The build caches of the board's repositories: the images and volumes the yard labels `herdr.shared=<repo>` (and any
/// container that carries the same label), in a stable order. A cache whose label names no repository of this board is
/// left out rather than shown as somebody else's.
pub fn shared_of(
    containers: &[Container],
    volumes: &[Vol],
    images: &[ImageInfo],
    index: &TaskIndex,
) -> Vec<SharedItem> {
    let mut out: Vec<SharedItem> = Vec::new();
    for c in containers.iter().filter(|c| c.class == Class::Shared) {
        if let Some(repo) = c.labels.get("herdr.shared").and_then(|v| index.repo_of(v)) {
            out.push(SharedItem { kind: "container".into(), name: c.name.clone(), repo, size_bytes: None });
        }
    }
    for i in images {
        if let Some(repo) = i.labels.get("herdr.shared").and_then(|v| index.repo_of(v)) {
            out.push(SharedItem {
                kind: "image".into(),
                name: i.names.first().cloned().unwrap_or_else(|| i.id.chars().take(12).collect()),
                repo,
                size_bytes: i.size,
            });
        }
    }
    for v in volumes {
        if let Some(repo) = v.labels.get("herdr.shared").and_then(|v| index.repo_of(v)) {
            out.push(SharedItem { kind: "volume".into(), name: v.name.clone(), repo, size_bytes: None });
        }
    }
    // Images, then volumes, then containers; a stable name order inside a kind.
    let rank = |k: &str| match k {
        "image" => 0,
        "volume" => 1,
        _ => 2,
    };
    out.sort_by(|a, b| {
        a.repo
            .cmp(&b.repo)
            .then(rank(&a.kind).cmp(&rank(&b.kind)))
            .then(a.name.cmp(&b.name))
    });
    out
}

/// Fills `shared` of a state from its own read of the yard and this board's repositories.
pub fn fill_shared(state: &mut RunnerState, index: &TaskIndex) {
    state.shared = shared_of(&state.containers, &state.volumes, &state.images, index);
}

/// Marks what the yard's rule asks for and the read does not show (PLN-43): a container of a live task without the
/// CPU or the memory limit, and a live task that keeps more containers than the rule allows. Runs after `classify`
/// and the limits of the list file; a rule of zeros asks for nothing and marks nothing.
pub fn apply_limits(state: &mut RunnerState) {
    let rules = state.limits;
    let mut counts: BTreeMap<String, OverCount> = BTreeMap::new();
    for c in state.containers.iter_mut() {
        let live = c.class == Class::Active;
        c.no_limits = live
            && ((rules.cpus > 0 && c.cpu_limit.is_none())
                || (rules.mem_mb > 0 && c.mem_limit.is_none()));
        if !live || rules.per_task == 0 {
            continue;
        }
        if let Some(t) = c.task.as_ref() {
            let e = counts.entry(t.id.clone()).or_insert_with(|| OverCount {
                task: t.id.clone(),
                tag: t.tag.clone(),
                containers: 0,
                max: rules.per_task,
            });
            e.containers += 1;
        }
    }
    state.over = counts
        .into_values()
        .filter(|o| o.containers > o.max)
        .collect();
}

/// Writes the machine's share into the runners that have no limits of their own yet (PLN-43); a yard that has not
/// answered keeps its absence. True when the file changed.
pub fn fill_limits(file: &mut RunnersFile, states: &[RunnerState]) -> bool {
    let mut changed = false;
    for r in &mut file.runners {
        if r.limits.is_some() {
            continue;
        }
        let Some(s) = states.iter().find(|s| s.id == r.id && s.ok) else {
            continue;
        };
        if s.host.cpus == 0 && s.host.mem_total == 0 {
            continue;
        }
        r.limits = Some(Limits::for_host(&s.host));
        changed = true;
    }
    changed
}

/* ───────── polling ───────── */

/// The planner's words for a runner whose ssh call failed: the hand check is the agents' own command.
fn why(end: &ssh::Ended, m: &MachineRef) -> Why {
    ssh::why_not_with(
        end,
        m,
        &CheckHint {
            test: format!(
                "Проверить с этой машины: {}",
                ssh::hand_check(&m.target, &[], "docker version")
            ),
            program: "docker".into(),
        },
    )
}

/// A failed probe keeps the last good read: the page shows it dimmed with the reason and when it was read.
/// The moment the silence began is kept too (PLN-42): an alarm needs to know how long the yard has been quiet.
pub fn keep_last(prev: Option<&RunnerState>, fresh: RunnerState) -> RunnerState {
    let prev = prev.filter(|p| p.fetched_at_ms > 0);
    match prev {
        Some(p) if !fresh.ok => {
            let down = if p.ok {
                fresh.fetched_at_ms
            } else {
                match p.down_since_ms {
                    0 => fresh.fetched_at_ms,
                    d => d,
                }
            };
            RunnerState {
                ok: false,
                why: fresh.why,
                down_since_ms: down,
                ..p.clone()
            }
        }
        Some(_) => RunnerState {
            down_since_ms: 0,
            ..fresh
        },
        None => {
            let down = if fresh.ok { 0 } else { fresh.fetched_at_ms };
            RunnerState {
                down_since_ms: down,
                ..fresh
            }
        }
    }
}

fn machine_of(r: &Runner) -> MachineRef {
    MachineRef {
        label: r.id.clone(),
        target: r.target.clone(),
        session: String::new(),
    }
}

pub async fn probe(cfg: &AppConfig, r: &Runner) -> RunnerState {
    let mut state = RunnerState::from_runner(r);
    let m = machine_of(r);
    let end = match ssh::ssh_ended(cfg, &m, PROBE_SCRIPT, PROBE_WAIT).await {
        Ok(e) => e,
        Err(e) => {
            state.why = Some(Why {
                line: "адрес верфи не принят".into(),
                detail: vec![e],
            });
            state.fetched_at_ms = now_ms();
            return state;
        }
    };
    if end.timed_out || !end.error.is_empty() || end.code != Some(0) {
        state.why = Some(why(&end, &m));
        state.fetched_at_ms = now_ms();
        return state;
    }
    match parse_probe(&end.stdout) {
        Ok((host, containers, networks, volumes, images)) => {
            state.ok = true;
            state.host = host;
            state.limits = r.effective_limits(Some(&state.host));
            state.containers = containers;
            state.networks = networks;
            state.volumes = volumes;
            state.images = images;
            state.fetched_at_ms = now_ms();
        }
        Err(e) => {
            state.why = Some(Why {
                line: "ответила не то, что ждёт планировщик".into(),
                detail: vec![e],
            });
            state.fetched_at_ms = now_ms();
        }
    }
    state
}

/// Probes every runner in the file at once, stores the reads and puts them into the snapshot.
pub async fn poll_all(app: &Shared) {
    let file = match load(&app.cfg.runners_file()) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("herdr-planner: runners.json: {e}");
            return;
        }
    };
    let calls = file.runners.iter().map(|r| probe(&app.cfg, r));
    let states = futures_util::future::join_all(calls).await;
    let mut old = app.runners.lock().await.clone();
    // A runner taken out of the file leaves the state too (its cache row is dropped by the page's own remove call).
    old.retain(|s| file.get(&s.id).is_some());
    let mut fresh: Vec<RunnerState> = Vec::new();
    for state in states {
        let kept = keep_last(old.iter().find(|s| s.id == state.id), state);
        if let Ok(json) = serde_json::to_string(&kept) {
            app.db.runner_put(
                &kept.id,
                &json,
                kept.ok,
                kept.why
                    .as_ref()
                    .map(|w| w.line.as_str())
                    .unwrap_or(""),
            );
        }
        old.retain(|s| s.id != kept.id);
        fresh.push(kept);
    }
    // A yard read for the first time gives its limits to the list (PLN-43), and the agents' block learns them.
    learn_limits(app, &fresh).await;
    old.extend(fresh);
    old.sort_by(|a, b| a.id.cmp(&b.id));
    *app.runners.lock().await = old;
    crate::app::rebuild_runners(app).await;
    crate::app::publish(app).await;
    crate::app::index_snapshot(app).await;
}

/// Writes the machine's share into the list for the yards that have no limits of their own yet (PLN-43), then brings
/// the agents' block up to date. Nothing is written for a yard that did not answer. True when the file changed.
pub async fn learn_limits(app: &Shared, states: &[RunnerState]) -> bool {
    let path = app.cfg.runners_file();
    let mut file = match load(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("herdr-planner: runners.json: {e}");
            return false;
        }
    };
    if !fill_limits(&mut file, states) {
        return false;
    }
    file.updated_at_ms = now_ms();
    if let Err(e) = save(&path, &file) {
        eprintln!("herdr-planner: runners.json: {e}");
        return false;
    }
    sync_blocks(app).await;
    true
}

/* ───────── the «было → стало» plan ───────── */

#[derive(Debug, Clone, Default, Serialize)]
pub struct PlanItem {
    pub id: String,
    pub name: String,
    pub task: String,
    pub status: String,
    pub state: String,
    pub age_ms: u64,
    pub mem_bytes: u64,
    pub size_bytes: u64,
    /// The networks and volumes that go with this container alone: the page computes «стало» from the ticked ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Counts {
    pub containers: usize,
    pub networks: usize,
    pub volumes: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct CleanPlan {
    pub runner: String,
    pub containers: Vec<PlanItem>,
    pub networks: Vec<PlanItem>,
    pub volumes: Vec<PlanItem>,
    pub was: Counts,
    pub now: Counts,
    pub unsure: Vec<String>,
    pub at: Instant,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Preview {
    pub ok: bool,
    pub runner: String,
    pub containers: Vec<PlanItem>,
    pub networks: Vec<PlanItem>,
    pub volumes: Vec<PlanItem>,
    pub was: Counts,
    pub now: Counts,
    pub unsure: Vec<String>,
    pub token: String,
    pub expires_ms: u64,
}

fn plan_item(c: &Container, now: u64) -> PlanItem {
    PlanItem {
        id: c.id.clone(),
        name: c.name.clone(),
        task: c
            .task
            .as_ref()
            .map(|t| t.tag.clone())
            .unwrap_or_else(|| "—".into()),
        status: c
            .task
            .as_ref()
            .map(|t| t.status.clone())
            .unwrap_or_default(),
        state: c.state.clone(),
        age_ms: now.saturating_sub(c.created_ms),
        mem_bytes: c.mem_bytes.unwrap_or(0),
        size_bytes: c.size_bytes,
        ..PlanItem::default()
    }
}

/// A tag (`KEY-n`) as two neighbouring segments of a name: `work-ALP-3_default` and `vol-ALP-3` yes,
/// `alp-30_default` no. A plain substring would take the neighbour's `ALP-30` for `ALP-3`.
fn tag_in_segments(name: &str, tag: &str) -> bool {
    let Some((key, num)) = tag.rsplit_once('-') else {
        return false;
    };
    let seg: Vec<String> = segments(name).iter().map(|s| s.to_ascii_uppercase()).collect();
    seg.windows(2)
        .any(|w| w[0] == key.to_ascii_uppercase() && w[1] == num)
}

fn project_owns(name: &str, projects: &BTreeSet<String>) -> bool {
    projects
        .iter()
        .any(|p| name == p || name.starts_with(&format!("{p}_")))
}

fn network_matches(name: &str, tags: &BTreeSet<String>, projects: &BTreeSet<String>) -> bool {
    tags.iter().any(|t| tag_in_segments(name, t)) || project_owns(name, projects)
}

pub fn volume_matches(name: &str, tags: &BTreeSet<String>, projects: &BTreeSet<String>) -> bool {
    tags.iter().any(|t| tag_in_segments(name, t)) || project_owns(name, projects)
}

fn mounts_of(c: &Container) -> Vec<String> {
    c.mounts
        .split(',')
        .filter_map(|m| m.split(':').next())
        .map(str::trim)
        .filter(|m| !m.is_empty() && !m.starts_with('/'))
        .map(str::to_string)
        .collect()
}

/// Builds the plan from the last read of a runner. `ids` narrows it to the containers the human ticked; a container
/// that is active or foreign is never in a plan.
pub fn plan_of(state: &RunnerState, ids: Option<&[String]>) -> CleanPlan {
    let now = now_ms();
    // Each picked container with the units that belong to it alone, so the page can count «стало» for any ticks.
    let mut picked: Vec<(&Container, Vec<String>, Vec<String>)> = Vec::new();
    let mut unsure: Vec<String> = Vec::new();
    for c in &state.containers {
        let asked = ids.is_some_and(|list| list.iter().any(|id| id == &c.id));
        let wanted = match ids {
            Some(_) => asked,
            // The unsure ones are shown in the card too, greyed and unticked: the human may tick them.
            None => matches!(c.class, Class::Forgotten | Class::Unsure),
        };
        if !wanted {
            continue;
        }
        match c.class {
            Class::Forgotten => {}
            // The task of an unsure container is unknown, so its networks and volumes are unknown too: the plan
            // takes only the container itself.
            Class::Unsure => {
                unsure.push(c.id.clone());
                picked.push((c, Vec::new(), Vec::new()));
                continue;
            }
            Class::Active | Class::Foreign | Class::Shared => continue,
        }
        let mut tags: BTreeSet<String> = BTreeSet::new();
        let mut projects: BTreeSet<String> = BTreeSet::new();
        if let Some(t) = &c.task {
            tags.insert(t.tag.to_uppercase());
        }
        if let Some(p) = c.labels.get("com.docker.compose.project") {
            projects.insert(p.clone());
        }
        let nets: Vec<String> = state
            .networks
            .iter()
            .filter(|n| network_matches(&n.name, &tags, &projects))
            .map(|n| n.id.clone())
            .collect();
        let shared = |name: &str| {
            state
                .volumes
                .iter()
                .any(|v| v.name == name && v.labels.contains_key("herdr.shared"))
        };
        let mut vols: Vec<String> = state
            .volumes
            .iter()
            .filter(|v| !v.labels.contains_key("herdr.shared") && volume_matches(&v.name, &tags, &projects))
            .map(|v| v.name.clone())
            .collect();
        // A shared build cache (scripts/yard.sh) is never a task's to take, even when the container mounts it.
        for m in mounts_of(c) {
            if !vols.contains(&m) && !shared(&m) {
                vols.push(m);
            }
        }
        picked.push((c, nets, vols));
    }
    let mut net_ids: BTreeSet<String> = BTreeSet::new();
    let mut vol_names: BTreeSet<String> = BTreeSet::new();
    let mut containers: Vec<PlanItem> = Vec::new();
    for (c, nets, vols) in &picked {
        net_ids.extend(nets.iter().cloned());
        vol_names.extend(vols.iter().cloned());
        let mut item = plan_item(*c, now);
        item.networks = nets.clone();
        item.volumes = vols.clone();
        containers.push(item);
    }
    let bytes = picked.iter().map(|(c, _, _)| c.size_bytes).sum();
    let networks: Vec<PlanItem> = state
        .networks
        .iter()
        .filter(|n| net_ids.contains(&n.id))
        .map(|n| PlanItem {
            id: n.id.clone(),
            name: n.name.clone(),
            ..PlanItem::default()
        })
        .collect();
    let volumes: Vec<PlanItem> = vol_names
        .into_iter()
        .map(|name| PlanItem {
            name,
            ..PlanItem::default()
        })
        .collect();
    // The card is about what can be cleaned: the board's shared build caches are not counted in «было → стало».
    let shared_containers = state.containers.iter().filter(|c| c.class == Class::Shared).count();
    let shared_volumes = state.volumes.iter().filter(|v| v.labels.contains_key("herdr.shared")).count();
    let was = Counts {
        containers: state.containers.len().saturating_sub(shared_containers),
        networks: state.networks.len(),
        volumes: state.volumes.len().saturating_sub(shared_volumes),
        bytes,
    };
    let now_counts = Counts {
        containers: was.containers.saturating_sub(containers.len()),
        networks: was.networks.saturating_sub(networks.len()),
        volumes: was.volumes.saturating_sub(volumes.len()),
        bytes,
    };
    CleanPlan {
        runner: state.id.clone(),
        containers,
        networks,
        volumes,
        was,
        now: now_counts,
        unsure,
        at: Instant::now(),
    }
}

pub fn preview_of(plan: &CleanPlan, token: &str) -> Preview {
    Preview {
        ok: true,
        runner: plan.runner.clone(),
        containers: plan.containers.clone(),
        networks: plan.networks.clone(),
        volumes: plan.volumes.clone(),
        was: plan.was.clone(),
        now: plan.now.clone(),
        unsure: plan.unsure.clone(),
        token: token.to_string(),
        expires_ms: now_ms() + PLAN_TTL_MS,
    }
}

/* ───────── what the agents are told ───────── */

pub const BLOCK_BEGIN: &str = "<!-- herdr-planner:runners -->";
pub const BLOCK_END: &str = "<!-- /herdr-planner:runners -->";

pub const MARKERS: crate::blocks::Markers = crate::blocks::Markers {
    begin: BLOCK_BEGIN,
    end: BLOCK_END,
};

/// The docker rule for the board's agents, generated from the list. It names the default yard, every other one, the
/// protected masks, and the task label the planner reads back.
pub fn block_text(file: &RunnersFile) -> String {
    let Some(def) = file.default_runner() else {
        return String::new();
    };
    let host_of = |r: &Runner| {
        if r.ports_host.is_empty() {
            host_of_target(&r.target)
        } else {
            r.ports_host.clone()
        }
    };
    let def_target = ssh::normalize_target(&def.target);
    let check = ssh::hand_check(&def.target, &["-o", "BatchMode=yes"], "true");
    let mut s = String::from(BLOCK_BEGIN);
    s.push('\n');
    s.push_str(&format!(
        "Docker-контейнеры (базы, стенды, заглушки, прокси и т.п.) не запускай на этой машине: используй верфь {} — export DOCKER_HOST=ssh://{} (вход по SSH-ключу уже настроен; проверка: {}). Порты контейнеров открываются на {}, не на localhost.\n",
        def.id, def_target, check, host_of(def)
    ));
    s.push_str("Каждому контейнеру, сети и тому давай имя с id своей задачи (например pg-PLN-26) и метку --label herdr.task=PLN-26; для docker compose — имя проекта с id задачи (docker compose -p pln-26 …) и та же метка в labels каждого сервиса; после себя всегда останавливай и удаляй их (docker rm -f -v, docker network rm, docker volume rm, docker compose -p pln-26 down -v), даже если работа прервалась.\n");
    let def_limits = def.effective_limits(None);
    if let Some(limits) = limits_paragraph(&def_limits) {
        s.push_str(&limits);
    }
    let masks = file.protected_all();
    if !masks.is_empty() {
        s.push_str(&format!(
            "Чужие контейнеры ({}) не трогай.\n",
            masks.join(", ")
        ));
    }
    let others: Vec<&Runner> = file.runners.iter().filter(|r| r.id != def.id).collect();
    if !others.is_empty() {
        let list = others
            .iter()
            .map(|r| {
                let tags = if r.tags.is_empty() {
                    String::new()
                } else {
                    format!(", {}", r.tags.join(", "))
                };
                // A yard with limits of its own says so here; the rest follow the default yard's.
                let limits = r.effective_limits(None);
                let differs = if limits == def_limits {
                    String::new()
                } else {
                    format!("; лимиты: {}", limits_words(&limits))
                };
                format!(
                    "{} (ssh://{}, порты {}{tags}{differs})",
                    r.id,
                    ssh::normalize_target(&r.target),
                    host_of(r)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        s.push_str(&format!("Ещё верфи: {list}.\n"));
    }
    s.push_str(&format!(
        "Если верфь не отвечает — не запускай контейнеры здесь, а остановись: herdr-projects report <id> --state blocked --kind permission --msg \"Верфь {} не отвечает\".\n",
        def.id
    ));
    s.push_str(BLOCK_END);
    s.push('\n');
    s
}

/// The limits as the agents' commands take them: `--cpus 4 --memory 4g`, and the containers' maximum when it is set.
fn limits_words(l: &Limits) -> String {
    let mut flags: Vec<String> = Vec::new();
    if l.cpus > 0 {
        flags.push(format!("--cpus {}", l.cpu_text()));
    }
    if l.mem_mb > 0 {
        flags.push(format!("--memory {}", l.mem_text()));
    }
    let mut s = flags.join(" ");
    if l.per_task == 1 {
        s.push_str(", не больше одного контейнера на задачу");
    } else if l.per_task > 1 {
        s.push_str(&format!(", не больше {} контейнеров на задачу", l.per_task));
    }
    s
}

/// The paragraph of the agents' rule about the limits of a task's container (PLN-43), or `None` when the yard asks
/// for none. The examples carry the very values of the rule: `docker run` and one compose service.
fn limits_paragraph(l: &Limits) -> Option<String> {
    if !l.any() {
        return None;
    }
    let mut flags: Vec<String> = Vec::new();
    let mut yaml = String::new();
    if l.cpus > 0 {
        flags.push(format!("--cpus {}", l.cpu_text()));
        yaml.push_str(&format!("          cpus: \"{}\"\n", l.cpu_text()));
    }
    if l.mem_mb > 0 {
        flags.push(format!("--memory {}", l.mem_text()));
        yaml.push_str(&format!("          memory: {}\n", l.mem_text()));
    }
    let flags = flags.join(" ");
    Some(format!(
        "Лимиты контейнера задачи: {words}. Так: docker run -d --name pg-PLN-26 --label herdr.task=PLN-26 {flags} postgres:16. В compose — те же значения каждому сервису:\n```yaml\nservices:\n  db:\n    image: postgres:16\n    labels: [\"herdr.task=PLN-26\"]\n    deploy:\n      resources:\n        limits:\n{yaml}```\nКонтейнер задачи без этих лимитов помечается в «Фокусе» верфи.\n",
        words = limits_words(l),
        flags = flags,
        yaml = yaml
    ))
}

pub fn connect_value(
    current: &str,
    block: &str,
    on: bool,
) -> Result<Option<String>, String> {
    crate::blocks::connect_value(current, MARKERS, block, on)
}

pub fn has_block(text: &str, block: &str) -> bool {
    crate::blocks::has_block(text, MARKERS, block)
}

pub fn has_any_block(text: &str) -> bool {
    crate::blocks::has_any_block(text, MARKERS)
}

pub fn has_old_docker_paragraph(text: &str) -> bool {
    crate::blocks::has_paragraph(text, MARKERS, "DOCKER_HOST=")
}

/* ───────── the agents' block ───────── */

/// Rewrites the docker block in this machine's instructions when the list changed. The consent was given when the
/// block was connected on the Station; before that nothing is touched. The last runner gone — the block goes too.
pub async fn sync_blocks(app: &Shared) {
    let file = match load(&app.cfg.runners_file()) {
        Ok(f) => f,
        Err(_) => return,
    };
    let block = block_text(&file);
    let was = app.runner_block.lock().await.clone();
    let settings = match crate::station::read_settings(&app.cfg, "").await {
        Ok(s) => s,
        Err(e) => {
            *app.runner_block.lock().await = BlockState {
                connected: was.connected,
                stale: was.connected,
                at: now_ms(),
                why: e.message(),
            };
            return;
        }
    };
    let mut connected = false;
    let mut stale = false;
    let mut why = String::new();
    let mut changed = false;
    for name in ["worker.instructions", "reviewer.instructions"] {
        let Some(setting) = crate::station::find(&settings, name) else {
            continue;
        };
        let current = if setting.from == "built in" || setting.value.trim() == "none" {
            String::new()
        } else {
            setting.value.clone()
        };
        if !has_any_block(&current) {
            continue;
        }
        connected = true;
        if !block.is_empty() && !has_block(&current, &block) {
            stale = true;
        }
        if !was.connected {
            continue;
        }
        let value = match connect_value(&current, &block, !block.is_empty()) {
            Ok(Some(v)) => v,
            Ok(None) => {
                let args = vec!["unset".to_string(), name.to_string()];
                let out = crate::station::board_run(&app.cfg, &args, crate::station::BOARD_WAIT).await;
                if out.ok() {
                    changed = true;
                } else {
                    why = out.words();
                }
                continue;
            }
            Err(e) => {
                why = e;
                continue;
            }
        };
        if value == current {
            continue;
        }
        let args = vec!["set".to_string(), name.to_string(), value];
        let out = crate::station::board_run(&app.cfg, &args, crate::station::BOARD_WAIT).await;
        if out.ok() {
            changed = true;
        } else {
            why = out.words();
        }
    }
    if block.is_empty() && changed {
        connected = false;
    }
    if changed {
        stale = false;
        crate::app::journal_note(
            app,
            "runner/block",
            "runner",
            "блок правил о верфях обновлён в инструкциях агентов",
        )
        .await;
    }
    let state = BlockState {
        connected,
        stale,
        at: now_ms(),
        why,
    };
    *app.runner_block.lock().await = state.clone();
    if let Ok(json) = serde_json::to_string(&state) {
        app.db.meta_set("runner_block", &json);
    }
    crate::app::rebuild_runners(app).await;
    crate::app::publish(app).await;
}

/* ───────── tests ───────── */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Status;

    fn task(id: &str, tag: &str, status: Status, archived: bool) -> Task {
        Task {
            id: id.into(),
            repo: id.split('/').next().unwrap_or("").into(),
            n: id.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(0),
            tag: tag.into(),
            key: tag_key(tag).unwrap_or_default(),
            status,
            archived,
            finished_ms: if archived || matches!(status, crate::board::Status::Done) { 1 } else { 0 },
            title: tag.into(),
            ..Task::default()
        }
    }

    fn cont(name: &str, labels: &[(&str, &str)]) -> Container {
        Container {
            id: "a".repeat(64),
            name: name.into(),
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            ..Container::default()
        }
    }

    #[test]
    fn a_mask_matches_with_a_star_only() {
        assert!(mask_match("neighbor-server-*", "neighbor-server-abc-1"));
        assert!(mask_match("buildx_*", "buildx_buildkit_x0"));
        assert!(mask_match("other", "other"));
        assert!(!mask_match("other", "other-db"));
        assert!(!mask_match("neighbor*", "nope"));
    }

    fn repo(name: &str, key: &str) -> crate::taskmeta::RepoInfo {
        crate::taskmeta::RepoInfo {
            name: name.into(),
            key: key.into(),
            keys: vec![key.into()],
            root: String::new(),
        }
    }

    #[test]
    fn the_name_parser_takes_only_keys_this_board_knows() {
        let mut idx = TaskIndex::default();
        for k in ["ALP", "PLN", "SSP9"] {
            idx.keys.insert(k.into());
        }
        assert_eq!(tag_in_name("pg-ALP-3", &idx).as_deref(), Some("ALP-3"));
        assert_eq!(tag_in_name("alp-3-pg-1", &idx).as_deref(), Some("ALP-3"));
        assert_eq!(tag_in_name("pg-PLN-26", &idx).as_deref(), Some("PLN-26"));
        // A key with a digit is a board key too (samsungs9plus → SSP9).
        assert_eq!(tag_in_name("pg-SSP9-3", &idx).as_deref(), Some("SSP9-3"));
        assert_eq!(tag_in_name("ssp9-3-pg-1", &idx).as_deref(), Some("SSP9-3"));
        assert_eq!(tag_in_name("ssp9-3_default", &idx).as_deref(), Some("SSP9-3"));
        // Compose names are not tags unless their key is a key of this board.
        assert_eq!(tag_in_name("myapp-db-1", &idx), None);
        assert_eq!(tag_in_name("neighbor-server-x-app-1", &idx), None);
        assert_eq!(tag_in_name("neighbor-db-1", &idx), None);
        its_foreign(&["myapp-db-1", "gigam-server-x-gigam-1"]);
    }

    /// The containers of a board without these keys stay foreign (a name alone is never enough).
    fn its_foreign(names: &[&str]) {
        let idx = TaskIndex::default();
        for n in names {
            assert_eq!(tag_in_name(n, &idx), None, "{n}");
        }
    }

    #[test]
    fn the_four_classes_follow_the_snapshot() {
        let active = vec![task("alpha/3", "ALP-3", Status::Working, false)];
        let archived = vec![task("alpha/1", "ALP-1", Status::Done, true)];
        let repos = vec![repo("alpha", "ALP")];
        let idx = TaskIndex::build(&active, &archived, &repos);
        let mut list = vec![
            cont("pg-ALP-3", &[("com.docker.compose.project", "work-3")]),
            cont("web-ALP-1", &[]),
            cont("x-ALP-99", &[]),
            cont("neighbor-db-1", &[("com.docker.compose.project", "neighbor")]),
            cont("buildx_buildkit_x0", &[]),
            cont("neighbor-server-x-app-1", &[]),
        ];
        classify(&mut list, &["buildx_*".into()], &idx);
        assert_eq!(list[0].class, Class::Active);
        assert_eq!(list[0].task.as_ref().unwrap().id, "alpha/3");
        assert_eq!(list[1].class, Class::Forgotten);
        assert_eq!(list[2].class, Class::Unsure);
        assert_eq!(list[3].class, Class::Foreign);
        assert_eq!(list[4].class, Class::Foreign);
        assert_eq!(list[5].class, Class::Foreign);
    }

    #[test]
    fn a_build_cache_of_a_board_repository_is_shared_not_foreign() {
        let active = vec![task("alpha/3", "ALP-3", Status::Working, false)];
        let idx = TaskIndex::build(&active, &[], &[repo("alpha", "ALP")]);
        let mut list = vec![
            // The yard's cache volume's sibling container, and a task container that also carries the cache label.
            cont("yard-ALP-3", &[("herdr.shared", "alpha")]),
            cont("pg-ALP-3", &[("herdr.task", "ALP-3"), ("herdr.shared", "alpha")]),
            // A label naming no repository of this board: still nobody to touch.
            cont("someone-else", &[("herdr.shared", "neighbor")]),
            // Case does not matter (the label comes from another machine).
            cont("yard-ALP-4", &[("herdr.shared", "Alpha")]),
        ];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Shared);
        assert_eq!(list[1].class, Class::Active, "a task's label wins over a cache's");
        assert_eq!(list[2].class, Class::Foreign);
        assert_eq!(list[3].class, Class::Shared);
    }

    #[test]
    fn the_shared_list_names_repository_image_and_volume_with_their_sizes() {
        let idx = TaskIndex::build(&[], &[], &[repo("alpha", "ALP"), repo("beta", "BET")]);
        let mut list = vec![cont("yard-ALP-3", &[("herdr.shared", "alpha")])];
        classify(&mut list, &[], &idx);
        let mut labels = BTreeMap::new();
        labels.insert("herdr.shared".to_string(), "alpha".to_string());
        let vols = vec![
            Vol { name: "hp-yard-cache".into(), driver: "local".into(), labels: labels.clone() },
            Vol { name: "work-3_data".into(), driver: "local".into(), labels: BTreeMap::new() },
        ];
        let images = vec![
            ImageInfo {
                id: "b".repeat(64),
                names: vec!["herdr-planner-yard:0123456789ab".into()],
                size: Some(2_500_000_000),
                labels,
            },
            ImageInfo { id: "c".repeat(64), ..ImageInfo::default() },
        ];
        let shared = shared_of(&list, &vols, &images, &idx);
        let kinds: Vec<(&str, &str)> = shared.iter().map(|s| (s.kind.as_str(), s.name.as_str())).collect();
        assert_eq!(kinds, vec![("image", "herdr-planner-yard:0123456789ab"), ("volume", "hp-yard-cache"), ("container", "yard-ALP-3")]);
        assert_eq!(shared[0].repo, "alpha");
        assert_eq!(shared[0].size_bytes, Some(2_500_000_000));
        assert_eq!(shared[1].size_bytes, None, "a volume's size is only known from docker system df -v");
    }

    #[test]
    fn a_shared_cache_volume_is_never_taken_with_a_forgotten_container() {
        let archived = vec![task("alpha/1", "ALP-1", Status::Done, true)];
        let idx = TaskIndex::build(&[], &archived, &[repo("alpha", "ALP")]);
        let mut c = cont("yard-ALP-1", &[("herdr.task", "ALP-1")]);
        c.id = "d".repeat(64);
        c.mounts = "hp-yard-cache:/cache".into();
        let mut cache = BTreeMap::new();
        cache.insert("herdr.shared".to_string(), "alpha".to_string());
        let mut state = RunnerState::default();
        state.containers = vec![c];
        state.volumes = vec![
            Vol { name: "hp-yard-cache".into(), driver: "local".into(), labels: cache },
            Vol { name: "work-ALP-1_data".into(), driver: "local".into(), labels: BTreeMap::new() },
        ];
        classify(&mut state.containers, &[], &idx);
        let plan = plan_of(&state, None);
        let names: Vec<&str> = plan.volumes.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, vec!["work-ALP-1_data"], "the shared cache stays; the task's own volume goes");
    }

    #[test]
    fn an_older_key_of_the_repository_finds_the_same_task() {
        let active = vec![task("alpha/4", "ALP-4", Status::Working, false)];
        let repos = vec![crate::taskmeta::RepoInfo {
            name: "alpha".into(),
            key: "ALP".into(),
            keys: vec!["AL9".into(), "ALP".into()],
            root: String::new(),
        }];
        let idx = TaskIndex::build(&active, &[], &repos);
        let mut list = vec![cont("pg-AL9-4", &[("herdr.task", "AL9-4")])];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Active);
        assert_eq!(list[0].task.as_ref().unwrap().id, "alpha/4");
    }

    #[test]
    fn a_done_task_whose_agent_still_runs_is_not_forgotten_yet() {
        let mut done = task("alpha/1", "ALP-1", Status::Done, false);
        done.finished_ms = 0;
        let idx = TaskIndex::build(&[], &[done], &[repo("alpha", "ALP")]);
        let mut list = vec![cont("web-ALP-1", &[])];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Active);
    }

    #[test]
    fn compose_containers_are_read_by_their_project_not_their_name() {
        let active = vec![task("alpha/3", "ALP-3", Status::Working, false)];
        let idx = TaskIndex::build(&active, &[], &[repo("alpha", "ALP")]);
        let mut list = vec![cont(
            "some-PLN-3-name",
            &[("com.docker.compose.project", "neighbor")],
        )];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Foreign);
        let mut list = vec![cont(
            "alp-3-pg-1",
            &[("com.docker.compose.project", "alp-3")],
        )];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Active);
    }

    #[test]
    fn a_key_with_a_digit_is_read_through_its_project_and_name() {
        let active = vec![task("samsungs9plus/3", "SSP9-3", Status::Working, false)];
        let idx = TaskIndex::build(&active, &[], &[repo("samsungs9plus", "SSP9")]);
        let mut list = vec![
            cont("pg-SSP9-3", &[]),
            cont("ssp9-3-pg-1", &[("com.docker.compose.project", "ssp9-3")]),
            cont("ssp9-3_default", &[]),
        ];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Active);
        assert_eq!(list[0].task.as_ref().unwrap().id, "samsungs9plus/3");
        assert_eq!(list[1].class, Class::Active);
        assert_eq!(list[2].class, Class::Active);
        let foreign = TaskIndex::build(&[], &[], &[repo("alpha", "ALP")]);
        classify(&mut list, &[], &foreign);
        assert!(list.iter().all(|c| c.class == Class::Foreign));
    }

    #[test]
    fn the_label_wins_and_an_unknown_key_is_foreign() {
        let active = vec![task("alpha/3", "ALP-3", Status::Working, false)];
        let idx = TaskIndex::build(&active, &[], &[repo("alpha", "ALP")]);
        let mut list = vec![
            cont("anything", &[("herdr.task", "ALP-3")]),
            cont("pg-ALP-3", &[("herdr.task", "ZZZ-9")]),
        ];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Active);
        assert_eq!(list[1].class, Class::Foreign);
    }

    #[test]
    fn one_tag_in_two_tasks_is_unsure() {
        let active = vec![
            task("alpha/3", "ALP-3", Status::Working, false),
            task("alpha/7", "ALP-3", Status::Working, false),
        ];
        let idx = TaskIndex::build(&active, &[], &[repo("alpha", "ALP")]);
        let mut list = vec![cont("pg-ALP-3", &[])];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Unsure);
    }

    #[test]
    fn the_moment_a_yard_went_quiet_survives_more_probes() {
        let mut good = RunnerState::default();
        good.ok = true;
        good.fetched_at_ms = 7;
        let mut bad = RunnerState::default();
        bad.fetched_at_ms = 10;
        bad.why = Some(Why { line: "нет ответа".into(), detail: vec![] });
        let first = keep_last(Some(&good), bad);
        assert!(!first.ok);
        assert_eq!(first.down_since_ms, 10, "the silence began with this failed read");
        let mut bad2 = RunnerState::default();
        bad2.fetched_at_ms = 40;
        let second = keep_last(Some(&first), bad2);
        assert_eq!(second.down_since_ms, 10, "the moment stays, not the newest read");
        let mut ok = RunnerState::default();
        ok.ok = true;
        ok.fetched_at_ms = 90;
        let third = keep_last(Some(&second), ok);
        assert_eq!(third.down_since_ms, 0, "an answer ends the silence");
        let mut never = RunnerState::default();
        never.fetched_at_ms = 5;
        let fourth = keep_last(None, never);
        assert_eq!(fourth.down_since_ms, 5, "a yard that never answered is silent since its first read");
    }

    #[test]
    fn a_failed_probe_keeps_the_last_good_read() {
        let mut good = RunnerState::default();
        good.ok = true;
        good.fetched_at_ms = 7;
        good.host.cpus = 16;
        good.containers = vec![cont("pg-ALP-3", &[])];
        let mut fresh = RunnerState::default();
        fresh.why = Some(Why { line: "не ответила вовремя".into(), detail: vec![] });
        fresh.fetched_at_ms = 9;
        let kept = keep_last(Some(&good), fresh);
        assert!(!kept.ok);
        assert_eq!(kept.host.cpus, 16);
        assert_eq!(kept.containers.len(), 1);
        assert_eq!(kept.fetched_at_ms, 7);
        assert_eq!(kept.why.unwrap().line, "не ответила вовремя");
    }

    #[test]
    fn the_probe_declares_every_section_its_output_has() {
        // parse_probe keys on the '== name' markers: JSON printed without its own marker is read as the previous
        // section's rows (the shared image once came out as a nameless volume, so «Кэш сборок» never saw it).
        for section in ["ps", "stats", "lim", "net", "vol", "img"] {
            assert!(PROBE_SCRIPT.contains(&format!("== {section}'")), "no '== {section}' marker in the probe");
        }
        let images = PROBE_SCRIPT.find("== img'").expect("the image marker");
        assert!(
            PROBE_SCRIPT[images..].contains("docker image inspect"),
            "the image JSON must come after its own marker"
        );
        // The limits come from one inspect of every container: `docker ps` and `docker stats` have no word on them.
        let limits = PROBE_SCRIPT.find("== lim'").expect("the limits marker");
        assert!(
            PROBE_SCRIPT[limits..].contains("docker inspect --format"),
            "the limits must be read from the container's HostConfig"
        );
        assert!(
            PROBE_SCRIPT[limits..].contains("HostConfig.NanoCpus") && PROBE_SCRIPT[limits..].contains("HostConfig.Memory"),
            "the CPU and the memory limit of a container"
        );
    }

    #[test]
    fn the_probe_output_is_read() {
        let out = format!(
            "Welcome to vm-r (Ubuntu 24.04 LTS)\nherdr-planner-runner 1\n\
             now\t1792700000\n\
             load\t0.66 0.46 0.37 1/100 2\n\
             cpus\t16\n\
             arch\tx86_64\n\
             host\tvm-r\n\
             os\tUbuntu 24.04 LTS\n\
             mem\tMemTotal:       32768000 kB\n\
             mem\tMemAvailable:   20000000 kB\n\
             mem\tSwapTotal:       4096000 kB\n\
             mem\tSwapFree:        3000000 kB\n\
             disk\t/var/lib/docker\t1000000000000 228000000000\n\
             docker\t28.3.0 x86_64\n\
             compose\tv2.17.3\n\
             info\t16|33649324032|30|30|152|Ubuntu 22.04.2 LTS|runc,nvidia,\n\
             gpu\t0, NVIDIA L4-24Q, 12, 1337, 24576, 45\n\
             gpuproc\t1234,1337\tdocker-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
             == ps\n\
             {{\"ID\":\"{}\",\"Names\":\"pg-ALP-3\",\"Image\":\"postgres\",\"State\":\"running\",\"Status\":\"Up 1 hour\",\"CreatedAt\":\"2026-10-22 09:00:00 +0000 UTC\",\"Labels\":\"herdr.task=ALP-3,com.docker.compose.project=work-3\",\"Ports\":\"0.0.0.0:15432->5432/tcp\",\"Networks\":\"work-3_default\",\"Mounts\":\"work-3_pgdata:/var/lib/postgresql/data\",\"Size\":\"28MB\"}}\n\
             == stats\n\
             {{\"ID\":\"aaaaaaaaaaaa\",\"CPUPerc\":\"0.40%\",\"MemUsage\":\"96MiB / 1GiB\",\"PIDs\":\"7\",\"NetIO\":\"1kB / 2kB\",\"BlockIO\":\"0B / 0B\"}}\n\
             == lim\n\
             {}|4000000000|1073741824|0|0\n\
             == net\n\
             {{\"ID\":\"net1\",\"Name\":\"work-3_default\",\"Driver\":\"bridge\",\"Labels\":\"\"}}\n\
             == vol\n\
             {{\"Name\":\"work-3_pgdata\",\"Driver\":\"local\",\"Labels\":\"\"}}\n\
             == img\n\
             {{\"Id\":\"{}\",\"RepoTags\":[\"herdr-planner-yard:0123456789ab\"],\"Size\":2500000000,\"Config\":{{\"Labels\":{{\"herdr.shared\":\"alpha\"}}}}}}\n",
            "a".repeat(64), "a".repeat(64), "b".repeat(64)
        );
        let (host, containers, nets, vols, images) = parse_probe(&out).unwrap();
        assert_eq!(host.cpus, 16);
        assert_eq!(host.mem_total, 32768000 * 1024);
        assert_eq!(host.disk_avail, 228000000000);
        assert_eq!(host.gpu[0].name, "NVIDIA L4-24Q");
        assert!(host.runtimes.contains(&"nvidia".to_string()));
        assert_eq!(containers.len(), 1);
        let c = &containers[0];
        assert_eq!(c.name, "pg-ALP-3");
        assert_eq!(c.cpu_pct, Some(0.4));
        assert_eq!(c.mem_bytes, Some(96 * 1024 * 1024));
        // The inspect's word, not the stats': 4 cores and 1 GiB, though the stats show the same limit this time.
        assert_eq!(c.cpu_limit, Some(4.0));
        assert_eq!(c.mem_limit, Some(1024 * 1024 * 1024));
        assert_eq!(c.gpu_mem_mib, Some(1337));
        assert_eq!(c.created_ms, 1792659600000);
        assert_eq!(nets[0].name, "work-3_default");
        assert_eq!(vols[0].name, "work-3_pgdata");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].names, vec!["herdr-planner-yard:0123456789ab"]);
        assert_eq!(images[0].size, Some(2_500_000_000));
        assert_eq!(images[0].labels.get("herdr.shared").map(String::as_str), Some("alpha"));
    }

    #[test]
    fn the_default_limits_are_a_share_of_the_machine() {
        // A 16-core yard with 32 GiB: a quarter of the cores, an eighth of the memory (rounded to half a GiB).
        let host = HostState {
            cpus: 16,
            mem_total: 32 * 1024 * 1024 * 1024,
            ..HostState::default()
        };
        assert_eq!(Limits::for_host(&host), Limits { cpus: 4, mem_mb: 4096, per_task: 0 });
        // A small yard: never below one core and half a GiB; never above eight of either.
        let small = HostState { cpus: 2, mem_total: 2 * 1024 * 1024 * 1024, ..HostState::default() };
        assert_eq!(Limits::for_host(&small), Limits { cpus: 1, mem_mb: 512, per_task: 0 });
        let big = HostState {
            cpus: 128,
            mem_total: 512 * 1024 * 1024 * 1024,
            ..HostState::default()
        };
        assert_eq!(Limits::for_host(&big), Limits { cpus: 8, mem_mb: 8192, per_task: 0 });
        // A machine never read: the fallback, not a share of zero cores.
        assert_eq!(Limits::for_host(&HostState::default()), Limits::fallback());
    }

    #[test]
    fn the_limits_of_the_list_file_win_over_the_machines_share() {
        let host = HostState { cpus: 16, mem_total: 32 * 1024 * 1024 * 1024, ..HostState::default() };
        let mut r = Runner {
            id: "vm-r".into(),
            target: "me@192.0.2.29".into(),
            ports_host: String::new(),
            tags: vec![],
            protected: vec![],
            note: String::new(),
            added_at_ms: 0,
            capacity: None,
            auto: AutoClean::default(),
            alerts: Default::default(),
            limits: None,
        };
        assert_eq!(r.effective_limits(Some(&host)).cpus, 4, "no word in the file: the machine's share");
        assert_eq!(r.effective_limits(None), Limits::fallback());
        r.limits = Some(Limits { cpus: 3, mem_mb: 1536, per_task: 2 });
        assert_eq!(r.effective_limits(Some(&host)), Limits { cpus: 3, mem_mb: 1536, per_task: 2 });
        // A human who set zeros meant «no limits», not «use the defaults again».
        r.limits = Some(Limits::default());
        assert_eq!(r.effective_limits(Some(&host)), Limits::default());
    }

    #[test]
    fn a_task_container_without_the_rules_limits_is_marked() {
        let mut state = RunnerState::default();
        state.id = "vm-r".into();
        state.limits = Limits { cpus: 2, mem_mb: 2048, per_task: 0 };
        let mut full = cont("pg-ALP-3", &[]);
        full.id = "1".repeat(64);
        full.class = Class::Active;
        full.task = Some(TaskRef { id: "alpha/3".into(), tag: "ALP-3".into(), ..TaskRef::default() });
        full.cpu_limit = Some(2.0);
        full.mem_limit = Some(2 * 1024 * 1024 * 1024);
        let mut no_cpu = cont("pg-ALP-4", &[]);
        no_cpu.id = "2".repeat(64);
        no_cpu.class = Class::Active;
        no_cpu.task = Some(TaskRef { id: "alpha/4".into(), tag: "ALP-4".into(), ..TaskRef::default() });
        no_cpu.mem_limit = Some(1024 * 1024 * 1024);
        let mut foreign = cont("neighbor-db-1", &[]);
        foreign.id = "3".repeat(64);
        foreign.class = Class::Foreign;
        state.containers = vec![full, no_cpu, foreign];
        apply_limits(&mut state);
        assert!(!state.containers[0].no_limits, "both limits there: no mark");
        assert!(state.containers[1].no_limits, "no CPU limit: marked");
        assert!(!state.containers[2].no_limits, "a foreign container is nobody's business");
        // A rule that asks for nothing marks nothing.
        state.limits = Limits::default();
        apply_limits(&mut state);
        assert!(state.containers.iter().all(|c| !c.no_limits));
    }

    #[test]
    fn a_live_task_over_the_container_maximum_is_named() {
        let mut state = RunnerState::default();
        state.id = "vm-r".into();
        state.limits = Limits { cpus: 2, mem_mb: 2048, per_task: 2 };
        let mk = |id: char, name: &str, task: &str, tag: &str, class: Class| {
            let mut c = cont(name, &[]);
            c.id = id.to_string().repeat(64);
            c.class = class;
            c.task = (class == Class::Active)
                .then(|| TaskRef { id: task.into(), tag: tag.into(), ..TaskRef::default() });
            c
        };
        state.containers = vec![
            mk('1', "pg-ALP-3", "alpha/3", "ALP-3", Class::Active),
            mk('2', "web-ALP-3", "alpha/3", "ALP-3", Class::Active),
            mk('3', "redis-ALP-3", "alpha/3", "ALP-3", Class::Active),
            mk('4', "pg-ALP-4", "alpha/4", "ALP-4", Class::Active),
            mk('5', "web-ALP-1", "alpha/1", "ALP-1", Class::Forgotten),
        ];
        apply_limits(&mut state);
        assert_eq!(state.over.len(), 1);
        assert_eq!(state.over[0].task, "alpha/3");
        assert_eq!(state.over[0].tag, "ALP-3");
        assert_eq!(state.over[0].containers, 3);
        assert_eq!(state.over[0].max, 2);
        // No maximum asked for: nothing is over.
        state.limits.per_task = 0;
        apply_limits(&mut state);
        assert!(state.over.is_empty());
    }

    #[test]
    fn the_machines_share_is_written_once_into_the_list() {
        let mut file = RunnersFile {
            default: "vm-r".into(),
            runners: vec![
                Runner {
                    id: "vm-r".into(),
                    target: "me@192.0.2.29".into(),
                    ports_host: String::new(),
                    tags: vec![],
                    protected: vec![],
                    note: String::new(),
                    added_at_ms: 0,
                    capacity: None,
                    auto: AutoClean::default(),
                    alerts: Default::default(),
                    limits: None,
                },
                Runner {
                    id: "silent".into(),
                    target: "me@192.0.2.30".into(),
                    ports_host: String::new(),
                    tags: vec![],
                    protected: vec![],
                    note: String::new(),
                    added_at_ms: 0,
                    capacity: None,
                    auto: AutoClean::default(),
                    alerts: Default::default(),
                    limits: None,
                },
            ],
            ..RunnersFile::default()
        };
        let mut read = RunnerState::default();
        read.id = "vm-r".into();
        read.ok = true;
        read.host = HostState { cpus: 8, mem_total: 16 * 1024 * 1024 * 1024, ..HostState::default() };
        let mut down = RunnerState::default();
        down.id = "silent".into();
        down.ok = false;
        assert!(fill_limits(&mut file, &[read.clone(), down]));
        assert_eq!(file.runners[0].limits, Some(Limits { cpus: 2, mem_mb: 2048, per_task: 0 }));
        assert_eq!(file.runners[1].limits, None, "a yard that did not answer teaches nothing");
        // A human's own numbers are never overwritten, and a second pass writes nothing.
        assert!(!fill_limits(&mut file, &[read]));
        assert_eq!(file.runners[0].limits, Some(Limits { cpus: 2, mem_mb: 2048, per_task: 0 }));
    }

    #[test]
    fn a_file_round_trips_and_bad_ones_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings/runners.json");
        let file = RunnersFile {
            updated_at_ms: 1,
            default: "vm-r".into(),
            runners: vec![Runner {
                id: "vm-r".into(),
                target: "me@192.0.2.29".into(),
                ports_host: "192.0.2.29".into(),
                tags: vec!["gpu".into()],
                protected: vec!["buildx_*".into()],
                note: String::new(),
                added_at_ms: 1,
                capacity: None,
                auto: AutoClean::default(),
                alerts: Default::default(),
                limits: Some(Limits { cpus: 3, mem_mb: 1536, per_task: 2 }),
            }],
            assign: BTreeMap::new(),
            version: FILE_VERSION,
        };
        save(&path, &file).unwrap();
        assert_eq!(load(&path).unwrap(), file);
        let mut bad = file.clone();
        bad.runners[0].id = "-x".into();
        assert!(save(&path, &bad).is_err());
        bad = file.clone();
        bad.runners[0].target = "a b".into();
        assert!(save(&path, &bad).is_err());
        bad = file.clone();
        bad.runners[0].protected = vec!["a;b".into()];
        assert!(save(&path, &bad).is_err());
        for l in [
            Limits { cpus: 1025, ..Limits::default() },
            Limits { mem_mb: 1024 * 1024 + 1, ..Limits::default() },
            Limits { per_task: 1025, ..Limits::default() },
        ] {
            bad = file.clone();
            bad.runners[0].limits = Some(l);
            assert!(save(&path, &bad).is_err(), "{l:?}");
        }
    }

    #[test]
    fn the_block_names_the_default_yard_and_the_masks() {
        let file = RunnersFile {
            default: "vm-r".into(),
            runners: vec![Runner {
                id: "vm-r".into(),
                target: "me@192.0.2.29".into(),
                ports_host: "192.0.2.29".into(),
                tags: vec![],
                protected: vec!["buildx_*".into(), "neighbor".into()],
                note: String::new(),
                added_at_ms: 0,
                capacity: None,
                auto: AutoClean::default(),
                alerts: Default::default(),
                limits: None,
            }],
            ..RunnersFile::default()
        };
        let block = block_text(&file);
        assert!(block.contains("DOCKER_HOST=ssh://me@192.0.2.29"));
        assert!(block.contains("порты 192.0.2.29") || block.contains("на 192.0.2.29"));
        assert!(block.contains("buildx_*, neighbor") || block.contains("buildx_*, neighbor"));
        // Without a word in the list the rule carries the fallback's numbers (a yard not read yet).
        assert!(block.contains("--cpus 2 --memory 2g"), "{block}");
        assert!(block.contains("docker run -d --name pg-PLN-26 --label herdr.task=PLN-26 --cpus 2 --memory 2g"), "{block}");
        assert!(block.contains("deploy:") && block.contains("cpus: \"2\"") && block.contains("memory: 2g"), "{block}");
        assert!(!block.contains("на задачу"), "no maximum is asked for: none is mentioned");
        let on = connect_value("Мои правила.", &block, true).unwrap().unwrap();
        assert!(on.starts_with("Мои правила.") && on.contains(BLOCK_BEGIN));
        assert_eq!(connect_value(&on, &block, false).unwrap().as_deref(), Some("Мои правила."));
        assert!(has_old_docker_paragraph("Мои правила.\n\nСтавьте DOCKER_HOST=ssh://x сюда.\n"));
    }

    #[test]
    fn the_block_carries_the_limits_of_the_yard() {
        let runner = |id: &str, target: &str, limits: Option<Limits>| Runner {
            id: id.into(),
            target: target.into(),
            ports_host: String::new(),
            tags: vec![],
            protected: vec![],
            note: String::new(),
            added_at_ms: 0,
            capacity: None,
            auto: AutoClean::default(),
            alerts: Default::default(),
            limits,
        };
        let file = RunnersFile {
            default: "vm-r".into(),
            runners: vec![
                runner("vm-r", "me@192.0.2.29", Some(Limits { cpus: 4, mem_mb: 1536, per_task: 2 })),
                // A second yard with limits of its own says them; one with the same numbers does not.
                runner("vm-g", "me@192.0.2.30", Some(Limits { cpus: 1, mem_mb: 1024, per_task: 0 })),
                runner("vm-h", "me@192.0.2.31", Some(Limits { cpus: 4, mem_mb: 1536, per_task: 2 })),
            ],
            ..RunnersFile::default()
        };
        let block = block_text(&file);
        assert!(block.contains("Лимиты контейнера задачи: --cpus 4 --memory 1536m, не больше 2 контейнеров на задачу."), "{block}");
        assert!(block.contains("docker run -d --name pg-PLN-26 --label herdr.task=PLN-26 --cpus 4 --memory 1536m postgres:16"), "{block}");
        assert!(block.contains("cpus: \"4\"") && block.contains("memory: 1536m"), "{block}");
        assert!(block.contains("vm-g (ssh://me@192.0.2.30, порты 192.0.2.30; лимиты: --cpus 1 --memory 1g)"), "{block}");
        assert!(!block.contains("vm-h (ssh://me@192.0.2.31, порты 192.0.2.31; лимиты"), "the same share as the default yard: no words wasted");
        // A yard whose human asked for no limits at all: no paragraph, no mark promise.
        let none = RunnersFile {
            default: "vm-r".into(),
            runners: vec![runner("vm-r", "me@192.0.2.29", Some(Limits::default()))],
            ..RunnersFile::default()
        };
        let block = block_text(&none);
        assert!(!block.contains("--cpus"), "{block}");
        assert!(!block.contains("--memory"), "{block}");
        assert!(!block.contains("Фокусе"), "{block}");
    }

    #[test]
    fn a_block_and_a_hint_with_a_port_give_ssh_a_working_command() {
        let file = RunnersFile {
            default: "vm-r".into(),
            runners: vec![Runner {
                id: "vm-r".into(),
                target: "me@192.0.2.29:2222".into(),
                ports_host: "192.0.2.29".into(),
                tags: vec![],
                protected: vec![],
                note: String::new(),
                added_at_ms: 0,
                capacity: None,
                auto: AutoClean::default(),
                alerts: Default::default(),
                limits: None,
            }],
            ..RunnersFile::default()
        };
        let block = block_text(&file);
        assert!(block.contains("DOCKER_HOST=ssh://me@192.0.2.29:2222"), "{block}");
        assert!(block.contains("ssh -p 2222 -o BatchMode=yes me@192.0.2.29 true"), "{block}");
        assert!(!block.contains("BatchMode=yes me@192.0.2.29:2222"), "{block}");
        assert!(block.contains("открываются на 192.0.2.29"), "{block}");
        let m = MachineRef {
            label: "vm-r".into(),
            target: "me@192.0.2.29:2222".into(),
            session: String::new(),
        };
        let end = ssh::Ended { code: Some(1), stdout: "x".into(), ..Default::default() };
        let w = why(&end, &m);
        assert_eq!(
            w.detail.last().unwrap(),
            "Проверить с этой машины: ssh -p 2222 me@192.0.2.29 docker version"
        );
    }

    #[test]
    fn a_plan_takes_forgotten_containers_and_their_units_only() {
        let mut state = RunnerState::default();
        state.id = "vm-r".into();
        let mut a = cont("pg-ALP-3", &[]);
        a.class = Class::Forgotten;
        a.task = Some(TaskRef {
            tag: "ALP-3".into(),
            ..TaskRef::default()
        });
        a.size_bytes = 1000;
        a.created_ms = now_ms() - 60_000;
        let mut b = cont("web-ALP-1", &[]);
        b.id = "b".repeat(64);
        b.class = Class::Active;
        let active_id = b.id.clone();
        let mut net = Net::default();
        net.name = "work-ALP-3_default".into();
        let mut vol = Vol::default();
        vol.name = "vol-ALP-3".into();
        state.containers = vec![a, b];
        state.networks = vec![net];
        state.volumes = vec![vol];
        let plan = plan_of(&state, None);
        assert_eq!(plan.containers.len(), 1);
        assert_eq!(plan.networks.len(), 1);
        assert_eq!(plan.volumes.len(), 1);
        assert_eq!(plan.was.containers, 2);
        assert_eq!(plan.now.containers, 1);
        assert_eq!(plan.was.bytes, 1000);
        assert!((59_000..=61_000).contains(&plan.containers[0].age_ms), "{}", plan.containers[0].age_ms);
        let only_active = plan_of(&state, Some(&[active_id]));
        assert!(only_active.containers.is_empty());
    }

    #[test]
    fn an_unsure_container_is_in_the_card_unticked_and_an_explicit_tick_takes_it() {
        let mut state = RunnerState::default();
        state.id = "vm-r".into();
        let mut forgotten = cont("web-ALP-1", &[]);
        forgotten.class = Class::Forgotten;
        forgotten.task = Some(TaskRef { tag: "ALP-1".into(), ..TaskRef::default() });
        let mut unsure = cont("x-ALP-99", &[]);
        unsure.id = "c".repeat(64);
        unsure.class = Class::Unsure;
        let mut foreign = cont("neighbor-db-1", &[]);
        foreign.id = "d".repeat(64);
        foreign.class = Class::Foreign;
        state.containers = vec![forgotten, unsure.clone(), foreign];
        let plan = plan_of(&state, None);
        // Both a forgotten and an unsure one are shown; the unsure one is marked so the page leaves it unticked.
        assert_eq!(plan.containers.len(), 2);
        assert_eq!(plan.unsure, vec![unsure.id.clone()]);
        assert_eq!(plan.now.containers, 1);
        // The human ticked it: it goes, alone (its units are unknown); a foreign one never does.
        let ticked = plan_of(&state, Some(&[unsure.id.clone()]));
        assert_eq!(ticked.containers.len(), 1);
        assert_eq!(ticked.containers[0].id, unsure.id);
        assert!(ticked.networks.is_empty() && ticked.volumes.is_empty());
        assert!(plan_of(&state, Some(&["d".repeat(64)])).containers.is_empty());
    }

    #[test]
    fn a_containers_units_belong_to_that_container() {
        let mut one = cont("pg-ALP-3", &[("com.docker.compose.project", "work-3")]);
        one.class = Class::Forgotten;
        one.task = Some(TaskRef { tag: "ALP-3".into(), ..TaskRef::default() });
        let mut two = cont("redis-ALP-4", &[("com.docker.compose.project", "alpha-4")]);
        two.id = "b".repeat(64);
        two.class = Class::Forgotten;
        two.task = Some(TaskRef { tag: "ALP-4".into(), ..TaskRef::default() });
        let mut state = RunnerState::default();
        state.id = "vm-r".into();
        state.containers = vec![one, two];
        state.networks = vec![
            Net { id: "n1".into(), name: "work-3_default".into(), ..Net::default() },
            Net { id: "n2".into(), name: "alpha-4_default".into(), ..Net::default() },
        ];
        state.volumes = vec![
            Vol { name: "work-3_pgdata".into(), ..Vol::default() },
            Vol { name: "alpha-4_redis".into(), ..Vol::default() },
        ];
        let plan = plan_of(&state, None);
        assert_eq!(plan.containers[0].networks, vec!["n1"]);
        assert_eq!(plan.containers[0].volumes, vec!["work-3_pgdata"]);
        assert_eq!(plan.containers[1].networks, vec!["n2"]);
        assert_eq!(plan.containers[1].volumes, vec!["alpha-4_redis"]);
        // A tick on one container brings only its own units into the plan.
        let one = plan_of(&state, Some(&["a".repeat(64)]));
        assert_eq!(one.containers.len(), 1);
        assert_eq!(one.networks.len(), 1);
        assert_eq!(one.networks[0].id, "n1");
        assert_eq!(one.volumes.len(), 1);
        assert_eq!(one.volumes[0].name, "work-3_pgdata");
        assert_eq!(one.now.networks, 1);
        assert_eq!(one.now.containers, 1);
    }

    #[test]
    fn a_tag_matches_a_unit_name_only_on_segment_borders() {
        let tags: BTreeSet<String> = ["ALP-3".to_string()].into_iter().collect();
        let projects: BTreeSet<String> = ["work-3".to_string()].into_iter().collect();
        assert!(network_matches("work-ALP-3_default", &tags, &projects));
        assert!(network_matches("work-3_default", &tags, &projects));
        assert!(volume_matches("vol-ALP-3", &tags, &projects));
        assert!(volume_matches("work-3_pgdata", &tags, &projects));
        // A neighbour's tag or project that merely starts the same is not ours.
        assert!(!network_matches("alp-30_default", &tags, &projects));
        assert!(!volume_matches("vol-ALP-30", &tags, &projects));
        assert!(!volume_matches("work-30_pgdata", &tags, &projects));
    }

    #[test]
    fn docker_times_are_read() {
        assert_eq!(parse_docker_time("1970-01-01 00:00:00 +0000 UTC"), Some(0));
        assert_eq!(
            parse_docker_time("2026-10-22 09:00:00 +0000 UTC"),
            Some(1792659600000)
        );
        assert_eq!(parse_bytes("96MiB"), Some(96 * 1024 * 1024));
        assert_eq!(parse_bytes("1.5GB"), Some(1_500_000_000));
        // `docker ps` with `--size` appends the virtual size of the image.
        assert_eq!(parse_bytes("182kB (virtual 9.74GB)"), Some(182_000));
        assert_eq!(host_of_target("me@192.0.2.29:2222"), "192.0.2.29");
    }

    #[test]
    fn a_published_port_carries_its_service_and_a_web_link() {
        let p = published_ports("0.0.0.0:15432->5432/tcp", "postgres:16");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].host_port, Some(15432));
        assert_eq!(p[0].container_port, Some(5432));
        assert_eq!(p[0].proto, "tcp");
        assert_eq!(p[0].service, Some(Service::Postgres));
        assert!(!p[0].http && !p[0].loopback);
        // A custom image still runs a database on its usual port.
        let p = published_ports("0.0.0.0:16379->6379/tcp", "acme/stand:7");
        assert_eq!(p[0].service, Some(Service::Redis));
        // A web port of an all-interface binding is a link; 443 is https.
        let p = published_ports("0.0.0.0:18080->8080/tcp", "acme/app:1");
        assert!(p[0].http && !p[0].https && p[0].service.is_none());
        let p = published_ports("0.0.0.0:18443->443/tcp", "acme/app:1");
        assert!(p[0].http && p[0].https);
        // 8443 is a TLS port too (Kong's https listener): a plain http link would not answer.
        let p = published_ports("0.0.0.0:18443->8443/tcp", "kong:3");
        assert!(p[0].http && p[0].https);
        // A port bound to one interface of the yard is reachable at that very address, not only at an any-address.
        let p = published_ports("10.31.21.29:18080->80/tcp", "acme/app:1");
        assert!(p[0].http && !p[0].loopback);
        assert_eq!(p[0].host_ip, "10.31.21.29");
        let p = published_ports("[fd00::1]:18080->80/tcp", "acme/app:1");
        assert!(p[0].http);
        // A web server on a port that says nothing still serves HTTP; udp never is.
        let p = published_ports("0.0.0.0:9999->9999/tcp", "nginx:1.27");
        assert!(p[0].http);
        let p = published_ports("0.0.0.0:8080->8080/udp", "acme/app:1");
        assert!(!p[0].http);
        // A database image can publish a browser UI: its web port is a link, not a client command.
        let p = published_ports("0.0.0.0:18001->8001/tcp", "redis/redis-stack:latest");
        assert_eq!(p[0].service, None);
        assert!(p[0].http);
    }

    #[test]
    fn a_port_bound_twice_is_one_entry_and_a_loopback_one_says_so() {
        let p = published_ports("0.0.0.0:18080->80/tcp, :::18080->80/tcp", "nginx:1.27");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].host_port, Some(18080));
        // The public binding wins over the loopback one, in either order.
        let p = published_ports("127.0.0.1:18080->80/tcp, 0.0.0.0:18080->80/tcp", "nginx:1.27");
        assert_eq!(p.len(), 1);
        assert!(!p[0].loopback);
        assert_eq!(p[0].host_ip, "0.0.0.0");
        let p = published_ports("127.0.0.1:15434->5432/tcp", "postgres:16");
        assert!(p[0].loopback && !p[0].http);
        let p = published_ports("[::1]:18080->80/tcp", "nginx:1.27");
        assert!(p[0].loopback && !p[0].http);
        // A port with no host side is not published; a range has no single port to link.
        assert!(published_ports("5432/tcp", "postgres:16").is_empty());
        let p = published_ports("0.0.0.0:8000-8010->8000-8010/tcp", "acme/app:1");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].host_port, None);
        assert_eq!(p[0].container_port, None);
    }

    #[test]
    fn the_container_takes_its_ports_from_the_probe() {
        let v: Value = serde_json::from_str(
            r#"{"ID":"a","Names":"pg-ALP-3","Image":"postgres:16","Ports":"0.0.0.0:15432->5432/tcp"}"#,
        )
        .unwrap();
        let c = container_of(&v);
        assert_eq!(c.published.len(), 1);
        assert_eq!(c.published[0].host_port, Some(15432));
        // An older saved read has the text only; the page falls back to it.
        let old: Container = serde_json::from_str(
            r#"{"id":"a","name":"n","image":"postgres:16","state":"running","status":"","created_ms":0,
                "labels":{},"ports":"0.0.0.0:15432->5432/tcp","networks":"","mounts":"","size":"",
                "size_bytes":0,"cpu_pct":null,"mem_bytes":null,"mem_limit":null,"pids":null,"net_io":"",
                "block_io":"","gpu_mem_mib":null,"task":null,"class":"active"}"#,
        )
        .unwrap();
        assert!(old.published.is_empty());
    }
}
