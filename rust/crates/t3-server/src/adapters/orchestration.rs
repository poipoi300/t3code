//! The only contact surface with `t3-orchestration`; compiled with the `orchestration` feature.
//!
//! The adapter owns one [`TransportService`] and exposes exactly the operations the RPC
//! handlers and HTTP routes need, so that sibling API movement is absorbed here.
use futures::{Stream, stream::BoxStream};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::{IsoDateTime, ThreadId},
    environment::{ORCHESTRATION_PROTOCOL_HEADER, ORCHESTRATION_PROTOCOL_VERSION_TEXT},
    orchestration_v2::*,
};
use t3_orchestration::{
    api::{Clock, ShellQuery, SnapshotWindow},
    ports::transport::{
        http::OrchestrationHttpRead,
        live_stream_budget::LiveStreamLimits,
        thread_history_paging::{
            THREAD_HISTORY_SNAPSHOT_ROW_LIMIT, ThreadHistoryPagePolicy,
            build_bounded_thread_projection,
        },
        wire_projection::project_thread_projection_for_wire,
    },
    projections::store::ProjectionStore,
    runtime::{commit::EventSink, worker::OutboxSignals},
    stores::StoreDatabase,
    stores::projects,
    transport::service::TransportService,
};

pub use t3_orchestration::{
    error::Error as OrchestrationError, ports::transport::http::HttpReadError,
};

/// Body of the 426 answer shared by `GET /ws` and the orchestration HTTP reads.
pub fn protocol_mismatch_body() -> serde_json::Value {
    serde_json::json!({
        "code": "orchestration_protocol_incompatible",
        "message": "Update this client to support orchestration protocol 2.",
        "orchestrationProtocolVersion": 2,
    })
}

/// Name of the header an orchestration HTTP read must carry with the value `"2"`.
pub const PROTOCOL_HEADER: &str = ORCHESTRATION_PROTOCOL_HEADER;

/// `true` when the (already lower-cased) header value is exactly the supported version.
pub fn protocol_header_accepted(value: Option<&str>) -> bool {
    value == Some(ORCHESTRATION_PROTOCOL_VERSION_TEXT)
}

pub type OrchestrationResult<T> = std::result::Result<T, OrchestrationError>;

/// UTC wall clock formatted like JavaScript's `Date.prototype.toISOString`.
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> IsoDateTime {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        iso_from_unix_millis(i64::try_from(millis).unwrap_or(i64::MAX))
    }
}

pub fn iso_from_unix_millis(millis: i64) -> IsoDateTime {
    let days = millis.div_euclid(86_400_000);
    let of_day = millis.rem_euclid(86_400_000);
    // Civil-from-days (H. Hinnant), valid for the full proleptic Gregorian range.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    t3_contracts::wire::WireDateTime(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1_000 % 60,
        of_day % 1_000
    ))
}

pub struct OrchestrationAdapter {
    transport: Arc<TransportService>,
    events: Arc<EventSink>,
    /// The composed runtime (commands, worker, recovery); `None` for a read-only adapter.
    runtime: Option<Arc<t3_orchestration::composition::OrchestrationRuntime>>,
}

pub use t3_orchestration::provider::ProviderAdapter;

/// Server settings as the runtime's settings port: reads the live snapshot and follows changes
/// until shutdown.
#[cfg(feature = "local-services")]
struct LiveSettings {
    settings: Arc<t3_config::settings::SettingsService>,
    current: tokio::sync::watch::Receiver<Arc<t3_contracts::settings::ServerSettings>>,
}
#[cfg(feature = "local-services")]
impl LiveSettings {
    async fn start(
        settings: Arc<t3_config::settings::SettingsService>,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> crate::Result<Self> {
        let mut changes = settings.subscribe_changes();
        let snapshot = settings
            .snapshot()
            .await
            .map_err(|e| crate::Error::Service(e.to_string()))?;
        let (sender, current) = tokio::sync::watch::channel(Arc::new(snapshot));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    changed = changes.recv() => match changed {
                        Some(next) => { sender.send_replace(Arc::new(next)); }
                        None => break,
                    },
                }
            }
        });
        Ok(Self { settings, current })
    }
}
#[cfg(feature = "local-services")]
#[async_trait::async_trait]
impl t3_orchestration::api::SettingsService for LiveSettings {
    async fn get(&self) -> OrchestrationResult<t3_contracts::settings::ServerSettings> {
        self.settings
            .snapshot()
            .await
            .map_err(|_| OrchestrationError::Unsupported("server settings unreadable"))
    }
    fn subscribe(
        &self,
    ) -> OrchestrationResult<
        tokio::sync::watch::Receiver<Arc<t3_contracts::settings::ServerSettings>>,
    > {
        Ok(self.current.clone())
    }
}

