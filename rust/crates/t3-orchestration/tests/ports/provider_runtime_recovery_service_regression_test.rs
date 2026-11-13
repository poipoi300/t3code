use super::provider_runtime_recovery_service_test::{
    item, payload, projection, seed, service, sink,
};
use serde_json::json;
use t3_orchestration::{api::RecoveryService, error::Result, testing::TestDatabase};

#[tokio::test]
async fn stale_background_without_provider_threads_uses_thread_provider() -> Result<()> {
    let database = TestDatabase::memory()?;
    let mut projection = projection()?;
    projection.provider_threads.clear();
    projection.provider_sessions.clear();
    projection.runs.clear();
    projection.attempts.clear();
    projection.nodes.clear();
    let mut background = item("background", "dynamic_tool");
    background["runId"] = json!(null);
    background["nodeId"] = json!(null);
    background["providerThreadId"] = json!(null);
    projection
        .turn_items
        .push(serde_json::from_value(background)?);
    seed(&sink(database.stores.clone()), &projection).await?;
    service(database.stores.clone()).recover().await?;
    assert_eq!(
        payload(
            &database.stores,
            "orchestration_v2_projection_turn_items",
            "turn_item_id",
            "background"
        )
        .await?["status"],
        "cancelled"
    );
    database.stores.read(|db|{
        let raw:String=db.query_row("SELECT metadata_json FROM orchestration_events WHERE event_type='turn-item.updated' ORDER BY sequence DESC LIMIT 1",[],|r|r.get(0))?;
        let metadata:serde_json::Value=serde_json::from_str(&raw)?;assert_eq!(metadata["providerInstanceId"],"codex");Ok(())
    }).await?;
    Ok(())
}
