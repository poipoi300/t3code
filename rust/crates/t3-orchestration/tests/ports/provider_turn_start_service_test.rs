use super::{
    provider_event_ingestor_test::{Harness, decode, fixture},
    run_execution_service_test::root_state,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use t3_contracts::{orchestration_v2_id_table::IdOrIndex, provider_instance::ProviderInstanceId};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::{
        decisions::{runtime_policy::resolve_base, thread_command_executor::KeyedLanes},
        runs_ingestion::{
            provider_event_ingestor::ProviderEventIngestor,
            provider_turn_start_service::ProviderTurnStartService,
            run_execution_service::{RootRunState, RunExecutionService},
        },
    },
    provider::*,
    runtime::ingestion::ProviderCommitter,
    testing::*,
};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

pub struct Sessions {
    pub adapter: Arc<FakeProviderAdapter>,
    pub failure: Option<&'static str>,
    pub opening: Option<mpsc::UnboundedSender<()>>,
    pub resume_open: Option<Arc<Notify>>,
}

#[async_trait]
impl SessionDirectory for Sessions {
    async fn get(&self, id: ProviderSessionId) -> Result<Option<Arc<dyn SessionRuntime>>> {
        Ok((self.adapter.runtime.session().id == IdOrIndex::Id(id))
            .then(|| self.adapter.runtime.clone() as Arc<dyn SessionRuntime>))
    }
    async fn attach(
        &self,
        _instance: ProviderInstanceId,
        input: OpenSession,
    ) -> Result<Arc<dyn SessionRuntime>> {
        if let Some(opening) = &self.opening {
            opening.send(()).unwrap();
        }
        if let Some(resume) = &self.resume_open {
            resume.notified().await;
        }
        if let Some(failure) = self.failure {
            if failure == "sign-in" {
                return Err(ProviderError::RequiresSignIn.into());
            }
            return Err(Error::Service {
                operation: "open",
                detail: failure.into(),
            });
        }
        Ok(self
            .adapter
            .open_session(input, CancellationToken::new())
            .await?)
    }
    async fn detach(&self, _id: ProviderSessionId, _revoke: bool) -> Result<()> {
        self.adapter.runtime.close().await?;
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        self.adapter.runtime.close().await?;
        Ok(())
    }
}

pub fn execution(h: &Harness) -> Arc<RunExecutionService> {
    Arc::new(RunExecutionService::new(
        Arc::new(ProviderEventIngestor::new(
            h.reader.clone(),
            Arc::new(ProviderCommitter((*h.sink).clone())),
            h.clock.clone(),
            Arc::new(SequenceIds::new("execution")),
            Arc::new(KeyedLanes::default()),
        )),
        None,
    ))
}

pub fn service(h: &Harness, sessions: Arc<dyn SessionDirectory>) -> ProviderTurnStartService {
    ProviderTurnStartService {
        projections: h.reader.clone(),
        committer: h.sink.clone(),
        sessions,
        execution: execution(h),
        clock: h.clock.clone(),
        ids: Arc::new(SequenceIds::new("start")),
    }
}

pub async fn seed(h: &Harness) -> Result<RootRunState> {
    let mut state = root_state();
    state.run.status = OrchestrationV2RunStatus::Starting;
    state.run.started_at = None;
    state.run.completed_at = None;
    state.run.model_selection = decode(json!({"instanceId":"codex","model":"model:test"}));
    state.attempt.status = RunAttemptStatus::Pending;
    state.attempt.provider_turn_id = None;
    state.attempt.started_at = None;
    state.attempt.completed_at = None;
    state.root_node.status = ExecutionNodeStatus::Pending;
    state.root_node.checkpoint_scope_id = Some(state.checkpoint_scope.id.clone());
    state.root_node.provider_turn_id = None;
    state.provider_thread.native_thread_ref = None;
    state.provider_thread.last_run_ordinal = Some(1);
    let mut message = fixture("OrchestrationV2ConversationMessage");
    message["id"] = json!(state.run.user_message_id);
    message["threadId"] = "thread:test".into();
    message["runId"] = "run:test".into();
    message["role"] = "user".into();
    message["text"] = "Hello provider".into();
    message["attachments"] = json!([]);
    message.as_object_mut().unwrap().shift_remove("context");
    message["createdBy"] = "user".into();
    message["creationSource"] = "web".into();
    for (kind, value) in [
        ("provider-thread.updated", json!(state.provider_thread)),
        ("checkpoint-scope.created", json!(state.checkpoint_scope)),
        ("run.created", json!(state.run)),
        ("run-attempt.created", json!(state.attempt)),
        ("node.updated", json!(state.root_node)),
        ("message.updated", message),
    ] {
        h.write(kind, value).await?;
    }
    Ok(state)
}

pub fn sessions(adapter: Arc<FakeProviderAdapter>) -> Arc<Sessions> {
    Arc::new(Sessions {
        adapter,
        failure: None,
        opening: None,
        resume_open: None,
    })
}

pub(crate) struct ProjectComposer;
impl t3_orchestration::ports::runs_ingestion::provider_turn_start_service::ComposerContextProjector
    for ProjectComposer
{
    fn project(&self, message: &OrchestrationV2ConversationMessage) -> Result<String> {
        assert!(message.text.contains("t3-context://"));
        Ok("Read the resolved context\n\n<context>exact source data</context>".into())
    }
}