impl OrchestrationAdapter {
    /// Starts the composed runtime (`t3_orchestration::composition`): recovery runs before this
    /// returns. Host ports supplied here: the database, the provider registry over `providers`,
    /// Git checkpoints, the system clock and live settings. Every other port keeps its typed
    /// `Unsupported` default (launch environment, titles, pull requests, workspaces, cleanup).
    #[cfg(feature = "local-services")]
    pub async fn start(
        context: &crate::services::DomainContext<'_>,
        providers: Vec<Arc<dyn ProviderAdapter>>,
        dev_replay: bool,
    ) -> crate::Result<Arc<Self>> {
        use t3_orchestration::{
            composition::{OrchestrationRuntime, RandomIds, RuntimePorts},
            ports::sessions_recovery::provider_adapter_registry::{
                AdapterRegistration, AdapterRegistry,
            },
            runtime::checkpoints::GitCheckpointBackend,
        };
        let service = |e: &dyn std::fmt::Display| crate::Error::Service(e.to_string());
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let mut registrations: Vec<AdapterRegistration> = providers
            .into_iter()
            .map(|adapter| AdapterRegistration {
                continuation_key: format!(
                    "{}:instance:{}",
                    adapter.driver().0,
                    adapter.instance_id().0
                ),
                adapter,
                enabled: true,
                credential_access: Vec::new(),
            })
            .collect();
        // TypeScript's dev server seeds this free ACP agent (`devReplay`, "Replay (dev)").
        if dev_replay {
            registrations.push(t3_orchestration::adapters::acp::dev_replay_registration(
                clock.clone(),
                Arc::new(RandomIds),
                true,
            ));
        }
        let mut ports = RuntimePorts::new(
            StoreDatabase(context.database.clone()),
            Arc::new(AdapterRegistry::new(registrations)),
            Arc::new(GitCheckpointBackend::new(t3_git::Git::new)),
            clock,
        );
        ports.settings = Arc::new(
            LiveSettings::start(context.local.settings.clone(), context.shutdown.clone()).await?,
        );
        let runtime = OrchestrationRuntime::start(ports)
            .await
            .map_err(|e| service(&e))?;
        Ok(Arc::new(Self {
            transport: runtime.transport.clone(),
            events: Arc::new(runtime.events.clone()),
            runtime: Some(Arc::new(runtime)),
        }))
    }

    /// The composed runtime, for host services that commit alongside it (projects).
    pub fn composed(&self) -> Option<&Arc<t3_orchestration::composition::OrchestrationRuntime>> {
        self.runtime.as_ref()
    }

    fn runtime(&self) -> OrchestrationResult<&t3_orchestration::composition::OrchestrationRuntime> {
        self.runtime
            .as_deref()
            .ok_or(OrchestrationError::Unsupported(
                "orchestration runtime not started",
            ))
    }

    /// `orchestration.dispatchCommand`: the client command with user provenance, through intake.
    pub async fn dispatch_command(
        &self,
        command: OrchestrationV2Command,
    ) -> OrchestrationResult<OrchestrationV2DispatchCommandResult> {
        use t3_orchestration::ports::thread_project::thread_management_service::with_creation_provenance;
        let value = serde_json::to_value(&command)?;
        let source = match value.get("creationSource") {
            Some(source) => serde_json::from_value(source.clone())?,
            None => t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        };
        let command = with_creation_provenance(
            command,
            t3_contracts::orchestration_v2::OrchestrationV2Actor::User,
            source,
        )?;
        let command: OrchestrationV2ServerCommand =
            serde_json::from_value(serde_json::to_value(command)?)?;
        let receipt = self.runtime()?.dispatch_command(command).await?;
        Ok(serde_json::from_value(
            serde_json::json!({"sequence": receipt.result_sequence}),
        )?)
    }

