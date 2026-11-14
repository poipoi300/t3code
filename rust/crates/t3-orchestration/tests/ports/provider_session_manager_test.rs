use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Arc;
use t3_contracts::{base_schemas::*, orchestration_v2::*, provider_instance::*};
use t3_orchestration::{api::*, error::Result, provider::*, runtime::sessions::*, testing::*};
use tokio::sync::mpsc;

enum CloseAction {
    Fail,
    Wait(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>),
}

#[tokio::test]
async fn idle_requests_and_recovered_commands_persist_before_any_run_subscribes() -> Result<()> {
    use super::provider_runtime_recovery_service_test::{item, projection, seed, sink};
    use t3_orchestration::{
        ports::decisions::thread_command_executor::KeyedLanes,
        projections::store::ProjectionStore,
        runtime::ingestion::{ProviderCommitter, ProviderEventIngestor},
    };
    let database = TestDatabase::memory()?;
    let sink = Arc::new(sink(database.stores.clone()));
    let reader = Arc::new(ProjectionStore::new(database.stores.clone()));
    let clock = Arc::new(ManualClock::new(TEST_TIME));
    let ids = Arc::new(SequenceIds::new("idle-artifacts"));
    let mut projection = projection()?;
    projection.runs.clear();
    projection.attempts.clear();
    projection.nodes.clear();
    seed(&sink, &projection).await?;
    let ingestor = Arc::new(ProviderEventIngestor::new(
        reader.clone(),
        Arc::new(ProviderCommitter((*sink).clone())),
        clock.clone(),
        ids.clone(),
        Arc::new(KeyedLanes::default()),
    ));
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        Some(ingestor.clone()),
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: Some(SessionPersistence {
                reader: reader.clone(),
                sink: sink.clone(),
                clock,
                ids,
            }),
            credentials: None,
        }),
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut committed = sink.subscribe();
    let node = serde_json::from_value(
        json!({"id":"node:idle","threadId":"thread:test","runId":null,"parentNodeId":null,"rootNodeId":"node:idle","kind":"user_input_request","status":"waiting","countsForRun":false,"providerThreadId":"provider-thread:test","providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":"request:idle","checkpointScopeId":null,"startedAt":TEST_TIME,"completedAt":null}),
    )?;
    let request = serde_json::from_value(
        json!({"id":"request:idle","providerTurnId":null,"nodeId":"node:idle","kind":"user_input","nativeRequestRef":null,"status":"pending","responseCapability":{"type":"live","providerSessionId":"session:test"},"createdAt":TEST_TIME,"resolvedAt":null}),
    )?;
    let mut question = item("question:idle", "user_input_request");
    question["runId"] = serde_json::Value::Null;
    question["providerTurnId"] = serde_json::Value::Null;
    question["nodeId"] = json!("node:idle");
    question["requestId"] = json!("request:idle");
    let mut command = item("command:recovered", "command_execution");
    command["runId"] = serde_json::Value::Null;
    command["nodeId"] = serde_json::Value::Null;
    command["input"] = json!("recovered background command");
    command["ordinal"] = json!(2);
    for kind in [
        ProviderEventKind::Node(Box::new(node)),
        ProviderEventKind::RuntimeRequest {
            request: Box::new(request),
            thread_id: Some(input.thread_id.clone()),
        },
        ProviderEventKind::TurnItem {
            item: Box::new(serde_json::from_value(question)?),
            session_scoped: None,
        },
        ProviderEventKind::TurnItem {
            item: Box::new(serde_json::from_value(command)?),
            session_scoped: Some(true),
        },
    ] {
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: ProviderDriverKind("codex".into()),
                kind,
            })
            .await?;
    }
    loop {
        if let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored) =
            committed.recv().await.unwrap()
        {
            match stored.event {
                OrchestrationV2DomainEvent::TurnItemUpdated(event) => {
                    if let OrchestrationV2TurnItem::CommandExecution(command) = event.payload
                        && t3_contracts::stored::to_value(&command.id)?
                            == json!("command:recovered")
                    {
                        break;
                    }
                }
                OrchestrationV2DomainEvent::ProviderSessionUpdated(event)
                    if event.payload.status == ProviderSessionStatus::Error =>
                {
                    panic!(
                        "idle artifact ingestion failed: {:?}",
                        event.payload.last_error
                    )
                }
                _ => {}
            }
        }
    }
    let stored = reader.projection(input.thread_id.clone()).await?.unwrap();
    assert!(stored.runs.is_empty());
    assert_eq!(stored.runtime_requests.len(), 1);
    assert!(stored.runtime_requests[0].provider_turn_id.is_none());
    assert_eq!(stored.nodes.len(), 1);
    assert!(stored.nodes[0].run_id.is_none());
    assert_eq!(stored.turn_items.len(), 2);
    database.stores.read(|db| {
        assert_eq!(db.query_row("SELECT count(*) FROM orchestration_events WHERE event_type='turn-item.updated' AND json_extract(payload_json,'$.id')='command:recovered' AND json_extract(metadata_json,'$.runId') IS NULL", [], |row| row.get::<_,i64>(0))?, 1);
        Ok(())
    }).await?;
    let mut subscription = runtime.subscribe().await?;
    let background = ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::BackgroundWork {
            provider_turn_id: ProviderTurnId("barrier".into()),
            pending: false,
        },
    };
    adapter.runtime.emit(background.clone()).await?;
    assert_eq!(subscription.events.recv().await.unwrap()?, background);
    manager.shutdown().await?;
    ingestor.shutdown().await;
    Ok(())
}

#[derive(Default)]
struct LeaseState {
    reservations: usize,
    owners: std::collections::BTreeSet<ProviderSessionId>,
    live: bool,
    revoked: usize,
}

#[derive(Default)]
struct LeaseAuthority {
    state: std::sync::Mutex<LeaseState>,
    validation: std::sync::Mutex<Option<Arc<RetentionGate>>>,
}

impl LeaseAuthority {
    fn clear_unused(state: &mut LeaseState) {
        if state.live && state.reservations == 0 && state.owners.is_empty() {
            state.live = false;
            state.revoked += 1;
        }
    }
}

struct LeaseCredentials(Arc<LeaseAuthority>);
struct LeaseReservation {
    authority: Arc<LeaseAuthority>,
    active: bool,
}
struct LeaseOwner {
    authority: Arc<LeaseAuthority>,
    owner: ProviderSessionId,
}

impl Drop for LeaseReservation {
    fn drop(&mut self) {
        if self.active {
            let mut state = self.authority.state.lock().expect("credential state");
            state.reservations -= 1;
            LeaseAuthority::clear_unused(&mut state);
        }
    }
}
impl Drop for LeaseOwner {
    fn drop(&mut self) {
        let mut state = self.authority.state.lock().expect("credential state");
        assert!(state.owners.remove(&self.owner));
        LeaseAuthority::clear_unused(&mut state);
    }
}
impl SessionCredentialReservation for LeaseReservation {
    fn publish(mut self: Box<Self>, owner: ProviderSessionId) -> Result<PublishedCredential> {
        let mut state = self.authority.state.lock().expect("credential state");
        assert!(state.live);
        assert!(state.owners.insert(owner.clone()));
        state.reservations -= 1;
        self.active = false;
        Ok(Box::new(LeaseOwner {
            authority: self.authority.clone(),
            owner,
        }))
    }
}
#[async_trait]
impl SessionCredentials for LeaseCredentials {
    async fn prepare_reserved(
        &self,
        _: ThreadId,
        _: ProviderInstanceId,
    ) -> Result<PreparedSessionCredential> {
        {
            let mut state = self.0.state.lock().expect("credential state");
            state.reservations += 1;
            state.live = true;
        }
        let reservation = LeaseReservation {
            authority: self.0.clone(),
            active: true,
        };
        let validation = self.0.validation.lock().expect("validation gate").take();
        if let Some(validation) = validation {
            validation.entered.notify_one();
            validation.release.notified().await;
        }
        Ok(PreparedSessionCredential::with_reservation(
            PreparedCredential {
                id: "credential:reserved".into(),
                issued: false,
            },
            Box::new(reservation),
        ))
    }
    async fn revoke(&self, _: ThreadId, _: String) -> Result<()> {
        panic!("reserved credentials must release through their native ownership guard");
    }
}

#[tokio::test]
async fn native_credential_publication_survives_pooled_detach_and_replacement_then_drops_once()
-> Result<()> {
    let (adapter, mut calls, input) = fake_with_pool(true)?;
    let authority = Arc::new(LeaseAuthority::default());
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: None,
            credentials: Some(Arc::new(LeaseCredentials(authority.clone()))),
        }),
    );
    manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    let mut other = input.clone();
    other.thread_id = ThreadId("thread:other".into());
    manager
        .attach(other.model_selection.instance_id.clone(), other.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    assert!(calls.try_recv().is_err());
    assert_eq!(authority.state.lock().unwrap().owners.len(), 2);
    manager
        .detach_thread(
            input.provider_session_id.clone(),
            input.thread_id.clone(),
            false,
        )
        .await?;
    assert_eq!(authority.state.lock().unwrap().owners.len(), 2);
    manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    {
        let state = authority.state.lock().unwrap();
        assert!(state.live);
        assert_eq!(state.owners.len(), 2);
        assert_eq!(state.reservations, 0);
        assert_eq!(state.revoked, 0);
    }
    manager
        .detach_thread(input.provider_session_id.clone(), input.thread_id, true)
        .await?;
    assert_eq!(authority.state.lock().unwrap().owners.len(), 1);
    manager
        .detach_thread(other.provider_session_id, other.thread_id, true)
        .await?;
    {
        let state = authority.state.lock().unwrap();
        assert!(!state.live);
        assert!(state.owners.is_empty());
        assert_eq!(state.revoked, 1);
    }
    manager.shutdown().await?;
    assert_eq!(authority.state.lock().unwrap().revoked, 1);
    Ok(())
}

