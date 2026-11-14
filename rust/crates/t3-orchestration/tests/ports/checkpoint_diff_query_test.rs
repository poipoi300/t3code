use crate::checkpoint_service_test::support::*;
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use t3_contracts::{checkpoint_diff::*, orchestration_v2_id_table::IdOrIndex};
use t3_orchestration::{
    api::*,
    error::{Error, Result},
    ports::checkpoints_forks::{checkpoint_diff_query::*, checkpoint_service::CheckpointMetadata},
    projections::store::ProjectionStore,
};

#[derive(Default)]
struct Diffs {
    calls: Mutex<Vec<(PathBuf, CheckpointRef, CheckpointRef, bool)>>,
    fail: bool,
}
#[async_trait]
impl CheckpointMetadata for Diffs {
    fn reference(&self, scope: &CheckpointScopeId, ordinal: u64) -> CheckpointRef {
        CheckpointRef(format!("refs/t3/test/{}/{ordinal}", scope.0))
    }
    async fn is_repository(&self, _: PathBuf) -> Result<bool> {
        Ok(true)
    }
    async fn delete_refs(
        &self,
        _: OrchestrationV2CheckpointScope,
        _: Vec<OrchestrationV2Checkpoint>,
    ) -> Result<()> {
        Ok(())
    }
    async fn query_diff(
        &self,
        cwd: PathBuf,
        from: CheckpointRef,
        to: CheckpointRef,
        ignore: bool,
    ) -> Result<String> {
        self.calls
            .lock()
            .expect("calls")
            .push((cwd, from, to, ignore));
        if self.fail {
            Err(Error::Unsupported("scripted diff failure"))
        } else {
            Ok("patch".into())
        }
    }
}
fn query(h: &Harness, diffs: Arc<Diffs>) -> CheckpointDiffQuery {
    CheckpointDiffQuery::new(Arc::new(ProjectionStore::new(h.db.stores.clone())), diffs)
}
fn input(from: u64, to: u64) -> OrchestrationGetTurnDiffInput {
    OrchestrationGetTurnDiffInput {
        thread_id: ThreadId("thread:test".into()),
        from_turn_count: from,
        to_turn_count: to,
        ignore_whitespace: None,
    }
}
async fn checkpoint(
    h: &Harness,
    scope: OrchestrationV2CheckpointScope,
    ordinal: u64,
    run_id: Option<u64>,
) -> Result<()> {
    let mut cp = Arc::new(Backend::default())
        .service()
        .materialize_baseline(capture_input(scope, ordinal))
        .await?;
    cp.status = CheckpointStatus::Ready;
    cp.app_run_ordinal = run_id;
    cp.run_id = run_id.map(|n| IdOrIndex::Id(RunId(format!("run:{n}"))));
    cp.r#ref = CheckpointRef(format!("refs/t3/test/ordinal/{ordinal}"));
    h.seed("checkpoint.captured", cp).await
}
#[tokio::test]
async fn stopped_scope_parent_and_full_thread_baseline_use_metadata_without_decoding_unrelated_execution_rows()
-> Result<()> {
    for stopped_capture in [false, true] {
        let root = tempfile::tempdir()?;
        let h = Harness::new().await?;
        let diffs = Arc::new(Diffs::default());
        h.seed_run(root.path(), 1, "completed").await?;
        h.seed_run(root.path(), 2, "interrupted").await?;
        let scope = h.seed_run(root.path(), 3, "completed").await?;
        for (ordinal, run) in [
            (0, None),
            (1, Some(1)),
            (2, stopped_capture.then_some(2)),
            (3, Some(3)),
        ] {
            checkpoint(&h, scope.clone(), ordinal, run).await?;
        }
        h.db.stores.transaction(|db|{db.execute("UPDATE orchestration_v2_projection_nodes SET payload_json='not json' WHERE thread_id='thread:test'",[])?;Ok(())}).await?;
        let service = query(&h, diffs.clone());
        assert_eq!(
            service.get_turn_diff(input(2, 3)).await.expect("diff").diff,
            "patch"
        );
        service.get_turn_diff(input(1, 3)).await.expect("range");
        service
            .get_full_thread_diff(OrchestrationGetFullThreadDiffInput {
                thread_id: ThreadId("thread:test".into()),
                to_turn_count: 3,
                ignore_whitespace: Some(false),
            })
            .await
            .expect("full");
        {
            let calls = diffs.calls.lock().expect("calls");
            assert_eq!(calls.len(), 3);
            assert_eq!(calls[0].1.0, "refs/t3/test/ordinal/2");
            assert_eq!(calls[1].1.0, "refs/t3/test/ordinal/1");
            assert_eq!(calls[2].1,diffs.reference(&t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(&scope.id)?,0));
            assert!(calls[0].3);
            assert!(!calls[2].3);
        }
        assert!(matches!(
            service.get_turn_diff(input(1, 2)).await,
            Err(CheckpointDiffQueryError::RefUnavailable {
                checkpoint: "to",
                turn_count: 2,
                ..
            })
        ));
    }
    Ok(())
}
#[tokio::test]
async fn rolled_back_runs_do_not_extend_available_range_and_empty_ranges_skip_storage() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let diffs = Arc::new(Diffs::default());
    let scope = h.seed_run(root.path(), 1, "completed").await?;
    h.seed_run(root.path(), 2, "rolled_back").await?;
    checkpoint(&h, scope.clone(), 1, Some(1)).await?;
    checkpoint(&h, scope, 2, Some(2)).await?;
    let service = query(&h, diffs.clone());
    let range = service
        .get_turn_diff(input(0, 2))
        .await
        .expect_err("rolled-back destination");
    assert_eq!(
        range.cause(),
        serde_json::json!({
            "_tag":"CheckpointTurnRangeUnavailableError","operation":"CheckpointDiffQuery.getTurnDiff","threadId":"thread:test",
            "requestedTurnCount":2,"availableTurnCount":1,
            "message":"Checkpoint unavailable for thread thread:test turn 2: Turn diff range exceeds current turn count: requested 2, current 1."
        })
    );
    let empty = CheckpointDiffQuery::new(
        Arc::new(t3_orchestration::testing::FakeProjectionReader::default()),
        diffs.clone(),
    )
    .get_turn_diff(input(9, 9))
    .await
    .expect("empty without lookup");
    assert_eq!(empty.diff, "");
    assert!(diffs.calls.lock().expect("calls").is_empty());
    Ok(())
}

