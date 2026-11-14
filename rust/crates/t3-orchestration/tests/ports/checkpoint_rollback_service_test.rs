use crate::checkpoint_service_test::support::*;
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use t3_contracts::provider_instance::ProviderInstanceId;
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::checkpoints_forks::{
        checkpoint_restore_safety::expanded_id, checkpoint_rollback_service::*,
    },
    provider::*,
    testing::{FakeCall, FakeProviderAdapter},
};
use tokio_util::sync::CancellationToken;

#[test]
fn rollback_failure_messages_only_expose_the_policy_ceiling_and_index_lock_diagnosis() {
    for operation in [INDEX_LOCKED_OPERATION, POLICY_CEILING_OPERATION] {
        let error = Error::Service {
            operation,
            detail: "safe diagnosis".into(),
        };
        assert_eq!(rollback_failure_client_message(&error), "safe diagnosis");
    }
    let error = Error::Service {
        operation: "checkpoint.restore",
        detail: "raw git output with internal paths".into(),
    };
    assert_eq!(
        rollback_failure_client_message(&error),
        ROLLBACK_FAILED_MESSAGE
    );
    assert_eq!(
        rollback_failure_client_message(&Error::Cancelled),
        ROLLBACK_FAILED_MESSAGE
    );
}

#[tokio::test]
async fn production_policy_adapter_preserves_ceiling_rejections_and_resolved_workspace()
-> Result<()> {
    let resolver = Arc::new(t3_orchestration::testing::FakeRuntimePolicyResolver::default());
    let policy = ResolvedRollbackPolicy(resolver.clone());
    let source = thread("thread:test", None);
    resolver.resolve.push(Err(Error::Service {
        operation: POLICY_CEILING_OPERATION,
        detail: "Organization policy forbids this runtime mode.".into(),
    }));
    let error = policy.resolve(&source).await.expect_err("ceiling refusal");
    assert_eq!(
        rollback_failure_client_message(&error),
        "Organization policy forbids this runtime mode."
    );
    let (input, selection) = resolver.resolve.next_call().await.expect("policy call");
    assert_eq!(input, source);
    assert_eq!(selection, source.model_selection);
    let mut expected = Policy.resolve(&source).await?;
    expected.cwd = Some(PathBuf::from("resolved-checkout"));
    resolver
        .resolve
        .push(Ok(expected.clone().with_reasoning_effort(None)));
    let resolved = policy.resolve(&source).await?;
    assert_eq!(resolved.cwd, expected.cwd);
    assert_eq!(resolved.runtime_mode, expected.runtime_mode);
    resolver
        .resolve
        .push(Ok(expected.with_reasoning_effort(Some("high".into()))));
    assert!(matches!(
        policy.resolve(&source).await,
        Err(Error::Unsupported("rollback session reasoning policy"))
    ));
    Ok(())
}

#[test]
fn rollback_targets_the_latest_native_turn_of_the_attempt_and_keeps_the_first_tie() -> Result<()> {
    use t3_contracts::orchestration_v2_id_table::IdOrIndex;
    let sample: OrchestrationV2ProviderTurn = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderTurn.jsonl")
            .lines()
            .next()
            .expect("turn fixture"),
    )?;
    let attempt = IdOrIndex::Id(RunAttemptId("attempt:target".into()));
    let turns = [(1, true), (4, true), (99, false), (4, true)]
        .into_iter()
        .enumerate()
        .map(|(index, (ordinal, target))| {
            let mut turn = sample.clone();
            turn.ordinal = ordinal;
            turn.id = IdOrIndex::Id(ProviderTurnId(format!("turn:{index}")));
            turn.run_attempt_id = Some(if target {
                attempt.clone()
            } else {
                IdOrIndex::Id(RunAttemptId("attempt:other".into()))
            });
            turn
        })
        .collect::<Vec<_>>();
    assert_eq!(
        latest_provider_turn_for_attempt(&turns, Some(&attempt)).map(|t| &t.id),
        Some(&turns[1].id)
    );
    assert!(latest_provider_turn_for_attempt(&turns, None).is_none());
    Ok(())
}

pub struct Registry(pub Arc<FakeProviderAdapter>);
#[async_trait]
impl ProviderRegistry for Registry {
    async fn adapter(&self, _instance: ProviderInstanceId) -> Result<Arc<dyn ProviderAdapter>> {
        Ok(self.0.clone())
    }
}
pub struct Sessions {
    pub adapter: Arc<FakeProviderAdapter>,
    pub opens: AtomicUsize,
    pub policies: std::sync::Mutex<Vec<RuntimePolicyOptions>>,
}
#[async_trait]
impl SessionDirectory for Sessions {
    async fn attach_with_policy(
        &self,
        instance: ProviderInstanceId,
        input: OpenSession,
        options: RuntimePolicyOptions,
    ) -> Result<Arc<dyn SessionRuntime>> {
        self.policies.lock().expect("policies").push(options);
        self.attach(instance, input).await
    }

    async fn get(&self, _id: ProviderSessionId) -> Result<Option<Arc<dyn SessionRuntime>>> {
        Ok(Some(self.adapter.runtime.clone()))
    }
    async fn attach(
        &self,
        _instance: ProviderInstanceId,
        _input: OpenSession,
    ) -> Result<Arc<dyn SessionRuntime>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(self.adapter.runtime.clone())
    }
    async fn detach(&self, _id: ProviderSessionId, _revoke: bool) -> Result<()> {
        Err(Error::Unsupported("unused detach"))
    }
    async fn shutdown(&self) -> Result<()> {
        self.adapter.runtime.close().await?;
        Ok(())
    }
}
pub struct Policy;
#[async_trait]
impl RollbackPolicy for Policy {
    async fn resolve(&self, thread: &OrchestrationV2AppThread) -> Result<RuntimePolicy> {
        Ok(RuntimePolicy {
            runtime_mode: thread.runtime_mode,
            interaction_mode: thread.interaction_mode,
            cwd: thread.worktree_path.as_ref().map(|p| PathBuf::from(&p.0)),
            approval_policy: None,
            sandbox_policy: None,
            max_runtime_mode: None,
        })
    }
}
pub struct Recovery;
#[async_trait]
impl RollbackWorkspaceRecovery for Recovery {
    async fn is_managed(&self, _path: &Path) -> Result<bool> {
        Ok(false)
    }
    async fn recreate(
        &self,
        _thread: &OrchestrationV2AppThread,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Err(Error::Unsupported("unused recreate"))
    }
}

struct BrokenRecovery {
    expected: PathBuf,
    cancelled: bool,
}
#[async_trait]
impl RollbackWorkspaceRecovery for BrokenRecovery {
    async fn is_managed(&self, path: &Path) -> Result<bool> {
        Ok(path == self.expected)
    }
    async fn recreate(
        &self,
        _thread: &OrchestrationV2AppThread,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        std::fs::create_dir(&self.expected)?;
        std::fs::write(self.expected.join("partial-checkout"), "partial")?;
        Err(if self.cancelled {
            Error::Cancelled
        } else {
            Error::Unsupported("failed checkout")
        })
    }
}