#[tokio::test]
async fn a_native_credential_is_reserved_before_validation_awaits_and_predecessor_release()
-> Result<()> {
    let (first, mut first_calls, input) = fake_named(false, "session:first")?;
    let (second, mut second_calls, other) = fake_named(false, "session:other")?;
    let authority = Arc::new(LeaseAuthority::default());
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![Arc::new(
            RoutedAdapter { first, second },
        )])),
        None,
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: None,
            credentials: Some(Arc::new(LeaseCredentials(authority.clone()))),
        }),
    );
    manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(first_calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(
        first_calls.recv().await,
        Some(FakeCall::Subscribe)
    ));
    let gate = Arc::new(RetentionGate {
        entered: Default::default(),
        release: Default::default(),
    });
    *authority.validation.lock().unwrap() = Some(gate.clone());
    let replacement = {
        let manager = manager.clone();
        let other = other.clone();
        tokio::spawn(async move {
            manager
                .attach(other.model_selection.instance_id.clone(), other)
                .await
        })
    };
    gate.entered.notified().await;
    manager.detach(input.provider_session_id, false).await?;
    assert!(matches!(first_calls.recv().await, Some(FakeCall::Close)));
    {
        let state = authority.state.lock().unwrap();
        assert!(state.live);
        assert_eq!(state.reservations, 1);
        assert!(state.owners.is_empty());
        assert_eq!(state.revoked, 0);
    }
    gate.release.notify_one();
    replacement.await.unwrap()?;
    assert!(matches!(second_calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(
        second_calls.recv().await,
        Some(FakeCall::Subscribe)
    ));
    assert_eq!(authority.state.lock().unwrap().owners.len(), 1);
    manager.shutdown().await?;
    assert_eq!(authority.state.lock().unwrap().revoked, 1);
    Ok(())
}

#[tokio::test]
async fn shutdown_drops_a_native_credential_reservation_during_validation() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    let authority = Arc::new(LeaseAuthority::default());
    let gate = Arc::new(RetentionGate {
        entered: Default::default(),
        release: Default::default(),
    });
    *authority.validation.lock().unwrap() = Some(gate.clone());
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: None,
            credentials: Some(Arc::new(LeaseCredentials(authority.clone()))),
        }),
    );
    let attach = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .attach(input.model_selection.instance_id.clone(), input)
                .await
        })
    };
    gate.entered.notified().await;
    manager.shutdown().await?;
    assert!(matches!(
        attach.await.unwrap(),
        Err(t3_orchestration::error::Error::Cancelled)
    ));
    assert!(calls.try_recv().is_err());
    let state = authority.state.lock().unwrap();
    assert!(!state.live);
    assert_eq!(state.reservations, 0);
    assert_eq!(state.revoked, 1);
    Ok(())
}

#[tokio::test]
async fn suggestion_subscriptions_clear_cached_values_when_the_thread_is_deleted() -> Result<()> {
    use super::provider_runtime_recovery_service_test::{projection, seed, sink};
    use t3_orchestration::projections::store::ProjectionStore;
    let database = TestDatabase::memory()?;
    let sink = Arc::new(sink(database.stores.clone()));
    let mut projection = projection()?;
    seed(&sink, &projection).await?;
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: Some(SessionPersistence {
                reader: Arc::new(ProjectionStore::new(database.stores)),
                sink: sink.clone(),
                clock: Arc::new(ManualClock::new(TEST_TIME)),
                ids: Arc::new(SequenceIds::new("suggestion-delete")),
            }),
            credentials: None,
        }),
    );
    manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut subscription = manager
        .subscribe_prompt_suggestion(input.thread_id.clone())
        .await?;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::PromptSuggestion {
                thread_id: input.thread_id.clone(),
                run_id: RunId("run:test".into()),
                text: "Next step".into(),
            },
        })
        .await?;
    subscription.changed().await.unwrap();
    assert_eq!(
        subscription.borrow_and_update().as_ref().unwrap().text,
        "Next step"
    );
    projection.thread.deleted_at = Some(t3_contracts::wire::WireDateTime(TEST_TIME.into()));
    sink.commit(CommitRequest { receipt: None, guard: CommitGuard::None, decision: Decision {
        events: vec![serde_json::from_value(json!({"id":"thread:deleted","threadId":input.thread_id,"type":"thread.deleted","occurredAt":TEST_TIME,"payload":t3_contracts::stored::to_value(&projection.thread)?}))?], ..Default::default()
    }}).await?;
    subscription.changed().await.unwrap();
    assert!(subscription.borrow_and_update().is_none());
    assert!(
        manager
            .subscribe_prompt_suggestion(input.thread_id)
            .await?
            .borrow()
            .is_none()
    );
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn errored_provider_thread_invalidates_its_native_resume_cache() -> Result<()> {
    let (adapter, mut calls, input) = fake_with_pool(true)?;
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut subscription = runtime.subscribe().await?;
    let provider_thread =
        super::provider_runtime_recovery_service_test::projection()?.provider_threads[0].clone();
    let resume = ResumeThread {
        provider_thread: provider_thread.clone(),
        thread_id: Some(input.thread_id),
        model_selection: Some(input.model_selection),
        runtime_policy: Some(input.runtime_policy),
    };
    runtime.resume_thread(resume.clone()).await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Resume(_))));
    runtime.resume_thread(resume.clone()).await?;
    assert!(calls.try_recv().is_err());
    let mut session = runtime.session().clone();
    session.status = ProviderSessionStatus::Error;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::Session(Box::new(session)),
        })
        .await?;
    subscription.events.recv().await.unwrap()?;
    runtime.resume_thread(resume.clone()).await?;
    assert!(
        calls.try_recv().is_err(),
        "a reusable session error must preserve the loaded native thread"
    );
    let mut failed = provider_thread;
    failed.status = ProviderThreadStatus::Error;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::Thread {
                provider_thread: Box::new(failed),
                pending_background_work: None,
                session_scoped: None,
                retained_native_turn_ids: None,
            },
        })
        .await?;
    subscription.events.recv().await.unwrap()?;
    runtime.resume_thread(resume).await?;
    assert!(
        matches!(calls.try_recv(), Ok(FakeCall::Resume(_))),
        "errored native thread must be resumed rather than served from its cached load"
    );
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn native_close_budget_keeps_failed_cleanup_retryable() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    let entered = Arc::new(tokio::sync::Notify::new());
    let runtime = Arc::new(LifecycleRuntime {
        native: adapter.runtime.clone(),
        closes: std::sync::Mutex::new(
            [CloseAction::Wait(
                entered.clone(),
                Arc::new(tokio::sync::Notify::new()),
            )]
            .into(),
        ),
        fail_subscription: false,
    });
    adapter.open_session_with_policy.push(Ok(runtime));
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
        SessionManagerOptions {
            close_timeout: std::time::Duration::from_secs(1),
            ..Default::default()
        },
        None,
    );
    SessionDirectory::attach_with_policy(
        &manager,
        input.model_selection.instance_id.clone(),
        input.clone(),
        RuntimePolicyOptions::default(),
    )
    .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let close = {
        let manager = manager.clone();
        let id = input.provider_session_id.clone();
        tokio::spawn(async move { manager.detach(id, false).await })
    };
    entered.notified().await;
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    assert!(matches!(
        close.await.unwrap(),
        Err(t3_orchestration::error::Error::Provider(
            ProviderError::Operation {
                operation: ProviderOperation::Close,
                ..
            }
        ))
    ));
    assert!(
        manager
            .get(input.provider_session_id.clone())
            .await?
            .is_none()
    );
    manager.detach(input.provider_session_id, false).await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_closes_independent_native_sessions_concurrently_and_joins_repeat_callers()
-> Result<()> {
    let (first, mut first_calls, input) = fake()?;
    let (second, mut second_calls, other) = fake_named(false, "session:other")?;
    let entered_first = Arc::new(tokio::sync::Notify::new());
    let entered_second = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    for (native, entered) in [
        (first.runtime.clone(), entered_first.clone()),
        (second.runtime.clone(), entered_second.clone()),
    ] {
        first
            .open_session_with_policy
            .push(Ok(Arc::new(LifecycleRuntime {
                native,
                closes: std::sync::Mutex::new([CloseAction::Wait(entered, release.clone())].into()),
                fail_subscription: false,
            })));
    }
    let manager =
        ProviderSessionManager::new(Arc::new(AdapterRegistry::from_adapters(vec![first])), None);
    for input in [input, other] {
        manager
            .attach_with_policy(
                input.model_selection.instance_id.clone(),
                input,
                RuntimePolicyOptions::default(),
            )
            .await?;
    }
    assert!(matches!(
        first_calls.recv().await,
        Some(FakeCall::Subscribe)
    ));
    assert!(matches!(
        second_calls.recv().await,
        Some(FakeCall::Subscribe)
    ));
    let shutdown = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.shutdown().await })
    };
    entered_first.notified().await;
    entered_second.notified().await;
    assert!(!shutdown.is_finished());
    let repeat = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.shutdown().await })
    };
    release.notify_waiters();
    shutdown.await.unwrap()?;
    repeat.await.unwrap()?;
    assert!(matches!(first_calls.recv().await, Some(FakeCall::Close)));
    assert!(matches!(second_calls.recv().await, Some(FakeCall::Close)));
    assert!(first_calls.try_recv().is_err());
    assert!(second_calls.try_recv().is_err());
    Ok(())
}

struct LifecycleRuntime {
    native: Arc<FakeSessionRuntime>,
    closes: std::sync::Mutex<std::collections::VecDeque<CloseAction>>,
    fail_subscription: bool,
}