#[tokio::test]
async fn composer_projection_reaches_the_provider_without_rewriting_the_saved_message() -> Result<()>
{
    let h = Harness::new().await?;
    seed(&h).await?;
    let mut message = h
        .reader
        .run_message(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await?
        .unwrap();
    let original = "Read [source](t3-context://v1/file/file_context)";
    message.text = original.into();
    h.write("message.updated", t3_contracts::stored::to_value(&message)?)
        .await?;
    let (adapter, mut calls) = h.fake();
    let adapter = Arc::new(adapter);
    let start = service(&h, sessions(adapter.clone()));
    let handle = start
        .start_configured_with_projector(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy().with_reasoning_effort(None),
            false,
            CancellationToken::new(),
            Some(Arc::new(ProjectComposer)),
        )
        .await?
        .unwrap();
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Ensure(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
    let FakeCall::Start(turn) = call(&mut calls).await else {
        panic!("expected start")
    };
    assert_eq!(
        turn.message.text,
        "Read the resolved context\n\n<context>exact source data</context>"
    );
    assert_eq!(
        h.reader
            .run_message(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await?
            .unwrap()
            .text,
        original
    );
    finish(&adapter, &h, handle).await?;
    start.execution.shutdown().await
}

pub fn policy() -> RuntimePolicy {
    resolve_base(&decode(fixture("OrchestrationV2AppThread")))
}

pub async fn call(calls: &mut mpsc::UnboundedReceiver<FakeCall>) -> FakeCall {
    calls.recv().await.expect("provider call feed closed")
}

pub async fn finish(
    adapter: &FakeProviderAdapter,
    h: &Harness,
    handle: t3_orchestration::ports::runs_ingestion::run_execution_service::RunExecutionHandle,
) -> Result<()> {
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["id"] = "turn:test".into();
    turn["providerThreadId"] = "pt:test".into();
    turn["nodeId"] = "node:test".into();
    turn["runAttemptId"] = "attempt:test".into();
    turn["status"] = "completed".into();
    turn["completedAt"] = json!(h.clock.now());
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::Turn {
                provider_turn: Box::new(decode(turn)),
                thread_id: None,
            },
        })
        .await?;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("pt:test".into()),
                provider_turn_id: ProviderTurnId("turn:test".into()),
                run_ordinal: 1,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Completed { reason: None },
            },
        })
        .await?;
    handle.settled().await
}

async fn seed_fork(h: &Harness) -> Result<RootRunState> {
    let target = seed(h).await?;
    let mut source_thread = fixture("OrchestrationV2AppThread");
    source_thread["id"] = "thread:source".into();
    source_thread["threadId"] = "thread:source".into();
    h.write("thread.created", source_thread).await?;
    let source = root_state();
    let mut provider = t3_contracts::stored::to_value(&source.provider_thread)?;
    provider["id"] = "pt:source".into();
    provider["appThreadId"] = "thread:source".into();
    provider["threadId"] = "thread:source".into();
    provider["nativeThreadRef"] =
        json!({"driver":"codex","nativeId":"native:source","strength":"strong"});
    h.write("provider-thread.updated", provider).await?;
    for (name, ordinal, status) in [
        ("source", 1, "completed"),
        ("rolled-back", 2, "rolled_back"),
    ] {
        let mut run = t3_contracts::stored::to_value(&source.run)?;
        run["id"] = format!("run:{name}").into();
        run["threadId"] = "thread:source".into();
        run["ordinal"] = ordinal.into();
        run["status"] = status.into();
        run["providerThreadId"] = "pt:source".into();
        run["activeAttemptId"] = format!("attempt:{name}").into();
        run["completedAt"] = json!(h.clock.now());
        h.write("run.created", run).await?;
        let mut attempt = t3_contracts::stored::to_value(&source.attempt)?;
        attempt["id"] = format!("attempt:{name}").into();
        attempt["threadId"] = "thread:source".into();
        attempt["runId"] = format!("run:{name}").into();
        attempt["providerThreadId"] = "pt:source".into();
        attempt["providerTurnId"] = format!("turn:{name}:old").into();
        attempt["status"] = "completed".into();
        h.write("run-attempt.created", attempt).await?;
        for (suffix, turn_ordinal) in [("old", ordinal * 2 - 1), ("latest", ordinal * 2)] {
            let mut turn = fixture("OrchestrationV2ProviderTurn");
            turn["id"] = format!("turn:{name}:{suffix}").into();
            turn["threadId"] = "thread:source".into();
            turn["providerThreadId"] = "pt:source".into();
            turn["runAttemptId"] = format!("attempt:{name}").into();
            turn["ordinal"] = turn_ordinal.into();
            turn["status"] = "completed".into();
            h.write("provider-turn.updated", turn).await?;
        }
    }
    let mut transfer = fixture("OrchestrationV2ContextTransfer");
    transfer["id"] = "transfer:fork".into();
    transfer["type"] = "fork".into();
    transfer["sourceThreadId"] = "thread:source".into();
    transfer["sourcePoint"] = json!({"threadId":"thread:source","runId":"run:source"});
    transfer["targetThreadId"] = "thread:test".into();
    transfer["targetRunId"] = "run:test".into();
    transfer["status"] = "pending".into();
    transfer["resolution"] = serde_json::Value::Null;
    h.write("context-transfer.created", transfer).await?;
    Ok(target)
}

