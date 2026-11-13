use super::legacy_legacy_v1_thread_importer_test::{NOW, importer, seed_legacy};
use serde_json::{Value, json};
use t3_contracts::base_schemas::ThreadId;
use t3_orchestration::{
    api::ProjectionReader, error::Result, projections::store::ProjectionStore,
    stores::StoreDatabase, testing::TestDatabase,
};

#[test]
fn migration_49_cutover_is_refused_before_mutating_legacy_rows_or_the_ledger() -> Result<()> {
    use t3_persistence::rusqlite::{Connection, params};
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("statev2.sqlite");
    {
        let db = Connection::open(&path)?;
        db.execute_batch("CREATE TABLE effect_sql_migrations(migration_id INTEGER PRIMARY KEY,created_at TEXT NOT NULL,name TEXT NOT NULL); CREATE TABLE projection_thread_messages(message_id TEXT PRIMARY KEY,thread_id TEXT NOT NULL,text TEXT NOT NULL); INSERT INTO projection_thread_messages VALUES ('message:v1','thread:v1','Unmigrated transcript');")?;
        for (id, name) in t3_persistence::migrations::MANIFEST
            .iter()
            .filter(|(id, _)| *id <= 49)
        {
            db.execute(
                "INSERT INTO effect_sql_migrations VALUES (?1,?2,?3)",
                params![id, NOW, name],
            )?;
        }
    }
    assert!(matches!(
        t3_persistence::Database::open(&path),
        Err(t3_persistence::Error::UnsupportedHead {
            head: 49,
            minimum: 69
        })
    ));
    let db = Connection::open(&path)?;
    assert_eq!(
        db.query_row(
            "SELECT text FROM projection_thread_messages WHERE message_id='message:v1'",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "Unmigrated transcript"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM effect_sql_migrations", [], |row| row
            .get::<_, i64>(
            0
        ))?,
        49
    );
    assert_eq!(db.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name LIKE 'orchestration_v2_%'", [], |row| row.get::<_,i64>(0))?, 0);
    Ok(())
}

async fn original_messages(stores: &StoreDatabase) -> Result<Vec<String>> {
    stores.read(|db| {
        Ok(db.prepare("SELECT json_array(message_id,thread_id,turn_id,role,text,is_streaming,created_at,updated_at,attachments_json,context_json) FROM projection_thread_messages ORDER BY message_id")?
            .query_map([],|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
    }).await
}

#[tokio::test]
async fn cutover_keeps_shell_states_and_resumes_only_uncommitted_transcript_batches_after_reopen()
-> Result<()> {
    let database = TestDatabase::temporary()?;
    let directory = database.directory.expect("temporary database");
    let stores = database.stores;
    for name in [
        "active",
        "archived",
        "pinned",
        "snoozed",
        "deleted",
        "interrupted",
        "long",
    ] {
        seed_legacy(
            &stores,
            &format!("thread:{name}"),
            if name == "long" { 130 } else { 4 },
        )
        .await?;
    }
    stores.transaction(|tx| {
        tx.execute("UPDATE projection_threads SET archived_at=?1 WHERE thread_id='thread:archived'",[NOW])?;
        tx.execute("UPDATE projection_threads SET deleted_at=?1 WHERE thread_id='thread:deleted'",[NOW])?;
        tx.execute("UPDATE projection_threads SET pinned_at=NULL,pin_order_key=NULL WHERE thread_id!='thread:pinned'",[])?;
        tx.execute("UPDATE projection_threads SET snoozed_until=NULL,snoozed_at=NULL WHERE thread_id!='thread:snoozed'",[])?;
        tx.execute_batch("CREATE TRIGGER fail_transcript_batch BEFORE INSERT ON orchestration_events WHEN NEW.event_id='migration:v1:message:thread:long:message:0060' BEGIN SELECT RAISE(ABORT,'interrupted batch'); END;")?;
        Ok(())
    }).await?;
    let original = original_messages(&stores).await?;
    {
        let importer = importer(stores.clone());
        assert_eq!(importer.pending_thread_count().await?, 7);
        let imported = importer.reconcile_shells().await?;
        assert_eq!(imported.imported_thread_count, 7);
        assert_eq!(imported.imported_message_count, 14);
        let reader = ProjectionStore::new(stores.clone());
        for (name, marker) in [
            ("archived", "archivedAt"),
            ("deleted", "deletedAt"),
            ("pinned", "pinnedAt"),
            ("snoozed", "snoozedUntil"),
        ] {
            let projection = reader
                .projection(ThreadId(format!("thread:{name}")))
                .await?
                .expect("shell");
            let value = serde_json::to_value(projection)?;
            assert!(value["thread"][marker].is_string());
            assert_eq!(value["thread"]["historyOrigin"], "v1_import");
            assert!(value["thread"]["activeProviderThreadId"].is_null());
            assert_eq!(value["providerThreads"], json!([]));
            assert_eq!(value["messages"].as_array().expect("previews").len(), 2);
        }
        for name in [
            "active",
            "archived",
            "pinned",
            "snoozed",
            "deleted",
            "interrupted",
        ] {
            importer
                .ensure_transcript(ThreadId(format!("thread:{name}")))
                .await?;
        }
        assert!(
            importer
                .ensure_transcript(ThreadId("thread:long".into()))
                .await
                .is_err()
        );
        assert_eq!(importer.pending_thread_count().await?, 1);
        stores.read(|db| {
            assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_events WHERE stream_id='thread:long'",[],|r|r.get::<_,i64>(0))?,106);
            assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_v2_turn_item_positions WHERE thread_id='thread:long'",[],|r|r.get::<_,i64>(0))?,52);
            Ok(())
        }).await?;
    }
    drop(stores);
    let reopened = StoreDatabase(t3_persistence::Database::open(
        directory.path().join("statev2.sqlite"),
    )?);
    reopened
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER fail_transcript_batch;")?;
            Ok(())
        })
        .await?;
    let importer = importer(reopened.clone());
    assert_eq!(importer.pending_thread_count().await?, 1);
    assert_eq!(importer.reconcile_shells().await?.imported_thread_count, 0);
    let hydrated = importer.import_pending_transcripts().await?;
    assert_eq!(hydrated.imported_thread_count, 1);
    assert_eq!(hydrated.imported_message_count, 78);
    assert_eq!(importer.pending_thread_count().await?, 0);
    assert_eq!(original_messages(&reopened).await?, original);
    let projection = ProjectionStore::new(reopened.clone())
        .projection(ThreadId("thread:long".into()))
        .await?
        .expect("long transcript");
    assert_eq!(projection.messages.len(), 130);
    let items = serde_json::to_value(&projection.turn_items)?;
    for (index, item) in items.as_array().expect("items").iter().enumerate() {
        assert_eq!(item["ordinal"], index + 1);
    }
    let last: Value = serde_json::to_value(projection.messages.last().expect("last message"))?;
    assert_eq!(last["text"], "Message 130");
    assert_eq!(last["streaming"], false);
    reopened.read(|db| {
        assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_events WHERE stream_id='thread:long'",[],|r|r.get::<_,i64>(0))?,262);
        assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_v2_turn_item_positions WHERE thread_id='thread:long'",[],|r|r.get::<_,i64>(0))?,130);
        Ok(())
    }).await?;
    assert_eq!(
        importer
            .import_pending_transcripts()
            .await?
            .imported_thread_count,
        0
    );
    Ok(())
}
