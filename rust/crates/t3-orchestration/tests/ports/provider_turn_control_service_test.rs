use super::{
    provider_event_ingestor_test::{Harness, decode, fixture},
    provider_turn_start_service_test::{call, seed, sessions},
};
use serde_json::json;
use std::sync::Arc;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_orchestration::{
    api::*, error::Result,
    ports::runs_ingestion::provider_turn_control_service::ProviderTurnControlService,
    testing::FakeCall,
};
use tokio_util::sync::CancellationToken;

fn target() -> ProviderControlTarget {
    ProviderControlTarget {
        thread_id: ThreadId("thread:test".into()),
        provider_session_id: ProviderSessionId("session:test".into()),
        provider_thread_id: ProviderThreadId("pt:test".into()),
        provider_turn_id: ProviderTurnId("turn:test".into()),
    }
}

async fn running_turn(h: &Harness) -> Result<OrchestrationV2ProviderTurn> {
    let mut state = seed(h).await?;
    state.attempt.status = RunAttemptStatus::Superseded;
    state.attempt.completed_at = Some(h.clock.now());
    h.write("run-attempt.updated", json!(state.attempt)).await?;
    let mut turn = fixture("OrchestrationV2ProviderTurn");
    turn["id"] = "turn:test".into();
    turn["providerThreadId"] = "pt:test".into();
    turn["nodeId"] = "node:test".into();
    turn["runAttemptId"] = "attempt:test".into();
    turn["status"] = "running".into();
    turn["completedAt"] = serde_json::Value::Null;
    h.write("provider-turn.updated", turn.clone()).await?;
    Ok(decode(turn))
}

#[tokio::test]
async fn historical_interrupt_requires_the_exact_committed_restart_replacement()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let h = Harness::new().await?;
    let mut turn = running_turn(&h).await?;
    let mut thread = h
        .reader
        .control(ControlQuery::ProviderThread(target().provider_thread_id))
        .await?
        .unwrap()
        .provider_thread
        .unwrap();
    thread.provider_session_id = Some(IdOrIndex::Id(ProviderSessionId("replacement".into())));
    h.write("provider-thread.updated", json!(thread)).await?;
    let (adapter, mut calls) = h.fake();
    let service = ProviderTurnControlService {
        projections: h.reader.clone(),
        sessions: sessions(Arc::new(adapter)),
        events: h.sink.clone(),
    };
    assert!(
        service
            .interrupt(
                InterruptTurn {
                    target: target(),
                    subagent: None,
                    requested_at: None
                },
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let restart = |replacement| RestartTurn {
        target: target(),
        replacement_provider_session_id: Some(ProviderSessionId(replacement)),
        interrupted_attempt_id: RunAttemptId("attempt:test".into()),
    };
    assert!(
        service
            .interrupt_and_await_terminal(restart("unrelated".into()), CancellationToken::new())
            .await
            .is_err()
    );
    assert!(calls.try_recv().is_err());
    let (result, ()) = tokio::join!(
        service
            .interrupt_and_await_terminal(restart("replacement".into()), CancellationToken::new()),
        async {
            let FakeCall::Interrupt(input) = call(&mut calls).await else {
                panic!("expected interrupt")
            };
            assert_eq!(
                input.provider_thread.provider_session_id,
                Some(IdOrIndex::Id(target().provider_session_id))
            );
            assert_eq!(input.provider_thread.id, thread.id);
            assert_eq!(
                input.provider_thread.native_thread_ref,
                thread.native_thread_ref
            );
            assert!(!input.request_runtime_restart);
            turn.status = ProviderTurnStatus::Interrupted;
            turn.completed_at = Some(h.clock.now());
            h.write("provider-turn.updated", json!(turn)).await.unwrap();
        }
    );
    result?;
    Ok(())
}

#[tokio::test]
async fn missing_runtime_is_stopped_for_control_but_cannot_confirm_a_subagent_stop()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let h = Harness::new().await?;
    let turn = running_turn(&h).await?;
    let (adapter, mut calls) = h.fake();
    let mut thread = h
        .reader
        .control(ControlQuery::ProviderThread(target().provider_thread_id))
        .await?
        .unwrap()
        .provider_thread
        .unwrap();
    thread.provider_session_id = Some(IdOrIndex::Id(ProviderSessionId("gone".into())));
    h.write("provider-thread.updated", json!(thread)).await?;
    let service = ProviderTurnControlService {
        projections: h.reader.clone(),
        sessions: sessions(Arc::new(adapter)),
        events: h.sink.clone(),
    };
    let target = ProviderControlTarget {
        provider_session_id: ProviderSessionId("gone".into()),
        ..target()
    };
    assert!(
        service
            .interrupt(
                InterruptTurn {
                    target: target.clone(),
                    subagent: None,
                    requested_at: None
                },
                CancellationToken::new()
            )
            .await?
            .is_none()
    );
    service
        .interrupt_and_await_terminal(
            RestartTurn {
                target: target.clone(),
                replacement_provider_session_id: None,
                interrupted_attempt_id: expanded_attempt(&turn),
            },
            CancellationToken::new(),
        )
        .await?;
    let error = service
        .interrupt(
            InterruptTurn {
                target,
                subagent: Some(t3_orchestration::effects::InterruptSubagent {
                    id: NodeId("agent".into()),
                    native_task_id: "native-agent".into(),
                }),
                requested_at: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("No live provider session"));
    assert!(calls.try_recv().is_err());
    Ok(())
}

fn expanded_attempt(turn: &OrchestrationV2ProviderTurn) -> RunAttemptId {
    let Some(IdOrIndex::Id(id)) = &turn.run_attempt_id else {
        panic!("expected attempt")
    };
    id.clone()
}

#[tokio::test]
async fn steering_a_completed_turn_reports_completion_without_delivering_to_the_provider()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let h = Harness::new().await?;
    let mut turn = running_turn(&h).await?;
    turn.status = ProviderTurnStatus::Completed;
    h.write("provider-turn.updated", json!(turn)).await?;
    let (adapter, mut calls) = h.fake();
    let service = ProviderTurnControlService {
        projections: h.reader.clone(),
        sessions: sessions(Arc::new(adapter)),
        events: h.sink.clone(),
    };
    let error = service
        .steer(
            SteerTurn {
                target: target(),
                message_id: MessageId("missing".into()),
                model_selection: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.turn_completed);
    assert!(!error.steer_declined);
    let error = ProviderTurnControl::steer(
        &service,
        SteerTurn {
            target: target(),
            message_id: MessageId("missing".into()),
            model_selection: None,
        },
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, t3_orchestration::error::Error::ProviderTurnControl(error) if error.turn_completed && !error.steer_declined)
    );
    assert!(calls.try_recv().is_err());
    Ok(())
}
