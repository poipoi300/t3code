use super::provider_event_ingestor_test::{Harness, item};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use t3_orchestration::{
    api::*, error::Result, ports::runs_ingestion::projection_maintenance::*, stores::StoreDatabase,
};
use t3_persistence::rusqlite::params;
async fn revision(h: &Harness, index: u64) -> Result<i64> {
    let mut value = item("assistant_message", "reply");
    value["messageId"] = "message:reply".into();
    value["text"] = format!("revision {index}").into();
    value["attachments"] = serde_json::json!([]);
    value["streaming"] = true.into();
    Ok(h.write("turn-item.updated", value).await?[0].sequence as i64)
}
#[tokio::test]
async fn pruning_waits_a_full_day_keeps_projection_heads_and_command_receipts() -> Result<()> {
    let h = Harness::new().await?;
    for index in 0..120 {
        revision(&h, index).await?;
    }
    let service = ProjectionMaintenance {
        database: h.database.stores.clone(),
        clock: h.clock.clone(),
    };
    assert_eq!(
        service.prune_event_store(None).await?.deleted_event_count,
        0
    );
    h.clock.set("2026-09-03T23:59:59.999Z");
    assert_eq!(
        service.prune_event_store(None).await?.deleted_event_count,
        0
    );
    h.clock.set("2026-09-04T00:00:00.000Z");
    let result = service.prune_event_store(None).await?;
    assert_eq!(result.deleted_event_count, 119);
    let remaining = h
        .database
        .stores
        .read(|db| {
            Ok(
                db.query_row("SELECT COUNT(*) FROM orchestration_events", [], |row| {
                    row.get::<_, i64>(0)
                })?,
            )
        })
        .await?;
    assert_eq!(remaining, 2);
    let projection = h
        .reader
        .projection(ThreadId("thread:test".into()))
        .await?
        .unwrap();
    assert_eq!(projection.turn_items.len(), 1);
    assert_eq!(
        t3_contracts::stored::to_value(&projection.turn_items[0])?["text"],
        "revision 119"
    );
    assert_eq!(result.deleted_receipt_count, 0);
    assert_eq!(
        service.prune_event_store(None).await?.deleted_event_count,
        0
    );
    Ok(())
}
async fn point_head_at(h: &Harness, anchor: i64) -> Result<()> {
    h.database.stores.transaction(move|db|{db.execute("UPDATE orchestration_events SET payload_json=json_set(payload_json,'$.promptSequence',?1) WHERE sequence=(SELECT MAX(sequence) FROM orchestration_events)",[anchor])?;Ok(())}).await
}
#[tokio::test]
async fn retained_stream_head_keeps_the_prompt_anchor_it_borrows() -> Result<()> {
    let h = Harness::new().await?;
    let anchor = revision(&h, 0).await?;
    revision(&h, 1).await?;
    revision(&h, 2).await?;
    point_head_at(&h, anchor).await?;
    let service = ProjectionMaintenance {
        database: h.database.stores.clone(),
        clock: h.clock.clone(),
    };
    service.prune_event_store(None).await?;
    h.clock.set("2026-09-04T00:00:00.000Z");
    let result = service.prune_event_store(None).await?;
    assert_eq!(result.deleted_event_count, 1);
    assert_eq!(
        h.database
            .stores
            .read(move |db| Ok(db.query_row(
                "SELECT COUNT(*) FROM orchestration_events WHERE sequence=?1",
                [anchor],
                |row| row.get::<_, i64>(0)
            )?))
            .await?,
        1
    );
    let prompt = "Review prompt A. ".repeat(40);
    let mut task = super::provider_event_ingestor_test::fixture("OrchestrationV2Subagent");
    task["id"] = "task:prompt".into();
    task["threadId"] = "thread:test".into();
    task["origin"] = "provider_native".into();
    task["prompt"] = prompt.clone().into();
    task["updatedAt"] = serde_json::json!(h.clock.now());
    for revision in 0..3 {
        task["progress"] = format!("Review progress {revision}").into();
        h.write("subagent.updated", task.clone()).await?;
    }
    service.prune_event_store(None).await?;
    h.clock.set("2026-09-05T00:00:00.000Z");
    service.prune_event_store(None).await?;
    let rows = h
        .database
        .stores
        .read(|db| {
            t3_orchestration::stores::event_store::EventStore::new().read(
                db,
                &t3_orchestration::stores::event_store::EventFilter {
                    thread_id: Some(ThreadId("thread:test".into())),
                    event_type: Some("subagent.updated".into()),
                    ..Default::default()
                },
            )
        })
        .await?;
    assert!(!rows.is_empty());
    for row in rows {
        let OrchestrationV2DomainEvent::SubagentUpdated(event) = row.event else {
            panic!("subagent event")
        };
        assert_eq!(event.payload.prompt, prompt);
    }
    Ok(())
}
struct AppendReference {
    database: StoreDatabase,
    anchor: i64,
    inserted: AtomicBool,
}
#[async_trait]
impl PrunePageObserver for AppendReference {
    async fn between_pages(&self) -> Result<()> {
        if self.inserted.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let anchor = self.anchor;
        self.database.transaction(move|db|{db.execute("INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,actor_kind,payload_json,metadata_json,application_event_version) SELECT 'event:appended','thread','thread:test',MAX(stream_version)+1,'turn-item.updated','2026-09-09T00:00:00.000Z','server',json_object('promptSequence',?1),'{}',2 FROM orchestration_events WHERE aggregate_kind='thread' AND stream_id='thread:test'",[anchor])?;Ok(())}).await
    }
}
#[tokio::test]
async fn references_appended_after_the_scan_are_rechecked_inside_the_delete_transaction()
-> Result<()> {
    let h = Harness::new().await?;
    let anchor = revision(&h, 0).await?;
    for index in 1..75 {
        revision(&h, index).await?;
    }
    let service = ProjectionMaintenance {
        database: h.database.stores.clone(),
        clock: h.clock.clone(),
    };
    service.prune_event_store(None).await?;
    h.clock.set("2026-09-04T00:00:00.000Z");
    let observer = AppendReference {
        database: h.database.stores.clone(),
        anchor,
        inserted: AtomicBool::new(false),
    };
    let result = service.prune_event_store(Some(&observer)).await?;
    assert_eq!(result.deleted_event_count, 74);
    assert!(observer.inserted.load(Ordering::SeqCst));
    let kept = h
        .database
        .stores
        .read(move |db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM orchestration_events WHERE sequence=?1",
                params![anchor],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await?;
    assert_eq!(kept, 1);
    let boundary=h.database.stores.read(|db|Ok(db.query_row("SELECT last_sequence FROM orchestration_v2_projection_metadata WHERE projection_name='event-log-pruned-through'",[],|row|row.get::<_,i64>(0))?)).await?;
    assert!(boundary > anchor);
    Ok(())
}
