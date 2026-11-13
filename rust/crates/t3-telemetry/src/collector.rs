use crate::{Result, privacy, resources::ResourceSampler};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use t3_contracts::resource_telemetry::*;

pub struct NativeSampler {
    system: System,
    sequence: u64,
}

impl Default for NativeSampler {
    fn default() -> Self {
        Self {
            system: System::new(),
            sequence: 0,
        }
    }
}

pub fn select_tracked(rows: &[(u64, u64, u64)], roots: &BTreeSet<u64>) -> BTreeSet<u64> {
    let mut children = BTreeMap::<u64, Vec<(u64, u64)>>::new();
    for (pid, ppid, start) in rows {
        children.entry(*ppid).or_default().push((*pid, *start));
    }
    let mut queue: VecDeque<_> = rows
        .iter()
        .filter(|(pid, _, _)| roots.contains(pid))
        .map(|(pid, _, start)| (*pid, *start))
        .collect();
    let mut selected = BTreeSet::new();
    while let Some((pid, start)) = queue.pop_front() {
        if !selected.insert(pid) {
            continue;
        }
        queue.extend(
            children
                .get(&pid)
                .into_iter()
                .flatten()
                .filter(|(_, child_start)| *child_start >= start),
        );
    }
    selected
}

fn truncate(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

impl ResourceSampler for NativeSampler {
    fn sample(
        &mut self,
        root_pid: u64,
        external: &[ResourceMonitorExternalProcess],
        now_ms: u64,
    ) -> Result<ResourceMonitorSnapshotEvent> {
        let started = std::time::Instant::now();
        let two_stage = cfg!(any(target_os = "linux", target_os = "macos"));
        let detail_kind = ProcessRefreshKind::nothing()
            .with_memory()
            .with_cpu()
            .with_disk_usage()
            .with_cmd(UpdateKind::Always)
            .without_tasks();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            if two_stage {
                ProcessRefreshKind::nothing().with_cpu().without_tasks()
            } else {
                detail_kind
            },
        );
        let rows: Vec<_> = self
            .system
            .processes()
            .iter()
            .map(|(pid, p)| {
                (
                    u64::from(pid.as_u32()),
                    p.parent().map(|p| u64::from(p.as_u32())).unwrap_or(0),
                    p.start_time().saturating_mul(1000),
                )
            })
            .collect();
        let external: Vec<_> = external
            .iter()
            .filter(|e| {
                rows.iter().any(|(pid, _, start)| {
                    *pid == e.pid
                        && e.start_time_ms
                            .is_none_or(|expected| *start == expected - expected % 1000)
                })
            })
            .cloned()
            .collect();
        let roots = std::iter::once(root_pid)
            .chain(external.iter().map(|e| e.pid))
            .collect();
        let selected = select_tracked(&rows, &roots);
        let mut details = System::new();
        if two_stage && !selected.is_empty() {
            let pids: Vec<_> = selected
                .iter()
                .filter_map(|pid| u32::try_from(*pid).ok().map(Pid::from_u32))
                .chain(std::iter::once(Pid::from_u32(std::process::id())))
                .collect();
            details.refresh_processes_specifics(
                ProcessesToUpdate::Some(&pids),
                true,
                detail_kind.without_cpu(),
            );
        }
        let detail_system = if two_stage { &details } else { &self.system };
        let monitor = Pid::from_u32(std::process::id());
        let offset = if two_stage {
            self.system
                .process(monitor)
                .zip(details.process(monitor))
                .map(|(a, b)| i128::from(b.start_time()) - i128::from(a.start_time()))
        } else {
            Some(0)
        };
        let processes: Vec<_> = selected
            .iter()
            .filter_map(|pid| {
                let pid32 = u32::try_from(*pid).ok()?;
                let p = self.system.process(Pid::from_u32(pid32))?;
                let d = detail_system.process(Pid::from_u32(pid32))?;
                if i128::from(d.start_time()) - offset? != i128::from(p.start_time()) {
                    return None;
                }
                let command = if d.cmd().is_empty() {
                    p.name().to_string_lossy().into_owned()
                } else {
                    privacy::redact_arguments(d.cmd().iter().map(|part| part.to_string_lossy()))
                };
                Some(ResourceMonitorProcessSample {
                    pid: *pid,
                    ppid: p.parent().map(|v| u64::from(v.as_u32())).unwrap_or(0),
                    start_time_ms: p.start_time().saturating_mul(1000),
                    run_time_ms: p.run_time().saturating_mul(1000),
                    name: truncate(p.name().to_string_lossy().into_owned(), 1024),
                    command: privacy::redact_text(&truncate(command, 16 * 1024)),
                    status: truncate(format!("{:?}", p.status()), 256),
                    cpu_percent: crate::number(f64::from(p.cpu_usage())),
                    cpu_time_ms: p.accumulated_cpu_time(),
                    resident_bytes: d.memory(),
                    virtual_bytes: d.virtual_memory(),
                    io_read_bytes: d.disk_usage().total_read_bytes,
                    io_write_bytes: d.disk_usage().total_written_bytes,
                    io_semantics: if cfg!(target_os = "windows") {
                        ResourceMonitorProcessSampleIoSemantics::AllIo
                    } else {
                        ResourceMonitorProcessSampleIoSemantics::Storage
                    },
                })
            })
            .collect();
        self.sequence += 1;
        Ok(ResourceMonitorSnapshotEvent {
            version: WireLiteral,
            r#type: ResourceMonitorSnapshotEventType::Snapshot,
            sequence: self.sequence,
            sampled_at_unix_ms: now_ms,
            collection_duration_micros: started.elapsed().as_micros() as u64,
            scanned_process_count: self.system.processes().len() as u64,
            retained_process_count: processes.len() as u64,
            inaccessible_process_count: selected.len().saturating_sub(processes.len()) as u64,
            request_id: None,
            external_processes: Some(external),
            processes,
        })
    }

    fn signal(
        &mut self,
        pid: u64,
        start_time_ms: u64,
        signal: t3_contracts::server::ServerProcessSignal,
    ) -> Result<bool> {
        let Ok(pid) = u32::try_from(pid) else {
            return Ok(false);
        };
        let pid = Pid::from_u32(pid);
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().without_tasks(),
        );
        let Some(process) = self.system.process(pid) else {
            return Ok(false);
        };
        if process.start_time().saturating_mul(1000) != start_time_ms {
            return Ok(false);
        }
        let signal = match signal {
            t3_contracts::server::ServerProcessSignal::SIGINT => sysinfo::Signal::Interrupt,
            t3_contracts::server::ServerProcessSignal::SIGKILL => sysinfo::Signal::Kill,
        };
        Ok(process.kill_with(signal).unwrap_or(false))
    }
}