struct HandshakeAdapter {
    native: Arc<FakeProviderAdapter>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ProviderAdapter for HandshakeAdapter {
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        None
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        self.native.instance_id()
    }
    fn driver(&self) -> ProviderDriverKind {
        self.native.driver()
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        self.native.capabilities().await
    }
    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        self.native.plan_selection_transition(input).await
    }
    async fn open_session(
        &self,
        input: OpenSession,
        _: tokio_util::sync::CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        self.entered.notify_one();
        self.release.notified().await;
        self.native
            .open_session(input, tokio_util::sync::CancellationToken::new())
            .await
    }
}

#[tokio::test]
async fn shutdown_joins_a_late_handshake_after_the_attach_caller_is_cancelled() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let credentials = Arc::new(Credentials {
        next: Default::default(),
        reuse: false,
        revoked: Default::default(),
    });
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![Arc::new(
            HandshakeAdapter {
                native: adapter,
                entered: entered.clone(),
                release: release.clone(),
            },
        )])),
        None,
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: None,
            credentials: Some(credentials.clone()),
        }),
    );
    let attach = {
        let manager = manager.clone();
        let input = input.clone();
        tokio::spawn(async move {
            manager
                .attach(input.model_selection.instance_id.clone(), input)
                .await
        })
    };
    entered.notified().await;
    attach.abort();
    assert!(matches!(attach.await, Err(error) if error.is_cancelled()));
    let shutdown = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.shutdown().await })
    };
    release.notify_one();
    shutdown.await.unwrap()?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(calls.try_recv().is_err());
    assert!(!manager.is_live(&input.provider_session_id));
    assert_eq!(
        *credentials.revoked.lock().unwrap(),
        vec![(input.thread_id, "credential:0".into())]
    );
    Ok(())
}

struct UnloadRuntime {
    native: Arc<FakeSessionRuntime>,
    unloaded: mpsc::UnboundedSender<(OrchestrationV2ProviderThread, bool)>,
    release: Arc<tokio::sync::Notify>,
    fail_archive: bool,
}

#[tokio::test(start_paused = true)]
async fn delivered_delegated_children_archive_natively_and_fall_back_to_unload_on_failure()
-> Result<()> {
    use super::provider_runtime_recovery_service_test::{projection, seed, sink};
    use t3_orchestration::projections::store::ProjectionStore;
    for fail_archive in [false, true] {
        let database = TestDatabase::memory()?;
        let sink = Arc::new(sink(database.stores.clone()));
        let mut projection = projection()?;
        let mut child = t3_contracts::stored::to_value(&projection.thread)?;
        child["lineage"] = json!({"parentThreadId":"thread:parent","relationshipToParent":"subagent","rootThreadId":"thread:parent"});
        projection.thread = serde_json::from_value(child.clone())?;
        let mut run = t3_contracts::stored::to_value(&projection.runs[0])?;
        run["delegatedTaskId"] = json!("task:child");
        projection.runs[0] = serde_json::from_value(run)?;
        let mut parent = child;
        parent["id"] = json!("thread:parent");
        parent["lineage"] = json!({"parentThreadId":null,"relationshipToParent":null,"rootThreadId":"thread:parent"});
        parent["activeProviderThreadId"] = serde_json::Value::Null;
        let mut task = t3_contracts::stored::to_value(&sample::<OrchestrationV2Subagent>(
            "OrchestrationV2Subagent",
        )?)?;
        for (key, value) in [
            ("id", json!("task:child")),
            ("threadId", json!("thread:parent")),
            ("runId", serde_json::Value::Null),
            ("origin", json!("app_owned")),
            ("driver", json!("codex")),
            ("providerInstanceId", json!("codex")),
            ("childThreadId", json!("thread:test")),
            ("status", json!("completed")),
            (
                "completionDelivery",
                json!({"state":"delivered","observedByRunId":null}),
            ),
        ] {
            task[key] = value;
        }
        sink.commit(CommitRequest { receipt: None, guard: CommitGuard::None, decision: Decision {
            events: vec![
                serde_json::from_value(json!({"id":"parent:created","threadId":"thread:parent","type":"thread.created","occurredAt":TEST_TIME,"payload":parent}))?,
                serde_json::from_value(json!({"id":"task:completed","threadId":"thread:parent","type":"subagent.updated","occurredAt":TEST_TIME,"payload":task}))?,
            ], ..Default::default()
        }}).await?;
        seed(&sink, &projection).await?;
        let (adapter, mut calls, input) = fake_with_pool(true)?;
        let (send, mut unloaded) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        adapter
            .open_session_with_policy
            .push(Ok(Arc::new(UnloadRuntime {
                native: adapter.runtime.clone(),
                unloaded: send,
                release: release.clone(),
                fail_archive,
            })));
        let manager = ProviderSessionManager::with_options(
            Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
            None,
            SessionManagerOptions {
                idle_timeout: std::time::Duration::from_secs(10),
                ..Default::default()
            },
            Some(SessionServices {
                persistence: Some(SessionPersistence {
                    reader: Arc::new(ProjectionStore::new(database.stores)),
                    sink,
                    clock: Arc::new(ManualClock::new(TEST_TIME)),
                    ids: Arc::new(SequenceIds::new("archive-native")),
                }),
                credentials: None,
            }),
        );
        let runtime = manager
            .attach_with_policy(
                input.model_selection.instance_id.clone(),
                input.clone(),
                RuntimePolicyOptions::default(),
            )
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
        let mut subscription = runtime.subscribe().await?;
        runtime
            .resume_thread(ResumeThread {
                provider_thread: projection.provider_threads[0].clone(),
                thread_id: Some(input.thread_id.clone()),
                model_selection: Some(input.model_selection.clone()),
                runtime_policy: Some(input.runtime_policy.clone()),
            })
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Resume(_))));
        runtime.start_turn(turn_input(&input, 1)?).await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Start(_))));
        adapter
            .runtime
            .emit(ProviderEvent {
                driver: ProviderDriverKind("codex".into()),
                kind: ProviderEventKind::Terminal {
                    provider_thread_id: ProviderThreadId("provider-thread:test".into()),
                    provider_turn_id: ProviderTurnId("turn".into()),
                    run_ordinal: 1,
                    disposition: ThreadDisposition::Reusable,
                    outcome: TerminalOutcome::Completed { reason: None },
                },
            })
            .await?;
        subscription.events.recv().await.unwrap()?;
        tokio::time::advance(std::time::Duration::from_secs(10)).await;
        assert!(unloaded.recv().await.unwrap().1);
        release.notify_one();
        if fail_archive {
            assert!(!unloaded.recv().await.unwrap().1);
            release.notify_one();
        }
        assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
        manager.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn detaching_a_parent_unloads_native_children_except_an_attached_child_owner() -> Result<()> {
    use super::provider_runtime_recovery_service_test::{projection, seed, sink};
    use t3_orchestration::projections::store::ProjectionStore;
    for attached in [false, true] {
        let database = TestDatabase::memory()?;
        let sink = Arc::new(sink(database.stores.clone()));
        let mut projection = projection()?;
        let mut child_thread = t3_contracts::stored::to_value(&projection.thread)?;
        child_thread["id"] = json!("thread:child");
        child_thread["activeProviderThreadId"] = serde_json::Value::Null;
        sink.commit(CommitRequest { receipt: None, guard: CommitGuard::None, decision: Decision {
            events: vec![serde_json::from_value(json!({"id":"child:created","threadId":"thread:child","type":"thread.created","occurredAt":TEST_TIME,"payload":child_thread}))?], ..Default::default()
        }}).await?;
        let mut child = projection.provider_threads[0].clone();
        child.id = serde_json::from_value(json!("provider-thread:child"))?;
        child.app_thread_id = Some(serde_json::from_value(json!("thread:child"))?);
        child.owner_node_id = Some(serde_json::from_value(json!("node:test"))?);
        projection.provider_threads.push(child);
        seed(&sink, &projection).await?;
        let (adapter, mut calls, input) = fake_with_pool(true)?;
        let (send, mut unloaded) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        adapter
            .open_session_with_policy
            .push(Ok(Arc::new(UnloadRuntime {
                native: adapter.runtime.clone(),
                unloaded: send,
                release: release.clone(),
                fail_archive: false,
            })));
        let manager = ProviderSessionManager::with_options(
            Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
            None,
            SessionManagerOptions::default(),
            Some(SessionServices {
                persistence: Some(SessionPersistence {
                    reader: Arc::new(ProjectionStore::new(database.stores)),
                    sink,
                    clock: Arc::new(ManualClock::new(TEST_TIME)),
                    ids: Arc::new(SequenceIds::new("detach-native")),
                }),
                credentials: None,
            }),
        );
        manager
            .attach_with_policy(
                input.model_selection.instance_id.clone(),
                input.clone(),
                RuntimePolicyOptions::default(),
            )
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
        if attached {
            let mut child_input = input.clone();
            child_input.thread_id = ThreadId("thread:child".into());
            manager
                .attach(child_input.model_selection.instance_id.clone(), child_input)
                .await?;
        }
        let detach = {
            let manager = manager.clone();
            let input = input.clone();
            tokio::spawn(async move {
                manager
                    .detach_thread(input.provider_session_id, input.thread_id, false)
                    .await
            })
        };
        let mut ids = Vec::new();
        for _ in 0..if attached { 1 } else { 2 } {
            let (thread, archive) = unloaded.recv().await.unwrap();
            ids.push(t3_contracts::stored::to_value(&thread.id)?);
            assert!(!archive);
            release.notify_one();
        }
        detach.await.unwrap()?;
        assert!(ids.contains(&json!("provider-thread:test")));
        assert_eq!(ids.contains(&json!("provider-thread:child")), !attached);
        assert!(unloaded.try_recv().is_err());
        assert!(manager.is_live(&input.provider_session_id));
        manager.shutdown().await?;
    }
    Ok(())
}

