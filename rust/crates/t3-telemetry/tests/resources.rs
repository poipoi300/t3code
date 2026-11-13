use serde_json::json;
use std::collections::BTreeMap;
use t3_contracts::resource_telemetry::*;
use t3_telemetry::{
    TelemetryNumber, history,
    model::{self, MergeInput, TelemetryCounters},
    resources::{ResourceSampler, ResourceTelemetry, initial_health},
};

fn process(pid: u64, ppid: u64, start: u64, cpu: u64, read: u64) -> ResourceMonitorProcessSample {
    serde_json::from_value(json!({"pid":pid,"ppid":ppid,"startTimeMs":start,"runTimeMs":1000,"name":"node","command":"node worker","status":"Running","cpuPercent":12,"cpuTimeMs":cpu,"residentBytes":100,"virtualBytes":1000,"ioReadBytes":read,"ioWriteBytes":0,"ioSemantics":"all-io"})).expect("sample")
}
fn snapshot(
    sequence: u64,
    time: u64,
    processes: Vec<ResourceMonitorProcessSample>,
) -> ResourceMonitorSnapshotEvent {
    ResourceMonitorSnapshotEvent {
        version: WireLiteral,
        r#type: ResourceMonitorSnapshotEventType::Snapshot,
        sequence,
        sampled_at_unix_ms: time,
        collection_duration_micros: 0,
        scanned_process_count: processes.len() as u64,
        retained_process_count: processes.len() as u64,
        inaccessible_process_count: 0,
        request_id: None,
        external_processes: None,
        processes,
    }
}
fn merged<'a>(
    native: &'a ResourceMonitorSnapshotEvent,
    previous: &'a BTreeMap<model::Identity, model::ProcessState>,
    counters: &'a TelemetryCounters,
) -> model::MergeResult {
    model::merge_processes(MergeInput {
        server_pid: 1,
        sidecar_pid: Some(8),
        fallback_sampled_at_ms: 0,
        native: Some(native),
        desktop: None,
        electron_roots: &BTreeMap::new(),
        previous,
        counters,
        update_previous: true,
        accumulate_counters: true,
    })
}

#[test]
fn cumulative_rates_reset_on_pid_reuse_regression_and_long_gap() {
    let first = snapshot(
        1,
        1000,
        vec![
            process(1, 0, 0, 100, 100),
            process(2, 1, 50, 10, 50),
            process(8, 1, 50, 0, 0),
        ],
    );
    let initial = merged(&first, &BTreeMap::new(), &TelemetryCounters::default());
    assert_eq!(initial.groups.backend.process_count, 2);
    assert_eq!(initial.groups.monitor.process_count, 1);
    let second = snapshot(
        2,
        2000,
        vec![process(1, 0, 0, 350, 600), process(2, 1, 50, 110, 70)],
    );
    let next = merged(&second, &initial.previous, &initial.counters);
    assert_eq!(next.groups.backend.cpu_time_ms, 350);
    assert_eq!(next.processes[0].cpu_percent.value(), 25.0);
    assert_eq!(next.processes[0].io_read_bytes_per_second.value(), 500.0);
    assert_eq!(next.processes[1].depth, 1);
    assert_eq!(next.processes[0].child_pids, vec![2]);
    assert_eq!(
        next.processes[0].io_semantics,
        ResourceTelemetryIoSemantics::AllIo
    );
    let reused = merged(
        &snapshot(
            3,
            3000,
            vec![process(1, 0, 0, 1, 1), process(2, 1, 3000, 900, 999)],
        ),
        &next.previous,
        &next.counters,
    );
    assert_eq!(reused.groups.backend.cpu_time_ms, 350);
    assert_eq!(reused.groups.backend.process_starts, 3);
    assert_eq!(reused.groups.backend.process_exits, 1);
    let gap = merged(
        &snapshot(4, 40_000, vec![process(1, 0, 0, 1000, 1000)]),
        &reused.previous,
        &reused.counters,
    );
    assert_eq!(gap.groups.backend.cpu_time_ms, 350);
    assert_eq!(gap.processes[0].io_read_bytes_per_second.value(), 0.0);
}

