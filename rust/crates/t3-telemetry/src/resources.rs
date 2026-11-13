use crate::TelemetryNumber;
use crate::{
    Error, Result, datetime, history,
    model::{self, ProcessState, TelemetryCounters},
    unix_millis,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use t3_contracts::{background::*, resource_telemetry::*, server::*};
use tokio::sync::{Notify, broadcast};

pub trait ResourceSampler: Send + 'static {
    fn sample(
        &mut self,
        root_pid: u64,
        external: &[ResourceMonitorExternalProcess],
        now_ms: u64,
    ) -> Result<ResourceMonitorSnapshotEvent>;
    fn signal(
        &mut self,
        _pid: u64,
        _start_time_ms: u64,
        _signal: ServerProcessSignal,
    ) -> Result<bool> {
        Ok(false)
    }
}

pub struct ResourceTelemetry {
    clock: Arc<dyn crate::Clock>,
    server_pid: u64,
    sampler: Arc<Mutex<Box<dyn ResourceSampler>>>,
    state: tokio::sync::Mutex<State>,
    sampling: tokio::sync::Mutex<()>,
    changes: broadcast::Sender<ResourceTelemetrySnapshot>,
    demand: Arc<Demand>,
}

struct Demand {
    count: AtomicUsize,
    changed: Notify,
}
struct State {
    latest: ResourceTelemetrySnapshot,
    previous: BTreeMap<model::Identity, ProcessState>,
    counters: TelemetryCounters,
    history: VecDeque<ResourceMonitorSnapshotEvent>,
    history_bytes: usize,
    history_entries: usize,
    desktop: Option<DesktopHostTelemetrySnapshot>,
    desktop_started_at: Option<u64>,
    last_sequence: Option<(u64, u64)>,
    generation: u64,
    attribution: BTreeMap<(String, String), ResourceAttributionEntry>,
}