#[async_trait]
impl SessionRuntime for UnloadRuntime {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        self.native.session()
    }
    fn can_resume_after_restart(&self) -> bool {
        true
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    fn extensions(&self) -> Option<&dyn SessionExtensions> {
        Some(self)
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        self.native.subscribe().await
    }
    async fn close(&self) -> ProviderResult<()> {
        self.native.close().await
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.native.ensure_thread(input).await
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.native.resume_thread(input).await
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        self.native.start_turn(input).await
    }
    async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()> {
        self.native.steer_turn(input).await
    }
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
        self.native.interrupt_turn(input).await
    }
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
        self.native.respond(input).await
    }
    async fn snapshot(
        &self,
        input: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        self.native.snapshot(input).await
    }
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        self.native.rollback(input).await
    }
    async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.native.fork(input).await
    }
}

#[async_trait]
impl SessionExtensions for UnloadRuntime {
    fn capabilities(&self) -> ExtensionCapabilities {
        ExtensionCapabilities {
            unload: true,
            ..Default::default()
        }
    }
    async fn pending_background_work(
        &self,
        _: Option<OrchestrationV2ProviderThread>,
        _: bool,
    ) -> ProviderResult<bool> {
        Ok(false)
    }
    async fn model_context_window(
        &self,
        _: t3_contracts::model_selection::ModelSelection,
        _: Option<std::path::PathBuf>,
    ) -> ProviderResult<Option<u64>> {
        Ok(None)
    }
    fn can_reuse_context_usage(
        &self,
        _: &t3_contracts::model_selection::ModelSelection,
        _: &t3_contracts::model_selection::ModelSelection,
    ) -> bool {
        false
    }
    fn normalize_thread_load_selection(
        &self,
        selection: t3_contracts::model_selection::ModelSelection,
    ) -> t3_contracts::model_selection::ModelSelection {
        selection
    }
    async fn unload(
        &self,
        thread: OrchestrationV2ProviderThread,
        archive: bool,
    ) -> ProviderResult<()> {
        self.unloaded
            .send((thread, archive))
            .expect("unload observer");
        self.release.notified().await;
        if archive && self.fail_archive {
            return Err(ProviderError::Unsupported("native archive failed"));
        }
        Ok(())
    }
    async fn inject_history(
        &self,
        _: OrchestrationV2ProviderThread,
        _: Vec<OrchestrationV2HistoricalMessage>,
        _: String,
    ) -> ProviderResult<bool> {
        Err(ProviderError::Unsupported("inject_history"))
    }
    async fn compact(&self, _: TurnInput) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("compact"))
    }
    async fn apply_selection(
        &self,
        _: OrchestrationV2ProviderThread,
        _: t3_contracts::model_selection::ModelSelection,
    ) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("apply_selection"))
    }
    async fn stop_task(&self, _: OrchestrationV2ProviderThread, _: String) -> ProviderResult<()> {
        Err(ProviderError::Unsupported("stop_task"))
    }
    async fn await_tasks(
        &self,
        _: OrchestrationV2ProviderThread,
        _: Vec<String>,
        _: bool,
    ) -> ProviderResult<Vec<String>> {
        Err(ProviderError::Unsupported("await_tasks"))
    }
    async fn stop_subagent(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
        _: Option<IsoDateTime>,
    ) -> ProviderResult<Option<SubagentOutcome>> {
        Err(ProviderError::Unsupported("stop_subagent"))
    }
    async fn upload_feedback(
        &self,
        _: OrchestrationV2ProviderThread,
        _: Option<String>,
    ) -> ProviderResult<String> {
        Err(ProviderError::Unsupported("upload_feedback"))
    }
    async fn list_tools(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
    ) -> ProviderResult<Vec<McpTool>> {
        Err(ProviderError::Unsupported("list_tools"))
    }
    async fn call_tool(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
        _: String,
        _: serde_json::Value,
    ) -> ProviderResult<McpResult> {
        Err(ProviderError::Unsupported("call_tool"))
    }
    async fn read_resource(
        &self,
        _: OrchestrationV2ProviderThread,
        _: String,
        _: String,
    ) -> ProviderResult<Vec<serde_json::Value>> {
        Err(ProviderError::Unsupported("read_resource"))
    }
}

#[tokio::test(start_paused = true)]
async fn idle_native_unload_finishes_before_reattaching_and_resuming_the_thread() -> Result<()> {
    let (adapter, mut calls, input) = fake_with_pool(true)?;
    let (send, mut unloaded) = mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    adapter
        .open_session_with_policy
        .push(Ok(Arc::new(UnloadRuntime {
            native: adapter.runtime.clone(),
            unloaded: send,
            release: release.clone(),
            fail_archive: false,
        })));
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
        SessionManagerOptions {
            idle_timeout: std::time::Duration::from_secs(10),
            ..Default::default()
        },
        None,
    );
    let runtime = manager
        .attach_with_policy(
            input.model_selection.instance_id.clone(),
            input.clone(),
            RuntimePolicyOptions::default(),
        )
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut subscription = runtime.subscribe().await?;
    let provider_thread =
        super::provider_runtime_recovery_service_test::projection()?.provider_threads[0].clone();
    let resume = ResumeThread {
        provider_thread,
        thread_id: Some(input.thread_id.clone()),
        model_selection: Some(input.model_selection.clone()),
        runtime_policy: Some(input.runtime_policy.clone()),
    };
    runtime.resume_thread(resume.clone()).await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Resume(_))));
    runtime.start_turn(turn_input(&input, 1)?).await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Start(_))));
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("provider-thread:test".into()),
                provider_turn_id: ProviderTurnId("turn".into()),
                run_ordinal: 1,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Completed { reason: None },
            },
        })
        .await?;
    subscription.events.recv().await.unwrap()?;
    tokio::time::advance(std::time::Duration::from_secs(9)).await;
    manager.get(input.provider_session_id.clone()).await?;
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let (unloading, archive) = unloaded.recv().await.unwrap();
    assert_eq!(unloading.id, resume.provider_thread.id);
    assert!(!archive);
    let resume = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.resume_thread(resume).await })
    };
    assert!(calls.try_recv().is_err());
    release.notify_one();
    resume.await.unwrap()?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Resume(_))));
    manager.shutdown().await?;
    Ok(())
}

#[async_trait]
impl SessionRuntime for LifecycleRuntime {
    fn session(&self) -> &OrchestrationV2ProviderSession {
        self.native.session()
    }
    fn can_resume_after_restart(&self) -> bool {
        true
    }
    fn background_tasks_end_with_process(&self) -> bool {
        true
    }
    async fn subscribe(&self) -> ProviderResult<EventSubscription> {
        if self.fail_subscription {
            return Err(ProviderError::SubscriptionClosed);
        }
        self.native.subscribe().await
    }
    async fn close(&self) -> ProviderResult<()> {
        let action = self
            .closes
            .lock()
            .expect("native cleanup script")
            .pop_front();
        match action {
            Some(CloseAction::Fail) => {
                return Err(ProviderError::Operation {
                    operation: ProviderOperation::Close,
                    detail: "close failed".into(),
                });
            }
            Some(CloseAction::Wait(entered, release)) => {
                entered.notify_one();
                release.notified().await;
            }
            None => {}
        }
        self.native.close().await
    }
    async fn ensure_thread(
        &self,
        input: EnsureThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.native.ensure_thread(input).await
    }
    async fn resume_thread(
        &self,
        input: ResumeThread,
    ) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.native.resume_thread(input).await
    }
    async fn start_turn(&self, input: TurnInput) -> ProviderResult<()> {
        self.native.start_turn(input).await
    }
    async fn steer_turn(&self, input: SteerInput) -> ProviderResult<()> {
        self.native.steer_turn(input).await
    }
    async fn interrupt_turn(&self, input: InterruptInput) -> ProviderResult<()> {
        self.native.interrupt_turn(input).await
    }
    async fn respond(&self, input: RuntimeResponse) -> ProviderResult<()> {
        self.native.respond(input).await
    }
    async fn snapshot(
        &self,
        input: OrchestrationV2ProviderThread,
    ) -> ProviderResult<ThreadSnapshot> {
        self.native.snapshot(input).await
    }
    async fn rollback(&self, input: RollbackInput) -> ProviderResult<ThreadSnapshot> {
        self.native.rollback(input).await
    }
    async fn fork(&self, input: ForkInput) -> ProviderResult<OrchestrationV2ProviderThread> {
        self.native.fork(input).await
    }
}

