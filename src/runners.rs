//! Shipyards («верфи»): machines that run the docker containers of the board's tasks (PLN-26).
//!
//! A runner is not a galaxy: it has no board, no herdr and no agents. Every galaxy's agents reach it with
//! `DOCKER_HOST=ssh://…`; the planner reads its state (one fixed ssh script), matches its containers to the tasks of
//! all galaxies and removes the forgotten ones by exact id, after a «было → стало» card.
//!
//! The source of truth is `~/.herdr-projects/settings/runners.json` (the same path on every machine); the planner
//! mirrors it to the far galaxies and keeps the generated docker rule in their `worker.instructions` and
//! `reviewer.instructions`.

use crate::app::Shared;
use crate::board::Task;
use crate::config::{now_ms, AppConfig};
use crate::machines::{self, MachineRef, Why};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

pub const FILE_VERSION: u32 = 1;
pub const PROBE_WAIT: Duration = Duration::from_secs(25);
pub const DF_WAIT: Duration = Duration::from_secs(120);
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
        if !machines::valid_label(&r.id) {
            return Err(format!("runners.json: недопустимая метка верфи: {:?}", r.id));
        }
        if !ids.insert(r.id.clone()) {
            return Err(format!("runners.json: метка верфи повторяется: {}", r.id));
        }
        if !machines::valid_target(&r.target) {
            return Err(format!("runners.json: недопустимый адрес верфи {}: {:?}", r.id, r.target));
        }
        if !r.ports_host.is_empty() && !machines::valid_target(&r.ports_host) {
            return Err(format!("runners.json: недопустимый адрес портов верфи {}: {:?}", r.id, r.ports_host));
        }
        for m in &r.protected {
            if !valid_mask(m) {
                return Err(format!("runners.json: маска защиты {:?} у верфи {}", m, r.id));
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
echo '== net';   docker network ls --format '{{json .}}'
echo '== vol';   docker volume ls --format '{{json .}}'"##;

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
    /// A tag whose task is not found (removed, or its galaxy is away), or one tag in two galaxies.
    Unsure,
    /// Not a board task, or under a protected mask: never touched.
    Foreign,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskRef {
    pub galaxy: String,
    pub id: String,
    pub tag: String,
    pub status: String,
    pub title: String,
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
    pub networks: String,
    pub mounts: String,
    pub size: String,
    pub size_bytes: u64,
    pub cpu_pct: Option<f64>,
    pub mem_bytes: Option<u64>,
    pub mem_limit: Option<u64>,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mirror {
    pub galaxy: String,
    pub ok: bool,
    pub at: u64,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BlockState {
    /// Empty for this machine.
    pub galaxy: String,
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
    #[serde(default)]
    pub mirrors: Vec<Mirror>,
}

impl RunnerState {
    pub fn from_runner(r: &Runner) -> Self {
        Self {
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
            ..Self::default()
        }
    }
}

pub fn parse_probe(out: &str) -> Result<(HostState, Vec<Container>, Vec<Net>, Vec<Vol>), String> {
    if !out.lines().any(|l| l.contains("herdr-planner-runner 1")) {
        return Err("машина ответила не тем, что ждёт планировщик: нет отметки опроса".into());
    }
    let mut host = HostState::default();
    let mut containers: Vec<Container> = Vec::new();
    let mut stats: Vec<Value> = Vec::new();
    let mut nets: Vec<Net> = Vec::new();
    let mut vols: Vec<Vol> = Vec::new();
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
    Ok((host, containers, nets, vols))
}

fn s_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn labels_of(s: &str) -> BTreeMap<String, String> {
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

fn container_of(v: &Value) -> Container {
    let created = s_of(v, "CreatedAt");
    let size = s_of(v, "Size");
    Container {
        id: s_of(v, "ID"),
        name: s_of(v, "Names"),
        image: s_of(v, "Image"),
        state: s_of(v, "State"),
        status: s_of(v, "Status"),
        created_ms: parse_docker_time(&created).unwrap_or(0),
        labels: labels_of(&s_of(v, "Labels")),
        ports: s_of(v, "Ports"),
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
    let s = s.trim();
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
    galaxy: String,
    id: String,
    tag: String,
    status: String,
    archived: bool,
    title: String,
}

#[derive(Debug, Clone, Default)]
pub struct TaskIndex {
    tags: BTreeMap<String, Vec<Cand>>,
    keys: BTreeSet<String>,
    /// hostname (lower case) → galaxy label.
    hosts: HashMap<String, String>,
}

impl TaskIndex {
    pub fn build(
        active: &[Task],
        archived: &[Task],
        repos: &[crate::taskmeta::RepoInfo],
        galaxies: &[machines::Galaxy],
        cached: &[Value],
    ) -> TaskIndex {
        let mut idx = TaskIndex::default();
        for r in repos {
            if !r.key.is_empty() {
                idx.keys.insert(r.key.to_uppercase());
            }
        }
        for t in active.iter().chain(archived.iter()) {
            idx.local_task(t);
        }
        for g in galaxies {
            if !g.host.is_empty() {
                idx.hosts.insert(g.host.to_lowercase(), g.label.clone());
            }
            idx.hosts.entry(g.label.to_lowercase()).or_insert(g.label.clone());
            for p in &g.projects {
                if !p.key.is_empty() {
                    idx.keys.insert(p.key.to_uppercase());
                }
            }
            for t in &g.tasks {
                idx.remote_task(
                    &g.label,
                    &t.id,
                    &t.tag,
                    &t.key,
                    t.n,
                    t.status.key(),
                    t.archived,
                    &t.title,
                );
            }
        }
        for v in cached {
            let label = v.get("label").and_then(Value::as_str).unwrap_or("");
            if label.is_empty() {
                continue;
            }
            if let Some(h) = v.get("host").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                idx.hosts.insert(h.to_lowercase(), label.to_string());
            }
            idx.hosts.entry(label.to_lowercase()).or_insert(label.to_string());
            if let Some(ps) = v.get("projects").and_then(Value::as_array) {
                for p in ps {
                    if let Some(k) = p.get("key").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                        idx.keys.insert(k.to_uppercase());
                    }
                }
            }
            if let Some(ts) = v.get("tasks").and_then(Value::as_array) {
                for t in ts {
                    let id = t.get("id").and_then(Value::as_str).unwrap_or("");
                    let tag = t.get("tag").and_then(Value::as_str).unwrap_or("");
                    let key = t.get("key").and_then(Value::as_str).unwrap_or("");
                    let n = t.get("n").and_then(Value::as_u64).unwrap_or(0);
                    let status = t.get("status").and_then(Value::as_str).unwrap_or("");
                    let archived = t.get("archived").and_then(Value::as_bool).unwrap_or(false);
                    let title = t.get("title").and_then(Value::as_str).unwrap_or("");
                    idx.remote_task(label, id, tag, key, n, status, archived, title);
                }
            }
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
        self.tags.entry(tag.clone()).or_default().push(Cand {
            galaxy: String::new(),
            id: t.id.clone(),
            tag,
            status: t.status.key().to_string(),
            archived: t.archived,
            title: t.title.clone(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn remote_task(
        &mut self,
        galaxy: &str,
        id: &str,
        tag: &str,
        key: &str,
        n: u64,
        status: &str,
        archived: bool,
        title: &str,
    ) {
        if id.is_empty() {
            return;
        }
        let key = tag_key(tag)
            .or_else(|| {
                let k = key.to_uppercase();
                (!k.is_empty()).then_some(k)
            })
            .unwrap_or_default();
        if key.is_empty() {
            return;
        }
        self.keys.insert(key.clone());
        let tag = if tag.is_empty() {
            format!("{key}-{n}")
        } else {
            tag.to_uppercase()
        };
        self.tags.entry(tag.clone()).or_default().push(Cand {
            galaxy: galaxy.to_string(),
            id: id.to_string(),
            tag,
            status: status.to_string(),
            archived,
            title: title.to_string(),
        });
    }

    fn known_key(&self, key: &str) -> bool {
        self.keys.contains(&key.to_uppercase())
    }

    fn galaxy_of_host(&self, host: &str) -> Option<String> {
        self.hosts.get(&host.to_lowercase()).cloned()
    }
}

fn tag_key(tag: &str) -> Option<String> {
    let (key, n) = tag.rsplit_once('-')?;
    if key.is_empty() || n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(key.to_uppercase())
}

fn segments(name: &str) -> Vec<&str> {
    name.split(|c| matches!(c, '-' | '_' | '.')).collect()
}

/// The first `KEY-n` in a name whose key is known to some galaxy.
fn tag_in_name(name: &str, index: &TaskIndex) -> Option<String> {
    let seg = segments(name);
    let mut i = 0;
    while i + 1 < seg.len() {
        let key = seg[i];
        let num = seg[i + 1];
        if (2..=6).contains(&key.len())
            && key.chars().all(|c| c.is_ascii_alphabetic())
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
    if !(1..=6).contains(&n.len()) || !n.chars().all(|c| c.is_ascii_digit()) {
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

fn is_protected(c: &Container, masks: &[String]) -> bool {
    let project = c
        .labels
        .get("com.docker.compose.project")
        .cloned()
        .unwrap_or_default();
    masks
        .iter()
        .any(|m| mask_match(m, &c.name) || (!project.is_empty() && mask_match(m, &project)))
}

/// Fills `task` and `class` of every container. `herdr.task` is the rule; a tag in the name (its key known to a
/// galaxy) is the fallback; compose containers are read by their project, never by their generated name.
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
    let mut uniq: BTreeMap<(String, String), &Cand> = BTreeMap::new();
    for cand in cands {
        uniq.insert((cand.galaxy.clone(), cand.id.clone()), cand);
    }
    let list: Vec<&Cand> = uniq.into_values().collect();
    let host_label = c.labels.get("herdr.galaxy").map(String::as_str).unwrap_or("");
    let chosen: Option<&Cand> = if list.len() == 1 {
        list.first().copied()
    } else if !host_label.is_empty() {
        let galaxy = index.galaxy_of_host(host_label);
        match galaxy {
            Some(g) => {
                let hits: Vec<&Cand> = list.iter().copied().filter(|c| c.galaxy == g).collect();
                (hits.len() == 1).then(|| hits[0])
            }
            None => None,
        }
    } else {
        None
    };
    let Some(cand) = chosen else {
        return (Class::Unsure, None);
    };
    let done = cand.archived || cand.status == "done";
    let class = if done { Class::Forgotten } else { Class::Active };
    (
        class,
        Some(TaskRef {
            galaxy: cand.galaxy.clone(),
            id: cand.id.clone(),
            tag: cand.tag.clone(),
            status: cand.status.clone(),
            title: cand.title.clone(),
        }),
    )
}

/* ───────── polling ───────── */

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
    let end = match machines::ssh_ended(cfg, &m, PROBE_SCRIPT, PROBE_WAIT).await {
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
        state.why = Some(machines::why_not(&end, &m));
        state.fetched_at_ms = now_ms();
        return state;
    }
    match parse_probe(&end.stdout) {
        Ok((host, containers, networks, volumes)) => {
            state.ok = true;
            state.host = host;
            state.containers = containers;
            state.networks = networks;
            state.volumes = volumes;
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
    for state in &states {
        if let Ok(json) = serde_json::to_string(state) {
            app.db.runner_put(
                &state.id,
                &json,
                state.ok,
                state
                    .why
                    .as_ref()
                    .map(|w| w.line.as_str())
                    .unwrap_or(""),
            );
        }
        old.retain(|s| s.id != state.id);
    }
    old.extend(states);
    old.sort_by(|a, b| a.id.cmp(&b.id));
    *app.runners.lock().await = old;
    crate::app::rebuild_extras(app).await;
    crate::app::publish(app).await;
    crate::app::index_snapshot(app).await;
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
    }
}

fn network_matches(name: &str, tags: &BTreeSet<String>, projects: &BTreeSet<String>) -> bool {
    let upper = name.to_uppercase();
    tags.iter().any(|t| upper.contains(t))
        || projects
            .iter()
            .any(|p| name == p || name.starts_with(&format!("{p}_")))
}

fn volume_matches(name: &str, tags: &BTreeSet<String>, projects: &BTreeSet<String>) -> bool {
    let upper = name.to_uppercase();
    tags.iter().any(|t| upper.contains(t)) || projects.iter().any(|p| name.starts_with(p))
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
    let mut tags: BTreeSet<String> = BTreeSet::new();
    let mut projects: BTreeSet<String> = BTreeSet::new();
    let mut picked: Vec<&Container> = Vec::new();
    let mut unsure: Vec<String> = Vec::new();
    for c in &state.containers {
        let asked = ids.is_some_and(|list| list.iter().any(|id| id == &c.id));
        let wanted = match ids {
            Some(_) => asked,
            None => c.class == Class::Forgotten,
        };
        if !wanted {
            if ids.is_none() && c.class == Class::Unsure {
                unsure.push(c.id.clone());
            }
            continue;
        }
        match c.class {
            Class::Forgotten => {}
            Class::Unsure => unsure.push(c.id.clone()),
            Class::Active | Class::Foreign => continue,
        }
        if let Some(t) = &c.task {
            tags.insert(t.tag.to_uppercase());
        }
        if let Some(p) = c.labels.get("com.docker.compose.project") {
            projects.insert(p.clone());
        }
        picked.push(c);
    }
    let containers: Vec<PlanItem> = picked.iter().map(|c| plan_item(c, now)).collect();
    let bytes = picked.iter().map(|c| c.size_bytes).sum();
    let networks: Vec<PlanItem> = state
        .networks
        .iter()
        .filter(|n| network_matches(&n.name, &tags, &projects))
        .map(|n| PlanItem {
            id: n.id.clone(),
            name: n.name.clone(),
            ..PlanItem::default()
        })
        .collect();
    let mut vol_names: BTreeSet<String> = state
        .volumes
        .iter()
        .filter(|v| volume_matches(&v.name, &tags, &projects))
        .map(|v| v.name.clone())
        .collect();
    for c in &picked {
        for m in mounts_of(c) {
            vol_names.insert(m);
        }
    }
    let volumes: Vec<PlanItem> = vol_names
        .into_iter()
        .map(|name| PlanItem {
            name,
            ..PlanItem::default()
        })
        .collect();
    let was = Counts {
        containers: state.containers.len(),
        networks: state.networks.len(),
        volumes: state.volumes.len(),
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

/// The docker rule for the board's agents. The same text on every galaxy: the machine's name is taken from
/// `hostname` by the agent, so a task sent to another machine still works.
pub fn block_text(file: &RunnersFile) -> String {
    let Some(def) = file.default_runner() else {
        return String::new();
    };
    let ports = if def.ports_host.is_empty() {
        host_of_target(&def.target)
    } else {
        def.ports_host.clone()
    };
    let mut s = String::from(BLOCK_BEGIN);
    s.push('\n');
    s.push_str(&format!(
        "Docker-контейнеры (базы, стенды, заглушки, прокси и т.п.) не запускай на этой машине: используй верфь {} — export DOCKER_HOST=ssh://{} (вход по SSH-ключу уже настроен; проверка: ssh -o BatchMode=yes {} true). Порты контейнеров открываются на {}, не на localhost.\n",
        def.id, def.target, def.target, ports
    ));
    s.push_str("Каждому контейнеру, сети и тому давай имя с id своей задачи (например pg-PLN-26) и метки --label herdr.task=PLN-26 --label herdr.galaxy=$(hostname); для docker compose — имя проекта с id задачи (docker compose -p pln-26 …) и те же метки в labels каждого сервиса; после себя всегда останавливай и удаляй их (docker rm -f -v, docker network rm, docker volume rm, docker compose -p pln-26 down -v), даже если работа прервалась.\n");
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
                let p = if r.ports_host.is_empty() {
                    host_of_target(&r.target)
                } else {
                    r.ports_host.clone()
                };
                format!("{} (ssh://{}, порты {})", r.id, r.target, p)
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

/* ───────── mirrors ───────── */

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The far-side command: the file written atomically (base64 keeps the text intact), then its hash printed.
pub fn mirror_script(content: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
    format!(
        "mkdir -p ~/.herdr-projects/settings && printf %s '{}' | base64 -d > ~/.herdr-projects/settings/runners.json.tmp && chmod 600 ~/.herdr-projects/settings/runners.json.tmp && mv ~/.herdr-projects/settings/runners.json.tmp ~/.herdr-projects/settings/runners.json && sha256sum ~/.herdr-projects/settings/runners.json",
        b64
    )
}

/// Writes the list to every galaxy that is on line (one ssh call each), remembering the hash that arrived.
pub async fn mirror_all(app: &Shared) {
    let path = app.cfg.runners_file();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => serde_json::to_string_pretty(&RunnersFile::default()).unwrap_or_default(),
    };
    let hash = sha256_hex(&content);
    let script = mirror_script(&content);
    let galaxies = app.galaxies.lock().await.clone();
    let calls = galaxies.iter().map(|g| {
        let script = script.clone();
        async move {
            if !g.ok || g.target.is_empty() {
                return (g.label.clone(), false, String::new(), now_ms(), "нет связи".to_string());
            }
            let m = MachineRef {
                label: g.label.clone(),
                target: g.target.clone(),
                session: String::new(),
            };
            let end = machines::ssh_ended(&app.cfg, &m, &script, PROBE_WAIT).await;
            let (ok, got, why) = match end {
                Ok(e) if !e.timed_out && e.error.is_empty() && e.code == Some(0) => {
                    let got = e.stdout.split_whitespace().next().unwrap_or("").to_string();
                    (true, got, String::new())
                }
                Ok(e) => (false, String::new(), machines::why_not(&e, &m).line),
                Err(e) => (false, String::new(), e),
            };
            (g.label.clone(), ok, got, now_ms(), why)
        }
    });
    let done = futures_util::future::join_all(calls).await;
    for (galaxy, ok, got, at, why) in done {
        app.db.mirror_put(
            &galaxy,
            if got.is_empty() { &hash } else { &got },
            at,
            ok,
            &why,
        );
    }
    crate::app::rebuild_extras(app).await;
}

/// Rewrites the docker block in every galaxy that had it (the consent was given when it was connected).
pub async fn sync_blocks(app: &Shared) {
    // Nothing was ever connected: there is no block to update (and no board to ask).
    if !app.runner_blocks.lock().await.iter().any(|b| b.connected) {
        return;
    }
    let file = match load(&app.cfg.runners_file()) {
        Ok(f) => f,
        Err(_) => return,
    };
    let block = block_text(&file);
    let known = app.runner_blocks.lock().await.clone();
    let mut out: Vec<BlockState> = Vec::new();
    // This machine first.
    let mut places: Vec<(String, crate::station::Place)> = vec![(
        String::new(),
        crate::station::Place::Home,
    )];
    for g in app.galaxies.lock().await.iter() {
        if let Ok(place) = crate::station::place_of(&g.label).await {
            places.push((g.label.clone(), place));
        }
    }
    for (label, place) in places {
        let was_connected = known
            .iter()
            .find(|b| b.galaxy == label)
            .map(|b| b.connected)
            .unwrap_or(false);
        let settings = match crate::station::read_settings(&app.cfg, &place, "").await {
            Ok(s) => s,
            Err(e) => {
                out.push(BlockState {
                    galaxy: label,
                    connected: was_connected,
                    stale: was_connected,
                    at: now_ms(),
                    why: e.message(),
                });
                continue;
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
            if !was_connected {
                continue;
            }
            let value = match connect_value(&current, &block, !block.is_empty()) {
                Ok(Some(v)) => v,
                Ok(None) => {
                    let args = vec![
                        "unset".to_string(),
                        name.to_string(),
                    ];
                    if let Err(e) = crate::station::board_run(
                        &app.cfg,
                        &place,
                        &args,
                        crate::station::BOARD_WAIT,
                    )
                    .await
                    {
                        why = e.message();
                        continue;
                    }
                    changed = true;
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
            match crate::station::board_run(&app.cfg, &place, &args, crate::station::BOARD_WAIT)
                .await
            {
                Ok(o) if o.ok() => changed = true,
                Ok(o) => why = o.words(),
                Err(e) => why = e.message(),
            }
        }
        if changed {
            stale = false;
        }
        out.push(BlockState {
            galaxy: label,
            connected,
            stale,
            at: now_ms(),
            why,
        });
    }
    *app.runner_blocks.lock().await = out.clone();
    if let Ok(json) = serde_json::to_string(&out) {
        app.db.meta_set("runner_blocks", &json);
    }
    crate::app::rebuild_extras(app).await;
    crate::app::publish(app).await;
}

/* ───────── docker system df ───────── */

pub async fn disk_use(cfg: &AppConfig, r: &Runner) -> Result<Value, String> {
    let m = machine_of(r);
    let end = machines::ssh_ended(
        cfg,
        &m,
        "docker system df --format '{{json .}}'",
        DF_WAIT,
    )
    .await?;
    if end.timed_out || !end.error.is_empty() || end.code != Some(0) {
        return Err(machines::why_not(&end, &m).line);
    }
    let mut rows = Vec::new();
    for line in end.stdout.lines().filter(|l| l.trim().starts_with('{')) {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            rows.push(v);
        }
    }
    Ok(json!({ "ok": true, "rows": rows }))
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

    #[test]
    fn the_name_parser_takes_only_keys_a_galaxy_knows() {
        let mut idx = TaskIndex::default();
        idx.keys.insert("ALP".into());
        idx.keys.insert("PLN".into());
        assert_eq!(tag_in_name("pg-ALP-3", &idx).as_deref(), Some("ALP-3"));
        assert_eq!(tag_in_name("alp-3-pg-1", &idx).as_deref(), Some("ALP-3"));
        assert_eq!(tag_in_name("pg-PLN-26", &idx).as_deref(), Some("PLN-26"));
        assert_eq!(tag_in_name("myapp-db-1", &idx), None);
        assert_eq!(tag_in_name("neighbor-server-x-app-1", &idx), None);
        assert_eq!(tag_in_name("neighbor-db-1", &idx), None);
    }

    #[test]
    fn the_four_classes_follow_the_snapshot() {
        let active = vec![task("alpha/3", "ALP-3", Status::Working, false)];
        let archived = vec![task("alpha/1", "ALP-1", Status::Done, true)];
        let idx = TaskIndex::build(&active, &archived, &[], &[], &[]);
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
    fn compose_containers_are_read_by_their_project_not_their_name() {
        let active = vec![task("alpha/3", "ALP-3", Status::Working, false)];
        let idx = TaskIndex::build(&active, &[], &[], &[], &[]);
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
    fn the_label_wins_and_an_unknown_key_is_foreign() {
        let active = vec![task("alpha/3", "ALP-3", Status::Working, false)];
        let idx = TaskIndex::build(&active, &[], &[], &[], &[]);
        let mut list = vec![
            cont("anything", &[("herdr.task", "ALP-3")]),
            cont("pg-ALP-3", &[("herdr.task", "ZZZ-9")]),
        ];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Active);
        assert_eq!(list[1].class, Class::Foreign);
    }

    #[test]
    fn the_same_tag_in_two_galaxies_needs_the_host_label() {
        let mut gal = machines::Galaxy::default();
        gal.label = "vm-a".into();
        gal.host = "orion".into();
        gal.tasks = vec![task("alpha/3", "ALP-3", Status::Working, false)];
        let local = vec![task("alpha/7", "ALP-3", Status::Working, false)];
        let idx = TaskIndex::build(&local, &[], &[], &[gal], &[]);
        let mut list = vec![
            cont("pg-ALP-3", &[]),
            cont("pg-ALP-3", &[("herdr.galaxy", "vm-a")]),
            cont("pg-ALP-3", &[("herdr.galaxy", "elsewhere")]),
        ];
        classify(&mut list, &[], &idx);
        assert_eq!(list[0].class, Class::Unsure);
        assert_eq!(list[1].class, Class::Active);
        assert_eq!(list[1].task.as_ref().unwrap().galaxy, "vm-a");
        assert_eq!(list[2].class, Class::Unsure);
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
             == net\n\
             {{\"ID\":\"net1\",\"Name\":\"work-3_default\",\"Driver\":\"bridge\",\"Labels\":\"\"}}\n\
             == vol\n\
             {{\"Name\":\"work-3_pgdata\",\"Driver\":\"local\",\"Labels\":\"\"}}\n",
            "a".repeat(64)
        );
        let (host, containers, nets, vols) = parse_probe(&out).unwrap();
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
        assert_eq!(c.gpu_mem_mib, Some(1337));
        assert_eq!(c.created_ms, 1792659600000);
        assert_eq!(nets[0].name, "work-3_default");
        assert_eq!(vols[0].name, "work-3_pgdata");
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
            }],
            ..RunnersFile::default()
        };
        let block = block_text(&file);
        assert!(block.contains("DOCKER_HOST=ssh://me@192.0.2.29"));
        assert!(block.contains("порты 192.0.2.29") || block.contains("на 192.0.2.29"));
        assert!(block.contains("buildx_*, neighbor") || block.contains("buildx_*, neighbor"));
        let on = connect_value("Мои правила.", &block, true).unwrap().unwrap();
        assert!(on.starts_with("Мои правила.") && on.contains(BLOCK_BEGIN));
        assert_eq!(connect_value(&on, &block, false).unwrap().as_deref(), Some("Мои правила."));
        assert!(has_old_docker_paragraph("Мои правила.\n\nСтавьте DOCKER_HOST=ssh://x сюда.\n"));
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
        assert_eq!(plan.containers[0].age_ms, 60_000);
        let only_active = plan_of(&state, Some(&[active_id]));
        assert!(only_active.containers.is_empty());
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
        assert_eq!(host_of_target("me@192.0.2.29:2222"), "192.0.2.29");
    }
}
