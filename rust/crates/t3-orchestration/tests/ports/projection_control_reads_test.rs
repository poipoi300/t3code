#![allow(clippy::unwrap_used)]
use crate::projection_store_test::{apply_values, fixture};
use t3_orchestration::{api::*, projections::store::ProjectionStore, testing::TestDatabase};

#[tokio::test]
async fn provider_targets_read_the_requested_history_and_respect_thread_ownership() {
    let db = TestDatabase::memory().unwrap();
    let mut newer = fixture("provider-turn.updated", "turn:newer");
    newer["payload"]["ordinal"] = 100.into();
    newer["payload"]["runAttemptId"] = "attempt:newer".into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            fixture("run-attempt.created", "attempt:test"),
            fixture("provider-thread.updated", "provider:test"),
            fixture("provider-turn.updated", "turn:test"),
            newer,
            fixture("message.updated", "message:test"),
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let query = ProviderTargetQuery {
        thread_id: ThreadId("thread:test".into()),
        provider_thread_id: ProviderThreadId("provider:test".into()),
        provider_turn_id: ProviderTurnId("turn:test".into()),
        attempt_id: Some(RunAttemptId("attempt:test".into())),
        message_id: Some(MessageId("message:test".into())),
    };
    let result = store.provider_target(query.clone()).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(result.provider_turn.unwrap()).unwrap()["id"],
        "turn:test"
    );
    assert!(result.provider_thread.is_some());
    assert!(result.attempt.is_some());
    assert!(result.message.is_some());
    assert!(result.run.is_some());
    let mut other = fixture("thread.created", "thread:other");
    other["threadId"] = "thread:other".into();
    apply_values(&db, vec![other]).await;
    let result = store
        .provider_target(ProviderTargetQuery {
            thread_id: ThreadId("thread:other".into()),
            ..query
        })
        .await
        .unwrap()
        .unwrap();
    assert!(result.provider_thread.is_none());
    assert!(result.provider_turn.is_none());
    assert!(result.attempt.is_none());
    assert!(result.message.is_none());
    assert!(result.run.is_none());
}

#[tokio::test]
async fn runtime_response_scope_never_decodes_a_foreign_thread_or_request() {
    let db = TestDatabase::memory().unwrap();
    let mut other = fixture("thread.created", "thread:other");
    other["threadId"] = "thread:other".into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            other,
            fixture("runtime-request.updated", "request:test"),
        ],
    )
    .await;
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json='invalid' WHERE thread_id='thread:test'",[])?;tx.execute("UPDATE orchestration_v2_projection_runtime_requests SET payload_json='invalid'",[])?;Ok(())}).await.unwrap();
    let concrete = ProjectionStore::new(db.stores);
    let store: &dyn ProjectionReader = &concrete;
    let result = store
        .runtime_response_context(
            ThreadId("thread:other".into()),
            RuntimeRequestId("request:test".into()),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(result.request.is_none());
    assert!(result.node.is_none());
    assert!(result.item.is_none());
    assert!(result.provider_session.is_none());
}

#[tokio::test]
async fn control_reads_use_current_attempt_and_do_not_decode_unrelated_history() {
    let db = TestDatabase::memory().unwrap();
    let mut old_turn = fixture("provider-turn.updated", "turn:old");
    old_turn["payload"]["ordinal"] = 2.into();
    old_turn["payload"]["runAttemptId"] = "attempt:old".into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            fixture("run-attempt.created", "attempt:test"),
            fixture("node.updated", "node:test"),
            fixture("provider-session.attached", "session:test"),
            fixture("provider-thread.updated", "provider:test"),
            fixture("provider-turn.updated", "turn:test"),
            old_turn,
            fixture("message.updated", "message:test"),
            fixture("runtime-request.updated", "request:test"),
        ],
    )
    .await;
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_messages SET payload_json='{}' WHERE message_id='message:test'",[])?;Ok(())}).await.unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    assert!(
        store
            .projection(ThreadId("thread:test".into()))
            .await
            .is_err()
    );
    apply_values(
        &db,
        vec![
            fixture("message.updated", "message:test"),
            fixture("message.updated", "message:unrelated"),
        ],
    )
    .await;
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_messages SET payload_json='{}' WHERE message_id='message:unrelated'",[])?;Ok(())}).await.unwrap();
    let context = store
        .control(ControlQuery::RunningTurn(ThreadId("thread:test".into())))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(context.provider_turn.unwrap()).unwrap()["id"],
        "turn:test"
    );
    assert_eq!(
        serde_json::to_value(context.run.unwrap()).unwrap()["id"],
        "run:test"
    );
    assert!(context.provider_session.is_none());
    let response = store
        .control(ControlQuery::RuntimeResponse(RuntimeRequestId(
            "request:test".into(),
        )))
        .await
        .unwrap()
        .unwrap();
    assert!(response.request.is_some());
    assert!(response.node.is_some());
    assert!(response.provider_session.is_some());
    assert!(
        store
            .run(ThreadId("thread:other".into()), RunId("run:test".into()))
            .await
            .is_err()
    );
    assert!(
        store
            .control(ControlQuery::RuntimeResponse(RuntimeRequestId(
                "request:missing".into()
            )))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn detached_session_cannot_answer_a_live_request() {
    let db = TestDatabase::memory().unwrap();
    let mut detached = crate::projection_store_test::sample("provider-session.detached");
    detached["threadId"] = "thread:test".into();
    detached["payload"]["providerSessionId"] = "session:test".into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("provider-session.attached", "session:test"),
            fixture("runtime-request.updated", "request:test"),
            detached,
        ],
    )
    .await;
    let store = ProjectionStore::new(db.stores.clone());
    let context = store
        .control(ControlQuery::RuntimeResponse(RuntimeRequestId(
            "request:test".into(),
        )))
        .await
        .unwrap()
        .unwrap();
    assert!(context.provider_session.is_none());
    assert!(context.request.is_some());
}