#[test]
fn history_prorates_boundary_keeps_absent_baseline_and_uses_window_rss_peak() {
    let first = snapshot(1, 1000, vec![process(1, 0, 0, 0, 0)]);
    let second = snapshot(2, 3000, vec![process(1, 0, 0, 1000, 400)]);
    let absent = snapshot(3, 3500, vec![]);
    let last = snapshot(4, 4000, vec![process(1, 0, 0, 1500, 500)]);
    let result = history::build_history(
        4000,
        &ResourceTelemetryHistoryInput {
            window_ms: 2000,
            bucket_ms: 1000,
        },
        1000,
        1,
        None,
        &[first, second, absent, last],
        initial_health(),
    );
    let h = result.history;
    assert_eq!(h.top_processes[0].cpu_time_ms, 1000);
    assert_eq!(h.top_processes[0].io_read_bytes, 300);
    assert_eq!(h.buckets[0].io_read_bytes, 0);
    assert_eq!(h.buckets[1].io_read_bytes, 300);
    assert_eq!(h.retained_sample_count, 5);
    assert_eq!(h.top_processes[0].peak_rss_bytes, 100);
    assert_eq!(history::normalize_input(u64::MAX, 0), (3_600_000, 1000));
}

#[test]
fn desktop_metrics_deduplicate_and_backend_remains_separate() {
    let native = snapshot(
        1,
        2000,
        vec![
            process(10, 0, 0, 100, 0),
            process(1, 10, 0, 100, 0),
            process(2, 1, 0, 100, 0),
            process(11, 10, 0, 100, 0),
        ],
    );
    let desktop:DesktopHostTelemetrySnapshot=serde_json::from_value(json!({"version":1,"type":"desktopTelemetry","sequence":1,"sampledAtUnixMs":2500,"electronPid":10,
        "power":{"source":"unknown","idle":"unknown","idleSeconds":null,"locked":"unknown","suspended":false,"onBattery":"unknown","lowPowerMode":"unknown","thermalState":"unknown","stale":true,"updatedAt":"1970-01-01T00:00:02.000Z"},"speedLimitPercent":null,
        "electronProcesses":[{"pid":10,"creationTimeMs":0,"type":"Browser","cpuPercent":10,"idleWakeupsPerSecond":1,"workingSetBytes":100,"peakWorkingSetBytes":200}]})).expect("desktop");
    let result = model::merge_processes(MergeInput {
        server_pid: 1,
        sidecar_pid: None,
        fallback_sampled_at_ms: 0,
        native: Some(&native),
        desktop: Some(&desktop),
        electron_roots: &[(10, Some(0))].into_iter().collect(),
        previous: &BTreeMap::new(),
        counters: &TelemetryCounters::default(),
        update_previous: true,
        accumulate_counters: true,
    });
    assert_eq!(result.processes.len(), 4);
    assert_eq!(result.groups.backend.process_count, 2);
    assert_eq!(result.groups.electron.process_count, 2);
    assert_eq!(
        result
            .processes
            .iter()
            .find(|p| p.identity.pid == 2)
            .expect("child")
            .category,
        ResourceTelemetryProcessCategory::ServerChild
    );
    assert_eq!(
        result
            .processes
            .iter()
            .find(|p| p.identity.pid == 10)
            .expect("electron")
            .peak_resident_bytes,
        200
    );
    let mut nested = native.clone();
    nested.processes[0].ppid = 1;
    let nested = model::merge_processes(MergeInput {
        server_pid: 1,
        sidecar_pid: None,
        fallback_sampled_at_ms: 0,
        native: Some(&nested),
        desktop: Some(&desktop),
        electron_roots: &[(10, Some(0))].into_iter().collect(),
        previous: &BTreeMap::new(),
        counters: &TelemetryCounters::default(),
        update_previous: true,
        accumulate_counters: true,
    });
    assert_eq!(
        nested
            .processes
            .iter()
            .find(|p| p.identity.pid == 10)
            .expect("explicit root")
            .depth,
        0
    );
}

struct NoSample;
impl ResourceSampler for NoSample {
    fn sample(
        &mut self,
        _: u64,
        _: &[ResourceMonitorExternalProcess],
        _: u64,
    ) -> t3_telemetry::Result<ResourceMonitorSnapshotEvent> {
        Err(t3_telemetry::Error::WorkerStopped)
    }
}