pub struct ResourceSubscription {
    pub latest: ResourceTelemetrySnapshot,
    pub changes: broadcast::Receiver<ResourceTelemetrySnapshot>,
    demand: Arc<Demand>,
}
impl Drop for ResourceSubscription {
    fn drop(&mut self) {
        self.demand.count.fetch_sub(1, Ordering::AcqRel);
        self.demand.changed.notify_waiters();
    }
}
impl ResourceSubscription {
    pub async fn recv(&mut self) -> Option<ResourceTelemetrySnapshot> {
        loop {
            match self.changes.recv().await {
                Ok(snapshot) => return Some(snapshot),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

pub struct ResourceWorker {
    cancel: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl ResourceWorker {
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for ResourceWorker {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub fn unknown_power(now_ms: u64) -> HostPowerSnapshot {
    HostPowerSnapshot {
        source: HostPowerSource::Unknown,
        idle: BackgroundBooleanState::Unknown,
        idle_seconds: None,
        locked: BackgroundBooleanState::Unknown,
        suspended: false,
        on_battery: BackgroundBooleanState::Unknown,
        low_power_mode: BackgroundBooleanState::Unknown,
        thermal_state: HostPowerThermalState::Unknown,
        stale: true,
        updated_at: datetime(now_ms),
    }
}

fn source_health(status: ResourceTelemetrySourceStatus) -> ResourceTelemetrySourceHealth {
    ResourceTelemetrySourceHealth {
        status,
        last_sample_at: WireOption::None,
        last_error: WireOption::None,
    }
}
pub fn initial_health() -> ResourceTelemetryHealth {
    ResourceTelemetryHealth {
        native: source_health(ResourceTelemetrySourceStatus::Starting),
        desktop: source_health(ResourceTelemetrySourceStatus::Unavailable),
        sidecar_version: WireOption::None,
        sidecar_pid: WireOption::None,
        restart_count: 0,
        collection_duration_micros: 0,
        scanned_process_count: 0,
        retained_process_count: 0,
        inaccessible_process_count: 0,
    }
}

fn history_entries(sample: &ResourceMonitorSnapshotEvent) -> usize {
    sample.processes.len() + sample.external_processes.as_ref().map_or(0, Vec::len)
}
fn history_bytes(sample: &ResourceMonitorSnapshotEvent) -> usize {
    std::mem::size_of::<ResourceMonitorSnapshotEvent>()
        + sample
            .processes
            .iter()
            .map(|p| {
                std::mem::size_of::<ResourceMonitorProcessSample>()
                    + p.name.len()
                    + p.command.len()
                    + p.status.len()
            })
            .sum::<usize>()
        + sample.external_processes.as_ref().map_or(0, |p| {
            p.len() * std::mem::size_of::<ResourceMonitorExternalProcess>()
        })
}
fn power_interval(power: &HostPowerSnapshot) -> u64 {
    if power.low_power_mode == BackgroundBooleanState::True
        || matches!(
            power.thermal_state,
            HostPowerThermalState::Serious | HostPowerThermalState::Critical
        )
    {
        15000
    } else if power.on_battery == BackgroundBooleanState::True {
        5000
    } else {
        1000
    }
}

impl ResourceTelemetry {
    pub fn new(server_pid: u64) -> Arc<Self> {
        Self::with_sampler(
            server_pid,
            Box::<crate::collector::NativeSampler>::default(),
            unix_millis(),
        )
    }

    pub fn with_sampler(
        server_pid: u64,
        sampler: Box<dyn ResourceSampler>,
        now_ms: u64,
    ) -> Arc<Self> {
        Self::with_sampler_and_clock(server_pid, sampler, now_ms, Arc::new(crate::SystemClock))
    }
    pub fn with_sampler_and_clock(
        server_pid: u64,
        sampler: Box<dyn ResourceSampler>,
        now_ms: u64,
        clock: Arc<dyn crate::Clock>,
    ) -> Arc<Self> {
        let previous = BTreeMap::new();
        let counters = TelemetryCounters::default();
        let merged = model::merge_processes(model::MergeInput {
            server_pid,
            sidecar_pid: None,
            fallback_sampled_at_ms: now_ms,
            native: None,
            desktop: None,
            electron_roots: &BTreeMap::new(),
            previous: &previous,
            counters: &counters,
            update_previous: false,
            accumulate_counters: false,
        });
        Arc::new(Self {
            clock,
            server_pid,
            sampler: Arc::new(Mutex::new(sampler)),
            state: tokio::sync::Mutex::new(State {
                latest: ResourceTelemetrySnapshot {
                    read_at: datetime(now_ms),
                    sample_interval_ms: 1000,
                    processes: vec![],
                    groups: merged.groups,
                    power: unknown_power(now_ms),
                    speed_limit_percent: WireOption::None,
                    attribution: ResourceAttributionSnapshot {
                        read_at: datetime(now_ms),
                        entries: vec![],
                    },
                    health: initial_health(),
                },
                previous,
                counters,
                history: VecDeque::new(),
                history_bytes: 0,
                history_entries: 0,
                desktop: None,
                desktop_started_at: None,
                last_sequence: None,
                generation: 0,
                attribution: BTreeMap::new(),
            }),
            sampling: tokio::sync::Mutex::new(()),
            changes: broadcast::channel(8).0,
            demand: Arc::new(Demand {
                count: AtomicUsize::new(0),
                changed: Notify::new(),
            }),
        })
    }

    pub async fn latest(&self) -> ResourceTelemetrySnapshot {
        self.state.lock().await.latest.clone()
    }

    pub async fn subscribe(&self) -> ResourceSubscription {
        let state = self.state.lock().await;
        let changes = self.changes.subscribe();
        self.demand.count.fetch_add(1, Ordering::AcqRel);
        self.demand.changed.notify_waiters();
        ResourceSubscription {
            latest: state.latest.clone(),
            changes,
            demand: Arc::clone(&self.demand),
        }
    }

    pub fn start(self: &Arc<Self>, metrics_enabled: bool) -> ResourceWorker {
        let cancel = tokio_util::sync::CancellationToken::new();
        let token = cancel.clone();
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            let mut last_metrics = None;
            loop {
                let Some(service) = weak.upgrade() else {
                    break;
                };
                let changed = service.demand.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let live = service.demand.count.load(Ordering::Acquire) > 0;
                if !live && !metrics_enabled {
                    service
                        .update_desktop_health(service.clock.now_ms(), 120_000)
                        .await;
                    tokio::select! { _ = token.cancelled() => break, _ = &mut changed => {},_=tokio::time::sleep(Duration::from_secs(30))=>{} }
                    continue;
                }
                if let Ok(snapshot) = service.refresh().await
                    && metrics_enabled
                    && last_metrics.is_none_or(|at: tokio::time::Instant| {
                        at.elapsed() >= Duration::from_secs(60)
                    })
                {
                    crate::metrics::record_process_groups(&snapshot.processes);
                    last_metrics = Some(tokio::time::Instant::now());
                }
                service
                    .update_desktop_health(service.clock.now_ms(), 120_000)
                    .await;
                let interval = if live {
                    service.sample_interval().await
                } else {
                    60_000
                };
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = &mut changed => {},
                    _ = tokio::time::sleep(Duration::from_millis(interval)) => {},
                }
            }
        });
        ResourceWorker {
            cancel,
            task: Some(task),
        }
    }

    async fn sample_interval(&self) -> u64 {
        power_interval(&self.state.lock().await.latest.power)
    }

    pub async fn refresh(&self) -> Result<ResourceTelemetrySnapshot> {
        self.refresh_at(self.clock.now_ms()).await
    }

    pub async fn refresh_at(&self, now_ms: u64) -> Result<ResourceTelemetrySnapshot> {
        let _sampling = self.sampling.lock().await;
        let (external, generation) = {
            let state = self.state.lock().await;
            (
                state
                    .desktop
                    .as_ref()
                    .map(|desktop| {
                        vec![ResourceMonitorExternalProcess {
                            pid: desktop.electron_pid,
                            start_time_ms: desktop
                                .electron_processes
                                .iter()
                                .find(|p| p.pid == desktop.electron_pid)
                                .map(|p| p.creation_time_ms),
                        }]
                    })
                    .unwrap_or_default(),
                state.generation,
            )
        };
        let sampler = Arc::clone(&self.sampler);
        let pid = self.server_pid;
        let sample = tokio::task::spawn_blocking(move || {
            sampler
                .lock()
                .map_err(|_| Error::WorkerStopped)?
                .sample(pid, &external, now_ms)
        })
        .await
        .map_err(|_| Error::WorkerStopped)?;
        match sample {
            Ok(sample) => self.ingest_native(sample, generation).await,
            Err(error) => {
                let mut state = self.state.lock().await;
                state.latest.health.native.status = ResourceTelemetrySourceStatus::Degraded;
                state.latest.health.native.last_error =
                    WireOption::Some("Resource telemetry refresh failed.".into());
                let _ = self.changes.send(state.latest.clone());
                Err(error)
            }
        }
    }

    pub async fn ingest_native(
        &self,
        mut sample: ResourceMonitorSnapshotEvent,
        generation: u64,
    ) -> Result<ResourceTelemetrySnapshot> {
        let mut state = self.state.lock().await;
        if generation < state.generation
            || state
                .last_sequence
                .is_some_and(|last| (generation, sample.sequence) <= last)
        {
            return Ok(state.latest.clone());
        }
        for process in &mut sample.processes {
            process.command = crate::privacy::redact_text(&process.command);
        }
        state.generation = generation;
        state.last_sequence = Some((generation, sample.sequence));
        let roots = sample
            .external_processes
            .as_ref()
            .into_iter()
            .flatten()
            .map(|e| (e.pid, e.start_time_ms))
            .collect();
        let merged = model::merge_processes(model::MergeInput {
            server_pid: self.server_pid,
            sidecar_pid: None,
            fallback_sampled_at_ms: sample.sampled_at_unix_ms,
            native: Some(&sample),
            desktop: state.desktop.as_ref(),
            electron_roots: &roots,
            previous: &state.previous,
            counters: &state.counters,
            update_previous: true,
            accumulate_counters: true,
        });
        state.previous = merged.previous;
        state.counters = merged.counters;
        state.latest.read_at = datetime(merged.sampled_at_ms);
        state.latest.processes = merged.processes;
        state.latest.groups = merged.groups;
        state.latest.health.native = ResourceTelemetrySourceHealth {
            status: ResourceTelemetrySourceStatus::Healthy,
            last_sample_at: WireOption::Some(datetime(sample.sampled_at_unix_ms)),
            last_error: WireOption::None,
        };
        state.latest.health.restart_count = generation;
        state.latest.health.collection_duration_micros = sample.collection_duration_micros;
        state.latest.health.scanned_process_count = sample.scanned_process_count;
        state.latest.health.retained_process_count = sample.retained_process_count;
        state.latest.health.inaccessible_process_count = sample.inaccessible_process_count;
        state.latest.attribution = ResourceAttributionSnapshot {
            read_at: state.latest.read_at.clone(),
            entries: state.attribution.values().cloned().collect(),
        };
        let bytes = history_bytes(&sample);
        state.history_bytes += bytes;
        state.history_entries += history_entries(&sample);
        let now = sample.sampled_at_unix_ms;
        state.history.push_back(sample);
        while state.history.len() > 3600
            || state.history_bytes > 64 * 1024 * 1024
            || state.history_entries > 20_000
            || state
                .history
                .front()
                .is_some_and(|s| now.saturating_sub(s.sampled_at_unix_ms) > 3_600_000)
        {
            if let Some(sample) = state.history.pop_front() {
                state.history_bytes = state.history_bytes.saturating_sub(history_bytes(&sample));
                state.history_entries = state
                    .history_entries
                    .saturating_sub(history_entries(&sample));
            } else {
                break;
            }
        }
        let snapshot = state.latest.clone();
        let _ = self.changes.send(snapshot.clone());
        Ok(snapshot)
    }

    pub async fn ingest_desktop(
        &self,
        desktop: DesktopHostTelemetrySnapshot,
    ) -> ResourceTelemetrySnapshot {
        let mut state = self.state.lock().await;
        if state.desktop.as_ref().is_some_and(|old| {
            desktop.sequence <= old.sequence || desktop.sampled_at_unix_ms < old.sampled_at_unix_ms
        }) {
            return state.latest.clone();
        }
        let roots = [(
            desktop.electron_pid,
            desktop
                .electron_processes
                .iter()
                .find(|p| p.pid == desktop.electron_pid)
                .map(|p| p.creation_time_ms),
        )]
        .into_iter()
        .collect();
        let merged = model::merge_processes(model::MergeInput {
            server_pid: self.server_pid,
            sidecar_pid: None,
            fallback_sampled_at_ms: desktop.sampled_at_unix_ms,
            native: state.history.back(),
            desktop: Some(&desktop),
            electron_roots: &roots,
            previous: &state.previous,
            counters: &state.counters,
            update_previous: false,
            accumulate_counters: false,
        });
        state.latest.processes = merged.processes;
        state.latest.groups = merged.groups;
        state.latest.read_at = datetime(merged.sampled_at_ms);
        state.latest.power = serde_json::from_value(
            serde_json::to_value(&desktop.power).expect("power serialization"),
        )
        .expect("shared power fields");
        state.latest.speed_limit_percent = desktop
            .speed_limit_percent
            .clone()
            .map_or(WireOption::None, WireOption::Some);
        state.latest.health.desktop = ResourceTelemetrySourceHealth {
            status: ResourceTelemetrySourceStatus::Healthy,
            last_sample_at: WireOption::Some(datetime(desktop.sampled_at_unix_ms)),
            last_error: WireOption::None,
        };
        state.desktop = Some(desktop);
        state.latest.sample_interval_ms = power_interval(&state.latest.power);
        self.demand.changed.notify_waiters();
        let _ = self.changes.send(state.latest.clone());
        state.latest.clone()
    }

    pub async fn update_desktop_health(&self, now_ms: u64, idle_interval_ms: u64) {
        let mut state = self.state.lock().await;
        let stale_after = idle_interval_ms.saturating_add(30_000).max(90_000);
        if state
            .desktop
            .as_ref()
            .map(|d| now_ms.saturating_sub(d.sampled_at_unix_ms) > stale_after)
            .unwrap_or_else(|| {
                state
                    .desktop_started_at
                    .is_some_and(|at| now_ms.saturating_sub(at) > 90_000)
            })
        {
            state.latest.health.desktop.status = ResourceTelemetrySourceStatus::Degraded;
            state.latest.health.desktop.last_error =
                WireOption::Some("Desktop telemetry is stale.".into());
            state.latest.power.stale = true;
            let _ = self.changes.send(state.latest.clone());
        }
    }

    pub async fn desktop_connected(&self) {
        let mut state = self.state.lock().await;
        state.desktop_started_at = Some(self.clock.now_ms());
        state.desktop = None;
        state.latest.health.desktop = source_health(ResourceTelemetrySourceStatus::Starting);
        let _ = self.changes.send(state.latest.clone());
    }

    pub async fn desktop_disconnected(&self) {
        let mut state = self.state.lock().await;
        state.latest.health.desktop.status = ResourceTelemetrySourceStatus::Stopped;
        state.latest.power.stale = true;
        state.desktop = None;
        state.desktop_started_at = None;
        let _ = self.changes.send(state.latest.clone());
    }

    pub async fn record_attribution(&self, entry: ResourceAttributionEntry) {
        let mut state = self.state.lock().await;
        let value = state
            .attribution
            .entry((entry.component.to_string(), entry.operation.to_string()))
            .or_insert_with(|| ResourceAttributionEntry {
                logical_read_bytes: 0,
                logical_write_bytes: 0,
                count: 0,
                duration_ms: 0,
                ..entry.clone()
            });
        value.logical_read_bytes = value
            .logical_read_bytes
            .saturating_add(entry.logical_read_bytes);
        value.logical_write_bytes = value
            .logical_write_bytes
            .saturating_add(entry.logical_write_bytes);
        value.count = value.count.saturating_add(entry.count);
        value.duration_ms = value.duration_ms.saturating_add(entry.duration_ms);
    }

    async fn history_at(
        &self,
        input: &ResourceTelemetryHistoryInput,
        now_ms: u64,
    ) -> history::HistoryResult {
        let state = self.state.lock().await;
        history::build_history(
            now_ms,
            input,
            state.latest.sample_interval_ms,
            self.server_pid,
            None,
            &state.history.iter().cloned().collect::<Vec<_>>(),
            state.latest.health.clone(),
        )
    }
    pub async fn read_history(
        &self,
        input: &ResourceTelemetryHistoryInput,
    ) -> ResourceTelemetryHistory {
        self.read_history_at(input, self.clock.now_ms()).await
    }
    pub async fn read_history_at(
        &self,
        input: &ResourceTelemetryHistoryInput,
        now_ms: u64,
    ) -> ResourceTelemetryHistory {
        self.history_at(input, now_ms).await.history
    }
    pub async fn read_process_history(
        &self,
        input: &ServerProcessResourceHistoryInput,
    ) -> ServerProcessResourceHistoryResult {
        history::legacy_history(
            self.history_at(
                &ResourceTelemetryHistoryInput {
                    window_ms: input.window_ms,
                    bucket_ms: input.bucket_ms,
                },
                self.clock.now_ms(),
            )
            .await,
        )
    }
    pub async fn retry(&self) -> ResourceTelemetryRetryResult {
        self.state.lock().await.generation += 1;
        let accepted = self.refresh().await.is_ok();
        ResourceTelemetryRetryResult {
            accepted,
            snapshot: self.latest().await,
        }
    }

    pub async fn process_diagnostics(&self) -> ServerProcessDiagnosticsResult {
        let snapshot = self.refresh().await.unwrap_or(self.latest().await);
        let processes: Vec<_> = snapshot
            .processes
            .into_iter()
            .filter(|p| signalable(&p.category))
            .map(|p| {
                let seconds = p.run_time_ms / 1000;
                let elapsed = if seconds >= 3600 {
                    format!(
                        "{}:{:02}:{:02}",
                        seconds / 3600,
                        seconds / 60 % 60,
                        seconds % 60
                    )
                } else {
                    format!("{}:{:02}", seconds / 60, seconds % 60)
                };
                ServerProcessDiagnosticsEntry {
                    pid: p.identity.pid,
                    start_time_ms: p.identity.start_time_ms,
                    ppid: p.ppid,
                    pgid: WireOption::None,
                    status: nonempty(p.status, "Unknown"),
                    cpu_percent: crate::number(p.cpu_percent.value()),
                    rss_bytes: p.resident_bytes,
                    elapsed: elapsed.into(),
                    command: nonempty(p.command, "unknown"),
                    depth: p.depth.saturating_sub(1),
                    child_pids: p.child_pids,
                }
            })
            .collect();
        ServerProcessDiagnosticsResult {
            server_pid: self.server_pid,
            read_at: snapshot.read_at,
            process_count: processes.len() as u64,
            total_rss_bytes: processes.iter().map(|p| p.rss_bytes).sum(),
            total_cpu_percent: crate::number(processes.iter().map(|p| p.cpu_percent.value()).sum()),
            processes,
            error: match snapshot.health.native.last_error {
                WireOption::Some(message) => {
                    WireOption::Some(ServerProcessDiagnosticsFailure { message })
                }
                WireOption::None => WireOption::None,
            },
        }
    }

    pub async fn signal_process(
        &self,
        input: &ServerSignalProcessInput,
    ) -> ServerSignalProcessResult {
        let refuse = |message: String| ServerSignalProcessResult {
            pid: input.pid,
            signal: input.signal,
            signaled: false,
            message: WireOption::Some(message.into()),
        };
        if input.pid == self.server_pid {
            return refuse("Refusing to signal the T3 server process.".into());
        }
        let Ok(snapshot) = self.refresh().await else {
            return refuse(format!(
                "Could not refresh process {}; refusing to signal a stale identity.",
                input.pid
            ));
        };
        let Some(selected) = snapshot.processes.iter().find(|p| {
            p.identity.pid == input.pid && p.identity.start_time_ms == input.start_time_ms
        }) else {
            return refuse(format!(
                "Process {} no longer matches the selected process identity.",
                input.pid
            ));
        };
        if !signalable(&selected.category) {
            return refuse(format!(
                "Process {} is not a signalable T3 backend descendant.",
                input.pid
            ));
        }
        let sampler = Arc::clone(&self.sampler);
        let input = input.clone();
        let pid = input.pid;
        let signal = input.signal;
        let result = tokio::task::spawn_blocking(move || {
            sampler.lock().map_err(|_| Error::WorkerStopped)?.signal(
                input.pid,
                input.start_time_ms,
                input.signal,
            )
        })
        .await;
        let signaled = matches!(result, Ok(Ok(true)));
        ServerSignalProcessResult {
            pid,
            signal,
            signaled,
            message: if signaled {
                WireOption::None
            } else {
                WireOption::Some("Failed to signal process or process identity changed.".into())
            },
        }
    }
}

fn signalable(category: &ResourceTelemetryProcessCategory) -> bool {
    matches!(
        category,
        ResourceTelemetryProcessCategory::ServerChild
            | ResourceTelemetryProcessCategory::ProviderRoot
            | ResourceTelemetryProcessCategory::TerminalRoot
    )
}
fn nonempty(value: String, fallback: &str) -> t3_contracts::base_schemas::TrimmedNonEmptyString {
    if value.trim().is_empty() {
        fallback.into()
    } else {
        value.into()
    }
}

pub use crate::host::HostResources;
