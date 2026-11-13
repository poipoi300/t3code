#![allow(clippy::unwrap_used)]
use crate::projection_store_test::{apply_values, fixture};
use t3_orchestration::{api::*, projections::store::ProjectionStore, testing::TestDatabase};

#[tokio::test]
async fn completion_reads_avoid_corrupt_transcript_and_scope_capture_by_primary_keys() {
    let db = TestDatabase::memory().unwrap();
    let mut scope = fixture("checkpoint-scope.created", "scope:test");
    scope["payload"]["runId"] = "run:test".into();
    scope["payload"]["nodeId"] = "node:test".into();
    let mut checkpoint = fixture("checkpoint.captured", "checkpoint:ready");
    checkpoint["payload"]["scopeId"] = "scope:test".into();
    checkpoint["payload"]["status"] = "ready".into();
    checkpoint["payload"]["ordinalWithinScope"] = 0.into();
    let mut missing = checkpoint.clone();
    missing["payload"]["id"] = "checkpoint:missing".into();
    missing["payload"]["status"] = "missing".into();
    missing["payload"]["runId"] = "run:test".into();
    missing["payload"]["ordinalWithinScope"] = 1.into();
    let mut stale = missing.clone();
    stale["payload"]["id"] = "checkpoint:stale".into();
    stale["payload"]["status"] = "stale".into();
    stale["payload"]["ordinalWithinScope"] = 2.into();
    apply_values(
        &db,
        vec![
            fixture("thread.created", "thread:test"),
            fixture("run.created", "run:test"),
            fixture("node.updated", "node:test"),
            scope,
            checkpoint,
            missing,
            stale,
            fixture("message.updated", "message:test"),
        ],
    )
    .await;
    db.stores
        .transaction(|tx| {
            tx.execute(
                "UPDATE orchestration_v2_projection_messages SET payload_json='{}'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let store = ProjectionStore::new(db.stores.clone());
    assert!(
        store
            .projection(ThreadId("thread:test".into()))
            .await
            .is_err()
    );
    assert!(
        store
            .run(ThreadId("thread:test".into()), RunId("run:test".into()))
            .await
            .unwrap()
            .is_some()
    );
    let context = store
        .checkpoint_capture(
            ThreadId("thread:test".into()),
            RunId("run:test".into()),
            CheckpointScopeId("scope:test".into()),
        )
        .await
        .unwrap();
    assert!(context.run.is_some());
    assert!(context.root_node.is_some());
    assert!(context.scope.is_some());
    assert_eq!(context.preserved_ordinals, vec![0, 1]);
    db.stores.transaction(|tx|{tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json='invalid'",[])?;tx.execute("UPDATE orchestration_v2_projection_checkpoints SET payload_json=json_set(payload_json,'$.files','invalid files')",[])?;Ok(())}).await.unwrap();
    let reader: &dyn ProjectionReader = &store;
    let metadata = reader
        .checkpoint_context(ThreadId("thread:test".into()))
        .await
        .unwrap();
    assert_eq!(metadata.runs.len(), 1);
    assert_eq!(metadata.runs[0].id, RunId("run:test".into()));
    assert_eq!(metadata.checkpoint_scopes.len(), 1);
    assert_eq!(
        metadata
            .checkpoints
            .iter()
            .map(|c| c.ordinal_within_scope)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}
