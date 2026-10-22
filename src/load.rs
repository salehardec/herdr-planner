//! How loaded this machine is: CPU, memory and video cards, for the bars in the top of the page.
//!
//! Cheap by construction: the CPU and memory come from `/proc` (two small file reads), the cards from
//! `nvidia-smi` (one short process) or from the `amdgpu` counters in `/sys` when there is no NVIDIA driver.
//! The loop sleeps while nobody has the page open. The numbers are the machine's own; nothing leaves it.
//!
//! The checks replace the machine: `HERDR_PLANNER_LOAD_FILE` names a JSON file that is read instead of
//! `/proc` (a missing file means the real machine; `{"ok": false}` is a machine that cannot be read),
//! and `HERDR_PLANNER_LOAD_MS` shortens the interval. `nvidia-smi` is found through `PATH` (or
//! `HERDR_PLANNER_NVIDIA_SMI`), so a stand-in on the `PATH` plays the driver.

use crate::app::Shared;
use crate::config::now_ms;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// How often the machine is looked at (milliseconds).
pub const EVERY_MS: u64 = 3000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Load {
    /// When the numbers were read (unix ms); 0 before the first reading.
    #[serde(default)]
    pub at: u64,
    /// False when the machine could not be read at all (then `error` says why and the bars are grey).
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: String,
    /// The interval between readings, so the page can tell a stale one.
    #[serde(default)]
    pub every_ms: u64,
    #[serde(default)]
    pub cpu: Cpu,
    #[serde(default)]
    pub mem: Mem,
    /// Empty when the machine has no video card (or no way to read one).
    #[serde(default)]
    pub gpus: Vec<Gpu>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Cpu {
    /// Busy share of all cores since the previous reading, 0..100.
    #[serde(default)]
    pub percent: f64,
    /// Logical cores.
    #[serde(default)]
    pub cores: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Mem {
    /// Bytes in use (what is not available for a new program).
    #[serde(default)]
    pub used: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Gpu {
    #[serde(default)]
    pub name: String,
    /// Busy share of the card, 0..100.
    #[serde(default)]
    pub util: f64,
    /// Video memory in bytes.
    #[serde(default)]
    pub mem_used: u64,
    #[serde(default)]
    pub mem_total: u64,
}

/// What `/proc/stat` says about the processor: the time spent busy and the time in total, in ticks, and the core count.
pub fn parse_stat(text: &str) -> Option<(u64, u64, u32)> {
    let mut total = None;
    let mut cores = 0u32;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let Some(name) = it.next() else { continue };
        if name == "cpu" {
            let v: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
            if v.len() >= 4 {
                // user nice system idle iowait irq softirq steal …: guest time is already inside user/nice
                let idle = v[3] + v.get(4).copied().unwrap_or(0);
                let sum: u64 = v.iter().take(8).sum();
                total = Some((sum.saturating_sub(idle), sum));
            }
        } else if name.len() > 3 && name.starts_with("cpu") && name[3..].bytes().all(|b| b.is_ascii_digit()) {
            cores += 1;
        }
    }
    total.map(|(busy, all)| (busy, all, cores.max(1)))
}

/// Busy share between two readings of [`parse_stat`], 0..100 (0 when no time has passed).
pub fn cpu_percent(prev: (u64, u64), now: (u64, u64)) -> f64 {
    let all = now.1.saturating_sub(prev.1);
    if all == 0 {
        return 0.0;
    }
    let busy = now.0.saturating_sub(prev.0);
    (busy as f64 / all as f64 * 100.0).clamp(0.0, 100.0)
}

/// `/proc/meminfo`: bytes in use (total minus what is available) and in all.
pub fn parse_meminfo(text: &str) -> Option<Mem> {
    let kb = |key: &str| -> Option<u64> {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.trim_start_matches(':').split_whitespace().next()?.parse::<u64>().ok())
    };
    let total = kb("MemTotal")? * 1024;
    if total == 0 {
        return None;
    }
    let free = kb("MemAvailable").or_else(|| Some(kb("MemFree")? + kb("Buffers").unwrap_or(0) + kb("Cached").unwrap_or(0)))? * 1024;
    let used = total.saturating_sub(free);
    Some(Mem { used, total, percent: (used as f64 / total as f64 * 100.0).clamp(0.0, 100.0) })
}

/// `nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total --format=csv,noheader,nounits`
/// (memory in MiB). A line it cannot read is skipped; `[N/A]` is a number it does not know.
pub fn parse_nvidia(text: &str) -> Vec<Gpu> {
    let mib = |s: &str| s.trim().parse::<f64>().map(|v| (v.max(0.0) * 1048576.0) as u64).unwrap_or(0);
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').map(str::trim).collect();
            if f.len() < 4 || f[0].is_empty() {
                return None;
            }
            // a name may hold a comma: the last three fields are the numbers
            let n = f.len();
            let name = f[..n - 3].join(", ");
            Some(Gpu {
                name,
                util: f[n - 3].parse::<f64>().unwrap_or(0.0).clamp(0.0, 100.0),
                mem_used: mib(f[n - 2]),
                mem_total: mib(f[n - 1]),
            })
        })
        .collect()
}

