use crate::TelemetryNumber;
use crate::{datetime, number};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use t3_contracts::resource_telemetry::*;

pub type Identity = (u64, u64);

#[derive(Debug, Clone)]
pub struct ProcessState {
    pub process: ResourceTelemetryProcess,
    pub sampled_at_ms: u64,
}

#[derive(Debug, Default, Clone)]
pub struct GroupCounters {
    pub cpu_time_ms: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub process_starts: u64,
    pub process_exits: u64,
}

#[derive(Debug, Default, Clone)]
pub struct TelemetryCounters(pub [GroupCounters; 4]);

#[derive(Debug, Clone)]
pub struct ProcessDelta {
    pub identity: Identity,
    pub category: ResourceTelemetryProcessCategory,
    pub cpu_time_ms: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
}

pub struct MergeInput<'a> {
    pub server_pid: u64,
    pub sidecar_pid: Option<u64>,
    pub fallback_sampled_at_ms: u64,
    pub native: Option<&'a ResourceMonitorSnapshotEvent>,
    pub desktop: Option<&'a DesktopHostTelemetrySnapshot>,
    pub electron_roots: &'a BTreeMap<u64, Option<u64>>,
    pub previous: &'a BTreeMap<Identity, ProcessState>,
    pub counters: &'a TelemetryCounters,
    pub update_previous: bool,
    pub accumulate_counters: bool,
}

pub struct MergeResult {
    pub sampled_at_ms: u64,
    pub processes: Vec<ResourceTelemetryProcess>,
    pub previous: BTreeMap<Identity, ProcessState>,
    pub counters: TelemetryCounters,
    pub groups: ResourceTelemetryGroups,
    pub deltas: Vec<ProcessDelta>,
}

pub fn identity(process: &ResourceTelemetryProcess) -> Identity {
    (process.identity.pid, process.identity.start_time_ms)
}

pub fn group(category: &ResourceTelemetryProcessCategory) -> usize {
    match category {
        ResourceTelemetryProcessCategory::ResourceMonitor => 2,
        ResourceTelemetryProcessCategory::ElectronMain
        | ResourceTelemetryProcessCategory::ElectronGpu
        | ResourceTelemetryProcessCategory::ElectronRenderer
        | ResourceTelemetryProcessCategory::ElectronUtility => 1,
        _ => 0,
    }
}

fn delta(current: u64, previous: u64, elapsed: u64) -> u64 {
    if elapsed == 0 || elapsed > 30_000 {
        0
    } else {
        current.saturating_sub(previous)
    }
}

fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