#[tokio::test]
async fn failed_native_close_keeps_the_writer_reserved_until_a_successful_retry() -> Result<()> {
    let (adapter, mut calls, mut input) = fake()?;
    input.initial_native_thread_id = Some("native:retry-close".into());
    let runtime = Arc::new(LifecycleRuntime {
        native: adapter.runtime.clone(),
        closes: std::sync::Mutex::new([CloseAction::Fail].into()),
        fail_subscription: false,
    });
    adapter.open_session_with_policy.push(Ok(runtime));
    let authority = Arc::new(LeaseAuthority::default());
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: None,
            credentials: Some(Arc::new(LeaseCredentials(authority.clone()))),
        }),
    );
    manager
        .attach_with_policy(
            input.model_selection.instance_id.clone(),
            input.clone(),
            RuntimePolicyOptions::default(),
        )
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    assert!(
        manager
            .detach(input.provider_session_id.clone(), false)
            .await
            .is_err()
    );
    assert!(
        manager
            .get(input.provider_session_id.clone())
            .await?
            .is_none()
    );
    let mut replacement = input.clone();
    assert_eq!(authority.state.lock().unwrap().owners.len(), 1);
    assert_eq!(authority.state.lock().unwrap().revoked, 0);
    replacement.provider_session_id = ProviderSessionId("session:replacement".into());
    assert!(matches!(
        manager
            .attach(replacement.model_selection.instance_id.clone(), replacement)
            .await,
        Err(t3_orchestration::error::Error::Provider(
            ProviderError::ThreadInUse(_)
        ))
    ));
    manager
        .detach(input.provider_session_id.clone(), false)
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(!manager.is_live(&input.provider_session_id));
    assert_eq!(authority.state.lock().unwrap().revoked, 1);
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn subscription_setup_failure_awaits_native_cleanup_before_returning() -> Result<()> {
    use std::{future::Future, task::Poll};
    let (adapter, mut calls, input) = fake()?;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let runtime = Arc::new(LifecycleRuntime {
        native: adapter.runtime.clone(),
        closes: std::sync::Mutex::new([CloseAction::Wait(entered.clone(), release.clone())].into()),
        fail_subscription: true,
    });
    adapter.open_session_with_policy.push(Ok(runtime));
    let authority = Arc::new(LeaseAuthority::default());
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: None,
            credentials: Some(Arc::new(LeaseCredentials(authority.clone()))),
        }),
    );
    let attach = manager.attach_with_policy(
        input.model_selection.instance_id.clone(),
        input.clone(),
        RuntimePolicyOptions::default(),
    );
    tokio::pin!(attach);
    tokio::select! { _ = entered.notified() => {}, _ = &mut attach => panic!("attachment returned before native cleanup") }
    assert_eq!(authority.state.lock().unwrap().reservations, 1);
    assert_eq!(authority.state.lock().unwrap().revoked, 0);
    assert!(matches!(
        std::future::poll_fn(|cx| Poll::Ready(attach.as_mut().poll(cx))).await,
        Poll::Pending
    ));
    release.notify_one();
    assert!(matches!(
        attach.await,
        Err(t3_orchestration::error::Error::Provider(
            ProviderError::SubscriptionClosed
        ))
    ));
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(!manager.is_live(&input.provider_session_id));
    assert_eq!(authority.state.lock().unwrap().reservations, 0);
    assert_eq!(authority.state.lock().unwrap().revoked, 1);
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn only_a_provider_announced_stop_ends_the_stream_without_a_failure() -> Result<()> {
    for announced in [false, true] {
        let (adapter, mut calls, input) = fake()?;
        let manager = ProviderSessionManager::new(
            Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
            None,
        );
        let runtime = manager
            .attach(input.model_selection.instance_id.clone(), input.clone())
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
        assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
        let mut subscription = runtime.subscribe().await?;
        if announced {
            let mut session = runtime.session().clone();
            session.status = ProviderSessionStatus::Stopped;
            adapter
                .runtime
                .emit(ProviderEvent {
                    driver: ProviderDriverKind("codex".into()),
                    kind: ProviderEventKind::Session(Box::new(session)),
                })
                .await?;
            assert!(matches!(
                subscription.events.recv().await,
                Some(Ok(ProviderEvent {
                    kind: ProviderEventKind::Session(_),
                    ..
                }))
            ));
        }
        adapter.runtime.close().await?;
        let end = subscription.events.recv().await;
        if announced {
            assert!(end.is_none());
        } else {
            assert!(matches!(
                end,
                Some(Err(ProviderError::Operation {
                    operation: ProviderOperation::Stream,
                    ..
                }))
            ));
        }
        manager.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn exclusive_release_drains_queued_events_and_only_intentional_detach_names_its_thread()
-> Result<()> {
    use t3_orchestration::ports::runs_ingestion::run_execution_service::ProviderStreamContext;
    for release in ["detach", "stream failure", "provider stop", "early close"] {
        let (adapter, mut calls, input) = fake()?;
        let manager = ProviderSessionManager::new(
            Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
            None,
        );
        let runtime = manager
            .attach(input.model_selection.instance_id.clone(), input.clone())
            .await?;
        assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
        assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
        let mut subscription = runtime.subscribe().await?;
        let mut observed = runtime.subscribe().await?;
        let queued = ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::BackgroundWork {
                provider_turn_id: ProviderTurnId("queued-before-release".into()),
                pending: false,
            },
        };
        adapter.runtime.emit(queued.clone()).await?;
        assert_eq!(observed.events.recv().await.unwrap()?, queued);
        observed.close();
        if release == "early close" {
            assert_eq!(subscription.events.recv().await.unwrap()?, queued);
            subscription.close();
        }
        if release == "detach" || release == "early close" {
            SessionDirectory::detach_thread(
                &manager,
                DetachSession {
                    provider_session_id: input.provider_session_id.clone(),
                    thread_id: input.thread_id.clone(),
                    detail: Some("Workspace changed.".into()),
                    revoke_credential: false,
                },
            )
            .await?;
            if release == "detach" {
                assert!(
                    !subscription.cancellation.is_cancelled(),
                    "detach must let the subscriber consume its queued failure"
                );
            }
        } else {
            if release == "provider stop" {
                let mut session = runtime.session().clone();
                session.status = ProviderSessionStatus::Stopped;
                adapter
                    .runtime
                    .emit(ProviderEvent {
                        driver: ProviderDriverKind("codex".into()),
                        kind: ProviderEventKind::Session(Box::new(session)),
                    })
                    .await?;
            }
            adapter.runtime.close().await?;
        }
        if release != "early close" {
            assert_eq!(subscription.events.recv().await.unwrap()?, queued);
            if release == "provider stop" {
                assert!(matches!(
                    subscription.events.recv().await,
                    Some(Ok(ProviderEvent {
                        kind: ProviderEventKind::Session(_),
                        ..
                    }))
                ));
            }
            let terminal = subscription.events.recv().await;
            if release == "provider stop" {
                assert!(terminal.is_none());
            } else {
                let error = terminal
                    .unwrap_or_else(|| {
                        panic!("{release}: release must fail the subscriber after queued events")
                    })
                    .expect_err("release must not fabricate a provider terminal");
                assert_eq!(
                    manager.detached_thread_id(&error),
                    (release == "detach").then(|| input.thread_id.clone())
                );
                let detached = match error {
                    ProviderError::Protocol {
                        payload: Some(payload),
                        ..
                    } if payload["_tag"] == "ProviderAdapterEventStreamError" => {
                        assert_eq!(
                            payload["providerSessionId"],
                            json!(input.provider_session_id)
                        );
                        payload.get("detachedThreadId").cloned()
                    }
                    _ => None,
                };
                assert_eq!(
                    detached,
                    (release == "detach").then(|| json!(input.thread_id)),
                    "{release}"
                );
                assert!(subscription.events.recv().await.is_none());
            }
        }
        manager.shutdown().await?;
        assert!(!manager.is_live(&input.provider_session_id));
        assert!(
            manager
                .detached_thread_id(&ProviderError::Protocol {
                    reason: None,
                    detail: "Unmarked provider error".into(),
                    payload: Some(json!({"detachedThreadId":input.thread_id})),
                })
                .is_none()
        );
    }
    Ok(())
}

pub(super) fn sample<T: DeserializeOwned>(name: &str) -> Result<T> {
    let data = match name {
        "OrchestrationV2ProviderSession" => {
            include_str!("../../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
        }
        "OrchestrationV2ProviderThread" => {
            include_str!("../../../../fixtures/samples/OrchestrationV2ProviderThread.jsonl")
        }
        "OrchestrationV2AppThread" => {
            include_str!("../../../../fixtures/samples/OrchestrationV2AppThread.jsonl")
        }
        "OrchestrationV2Subagent" => {
            include_str!("../../../../fixtures/samples/OrchestrationV2Subagent.jsonl")
        }
        _ => return Err(t3_orchestration::error::Error::Unsupported("test fixture")),
    };
    Ok(serde_json::from_str(
        data.lines().next().expect("golden fixture"),
    )?)
}

pub(super) fn fake() -> Result<(
    Arc<FakeProviderAdapter>,
    mpsc::UnboundedReceiver<FakeCall>,
    OpenSession,
)> {
    fake_with_pool(false)
}

fn fake_with_pool(
    pooled: bool,
) -> Result<(
    Arc<FakeProviderAdapter>,
    mpsc::UnboundedReceiver<FakeCall>,
    OpenSession,
)> {
    fake_named(pooled, "session:test")
}

fn fake_named(
    pooled: bool,
    session_id: &str,
) -> Result<(
    Arc<FakeProviderAdapter>,
    mpsc::UnboundedReceiver<FakeCall>,
    OpenSession,
)> {
    let mut session = serde_json::to_value(sample::<OrchestrationV2ProviderSession>(
        "OrchestrationV2ProviderSession",
    )?)?;
    for (key, value) in [
        ("id", json!(session_id)),
        ("providerInstanceId", json!("codex")),
        ("driver", json!("codex")),
        ("status", json!("ready")),
        ("cwd", json!("/repo")),
        ("createdAt", json!(TEST_TIME)),
        ("updatedAt", json!(TEST_TIME)),
    ] {
        session[key] = value;
    }
    session["capabilities"]["sessions"]["supportsMultipleProviderThreadsPerSession"] =
        json!(pooled);
    let session: OrchestrationV2ProviderSession = serde_json::from_value(session)?;
    let mut thread = serde_json::to_value(sample::<OrchestrationV2ProviderThread>(
        "OrchestrationV2ProviderThread",
    )?)?;
    for (key, value) in [
        ("id", json!("provider-thread:test")),
        ("providerSessionId", json!(session_id)),
        ("providerInstanceId", json!("codex")),
        ("appThreadId", json!("thread:test")),
        ("ownerNodeId", json!(null)),
        ("driver", json!("codex")),
        ("status", json!("active")),
        ("createdAt", json!(TEST_TIME)),
        ("updatedAt", json!(TEST_TIME)),
    ] {
        thread[key] = value;
    }
    let (adapter, calls) = FakeProviderAdapter::new(
        ProviderInstanceId("codex".into()),
        ProviderDriverKind("codex".into()),
        session.capabilities.clone(),
        session,
        ThreadSnapshot {
            provider_thread: serde_json::from_value(thread)?,
            provider_turns: vec![],
            messages: vec![],
            reasoning_message_ids: None,
            runtime_requests: vec![],
            provider_payload: None,
        },
    );
    let input = OpenSession {
        thread_id: ThreadId("thread:test".into()),
        provider_session_id: ProviderSessionId(session_id.into()),
        model_selection: serde_json::from_value(json!({"instanceId":"codex","model":"test"}))?,
        runtime_policy: RuntimePolicy {
            runtime_mode: serde_json::from_value(json!("full-access"))?,
            interaction_mode: serde_json::from_value(json!("default"))?,
            cwd: None,
            approval_policy: None,
            sandbox_policy: None,
            max_runtime_mode: None,
        },
        prompt_suggestions: None,
        resume_from_session: None,
        initial_native_thread_id: None,
        initial_provider_item_identity_version: None,
    };
    Ok((Arc::new(adapter), calls, input))
}

pub(super) fn turn_input(input: &OpenSession, ordinal: u64) -> Result<TurnInput> {
    let projection = super::provider_runtime_recovery_service_test::projection()?;
    Ok(TurnInput {
        app_thread: projection.thread,
        thread_id: input.thread_id.clone(),
        run_id: RunId(format!("run:{ordinal}")),
        run_ordinal: ordinal,
        native_thread_has_turns: None,
        provider_turn_ordinal: ordinal,
        restart_continuation_of_run_id: None,
        attempt_id: RunAttemptId(format!("attempt:{ordinal}")),
        root_node_id: NodeId("root".into()),
        provider_thread: projection.provider_threads[0].clone(),
        message: TurnMessage {
            message_id: MessageId("message".into()),
            text: "Work".into(),
            attachments: vec![],
            created_by: OrchestrationV2Actor::User,
            creation_source: None,
            scheduled_task_id: None,
            sender_thread_id: None,
        },
        model_selection: input.model_selection.clone(),
        runtime_policy: input.runtime_policy.clone(),
        app_context: vec![],
        inline_context_length: None,
    })
}

struct RetentionGate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl t3_orchestration::api::SessionRetentionCheck for RetentionGate {
    async fn keep(&self) -> Result<bool> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(false)
    }
}

#[tokio::test]
async fn release_retention_check_excludes_new_turn_admission() -> Result<()> {
    use std::{future::Future, task::Poll};
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let check = Arc::new(RetentionGate {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let releasing = {
        let manager = manager.clone();
        let check = check.clone();
        let session = input.provider_session_id.clone();
        tokio::spawn(async move {
            SessionDirectory::release_unless_busy(
                &manager,
                t3_orchestration::api::ReleaseSession {
                    provider_session_id: session,
                    own_provider_thread_ids: vec![],
                    through_run_ordinal: 0,
                    reason: "manual_shutdown".into(),
                    detail: None,
                    keep_if: Some(check),
                },
            )
            .await
        })
    };
    check.entered.notified().await;
    let start = runtime.start_turn(turn_input(&input, 1)?);
    tokio::pin!(start);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(start.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    check.release.notify_one();
    assert!(releasing.await.expect("release task")?);
    assert!(matches!(start.await, Err(ProviderError::SessionStopped)));
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(calls.try_recv().is_err());
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn managed_operations_forward_reasoning_and_reload_when_it_changes() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let thread = super::provider_runtime_recovery_service_test::projection()?
        .provider_threads
        .remove(0);
    let options = |effort: &str| RuntimePolicyOptions {
        reasoning_effort: Some(effort.into()),
    };
    adapter
        .runtime
        .ensure_thread_with_policy
        .push(Ok(thread.clone()));
    runtime
        .ensure_thread_with_policy(
            EnsureThread {
                thread_id: input.thread_id.clone(),
                title: None,
                model_selection: input.model_selection.clone(),
                runtime_policy: input.runtime_policy.clone(),
                provider_session_id: Some(input.provider_session_id.clone()),
                existing_provider_thread: None,
                force_new_native_thread: false,
            },
            options("high"),
        )
        .await?;
    assert_eq!(
        adapter
            .runtime
            .ensure_thread_with_policy
            .next_call()
            .await
            .expect("ensure")
            .1
            .reasoning_effort
            .as_deref(),
        Some("high")
    );
    let resume = ResumeThread {
        provider_thread: thread.clone(),
        thread_id: Some(input.thread_id.clone()),
        model_selection: Some(input.model_selection.clone()),
        runtime_policy: Some(input.runtime_policy.clone()),
    };
    adapter
        .runtime
        .resume_thread_with_policy
        .push(Ok(thread.clone()));
    runtime
        .resume_thread_with_policy(resume.clone(), options("low"))
        .await?;
    assert_eq!(
        adapter
            .runtime
            .resume_thread_with_policy
            .next_call()
            .await
            .expect("resume")
            .1
            .reasoning_effort
            .as_deref(),
        Some("low")
    );
    runtime
        .resume_thread_with_policy(resume, options("low"))
        .await?;
    adapter.runtime.fork_with_policy.push(Ok(thread.clone()));
    runtime
        .fork_with_policy(
            ForkInput {
                source_provider_thread: thread,
                source_provider_turns: None,
                provider_turn_id: None,
                target_thread_id: input.thread_id.clone(),
                owner_node_id: None,
                model_selection: Some(input.model_selection.clone()),
                runtime_policy: Some(input.runtime_policy.clone()),
            },
            options("medium"),
        )
        .await?;
    assert_eq!(
        adapter
            .runtime
            .fork_with_policy
            .next_call()
            .await
            .expect("fork")
            .1
            .reasoning_effort
            .as_deref(),
        Some("medium")
    );
    adapter.runtime.start_turn_with_policy.push(Ok(()));
    runtime
        .start_turn_with_policy(turn_input(&input, 1)?, options("high"))
        .await?;
    assert_eq!(
        adapter
            .runtime
            .start_turn_with_policy
            .next_call()
            .await
            .expect("start")
            .1
            .reasoning_effort
            .as_deref(),
        Some("high")
    );
    assert!(calls.try_recv().is_err());
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stale_terminal_cannot_clear_a_newer_busy_turn_or_release_it() -> Result<()> {
    use std::time::Duration;
    use t3_orchestration::ports::sessions_recovery::provider_session_manager::SessionManagerOptions;
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
        SessionManagerOptions {
            idle_timeout: Duration::from_secs(10),
            ..Default::default()
        },
        None,
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut subscription = runtime.subscribe().await?;
    runtime.start_turn(turn_input(&input, 2)?).await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Start(_))));
    let terminal = |ordinal| ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::Terminal {
            provider_thread_id: ProviderThreadId("provider-thread:test".into()),
            provider_turn_id: ProviderTurnId(format!("turn:{ordinal}")),
            run_ordinal: ordinal,
            disposition: ThreadDisposition::Reusable,
            outcome: TerminalOutcome::Completed { reason: None },
        },
    };
    adapter.runtime.emit(terminal(1)).await?;
    subscription.events.recv().await.expect("terminal")?;
    tokio::time::advance(Duration::from_secs(20)).await;
    assert!(manager.is_live(&input.provider_session_id));
    assert!(
        !manager
            .release_unless_busy(
                input.provider_session_id.clone(),
                &[ProviderThreadId("provider-thread:test".into())],
                1
            )
            .await?
    );
    adapter.runtime.emit(terminal(2)).await?;
    subscription.events.recv().await.expect("terminal")?;
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn suggestions_survive_session_release_and_replace_by_thread() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
    );
    manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut suggestion = manager
        .subscribe_prompt_suggestion(input.thread_id.clone())
        .await?;
    assert_eq!(*suggestion.borrow_and_update(), None);
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::PromptSuggestion {
                thread_id: input.thread_id.clone(),
                run_id: RunId("run:1".into()),
                text: "Continue".into(),
            },
        })
        .await?;
    suggestion.changed().await.expect("suggestion update");
    manager.detach(input.provider_session_id, false).await?;
    assert_eq!(
        manager.prompt_suggestion(input.thread_id).await?,
        Some(OrchestrationV2PromptSuggestionValue {
            run_id: RunId("run:1".into()),
            text: "Continue".into()
        })
    );
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn release_persists_stopped_session_and_closes_live_requests_in_sqlite() -> Result<()> {
    use super::provider_runtime_recovery_service_test::{item, projection, seed, sink};
    use t3_orchestration::{
        ports::sessions_recovery::provider_session_manager::{
            SessionManagerOptions, SessionPersistence, SessionServices,
        },
        projections::store::ProjectionStore,
    };
    let database = TestDatabase::memory()?;
    let mut projection = projection()?;
    projection.runtime_requests = vec![serde_json::from_value(
        json!({"id":"request","providerTurnId":null,"nodeId":"request-node","kind":"user_input","nativeRequestRef":null,"status":"pending","responseCapability":{"type":"live","providerSessionId":"session:test"},"createdAt":TEST_TIME,"resolvedAt":null}),
    )?];
    let mut request_item = item("question", "user_input_request");
    request_item["requestId"] = json!("request");
    request_item["nodeId"] = json!("request-node");
    projection.turn_items = vec![serde_json::from_value(request_item)?];
    let committer = Arc::new(sink(database.stores.clone()));
    seed(&committer, &projection).await?;
    let reader = Arc::new(ProjectionStore::new(database.stores.clone()));
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: Some(SessionPersistence {
                reader: reader.clone(),
                sink: committer,
                clock: Arc::new(ManualClock::new(TEST_TIME)),
                ids: Arc::new(SequenceIds::new("release")),
            }),
            credentials: None,
        }),
    );
    manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    manager.detach(input.provider_session_id, false).await?;
    let stored = reader
        .projection(input.thread_id)
        .await?
        .expect("projection");
    assert_eq!(
        stored.provider_sessions[0].status,
        ProviderSessionStatus::Stopped
    );
    assert_eq!(
        stored.runtime_requests[0].status,
        RuntimeRequestStatus::Cancelled
    );
    let question = serde_json::to_value(&stored.turn_items[0])?;
    assert_eq!(question["status"], "cancelled");
    assert_eq!(question["completedAt"], TEST_TIME);
    database.stores.read(|db| {
        for kind in ["provider-session.updated", "runtime-request.updated", "turn-item.updated"] {
            let metadata: String = db.query_row("SELECT metadata_json FROM orchestration_events WHERE stream_id='thread:test' AND event_type=?1 ORDER BY sequence DESC LIMIT 1", [kind], |row| row.get(0))?;
            let metadata: serde_json::Value = serde_json::from_str(&metadata)?;
            assert_eq!(metadata["driver"], "codex");
            if kind == "provider-session.updated" { assert_eq!(metadata["providerInstanceId"], "codex"); }
            else { assert_eq!(metadata["nodeId"], "request-node"); }
        }
        Ok(())
    }).await?;
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn failed_ingestion_releases_the_session_as_error_instead_of_manual_stop() -> Result<()> {
    use super::provider_runtime_recovery_service_test::{projection, seed, sink};
    use t3_orchestration::projections::store::ProjectionStore;
    let database = TestDatabase::memory()?;
    let sink = Arc::new(sink(database.stores.clone()));
    seed(&sink, &projection()?).await?;
    let ingestor = Arc::new(FakeEventIngestor::default());
    ingestor
        .ingest
        .push(Err(t3_orchestration::error::Error::Service {
            operation: "fixture ingest",
            detail: "ingest failure".into(),
        }));
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        Some(ingestor),
        SessionManagerOptions::default(),
        Some(SessionServices {
            persistence: Some(SessionPersistence {
                reader: Arc::new(ProjectionStore::new(database.stores)),
                sink: sink.clone(),
                clock: Arc::new(ManualClock::new(TEST_TIME)),
                ids: Arc::new(SequenceIds::new("stream-failure")),
            }),
            credentials: None,
        }),
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input)
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut persisted = sink.subscribe();
    let mut subscription = runtime.subscribe().await?;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::Thread {
                provider_thread: Box::new(projection()?.provider_threads[0].clone()),
                pending_background_work: None,
                session_scoped: Some(true),
                retained_native_turn_ids: None,
            },
        })
        .await?;
    loop {
        let event = persisted.recv().await.expect("release event");
        if let t3_contracts::application_event::ApplicationStoredEvent::Orchestration(stored) =
            event
            && let OrchestrationV2DomainEvent::ProviderSessionUpdated(event) = stored.event
        {
            assert_eq!(event.payload.status, ProviderSessionStatus::Error);
            assert!(
                event
                    .payload
                    .last_error
                    .as_ref()
                    .expect("failure detail")
                    .contains("ingest failure")
            );
            break;
        }
    }
    assert!(
        subscription
            .events
            .recv()
            .await
            .expect("stream failure")
            .is_err()
    );
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn pooled_attachment_reuses_process_and_detaching_one_thread_keeps_the_other() -> Result<()> {
    let (adapter, mut calls, input) = fake_with_pool(true)?;
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
    );
    let first = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    let mut other = input.clone();
    other.thread_id = ThreadId("thread:other".into());
    let second = manager
        .attach(other.model_selection.instance_id.clone(), other.clone())
        .await?;
    assert!(Arc::ptr_eq(&first, &second));
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    assert!(calls.try_recv().is_err());
    let mut subscription = second.subscribe().await?;
    manager
        .detach_thread(input.provider_session_id.clone(), input.thread_id, false)
        .await?;
    assert!(manager.is_live(&input.provider_session_id));
    assert!(calls.try_recv().is_err());
    let event = ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::BackgroundWork {
            provider_turn_id: ProviderTurnId("other-owner".into()),
            pending: false,
        },
    };
    adapter.runtime.emit(event.clone()).await?;
    assert_eq!(subscription.events.recv().await.unwrap()?, event);
    manager.shutdown().await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn idle_expiry_is_reset_by_lookup_and_closes_the_native_runtime() -> Result<()> {
    use std::time::Duration;
    use t3_orchestration::ports::sessions_recovery::provider_session_manager::SessionManagerOptions;
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
        SessionManagerOptions {
            idle_timeout: Duration::from_secs(10),
            ..Default::default()
        },
        None,
    );
    manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(
        manager
            .get(input.provider_session_id.clone())
            .await?
            .is_some()
    );
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(manager.is_live(&input.provider_session_id));
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(!manager.is_live(&input.provider_session_id));
    manager.shutdown().await?;
    Ok(())
}

