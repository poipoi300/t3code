pub mod support {
    #![allow(dead_code)]
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::{
        collections::HashSet,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU8, AtomicUsize, Ordering},
        },
    };
    use t3_orchestration::{
        api::*,
        error::{Error, Result},
        stores::StoreDatabase,
    };
    use t3_orchestration::{
        ports::checkpoints_forks::{
            checkpoint_capture_service::domain_event, checkpoint_service::*,
        },
        projections::write::SqliteProjectionWriter,
        runtime::{commit::EventSink, worker::OutboxSignals},
        testing::*,
    };
    use tokio_util::sync::CancellationToken;

    pub fn time() -> IsoDateTime {
        t3_contracts::wire::WireDateTime(TEST_TIME.into())
    }
    pub fn thread(id: &str, worktree: Option<&Path>) -> OrchestrationV2AppThread {
        serde_json::from_value(json!({"createdBy":"user","creationSource":"web","id":id,"projectId":"project:test","title":"Source","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"test"},"runtimeMode":"full-access","interactionMode":"plan","branch":"feature/test","worktreePath":worktree.map(|p|p.to_string_lossy()),"activeProviderThreadId":"provider:test","lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":id},"forkedFrom":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})).expect("thread")
    }
    pub fn run(ordinal: u64, status: &str) -> OrchestrationV2Run {
        serde_json::from_value(json!({"id":format!("run:{ordinal}"),"threadId":"thread:test","ordinal":ordinal,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"test"},"providerThreadId":"provider:test","userMessageId":format!("message:{ordinal}"),"rootNodeId":format!("node:{ordinal}"),"activeAttemptId":null,"status":status,"requestedAt":TEST_TIME,"startedAt":TEST_TIME,"completedAt":if status=="waiting" {None}else{Some(TEST_TIME)},"checkpointId":null,"contextHandoffId":null})).expect("run")
    }
    pub fn scope(cwd: &Path, ordinal: u64) -> OrchestrationV2CheckpointScope {
        prepare_root_run_scope(PrepareRootScope {
            thread_id: ThreadId("thread:test".into()),
            run_id: RunId(format!("run:{ordinal}")),
            root_node_id: NodeId(format!("node:{ordinal}")),
            provider_thread_id: ProviderThreadId("provider:test".into()),
            cwd: cwd.to_owned(),
            created_at: time(),
        })
    }
    pub fn node(
        ordinal: u64,
        scope: &OrchestrationV2CheckpointScope,
        status: &str,
    ) -> OrchestrationV2ExecutionNode {
        serde_json::from_value(json!({"id":format!("node:{ordinal}"),"threadId":"thread:test","runId":format!("run:{ordinal}"),"parentNodeId":null,"rootNodeId":format!("node:{ordinal}"),"kind":"root_turn","status":status,"countsForRun":true,"providerThreadId":"provider:test","providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":scope.id,"startedAt":TEST_TIME,"completedAt":null})).expect("node")
    }
    pub fn provider() -> OrchestrationV2ProviderThread {
        serde_json::from_value(json!({"id":"provider:test","driver":"codex","providerInstanceId":"codex","providerSessionId":"session:test","appThreadId":"thread:test","ownerNodeId":null,"nativeThreadRef":{"driver":"codex","nativeId":"native:test","strength":"strong"},"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":1,"lastRunOrdinal":2,"handoffIds":[],"forkedFrom":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME})).expect("provider")
    }
    pub fn session() -> OrchestrationV2ProviderSession {
        let mut value: Value = serde_json::from_str(
            include_str!("../../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
                .lines()
                .next()
                .expect("fixture"),
        )
        .expect("session fixture");
        for (key,val) in json!({"id":"session:test","driver":"codex","providerInstanceId":"codex","status":"ready","cwd":"/test","model":"test","createdAt":TEST_TIME,"updatedAt":TEST_TIME,"lastError":null}).as_object().expect("session"){value[key]=val.clone();}
        value["capabilities"]["threads"]["canRollbackThread"] = true.into();
        value["capabilities"]["checkpointing"]["providerCanRollbackConversation"] = true.into();
        value["capabilities"]["sessions"]["supportsMultipleProviderThreadsPerSession"] =
            false.into();
        serde_json::from_value(value).expect("session")
    }
    pub fn capture_input(scope: OrchestrationV2CheckpointScope, ordinal: u64) -> Capture {
        Capture {
            scope,
            run_id: Some(RunId(format!("run:{ordinal}"))),
            node_id: NodeId(format!("node:{ordinal}")),
            ordinal,
            app_run_ordinal: (ordinal > 0).then_some(ordinal),
            captured_at: time(),
            force_missing: false,
        }
    }

    pub struct Gate {
        pub entered: tokio::sync::Semaphore,
        pub release: tokio::sync::Semaphore,
    }
    impl Default for Gate {
        fn default() -> Self {
            Self {
                entered: tokio::sync::Semaphore::new(0),
                release: tokio::sync::Semaphore::new(0),
            }
        }
    }
    #[derive(Default)]
    pub struct Backend {
        pub mode: AtomicU8,
        pub captures: AtomicUsize,
        pub lookups: AtomicUsize,
        pub restores: AtomicUsize,
        pub refs: Mutex<HashSet<String>>,
        pub deleted: Mutex<Vec<String>>,
        pub gate: Mutex<Option<Arc<Gate>>>,
        pub restore_gate: Mutex<Option<Arc<Gate>>>,
    }
    impl Backend {
        pub fn service(self: &Arc<Self>) -> Arc<CheckpointService> {
            Arc::new(CheckpointService::new(
                self.clone(),
                self.clone(),
                Arc::new(WorkspaceLocks::default()),
            ))
        }
        pub fn reference(scope: &OrchestrationV2CheckpointScope, ordinal: u64) -> CheckpointRef {
            CheckpointRef(format!(
                "refs/test/{}/{ordinal}",
                encode_id_part(&serde_json::to_string(&scope.id).expect("id"))
            ))
        }
    }
    #[async_trait]
    impl CheckpointBackend for Backend {
        async fn capture(
            &self,
            input: CaptureCheckpoint,
            cancellation: CancellationToken,
        ) -> Result<CheckpointRef> {
            self.captures.fetch_add(1, Ordering::SeqCst);
            let gate = self.gate.lock().expect("gate").clone();
            if let Some(gate) = gate {
                gate.entered.add_permits(1);
                tokio::select! {_=cancellation.cancelled()=>return Err(Error::Cancelled),permit=gate.release.acquire()=>{permit.expect("release").forget();}}
            }
            match self.mode.load(Ordering::SeqCst) {
                1 => return Err(Error::Unsupported("capture")),
                2 => {
                    return Err(Error::Service {
                        operation: "capture",
                        detail: "failed capture".into(),
                    });
                }
                _ => {}
            }
            let reference = Self::reference(&input.scope, input.ordinal);
            self.refs.lock().expect("refs").insert(reference.0.clone());
            Ok(reference)
        }
        async fn lookup(
            &self,
            scope: OrchestrationV2CheckpointScope,
            ordinal: u64,
        ) -> Result<Option<CheckpointRef>> {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            if self.mode.load(Ordering::SeqCst) == 4 {
                return Err(Error::Service {
                    operation: "lookup",
                    detail: "failed lookup".into(),
                });
            }
            let reference = Self::reference(&scope, ordinal);
            Ok(self
                .refs
                .lock()
                .expect("refs")
                .contains(&reference.0)
                .then_some(reference))
        }
        async fn diff(
            &self,
            _cwd: PathBuf,
            _from: CheckpointRef,
            _to: CheckpointRef,
        ) -> Result<CheckpointDiff> {
            if self.mode.load(Ordering::SeqCst) == 5 {
                return Err(Error::Unsupported("diff"));
            }
            Ok(CheckpointDiff {
                files: vec![],
                patch: String::new(),
            })
        }
        async fn restore(
            &self,
            _scope: OrchestrationV2CheckpointScope,
            _ordinal: u64,
            cancellation: CancellationToken,
        ) -> Result<()> {
            self.restores.fetch_add(1, Ordering::SeqCst);
            let gate = self.restore_gate.lock().expect("restore gate").clone();
            if let Some(gate) = gate {
                gate.entered.add_permits(1);
                tokio::select! {
                    _ = cancellation.cancelled() => return Err(Error::Cancelled),
                    permit = gate.release.acquire() => permit.expect("restore release").forget(),
                }
            }
            Ok(())
        }
        async fn prune(
            &self,
            _scope: OrchestrationV2CheckpointScope,
            _preserve: Vec<u64>,
        ) -> Result<()> {
            Err(Error::Unsupported("unused prune"))
        }
    }
    #[async_trait]
    impl CheckpointMetadata for Backend {
        fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
            CheckpointRef(format!(
                "refs/test/{}/{ordinal}",
                encode_id_part(&serde_json::to_string(scope).expect("scope"))
            ))
        }
        async fn is_repository(&self, _cwd: PathBuf) -> Result<bool> {
            match self.mode.load(Ordering::SeqCst) {
                3 => Err(Error::Service {
                    operation: "probe",
                    detail: "bad config".into(),
                }),
                6 => Ok(false),
                _ => Ok(true),
            }
        }
        async fn delete_refs(
            &self,
            _scope: OrchestrationV2CheckpointScope,
            checkpoints: Vec<OrchestrationV2Checkpoint>,
        ) -> Result<()> {
            self.deleted
                .lock()
                .expect("delete")
                .extend(checkpoints.into_iter().map(|c| c.r#ref.0));
            Ok(())
        }
    }

    pub struct Harness {
        pub db: TestDatabase,
        pub reader: Arc<Reader>,
        pub sink: Arc<EventSink>,
        pub ids: Arc<SequenceIds>,
        pub clock: Arc<ManualClock>,
    }
    impl Harness {
        pub async fn new() -> Result<Self> {
            let db = TestDatabase::memory()?;
            let clock = Arc::new(ManualClock::new(TEST_TIME));
            let reader = Arc::new(Reader(db.stores.clone(), None));
            let sink = Arc::new(EventSink::new(
                db.stores.clone(),
                Arc::new(SqliteProjectionWriter),
                clock.clone(),
                OutboxSignals::default(),
            ));
            let harness = Self {
                db,
                reader,
                sink,
                ids: Arc::new(SequenceIds::new("checkpoint-test")),
                clock,
            };
            harness
                .seed("thread.created", thread("thread:test", None))
                .await?;
            Ok(harness)
        }
        pub async fn seed(&self, kind: &str, payload: impl serde::Serialize) -> Result<()> {
            let value = serde_json::to_value(&payload)?;
            let thread_id = value["threadId"]
                .as_str()
                .or_else(|| value["appThreadId"].as_str())
                .or_else(|| value["targetThreadId"].as_str())
                .or_else(|| {
                    (kind == "thread.created")
                        .then(|| value["id"].as_str())
                        .flatten()
                })
                .unwrap_or("thread:test");
            self.sink
                .commit(CommitRequest {
                    receipt: None,
                    guard: CommitGuard::None,
                    decision: Decision {
                        events: vec![domain_event(
                            self.ids.as_ref(),
                            &ThreadId(thread_id.into()),
                            &time(),
                            kind,
                            payload,
                            json!({}),
                        )?],
                        ..Default::default()
                    },
                })
                .await?;
            Ok(())
        }
        pub async fn seed_run(
            &self,
            cwd: &Path,
            ordinal: u64,
            status: &str,
        ) -> Result<OrchestrationV2CheckpointScope> {
            let scope = scope(cwd, ordinal);
            self.seed("provider-thread.updated", provider()).await?;
            self.seed("run.created", run(ordinal, status)).await?;
            self.seed("checkpoint-scope.created", &scope).await?;
            self.seed("node.updated", node(ordinal, &scope, status))
                .await?;
            Ok(scope)
        }
    }
    #[derive(Clone)]
    pub struct Reader(pub StoreDatabase, pub Option<ReadyRollback>);
    #[derive(Clone)]
    pub struct ReadyRollback {
        pub context: CheckpointRollbackContext,
        pub shells: OrchestrationV2ShellSnapshot,
    }
    impl Reader {
        async fn shells(
            &self,
        ) -> Result<(
            Vec<OrchestrationV2ThreadShell>,
            Vec<OrchestrationV2ThreadShell>,
        )> {
            self.0.read(|db| {
            let mut threads=Vec::new(); let mut archived=Vec::new();
            let mut query=db.prepare("SELECT payload_json FROM orchestration_v2_projection_threads ORDER BY created_at,thread_id")?;
            for raw in query.query_map([],|row|row.get::<_,String>(0))? {
                let thread:serde_json::Value=serde_json::from_str(&raw?)?;
                let mut shell:serde_json::Value=serde_json::from_str(include_str!("../../../../fixtures/samples/OrchestrationV2ThreadShell.jsonl").lines().next().expect("fixture"))?;
                for (key,value) in thread.as_object().expect("thread"){shell[key]=value.clone();}
                shell["activeRunId"]=serde_json::Value::Null;
                shell["latestRunId"]=serde_json::Value::Null;
                shell["pendingRuntimeRequest"]=serde_json::Value::Null;
                shell["pendingBackgroundTasks"]=serde_json::json!([]);
                let active:Option<String>=db.query_row("SELECT run_id FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND status='running' LIMIT 1",[thread["id"].as_str()],|row|row.get(0)).optional()?;
                if let Some(active)=active {shell["activeRunId"]=active.into();}
                let shell:OrchestrationV2ThreadShell=serde_json::from_value(shell)?;
                if shell.archived_at.is_some(){archived.push(shell);}else{threads.push(shell);}
            }
            Ok((threads,archived))
        }).await
        }
    }
    use t3_persistence::rusqlite::OptionalExtension;
    #[async_trait]
    impl ProjectionReader for Reader {
        async fn message_count(&self, thread_id: ThreadId, _filter: MessageFilter) -> Result<u64> {
            self.0.read(move |db| Ok(db.query_row("SELECT COUNT(*) FROM orchestration_v2_projection_messages WHERE thread_id=?1", [&thread_id.0], |r|r.get::<_,i64>(0))? as u64)).await
        }
        async fn records(
            &self,
            thread_id: ThreadId,
            fields: Vec<RecordKind>,
            filter: RecordFilter,
        ) -> Result<ProjectionRecords> {
            self.0
                .read(move |db| {
                    t3_orchestration::projections::read::records(db, &thread_id, &fields, &filter)
                })
                .await
        }
        async fn snapshot(
            &self,
            _thread_id: ThreadId,
            _window: Option<SnapshotWindow>,
        ) -> Result<ProjectionSnapshot> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn history(&self, _query: HistoryQuery) -> Result<OrchestrationV2ThreadHistoryPage> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn search(
            &self,
            _query: OrchestrationV2SearchThreadInput,
        ) -> Result<OrchestrationV2SearchThreadResult> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn thread_shells(
            &self,
            query: ShellQuery,
        ) -> Result<OrchestrationV2ThreadShellSnapshot> {
            let (mut threads, mut archived_threads) = self.shells().await?;
            if let Some(ids) = query.thread_ids {
                threads.retain(|t| ids.contains(&t.id));
                archived_threads.retain(|t| ids.contains(&t.id));
            }
            Ok(OrchestrationV2ThreadShellSnapshot {
                schema_version: 2,
                snapshot_sequence: 0,
                threads,
                archived_threads,
            })
        }
        async fn checkpoint_capture(
            &self,
            thread_id: ThreadId,
            run_id: RunId,
            scope_id: CheckpointScopeId,
        ) -> Result<CheckpointCaptureContext> {
            self.0
                .read(move |db| {
                    t3_orchestration::projections::control::checkpoint_capture(
                        db, &thread_id, &run_id, &scope_id,
                    )
                })
                .await
        }
        async fn checkpoint_rollback(
            &self,
            thread_id: ThreadId,
            provider_thread_id: ProviderThreadId,
            checkpoint_id: CheckpointId,
            scope_id: CheckpointScopeId,
        ) -> Result<CheckpointRollbackContext> {
            if let Some(snapshot) = &self.1 {
                return Ok(snapshot.context.clone());
            }
            self.0
                .read(move |db| {
                    t3_orchestration::projections::control::checkpoint_rollback(
                        db,
                        &thread_id,
                        &provider_thread_id,
                        &checkpoint_id,
                        &scope_id,
                    )
                })
                .await
        }
        async fn runtime_request(
            &self,
            _thread_id: ThreadId,
            _request_id: RuntimeRequestId,
        ) -> Result<Option<OrchestrationV2RuntimeRequest>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn turn_item(
            &self,
            _thread_id: ThreadId,
            _item_id: TurnItemId,
        ) -> Result<Option<OrchestrationV2TurnItem>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn plan(
            &self,
            _thread_id: ThreadId,
            _plan_id: PlanId,
        ) -> Result<Option<OrchestrationV2PlanArtifact>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn held_queued_runs(&self, _thread_id: ThreadId) -> Result<Vec<OrchestrationV2Run>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn app_owned_child_threads(&self, _thread_id: ThreadId) -> Result<Vec<ThreadId>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn pending_delegated_completion(&self, _child_thread_id: ThreadId) -> Result<bool> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn turn_start_history(
            &self,
            _thread_id: ThreadId,
            _run_ids: Option<Vec<RunId>>,
        ) -> Result<Vec<OrchestrationV2TurnItem>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn unpaired_interrupt(
            &self,
            _thread_id: ThreadId,
            _request_id: TurnItemId,
            _result_id: TurnItemId,
        ) -> Result<bool> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn attachment_ids(&self, thread_id: ThreadId) -> Result<Vec<String>> {
            self.0
                .read(move |db| t3_orchestration::projections::read::attachment_ids(db, &thread_id))
                .await
        }
        async fn transcript_byte_floor(&self, _thread_id: ThreadId, _limit: u64) -> Result<u64> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn thread(&self, thread_id: ThreadId) -> Result<Option<OrchestrationV2AppThread>> {
            if let Some(snapshot) = &self.1 {
                return Ok(Some(snapshot.context.thread.clone()));
            }
            self.0
                .read(move |db| t3_orchestration::projections::read::thread(db, &thread_id))
                .await
        }
        async fn projection(
            &self,
            thread_id: ThreadId,
        ) -> Result<Option<OrchestrationV2ThreadProjection>> {
            if self.thread(thread_id.clone()).await?.is_none() {
                return Ok(None);
            }
            let records = self
                .records(
                    thread_id,
                    vec![
                        RecordKind::Runs,
                        RecordKind::Attempts,
                        RecordKind::Nodes,
                        RecordKind::Subagents,
                        RecordKind::ProviderSessions,
                        RecordKind::ProviderThreads,
                        RecordKind::ProviderTurns,
                        RecordKind::RuntimeRequests,
                        RecordKind::Messages,
                        RecordKind::Plans,
                        RecordKind::TurnItems,
                        RecordKind::CheckpointScopes,
                        RecordKind::Checkpoints,
                        RecordKind::ContextHandoffs,
                        RecordKind::ContextTransfers,
                    ],
                    RecordFilter::default(),
                )
                .await?;
            Ok(Some(
                t3_orchestration::runtime::dispatch::projection_from_records(records),
            ))
        }
        async fn shell(&self, _thread_id: ThreadId) -> Result<Option<OrchestrationV2ThreadShell>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn shell_snapshot(
            &self,
            _project_ids: Option<Vec<ProjectId>>,
            _root_thread_id: Option<ThreadId>,
        ) -> Result<OrchestrationV2ShellSnapshot> {
            if let Some(snapshot) = &self.1 {
                return Ok(snapshot.shells.clone());
            }
            let (threads, archived_threads) = self.shells().await?;
            Ok(OrchestrationV2ShellSnapshot {
                schema_version: 2,
                snapshot_sequence: 0,
                threads,
                archived_threads,
                projects: vec![],
                older_settled: None,
            })
        }
        async fn control(&self, _query: ControlQuery) -> Result<Option<ControlContext>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn timeline(&self, _query: TimelineQuery) -> Result<TimelinePage> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn run(
            &self,
            thread_id: ThreadId,
            run_id: RunId,
        ) -> Result<Option<OrchestrationV2Run>> {
            self.0.read(move |db| t3_orchestration::projections::read::one(db, "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id=?1 AND run_id=?2", t3_persistence::rusqlite::params![thread_id.0,run_id.0])).await
        }
        async fn run_message(
            &self,
            thread_id: ThreadId,
            run_id: RunId,
        ) -> Result<Option<OrchestrationV2ConversationMessage>> {
            self.0.read(move |db| t3_orchestration::projections::read::one(db, "SELECT payload_json FROM orchestration_v2_projection_messages WHERE thread_id=?1 AND run_id=?2 AND role='user' ORDER BY created_at LIMIT 1", t3_persistence::rusqlite::params![thread_id.0,run_id.0])).await
        }
        async fn can_start_queued_run(&self, _thread_id: ThreadId, _run_id: RunId) -> Result<bool> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn recovery_thread_ids(&self) -> Result<Vec<ThreadId>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn unreadable_thread_ids(&self) -> Result<Vec<ThreadId>> {
            Err(Error::Unsupported("unused fixture read"))
        }
        async fn next_turn_item_ordinal(
            &self,
            _thread_id: ThreadId,
            _run_id: Option<RunId>,
        ) -> Result<u64> {
            Err(Error::Unsupported("unused fixture read"))
        }
    }
}

use std::sync::{Arc, atomic::Ordering};
use support::*;
use t3_orchestration::{api::*, error::Result};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn capture_distinguishes_unsupported_operational_and_repository_failures() -> Result<()> {
    let temp = tempfile::tempdir()?;
    for (mode, status, baseline_ok) in [
        (1, CheckpointStatus::Missing, true),
        (2, CheckpointStatus::Error, false),
        (3, CheckpointStatus::Error, false),
        (6, CheckpointStatus::Missing, true),
    ] {
        let backend = Arc::new(Backend::default());
        backend.mode.store(mode, Ordering::SeqCst);
        let service = backend.service();
        let scope = scope(temp.path(), 1);
        assert_eq!(
            service
                .capture_baseline(scope.clone(), 0, CancellationToken::new())
                .await
                .is_ok(),
            baseline_ok
        );
        let checkpoint = service
            .capture(capture_input(scope.clone(), 1), CancellationToken::new())
            .await?;
        assert_eq!(checkpoint.status, status);
        assert!(checkpoint.files.is_empty());
        let baseline = service
            .materialize_baseline(capture_input(scope, 0))
            .await?;
        assert_eq!(
            baseline.status,
            if mode == 3 {
                CheckpointStatus::Error
            } else {
                CheckpointStatus::Missing
            }
        );
        assert!(baseline.run_id.is_none());
        assert!(baseline.parent_checkpoint_id.is_none());
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn baseline_budget_excludes_lock_wait_and_cancels_the_snapshot() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let scope = scope(temp.path(), 1);
    let backend = Arc::new(Backend::default());
    let service = backend.service();
    let gate = Arc::new(Gate::default());
    *backend.gate.lock().expect("gate") = Some(gate.clone());
    let first = {
        let service = service.clone();
        let scope = scope.clone();
        tokio::spawn(async move {
            service
                .capture(capture_input(scope, 1), CancellationToken::new())
                .await
        })
    };
    gate.entered.acquire().await.expect("entered").forget();
    let waiting = {
        let service = service.clone();
        let scope = scope.clone();
        tokio::spawn(async move {
            service
                .capture_baseline(scope, 1, CancellationToken::new())
                .await
        })
    };
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    gate.release.add_permits(1);
    assert_eq!(
        first.await.expect("capture")?.status,
        CheckpointStatus::Ready
    );
    waiting.await.expect("baseline")?;
    assert_eq!(backend.captures.load(Ordering::SeqCst), 1);
    let slow = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .capture_baseline(scope, 2, CancellationToken::new())
                .await
        })
    };
    gate.entered.acquire().await.expect("entered").forget();
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    assert!(slow.await.expect("slow baseline").is_err());
    assert_eq!(backend.captures.load(Ordering::SeqCst), 2);
    assert_eq!(backend.refs.lock().expect("refs").len(), 1);
    Ok(())
}

#[tokio::test]
async fn preserves_ready_capture_when_previous_ref_or_diff_lookup_fails() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let backend = Arc::new(Backend::default());
    let service = backend.service();
    let scope = scope(temp.path(), 1);
    service
        .capture_baseline(scope.clone(), 0, CancellationToken::new())
        .await?;
    for mode in [4, 5] {
        backend.mode.store(mode, Ordering::SeqCst);
        let checkpoint = service
            .capture(capture_input(scope.clone(), 1), CancellationToken::new())
            .await?;
        assert_eq!(checkpoint.status, CheckpointStatus::Ready);
        assert!(checkpoint.files.is_empty());
    }
    let mut input = capture_input(scope, 2);
    input.force_missing = true;
    let before = backend.captures.load(Ordering::SeqCst);
    assert_eq!(
        service
            .capture(input, CancellationToken::new())
            .await?
            .status,
        CheckpointStatus::Missing
    );
    assert_eq!(backend.captures.load(Ordering::SeqCst), before);
    Ok(())
}