#[tokio::test]
async fn native_fork_selects_the_source_attempt_excludes_rolled_back_turns_and_consumes_the_transfer()
-> Result<()> {
    let h = Harness::new().await?;
    let target = seed_fork(&h).await?;
    let (base, _base_calls) = h.fake();
    let mut forked = target.provider_thread.clone();
    forked.native_thread_ref = Some(decode(
        json!({"driver":"codex","nativeId":"native:forked","strength":"strong"}),
    ));
    let (adapter, mut calls) = FakeProviderAdapter::new(
        base.instance_id().clone(),
        base.driver(),
        base.runtime.session().capabilities.clone(),
        base.runtime.session().clone(),
        ThreadSnapshot {
            provider_thread: forked,
            provider_turns: vec![],
            messages: vec![],
            reasoning_message_ids: None,
            runtime_requests: vec![],
            provider_payload: None,
        },
    );
    let adapter = Arc::new(adapter);
    let start = service(&h, sessions(adapter.clone()));
    let handle = start
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    let FakeCall::Fork(input) = call(&mut calls).await else {
        panic!("expected native fork")
    };
    assert_eq!(
        input.source_provider_thread.id,
        IdOrIndex::Id(ProviderThreadId("pt:source".into()))
    );
    assert_eq!(
        input.provider_turn_id,
        Some(ProviderTurnId("turn:source:latest".into()))
    );
    assert_eq!(
        input
            .source_provider_turns
            .unwrap()
            .iter()
            .map(|turn| turn.id.clone())
            .collect::<Vec<_>>(),
        vec![
            IdOrIndex::Id(ProviderTurnId("turn:source:old".into())),
            IdOrIndex::Id(ProviderTurnId("turn:source:latest".into()))
        ]
    );
    assert_eq!(input.target_thread_id.0, "thread:test");
    assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
    assert!(matches!(call(&mut calls).await, FakeCall::Start(_)));
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    let transfer = &projection.context_transfers[0];
    assert_eq!(transfer.status, ContextTransferStatus::Consumed);
    assert_eq!(
        t3_contracts::stored::to_value(transfer)?["resolution"],
        json!({"strategy":"native_fork","providerThreadRef":{"driver":"codex","nativeId":"native:forked","strength":"strong"}})
    );
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Running);
    finish(&adapter, &h, handle).await?;
    start.execution.shutdown().await
}

#[tokio::test]
async fn completed_runs_are_read_from_the_turn_start_snapshot_without_using_legacy_control_reads()
-> Result<()> {
    let h = Harness::new().await?;
    seed(&h).await?;
    let mut context = h
        .reader
        .turn_start_context(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await?;
    context.projection.runs[0].status = OrchestrationV2RunStatus::Completed;
    let reader = Arc::new(FakeProjectionReader::default());
    reader.turn_start_context.push(Ok(context));
    let (adapter, mut calls) = h.fake();
    let mut service = service(&h, sessions(Arc::new(adapter)));
    service.projections = reader.clone();
    assert!(
        service
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                false,
                CancellationToken::new()
            )
            .await?
            .is_none()
    );
    assert_eq!(
        reader.turn_start_context.next_call().await.unwrap(),
        (ThreadId("thread:test".into()), RunId("run:test".into()))
    );
    assert!(calls.try_recv().is_err());
    service.execution.shutdown().await
}

#[tokio::test]
async fn busy_and_exhausted_native_forks_deliver_portable_history_but_retryable_failures_do_not()
-> Result<()> {
    for mode in ["busy", "exhausted", "retry"] {
        let h = Harness::new().await?;
        let mut target = seed_fork(&h).await?;
        if mode == "busy" {
            let mut active = t3_contracts::stored::to_value(&target.run)?;
            active["id"] = "run:source:active".into();
            active["threadId"] = "thread:source".into();
            active["ordinal"] = 3.into();
            active["status"] = "running".into();
            h.write("run.created", active).await?;
        }
        let mut user = super::provider_event_ingestor_test::item("user_message", "item:source");
        user["threadId"] = "thread:source".into();
        user["runId"] = "run:source".into();
        user["providerThreadId"] = "pt:source".into();
        user["messageId"] = "message:source".into();
        user["text"] = "Retain this source request exactly".into();
        user["attachments"] = json!([]);
        user["createdBy"] = "user".into();
        user["creationSource"] = "server".into();
        user["inputIntent"] = "turn_start".into();
        h.write("turn-item.updated", user).await?;
        let (adapter, _calls) = h.fake();
        let adapter = Arc::new(adapter);
        adapter
            .open_session_with_policy
            .push(Ok(adapter.runtime.clone()));
        adapter
            .runtime
            .fork_with_policy
            .push(Err(Error::Provider(ProviderError::Operation {
                operation: ProviderOperation::Fork,
                detail: "Source session no longer exists".into(),
            })));
        target.provider_thread.native_thread_ref = Some(decode(
            json!({"driver":"codex","nativeId":"native:portable","strength":"strong"}),
        ));
        adapter
            .runtime
            .ensure_thread_with_policy
            .push(Ok(target.provider_thread.clone()));
        adapter.runtime.start_turn_with_policy.push(Ok(()));
        let start = service(
            &h,
            Arc::new(super::selection_restart_integration_test::PolicySessions(
                adapter.clone(),
            )),
        );
        let result = start
            .start_configured(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy().with_reasoning_effort(Some("high".into())),
                mode == "retry",
                CancellationToken::new(),
            )
            .await;
        if mode == "retry" {
            assert!(matches!(
                result,
                Err(Error::Provider(ProviderError::Operation {
                    operation: ProviderOperation::Fork,
                    ..
                }))
            ));
            let projection = h
                .reader
                .projection(ThreadId("thread:test".into()))
                .await?
                .unwrap();
            assert!(projection.context_handoffs.is_empty());
            assert_eq!(
                projection.context_transfers[0].status,
                ContextTransferStatus::Pending
            );
            assert_eq!(
                projection.runs[0].status,
                OrchestrationV2RunStatus::Starting
            );
        } else {
            let handle = result?.unwrap();
            let (ensure, options) = adapter
                .runtime
                .ensure_thread_with_policy
                .next_call()
                .await
                .unwrap();
            assert_eq!(ensure.force_new_native_thread, mode == "exhausted");
            assert!(
                ensure
                    .existing_provider_thread
                    .unwrap()
                    .forked_from
                    .is_none()
            );
            assert_eq!(options.reasoning_effort.as_deref(), Some("high"));
            let (turn, options) = adapter
                .runtime
                .start_turn_with_policy
                .next_call()
                .await
                .unwrap();
            assert!(
                turn.message
                    .text
                    .contains("Retain this source request exactly")
            );
            assert_eq!(options.reasoning_effort.as_deref(), Some("high"));
            let projection = h
                .reader
                .projection(ThreadId("thread:test".into()))
                .await?
                .unwrap();
            assert_eq!(
                projection.context_transfers[0].status,
                ContextTransferStatus::ResolvedPortable
            );
            assert_eq!(projection.context_handoffs.len(), 1);
            assert_eq!(
                projection.context_handoffs[0]
                    .delivery
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .status,
                ContextHandoffDeliveryStatus::Inline
            );
            assert!(projection.provider_threads[0].forked_from.is_none());
            assert!(
                projection.provider_threads[0]
                    .handoff_ids
                    .contains(&projection.context_handoffs[0].id)
            );
            let notices = projection
                .turn_items
                .iter()
                .filter(|item| matches!(item, OrchestrationV2TurnItem::SystemNotice(_)))
                .count();
            assert_eq!(notices, usize::from(mode == "exhausted"));
            finish(&adapter, &h, handle).await?;
        }
        start.execution.shutdown().await?;
    }
    Ok(())
}

