use super::provider_event_ingestor_test::{Harness, decode, fixture};
use serde_json::json;
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::runs_ingestion::run_metrics::*,
};

fn outcome_event(id: &str, status: &str) -> OrchestrationV2StoredEvent {
    let mut run = fixture("OrchestrationV2Run");
    run["id"] = id.into();
    run["threadId"] = "thread:test".into();
    run["providerInstanceId"] = "codex".into();
    run["modelSelection"] = json!({"instanceId":"codex","model":"claude-opus-5-5"});
    run["status"] = status.into();
    run["requestedAt"] = "2026-10-19T12:00:00.000Z".into();
    run["startedAt"] = "2026-10-19T12:00:30.000Z".into();
    run["completedAt"] = "2026-10-19T12:10:30.000Z".into();
    run.as_object_mut().unwrap().shift_remove("delegatedTaskId");
    decode(
        json!({"sequence":1,"commandId":null,"event":{"id":format!("event:{id}"),"threadId":"thread:test","type":"run.updated","occurredAt":"2026-10-19T12:10:30.000Z","payload":run}}),
    )
}

#[test]
fn preparation_does_not_reserve_rolledback_transactions_and_recording_counts_each_run_once()
-> Result<()> {
    let recorder = RunOutcomeRecorder::default();
    let event = outcome_event("run:test", "completed");
    let mut undefined = event.clone();
    if let OrchestrationV2DomainEvent::RunUpdated(event) = &mut undefined.event {
        event.payload.delegated_task_id = Some(None);
    }
    assert!(!RunOutcomeRecorder::prepare(&[undefined], |_| Ok(false))?[0].delegated);
    assert!(
        RunOutcomeRecorder::prepare(&[outcome_event("run:active", "running")], |_| panic!(
            "active run lineage"
        ))?
        .is_empty()
    );
    RunOutcomeRecorder::prepare(std::slice::from_ref(&event), |_| Ok(true))?;
    let mut recorded = Vec::new();
    for _ in 0..2 {
        recorder.record(
            RunOutcomeRecorder::prepare(std::slice::from_ref(&event), |_| Ok(true))?,
            |outcome| recorded.push(outcome.clone()),
        );
    }
    assert_eq!(recorded.len(), 1);
    let outcome = &recorded[0];
    assert!(outcome.delegated);
    assert_eq!(outcome.model_family, Some("claude"));
    assert_eq!(outcome.provider.0, "codex");
    assert_eq!(outcome.outcome, "completed");
    assert_eq!(outcome.queue_wait_ms, Some(30000));
    assert_eq!(outcome.duration_ms, Some(600000));
    assert!(
        RunOutcomeRecorder::prepare(
            &[outcome_event("run:rolledback", "rolled_back")],
            |_| panic!("rollback lineage")
        )?
        .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn lineage_is_read_inside_the_writer_transaction_and_failure_is_nonfatal() -> Result<()> {
    let h = Harness::new().await?;
    let outcomes = h
        .database
        .stores
        .transaction(|tx| {
            RunOutcomeRecorder::prepare(
                &[
                    outcome_event("one", "failed"),
                    outcome_event("two", "cancelled"),
                ],
                |_| {
                    assert!(!tx.is_autocommit());
                    Err(Error::Unsupported("lineage unavailable"))
                },
            )
        })
        .await?;
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|outcome| !outcome.delegated));
    Ok(())
}

#[test]
fn eviction_happens_while_recording_in_batch_order_and_preserves_late_repeated_outcomes()
-> Result<()> {
    let recorder = RunOutcomeRecorder::default();
    let first =
        RunOutcomeRecorder::prepare(&[outcome_event("old", "completed")], |_| Ok(false))?.remove(0);
    recorder.record([first.clone()], |_| {});
    let mut batch = Vec::new();
    for index in 0..4096 {
        let mut outcome = first.clone();
        outcome.run_id = RunId(format!("new:{index}"));
        batch.push(outcome);
    }
    batch.push(first);
    let mut count = 0;
    recorder.record(batch, |_| count += 1);
    assert_eq!(count, 4097);
    Ok(())
}

struct RejectOnce(std::sync::atomic::AtomicBool);
impl ProjectionWriter for RejectOnce {
    fn apply(
        &self,
        tx: &t3_persistence::rusqlite::Transaction<'_>,
        event: &OrchestrationV2StoredEvent,
    ) -> Result<()> {
        t3_orchestration::projections::write::SqliteProjectionWriter.apply(tx, event)?;
        if self.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::Unsupported("injected transaction rollback"));
        }
        Ok(())
    }
}
#[tokio::test]
async fn committed_outcomes_are_published_once_and_a_rolled_back_commit_does_not_reserve_the_run()
-> Result<()> {
    use std::sync::{Arc, atomic::AtomicBool};
    use t3_orchestration::runtime::{commit::EventSink, worker::OutboxSignals};
    let h = Harness::new().await?;
    let sink = EventSink::new(
        h.database.stores.clone(),
        Arc::new(RejectOnce(AtomicBool::new(true))),
        h.clock.clone(),
        OutboxSignals::default(),
    );
    let mut outcomes = sink.subscribe_outcomes();
    let event = outcome_event("run:transaction", "completed").event;
    let request = CommitRequest {
        receipt: None,
        guard: CommitGuard::None,
        decision: Decision {
            events: vec![event],
            ..Default::default()
        },
    };
    assert!(sink.commit(request.clone()).await.is_err());
    assert!(outcomes.try_recv().is_err());
    assert!(
        h.reader
            .run(
                ThreadId("thread:test".into()),
                RunId("run:transaction".into())
            )
            .await?
            .is_none()
    );
    sink.commit(request).await?;
    let outcome = outcomes.recv().await.unwrap();
    assert_eq!(outcome.run_id.0, "run:transaction");
    assert_eq!(outcome.queue_wait_ms, Some(30_000));
    assert_eq!(outcome.duration_ms, Some(600_000));
    assert_eq!(outcome.outcome, "completed");
    let mut duplicate = outcome_event("run:transaction", "completed").event;
    if let OrchestrationV2DomainEvent::RunUpdated(event) = &mut duplicate {
        event.id = serde_json::from_value(json!("event:duplicate"))?;
    }
    sink.commit(CommitRequest {
        receipt: None,
        guard: CommitGuard::None,
        decision: Decision {
            events: vec![duplicate],
            ..Default::default()
        },
    })
    .await?;
    assert!(outcomes.try_recv().is_err());
    sink.drain().await;
    Ok(())
}