#[tokio::test]
async fn missing_refs_and_destination_workspace_report_typed_errors_and_backend_failure_propagates()
-> Result<()> {
    use t3_orchestration::{
        projections::control::CheckpointContext, testing::FakeProjectionReader,
    };
    let root = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let scope = h.seed_run(root.path(), 3, "completed").await?;
    checkpoint(&h, scope, 7, Some(3)).await?;
    let context = ProjectionStore::new(h.db.stores.clone())
        .checkpoint_context(ThreadId("thread:test".into()))
        .await?;
    let reader = Arc::new(FakeProjectionReader::default());
    reader
        .checkpoint_context
        .push(Err(Error::Unsupported("database failure")));
    reader.checkpoint_context.push(Ok(CheckpointContext {
        checkpoint_scopes: vec![],
        ..context.clone()
    }));
    reader.checkpoint_context.push(Ok(context.clone()));
    reader.checkpoint_context.push(Ok(context.clone()));
    reader.checkpoint_context.push(Ok(context));
    let diffs = Arc::new(Diffs {
        fail: true,
        ..Default::default()
    });
    let service = CheckpointDiffQuery::new(reader, diffs.clone());
    assert!(matches!(
        service.get_turn_diff(input(0, 3)).await,
        Err(CheckpointDiffQueryError::ThreadNotFound { .. })
    ));
    assert!(matches!(
        service.get_turn_diff(input(0, 3)).await,
        Err(CheckpointDiffQueryError::WorkspacePathMissing { .. })
    ));
    assert!(matches!(
        service.get_turn_diff(input(1, 2)).await,
        Err(CheckpointDiffQueryError::RefUnavailable {
            checkpoint: "to",
            turn_count: 2,
            ..
        })
    ));
    assert!(matches!(
        service.get_turn_diff(input(1, 3)).await,
        Err(CheckpointDiffQueryError::RefUnavailable {
            checkpoint: "from",
            turn_count: 1,
            ..
        })
    ));
    assert!(matches!(
        service
            .get_full_thread_diff(OrchestrationGetFullThreadDiffInput {
                thread_id: ThreadId("thread:test".into()),
                to_turn_count: 3,
                ignore_whitespace: None
            })
            .await,
        Err(CheckpointDiffQueryError::Backend(Error::Unsupported(
            "scripted diff failure"
        )))
    ));
    assert_eq!(diffs.calls.lock().expect("calls").len(), 1);
    Ok(())
}

#[tokio::test]
async fn cross_scope_comparisons_use_the_destination_cwd_and_the_first_root_baseline() -> Result<()>
{
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let h = Harness::new().await?;
    let diffs = Arc::new(Diffs::default());
    let from = h.seed_run(first.path(), 1, "completed").await?;
    checkpoint(&h, from.clone(), 1, Some(1)).await?;
    let to = h.seed_run(second.path(), 3, "completed").await?;
    checkpoint(&h, to, 3, Some(3)).await?;
    let service = query(&h, diffs.clone());
    service
        .get_turn_diff(input(1, 3))
        .await
        .expect("cross-scope");
    service
        .get_full_thread_diff(OrchestrationGetFullThreadDiffInput {
            thread_id: ThreadId("thread:test".into()),
            to_turn_count: 3,
            ignore_whitespace: None,
        })
        .await
        .expect("full");
    let calls = diffs.calls.lock().expect("calls");
    assert_eq!(calls[0].0, second.path());
    assert_eq!(calls[1].0, second.path());
    assert_eq!(calls[0].1.0, "refs/t3/test/ordinal/1");
    assert_eq!(
        calls[1].1,
        diffs.reference(
            &t3_orchestration::ports::checkpoints_forks::checkpoint_restore_safety::expanded_id(
                &from.id
            )?,
            0
        )
    );
    Ok(())
}