async fn ready_handoff(
    h: &Harness,
) -> Result<(Arc<FakeProviderAdapter>, mpsc::UnboundedReceiver<FakeCall>)> {
    let target = seed(h).await?;
    let mut handoff = fixture("OrchestrationV2ContextHandoff");
    handoff["id"] = "handoff:ready".into();
    handoff["threadId"] = "thread:test".into();
    handoff["targetRunId"] = "run:test".into();
    handoff["toProviderThreadId"] = "pt:test".into();
    handoff["status"] = "ready".into();
    handoff["strategy"] = "full_thread_summary".into();
    handoff["transferId"] = serde_json::Value::Null;
    handoff.as_object_mut().unwrap().remove("delivery");
    handoff["history"] = json!({"coverage":"Previous accepted work", "messages":[{"itemId":"item:previous","threadId":"thread:test","runId":"run:previous","providerThreadId":"pt:previous","role":"user","text":"Keep the 日本語 result 🧪", "status":"completed","kind":"user_message"}],"omittedItems":0});
    h.write("context-handoff.updated", handoff).await?;
    let (base, _base_calls) = h.fake();
    let mut provider_session = base.runtime.session().clone();
    provider_session.driver = base.driver();
    let mut loaded = target.provider_thread;
    loaded.native_thread_ref = Some(decode(
        json!({"driver":"codex","nativeId":"native:ready","strength":"strong"}),
    ));
    let (adapter, calls) = FakeProviderAdapter::new(
        base.instance_id().clone(),
        base.driver(),
        base.runtime.session().capabilities.clone(),
        provider_session,
        ThreadSnapshot {
            provider_thread: loaded,
            provider_turns: vec![],
            messages: vec![],
            reasoning_message_ids: None,
            runtime_requests: vec![],
            provider_payload: None,
        },
    );
    let adapter = Arc::new(adapter);
    Ok((adapter, calls))
}