    /// `orchestration.stopWorkflow`.
    pub async fn stop_workflow(
        &self,
        input: OrchestrationV2StopWorkflowInput,
    ) -> OrchestrationResult<()> {
        self.runtime()?.stop.stop_workflow(input, None).await
    }

    /// `orchestration.forceStopThread`.
    pub async fn force_stop_thread(
        &self,
        input: OrchestrationV2ForceStopThreadInput,
    ) -> OrchestrationResult<OrchestrationV2ForceStopThreadResult> {
        self.runtime()?.stop.force_stop_thread(input).await
    }

    pub fn from_database(database: t3_persistence::Database) -> Arc<Self> {
        let database = StoreDatabase(database);
        let projections = Arc::new(ProjectionStore::new(database.clone()));
        let events = Arc::new(EventSink::new(
            database.clone(),
            projections.clone(),
            Arc::new(SystemClock),
            OutboxSignals::default(),
        ));
        Arc::new(Self::new(
            Arc::new(TransportService::new(projections, events.clone(), database)),
            events,
        ))
    }

    pub fn new(transport: Arc<TransportService>, events: Arc<EventSink>) -> Self {
        Self {
            transport,
            events,
            runtime: None,
        }
    }

    /// Scope-checked HTTP reads (`/api/orchestration/*`); the caller supplies the session scopes.
    pub fn http_read(&self) -> OrchestrationHttpRead<'_> {
        OrchestrationHttpRead {
            service: &self.transport,
        }
    }

    /// `orchestration.getThreadProjection`: one bounded compatibility snapshot.
    pub async fn thread_projection(
        &self,
        thread_id: ThreadId,
    ) -> OrchestrationResult<OrchestrationV2ThreadProjection> {
        let snapshot = self
            .transport
            .projections
            .snapshot(
                thread_id,
                Some(SnapshotWindow {
                    row_limit: THREAD_HISTORY_SNAPSHOT_ROW_LIMIT,
                    ..Default::default()
                }),
            )
            .await?;
        let mut projection = snapshot.projection;
        project_thread_projection_for_wire(&mut projection)?;
        Ok(build_bounded_thread_projection(
            projection,
            snapshot.snapshot_sequence,
            ThreadHistoryPagePolicy::default(),
        )?
        .projection)
    }

    pub async fn turn_item(
        &self,
        input: OrchestrationV2GetTurnItemInput,
    ) -> OrchestrationResult<OrchestrationV2GetTurnItemResult> {
        let item = self
            .transport
            .projections
            .turn_item(input.thread_id, input.item_id)
            .await?;
        Ok(OrchestrationV2GetTurnItemResult { item })
    }

    pub async fn search_thread(
        &self,
        input: OrchestrationV2SearchThreadInput,
    ) -> OrchestrationResult<OrchestrationV2SearchThreadResult> {
        self.transport.search_thread(input).await
    }

    /// Archived rows with the project shells (repository identity is not enriched here).
    pub async fn archived_shell_snapshot(
        &self,
    ) -> OrchestrationResult<OrchestrationV2ArchivedShellSnapshot> {
        let shells = self
            .transport
            .projections
            .thread_shells(ShellQuery {
                archived: true,
                ..Default::default()
            })
            .await?;
        let projects = self
            .transport
            .database
            .read(|db| projects::list_shells(db, None))
            .await?;
        Ok(OrchestrationV2ArchivedShellSnapshot {
            schema_version: shells.schema_version,
            snapshot_sequence: shells.snapshot_sequence,
            projects,
            threads: shells.archived_threads,
        })
    }

    pub async fn thread_family(
        &self,
        thread_id: ThreadId,
    ) -> OrchestrationResult<OrchestrationV2ThreadFamily> {
        let shells = self
            .transport
            .projections
            .thread_shells(ShellQuery {
                family_of: Some(thread_id),
                ..Default::default()
            })
            .await?;
        Ok(OrchestrationV2ThreadFamily {
            snapshot_sequence: shells.snapshot_sequence,
            threads: shells.threads,
        })
    }

    pub async fn worktree_thread_ids(
        &self,
        worktree_path: String,
    ) -> OrchestrationResult<OrchestrationV2WorktreeThreadIds> {
        let shells = self
            .transport
            .projections
            .thread_shells(ShellQuery {
                worktree_path: Some(worktree_path),
                ..Default::default()
            })
            .await?;
        Ok(OrchestrationV2WorktreeThreadIds {
            thread_ids: shells.threads.into_iter().map(|t| t.id).collect(),
        })
    }

    /// `orchestration.subscribeShell`: the authoritative snapshot (or resume deltas), the
    /// `synchronized` marker, then live deltas. Dropping the stream cancels both workers.
    pub fn subscribe_shell(
        &self,
        input: OrchestrationV2SubscribeShellInput,
    ) -> BoxStream<'static, OrchestrationResult<OrchestrationV2ShellStreamItem>> {
        let transport = self.transport.clone();
        boxed(async_stream::try_stream! {
            let mut subscription = transport
                .subscribe_shell(input, LiveStreamLimits::default())
                .await?;
            while let Some(batch) = subscription.pull().await? {
                for item in batch {
                    yield item.value.clone();
                }
            }
            subscription.close().await?;
        })
    }

    /// `orchestration.subscribeThread`: snapshot or exact-cursor replay, then the live tail.
    ///
    /// The subscription is established before any database read; dropping the stream
    /// cancels its drain task. Items are yielded in the budgeted batches the live buffer
    /// produces, so a slow consumer fails the stream (`Lagged`) instead of growing memory.
    pub fn subscribe_thread(
        &self,
        input: OrchestrationV2SubscribeThreadInput,
    ) -> BoxStream<'static, OrchestrationResult<OrchestrationV2ThreadStreamItem>> {
        let transport = self.transport.clone();
        boxed(async_stream::try_stream! {
            let mut subscription = transport
                .subscribe_thread(input, LiveStreamLimits::default())
                .await?;
            while let Some(batch) = subscription.pull().await? {
                for item in batch {
                    yield item.value.clone();
                }
            }
            subscription.close().await?;
        })
    }
}

