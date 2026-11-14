use super::thread_stream_test::{command, projection_value};
use serde_json::json;
use std::sync::Arc;
use t3_contracts::orchestration_v2::*;
use t3_orchestration::{
    api::{CommitGuard, CommitRequest, Decision, EventCommitter},
    error::Result,
    projections::store::ProjectionStore,
    runtime::{commit::EventSink, worker::OutboxSignals},
    testing::{ManualClock, TEST_TIME, TestDatabase, detached_event, project_event},
    transport::service::TransportService,
};
pub fn service(fixture: &TestDatabase) -> (TransportService, EventSink) {
    let projections = Arc::new(ProjectionStore::new(fixture.stores.clone()));
    let sink = EventSink::new(
        fixture.stores.clone(),
        projections.clone(),
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
    );
    (
        TransportService::new(projections, Arc::new(sink.clone()), fixture.stores.clone()),
        sink,
    )
}
pub async fn commit(sink: &EventSink, events: Vec<OrchestrationV2DomainEvent>) -> Result<u64> {
    let result = sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events,
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await?;
    Ok(result.events.last().map_or(0, |event| event.sequence))
}
pub async fn seed(fixture: &TestDatabase, sink: &EventSink) -> Result<u64> {
    fixture
        .stores
        .transaction(|tx| {
            t3_orchestration::stores::projects::apply(
                tx,
                &project_event("project-create", "project")?,
            )
        })
        .await?;
    commit(sink,vec![serde_json::from_value(json!({"id":"thread-create","threadId":"thread","type":"thread.created","occurredAt":TEST_TIME,"payload":projection_value(0)["thread"]}))?]).await
}
fn input(after: u64) -> OrchestrationV2SubscribeThreadInput {
    serde_json::from_value(
        json!({"threadId":"thread","afterSequence":after,"requestCompletionMarker":true}),
    )
    .expect("test value")
}
#[tokio::test]
async fn replay_obeys_global_sequence_gaps_and_completion_precedes_committed_live_events()
-> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (service, sink) = service(&fixture);
    let base = seed(&fixture, &sink).await?;
    fixture
        .stores
        .transaction(|tx| {
            t3_orchestration::stores::event_store::EventStore::new()
                .append_project(tx, &project_event("project-gap", "other-project")?)?;
            Ok(())
        })
        .await?;
    let replay = commit(&sink, vec![detached_event("replayed", "thread")?]).await?;
    assert!(replay > base + 1);
    let mut subscription = service
        .subscribe_thread(input(base), Default::default())
        .await?;
    let first = subscription.pull().await?.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].sequence, replay);
    assert!(matches!(
        first[0].value,
        OrchestrationV2ThreadStreamItem::Event(_)
    ));
    let live = commit(&sink, vec![detached_event("live", "thread")?]).await?;
    assert!(matches!(
        subscription.pull().await?.unwrap()[0].value,
        OrchestrationV2ThreadStreamItem::Synchronized(_)
    ));
    let batch = subscription.pull().await?.unwrap();
    assert_eq!(
        batch.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        vec![live]
    );
    subscription.close().await?;
    Ok(())
}
#[tokio::test]
async fn an_ahead_client_receives_a_lower_authoritative_snapshot_then_resumes() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (service, sink) = service(&fixture);
    let sequence = seed(&fixture, &sink).await?;
    let mut subscription = service
        .subscribe_thread(input(sequence + 1000), Default::default())
        .await?;
    let snapshot = subscription.pull().await?.unwrap();
    let OrchestrationV2ThreadStreamItem::Snapshot(snapshot) = &snapshot[0].value else {
        panic!()
    };
    assert_eq!(snapshot.snapshot_sequence, sequence);
    assert!(matches!(
        subscription.pull().await?.unwrap()[0].value,
        OrchestrationV2ThreadStreamItem::Synchronized(_)
    ));
    let live = commit(&sink, vec![detached_event("after-reset", "thread")?]).await?;
    assert_eq!(subscription.pull().await?.unwrap()[0].sequence, live);
    subscription.close().await?;
    Ok(())
}
#[tokio::test]
async fn pruning_beyond_an_idle_thread_does_not_force_a_snapshot() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (service, sink) = service(&fixture);
    let head = seed(&fixture, &sink).await?;
    fixture.stores.transaction(|tx|{tx.execute("INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES('event-log-pruned-through',2,10000,?1)",[TEST_TIME])?;Ok(())}).await?;
    let mut subscription = service
        .subscribe_thread(input(head), Default::default())
        .await?;
    assert!(matches!(
        subscription.pull().await?.unwrap()[0].value,
        OrchestrationV2ThreadStreamItem::Synchronized(_)
    ));
    subscription.close().await?;
    Ok(())
}
#[tokio::test]
async fn raw_replay_bytes_force_snapshot_before_decoding_projectable_tool_output() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (service, sink) = service(&fixture);
    let base = seed(&fixture, &sink).await?;
    let mut tool = command("large-result", 1);
    tool["type"] = "dynamic_tool".into();
    tool["toolName"] = "large".into();
    tool["input"] = json!({});
    tool["output"] = json!({"text":"x".repeat(2*1_048_576)});
    let head=commit(&sink,vec![serde_json::from_value(json!({"id":"large-result-event","threadId":"thread","type":"turn-item.updated","occurredAt":TEST_TIME,"payload":tool}))?]).await?;
    let mut subscription = service
        .subscribe_thread(input(base), Default::default())
        .await?;
    let first = subscription.pull().await?.unwrap();
    let OrchestrationV2ThreadStreamItem::Snapshot(snapshot) = &first[0].value else {
        panic!()
    };
    assert_eq!(snapshot.snapshot_sequence, head);
    assert!(!serde_json::to_string(&snapshot.projection)?.contains(&"x".repeat(1000)));
    subscription.close().await?;
    Ok(())
}
#[tokio::test]
async fn explicit_client_replay_skips_unknown_types_and_keeps_the_high_water() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    let (service, sink) = service(&fixture);
    let base = seed(&fixture, &sink).await?;
    let unknown = commit(&sink, vec![detached_event("future-event", "thread")?]).await?;
    fixture
        .stores
        .transaction(move |tx| {
            tx.execute(
                "UPDATE orchestration_events SET event_type='future.updated' WHERE sequence=?1",
                [unknown as i64],
            )?;
            Ok(())
        })
        .await?;
    let retained = commit(&sink, vec![detached_event("known-after-future", "thread")?]).await?;
    let mut subscription = service
        .subscribe_thread(input(base), Default::default())
        .await?;
    assert_eq!(subscription.pull().await?.unwrap()[0].sequence, retained);
    assert!(matches!(
        subscription.pull().await?.unwrap()[0].value,
        OrchestrationV2ThreadStreamItem::Synchronized(_)
    ));
    subscription.close().await?;
    Ok(())
}
#[tokio::test]
async fn failed_commits_never_reach_thread_subscribers() -> Result<()> {
    use t3_orchestration::{api::ProjectionWriter, error::Error};
    struct Reject;
    impl ProjectionWriter for Reject {
        fn apply(
            &self,
            _tx: &t3_persistence::rusqlite::Transaction<'_>,
            _event: &OrchestrationV2StoredEvent,
        ) -> Result<()> {
            Err(Error::Service {
                operation: "test",
                detail: "projection failed".into(),
            })
        }
    }
    let fixture = TestDatabase::memory()?;
    let (service, sink) = service(&fixture);
    let base = seed(&fixture, &sink).await?;
    let rejected = EventSink::new(
        fixture.stores.clone(),
        Arc::new(Reject),
        Arc::new(ManualClock::new(TEST_TIME)),
        OutboxSignals::default(),
    );
    let mut committed = rejected.subscribe();
    assert!(
        commit(&rejected, vec![detached_event("failed", "thread")?])
            .await
            .is_err()
    );
    assert!(matches!(
        committed.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let mut subscription = service
        .subscribe_thread(input(base), Default::default())
        .await?;
    let _marker = subscription.pull().await?;
    let live = commit(&sink, vec![detached_event("survived", "thread")?]).await?;
    assert_eq!(subscription.pull().await?.unwrap()[0].sequence, live);
    subscription.close().await?;
    Ok(())
}