pub fn merge_processes(input: MergeInput<'_>) -> MergeResult {
    let sampled_at_ms = input
        .native
        .map(|s| s.sampled_at_unix_ms)
        .into_iter()
        .chain(input.desktop.map(|s| s.sampled_at_unix_ms))
        .max()
        .unwrap_or(input.fallback_sampled_at_ms);
    let mut native: BTreeMap<_, _> = input
        .native
        .into_iter()
        .flat_map(|s| &s.processes)
        .map(|p| (p.pid, p.clone()))
        .collect();
    let native_pids: BTreeSet<_> = native.keys().copied().collect();
    let mut metrics = BTreeMap::new();
    if let Some(desktop) = input.desktop {
        for metric in &desktop.electron_processes {
            if let Some(process) = native.get(&metric.pid) {
                if process.start_time_ms.abs_diff(metric.creation_time_ms) > 2_000 {
                    continue;
                }
            } else {
                let previous = input.previous.get(&(metric.pid, metric.creation_time_ms));
                let cpu_time_ms = metric
                    .cumulative_cpu_seconds
                    .as_ref()
                    .map(|v| (v.value() * 1_000.0).max(0.0).round() as u64)
                    .unwrap_or_else(|| {
                        previous
                            .map(|p| {
                                p.process.cpu_time_ms.saturating_add(
                                    (desktop.sampled_at_unix_ms.saturating_sub(p.sampled_at_ms)
                                        as f64
                                        * metric.cpu_percent.value()
                                        / 100.0)
                                        .max(0.0) as u64,
                                )
                            })
                            .unwrap_or(0)
                    });
                let name = metric
                    .name
                    .clone()
                    .or_else(|| metric.service_name.clone())
                    .unwrap_or_else(|| format!("{:?}", metric.r#type));
                native.insert(
                    metric.pid,
                    ResourceMonitorProcessSample {
                        pid: metric.pid,
                        ppid: 0,
                        start_time_ms: metric.creation_time_ms,
                        run_time_ms: sampled_at_ms.saturating_sub(metric.creation_time_ms),
                        name: name.clone(),
                        command: name,
                        status: "Running".into(),
                        cpu_percent: metric.cpu_percent.clone(),
                        cpu_time_ms,
                        resident_bytes: metric.working_set_bytes,
                        virtual_bytes: 0,
                        io_read_bytes: 0,
                        io_write_bytes: 0,
                        io_semantics: ResourceMonitorProcessSampleIoSemantics::Storage,
                    },
                );
            }
            metrics.insert(metric.pid, metric);
        }
    }
    let mut explicit_roots = BTreeSet::new();
    for (pid, expected) in input.electron_roots {
        if let Some(process) = native.get(pid) {
            let matches = expected
                .map(|s| s.abs_diff(process.start_time_ms) <= 2_000)
                .unwrap_or_else(|| {
                    metrics.contains_key(pid)
                        || input
                            .previous
                            .get(&(*pid, process.start_time_ms))
                            .is_some_and(|p| {
                                p.process.category == ResourceTelemetryProcessCategory::ElectronMain
                            })
                });
            if matches {
                explicit_roots.insert(*pid);
            }
        }
    }
    let electron_pids: BTreeSet<_> = explicit_roots
        .iter()
        .copied()
        .chain(metrics.keys().copied())
        .collect();
    let mut children: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for p in native.values() {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    let mut roots = vec![input.server_pid];
    roots.extend(explicit_roots.iter().copied());
    roots.extend(
        electron_pids
            .iter()
            .filter(|pid| {
                !explicit_roots.contains(pid)
                    && !has_ancestor(**pid, &native, &electron_pids, None, true)
            })
            .copied(),
    );
    let mut depths = BTreeMap::new();
    let mut queue: VecDeque<_> = roots.iter().map(|pid| (*pid, 0)).collect();
    while let Some((pid, depth)) = queue.pop_front() {
        let std::collections::btree_map::Entry::Vacant(entry) = depths.entry(pid) else {
            continue;
        };
        entry.insert(depth);
        for child in children.get(&pid).into_iter().flatten() {
            if !depths.contains_key(child) {
                queue.push_back((*child, depth + 1));
            }
        }
    }
    let mut previous = BTreeMap::new();
    let mut deltas = Vec::new();
    for process in native.values() {
        let key = (process.pid, process.start_time_ms);
        let prev = input.previous.get(&key);
        let timestamp = if native_pids.contains(&process.pid) {
            input.native.map(|s| s.sampled_at_unix_ms)
        } else {
            input.desktop.map(|s| s.sampled_at_unix_ms)
        }
        .unwrap_or(sampled_at_ms);
        let elapsed = prev
            .map(|p| timestamp.saturating_sub(p.sampled_at_ms))
            .unwrap_or(0);
        let cpu = prev
            .map(|p| delta(process.cpu_time_ms, p.process.cpu_time_ms, elapsed))
            .unwrap_or(0);
        let read = prev
            .map(|p| delta(process.io_read_bytes, p.process.io_read_bytes, elapsed))
            .unwrap_or(0);
        let write = prev
            .map(|p| delta(process.io_write_bytes, p.process.io_write_bytes, elapsed))
            .unwrap_or(0);
        let metric = metrics.get(&process.pid);
        let category = if process.pid == input.server_pid {
            ResourceTelemetryProcessCategory::Server
        } else if Some(process.pid) == input.sidecar_pid {
            ResourceTelemetryProcessCategory::ResourceMonitor
        } else if explicit_roots.contains(&process.pid) {
            ResourceTelemetryProcessCategory::ElectronMain
        } else if let Some(metric) = metric {
            match metric.r#type {
                DesktopElectronProcessType::Browser => {
                    ResourceTelemetryProcessCategory::ElectronMain
                }
                DesktopElectronProcessType::Tab => {
                    ResourceTelemetryProcessCategory::ElectronRenderer
                }
                DesktopElectronProcessType::GPU => ResourceTelemetryProcessCategory::ElectronGpu,
                _ => ResourceTelemetryProcessCategory::ElectronUtility,
            }
        } else if has_ancestor(
            process.pid,
            &native,
            &electron_pids,
            Some(input.server_pid),
            false,
        ) {
            let command = process.command.to_lowercase();
            if command.contains("--type=renderer") {
                ResourceTelemetryProcessCategory::ElectronRenderer
            } else if command.contains("--type=gpu-process") {
                ResourceTelemetryProcessCategory::ElectronGpu
            } else {
                ResourceTelemetryProcessCategory::ElectronUtility
            }
        } else {
            ResourceTelemetryProcessCategory::ServerChild
        };
        let preserve = !input.update_previous && prev.is_some();
        let cpu_percent = if preserve {
            prev.map(|p| p.process.cpu_percent.value()).unwrap_or(0.0)
        } else if prev.is_some() && elapsed > 0 && elapsed <= 30_000 {
            cpu as f64 / elapsed as f64 * 100.0
        } else {
            process.cpu_percent.value()
        };
        let rate = |amount: u64| {
            if elapsed > 0 {
                amount as f64 * 1_000.0 / elapsed as f64
            } else {
                0.0
            }
        };
        let normalized = ResourceTelemetryProcess {
            identity: ResourceTelemetryProcessIdentity {
                pid: process.pid,
                start_time_ms: process.start_time_ms,
            },
            ppid: process.ppid,
            child_pids: children.get(&process.pid).cloned().unwrap_or_default(),
            depth: *depths.get(&process.pid).unwrap_or(&0),
            name: process.name.clone(),
            command: process.command.clone(),
            status: process.status.clone(),
            category: category.clone(),
            electron_type: metric.map(|m| m.r#type.clone()),
            electron_service_name: metric.and_then(|m| m.service_name.clone()),
            cpu_percent: number(finite_nonnegative(cpu_percent)),
            cpu_time_ms: process.cpu_time_ms,
            resident_bytes: process.resident_bytes,
            peak_resident_bytes: process
                .resident_bytes
                .max(metric.map(|m| m.peak_working_set_bytes).unwrap_or(0))
                .max(prev.map(|p| p.process.peak_resident_bytes).unwrap_or(0)),
            virtual_bytes: process.virtual_bytes,
            io_read_bytes: process.io_read_bytes,
            io_write_bytes: process.io_write_bytes,
            io_read_bytes_per_second: number(if preserve {
                prev.map(|p| p.process.io_read_bytes_per_second.value())
                    .unwrap_or(0.0)
            } else {
                rate(read)
            }),
            io_write_bytes_per_second: number(if preserve {
                prev.map(|p| p.process.io_write_bytes_per_second.value())
                    .unwrap_or(0.0)
            } else {
                rate(write)
            }),
            io_semantics: match process.io_semantics {
                ResourceMonitorProcessSampleIoSemantics::AllIo => {
                    ResourceTelemetryIoSemantics::AllIo
                }
                _ => ResourceTelemetryIoSemantics::Storage,
            },
            idle_wakeups_per_second: metric.map(|m| m.idle_wakeups_per_second.clone()),
            run_time_ms: process.run_time_ms,
            first_seen_at: prev
                .map(|p| p.process.first_seen_at.clone())
                .unwrap_or_else(|| datetime(sampled_at_ms)),
            last_seen_at: datetime(sampled_at_ms),
        };
        previous.insert(
            key,
            ProcessState {
                process: normalized,
                sampled_at_ms: timestamp,
            },
        );
        deltas.push(ProcessDelta {
            identity: key,
            category,
            cpu_time_ms: cpu,
            io_read_bytes: read,
            io_write_bytes: write,
        });
    }
    let mut ordered = Vec::new();
    let mut visited = BTreeSet::new();
    let by_pid: BTreeMap<_, _> = previous
        .values()
        .map(|p| (p.process.identity.pid, &p.process))
        .collect();
    let mut remaining: Vec<_> = by_pid.values().copied().collect();
    remaining.sort_by_key(|p| (p.depth, p.identity.pid));
    for pid in roots
        .into_iter()
        .chain(remaining.iter().map(|p| p.identity.pid))
    {
        let mut stack = vec![pid];
        while let Some(pid) = stack.pop() {
            if !visited.insert(pid) {
                continue;
            }
            if let Some(process) = by_pid.get(&pid) {
                ordered.push((*process).clone());
                stack.extend(process.child_pids.iter().rev());
            }
        }
    }
    let mut counters = input.counters.clone();
    if input.update_previous && input.accumulate_counters {
        for d in &deltas {
            for index in [group(&d.category), 3] {
                let c = &mut counters.0[index];
                c.cpu_time_ms = c.cpu_time_ms.saturating_add(d.cpu_time_ms);
                c.io_read_bytes = c.io_read_bytes.saturating_add(d.io_read_bytes);
                c.io_write_bytes = c.io_write_bytes.saturating_add(d.io_write_bytes);
            }
        }
        for (states, other, starts) in [
            (&previous, input.previous, true),
            (input.previous, &previous, false),
        ] {
            for (id, state) in states {
                if other.contains_key(id) {
                    continue;
                }
                for index in [group(&state.process.category), 3] {
                    if starts {
                        counters.0[index].process_starts += 1;
                    } else {
                        counters.0[index].process_exits += 1;
                    }
                }
            }
        }
    }
    let groups = ResourceTelemetryGroups {
        backend: aggregate(&ordered, &counters.0[0], Some(0)),
        electron: aggregate(&ordered, &counters.0[1], Some(1)),
        monitor: aggregate(&ordered, &counters.0[2], Some(2)),
        all_t3: aggregate(&ordered, &counters.0[3], None),
    };
    MergeResult {
        sampled_at_ms,
        processes: ordered,
        previous: if input.update_previous {
            previous
        } else {
            input.previous.clone()
        },
        counters,
        groups,
        deltas,
    }
}

fn has_ancestor(
    pid: u64,
    processes: &BTreeMap<u64, ResourceMonitorProcessSample>,
    roots: &BTreeSet<u64>,
    stop: Option<u64>,
    skip_self: bool,
) -> bool {
    let mut pid = if skip_self {
        processes.get(&pid).map(|p| p.ppid).unwrap_or(0)
    } else {
        pid
    };
    let mut visited = BTreeSet::new();
    while pid > 0 && visited.insert(pid) {
        if Some(pid) == stop {
            return false;
        }
        if roots.contains(&pid) {
            return true;
        }
        pid = processes.get(&pid).map(|p| p.ppid).unwrap_or(0);
    }
    false
}

fn aggregate(
    processes: &[ResourceTelemetryProcess],
    counters: &GroupCounters,
    category: Option<usize>,
) -> ResourceTelemetryAggregate {
    let processes: Vec<_> = processes
        .iter()
        .filter(|p| category.is_none_or(|g| group(&p.category) == g))
        .collect();
    ResourceTelemetryAggregate {
        process_count: processes.len() as u64,
        current_cpu_percent: number(processes.iter().map(|p| p.cpu_percent.value()).sum()),
        cpu_time_ms: counters.cpu_time_ms,
        current_rss_bytes: processes.iter().map(|p| p.resident_bytes).sum(),
        peak_rss_bytes: processes.iter().map(|p| p.peak_resident_bytes).sum(),
        io_read_bytes: counters.io_read_bytes,
        io_write_bytes: counters.io_write_bytes,
        io_read_bytes_per_second: number(
            processes
                .iter()
                .map(|p| p.io_read_bytes_per_second.value())
                .sum(),
        ),
        io_write_bytes_per_second: number(
            processes
                .iter()
                .map(|p| p.io_write_bytes_per_second.value())
                .sum(),
        ),
        process_starts: counters.process_starts,
        process_exits: counters.process_exits,
    }
}
