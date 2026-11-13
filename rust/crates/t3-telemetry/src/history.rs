use crate::TelemetryNumber;
use crate::{
    datetime,
    model::{self, Identity, MergeInput, ProcessDelta, ProcessState, TelemetryCounters},
    number,
};
use std::collections::BTreeMap;
use t3_contracts::{resource_telemetry::*, server::*};

pub fn normalize_input(window_ms: u64, bucket_ms: u64) -> (u64, u64) {
    let window_ms = window_ms.clamp(1_000, 3_600_000);
    (window_ms, bucket_ms.clamp(1_000, window_ms))
}

pub struct HistoryResult {
    pub history: ResourceTelemetryHistory,
    pub backend_buckets: Vec<ResourceTelemetryHistoryBucket>,
}

pub fn build_history(
    read_at_ms: u64,
    input: &ResourceTelemetryHistoryInput,
    sample_interval_ms: u64,
    server_pid: u64,
    sidecar_pid: Option<u64>,
    snapshots: &[ResourceMonitorSnapshotEvent],
    health: ResourceTelemetryHealth,
) -> HistoryResult {
    let (window_ms, bucket_ms) = normalize_input(input.window_ms, input.bucket_ms);
    let start = read_at_ms.saturating_sub(window_ms);
    let mut eligible: Vec<_> = snapshots
        .iter()
        .filter(|s| s.sampled_at_unix_ms <= read_at_ms)
        .collect();
    eligible.sort_by_key(|s| s.sampled_at_unix_ms);
    let first = eligible
        .iter()
        .position(|s| s.sampled_at_unix_ms >= start)
        .unwrap_or(eligible.len())
        .saturating_sub(1);
    let mut previous = BTreeMap::<Identity, ProcessState>::new();
    let counters = TelemetryCounters::default();
    let mut samples = Vec::new();
    let mut process_samples: BTreeMap<Identity, Vec<(ResourceTelemetryProcess, ProcessDelta)>> =
        BTreeMap::new();
    let mut previous_at = None;
    for native in eligible.into_iter().skip(first) {
        let roots = native
            .external_processes
            .as_ref()
            .into_iter()
            .flatten()
            .map(|r| (r.pid, r.start_time_ms))
            .collect();
        let merged = model::merge_processes(MergeInput {
            server_pid,
            sidecar_pid,
            fallback_sampled_at_ms: native.sampled_at_unix_ms,
            native: Some(native),
            desktop: None,
            electron_roots: &roots,
            previous: &previous,
            counters: &counters,
            update_previous: true,
            accumulate_counters: false,
        });
        previous.extend(merged.previous);
        let fraction = previous_at
            .filter(|at| *at < start && *at < native.sampled_at_unix_ms)
            .map(|at| {
                (native.sampled_at_unix_ms - start) as f64 / (native.sampled_at_unix_ms - at) as f64
            })
            .unwrap_or(1.0);
        previous_at = Some(native.sampled_at_unix_ms);
        if native.sampled_at_unix_ms < start {
            continue;
        }
        let deltas: BTreeMap<_, _> = merged
            .deltas
            .into_iter()
            .map(|mut d| {
                d.cpu_time_ms = (d.cpu_time_ms as f64 * fraction).round() as u64;
                d.io_read_bytes = (d.io_read_bytes as f64 * fraction).round() as u64;
                d.io_write_bytes = (d.io_write_bytes as f64 * fraction).round() as u64;
                (d.identity, d)
            })
            .collect();
        let all_io = deltas.values().fold((0, 0), |(r, w), d| {
            (r + d.io_read_bytes, w + d.io_write_bytes)
        });
        let backend_io = deltas
            .values()
            .filter(|d| model::group(&d.category) == 0)
            .fold((0, 0), |(r, w), d| {
                (r + d.io_read_bytes, w + d.io_write_bytes)
            });
        samples.push((native.sampled_at_unix_ms, merged.groups, all_io, backend_io));
        for p in merged.processes {
            if let Some(d) = deltas.get(&model::identity(&p)) {
                process_samples
                    .entry(model::identity(&p))
                    .or_default()
                    .push((p, d.clone()));
            }
        }
    }
    let retained_sample_count =
        samples.len() + process_samples.values().map(Vec::len).sum::<usize>();
    let mut summaries = Vec::new();
    for values in process_samples.values() {
        let Some((first, _)) = values.first() else {
            continue;
        };
        let Some((last, _)) = values.last() else {
            continue;
        };
        summaries.push(ResourceTelemetryProcessSummary {
            identity: last.identity.clone(),
            ppid: last.ppid,
            depth: last.depth,
            name: last.name.clone(),
            command: last.command.clone(),
            category: last.category.clone(),
            first_seen_at: first.first_seen_at.clone(),
            last_seen_at: last.last_seen_at.clone(),
            current_cpu_percent: last.cpu_percent.clone(),
            avg_cpu_percent: number(
                values
                    .iter()
                    .map(|(p, _)| p.cpu_percent.value())
                    .sum::<f64>()
                    / values.len() as f64,
            ),
            max_cpu_percent: number(
                values
                    .iter()
                    .map(|(p, _)| p.cpu_percent.value())
                    .fold(0.0, f64::max),
            ),
            cpu_time_ms: values.iter().map(|(_, d)| d.cpu_time_ms).sum(),
            current_rss_bytes: last.resident_bytes,
            peak_rss_bytes: values
                .iter()
                .map(|(p, _)| p.resident_bytes)
                .max()
                .unwrap_or(0),
            io_read_bytes: values.iter().map(|(_, d)| d.io_read_bytes).sum(),
            io_write_bytes: values.iter().map(|(_, d)| d.io_write_bytes).sum(),
            io_semantics: last.io_semantics.clone(),
            sample_count: values.len() as u64,
        });
    }
    summaries.sort_by(|a, b| {
        b.cpu_time_ms
            .cmp(&a.cpu_time_ms)
            .then(b.peak_rss_bytes.cmp(&a.peak_rss_bytes))
            .then(a.identity.pid.cmp(&b.identity.pid))
    });
    let buckets = |backend: bool| {
        let mut buckets = Vec::new();
        let mut at = start;
        while at < read_at_ms {
            let end = read_at_ms.min(at.saturating_add(bucket_ms));
            let selected: Vec<_> = samples
                .iter()
                .filter(|(time, _, _, _)| {
                    *time >= at && (*time < end || end == read_at_ms && *time == end)
                })
                .collect();
            let group = |g: &ResourceTelemetryGroups| {
                if backend {
                    g.backend.clone()
                } else {
                    g.all_t3.clone()
                }
            };
            let cpu_total: f64 = selected
                .iter()
                .map(|(_, g, _, _)| group(g).current_cpu_percent.value())
                .sum();
            buckets.push(ResourceTelemetryHistoryBucket {
                started_at: datetime(at),
                ended_at: datetime(end),
                avg_cpu_percent: number(if selected.is_empty() {
                    0.0
                } else {
                    cpu_total / selected.len() as f64
                }),
                max_cpu_percent: number(
                    selected
                        .iter()
                        .map(|(_, g, _, _)| group(g).current_cpu_percent.value())
                        .fold(0.0, f64::max),
                ),
                max_rss_bytes: selected
                    .iter()
                    .map(|(_, g, _, _)| group(g).current_rss_bytes)
                    .max()
                    .unwrap_or(0),
                io_read_bytes: selected
                    .iter()
                    .map(|(_, _, all, back)| if backend { back.0 } else { all.0 })
                    .sum(),
                io_write_bytes: selected
                    .iter()
                    .map(|(_, _, all, back)| if backend { back.1 } else { all.1 })
                    .sum(),
                max_process_count: selected
                    .iter()
                    .map(|(_, g, _, _)| group(g).process_count)
                    .max()
                    .unwrap_or(0),
            });
            at = end;
        }
        buckets
    };
    HistoryResult {
        backend_buckets: buckets(true),
        history: ResourceTelemetryHistory {
            read_at: datetime(read_at_ms),
            window_ms,
            bucket_ms,
            sample_interval_ms,
            retained_sample_count: retained_sample_count as u64,
            buckets: buckets(false),
            top_processes: summaries,
            health,
        },
    }
}