struct RecordingIngestor(mpsc::UnboundedSender<ProviderEvent>);

#[tokio::test]
async fn session_artifacts_are_ingested_once_while_run_events_reach_the_run_subscriber()
-> Result<()> {
    let (adapter, mut calls, input) = fake_with_pool(true)?;
    let (send, mut ingested) = mpsc::unbounded_channel();
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        Some(Arc::new(RecordingIngestor(send))),
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut subscription = runtime.subscribe().await?;
    let projection = super::provider_runtime_recovery_service_test::projection()?;
    let artifact = ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::Thread {
            provider_thread: Box::new(projection.provider_threads[0].clone()),
            pending_background_work: None,
            session_scoped: Some(true),
            retained_native_turn_ids: None,
        },
    };
    adapter.runtime.emit(artifact.clone()).await?;
    assert_eq!(ingested.recv().await, Some(artifact));
    let turn_event = ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::BackgroundWork {
            provider_turn_id: ProviderTurnId("turn".into()),
            pending: true,
        },
    };
    adapter.runtime.emit(turn_event.clone()).await?;
    assert_eq!(subscription.events.recv().await.unwrap()?, turn_event);
    assert!(ingested.try_recv().is_err());
    assert!(subscription.events.try_recv().is_err());
    manager.shutdown().await?;
    Ok(())
}