/// The `amdgpu` cards in `/sys/class/drm` (their busy share and video memory); the folder is a parameter for the tests.
pub fn read_amd(drm: &Path) -> Vec<Gpu> {
    let Ok(rd) = fs::read_dir(drm) else { return Vec::new() };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.len() > 4 && n.starts_with("card") && n[4..].bytes().all(|b| b.is_ascii_digit()))
        .collect();
    names.sort();
    let num = |p: &Path| fs::read_to_string(p).ok().and_then(|s| s.trim().parse::<u64>().ok());
    let mut out = Vec::new();
    for n in names {
        let dev = drm.join(&n).join("device");
        let Some(busy) = num(&dev.join("gpu_busy_percent")) else { continue };
        let name = fs::read_to_string(dev.join("product_name"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("AMD GPU ({n})"));
        out.push(Gpu {
            name,
            util: (busy as f64).clamp(0.0, 100.0),
            mem_used: num(&dev.join("mem_info_vram_used")).unwrap_or(0),
            mem_total: num(&dev.join("mem_info_vram_total")).unwrap_or(0),
        });
    }
    out
}

fn smi_bin() -> String {
    std::env::var("HERDR_PLANNER_NVIDIA_SMI").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "nvidia-smi".into())
}

/// The video cards: NVIDIA through `nvidia-smi` (a short process with a time limit), else AMD from `/sys`.
async fn read_gpus() -> Vec<Gpu> {
    let run = tokio::process::Command::new(smi_bin())
        .args(["--query-gpu=name,utilization.gpu,memory.used,memory.total", "--format=csv,noheader,nounits"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    if let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(4), run).await {
        if out.status.success() {
            let cards = parse_nvidia(&String::from_utf8_lossy(&out.stdout));
            if !cards.is_empty() {
                return cards;
            }
        }
    }
    tokio::task::spawn_blocking(|| read_amd(Path::new("/sys/class/drm"))).await.unwrap_or_default()
}

/// The machine's counters as they stand now: the busy and total ticks of the processor, the core count, the memory.
fn read_proc() -> Result<((u64, u64), u32, Mem), String> {
    let stat = fs::read_to_string("/proc/stat").map_err(|e| format!("/proc/stat: {e}"))?;
    let (busy, all, cores) = parse_stat(&stat).ok_or("/proc/stat: no cpu line")?;
    let mem = parse_meminfo(&fs::read_to_string("/proc/meminfo").map_err(|e| format!("/proc/meminfo: {e}"))?).ok_or("/proc/meminfo: no MemTotal")?;
    Ok(((busy, all), cores, mem))
}

/// A reading from the file the checks put in place of the machine; `None` when there is no such file.
fn read_override() -> Option<Load> {
    let path = std::env::var_os("HERDR_PLANNER_LOAD_FILE").filter(|v| !v.is_empty())?;
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str::<Load>(&text).ok()
}

fn every_ms() -> u64 {
    std::env::var("HERDR_PLANNER_LOAD_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|v| *v >= 100).unwrap_or(EVERY_MS)
}

type Ticks = (u64, u64);

async fn read_now() -> Result<(Ticks, u32, Mem), String> {
    tokio::task::spawn_blocking(read_proc).await.map_err(|e| e.to_string())?
}

/// One reading of the machine. `prev` is the processor's counters of the reading before (and when it was taken).
async fn sample(prev: &mut Option<(Ticks, u64)>, every: u64) -> Load {
    if let Some(o) = read_override() {
        return Load { every_ms: every, at: if o.at == 0 { now_ms() } else { o.at }, ..o };
    }
    let now = now_ms();
    let (ticks, cores, mem) = match read_now().await {
        Ok(v) => v,
        Err(e) => {
            *prev = None;
            return Load { at: now, ok: false, error: e, every_ms: every, ..Default::default() };
        }
    };
    // The share is a change between two readings: after a long sleep the old reading says nothing about now.
    let (base, cur, cores, mem) = match *prev {
        Some((p, at)) if now.saturating_sub(at) <= every * 2 => (p, ticks, cores, mem),
        _ => {
            tokio::time::sleep(Duration::from_millis(300)).await;
            match read_now().await {
                Ok((t2, c2, m2)) => (ticks, t2, c2, m2),
                Err(_) => (ticks, ticks, cores, mem),
            }
        }
    };
    *prev = Some((cur, now_ms()));
    Load { at: now_ms(), ok: true, every_ms: every, cpu: Cpu { percent: cpu_percent(base, cur), cores }, mem, gpus: read_gpus().await, ..Default::default() }
}

/// Keeps `app.load` fresh and tells the open pages (the `load` event). Nothing is read while nobody listens.
pub async fn run(app: Shared) {
    let every = every_ms();
    let mut prev = None;
    let mut last = 0;
    loop {
        // A page that connects right after a reading takes that one: a share over a few milliseconds is noise.
        if app.bus.subscribers() > 0 && now_ms().saturating_sub(last) >= 1000.min(every) {
            publish(&app, sample(&mut prev, every).await).await;
            last = now_ms();
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(every)) => {}
            _ = app.load_poke.notified() => {}
        }
    }
}

async fn publish(app: &Shared, load: Load) {
    if let Ok(json) = serde_json::to_string(&load) {
        *app.load.write().await = load;
        app.bus.send("load", json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  4705 150 1120 16250 520 30 45 0 0 0\ncpu0 1175 37 280 4062 130 7 11 0 0 0\ncpu1 1180 38 280 4063 130 8 11 0 0 0\nintr 12345\nctxt 999\n";

    #[test]
    fn stat_gives_busy_total_and_cores() {
        let (busy, all, cores) = parse_stat(STAT).unwrap();
        assert_eq!(cores, 2);
        assert_eq!(all, 4705 + 150 + 1120 + 16250 + 520 + 30 + 45);
        // idle and iowait are not busy
        assert_eq!(busy, all - 16250 - 520);
        assert!(parse_stat("intr 1\n").is_none());
    }

    #[test]
    fn the_share_is_the_change_between_readings() {
        assert_eq!(cpu_percent((100, 1000), (150, 1100)), 50.0);
        assert_eq!(cpu_percent((100, 1000), (100, 1000)), 0.0);
        // a counter that went back (a reset) is not a negative load
        assert_eq!(cpu_percent((500, 1000), (100, 900)), 0.0);
        assert_eq!(cpu_percent((0, 0), (200, 100)), 100.0);
    }

    #[test]
    fn meminfo_counts_what_is_available_as_free() {
        let m = parse_meminfo("MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    4000000 kB\nBuffers: 5 kB\n").unwrap();
        assert_eq!(m.total, 16_000_000 * 1024);
        assert_eq!(m.used, 12_000_000 * 1024);
        assert!((m.percent - 75.0).abs() < 1e-9);
        // an old kernel without MemAvailable
        let old = parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 100 kB\nCached: 300 kB\n").unwrap();
        assert_eq!(old.used, 500 * 1024);
        assert!(parse_meminfo("MemFree: 5 kB\n").is_none());
    }

    #[test]
    fn nvidia_lines_become_cards() {
        let g = parse_nvidia("NVIDIA GeForce RTX 4090, 37, 2048, 24564\nNVIDIA A100, SXM4 80GB, 5, [N/A], 81920\n\nbroken\n");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(g[0].util, 37.0);
        assert_eq!(g[0].mem_used, 2048 * 1048576);
        assert_eq!(g[0].mem_total, 24564 * 1048576);
        assert_eq!(g[1].name, "NVIDIA A100, SXM4 80GB");
        assert_eq!(g[1].mem_used, 0);
        assert!(parse_nvidia("").is_empty());
        assert!(parse_nvidia("No devices were found\n").is_empty());
    }

    #[test]
    fn amd_cards_are_read_from_sys() {
        let d = std::env::temp_dir().join(format!("hp-load-amd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let dev = d.join("card1/device");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("gpu_busy_percent"), "42\n").unwrap();
        fs::write(dev.join("mem_info_vram_used"), "1073741824\n").unwrap();
        fs::write(dev.join("mem_info_vram_total"), "8589934592\n").unwrap();
        // a card without the counters (an Intel one) and a connector folder are not cards of ours
        fs::create_dir_all(d.join("card0/device")).unwrap();
        fs::create_dir_all(d.join("card1-DP-1")).unwrap();
        let g = read_amd(&d);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].util, 42.0);
        assert_eq!(g[0].mem_total, 8 << 30);
        assert!(g[0].name.contains("card1"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_reading_round_trips_through_json_with_missing_fields() {
        let l: Load = serde_json::from_str(r#"{"ok": true, "cpu": {"percent": 12.5, "cores": 8}}"#).unwrap();
        assert!(l.ok);
        assert_eq!(l.cpu.cores, 8);
        assert!(l.gpus.is_empty());
        let off: Load = serde_json::from_str(r#"{"ok": false, "error": "x"}"#).unwrap();
        assert!(!off.ok);
    }
}
