use crate::{Error, Result, number, unix_millis};

pub fn hostname()->String {sysinfo::System::host_name().unwrap_or_else(||"unknown".into())}
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_contracts::resource_telemetry::HostResourcesSnapshot;

pub fn parse_cpu_list(text: &str) -> Option<BTreeSet<usize>> {
    let mut cpus = BTreeSet::new();
    for part in text.split(',') {
        let (start, end) = part
            .trim()
            .split_once('-')
            .map_or((part.trim(), part.trim()), |(a, b)| (a, b));
        let (start, end) = (start.parse::<usize>().ok()?, end.parse::<usize>().ok()?);
        if end < start || end > 1_000_000 {
            return None;
        }
        cpus.extend(start..=end);
    }
    Some(cpus)
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuTimes {
    pub idle: u64,
    pub total: u64,
    pub count: u64,
}
pub fn pinned_cpu_times(stat: &str, allowed: &BTreeSet<usize>) -> Option<CpuTimes> {
    let mut result = CpuTimes {
        idle: 0,
        total: 0,
        count: 0,
    };
    for line in stat.lines() {
        let mut parts = line.split_whitespace();
        let Some(cpu) = parts
            .next()
            .and_then(|s| s.strip_prefix("cpu"))
            .and_then(|s| s.parse::<usize>().ok())
        else {
            continue;
        };
        if !allowed.contains(&cpu) {
            continue;
        }
        let counters: Vec<u64> = parts.filter_map(|v| v.parse().ok()).take(8).collect();
        if counters.len() < 4 {
            continue;
        }
        result.idle = result
            .idle
            .saturating_add(counters[3] + counters.get(4).copied().unwrap_or(0));
        result.total = result.total.saturating_add(counters.iter().sum());
        result.count += 1;
    }
    (result.count > 0).then_some(result)
}
pub fn darwin_available_memory(text: &str) -> Option<u64> {
    let page = text
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    if page == 0 {
        return None;
    }
    let read = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|s| s.trim().trim_end_matches('.').parse::<u64>().ok())
    };
    read("Pages free:")?
        .checked_add(read("Pages inactive:")?)?
        .checked_add(read("Pages speculative:")?)?
        .checked_mul(page)
}
pub fn utilization(previous: CpuTimes, next: CpuTimes) -> Option<f64> {
    let total = next.total.checked_sub(previous.total)?;
    let idle = next.idle.checked_sub(previous.idle)?;
    if previous.count != next.count || total == 0 {
        return None;
    }
    Some((1.0 - idle as f64 / total as f64).clamp(0.0, 1.0))
}
pub fn constrained_cpu(
    host_utilization: Option<f64>,
    host_cpus: u64,
    quota: u64,
    period: u64,
    usage_delta: Option<u64>,
    elapsed_micros: u64,
) -> (u64, Option<f64>) {
    if period == 0 || host_cpus == 0 {
        return (host_cpus, host_utilization);
    }
    let capacity = (quota as f64 / period as f64).min(host_cpus as f64);
    if capacity <= 0.0 {
        return (host_cpus, host_utilization);
    }
    let cgroup_idle = usage_delta
        .filter(|_| elapsed_micros > 0)
        .map(|usage| capacity - usage as f64 / elapsed_micros as f64);
    let host_idle = host_utilization.map(|usage| host_cpus as f64 * (1.0 - usage));
    let idle = match (cgroup_idle, host_idle) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    (
        (capacity.floor() as u64).max(1),
        idle.map(|idle| (1.0 - idle / capacity).clamp(0.0, 1.0)),
    )
}
pub fn constrained_memory(
    total: u64,
    available: u64,
    limit: Option<u64>,
    current: Option<u64>,
    reclaimable: u64,
) -> (u64, u64) {
    if let (Some(limit), Some(current)) = (limit, current) {
        let total = total.min(limit);
        let available_cgroup = (i128::from(limit) - i128::from(current) + i128::from(reclaimable))
            .clamp(0, i128::from(total)) as u64;
        (total, available.min(available_cgroup).min(total))
    } else {
        (total, available.min(total))
    }
}
fn file(path: impl AsRef<std::path::Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}
fn stat_number(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name))
        .and_then(|s| s.trim().parse().ok())
}
struct LinuxReadings {
    cgroup: Option<PathBuf>,
    allowed: Option<BTreeSet<usize>>,
    cpu_max: Option<(u64, u64)>,
    times: Option<CpuTimes>,
    usage: Option<u64>,
}
fn linux_readings() -> LinuxReadings {
    let cgroup = if cfg!(target_os = "linux") {
        file("/proc/self/cgroup")
            .lines()
            .find_map(|l| l.strip_prefix("0::"))
            .filter(|p| p.starts_with('/'))
            .map(|path| PathBuf::from(format!("/sys/fs/cgroup{}", path.trim_end_matches('/'))))
    } else {
        None
    };
    let allowed = if cfg!(target_os = "linux") {
        file("/proc/self/status")
            .lines()
            .find_map(|l| l.strip_prefix("Cpus_allowed_list:"))
            .and_then(|s| parse_cpu_list(s.trim()))
    } else {
        None
    };
    let cpu_max = cgroup.as_ref().and_then(|path| {
        let text = file(path.join("cpu.max"));
        let mut parts = text.split_whitespace();
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    });
    let times = allowed
        .as_ref()
        .and_then(|cpus| pinned_cpu_times(&file("/proc/stat"), cpus));
    let usage = cgroup
        .as_ref()
        .and_then(|path| stat_number(&file(path.join("cpu.stat")), "usage_usec "));
    LinuxReadings {
        cgroup,
        allowed,
        cpu_max,
        times,
        usage,
    }
}
type CachedHost = Option<(tokio::time::Instant, HostResourcesSnapshot)>;
pub struct HostResources {
    system: Arc<Mutex<sysinfo::System>>,
    cached: tokio::sync::Mutex<CachedHost>,
}
impl Default for HostResources {
    fn default() -> Self {
        Self {
            system: Arc::new(Mutex::new(sysinfo::System::new())),
            cached: tokio::sync::Mutex::new(None),
        }
    }
}
impl HostResources {
    pub async fn read(&self) -> Result<HostResourcesSnapshot> {
        let mut cached = self.cached.lock().await;
        if let Some((at, value)) = &*cached
            && at.elapsed() < Duration::from_secs(5)
        {
            return Ok(value.clone());
        }
        let system = Arc::clone(&self.system);
        let previous = tokio::task::spawn_blocking(move || -> Result<_> {
            system
                .lock()
                .map_err(|_| Error::WorkerStopped)?
                .refresh_cpu_usage();
            Ok(linux_readings())
        })
        .await
        .map_err(|_| Error::WorkerStopped)??;
        let start = tokio::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let elapsed = start.elapsed().as_micros() as u64;
        let system = Arc::clone(&self.system);
        let mut snapshot = tokio::task::spawn_blocking(move || -> Result<_> {
            let mut system = system.lock().map_err(|_| Error::WorkerStopped)?;
            system.refresh_cpu_usage();
            system.refresh_memory();
            let next_times = previous
                .allowed
                .as_ref()
                .and_then(|allowed| pinned_cpu_times(&file("/proc/stat"), allowed));
            let cpu_count = next_times
                .map(|t| t.count)
                .unwrap_or(system.cpus().len() as u64);
            let host_utilization = if let Some((a, b)) = previous.times.zip(next_times) {
                utilization(a, b)
            } else {
                Some(f64::from(system.global_cpu_usage()) / 100.0)
            };
            let (cpu_count, cpu_utilization) = if let Some((quota, period)) = previous.cpu_max {
                let usage = previous
                    .cgroup
                    .as_ref()
                    .and_then(|p| stat_number(&file(p.join("cpu.stat")), "usage_usec "))
                    .zip(previous.usage)
                    .and_then(|(now, old)| now.checked_sub(old));
                constrained_cpu(host_utilization, cpu_count, quota, period, usage, elapsed)
            } else {
                (cpu_count, host_utilization)
            };
            let total = system.total_memory();
            let available = system.available_memory();
            let (total_memory_bytes, available_memory_bytes) = if let Some(cgroup) = previous.cgroup
            {
                let limit = file(cgroup.join("memory.max")).trim().parse().ok();
                let current = file(cgroup.join("memory.current")).trim().parse().ok();
                let reclaimable =
                    stat_number(&file(cgroup.join("memory.stat")), "inactive_file ").unwrap_or(0);
                constrained_memory(total, available, limit, current, reclaimable)
            } else {
                (total, available)
            };
            Ok(HostResourcesSnapshot {
                sampled_at: unix_millis(),
                cpu_utilization: cpu_utilization.map(number),
                cpu_count,
                available_memory_bytes,
                total_memory_bytes,
            })
        })
        .await
        .map_err(|_| Error::WorkerStopped)??;
        if cfg!(target_os = "macos") {
            let mut options = t3_process::ProcessOptions::new("/usr/bin/vm_stat");
            options.timeout = Some(Duration::from_secs(1));
            options.max_output_bytes = 64 * 1024;
            if let Ok(output) = t3_process::run(options).await
                && let Some(available) = darwin_available_memory(&output.stdout_text())
            {
                snapshot.available_memory_bytes = available.min(snapshot.total_memory_bytes);
            }
        }
        *cached = Some((tokio::time::Instant::now(), snapshot.clone()));
        Ok(snapshot)
    }
}