struct RoutedAdapter {
    first: Arc<FakeProviderAdapter>,
    second: Arc<FakeProviderAdapter>,
}

#[async_trait]
impl ProviderAdapter for RoutedAdapter {
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        None
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        self.first.instance_id()
    }
    fn driver(&self) -> ProviderDriverKind {
        self.first.driver()
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        self.first.capabilities().await
    }
    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        self.first.plan_selection_transition(input).await
    }
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        if serde_json::to_value(&self.first.runtime.session().id).expect("session ID")
            == json!(input.provider_session_id)
        {
            self.first.open_session(input, cancellation).await
        } else {
            self.second.open_session(input, cancellation).await
        }
    }
}

struct Credentials {
    next: std::sync::atomic::AtomicUsize,
    reuse: bool,
    revoked: std::sync::Mutex<Vec<(ThreadId, String)>>,
}

#[async_trait]
impl t3_orchestration::ports::sessions_recovery::provider_session_manager::SessionCredentials
    for Credentials
{
    async fn prepare(
        &self,
        _thread: ThreadId,
        _instance: ProviderInstanceId,
    ) -> Result<
        t3_orchestration::ports::sessions_recovery::provider_session_manager::PreparedCredential,
    > {
        let next = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(t3_orchestration::ports::sessions_recovery::provider_session_manager::PreparedCredential {id:format!("credential:{}",if self.reuse {0} else {next}),issued:true})
    }
    async fn revoke(&self, thread: ThreadId, credential: String) -> Result<()> {
        self.revoked
            .lock()
            .expect("credentials")
            .push((thread, credential));
        Ok(())
    }
}