#[tokio::test(start_paused = true)]
async fn exact_control_contexts_drive_steering_replies_and_confirm_interrupt_attempt_settlement() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use t3_orchestration::{
        ports::runs_ingestion::{
            provider_turn_control_service::ProviderTurnControlService,
            runtime_request_service::RuntimeRequestService,
        },
        provider::*,
        testing::{
            FakeCall, FakeEventCommitter, FakeProjectionReader, FakeProviderAdapter,
            FakeSessionDirectory,
        },
    };
    use tokio_util::sync::CancellationToken;
    let db = TestDatabase::memory().unwrap();
    let mut message = fixture("message.updated", "message:test");
    message["payload"]["text"] = "Use the smaller fix.".into();
    let mut attempt = fixture("run-attempt.created", "attempt:test");
    attempt["payload"]["status"] = "running".into();
    attempt["payload"]["providerTurnId"] = serde_json::Value::Null;
    let mut turn = fixture("provider-turn.updated", "turn:test");
    turn["payload"]["status"] = "running".into();
    let mut request = fixture("runtime-request.updated", "request:test");
    request["payload"]["status"] = "resolved".into();
    request["payload"]["resolvedAt"] = crate::projection_store_test::NOW.into();
    request["payload"]["responseCapability"] =
        serde_json::json!({"type":"live","providerSessionId":"session:test"});
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            attempt.clone(),
            fixture("provider-session.attached", "session:test"),
            fixture("provider-thread.updated", "provider:test"),
            turn.clone(),
            message,
            request,
        ],
    )
    .await;
    let store = Arc::new(ProjectionStore::new(db.stores.clone()));
    let query = ProviderTargetQuery {
        thread_id: ThreadId("thread:test".into()),
        provider_thread_id: ProviderThreadId("provider:test".into()),
        provider_turn_id: ProviderTurnId("turn:test".into()),
        attempt_id: Some(RunAttemptId("attempt:test".into())),
        message_id: Some(MessageId("message:test".into())),
    };
    let running = store.provider_target(query.clone()).await.unwrap().unwrap();
    let session: OrchestrationV2ProviderSession = serde_json::from_value(
        fixture("provider-session.attached", "session:test")["payload"].clone(),
    )
    .unwrap();
    let snapshot = ThreadSnapshot {
        provider_thread: running.provider_thread.clone().unwrap(),
        provider_turns: vec![running.provider_turn.clone().unwrap()],
        messages: vec![],
        reasoning_message_ids: None,
        runtime_requests: vec![],
        provider_payload: None,
    };
    let instance = t3_contracts::provider_instance::ProviderInstanceId(
        t3_orchestration::projections::read::id_text(&session.provider_instance_id)
            .unwrap()
            .to_owned(),
    );
    let (adapter, mut calls) = FakeProviderAdapter::new(
        instance,
        session.driver.clone(),
        session.capabilities.clone(),
        session,
        snapshot,
    );
    let sessions = Arc::new(FakeSessionDirectory::default());
    for _ in 0..8 {
        sessions.get.push(Ok(Some(adapter.runtime.clone())));
    }
    let probes = Arc::new(FakeProjectionReader::default());
    let events = Arc::new(FakeEventCommitter::new(db.stores.clone()));
    let control = ProviderTurnControlService {
        projections: probes.clone(),
        sessions: sessions.clone(),
        events: events.clone(),
    };
    let target = ProviderControlTarget {
        thread_id: query.thread_id.clone(),
        provider_thread_id: query.provider_thread_id.clone(),
        provider_turn_id: query.provider_turn_id.clone(),
        provider_session_id: ProviderSessionId("session:test".into()),
    };
    let interrupt = || InterruptTurn {
        target: target.clone(),
        subagent: None,
        requested_at: None,
    };
    for _ in 0..2 {
        probes.provider_target.push(Ok(Some(running.clone())));
    }
    let (result, ()) = tokio::join!(
        control.interrupt(interrupt(), CancellationToken::new()),
        async {
            for _ in 0..2 {
                assert_eq!(
                    probes
                        .provider_target
                        .next_call()
                        .await
                        .unwrap()
                        .0
                        .thread_id,
                    target.thread_id
                );
            }
            assert!(matches!(
                calls.recv().await.unwrap(),
                FakeCall::Interrupt(_)
            ));
            tokio::time::advance(std::time::Duration::from_secs(2)).await;
        }
    );
    result.unwrap();
    for _ in 0..2 {
        probes.provider_target.push(Ok(Some(running.clone())));
    }
    control
        .steer(
            SteerTurn {
                target: target.clone(),
                message_id: MessageId("message:test".into()),
                model_selection: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    for _ in 0..2 {
        probes.provider_target.next_call().await.unwrap();
    }
    let FakeCall::Steer(steer) = calls.recv().await.unwrap() else {
        panic!("expected steering")
    };
    assert_eq!(steer.run_id, RunId("run:test".into()));
    assert_eq!(steer.message.text, "Use the smaller fix.");
    let replies = RuntimeRequestService {
        projections: store.clone(),
        sessions: sessions.clone(),
    };
    let response = RuntimeResponse {
        request_id: RuntimeRequestId("request:test".into()),
        decision: Some(serde_json::from_value(serde_json::json!("accept")).unwrap()),
        feedback: None,
        answers: None,
    };
    replies
        .respond(
            target.thread_id.clone(),
            target.provider_session_id.clone(),
            response.clone(),
        )
        .await
        .unwrap();
    assert!(matches!(calls.recv().await.unwrap(), FakeCall::Respond(_)));
    probes.provider_target.push(Ok(Some(running.clone())));
    let mut wrong = interrupt();
    wrong.target.provider_session_id = ProviderSessionId("session:wrong".into());
    assert!(
        control
            .interrupt(wrong, CancellationToken::new())
            .await
            .is_err()
    );
    probes.provider_target.next_call().await.unwrap();
    assert!(
        replies
            .respond(
                target.thread_id.clone(),
                ProviderSessionId("session:wrong".into()),
                response
            )
            .await
            .is_err()
    );
    assert!(calls.try_recv().is_err());
    // SQLite blocking tasks must finish before paused-clock event scheduling begins.
    turn["payload"]["status"] = "completed".into();
    turn["payload"]["completedAt"] = crate::projection_store_test::NOW.into();
    apply_values(&db, vec![turn.clone()]).await;
    let turn_ended = store.provider_target(query.clone()).await.unwrap();
    assert_eq!(
        turn_ended
            .as_ref()
            .unwrap()
            .attempt
            .as_ref()
            .unwrap()
            .status,
        RunAttemptStatus::Running
    );
    attempt["payload"]["status"] = "completed".into();
    attempt["payload"]["completedAt"] = crate::projection_store_test::NOW.into();
    apply_values(&db, vec![attempt.clone()]).await;
    let all_ended = store.provider_target(query.clone()).await.unwrap();
    for _ in 0..2 {
        probes.provider_target.push(Ok(Some(running.clone())));
    }
    let finished = AtomicBool::new(false);
    let (result, ()) = tokio::join!(
        async {
            let result = control
                .interrupt(interrupt(), CancellationToken::new())
                .await;
            finished.store(true, Ordering::SeqCst);
            result
        },
        async {
            for _ in 0..2 {
                probes.provider_target.next_call().await.unwrap();
            }
            assert!(matches!(
                calls.recv().await.unwrap(),
                FakeCall::Interrupt(_)
            ));
            probes.provider_target.push(Ok(turn_ended));
            events.publish(
                t3_contracts::application_event::ApplicationStoredEvent::Orchestration(Box::new(
                    OrchestrationV2StoredEvent {
                        sequence: 1,
                        command_id: None,
                        event: serde_json::from_value(turn.clone()).unwrap(),
                    },
                )),
            );
            probes.provider_target.next_call().await.unwrap();
            assert!(!finished.load(Ordering::SeqCst));
            probes.provider_target.push(Ok(all_ended));
            events.publish(
                t3_contracts::application_event::ApplicationStoredEvent::Orchestration(Box::new(
                    OrchestrationV2StoredEvent {
                        sequence: 2,
                        command_id: None,
                        event: serde_json::from_value(attempt).unwrap(),
                    },
                )),
            );
        }
    );
    result.unwrap();
    probes.provider_target.next_call().await.unwrap();
    probes
        .provider_target
        .push(Ok(store.provider_target(query).await.unwrap()));
    control
        .interrupt(interrupt(), CancellationToken::new())
        .await
        .unwrap();
    probes.provider_target.next_call().await.unwrap();
    assert!(matches!(
        calls.recv().await.unwrap(),
        FakeCall::Interrupt(_)
    ));
    assert!(calls.try_recv().is_err());
}