#[tokio::test]
async fn failed_or_cancelled_checkout_restores_the_set_aside_user_folder() -> Result<()> {
    let parent = tempfile::tempdir()?;
    for cancelled in [false, true] {
        let cwd = parent
            .path()
            .join(if cancelled { "cancelled" } else { "failed" });
        std::fs::create_dir(&cwd)?;
        std::fs::write(cwd.join("user-work"), "preserve this work")?;
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        h.seed("thread.created", thread("thread:test", Some(&cwd)))
            .await?;
        let scope = h.seed_run(&cwd, 1, "completed").await?;
        let mut checkpoint = backend
            .service()
            .materialize_baseline(capture_input(scope.clone(), 0))
            .await?;
        checkpoint.status = CheckpointStatus::Ready;
        h.seed("checkpoint.captured", &checkpoint).await?;
        let (mut service, sessions, _calls) = service(&h, &backend, true);
        service.workspace_recovery = Arc::new(BrokenRecovery {
            expected: cwd.clone(),
            cancelled,
        });
        assert!(
            service
                .execute(
                    request(&scope, &checkpoint, true)?,
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("user-work"))?,
            "preserve this work"
        );
        assert!(!cwd.join("partial-checkout").exists());
        assert_eq!(sessions.opens.load(Ordering::SeqCst), 0);
        assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
    }
    Ok(())
}
pub fn service(
    h: &Harness,
    backend: &Arc<Backend>,
    capable: bool,
) -> (
    CheckpointRollbackService,
    Arc<Sessions>,
    tokio::sync::mpsc::UnboundedReceiver<FakeCall>,
) {
    let mut session = session();
    session.capabilities.threads.can_rollback_thread = capable;
    session
        .capabilities
        .checkpointing
        .provider_can_rollback_conversation = capable;
    let snapshot = ThreadSnapshot {
        provider_thread: provider(),
        provider_turns: vec![],
        messages: vec![],
        reasoning_message_ids: None,
        runtime_requests: vec![],
        provider_payload: None,
    };
    let (adapter, calls) = FakeProviderAdapter::new(
        ProviderInstanceId("codex".into()),
        t3_contracts::provider_instance::ProviderDriverKind("codex".into()),
        session.capabilities.clone(),
        session,
        snapshot,
    );
    let adapter = Arc::new(adapter);
    let sessions = Arc::new(Sessions {
        adapter: adapter.clone(),
        opens: AtomicUsize::new(0),
        policies: Default::default(),
    });
    (
        CheckpointRollbackService {
            command_lanes: Arc::new(Default::default()),
            checkpoints: backend.service(),
            projections: h.reader.clone(),
            sessions: sessions.clone(),
            providers: Arc::new(Registry(adapter)),
            policy: Arc::new(Policy),
            workspace_recovery: Arc::new(Recovery),
            events: h.sink.clone(),
            database: h.db.stores.clone(),
            ids: h.ids.clone(),
            clock: h.clock.clone(),
            writers: Arc::new(t3_orchestration::composition::NoHostWriters),
            agent_scope: Arc::new(NoAgentScope),
        },
        sessions,
        calls,
    )
}
/// A host that cannot see the agent's processes.
pub struct NoAgentScope;
impl AgentScope for NoAgentScope {}

pub fn request(
    scope: &OrchestrationV2CheckpointScope,
    checkpoint: &OrchestrationV2Checkpoint,
    restore_files: bool,
) -> Result<RollbackRequest> {
    Ok(RollbackRequest {
        thread_id: ThreadId("thread:test".into()),
        provider_thread_id: ProviderThreadId("provider:test".into()),
        checkpoint_id: expanded_id(&checkpoint.id)?,
        scope_id: expanded_id(&scope.id)?,
        restore_files,
    })
}