#[tokio::test]
async fn subscriptions_are_atomic_and_old_generations_do_not_rollback() {
    let service = ResourceTelemetry::with_sampler(1, Box::new(NoSample), 0);
    let mut subscription = service.subscribe().await;
    assert!(subscription.latest.processes.is_empty());
    service
        .ingest_native(snapshot(5, 5000, vec![process(1, 0, 0, 100, 0)]), 1)
        .await
        .expect("ingest");
    assert_eq!(
        subscription
            .changes
            .recv()
            .await
            .expect("change")
            .health
            .restart_count,
        1
    );
    service
        .ingest_native(snapshot(9, 9000, vec![]), 0)
        .await
        .expect("stale");
    service
        .ingest_native(snapshot(4, 4000, vec![]), 1)
        .await
        .expect("out of order");
    assert_eq!(service.latest().await.processes.len(), 1);
    assert!(subscription.changes.try_recv().is_err());
    let refused = service
        .signal_process(&t3_contracts::server::ServerSignalProcessInput {
            pid: 1,
            start_time_ms: 0,
            signal: t3_contracts::server::ServerProcessSignal::SIGKILL,
        })
        .await;
    assert!(!refused.signaled);
}

#[tokio::test]
async fn real_native_sampler_collects_only_current_server_tree() {
    let service = ResourceTelemetry::new(u64::from(std::process::id()));
    let snapshot = service.refresh().await.expect("native sample");
    assert_eq!(
        snapshot.health.native.status,
        ResourceTelemetrySourceStatus::Healthy
    );
    assert!(
        snapshot
            .processes
            .iter()
            .any(|p| p.identity.pid == u64::from(std::process::id()) && p.resident_bytes > 0)
    );
    assert_eq!(
        snapshot.groups.all_t3.process_count,
        snapshot.processes.len() as u64
    );
}

struct RecordingSampler(tokio::sync::mpsc::UnboundedSender<u64>, u64);
impl ResourceSampler for RecordingSampler {
    fn sample(
        &mut self,
        _: u64,
        _: &[ResourceMonitorExternalProcess],
        now: u64,
    ) -> t3_telemetry::Result<ResourceMonitorSnapshotEvent> {
        self.1 += 1;
        let _ = self.0.send(now);
        Ok(snapshot(self.1, now, vec![process(1, 0, 0, self.1, 0)]))
    }
}
#[tokio::test(start_paused = true)]
async fn live_sampling_uses_injected_time_and_releases_demand_on_drop() {
    use std::sync::Arc;
    use t3_telemetry::Clock;
    let clock = Arc::new(t3_telemetry::TokioClock::new(1000));
    let (tx, mut samples) = tokio::sync::mpsc::unbounded_channel();
    let service = ResourceTelemetry::with_sampler_and_clock(
        1,
        Box::new(RecordingSampler(tx, 0)),
        1000,
        clock.clone(),
    );
    let worker = service.start(false);
    let mut subscription = service.subscribe().await;
    let _ = samples.recv().await.expect("first sample");
    let first = subscription.recv().await.expect("snapshot");
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let _ = samples.recv().await.expect("next sample");
    let next = subscription.recv().await.expect("next snapshot");
    assert_ne!(first.read_at, next.read_at);
    assert_eq!(next.sample_interval_ms, 1000);
    drop(subscription);
    worker.shutdown().await;
    assert!(samples.try_recv().is_err());
    service.desktop_connected().await;
    let connected_at = clock.now_ms();
    service
        .update_desktop_health(connected_at + 90_000, 120000)
        .await;
    assert_eq!(
        service.latest().await.health.desktop.status,
        ResourceTelemetrySourceStatus::Starting
    );
    service
        .update_desktop_health(connected_at + 90_001, 120000)
        .await;
    assert_eq!(
        service.latest().await.health.desktop.status,
        ResourceTelemetrySourceStatus::Degraded
    );
    service.desktop_disconnected().await;
    assert_eq!(
        service.latest().await.health.desktop.status,
        ResourceTelemetrySourceStatus::Stopped
    );
}