#[tokio::test]
async fn ready_handoff_is_delivered_with_the_prompt_and_recorded_after_acceptance() -> Result<()> {
    let h = Harness::new().await?;
    let (adapter, mut calls) = ready_handoff(&h).await?;
    let user_text = "Current 🚀 request";
    let mut message = h
        .reader
        .run_message(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await?
        .unwrap();
    message.text = user_text.into();
    h.write("message.updated", t3_contracts::stored::to_value(&message)?)
        .await?;
    let start = service(&h, sessions(adapter.clone()));
    let handle = start
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Ensure(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
    let FakeCall::Start(turn) = call(&mut calls).await else {
        panic!("expected prompted start")
    };
    assert!(turn.message.text.contains("Keep the 日本語 result"));
    assert!(
        turn.message
            .text
            .ends_with(&format!("User message:\n{user_text}"))
    );
    let text_units: Vec<_> = turn.message.text.encode_utf16().collect();
    let prefix_units = text_units.len() - user_text.encode_utf16().count();
    assert_eq!(turn.inline_context_length, Some(prefix_units));
    assert_eq!(
        String::from_utf16(&text_units[prefix_units..]).unwrap(),
        user_text
    );
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    let delivery = projection.context_handoffs[0]
        .delivery
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap();
    assert_eq!(delivery.status, ContextHandoffDeliveryStatus::Inline);
    assert_eq!(delivery.item_ids, vec![TurnItemId("item:previous".into())]);
    finish(&adapter, &h, handle).await?;
    start.execution.shutdown().await
}

#[tokio::test]
async fn native_goal_defers_history_until_the_next_prompt_even_after_the_goal_run_completes()
-> Result<()> {
    let h = Harness::new().await?;
    let (adapter, mut calls) = ready_handoff(&h).await?;
    let mut message = h
        .reader
        .run_message(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await?
        .unwrap();
    message.text = "/goal keep the migration small".into();
    h.write("message.updated", t3_contracts::stored::to_value(&message)?)
        .await?;
    let start = service(&h, sessions(adapter.clone()));
    let handle = start
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Ensure(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
    let FakeCall::Start(goal) = call(&mut calls).await else {
        panic!("expected goal start")
    };
    assert_eq!(goal.message.text, message.text);
    assert_eq!(goal.inline_context_length, None);
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert!(
        projection.context_handoffs[0]
            .delivery
            .as_ref()
            .and_then(Option::as_ref)
            .is_none()
    );
    finish(&adapter, &h, handle).await?;
    let mut next = root_state();
    let outside_repository=Arc::new(t3_orchestration::ports::runs_ingestion::provider_turn_start_service_testkit::NoRepositoryCheckpoints);
    t3_orchestration::ports::checkpoints_forks::checkpoint_capture_service::CheckpointCaptureService {
        checkpoints:Arc::new(t3_orchestration::ports::checkpoints_forks::checkpoint_service::CheckpointService::new(
            outside_repository.clone(),outside_repository,Arc::new(t3_orchestration::ports::checkpoints_forks::checkpoint_service::WorkspaceLocks::default()))),
        projections:h.reader.clone(),events:h.sink.clone(),ids:Arc::new(SequenceIds::new("goal-checkpoint")),clock:h.clock.clone(),
    }.execute(ThreadId("thread:test".into()),RunId("run:test".into()),CheckpointScopeId("scope:test".into()),CancellationToken::new()).await?;
    next.run.id = IdOrIndex::Id(RunId("run:next".into()));
    next.run.ordinal = 2;
    next.run.status = OrchestrationV2RunStatus::Starting;
    next.run.active_attempt_id = Some(IdOrIndex::Id(RunAttemptId("attempt:next".into())));
    next.run.root_node_id = Some(IdOrIndex::Id(NodeId("node:next".into())));
    next.run.user_message_id = IdOrIndex::Id(MessageId("message:next".into()));
    next.run.started_at = None;
    next.run.completed_at = None;
    next.run.model_selection = goal.model_selection;
    next.attempt.id = IdOrIndex::Id(RunAttemptId("attempt:next".into()));
    next.attempt.run_id = next.run.id.clone();
    next.attempt.root_node_id = IdOrIndex::Id(NodeId("node:next".into()));
    next.attempt.status = RunAttemptStatus::Pending;
    next.attempt.provider_turn_id = None;
    next.attempt.started_at = None;
    next.attempt.completed_at = None;
    next.root_node.id = IdOrIndex::Id(NodeId("node:next".into()));
    next.root_node.run_id = Some(next.run.id.clone());
    next.root_node.status = ExecutionNodeStatus::Pending;
    next.root_node.provider_turn_id = None;
    next.root_node.checkpoint_scope_id = Some(next.checkpoint_scope.id.clone());
    message.id = next.run.user_message_id.clone();
    message.run_id = Some(next.run.id.clone());
    message.text = "Apply it now".into();
    for (kind, value) in [
        ("run.created", t3_contracts::stored::to_value(&next.run)?),
        (
            "run-attempt.created",
            t3_contracts::stored::to_value(&next.attempt)?,
        ),
        (
            "node.updated",
            t3_contracts::stored::to_value(&next.root_node)?,
        ),
        ("message.updated", t3_contracts::stored::to_value(&message)?),
    ] {
        h.write(kind, value).await?;
    }
    let before = h
        .reader
        .turn_start_context(ThreadId("thread:test".into()), RunId("run:next".into()))
        .await?;
    assert!(
        before
            .projection
            .messages
            .iter()
            .any(|message| message.text.starts_with("/goal ")),
        "turn-start seam must include deferred control messages"
    );
    let goal_run = before
        .projection
        .runs
        .iter()
        .find(|run| run.id == IdOrIndex::Id(RunId("run:test".into())))
        .unwrap();
    assert_eq!(goal_run.status, OrchestrationV2RunStatus::Completed);
    assert!(before.projection.messages.iter().any(|message| message.id
        == goal_run.user_message_id
        && message.text.starts_with("/goal ")));
    assert!(
        before.projection.context_handoffs[0]
            .delivery
            .as_ref()
            .and_then(Option::as_ref)
            .is_none()
    );
    let handle = start
        .start(
            ThreadId("thread:test".into()),
            RunId("run:next".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Resume(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
    let FakeCall::Start(prompt) = call(&mut calls).await else {
        panic!("expected follow-up prompt")
    };
    assert!(prompt.message.text.contains("Keep the 日本語 result"));
    assert!(prompt.message.text.ends_with("User message:\nApply it now"));
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(
        projection.context_handoffs[0]
            .delivery
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .status,
        ContextHandoffDeliveryStatus::Inline
    );
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["id"] = "turn:next".into();
    turn["providerThreadId"] = "pt:test".into();
    turn["nodeId"] = "node:next".into();
    turn["runAttemptId"] = "attempt:next".into();
    turn["ordinal"] = json!(prompt.provider_turn_ordinal);
    turn["status"] = "completed".into();
    turn["completedAt"] = json!(h.clock.now());
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::Turn {
                provider_turn: Box::new(decode(turn)),
                thread_id: None,
            },
        })
        .await?;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("pt:test".into()),
                provider_turn_id: ProviderTurnId("turn:next".into()),
                run_ordinal: 2,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Completed { reason: None },
            },
        })
        .await?;
    handle.settled().await?;
    start.execution.shutdown().await
}

#[tokio::test]
async fn resume_recovers_lost_or_uncertain_native_context_and_budget_exhaustion_without_splitting_a_live_writer()
-> Result<()> {
    for mode in [
        "missed",
        "failed",
        "uncertain",
        "budget",
        "in-use",
        "restart-note",
        "note-continuation",
    ] {
        let h = Harness::new().await?;
        let mut target = seed(&h).await?;
        target.run.ordinal = 2;
        if mode == "note-continuation" {
            target.run.restart_continuation_of_run_id =
                Some(Some(IdOrIndex::Id(RunId("run:previous".into()))));
        }
        target.provider_thread.native_thread_ref = Some(decode(
            json!({"driver":"codex","nativeId":"native:old","strength":"strong"}),
        ));
        target.provider_thread.context_usage = None;
        h.write("run.updated", t3_contracts::stored::to_value(&target.run)?)
            .await?;
        h.write(
            "provider-thread.updated",
            t3_contracts::stored::to_value(&target.provider_thread)?,
        )
        .await?;
        let mut previous = target.run.clone();
        previous.id = IdOrIndex::Id(RunId("run:previous".into()));
        previous.ordinal = 1;
        previous.status = OrchestrationV2RunStatus::Failed;
        previous.restart_continuation_of_run_id = None;
        if matches!(mode, "restart-note" | "note-continuation") {
            previous.status = OrchestrationV2RunStatus::Completed;
            previous.restart_cancelled_background_work = Some(Some(vec![decode(
                json!({"kind":"task","id":"task:lost","label":"lost analysis"}),
            )]));
        }
        previous.active_attempt_id = Some(IdOrIndex::Id(RunAttemptId("attempt:previous".into())));
        previous.user_message_id = IdOrIndex::Id(MessageId("message:previous".into()));
        previous.started_at = Some(h.clock.now());
        previous.completed_at = Some(h.clock.now());
        h.write("run.created", t3_contracts::stored::to_value(&previous)?)
            .await?;
        let mut user = super::provider_event_ingestor_test::item("user_message", "item:previous");
        user["runId"] = "run:previous".into();
        user["providerThreadId"] = "pt:test".into();
        user["messageId"] = "message:previous".into();
        user["createdBy"] = "user".into();
        user["creationSource"] = "server".into();
        user["inputIntent"] = "turn_start".into();
        user["attachments"] = json!([]);
        user["text"] = "Original constraint must survive recovery".into();
        h.write("turn-item.updated", user).await?;
        if matches!(mode, "uncertain" | "budget") {
            let mut handoff = fixture("OrchestrationV2ContextHandoff");
            handoff["id"] = "handoff:pending".into();
            handoff["threadId"] = "thread:test".into();
            handoff["targetRunId"] = "run:test".into();
            handoff["toProviderThreadId"] = "pt:test".into();
            handoff["status"] = "ready".into();
            handoff["transferId"] = serde_json::Value::Null;
            handoff["history"] = json!({"coverage":"Prior work","messages":[],"omittedItems":0});
            if mode == "uncertain" {
                handoff["delivery"] =
                    json!({"nativeThreadId":"native:old","status":"pending","itemIds":[]});
            } else {
                handoff.as_object_mut().unwrap().remove("delivery");
            }
            h.write("context-handoff.updated", handoff).await?;
        }
        let (adapter, _calls) = h.fake();
        let adapter = Arc::new(adapter);
        adapter
            .open_session_with_policy
            .push(Ok(adapter.runtime.clone()));
        if mode == "failed" {
            adapter
                .runtime
                .resume_thread_with_policy
                .push(Err(Error::Provider(ProviderError::Operation {
                    operation: ProviderOperation::Resume,
                    detail: "Native history was removed".into(),
                })));
        } else if mode == "in-use" {
            adapter
                .runtime
                .resume_thread_with_policy
                .push(Err(Error::Provider(ProviderError::ThreadInUse(
                    ProviderThreadId("pt:test".into()),
                ))));
        } else {
            let mut resumed = target.provider_thread.clone();
            if mode == "budget" {
                resumed.context_usage = Some(Some(decode(
                    json!({"usedTokens":31_000,"maxTokens":32_000}),
                )));
            }
            adapter.runtime.resume_thread_with_policy.push(Ok(resumed));
        }
        let mut replacement = target.provider_thread.clone();
        replacement.native_thread_ref = Some(decode(
            json!({"driver":"codex","nativeId":"native:replacement","strength":"strong"}),
        ));
        adapter
            .runtime
            .ensure_thread_with_policy
            .push(Ok(replacement));
        adapter.runtime.start_turn_with_policy.push(Ok(()));
        let start = service(
            &h,
            Arc::new(super::selection_restart_integration_test::PolicySessions(
                adapter.clone(),
            )),
        );
        let result = start
            .start_configured(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy().with_reasoning_effort(Some("high".into())),
                true,
                CancellationToken::new(),
            )
            .await;
        if mode == "in-use" {
            assert!(matches!(
                result,
                Err(Error::Provider(ProviderError::ThreadInUse(_)))
            ));
            let projection = h
                .reader
                .projection(ThreadId("thread:test".into()))
                .await?
                .unwrap();
            assert!(projection.context_transfers.is_empty());
            assert_eq!(
                projection
                    .runs
                    .iter()
                    .find(|run| run.id == target.run.id)
                    .unwrap()
                    .status,
                OrchestrationV2RunStatus::Starting
            );
        } else {
            let handle = result?.unwrap();
            let (turn, _) = adapter
                .runtime
                .start_turn_with_policy
                .next_call()
                .await
                .unwrap();
            if mode == "restart-note" {
                assert!(turn.message.text.contains("lost analysis"));
                assert!(turn.message.text.ends_with("User message:\nHello provider"));
            } else if mode == "note-continuation" {
                assert!(turn.restart_continuation_of_run_id.is_none());
            } else {
                assert!(
                    turn.message
                        .text
                        .contains("Original constraint must survive recovery"),
                    "mode {mode}"
                );
            }
            let projection = h
                .reader
                .projection(ThreadId("thread:test".into()))
                .await?
                .unwrap();
            if matches!(mode, "missed" | "restart-note" | "note-continuation") {
                assert!(projection.context_transfers.is_empty());
                assert_eq!(
                    turn.provider_thread
                        .native_thread_ref
                        .as_ref()
                        .unwrap()
                        .native_id
                        .as_ref()
                        .unwrap()
                        .0,
                    "native:old"
                );
            } else {
                let (ensure, _) = adapter
                    .runtime
                    .ensure_thread_with_policy
                    .next_call()
                    .await
                    .unwrap();
                assert!(ensure.force_new_native_thread);
                assert!(
                    ensure
                        .existing_provider_thread
                        .unwrap()
                        .native_thread_ref
                        .is_none()
                );
                assert_eq!(projection.context_transfers.len(), 1);
                assert_eq!(
                    projection.context_transfers[0].status,
                    ContextTransferStatus::ResolvedPortable
                );
                let recovery_notice = projection
                    .turn_items
                    .iter()
                    .find_map(|item| match item {
                        OrchestrationV2TurnItem::Handoff(item)
                            if item.title.as_deref() == Some("Recovered context") =>
                        {
                            Some(item)
                        }
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(
                    recovery_notice.ordinal,
                    target.run.ordinal * 1_000_000,
                    "recovery history must precede the current run's own message: {mode}"
                );
                assert_eq!(
                    turn.provider_thread
                        .native_thread_ref
                        .as_ref()
                        .unwrap()
                        .native_id
                        .as_ref()
                        .unwrap()
                        .0,
                    "native:replacement"
                );
                assert!(
                    projection.provider_threads[0]
                        .context_usage
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_none()
                );
            }
            finish(&adapter, &h, handle).await?;
        }
        start.execution.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn compacting_an_empty_conversation_fails_before_opening_a_provider() -> Result<()> {
    let h = Harness::new().await?;
    seed(&h).await?;
    let mut message = h
        .reader
        .run_message(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await?
        .unwrap();
    message.text = " /COMPACT ".into();
    h.write("message.updated", t3_contracts::stored::to_value(&message)?)
        .await?;
    assert!(
        !h.reader
            .turn_start_context(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await?
            .has_conversation
    );
    let (adapter, mut calls) = h.fake();
    let service = service(&h, sessions(Arc::new(adapter)));
    assert!(
        service
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                false,
                CancellationToken::new()
            )
            .await?
            .is_none()
    );
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(projection.runs[0].status, OrchestrationV2RunStatus::Failed);
    assert_eq!(projection.attempts[0].status, RunAttemptStatus::Failed);
    assert_eq!(
        projection.provider_threads[0].status,
        ProviderThreadStatus::NotLoaded
    );
    let failure = projection
        .turn_items
        .iter()
        .find_map(|item| match item {
            OrchestrationV2TurnItem::Error(item) => Some(item),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        failure.failure.class,
        OrchestrationV2ProviderFailureClass::ValidationError
    );
    assert_eq!(
        failure.failure.message.0,
        "Start a conversation before compacting this thread."
    );
    assert!(calls.try_recv().is_err());
    service.execution.shutdown().await
}

#[tokio::test]
async fn starts_adopting_the_persisted_provider_identity_and_awaits_acceptance() -> Result<()> {
    let h = Harness::new().await?;
    let state = seed(&h).await?;
    let (adapter, mut calls) = h.fake();
    let adapter = Arc::new(adapter);
    let mut service = service(&h, sessions(adapter.clone()));
    let baseline = Arc::new(FakeCheckpointBackend::default());
    baseline.capture.push(Err(Error::Service {
        operation: "baseline",
        detail: "VCS process timed out".into(),
    }));
    Arc::get_mut(&mut service.execution).unwrap().checkpoints = Some(baseline.clone());
    let telemetry = Arc::new(FakeProviderTelemetry::default());
    telemetry.turn_completed.push(Ok(()));
    Arc::get_mut(&mut Arc::get_mut(&mut service.execution).unwrap().ingestor)
        .unwrap()
        .telemetry = Some(telemetry.clone());
    let handle = service
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    let FakeCall::Ensure(input) = call(&mut calls).await else {
        panic!("expected ensure");
    };
    assert_eq!(
        input.existing_provider_thread.unwrap().id,
        state.provider_thread.id
    );
    assert!(!input.force_new_native_thread);
    assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
    let FakeCall::Start(input) = call(&mut calls).await else {
        panic!("expected start");
    };
    assert_eq!(input.message.text, "Hello provider");
    assert_eq!(input.inline_context_length, None);
    assert_eq!(input.model_selection.model, "model:test");
    assert_eq!(input.run_ordinal, 1);
    assert_eq!(input.provider_turn_ordinal, 1);
    assert_eq!(input.provider_thread.id, state.provider_thread.id);
    let (capture, _) = baseline.capture.next_call().await.unwrap();
    assert_eq!(capture.ordinal, 0);
    let run = h
        .reader
        .run(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await?
        .unwrap();
    assert_eq!(run.status, OrchestrationV2RunStatus::Running);
    assert_eq!(run.started_at, Some(h.clock.now()));
    finish(&adapter, &h, handle).await?;
    let (completed, _) = telemetry.turn_completed.next_call().await.unwrap();
    let analytics = completed.context.unwrap();
    assert_eq!(analytics.run_id, Some(RunId("run:test".into())));
    assert_eq!(analytics.model_selection.model, "model:test");
    assert!(matches!(
        analytics.segment,
        Some(ProviderTurnSegment::Attempt(RunAttemptReason::Initial))
    ));
    let origin = analytics.origin.unwrap();
    assert_eq!(origin.created_by, OrchestrationV2Actor::User);
    assert_eq!(origin.creation_source, OrchestrationV2CreationSource::Web);
    let run = h
        .reader
        .run(ThreadId("thread:test".into()), RunId("run:test".into()))
        .await?
        .unwrap();
    assert_eq!(run.status, OrchestrationV2RunStatus::Waiting);
    assert_eq!(run.completed_at, None);
    service.execution.shutdown().await
}

#[tokio::test]
async fn open_failure_retries_without_settlement_but_exhaustion_and_sign_in_fail_durably()
-> Result<()> {
    for (failure, retry, failed) in [
        ("offline", true, false),
        ("offline", false, true),
        ("sign-in", true, true),
    ] {
        let h = Harness::new().await?;
        seed(&h).await?;
        let (adapter, mut calls) = h.fake();
        let service = service(
            &h,
            Arc::new(Sessions {
                adapter: Arc::new(adapter),
                failure: Some(failure),
                opening: None,
                resume_open: None,
            }),
        );
        let result = service
            .start(
                ThreadId("thread:test".into()),
                RunId("run:test".into()),
                policy(),
                retry,
                CancellationToken::new(),
            )
            .await;
        assert_eq!(
            result.is_ok(),
            failed,
            "{failure}/{retry}: {:?}",
            result.as_ref().err()
        );
        let projection = h
            .reader
            .projection(ThreadId("thread:test".into()))
            .await?
            .unwrap();
        assert_eq!(
            projection.runs[0].status,
            if failed {
                OrchestrationV2RunStatus::Failed
            } else {
                OrchestrationV2RunStatus::Starting
            }
        );
        assert_eq!(
            projection.attempts[0].status,
            if failed {
                RunAttemptStatus::Failed
            } else {
                RunAttemptStatus::Pending
            }
        );
        assert_eq!(
            projection
                .turn_items
                .iter()
                .filter(|item| matches!(item, OrchestrationV2TurnItem::Error(_)))
                .count(),
            usize::from(failed)
        );
        assert!(calls.try_recv().is_err());
        service.execution.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn stop_committed_during_open_prevents_provider_start_and_preserves_the_stop() -> Result<()> {
    let h = Harness::new().await?;
    let mut state = seed(&h).await?;
    let (adapter, mut calls) = h.fake();
    let (opening, mut opened) = mpsc::unbounded_channel();
    let resume = Arc::new(Notify::new());
    let service = service(
        &h,
        Arc::new(Sessions {
            adapter: Arc::new(adapter),
            failure: None,
            opening: Some(opening),
            resume_open: Some(resume.clone()),
        }),
    );
    let start = service.start(
        ThreadId("thread:test".into()),
        RunId("run:test".into()),
        policy(),
        false,
        CancellationToken::new(),
    );
    tokio::pin!(start);
    tokio::select! { result = &mut start => panic!("start completed before opening barrier: {}",result.is_ok()), _ = opened.recv() => {} }
    state.run.status = OrchestrationV2RunStatus::Interrupted;
    state.run.completed_at = Some(h.clock.now());
    h.write("run.updated", json!(state.run)).await?;
    resume.notify_one();
    assert!(start.await?.is_none());
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    assert!(calls.try_recv().is_err());
    assert_eq!(
        h.reader
            .run(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await?
            .unwrap()
            .status,
        OrchestrationV2RunStatus::Interrupted
    );
    service.execution.shutdown().await
}

#[tokio::test]
async fn cancellation_during_open_leaves_starting_state_for_recovery() -> Result<()> {
    let h = Harness::new().await?;
    seed(&h).await?;
    let (adapter, mut calls) = h.fake();
    let (opening, mut opened) = mpsc::unbounded_channel();
    let service = service(
        &h,
        Arc::new(Sessions {
            adapter: Arc::new(adapter),
            failure: None,
            opening: Some(opening),
            resume_open: Some(Arc::new(Notify::new())),
        }),
    );
    let cancellation = CancellationToken::new();
    let start = service.start(
        ThreadId("thread:test".into()),
        RunId("run:test".into()),
        policy(),
        false,
        cancellation.clone(),
    );
    tokio::pin!(start);
    tokio::select! { result = &mut start => panic!("unexpected result: {}",result.is_ok()), _ = opened.recv() => {} }
    cancellation.cancel();
    assert!(matches!(start.await, Err(Error::Cancelled)));
    assert_eq!(
        h.reader
            .run(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await?
            .unwrap()
            .status,
        OrchestrationV2RunStatus::Starting
    );
    assert!(calls.try_recv().is_err());
    service.execution.shutdown().await
}

#[tokio::test]
async fn recovers_an_abandoned_turn_before_opening_and_preserves_its_original_run_metadata()
-> Result<()> {
    use t3_contracts::application_event::ApplicationStoredEvent;
    let h = Harness::new().await?;
    let mut state = seed(&h).await?;
    state.run.ordinal = 2;
    h.write("run.updated", json!(state.run)).await?;
    let mut previous = json!(state.run);
    previous["id"] = "run:old".into();
    previous["ordinal"] = 1.into();
    previous["status"] = "failed".into();
    previous["rootNodeId"] = "node:old".into();
    previous["activeAttemptId"] = "attempt:old".into();
    previous["userMessageId"] = "message:old".into();
    h.write("run.created", previous).await?;
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["id"] = "turn:old".into();
    turn["providerThreadId"] = "pt:test".into();
    turn["nodeId"] = "node:old".into();
    turn["runAttemptId"] = "attempt:old".into();
    turn["status"] = "running".into();
    turn["ordinal"] = 1.into();
    h.write("provider-turn.updated", turn.clone()).await?;
    let mut events = h.sink.subscribe();
    let (adapter, mut calls) = h.fake();
    let adapter = Arc::new(adapter);
    let service = service(&h, sessions(adapter.clone()));
    let handle = service
        .start(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            policy(),
            false,
            CancellationToken::new(),
        )
        .await?
        .unwrap();
    let FakeCall::Interrupt(input) = call(&mut calls).await else {
        panic!("expected recovery interrupt")
    };
    assert_eq!(input.provider_turn_id.0, "turn:old");
    assert!(input.request_runtime_restart);
    let ApplicationStoredEvent::Orchestration(stored) = events.recv().await.unwrap() else {
        panic!("expected orchestration event")
    };
    let raw = json!(stored.event);
    assert_eq!(raw["type"], "provider-turn.updated");
    assert_eq!(raw["payload"]["status"], "interrupted");
    assert_eq!(raw["runId"], "run:old");
    assert_eq!(raw["nodeId"], "node:old");
    assert!(matches!(call(&mut calls).await, FakeCall::Open(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Ensure(_)));
    assert!(matches!(call(&mut calls).await, FakeCall::Subscribe));
    let FakeCall::Start(input) = call(&mut calls).await else {
        panic!("expected start")
    };
    assert_eq!(input.provider_turn_ordinal, 2);
    turn["id"] = "turn:test".into();
    turn["nodeId"] = "node:test".into();
    turn["runAttemptId"] = "attempt:test".into();
    turn["ordinal"] = 2.into();
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::Turn {
                provider_turn: Box::new(decode(turn)),
                thread_id: None,
            },
        })
        .await?;
    adapter
        .runtime
        .emit(ProviderEvent {
            driver: adapter.driver(),
            kind: ProviderEventKind::Terminal {
                provider_thread_id: ProviderThreadId("pt:test".into()),
                provider_turn_id: ProviderTurnId("turn:test".into()),
                run_ordinal: 2,
                disposition: ThreadDisposition::Reusable,
                outcome: TerminalOutcome::Completed { reason: None },
            },
        })
        .await?;
    handle.settled().await?;
    service.execution.shutdown().await
}
