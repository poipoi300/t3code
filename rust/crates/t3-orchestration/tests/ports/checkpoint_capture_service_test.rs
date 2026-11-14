use crate::checkpoint_service_test::support::*;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use t3_orchestration::{
    api::*,
    error::Result,
    ports::checkpoints_forks::{
        checkpoint_capture_service::CheckpointCaptureService,
        checkpoint_restore_safety::expanded_id,
    },
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn capture_finishes_runs_without_decoding_unrelated_history_and_is_idempotent() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    for lookup_failure in [false, true] {
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        let checkpoints = backend.service();
        let scope = h.seed_run(temp.path(), 1, "waiting").await?;
        checkpoints
            .capture_baseline(scope.clone(), 0, CancellationToken::new())
            .await?;
        if lookup_failure {
            backend.mode.store(4, Ordering::SeqCst);
        }
        let mut run = run(1, "waiting");
        run.delegated_completion = Some(Some(serde_json::from_value(
            json!({"disposition":"open","nextGeneration":2,"delivery":{"generation":1,"messageId":"delivery","taskIds":["task"]}}),
        )?));
        let mut older_run = run.clone();
        older_run.delegated_completion = Some(Some(serde_json::from_value(
            json!({"disposition":"open","nextGeneration":1,"delivery":null}),
        )?));
        h.seed("run.updated", older_run).await?;
        h.db.stores.transaction(|db|{db.execute("INSERT INTO orchestration_v2_projection_messages (message_id,thread_id,run_id,node_id,role,streaming,created_at,updated_at,payload_json) VALUES ('unrelated','thread:test',NULL,NULL,'assistant',0,?1,?1,'{}')",[t3_orchestration::testing::TEST_TIME])?;Ok(())}).await?;
        let service = Arc::new(CheckpointCaptureService {
            checkpoints,
            projections: h.reader.clone(),
            events: h.sink.clone(),
            ids: h.ids.clone(),
            clock: h.clock.clone(),
        });
        assert!(
            service
                .execute(
                    ThreadId("thread:test".into()),
                    RunId("run:1".into()),
                    CheckpointScopeId("missing".into()),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        let mut committed = h.sink.subscribe();
        let gate = Arc::new(Gate::default());
        *backend.gate.lock().expect("gate") = Some(gate.clone());
        let (probes, lookups) = (
            backend.probes.load(Ordering::SeqCst),
            backend.lookups.load(Ordering::SeqCst),
        );
        let capture = {
            let service = service.clone();
            let scope_id = expanded_id(&scope.id)?;
            tokio::spawn(async move {
                service
                    .execute(
                        ThreadId("thread:test".into()),
                        RunId("run:1".into()),
                        scope_id,
                        CancellationToken::new(),
                    )
                    .await
            })
        };
        gate.entered
            .acquire()
            .await
            .expect("capture entered")
            .forget();
        h.seed("run.updated", &run).await?;
        gate.release.add_permits(1);
        capture.await.expect("capture task")?;
        // Finalization probes the repository once and reuses the baseline lookup for the diff.
        assert_eq!(backend.probes.load(Ordering::SeqCst) - probes, 1);
        if !lookup_failure {
            assert_eq!(backend.lookups.load(Ordering::SeqCst) - lookups, 1);
        }
        let observed = committed.recv().await.expect("committed event");
        assert!(matches!(
            observed,
            t3_contracts::application_event::ApplicationStoredEvent::Orchestration(_)
        ));
        let projected = h
            .reader
            .run(ThreadId("thread:test".into()), RunId("run:1".into()))
            .await?
            .expect("run");
        assert_eq!(projected.status, OrchestrationV2RunStatus::Completed);
        assert!(projected.checkpoint_id.is_some());
        assert_eq!(projected.delegated_completion, run.delegated_completion);
        let before = backend.captures.load(Ordering::SeqCst);
        service
            .execute(
                ThreadId("thread:test".into()),
                RunId("run:1".into()),
                expanded_id(&scope.id)?,
                CancellationToken::new(),
            )
            .await?;
        assert_eq!(backend.captures.load(Ordering::SeqCst), before);
        let events = h
            .sink
            .events_by_command(CommandId("command:effect:checkpoint.capture:run:1".into()))
            .await?;
        let run_event = events
            .into_iter()
            .find_map(|stored| match stored.event {
                OrchestrationV2DomainEvent::RunUpdated(event) => Some(event),
                _ => None,
            })
            .expect("run event");
        assert!(
            serde_json::to_value(run_event.payload)?
                .get("delegatedCompletion")
                .is_none()
        );
    }
    Ok(())
}

#[tokio::test]
async fn stopped_runs_keep_terminal_state_and_rolled_back_runs_do_not_recapture() -> Result<()> {
    let temp = tempfile::tempdir()?;
    for status in ["interrupted", "cancelled", "rolled_back"] {
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        let scope = h.seed_run(temp.path(), 1, status).await?;
        let service = CheckpointCaptureService {
            checkpoints: backend.service(),
            projections: h.reader.clone(),
            events: h.sink.clone(),
            ids: h.ids.clone(),
            clock: h.clock.clone(),
        };
        service
            .execute(
                ThreadId("thread:test".into()),
                RunId("run:1".into()),
                expanded_id(&scope.id)?,
                CancellationToken::new(),
            )
            .await?;
        let projected = h
            .reader
            .run(ThreadId("thread:test".into()), RunId("run:1".into()))
            .await?
            .expect("run");
        assert_eq!(serde_json::to_value(projected.status)?, json!(status));
        assert_eq!(projected.completed_at, run(1, status).completed_at);
        assert_eq!(projected.checkpoint_id.is_some(), status != "rolled_back");
        assert_eq!(
            backend.captures.load(Ordering::SeqCst),
            usize::from(status != "rolled_back")
        );
        if status != "rolled_back" {
            let nodes = h
                .reader
                .records(
                    ThreadId("thread:test".into()),
                    vec![RecordKind::Nodes],
                    RecordFilter::default(),
                )
                .await?;
            let ProjectionRecordSet::Nodes(nodes) = &nodes.records[0] else {
                panic!("nodes")
            };
            assert_eq!(serde_json::to_value(&nodes[0].status)?, json!(status));
        }
    }
    Ok(())
}

#[tokio::test]
async fn baselines_belong_to_the_materializing_run_even_after_scope_ownership_moves() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    for ordinal in [1, 2] {
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        let first_scope = h.seed_run(temp.path(), ordinal, "waiting").await?;
        let later_scope = scope(temp.path(), ordinal + 1);
        h.seed("checkpoint-scope.created", later_scope).await?;
        let service = CheckpointCaptureService {
            checkpoints: backend.service(),
            projections: h.reader.clone(),
            events: h.sink.clone(),
            ids: h.ids.clone(),
            clock: h.clock.clone(),
        };
        service
            .execute(
                ThreadId("thread:test".into()),
                RunId(format!("run:{ordinal}")),
                expanded_id(&first_scope.id)?,
                CancellationToken::new(),
            )
            .await?;
        let result = h
            .reader
            .records(
                ThreadId("thread:test".into()),
                vec![RecordKind::Checkpoints],
                RecordFilter::default(),
            )
            .await?;
        let ProjectionRecordSet::Checkpoints(checkpoints) = &result.records[0] else {
            panic!("checkpoints")
        };
        let baselines: Vec<_> = checkpoints.iter().filter(|c| c.run_id.is_none()).collect();
        assert_eq!(baselines.len(), if ordinal == 1 { 1 } else { 2 });
        for baseline in baselines {
            assert_eq!(
                expanded_id(&baseline.node_id)?,
                NodeId(format!("node:{ordinal}"))
            );
            assert_eq!(baseline.captured_at, time());
            assert!(baseline.app_run_ordinal.is_none());
            assert!(baseline.parent_checkpoint_id.is_none());
        }
    }
    Ok(())
}

#[tokio::test]
async fn side_chats_complete_with_missing_checkpoints_without_git_access() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let backend = Arc::new(Backend::default());
    let mut thread = thread("thread:test", Some(temp.path()));
    thread.lineage.side_chat = Some(Some(true));
    h.seed("thread.created", thread).await?;
    let scope = h.seed_run(temp.path(), 1, "waiting").await?;
    let service = CheckpointCaptureService {
        checkpoints: backend.service(),
        projections: h.reader.clone(),
        events: h.sink.clone(),
        ids: h.ids.clone(),
        clock: h.clock.clone(),
    };
    service
        .execute(
            ThreadId("thread:test".into()),
            RunId("run:1".into()),
            expanded_id(&scope.id)?,
            CancellationToken::new(),
        )
        .await?;
    assert_eq!(backend.captures.load(Ordering::SeqCst), 0);
    assert_eq!(backend.lookups.load(Ordering::SeqCst), 0);
    let preserved = h
        .reader
        .checkpoint_capture(
            ThreadId("thread:test".into()),
            RunId("run:1".into()),
            expanded_id(&scope.id)?,
        )
        .await?;
    assert_eq!(preserved.preserved_ordinals, vec![1]);
    let result = h
        .reader
        .records(
            ThreadId("thread:test".into()),
            vec![RecordKind::Checkpoints],
            RecordFilter::default(),
        )
        .await?;
    let ProjectionRecordSet::Checkpoints(checkpoints) = &result.records[0] else {
        panic!("checkpoints")
    };
    assert_eq!(checkpoints.len(), 2);
    assert!(
        checkpoints
            .iter()
            .all(|c| c.status == CheckpointStatus::Missing)
    );
    let run = h
        .reader
        .run(ThreadId("thread:test".into()), RunId("run:1".into()))
        .await?
        .expect("run");
    assert_eq!(run.status, OrchestrationV2RunStatus::Completed);
    assert!(run.checkpoint_id.is_some());
    let raw: Value = serde_json::to_value(checkpoints)?;
    assert!(
        raw.as_array()
            .expect("checkpoints")
            .iter()
            .all(|c| c["files"] == json!([]))
    );
    Ok(())
}