#[tokio::test]
async fn restore_and_stale_ref_deletion_reject_foreign_scope_and_unready_snapshots() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let backend = Arc::new(Backend::default());
    let service = backend.service();
    let scope = scope(temp.path(), 1);
    let checkpoint = service
        .capture(capture_input(scope.clone(), 1), CancellationToken::new())
        .await?;
    for status in [
        CheckpointStatus::Missing,
        CheckpointStatus::Error,
        CheckpointStatus::Stale,
    ] {
        let mut unavailable = checkpoint.clone();
        unavailable.status = status;
        assert!(
            service
                .restore(scope.clone(), &unavailable, CancellationToken::new())
                .await
                .is_err()
        );
    }
    let mut foreign = checkpoint.clone();
    foreign.scope_id =
        t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(CheckpointScopeId("foreign".into()));
    assert!(
        service
            .restore(scope.clone(), &foreign, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(
        service
            .delete_stale_refs(scope.clone(), vec![foreign])
            .await
            .is_err()
    );
    let mut wrong_ref = checkpoint.clone();
    wrong_ref.r#ref = CheckpointRef("refs/t3/foreign".into());
    assert!(
        service
            .restore(scope.clone(), &wrong_ref, CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
    assert!(backend.deleted.lock().expect("deleted").is_empty());
    service
        .restore(scope.clone(), &checkpoint, CancellationToken::new())
        .await?;
    service
        .delete_stale_refs(scope, vec![checkpoint.clone()])
        .await?;
    assert_eq!(backend.restores.load(Ordering::SeqCst), 1);
    assert_eq!(
        *backend.deleted.lock().expect("deleted"),
        vec![checkpoint.r#ref.0]
    );
    Ok(())
}