impl OrchestrationAdapter {
    /// `orchestration.subscribePromptSuggestion`: the thread's latest suggestion, then each
    /// replacement; the session manager clears it for a deleted thread.
    pub fn subscribe_prompt_suggestion(
        &self,
        thread: ThreadId,
    ) -> BoxStream<'static, OrchestrationResult<OrchestrationV2PromptSuggestion>> {
        let sessions = self.runtime().map(|runtime| runtime.sessions.clone());
        boxed(async_stream::try_stream! {
            let mut suggestions = sessions?.subscribe_prompt_suggestion(thread).await?;
            loop {
                let current = suggestions.borrow_and_update().clone();
                yield current;
                if suggestions.changed().await.is_err() {
                    break;
                }
            }
        })
    }
}

fn boxed<T: Send + 'static>(
    stream: impl Stream<Item = OrchestrationResult<T>> + Send + 'static,
) -> BoxStream<'static, OrchestrationResult<T>> {
    Box::pin(stream)
}

impl crate::services::DomainRuntime for OrchestrationAdapter {
    fn name(&self) -> &'static str {
        "orchestration"
    }
    /// The runtime's ordered shutdown: producers, launch/title/stop/management, then admission,
    /// provider sessions, worker and reconcile, while the database is still open.
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            let stopped = match &self.runtime {
                Some(runtime) => runtime
                    .shutdown()
                    .await
                    .map_err(|e| crate::Error::Service(e.to_string())),
                None => Ok(()),
            };
            self.events.drain().await;
            stopped
        })
    }
}