#[tokio::test]
async fn failed_native_rollback_never_restores_files_or_deletes_refs() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    h.seed("thread.created", thread("thread:test", Some(temp.path())))
        .await?;
    let scope = h.seed_run(temp.path(), 1, "completed").await?;
    let mut checkpoint = backend
        .service()
        .materialize_baseline(capture_input(scope.clone(), 0))
        .await?;
    checkpoint.status = CheckpointStatus::Ready;
    h.seed("checkpoint.captured", &checkpoint).await?;
    let (service, sessions, calls) = service(&h, &backend, true);
    drop(calls);
    assert!(
        service
            .execute(
                request(&scope, &checkpoint, true)?,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(sessions.opens.load(Ordering::SeqCst), 1);
    assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
    assert!(backend.deleted.lock().expect("deleted").is_empty());
    assert_eq!(
        h.reader
            .run(ThreadId("thread:test".into()), RunId("run:1".into()))
            .await?
            .expect("run")
            .status,
        OrchestrationV2RunStatus::Completed
    );
    Ok(())
}

struct RecreateFolder(PathBuf);
#[async_trait]
impl RollbackWorkspaceRecovery for RecreateFolder {
    async fn is_managed(&self, path: &Path) -> Result<bool> {
        Ok(path == self.0)
    }
    async fn recreate(
        &self,
        _thread: &OrchestrationV2AppThread,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        std::fs::create_dir_all(self.0.join(".git"))?;
        Ok(())
    }
}

#[tokio::test]
async fn successful_recreation_discards_leftover_before_a_later_provider_failure() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let cwd = temp.path().join("checkout");
    std::fs::create_dir(&cwd)?;
    std::fs::write(cwd.join("unsaved.txt"), "leftover user work")?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    h.seed("thread.created", thread("thread:test", Some(&cwd)))
        .await?;
    let scope = h.seed_run(&cwd, 1, "completed").await?;
    let mut checkpoint = backend
        .service()
        .materialize_baseline(capture_input(scope.clone(), 0))
        .await?;
    checkpoint.status = CheckpointStatus::Ready;
    h.seed("checkpoint.captured", &checkpoint).await?;
    let (mut service, _, calls) = service(&h, &backend, true);
    service.workspace_recovery = Arc::new(RecreateFolder(cwd.clone()));
    drop(calls);
    assert!(
        service
            .execute(
                request(&scope, &checkpoint, true)?,
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(cwd.join(".git").is_dir());
    // TypeScript discards the backup on recreation, before the provider confirms rollback.
    assert!(!cwd.join("unsaved.txt").exists());
    assert_eq!(std::fs::read_dir(temp.path())?.count(), 1);
    assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
    assert_eq!(
        h.reader
            .run(ThreadId("thread:test".into()), RunId("run:1".into()))
            .await?
            .expect("run")
            .status,
        OrchestrationV2RunStatus::Completed
    );
    Ok(())
}

struct BlockingPolicy(Arc<Gate>);
#[async_trait]
impl RollbackPolicy for BlockingPolicy {
    async fn resolve(&self, thread: &OrchestrationV2AppThread) -> Result<RuntimePolicy> {
        self.0.entered.add_permits(1);
        self.0
            .release
            .acquire()
            .await
            .expect("policy release")
            .forget();
        Policy.resolve(thread).await
    }
}

#[tokio::test]
async fn real_cleanup_and_rollback_races_hold_the_workspace_lease_in_both_orders() -> Result<()> {
    use t3_orchestration::ports::checkpoints_forks::checkpoint_service::WorkspaceLocks;
    for cleanup_first in [true, false] {
        let temp = tempfile::tempdir()?;
        let cwd = temp.path().join("checkout");
        std::fs::create_dir_all(cwd.join(".git"))?;
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        h.seed("thread.created", thread("thread:test", Some(&cwd)))
            .await?;
        let scope = h.seed_run(&cwd, 1, "completed").await?;
        let mut checkpoint = backend
            .service()
            .materialize_baseline(capture_input(scope.clone(), 0))
            .await?;
        checkpoint.status = CheckpointStatus::Ready;
        h.seed("checkpoint.captured", &checkpoint).await?;
        let (mut service, sessions, _calls) = service(&h, &backend, true);
        service.workspace_recovery = Arc::new(RecreateFolder(cwd.clone()));
        let policy_gate = Arc::new(Gate::default());
        service.policy = Arc::new(BlockingPolicy(policy_gate.clone()));
        let input = request(&scope, &checkpoint, true)?;
        // Ready SQLite snapshots make the first Pending poll identify the held lease, not a database read.
        service.projections = Arc::new(Reader(
            h.db.stores.clone(),
            Some(ReadyRollback {
                context: h
                    .reader
                    .checkpoint_rollback(
                        input.thread_id.clone(),
                        input.provider_thread_id.clone(),
                        input.checkpoint_id.clone(),
                        input.scope_id.clone(),
                    )
                    .await?,
                shells: h.reader.shell_snapshot(None, None).await?,
            }),
        ));
        let service = Arc::new(service);
        let leases = WorkspaceLocks::workspace_leases();
        let (entered, acquired) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let (blocked, waiting) = tokio::sync::oneshot::channel();
        let (rollback, cleanup) = if cleanup_first {
            let cleanup = {
                let cwd = cwd.clone();
                tokio::spawn(async move {
                    let _lease = leases.acquire(&cwd).await?;
                    entered.send(()).expect("cleanup entered");
                    released.await.expect("cleanup release");
                    std::fs::remove_dir_all(&cwd)?;
                    Result::Ok(())
                })
            };
            acquired.await.expect("cleanup acquired lease");
            let rollback = tokio::spawn(async move {
                let execute = service.execute(input, CancellationToken::new());
                tokio::pin!(execute);
                std::future::poll_fn(|cx| {
                    assert!(execute.as_mut().poll(cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                blocked.send(()).expect("rollback waiting");
                execute.await
            });
            waiting.await.expect("rollback waiting on cleanup");
            assert_eq!(sessions.opens.load(Ordering::SeqCst), 0);
            assert_eq!(policy_gate.entered.available_permits(), 0);
            release.send(()).expect("cleanup release");
            policy_gate
                .entered
                .acquire()
                .await
                .expect("rollback entered")
                .forget();
            policy_gate.release.add_permits(1);
            (rollback, cleanup)
        } else {
            let rollback =
                tokio::spawn(async move { service.execute(input, CancellationToken::new()).await });
            policy_gate
                .entered
                .acquire()
                .await
                .expect("rollback holds lease")
                .forget();
            let cleanup = {
                let cwd = cwd.clone();
                tokio::spawn(async move {
                    let acquire = leases.acquire(&cwd);
                    tokio::pin!(acquire);
                    std::future::poll_fn(|cx| {
                        assert!(acquire.as_mut().poll(cx).is_pending());
                        std::task::Poll::Ready(())
                    })
                    .await;
                    blocked.send(()).expect("cleanup waiting");
                    let _lease = acquire.await?;
                    std::fs::remove_dir_all(&cwd)?;
                    Result::Ok(())
                })
            };
            waiting.await.expect("cleanup waiting on rollback");
            policy_gate.release.add_permits(1);
            (rollback, cleanup)
        };
        rollback.await.expect("rollback task")?;
        cleanup.await.expect("cleanup task")?;
        assert_eq!(backend.restores.load(Ordering::SeqCst), 1);
        assert_eq!(cwd.exists(), cleanup_first);
    }
    Ok(())
}

#[tokio::test]
async fn real_turn_start_race_waits_through_restore_but_not_conversation_only_rollback()
-> Result<()> {
    use t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::{
        ReservationKind, WorkspaceReservations,
    };
    for restore_files in [true, false] {
        let temp = tempfile::tempdir()?;
        let cwd = temp.path().join("checkout");
        std::fs::create_dir_all(cwd.join(".git"))?;
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        h.seed("thread.created", thread("thread:test", Some(&cwd)))
            .await?;
        let scope = h.seed_run(&cwd, 1, "completed").await?;
        let mut checkpoint = backend
            .service()
            .materialize_baseline(capture_input(scope.clone(), 0))
            .await?;
        checkpoint.status = CheckpointStatus::Ready;
        h.seed("checkpoint.captured", &checkpoint).await?;
        let (mut service, _, _calls) = service(&h, &backend, true);
        let policy = Arc::new(Gate::default());
        let restore = Arc::new(Gate::default());
        *backend.restore_gate.lock().expect("restore gate") = Some(restore.clone());
        service.policy = Arc::new(BlockingPolicy(policy.clone()));
        let input = request(&scope, &checkpoint, restore_files)?;
        let rollback =
            tokio::spawn(async move { service.execute(input, CancellationToken::new()).await });
        policy
            .entered
            .acquire()
            .await
            .expect("policy entered")
            .forget();
        let (blocked, waiting) = tokio::sync::oneshot::channel();
        let (entered, started) = tokio::sync::oneshot::channel();
        let start = tokio::spawn(async move {
            let token = CancellationToken::new();
            let reservations = WorkspaceReservations::global();
            let nested = cwd.join("packages/app");
            let reserve = reservations.reserve(&nested, ReservationKind::TurnStart, &token);
            tokio::pin!(reserve);
            if restore_files {
                std::future::poll_fn(|cx| {
                    assert!(reserve.as_mut().poll(cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                blocked.send(()).expect("start waiting");
            }
            let _reservation = reserve.await?;
            entered.send(()).expect("start entered");
            Result::Ok(())
        });
        if restore_files {
            waiting.await.expect("start blocked by rollback");
            policy.release.add_permits(1);
            restore.entered.acquire().await.expect("restoring").forget();
            assert!(!start.is_finished());
            restore.release.add_permits(1);
            started.await.expect("start after restore");
        } else {
            started.await.expect("conversation rewind permits starts");
            policy.release.add_permits(1);
        }
        rollback.await.expect("rollback")?;
        start.await.expect("turn start")?;
        assert_eq!(
            backend.restores.load(Ordering::SeqCst),
            usize::from(restore_files)
        );
    }
    Ok(())
}

async fn seed_history(
    h: &Harness,
    backend: &Arc<Backend>,
    cwd: &Path,
) -> Result<(
    OrchestrationV2CheckpointScope,
    Vec<OrchestrationV2Checkpoint>,
)> {
    use serde_json::json;
    use t3_contracts::orchestration_v2_id_table::IdOrIndex;
    h.seed("thread.created", thread("thread:test", Some(cwd)))
        .await?;
    let scope = h.seed_run(cwd, 1, "completed").await?;
    for (index, status) in [
        "completed",
        "completed",
        "completed",
        "interrupted",
        "failed",
        "cancelled",
        "rolled_back",
    ]
    .iter()
    .enumerate()
    {
        let ordinal = index as u64 + 1;
        let mut run = run(ordinal, status);
        run.active_attempt_id = Some(IdOrIndex::Id(RunAttemptId(format!("attempt:{ordinal}"))));
        h.seed("run.created", run).await?;
        h.seed("node.updated", node(ordinal, &scope, status))
            .await?;
        for (attempt_id, turn_ordinal, attempt_ordinal) in if ordinal == 7 {
            vec![("6.5".to_owned(), 13, 1), ("7".to_owned(), 14, 2)]
        } else {
            vec![(ordinal.to_string(), ordinal * 2, 1)]
        } {
            h.seed("run-attempt.created",json!({"id":format!("attempt:{attempt_id}"),"runId":format!("run:{ordinal}"),"attemptOrdinal":attempt_ordinal,"rootNodeId":format!("node:{ordinal}"),"providerInstanceId":"codex","providerThreadId":"provider:test","providerTurnId":format!("turn:{attempt_id}"),"reason":"initial","status":"completed","startedAt":time(),"completedAt":time()})).await?;
            h.seed("provider-turn.updated",json!({"id":format!("turn:{attempt_id}"),"providerThreadId":"provider:test","nodeId":format!("node:{ordinal}"),"runAttemptId":format!("attempt:{attempt_id}"),"nativeTurnRef":{"driver":"codex","nativeId":format!("native:{attempt_id}"),"strength":"strong"},"ordinal":turn_ordinal,"status":if *status=="rolled_back" {"completed"}else{status},"startedAt":time(),"completedAt":time()})).await?;
        }
    }
    h.seed("provider-turn.updated",json!({"id":"turn:2.5","providerThreadId":"provider:test","nodeId":"node:2","runAttemptId":"attempt:2","nativeTurnRef":{"driver":"codex","nativeId":"native:2-continuation","strength":"strong"},"ordinal":5,"status":"completed","startedAt":time(),"completedAt":time()})).await?;
    let mut checkpoints = vec![];
    let other = {
        let mut other = scope.clone();
        other.id = IdOrIndex::Id(CheckpointScopeId("scope:other".into()));
        other
    };
    h.seed("checkpoint-scope.created", &other).await?;
    for ordinal in 0..=8 {
        let input_scope = if ordinal == 8 {
            other.clone()
        } else {
            scope.clone()
        };
        let mut checkpoint = backend
            .service()
            .capture(
                capture_input(input_scope, ordinal),
                CancellationToken::new(),
            )
            .await?;
        if ordinal == 0 {
            checkpoint.run_id = None;
            checkpoint.parent_checkpoint_id = None;
        }
        if ordinal == 7 {
            checkpoint.status = CheckpointStatus::Stale;
        }
        h.seed("checkpoint.captured", &checkpoint).await?;
        checkpoints.push(checkpoint);
    }
    Ok((scope, checkpoints))
}

#[tokio::test]
async fn repeated_rollbacks_exclude_all_discarded_attempts_and_use_latest_target_continuation()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let (scope, checkpoints) = seed_history(&h, &backend, temp.path()).await?;
    let (service, _, mut calls) = service(&h, &backend, true);
    let mut counts = vec![];
    let mut terminal_updates = vec![];
    let mut observed = h.sink.subscribe();
    for ordinal in [5, 2, 0, 0] {
        service
            .execute(
                request(&scope, &checkpoints[ordinal], true)?,
                CancellationToken::new(),
            )
            .await?;
        if ordinal != 0 || counts.len() < 3 {
            let Some(FakeCall::Rollback(input)) = calls.recv().await else {
                panic!("native rollback")
            };
            let boundary = match &input.target {
                RollbackTarget::ThreadStart { .. } => 0,
                RollbackTarget::ProviderTurn { provider_turn, .. } => provider_turn.ordinal,
            };
            if ordinal == 2 {
                assert_eq!(boundary, 5);
            }
            counts.push(
                input
                    .provider_thread_turns
                    .iter()
                    .filter(|t| t.ordinal > boundary)
                    .count(),
            );
            assert!(
                input
                    .provider_thread_turns
                    .iter()
                    .all(|t| t.ordinal != 13 && t.ordinal != 14)
            );
        } else {
            assert!(calls.try_recv().is_err());
        }
        while let Ok(t3_contracts::application_event::ApplicationStoredEvent::Orchestration(
            stored,
        )) = observed.try_recv()
        {
            if let OrchestrationV2DomainEvent::RunUpdated(event) = stored.event {
                terminal_updates.push(expanded_id(&event.payload.id)?);
            }
        }
    }
    assert_eq!(counts, vec![1, 3, 3]);
    terminal_updates.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        terminal_updates,
        (1..=6)
            .map(|n| RunId(format!("run:{n}")))
            .collect::<Vec<_>>()
    );
    assert_eq!(backend.deleted.lock().expect("deleted").len(), 6);
    let records = h
        .reader
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::Runs, RecordKind::Nodes, RecordKind::Checkpoints],
            RecordFilter::default(),
        )
        .await?;
    let ProjectionRecordSet::Runs(runs) = &records.records[0] else {
        panic!("runs")
    };
    assert!(
        runs.iter()
            .all(|r| r.status == OrchestrationV2RunStatus::RolledBack)
    );
    let ProjectionRecordSet::Nodes(nodes) = &records.records[1] else {
        panic!("nodes")
    };
    assert!(
        nodes
            .iter()
            .all(|n| n.status == ExecutionNodeStatus::RolledBack)
    );
    let ProjectionRecordSet::Checkpoints(cps) = &records.records[2] else {
        panic!("checkpoints")
    };
    assert_eq!(
        cps.iter()
            .filter(|cp| cp.status == CheckpointStatus::Ready)
            .map(|cp| cp.id.clone())
            .collect::<Vec<_>>(),
        vec![checkpoints[0].id.clone(), checkpoints[8].id.clone()]
    );
    Ok(())
}

#[tokio::test]
async fn rollback_history_reads_do_not_decode_obsolete_unrelated_payloads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let (scope, checkpoints) = seed_history(&h, &backend, temp.path()).await?;
    h.db.stores.transaction(|db|{
        db.execute("UPDATE orchestration_v2_projection_runs SET payload_json='{}' WHERE run_id='run:1'",[])?;
        db.execute("UPDATE orchestration_v2_projection_nodes SET payload_json='{}' WHERE node_id='node:1'",[])?;
        db.execute("UPDATE orchestration_v2_projection_checkpoints SET payload_json=json_set(payload_json,'$.files','obsolete') WHERE ordinal_within_scope IN (0,1,7,8)",[])?;Ok(())
    }).await?;
    assert!(
        h.reader
            .projection(ThreadId("thread:test".into()))
            .await
            .is_err()
    );
    let (service, _, mut calls) = service(&h, &backend, true);
    service
        .execute(
            request(&scope, &checkpoints[2], true)?,
            CancellationToken::new(),
        )
        .await?;
    let Some(FakeCall::Rollback(input)) = calls.recv().await else {
        panic!("rollback")
    };
    assert_eq!(
        input
            .provider_thread_turns
            .iter()
            .filter(|t| t.ordinal > 5)
            .count(),
        4
    );
    let statuses = h
        .db
        .stores
        .read(|db| {
            let mut stmt =
                db.prepare("SELECT status FROM orchestration_v2_projection_runs ORDER BY ordinal")?;
            Ok(stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .await?;
    assert_eq!(
        statuses,
        vec![
            "completed",
            "completed",
            "rolled_back",
            "rolled_back",
            "rolled_back",
            "rolled_back",
            "rolled_back"
        ]
    );
    Ok(())
}

#[tokio::test]
async fn unsupported_provider_is_rejected_before_recovery_open_or_filesystem_restore() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    h.seed("thread.created", thread("thread:test", Some(temp.path())))
        .await?;
    let scope = h.seed_run(temp.path(), 1, "completed").await?;
    let checkpoint = backend
        .service()
        .materialize_baseline(capture_input(scope.clone(), 0))
        .await?;
    let mut checkpoint = checkpoint;
    checkpoint.status = CheckpointStatus::Ready;
    h.seed("checkpoint.captured", &checkpoint).await?;
    let (service, sessions, mut calls) = service(&h, &backend, false);
    assert!(matches!(
        service
            .execute(
                request(&scope, &checkpoint, true)?,
                CancellationToken::new()
            )
            .await,
        Err(Error::Unsupported("provider conversation rollback"))
    ));
    assert_eq!(sessions.opens.load(Ordering::SeqCst), 0);
    assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
    assert!(calls.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn invalid_or_changed_targets_are_rejected_before_native_or_git_operations() -> Result<()> {
    let temp = tempfile::tempdir()?;
    for status in [
        CheckpointStatus::Missing,
        CheckpointStatus::Error,
        CheckpointStatus::Stale,
    ] {
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        let scope = h.seed_run(temp.path(), 1, "completed").await?;
        let mut checkpoint = backend
            .service()
            .materialize_baseline(capture_input(scope.clone(), 0))
            .await?;
        checkpoint.status = status;
        h.seed("checkpoint.captured", &checkpoint).await?;
        let (service, sessions, mut calls) = service(&h, &backend, true);
        assert!(
            service
                .execute(
                    request(&scope, &checkpoint, true)?,
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(sessions.opens.load(Ordering::SeqCst), 0);
        assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
        assert!(calls.try_recv().is_err());
    }
    Ok(())
}

#[tokio::test]
async fn rollback_marks_later_runs_nodes_and_checkpoints() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    h.seed("thread.created", thread("thread:test", Some(temp.path())))
        .await?;
    let scope = h.seed_run(temp.path(), 1, "completed").await?;
    h.seed_run(temp.path(), 2, "cancelled").await?;
    let service_checkpoints = backend.service();
    service_checkpoints
        .capture_baseline(scope.clone(), 0, CancellationToken::new())
        .await?;
    let baseline = service_checkpoints
        .materialize_baseline(capture_input(scope.clone(), 0))
        .await?;
    h.seed("checkpoint.captured", &baseline).await?;
    for ordinal in [1, 2] {
        let captured = service_checkpoints
            .capture(
                capture_input(scope.clone(), ordinal),
                CancellationToken::new(),
            )
            .await?;
        h.seed("checkpoint.captured", captured).await?;
    }
    let (mut service, sessions, mut calls) = service(&h, &backend, true);
    let resolver = Arc::new(t3_orchestration::testing::FakeRuntimePolicyResolver::default());
    let configured = Policy
        .resolve(&thread("thread:test", Some(temp.path())))
        .await?
        .with_reasoning_effort(Some("high".into()));
    resolver.resolve.push(Ok(configured));
    service.policy = Arc::new(ResolvedRollbackPolicy(resolver));
    let mut events = h.sink.subscribe();
    service
        .execute(request(&scope, &baseline, true)?, CancellationToken::new())
        .await?;
    events.recv().await.expect("committed rollback");
    assert_eq!(
        sessions.policies.lock().expect("policies")[0]
            .reasoning_effort
            .as_deref(),
        Some("high")
    );
    assert_eq!(sessions.opens.load(Ordering::SeqCst), 1);
    assert!(matches!(calls.recv().await, Some(FakeCall::Rollback(_))));
    assert_eq!(backend.restores.load(Ordering::SeqCst), 1);
    assert_eq!(backend.deleted.lock().expect("deleted").len(), 2);
    for ordinal in [1, 2] {
        let run = h
            .reader
            .run(
                ThreadId("thread:test".into()),
                RunId(format!("run:{ordinal}")),
            )
            .await?
            .expect("run");
        assert_eq!(run.status, OrchestrationV2RunStatus::RolledBack);
    }
    let records = h
        .reader
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::Nodes, RecordKind::Checkpoints],
            RecordFilter::default(),
        )
        .await?;
    let ProjectionRecordSet::Nodes(nodes) = &records.records[0] else {
        panic!("nodes")
    };
    assert!(
        nodes
            .iter()
            .all(|n| n.status == ExecutionNodeStatus::RolledBack)
    );
    let ProjectionRecordSet::Checkpoints(checkpoints) = &records.records[1] else {
        panic!("checkpoints")
    };
    assert_eq!(
        checkpoints
            .iter()
            .filter(|c| c.status == CheckpointStatus::Stale)
            .count(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn overlapping_owner_blocks_file_restore_and_conversation_only_rewind_leaves_files()
-> Result<()> {
    let parent = tempfile::tempdir()?;
    let cwd = parent.path().join("worktree");
    std::fs::create_dir(&cwd)?;
    let file = cwd.join("other.txt");
    std::fs::write(&file, "other work")?;
    for restore_files in [true, false] {
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        h.seed("thread.created", thread("thread:test", Some(&cwd)))
            .await?;
        h.seed(
            "thread.created",
            thread("thread:other", Some(parent.path())),
        )
        .await?;
        let scope = h.seed_run(&cwd, 1, "completed").await?;
        let mut checkpoint = backend
            .service()
            .materialize_baseline(capture_input(scope.clone(), 0))
            .await?;
        checkpoint.status = if restore_files {
            CheckpointStatus::Ready
        } else {
            CheckpointStatus::Missing
        };
        h.seed("checkpoint.captured", &checkpoint).await?;
        let (service, sessions, mut calls) = service(&h, &backend, true);
        let result = service
            .execute(
                request(&scope, &checkpoint, restore_files)?,
                CancellationToken::new(),
            )
            .await;
        assert_eq!(result.is_ok(), !restore_files);
        assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
        assert_eq!(
            sessions.opens.load(Ordering::SeqCst),
            usize::from(!restore_files)
        );
        assert_eq!(std::fs::read_to_string(&file)?, "other work");
        if !restore_files {
            assert!(matches!(calls.recv().await, Some(FakeCall::Rollback(_))));
        }
    }
    Ok(())
}

#[tokio::test]
async fn composed_rollback_reaches_restore_while_holding_the_workspace_lease() -> Result<()> {
    use t3_orchestration::{
        composition::{OrchestrationRuntime, RuntimePorts, StaticSettings},
        ports::{
            checkpoints_forks::checkpoint_service::WorkspaceLocks,
            sessions_recovery::provider_adapter_registry::AdapterRegistry,
        },
    };
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let mut project = serde_json::to_value(t3_orchestration::testing::project_event(
        "composed:project",
        "project:test",
    )?)?;
    project["payload"]["workspaceRoot"] = serde_json::json!(temp.path());
    let project = serde_json::from_value(project)?;
    h.db.stores
        .transaction(move |db| t3_orchestration::stores::projects::apply(db, &project))
        .await?;
    h.seed("thread.created", thread("thread:test", Some(temp.path())))
        .await?;
    let scope = h.seed_run(temp.path(), 1, "completed").await?;
    let backend = Arc::new(Backend::default());
    backend
        .service()
        .capture_baseline(scope.clone(), 0, CancellationToken::new())
        .await?;
    let baseline = backend
        .service()
        .materialize_baseline(capture_input(scope.clone(), 0))
        .await?;
    assert_eq!(baseline.status, CheckpointStatus::Ready);
    h.seed("checkpoint.captured", &baseline).await?;
    let (_, sessions, mut calls) = service(&h, &backend, true);
    sessions
        .adapter
        .open_session_with_policy
        .push(Ok(sessions.adapter.runtime.clone()));
    let registry = AdapterRegistry::from_adapters(vec![sessions.adapter.clone()]);
    let mut ports = RuntimePorts::new(
        h.db.stores.clone(),
        Arc::new(registry),
        backend.clone(),
        h.clock.clone(),
    );
    ports.ids = h.ids.clone();
    ports.rollback_workspaces = Some(Arc::new(Recovery));
    let mut settings = serde_json::to_value(t3_contracts::settings::ServerSettings::default())?;
    settings["providerInstances"]["codex"] = serde_json::json!({"driver":"codex","enabled":true});
    ports.settings = Arc::new(StaticSettings::new(serde_json::from_value(settings)?));
    ports.worker.max_attempts = 1;
    let runtime = OrchestrationRuntime::start(ports).await?;
    let gate = Arc::new(Gate::default());
    *backend.restore_gate.lock().expect("restore gate") = Some(gate.clone());
    runtime.dispatch_command(serde_json::from_value(serde_json::json!({"type":"checkpoint.rollback","commandId":"composed:rollback","threadId":"thread:test","checkpointId":baseline.id,"scopeId":scope.id,"restoreFiles":true}))?).await?;
    let worker = runtime.effect_worker();
    let drain = worker.drain(64);
    tokio::pin!(drain);
    tokio::select! {
        biased;
        permit=gate.entered.acquire()=>permit.expect("restore reached").forget(),
        result=&mut drain=>{result?;
            let failures=h.db.stores.read(|db|{
                let mut query=db.prepare("SELECT status,last_error,payload_json FROM orchestration_v2_effect_outbox")?;
                let rows=query.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;Ok(rows)
            }).await?;
            panic!("composed rollback ended before backend restore; calls={:?}; outbox={failures:?}",calls.try_recv());},
    }
    let leases = WorkspaceLocks::workspace_leases();
    let mut lease = Box::pin(leases.acquire(temp.path()));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(lease.as_mut().poll(cx).is_pending()))
            .await,
        "composed rollback must retain its workspace lease during restore"
    );
    gate.release.add_permits(1);
    drain.await?;
    drop(lease.await?);
    assert_eq!(backend.restores.load(Ordering::SeqCst), 1);
    assert!(
        std::iter::from_fn(|| calls.try_recv().ok())
            .any(|call| matches!(call, FakeCall::Rollback(_)))
    );
    assert_eq!(
        runtime
            .projections
            .run(ThreadId("thread:test".into()), RunId("run:1".into()))
            .await?
            .expect("rolled back run")
            .status,
        OrchestrationV2RunStatus::RolledBack
    );
    eprintln!(
        "composed rollback: native rollback -> snapshot restore while workspace lease held -> durable rolled_back event -> lease released"
    );
    runtime.shutdown().await?;
    Ok(())
}

/// Host writers for one thread: `live` names those still running; while a hold is out, a writer
/// that tries to start is refused, and it records whether it ever wrote during a restore.
#[derive(Default)]
struct Writers {
    live: Vec<String>,
    held: Arc<std::sync::atomic::AtomicBool>,
}
impl Writers {
    /// A writer loop's next step: it writes only while no restore holds the thread.
    fn try_write(&self) -> bool {
        !self.held.load(Ordering::SeqCst)
    }
}
struct Release(Arc<std::sync::atomic::AtomicBool>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
#[async_trait]
impl ThreadWriters for Writers {
    async fn hold(&self, _thread_id: ThreadId, _cwd: PathBuf) -> Result<WriterHold> {
        self.held.store(true, Ordering::SeqCst);
        Ok(WriterHold {
            running: self.live.clone(),
            guard: Box::new(Release(self.held.clone())),
        })
    }
}

async fn restorable(
    h: &Harness,
    backend: &Arc<Backend>,
    cwd: &Path,
) -> Result<(OrchestrationV2CheckpointScope, OrchestrationV2Checkpoint)> {
    h.seed("thread.created", thread("thread:test", Some(cwd)))
        .await?;
    let scope = h.seed_run(cwd, 1, "completed").await?;
    let mut checkpoint = backend
        .service()
        .materialize_baseline(capture_input(scope.clone(), 0))
        .await?;
    checkpoint.status = CheckpointStatus::Ready;
    h.seed("checkpoint.captured", &checkpoint).await?;
    Ok((scope, checkpoint))
}

/// Review t3-git H-2: a file restore never runs under a live writer of its thread. A host
/// terminal, a provider command and a running turn each refuse it by name, before Git or the
/// provider's conversation rollback is called.
#[tokio::test]
async fn a_restore_refuses_while_the_thread_still_writes_to_its_folder() -> Result<()> {
    for writer in ["terminal", "command", "turn"] {
        let temp = tempfile::tempdir()?;
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        let (scope, checkpoint) = restorable(&h, &backend, temp.path()).await?;
        let writers = Arc::new(Writers {
            live: if writer == "terminal" {
                vec!["terminal `npm run dev`".into()]
            } else {
                vec![]
            },
            ..Default::default()
        });
        match writer {
            "command" => {
                h.seed("turn-item.updated", serde_json::json!({"id":"command:watch","threadId":"thread:test","runId":"run:1","nodeId":"node:1","providerThreadId":"provider:test","providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":"cargo watch","startedAt":time(),"completedAt":null,"updatedAt":time(),"type":"command_execution","input":"cargo watch"}))
                    .await?;
            }
            "turn" => {
                h.seed_run(temp.path(), 2, "running").await?;
            }
            _ => {}
        }
        let (mut service, _, mut calls) = service(&h, &backend, true);
        service.writers = writers.clone();
        let error = service
            .execute(
                request(&scope, &checkpoint, true)?,
                CancellationToken::new(),
            )
            .await
            .expect_err("a live writer refuses the restore");
        let named = match writer {
            "terminal" => "terminal `npm run dev`",
            "command" => "command `cargo watch`",
            _ => "a running turn",
        };
        assert!(
            rollback_failure_client_message(&error).contains(named),
            "{writer}: {error}"
        );
        assert_eq!(backend.restores.load(Ordering::SeqCst), 0, "{writer}");
        while let Ok(call) = calls.try_recv() {
            assert!(
                !matches!(call, FakeCall::Rollback(_)),
                "{writer}: a refused restore leaves the provider's conversation as it was"
            );
        }
        assert!(
            writers.try_write(),
            "{writer}: the refusal releases the hold"
        );
    }
    Ok(())
}

/// The reviewer's race through the service, without timing: a writer loop tries to write once
/// Git has started restoring, and is held off until the restore returns.
#[tokio::test]
async fn a_writer_cannot_start_while_a_restore_runs() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let (scope, checkpoint) = restorable(&h, &backend, temp.path()).await?;
    let gate = Arc::new(Gate::default());
    *backend.restore_gate.lock().expect("restore gate") = Some(gate.clone());
    let writers = Arc::new(Writers::default());
    let (mut service, _, _calls) = service(&h, &backend, true);
    service.writers = writers.clone();
    let rollback = tokio::spawn(async move {
        service
            .execute(
                request(&scope, &checkpoint, true)?,
                CancellationToken::new(),
            )
            .await
    });
    gate.entered
        .acquire()
        .await
        .expect("restore started")
        .forget();
    assert!(
        !writers.try_write(),
        "no writer of the thread may start while Git restores"
    );
    gate.release.add_permits(1);
    rollback.await.expect("rollback task")?;
    assert_eq!(backend.restores.load(Ordering::SeqCst), 1);
    assert!(
        writers.try_write(),
        "the hold ends when the restore returns"
    );
    Ok(())
}

/// The composed runtime over the fake provider and `backend`, with `thread:test` on a worktree at
/// `cwd` (project root `root`) and its baseline checkpoint.
async fn composed(
    h: &Harness,
    root: &Path,
    cwd: &Path,
    backend: &Arc<Backend>,
    recovery: Arc<dyn RollbackWorkspaceRecovery>,
    other: Option<Arc<FakeProviderAdapter>>,
) -> Result<(
    t3_orchestration::composition::OrchestrationRuntime,
    Arc<FakeProviderAdapter>,
    OrchestrationV2CheckpointScope,
    OrchestrationV2Checkpoint,
    tokio::sync::mpsc::UnboundedReceiver<FakeCall>,
)> {
    use t3_orchestration::{
        composition::{OrchestrationRuntime, RuntimePorts, StaticSettings},
        ports::sessions_recovery::provider_adapter_registry::AdapterRegistry,
    };
    let mut project = serde_json::to_value(t3_orchestration::testing::project_event(
        "composed:project",
        "project:test",
    )?)?;
    project["payload"]["workspaceRoot"] = serde_json::json!(root);
    let project = serde_json::from_value(project)?;
    h.db.stores
        .transaction(move |db| t3_orchestration::stores::projects::apply(db, &project))
        .await?;
    h.seed("thread.created", thread("thread:test", Some(cwd)))
        .await?;
    let scope = h.seed_run(cwd, 1, "completed").await?;
    backend
        .service()
        .capture_baseline(scope.clone(), 0, CancellationToken::new())
        .await?;
    let baseline = backend
        .service()
        .materialize_baseline(capture_input(scope.clone(), 0))
        .await?;
    h.seed("checkpoint.captured", &baseline).await?;
    let (_, sessions, calls) = service(h, backend, true);
    let adapter = sessions.adapter.clone();
    adapter
        .open_session_with_policy
        .push(Ok(adapter.runtime.clone()));
    let mut adapters: Vec<Arc<dyn ProviderAdapter>> = vec![adapter.clone()];
    adapters.extend(
        other
            .iter()
            .map(|other| other.clone() as Arc<dyn ProviderAdapter>),
    );
    let mut ports = RuntimePorts::new(
        h.db.stores.clone(),
        Arc::new(AdapterRegistry::from_adapters(adapters)),
        backend.clone(),
        h.clock.clone(),
    );
    ports.ids = h.ids.clone();
    ports.rollback_workspaces = Some(recovery);
    let mut settings = serde_json::to_value(t3_contracts::settings::ServerSettings::default())?;
    settings["providerInstances"]["codex"] = serde_json::json!({"driver":"codex","enabled":true});
    if other.is_some() {
        settings["providerInstances"]["codex-side"] =
            serde_json::json!({"driver":"codex","enabled":true});
    }
    ports.settings = Arc::new(StaticSettings::new(serde_json::from_value(settings)?));
    ports.worker.max_attempts = 1;
    Ok((
        OrchestrationRuntime::start(ports).await?,
        adapter,
        scope,
        baseline,
        calls,
    ))
}

/// Re-review 4 F2: a turn of another thread on the folder being restored waits for the restore.
/// A thread launched into the worktree after the isolation checks (or one they skip) is kept out
/// of the folder only by the workspace reservation while Git restores.
#[tokio::test]
async fn another_threads_turn_on_the_restored_folder_waits_for_the_restore() -> Result<()> {
    use t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::WorkspaceReservations;
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    // The other thread runs on its own provider instance and session, so only the folder's
    // reservation can hold its turn back.
    let mut session = serde_json::to_value(session())?;
    session["id"] = serde_json::json!("session:side");
    session["providerInstanceId"] = serde_json::json!("codex-side");
    let session: OrchestrationV2ProviderSession = serde_json::from_value(session)?;
    let (side_adapter, mut calls) = FakeProviderAdapter::new(
        ProviderInstanceId("codex-side".into()),
        t3_contracts::provider_instance::ProviderDriverKind("codex".into()),
        session.capabilities.clone(),
        session,
        ThreadSnapshot {
            provider_thread: provider(),
            provider_turns: vec![],
            messages: vec![],
            reasoning_message_ids: None,
            runtime_requests: vec![],
            provider_payload: None,
        },
    );
    let side_adapter = Arc::new(side_adapter);
    let (runtime, _adapter, scope, baseline, _calls) = composed(
        &h,
        temp.path(),
        temp.path(),
        &backend,
        Arc::new(Recovery),
        Some(side_adapter.clone()),
    )
    .await?;
    let adapter = side_adapter;
    let gate = Arc::new(Gate::default());
    *backend.restore_gate.lock().expect("restore gate") = Some(gate.clone());
    runtime.dispatch_command(serde_json::from_value(serde_json::json!({"type":"checkpoint.rollback","commandId":"composed:rollback","threadId":"thread:test","checkpointId":baseline.id,"scopeId":scope.id,"restoreFiles":true}))?).await?;
    let worker = runtime.effect_worker();
    let drain = worker.drain(64);
    tokio::pin!(drain);
    tokio::select! {
        biased;
        permit = gate.entered.acquire() => permit.expect("restore reached").forget(),
        result = &mut drain => panic!("the rollback ended before Git restored: {result:?}"),
    }
    adapter
        .open_session_with_policy
        .push(Ok(adapter.runtime.clone()));
    let mut side = serde_json::to_value(thread("thread:side", Some(temp.path())))?;
    side["providerInstanceId"] = serde_json::json!("codex-side");
    side["modelSelection"]["instanceId"] = serde_json::json!("codex-side");
    side["activeProviderThreadId"] = serde_json::Value::Null;
    h.seed("thread.created", side).await?;
    runtime.dispatch_command(serde_json::from_value(serde_json::json!({"type":"message.dispatch","commandId":"side:message","threadId":"thread:side","messageId":"message:side","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}))?).await?;
    // Any contact with the other thread's provider before Git returns is the failure.
    while calls.try_recv().is_ok() {}
    let fake = adapter.runtime.clone();
    let reservations = WorkspaceReservations::global();
    tokio::select! {
        biased;
        opened = adapter.open_session_with_policy.next_call() => panic!(
            "the other thread opened its provider while Git restored: {:?}",
            opened.map(|(input, _, _)| input.thread_id)
        ),
        ensured = fake.ensure_thread_with_policy.next_call() => panic!(
            "the other thread reached its provider while Git restored: {:?}",
            ensured.map(|(input, _)| input.thread_id)
        ),
        resumed = fake.resume_thread_with_policy.next_call() => panic!(
            "the other thread resumed its provider thread while Git restored: {:?}",
            resumed.is_some()
        ),
        started = fake.start_turn_with_policy.next_call() => panic!(
            "the other thread started a turn while Git restored: {:?}",
            started.is_some()
        ),
        call = calls.recv() => panic!("the other thread reached its provider while Git restored: {call:?}"),
        waiting = reservations.waiting_on(temp.path()) => waiting?,
        result = &mut drain => panic!("the worker settled while Git restored: {result:?}"),
    }
    gate.release.add_permits(1);
    drain.await?;
    assert_eq!(backend.restores.load(Ordering::SeqCst), 1);
    runtime.shutdown().await?;
    Ok(())
}

/// Re-review 4 F8: a restore refused for a live writer leaves a leftover worktree folder (one
/// without `.git`) where it is. Recovery would move it aside and delete it, with its ignored
/// files, while the writer still works in it.
#[tokio::test]
async fn a_refused_restore_leaves_a_leftover_worktree_folder_in_place() -> Result<()> {
    let parent = tempfile::tempdir()?;
    let cwd = parent.path().join("checkout");
    std::fs::create_dir(&cwd)?;
    std::fs::write(cwd.join(".env"), "TOKEN=kept")?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let (scope, checkpoint) = restorable(&h, &backend, &cwd).await?;
    let (mut service, _, mut calls) = service(&h, &backend, true);
    service.workspace_recovery = Arc::new(RecreateFolder(cwd.clone()));
    service.writers = Arc::new(Writers {
        live: vec!["terminal `npm run dev`".into()],
        ..Default::default()
    });
    let error = service
        .execute(
            request(&scope, &checkpoint, true)?,
            CancellationToken::new(),
        )
        .await
        .expect_err("a live writer refuses the restore");
    assert!(
        rollback_failure_client_message(&error).contains("npm run dev"),
        "{error}"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join(".env")).ok().as_deref(),
        Some("TOKEN=kept"),
        "the leftover folder was moved aside and deleted before the refusal"
    );
    assert!(!cwd.join(".git").exists(), "recovery ran before the check");
    assert_eq!(std::fs::read_dir(parent.path())?.count(), 1);
    assert!(
        !std::iter::from_fn(|| calls.try_recv().ok())
            .any(|call| matches!(call, FakeCall::Rollback(_)))
    );
    Ok(())
}

/// Re-review 4 F7: a folder whose repository changed is refused before the provider rolls back
/// its conversation, so the refusal leaves T3 and the provider as they were.
#[tokio::test]
async fn a_changed_repository_refuses_before_the_providers_rollback() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let (scope, checkpoint) = restorable(&h, &backend, temp.path()).await?;
    *backend.refuse_binding.lock().expect("binding") = Some("a repository was planted".into());
    let (service, _, mut calls) = service(&h, &backend, true);
    let error = service
        .execute(
            request(&scope, &checkpoint, true)?,
            CancellationToken::new(),
        )
        .await
        .expect_err("a changed repository refuses the restore");
    assert!(
        matches!(&error, Error::Service { operation, .. } if *operation == REPOSITORY_CHANGED_OPERATION),
        "{error}"
    );
    assert!(
        !std::iter::from_fn(|| calls.try_recv().ok())
            .any(|call| matches!(call, FakeCall::Rollback(_))),
        "the provider rolled back before the refusal"
    );
    assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
    Ok(())
}

/// Re-review 4 F9 and the untested claims: background work the provider reports, a probe that
/// fails, a session that cannot report at all, and live processes in the agent's scope each
/// refuse the restore before the provider's rollback.
#[tokio::test]
async fn unreported_or_reported_provider_work_refuses_a_restore() -> Result<()> {
    struct LiveScope;
    #[async_trait]
    impl AgentScope for LiveScope {
        async fn live_processes(&self, _thread_id: ThreadId) -> Result<Option<usize>> {
            Ok(Some(2))
        }
    }
    for case in [
        "reported",
        "probe fails",
        "cannot report",
        "agent processes",
    ] {
        let temp = tempfile::tempdir()?;
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        let (scope, checkpoint) = restorable(&h, &backend, temp.path()).await?;
        let (mut service, sessions, mut calls) = service(&h, &backend, true);
        let runtime = &sessions.adapter.runtime;
        match case {
            "reported" => *runtime.background_work.lock().expect("work") = Some(Ok(true)),
            "probe fails" => {
                *runtime.background_work.lock().expect("work") = Some(Err("probe".into()))
            }
            "cannot report" => runtime
                .leaves_no_background_work
                .store(false, Ordering::SeqCst),
            _ => service.agent_scope = Arc::new(LiveScope),
        }
        let Err(error) = service
            .execute(
                request(&scope, &checkpoint, true)?,
                CancellationToken::new(),
            )
            .await
        else {
            panic!("{case}: the restore ran without proof that the provider is quiet");
        };
        assert!(
            matches!(&error, Error::Service { operation, .. } if *operation == WRITERS_RUNNING_OPERATION),
            "{case}: {error}"
        );
        assert_eq!(backend.restores.load(Ordering::SeqCst), 0, "{case}");
        assert!(
            !std::iter::from_fn(|| calls.try_recv().ok())
                .any(|call| matches!(call, FakeCall::Rollback(_))),
            "{case}"
        );
    }
    Ok(())
}

/// The lane re-check: a turn recorded while the provider rolled back refuses the restore before
/// Git runs.
#[tokio::test]
async fn a_turn_recorded_during_the_providers_rollback_refuses_the_restore() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let (scope, checkpoint) = restorable(&h, &backend, temp.path()).await?;
    let (service, sessions, _calls) = service(&h, &backend, true);
    let (entered, entering) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    *sessions
        .adapter
        .runtime
        .rollback_hold
        .lock()
        .expect("rollback hold") = Some((entered, released));
    let input = request(&scope, &checkpoint, true)?;
    let rollback =
        tokio::spawn(async move { service.execute(input, CancellationToken::new()).await });
    entering.await.expect("provider rollback entered");
    h.seed_run(temp.path(), 2, "running").await?;
    release.send(()).expect("release the provider rollback");
    let error = rollback
        .await
        .expect("rollback task")
        .expect_err("the new turn refuses the restore");
    assert!(
        rollback_failure_client_message(&error).contains("a running turn"),
        "{error}"
    );
    assert_eq!(backend.restores.load(Ordering::SeqCst), 0);
    Ok(())
}

/// A recovery that reports success without creating the folder.
struct CreatesNothing;
#[async_trait]
impl RollbackWorkspaceRecovery for CreatesNothing {
    async fn is_managed(&self, _path: &Path) -> Result<bool> {
        Ok(true)
    }
    async fn recreate(
        &self,
        _thread: &OrchestrationV2AppThread,
        _cancellation: CancellationToken,
    ) -> Result<()> {
        Ok(())
    }
}

/// Re-review 4 F1: only a recovery that created the folder may rebind it. One that created
/// nothing (as when the project row is gone) leaves the saved binding alone.
#[tokio::test]
async fn a_recovery_that_creates_nothing_never_rebinds_the_folder() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let worktree = temp.path().join("missing-worktree");
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let (runtime, _adapter, scope, baseline, _calls) = composed(
        &h,
        temp.path(),
        &worktree,
        &backend,
        Arc::new(CreatesNothing),
        None,
    )
    .await?;
    runtime.dispatch_command(serde_json::from_value(serde_json::json!({"type":"checkpoint.rollback","commandId":"composed:rollback","threadId":"thread:test","checkpointId":baseline.id,"scopeId":scope.id,"restoreFiles":true}))?).await?;
    runtime.effect_worker().drain(64).await?;
    assert!(
        !backend
            .binds
            .lock()
            .expect("binds")
            .iter()
            .any(|(path, replace)| *replace && *path == worktree),
        "{:?}",
        backend.binds.lock().expect("binds")
    );
    runtime.shutdown().await?;
    Ok(())
}

/// Re-review 4 L4: a command title the agent chose reaches the refusal on one line and capped.
#[tokio::test]
async fn writer_names_in_a_refusal_are_capped_and_on_one_line() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let (scope, checkpoint) = restorable(&h, &backend, temp.path()).await?;
    let title = format!("watch\nInjected: {}", "x".repeat(300));
    h.seed("turn-item.updated", serde_json::json!({"id":"command:long","threadId":"thread:test","runId":"run:1","nodeId":"node:1","providerThreadId":"provider:test","providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":7,"status":"running","title":title,"startedAt":time(),"completedAt":null,"updatedAt":time(),"type":"command_execution","input":"watch"}))
        .await?;
    let (service, _, _calls) = service(&h, &backend, true);
    let error = service
        .execute(
            request(&scope, &checkpoint, true)?,
            CancellationToken::new(),
        )
        .await
        .expect_err("a running command refuses the restore");
    let message = rollback_failure_client_message(&error);
    assert!(
        message.contains("command `watch Injected: xxx"),
        "{message}"
    );
    assert!(!message.contains('\n'), "{message}");
    assert!(message.chars().count() < 260, "{message}");
    Ok(())
}