pub fn legacy_history(result: HistoryResult) -> ServerProcessResourceHistoryResult {
    let h = result.history;
    let top_processes: Vec<_> = h
        .top_processes
        .into_iter()
        .filter(|p| model::group(&p.category) == 0)
        .map(|p| ServerProcessResourceHistorySummary {
            process_key: format!("{}:{}", p.identity.pid, p.identity.start_time_ms).into(),
            pid: p.identity.pid,
            ppid: p.ppid,
            command: if p.command.is_empty() {
                if p.name.is_empty() {
                    "unknown".into()
                } else {
                    p.name.into()
                }
            } else {
                p.command.into()
            },
            depth: p.depth,
            is_server_root: p.category == ResourceTelemetryProcessCategory::Server,
            first_seen_at: p.first_seen_at,
            last_seen_at: p.last_seen_at,
            current_cpu_percent: number(p.current_cpu_percent.value()),
            avg_cpu_percent: number(p.avg_cpu_percent.value()),
            max_cpu_percent: number(p.max_cpu_percent.value()),
            cpu_seconds_approx: number(p.cpu_time_ms as f64 / 1000.0),
            current_rss_bytes: p.current_rss_bytes,
            max_rss_bytes: p.peak_rss_bytes,
            sample_count: p.sample_count,
        })
        .collect();
    ServerProcessResourceHistoryResult {
        read_at: h.read_at,
        window_ms: h.window_ms,
        bucket_ms: h.bucket_ms,
        sample_interval_ms: h.sample_interval_ms,
        retained_sample_count: h.retained_sample_count,
        total_cpu_seconds_approx: number(
            top_processes
                .iter()
                .map(|p| p.cpu_seconds_approx.value())
                .sum(),
        ),
        top_processes,
        buckets: result
            .backend_buckets
            .into_iter()
            .map(|b| ServerProcessResourceHistoryBucket {
                started_at: b.started_at,
                ended_at: b.ended_at,
                avg_cpu_percent: number(b.avg_cpu_percent.value()),
                max_cpu_percent: number(b.max_cpu_percent.value()),
                max_rss_bytes: b.max_rss_bytes,
                max_process_count: b.max_process_count,
            })
            .collect(),
        error: match h.health.native.last_error {
            WireOption::None => WireOption::None,
            WireOption::Some(message) => WireOption::Some(ServerProcessResourceHistoryFailure {
                failure_tag:
                    ServerProcessResourceHistoryFailureTag::ProcessDiagnosticsQueryFailedError,
                message,
            }),
        },
    }
}
