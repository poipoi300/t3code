use t3_contracts::base_schemas::*;
use t3_orchestration::{
    error::Result,
    stores::positions::{Placement, allocate, normalize},
    testing::TestDatabase,
};
#[tokio::test]
async fn reuse_is_insert_free_even_if_item_moves_runs() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture.stores.transaction(|tx| {
        let thread=ThreadId("thread".into());let item=TurnItemId("item".into());
        assert_eq!(allocate(tx,&thread,&item,None,None,Placement::Append)?,1);
        tx.execute_batch("CREATE TEMP TRIGGER reject_redundant_position_insert BEFORE INSERT ON orchestration_v2_turn_item_positions WHEN EXISTS(SELECT 1 FROM orchestration_v2_turn_item_positions WHERE thread_id=NEW.thread_id AND turn_item_id=NEW.turn_item_id) BEGIN SELECT RAISE(ABORT,'Existing positions must be reused'); END;")?;
        assert_eq!(allocate(tx,&thread,&TurnItemId("next".into()),None,None,Placement::Append)?,2);
        assert_eq!(allocate(tx,&thread,&item,Some(&RunId("moved".into())),Some(2),Placement::Append)?,1);
        assert_eq!(allocate(tx,&ThreadId("other".into()),&item,None,None,Placement::Append)?,1);Ok(())
    }).await
}

#[tokio::test]
async fn normalization_reuses_positions_and_reads_the_persisted_run_ordinal() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture.stores.transaction(|tx| {
        tx.execute("INSERT INTO orchestration_v2_projection_runs(run_id,thread_id,ordinal,provider,status,requested_at,payload_json) VALUES('run','thread',3,'codex','running','2026-09-03T00:00:00.000Z','{}')", [])?;
        let input = serde_json::json!({"type":"reasoning","id":"item","threadId":"thread","runId":"run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":999,"status":"running","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-03T00:00:00.000Z","text":"Reasoning","streaming":true});
        let normalized = normalize(tx, serde_json::from_value(input.clone())?, None)?;
        let actual = serde_json::to_value(&normalized)?;
        let mut expected = input.clone();
        expected["ordinal"] = 3_000_001.into();
        assert_eq!(actual, expected);
        assert_eq!(normalize(tx, normalized.clone(), Some(8))?, normalized);
        let mut other = input;
        other["id"] = "other".into();
        assert_eq!(serde_json::to_value(normalize(tx, serde_json::from_value(other)?, Some(5))?)?["ordinal"], 5_000_001);
        Ok(())
    }).await
}
#[tokio::test]
async fn run_start_reserves_first_slot_then_appends_and_supports_many_items() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let thread = ThreadId("thread".into());
            let run = RunId("run".into());
            let allocate = |id: &str, placement| {
                allocate(
                    tx,
                    &thread,
                    &TurnItemId(id.into()),
                    Some(&run),
                    Some(2),
                    placement,
                )
            };
            assert_eq!(allocate("message", Placement::Append)?, 2_000_001);
            assert_eq!(allocate("recovery", Placement::RunStart)?, 2_000_000);
            assert_eq!(allocate("retry-recovery", Placement::RunStart)?, 2_000_002);
            for i in 0..150 {
                assert_eq!(
                    allocate(&format!("reply-{i}"), Placement::Append)?,
                    2_000_003 + i
                );
            }
            Ok(())
        })
        .await
}

/// The same allocations through the real TypeScript `TurnItemPositionStoreV2`
/// (`tests/fixtures/store-oracle.json`): returned ordinals and the position table.
#[tokio::test]
async fn allocations_match_the_typescript_position_store() -> Result<()> {
    use crate::persistence_orchestration_event_store_test::{assert_rows, snapshot, store_oracle};
    let fixture = TestDatabase::memory()?;
    fixture
        .stores
        .transaction(|tx| {
            let oracle = &store_oracle()["positions"];
            tx.execute_batch(oracle["setup"].as_str().expect("setup"))?;
            let mut ordinals = Vec::new();
            for op in oracle["ops"].as_array().expect("ops") {
                let text = |key: &str| op[key].as_str().map(str::to_owned);
                ordinals.push(allocate(
                    tx,
                    &ThreadId(text("threadId").expect("thread")),
                    &TurnItemId(text("turnItemId").expect("item")),
                    text("runId").map(RunId).as_ref(),
                    op["runOrdinal"].as_i64(),
                    if op["placement"] == "run_start" {
                        Placement::RunStart
                    } else {
                        Placement::Append
                    },
                )?);
            }
            assert_eq!(serde_json::json!(ordinals), oracle["ordinals"]);
            assert_rows(
                &snapshot(tx, "orchestration_v2_turn_item_positions")?,
                &oracle["rows"],
                "positions",
            );
            Ok(())
        })
        .await
}

/// Positions are keyed by real ids; a compact id-table index is refused rather than stored.
#[tokio::test]
async fn normalization_refuses_compact_ids() -> Result<()> {
    let fixture = TestDatabase::memory()?;
    fixture.stores.transaction(|tx| {
        let input = serde_json::json!({"type":"reasoning","id":7,"threadId":"thread","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"running","title":null,"startedAt":null,"completedAt":null,"updatedAt":"2026-09-03T00:00:00.000Z","text":"Reasoning","streaming":true});
        assert!(normalize(tx, serde_json::from_value(input)?, None).is_err());
        assert_eq!(tx.query_row("SELECT COUNT(*) FROM orchestration_v2_turn_item_positions", [], |row| row.get::<_, i64>(0))?, 0);
        Ok(())
    }).await
}