#[tokio::test]
async fn unpreserved_baselines_are_rematerialized_while_ready_baselines_survive() -> Result<()> {
    for status in [
        CheckpointStatus::Ready,
        CheckpointStatus::Missing,
        CheckpointStatus::Error,
        CheckpointStatus::Stale,
    ] {
        let temp = tempfile::tempdir()?;
        let h = Harness::new().await?;
        let backend = Arc::new(Backend::default());
        let checkpoints = backend.service();
        let first = h.seed_run(temp.path(), 1, "completed").await?;
        for ordinal in [0, 1] {
            checkpoints
                .capture_baseline(first.clone(), ordinal, CancellationToken::new())
                .await?;
            let mut baseline = checkpoints
                .materialize_baseline(capture_input(first.clone(), ordinal))
                .await?;
            baseline.status = status.clone();
            baseline.node_id =
                t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(NodeId("node:1".into()));
            h.seed("checkpoint.captured", baseline).await?;
        }
        let scope = h.seed_run(temp.path(), 2, "waiting").await?;
        let service = CheckpointCaptureService {
            checkpoints,
            projections: h.reader.clone(),
            events: h.sink.clone(),
            ids: h.ids.clone(),
            clock: h.clock.clone(),
        };
        let scope_id = expanded_id(&scope.id)?;
        let before = h
            .reader
            .checkpoint_capture(
                ThreadId("thread:test".into()),
                RunId("run:2".into()),
                scope_id.clone(),
            )
            .await?;
        assert_eq!(
            before.preserved_ordinals,
            if status == CheckpointStatus::Ready {
                vec![0, 1]
            } else {
                vec![]
            }
        );
        service
            .execute(
                ThreadId("thread:test".into()),
                RunId("run:2".into()),
                scope_id,
                CancellationToken::new(),
            )
            .await?;
        let records = h
            .reader
            .records(
                ThreadId("thread:test".into()),
                vec![RecordKind::Checkpoints],
                RecordFilter::default(),
            )
            .await?;
        let ProjectionRecordSet::Checkpoints(checkpoints) = &records.records[0] else {
            panic!("checkpoints")
        };
        assert_eq!(checkpoints.len(), 3);
        for checkpoint in &checkpoints[..2] {
            assert_eq!(checkpoint.status, CheckpointStatus::Ready);
            assert_eq!(
                expanded_id(&checkpoint.node_id)?,
                NodeId(
                    if status == CheckpointStatus::Ready {
                        "node:1"
                    } else {
                        "node:2"
                    }
                    .into()
                )
            );
            assert!(checkpoint.run_id.is_none());
        }
    }
    Ok(())
}