#[tokio::test]
async fn release_revokes_only_credentials_not_held_by_the_replacement() -> Result<()> {
    use t3_orchestration::ports::sessions_recovery::provider_session_manager::{
        SessionManagerOptions, SessionServices,
    };
    for reuse in [false, true] {
        let (first, mut first_calls, input) = fake_named(false, "session:first")?;
        let (second, mut second_calls, replacement) = fake_named(false, "session:second")?;
        let credentials = Arc::new(Credentials {
            next: std::sync::atomic::AtomicUsize::new(0),
            reuse,
            revoked: std::sync::Mutex::new(vec![]),
        });
        let manager = ProviderSessionManager::with_options(
            Arc::new(AdapterRegistry::from_adapters(vec![Arc::new(
                RoutedAdapter { first, second },
            )])),
            None,
            SessionManagerOptions::default(),
            Some(SessionServices {
                persistence: None,
                credentials: Some(credentials.clone()),
            }),
        );
        manager
            .attach(input.model_selection.instance_id.clone(), input.clone())
            .await?;
        manager
            .attach(
                replacement.model_selection.instance_id.clone(),
                replacement.clone(),
            )
            .await?;
        for calls in [&mut first_calls, &mut second_calls] {
            assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
            assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
        }
        manager
            .detach(input.provider_session_id.clone(), false)
            .await?;
        assert!(matches!(first_calls.recv().await, Some(FakeCall::Close)));
        assert_eq!(
            credentials.revoked.lock().expect("revocations").as_slice(),
            if reuse {
                vec![]
            } else {
                vec![(input.thread_id.clone(), "credential:0".into())]
            }
        );
        assert!(manager.is_live(&replacement.provider_session_id));
        manager.detach(input.provider_session_id, false).await?;
        manager
            .detach(replacement.provider_session_id, false)
            .await?;
        assert!(matches!(second_calls.recv().await, Some(FakeCall::Close)));
        assert_eq!(
            credentials.revoked.lock().expect("revocations").len(),
            if reuse { 1 } else { 2 }
        );
        manager.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn manager_forwards_reasoning_when_opening_the_native_session() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    adapter
        .open_session_with_policy
        .push(Ok(adapter.runtime.clone()));
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
    );
    manager
        .attach_with_policy(
            input.model_selection.instance_id.clone(),
            input,
            RuntimePolicyOptions {
                reasoning_effort: Some("high".into()),
            },
        )
        .await?;
    assert_eq!(
        adapter
            .open_session_with_policy
            .next_call()
            .await
            .expect("open")
            .1
            .reasoning_effort
            .as_deref(),
        Some("high")
    );
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    manager.shutdown().await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stalled_retry_stop_preserves_the_retry_failure_through_interrupted_terminal_events()
-> Result<()> {
    use t3_orchestration::ports::sessions_recovery::provider_session_manager::SessionManagerOptions;
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::with_options(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
        SessionManagerOptions {
            retry_stall_timeout: std::time::Duration::from_secs(10),
            idle_timeout: std::time::Duration::from_secs(100),
            ..Default::default()
        },
        None,
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let mut subscription = runtime.subscribe().await?;
    runtime
        .ensure_thread(EnsureThread {
            thread_id: input.thread_id.clone(),
            title: None,
            model_selection: input.model_selection.clone(),
            runtime_policy: input.runtime_policy.clone(),
            provider_session_id: Some(input.provider_session_id.clone()),
            existing_provider_thread: None,
            force_new_native_thread: false,
        })
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Ensure(_))));
    runtime.start_turn(turn_input(&input, 1)?).await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Start(_))));
    let mut retry = super::provider_runtime_recovery_service_test::item("retry", "error");
    retry["providerTurnId"] = json!("turn:retry");
    retry["ordinal"] = json!(3);
    retry["failure"] = json!({"class":"usage_limit","message":"Rate limit exceeded","code":"rate_limited","retryable":true});
    retry["retry"] = json!({"attempt":2,"maxAttempts":5,"retryDelayMs":1000});
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::TurnItem {
                item: Box::new(serde_json::from_value(retry.clone())?),
                session_scoped: None,
            },
        })
        .await?;
    subscription.events.recv().await.expect("retry event")?;
    let Some(FakeCall::Interrupt(interrupt)) = calls.recv().await else {
        panic!("retry watchdog did not interrupt the native turn")
    };
    assert_eq!(
        interrupt.provider_turn_id,
        ProviderTurnId("turn:retry".into())
    );
    assert!(!interrupt.request_runtime_restart);
    retry["status"] = json!("interrupted");
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::TurnItem {
                item: Box::new(serde_json::from_value(retry)?),
                session_scoped: None,
            },
        })
        .await?;
    subscription.events.recv().await.expect("settled retry")?;
    adapter.runtime.emit(ProviderEvent {driver:ProviderDriverKind("codex".into()),kind:ProviderEventKind::Turn {provider_turn:Box::new(serde_json::from_value(json!({"id":"turn:retry","providerThreadId":"provider-thread:test","nodeId":"node:test","runAttemptId":null,"nativeTurnRef":null,"ordinal":1,"status":"interrupted","startedAt":TEST_TIME,"completedAt":TEST_TIME}))?),thread_id:Some(input.thread_id)}}).await?;
    let event = subscription.events.recv().await.expect("provider turn")?;
    let ProviderEventKind::Turn { provider_turn, .. } = event.kind else {
        panic!("provider turn")
    };
    assert_eq!(provider_turn.status, ProviderTurnStatus::Failed);
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: ProviderDriverKind("codex".into()),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("provider-thread:test".into()),
                provider_turn_id: ProviderTurnId("turn:retry".into()),
                run_ordinal: 1,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Interrupted { reason: None },
            },
        })
        .await?;
    let event = subscription.events.recv().await.expect("terminal")?;
    let ProviderEventKind::Terminal {
        outcome:
            TerminalOutcome::Failed {
                failure,
                failure_item_ordinal,
                retry,
                retry_started_at,
                ..
            },
        ..
    } = event.kind
    else {
        panic!("retry failure terminal")
    };
    assert_eq!(failure.retryable, Some(false));
    assert_eq!(failure.message.0, "Rate limit exceeded");
    assert_eq!(failure_item_ordinal, 3);
    assert_eq!(serde_json::to_value(retry)?["attempt"], 2);
    assert_eq!(retry_started_at.expect("retry start").0, TEST_TIME);
    manager.shutdown().await?;
    Ok(())
}
#[async_trait]
impl EventIngestor for RecordingIngestor {
    async fn ingest(
        &self,
        _: ProviderSessionId,
        _: ProviderInstanceId,
        _: ThreadId,
        event: ProviderEvent,
    ) -> Result<()> {
        self.0.send(event).expect("test receiver");
        Ok(())
    }
}

#[tokio::test]
async fn concurrent_attaches_open_once_and_shutdown_awaits_native_close() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    let registry = Arc::new(AdapterRegistry::from_adapters(vec![adapter]));
    let manager = ProviderSessionManager::new(registry, None);
    let (first, second) = tokio::join!(
        manager.attach(input.model_selection.instance_id.clone(), input.clone()),
        manager.attach(input.model_selection.instance_id.clone(), input.clone())
    );
    let first = first?;
    assert!(Arc::ptr_eq(&first, &second?));
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    assert!(calls.try_recv().is_err());
    let subscription = first.subscribe().await?;
    manager.shutdown().await?;
    assert!(subscription.cancellation.is_cancelled());
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(!manager.is_live(&input.provider_session_id));
    assert!(
        manager
            .attach(input.model_selection.instance_id.clone(), input)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn terminal_root_keeps_ingesting_background_work_until_detach() -> Result<()> {
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    let mut received = runtime.subscribe().await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let terminal = ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::Terminal {
            provider_thread_id: ProviderThreadId("provider-thread:test".into()),
            provider_turn_id: ProviderTurnId("provider-turn:test".into()),
            run_ordinal: 1,
            disposition: ThreadDisposition::Reusable,
            outcome: TerminalOutcome::Completed { reason: None },
        },
    };
    adapter.runtime.emit(terminal.clone()).await?;
    assert_eq!(received.events.recv().await.unwrap()?, terminal);
    let background = ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::BackgroundWork {
            provider_turn_id: ProviderTurnId("provider-turn:test".into()),
            pending: true,
        },
    };
    adapter.runtime.emit(background.clone()).await?;
    assert_eq!(received.events.recv().await.unwrap()?, background);
    manager.detach(input.provider_session_id, false).await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_native_thread_in_use_never_opens_a_second_writer() -> Result<()> {
    let (adapter, mut calls, mut input) = fake()?;
    input.initial_native_thread_id = Some("native:test".into());
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter])),
        None,
    );
    manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    input.provider_session_id = ProviderSessionId("session:second".into());
    let result = manager
        .attach(input.model_selection.instance_id.clone(), input)
        .await;
    assert!(matches!(
        result,
        Err(t3_orchestration::error::Error::Provider(
            ProviderError::ThreadInUse(_)
        ))
    ));
    assert!(calls.try_recv().is_err());
    manager.shutdown().await?;
    Ok(())
}

#[test]
fn delegated_work_wakes_only_while_its_result_can_still_be_delivered() -> Result<()> {
    use t3_orchestration::ports::sessions_recovery::provider_session_manager::delegated_task_will_wake;
    let mut projection = super::provider_runtime_recovery_service_test::projection()?;
    let mut task = serde_json::to_value(sample::<OrchestrationV2Subagent>(
        "OrchestrationV2Subagent",
    )?)?;
    for (key, value) in [
        ("id", json!("task")),
        ("origin", json!("app_owned")),
        ("runId", json!("run:test")),
        ("status", json!("running")),
    ] {
        task[key] = value;
    }
    task.as_object_mut()
        .expect("test value")
        .remove("completionDelivery");
    projection.subagents = vec![serde_json::from_value(task)?];
    projection.runs[0].delegated_completion = Some(Some(serde_json::from_value(
        json!({"disposition":"open","nextGeneration":1,"delivery":null}),
    )?));
    assert!(delegated_task_will_wake(&projection));
    projection.runs[0]
        .delegated_completion
        .as_mut()
        .expect("test value")
        .as_mut()
        .expect("test value")
        .disposition = DelegatedCompletionCohortDisposition::Disposed;
    assert!(!delegated_task_will_wake(&projection));
    projection.runs[0]
        .delegated_completion
        .as_mut()
        .expect("test value")
        .as_mut()
        .expect("test value")
        .disposition = DelegatedCompletionCohortDisposition::Open;
    projection.subagents[0].origin = SubagentOrigin::ProviderNative;
    assert!(!delegated_task_will_wake(&projection));
    Ok(())
}

#[tokio::test]
async fn native_stream_close_releases_the_managed_session_after_queued_events_drain() -> Result<()>
{
    let (adapter, mut calls, input) = fake()?;
    let manager = ProviderSessionManager::new(
        Arc::new(AdapterRegistry::from_adapters(vec![adapter.clone()])),
        None,
    );
    let runtime = manager
        .attach(input.model_selection.instance_id.clone(), input.clone())
        .await?;
    let mut received = runtime.subscribe().await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Open(_))));
    assert!(matches!(calls.recv().await, Some(FakeCall::Subscribe)));
    let event = ProviderEvent {
        driver: ProviderDriverKind("codex".into()),
        kind: ProviderEventKind::BackgroundWork {
            provider_turn_id: ProviderTurnId("turn".into()),
            pending: false,
        },
    };
    adapter.runtime.emit(event.clone()).await?;
    assert_eq!(received.events.recv().await.unwrap()?, event);
    adapter.runtime.close().await?;
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(matches!(calls.recv().await, Some(FakeCall::Close)));
    assert!(!manager.is_live(&input.provider_session_id));
    manager.shutdown().await?;
    Ok(())
}
