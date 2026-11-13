use super::provider_runtime_recovery_service_test::sink;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::base_schemas::ThreadId;
use t3_orchestration::{
    api::{CommitGuard, CommitRequest, Decision, EventCommitter, ProjectionReader},
    error::{Error, Result},
    ports::sessions_recovery::legacy::legacy_v1_thread_importer::{
        LegacyV1ImportSummary, LegacyV1ThreadImporter,
    },
    projections::store::ProjectionStore,
    runtime::commit::EventSink,
    stores::StoreDatabase,
    testing::{FakeEventCommitter, ManualClock, SequenceIds, TestDatabase},
};
use t3_persistence::rusqlite::params;

pub(super) const NOW: &str = "2026-07-13T15:00:00.000Z";

pub(super) fn importer(stores: StoreDatabase) -> LegacyV1ThreadImporter {
    LegacyV1ThreadImporter::new(
        stores.clone(),
        Arc::new(sink(stores)),
        Arc::new(ManualClock::new(NOW)),
        Arc::new(SequenceIds::new("legacy")),
    )
}

pub(super) async fn seed_legacy(stores: &StoreDatabase, id: &str, count: usize) -> Result<()> {
    let id = id.to_owned();
    stores.transaction(move |tx| {
        tx.execute("INSERT OR IGNORE INTO projection_projects(project_id,title,workspace_root,scripts_json,created_at,updated_at) VALUES ('project:legacy','Legacy project','/test/legacy','[]',?1,?1)", [NOW])?;
        tx.execute("INSERT INTO projection_threads(thread_id,project_id,title,model_selection_json,runtime_mode,interaction_mode,branch,worktree_path,created_at,updated_at,unsettled_at,snoozed_until,snoozed_at,pinned_at,pin_order_key,linked_pull_request_json,branch_pull_request_json,active_order_key) VALUES (?1,'project:legacy','Migrated conversation','{\"instanceId\":\"codex\",\"model\":\"gpt-5.4\"}','full-access','default',' main ',' /test/legacy ','2026-01-01T00:00:00.000Z','2026-01-04T00:00:00.000Z','2026-01-03T12:00:00.000Z','2026-02-01T00:00:00.000Z','2026-01-04T00:00:00.000Z','2026-01-02T00:00:00.000Z','m','{\"projectId\":\"project:legacy\",\"repository\":\"pingdotgg/t3code\",\"number\":9000,\"url\":\"https://github.com/pingdotgg/t3code/pull/9000\"}','{\"projectId\":\"project:legacy\",\"repository\":\"pingdotgg/t3code\",\"number\":9001,\"url\":\"https://github.com/pingdotgg/t3code/pull/9001\"}','az')", [&id])?;
        for index in 1..=count {
            let date = format!("2026-01-{:02}T01:00:00.000Z", index.min(28));
            let context = match index {
                2 => "{broken-json",
                3 => "{\"version\":2,\"records\":[]}",
                _ => "{\"version\":1,\"records\":[{\"version\":1,\"contextId\":\"ctx_1\",\"kind\":\"skill\",\"label\":\"$test\",\"name\":\"test\"}]}",
            };
            tx.execute("INSERT INTO projection_thread_messages(message_id,thread_id,role,text,attachments_json,context_json,is_streaming,created_at,updated_at) VALUES (?1,?2,?3,?4,'[]',?5,?6,?7,?7)", params![format!("{id}:message:{index:04}"), id, if index % 2 == 1 { "user" } else { "assistant" }, format!("Message {index}"), context, i64::from(index == count), date])?;
        }
        Ok(())
    }).await
}

async fn stored(stores: &StoreDatabase, id: &str) -> Result<Value> {
    let projection = ProjectionStore::new(stores.clone())
        .projection(ThreadId(id.into()))
        .await?
        .expect("imported thread");
    Ok(serde_json::to_value(projection)?)
}

async fn write_thread(sink: &EventSink, thread: Value, id: &str) -> Result<()> {
    sink.commit(CommitRequest { receipt: None, guard: CommitGuard::None, decision: Decision {
        events: vec![serde_json::from_value(json!({"id":id,"threadId":thread["id"],"type":"thread.metadata-updated","occurredAt":NOW,"payload":thread}))?], ..Default::default()
    }}).await?;
    Ok(())
}

#[tokio::test]
async fn imports_lightweight_shell_hydrates_exact_transcript_and_preserves_v2_changes() -> Result<()>
{
    let database = TestDatabase::temporary()?;
    seed_legacy(&database.stores, "thread:legacy", 4).await?;
    database.stores.transaction(|tx| {
        for number in [9002,9003] {
            tx.execute("INSERT INTO projection_thread_pull_requests(thread_id,host,repository,number,url,source,linked_at,snapshot_json) VALUES ('thread:legacy','github.com','pingdotgg/t3code',?1,?2,'created',?3,?4)", params![number, format!("https://github.com/pingdotgg/t3code/pull/{number}"), NOW, (number == 9002).then_some("{\"state\":\"open\",\"title\":\"Second PR\",\"headBranch\":\"feature-two\",\"baseBranch\":\"main\",\"isDraft\":false,\"updatedAt\":\"2026-01-03T00:00:00.000Z\",\"syncedAt\":\"2026-01-03T00:00:00.000Z\"}")])?;
        }
        Ok(())
    }).await?;
    let importer = importer(database.stores.clone());
    assert_eq!(importer.pending_thread_count().await?, 1);
    assert_eq!(
        importer.reconcile_shells().await?,
        LegacyV1ImportSummary {
            imported_thread_count: 1,
            imported_message_count: 2
        }
    );
    assert_eq!(importer.pending_thread_count().await?, 1);
    let shell = stored(&database.stores, "thread:legacy").await?;
    assert_eq!(
        shell["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|m| m["text"].as_str().expect("text"))
            .collect::<Vec<_>>(),
        ["Message 3", "Message 4"]
    );
    assert!(shell["messages"][0].get("context").is_none());
    assert_eq!(
        shell["messages"][1]["context"]["records"][0]["name"],
        "test"
    );
    let mut thread = shell["thread"].clone();
    assert_eq!(thread["historyOrigin"], "v1_import");
    assert_eq!(thread["createdBy"], "system");
    assert_eq!(thread["creationSource"], "server");
    assert_eq!(thread["branch"], "main");
    assert_eq!(thread["worktreePath"], "/test/legacy");
    assert_eq!(thread["pinnedAt"], "2026-01-02T00:00:00.000Z");
    assert_eq!(thread["pinOrderKey"], "m");
    assert_eq!(thread["unsettledAt"], "2026-01-03T12:00:00.000Z");
    assert_eq!(thread["snoozedUntil"], "2026-02-01T00:00:00.000Z");
    assert_eq!(
        thread["pullRequests"]
            .as_array()
            .expect("links")
            .iter()
            .map(|p| p["number"].as_u64().expect("number"))
            .collect::<Vec<_>>(),
        [9002, 9003, 9000]
    );
    assert_eq!(thread["pullRequests"][0]["snapshot"]["title"], "Second PR");
    database.stores.read(|db| {
        assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_events WHERE application_event_version=2 AND stream_id='thread:legacy'", [], |r|r.get::<_,i64>(0))?, 6);
        Ok(())
    }).await?;
    thread["title"] = json!("Renamed after shell import");
    thread["runtimeMode"] = json!("approval-required");
    thread["interactionMode"] = json!("plan");
    thread["archivedAt"] = json!(NOW);
    thread["settledOverride"] = json!("settled");
    thread["settledAt"] = json!(NOW);
    write_thread(
        &sink(database.stores.clone()),
        thread.clone(),
        "v2:metadata",
    )
    .await?;
    assert_eq!(
        importer.import_pending_transcripts().await?,
        LegacyV1ImportSummary {
            imported_thread_count: 1,
            imported_message_count: 2
        }
    );
    let projection = stored(&database.stores, "thread:legacy").await?;
    for field in [
        "title",
        "runtimeMode",
        "interactionMode",
        "archivedAt",
        "settledOverride",
        "settledAt",
        "lastVisitedAt",
        "pinnedAt",
        "pinOrderKey",
        "pullRequests",
    ] {
        assert_eq!(projection["thread"][field], thread[field]);
    }
    for (index, message) in projection["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .enumerate()
    {
        assert_eq!(message["text"], format!("Message {}", index + 1));
        assert_eq!(message["streaming"], false);
        assert_eq!(message.get("context").is_some(), matches!(index, 0 | 3));
    }
    for (index, item) in projection["turnItems"]
        .as_array()
        .expect("items")
        .iter()
        .enumerate()
    {
        assert_eq!(item["ordinal"], index + 1);
        assert_eq!(
            item["status"],
            if index == 3 {
                "interrupted"
            } else {
                "completed"
            }
        );
    }
    assert_eq!(importer.pending_thread_count().await?, 0);
    assert_eq!(
        importer
            .ensure_transcript(ThreadId("thread:legacy".into()))
            .await?,
        LegacyV1ImportSummary::default()
    );
    assert_eq!(
        importer.reconcile_shells().await?,
        LegacyV1ImportSummary::default()
    );
    database.stores.read(|db| {
        assert_eq!(db.query_row("SELECT COUNT(*) FROM projection_thread_messages WHERE thread_id='thread:legacy' AND is_streaming=1",[],|r|r.get::<_,i64>(0))?,1);
        assert_eq!(db.query_row("SELECT context_json FROM projection_thread_messages WHERE message_id='thread:legacy:message:0002'",[],|r|r.get::<_,String>(0))?,"{broken-json");
        assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_events WHERE stream_id='thread:legacy'",[],|r|r.get::<_,i64>(0))?,11);
        Ok(())
    }).await?;
    Ok(())
}

#[tokio::test]
async fn repeated_metadata_repairs_preserve_explicit_null_and_use_new_event_ids() -> Result<()> {
    let database = TestDatabase::memory()?;
    seed_legacy(&database.stores, "thread:repair", 0).await?;
    let importer = importer(database.stores.clone());
    importer.reconcile_shells().await?;
    let mut thread = stored(&database.stores, "thread:repair").await?["thread"].clone();
    thread["title"] = json!("Renamed in v2");
    for key in ["pinnedAt", "pinOrderKey", "linkedPullRequest"] {
        thread[key] = Value::Null;
    }
    thread["pullRequests"] = json!([]);
    write_thread(
        &sink(database.stores.clone()),
        thread,
        "migration:v1:thread:thread:repair:metadata-repair",
    )
    .await?;
    for field in ["branchPullRequest", "activeOrderKey"] {
        database.stores.transaction(move|tx| {
            tx.execute("UPDATE orchestration_v2_projection_threads SET payload_json=json_remove(payload_json,?1) WHERE thread_id='thread:repair'",[format!("$.{field}")])?; Ok(())
        }).await?;
        assert_eq!(importer.reconcile_shells().await?.imported_thread_count, 1);
    }
    let repaired = stored(&database.stores, "thread:repair").await?;
    assert_eq!(repaired["thread"]["title"], "Renamed in v2");
    assert_eq!(repaired["thread"]["branchPullRequest"]["number"], 9001);
    assert_eq!(repaired["thread"]["activeOrderKey"], "az");
    for key in ["pinnedAt", "pinOrderKey", "linkedPullRequest"] {
        assert!(repaired["thread"][key].is_null());
    }
    assert_eq!(repaired["thread"]["pullRequests"], json!([]));
    assert_eq!(
        importer.reconcile_shells().await?,
        LegacyV1ImportSummary::default()
    );
    database.stores.read(|db| {
        assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_events WHERE event_id LIKE '%metadata-repair%'",[],|r|r.get::<_,i64>(0))?,3);
        Ok(())
    }).await?;
    Ok(())
}

#[tokio::test]
async fn malformed_timestamps_fallback_without_clearing_archive_and_invalid_fields_default()
-> Result<()> {
    let database = TestDatabase::memory()?;
    seed_legacy(&database.stores, "thread:malformed", 1).await?;
    database.stores.transaction(|tx| {
        tx.execute("UPDATE projection_threads SET title='  ',model_selection_json='{broken',runtime_mode='invalid',interaction_mode='invalid',updated_at='2026-07-13T14:13:00.3NZ',archived_at='2026-07-13T14:13:00.3NZ'",[])?;
        tx.execute("UPDATE projection_thread_messages SET created_at='bad',updated_at='2026-07-13T10:13:00-04:00',attachments_json='invalid'",[])?;
        Ok(())
    }).await?;
    importer(database.stores.clone()).reconcile_shells().await?;
    let projection = stored(&database.stores, "thread:malformed").await?;
    assert_eq!(
        projection["thread"]["archivedAt"],
        "2026-01-01T00:00:00.000Z"
    );
    assert_eq!(projection["thread"]["title"], "Untitled thread");
    assert_eq!(
        projection["thread"]["modelSelection"]["model"],
        t3_contracts::model::DEFAULT_MODEL
    );
    assert_eq!(projection["thread"]["runtimeMode"], "full-access");
    assert_eq!(projection["thread"]["interactionMode"], "default");
    assert_eq!(
        projection["messages"][0]["createdAt"],
        "2026-07-13T14:13:00.000Z"
    );
    assert_eq!(projection["messages"][0]["attachments"], json!([]));
    Ok(())
}

#[tokio::test]
async fn shell_and_transcript_failures_rollback_positions_markers_and_partial_events() -> Result<()>
{
    let database = TestDatabase::memory()?;
    seed_legacy(&database.stores, "thread:first", 3).await?;
    seed_legacy(&database.stores, "thread:later", 3).await?;
    database.stores.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER reject_legacy BEFORE INSERT ON orchestration_v2_projection_messages WHEN NEW.message_id='thread:first:message:0003' BEGIN SELECT RAISE(FAIL,'blocked shell'); END;")?; Ok(())
    }).await?;
    let importer = importer(database.stores.clone());
    assert!(importer.reconcile_shells().await.is_err());
    database
        .stores
        .read(|db| {
            for table in [
                "orchestration_events",
                "orchestration_v2_legacy_imports",
                "orchestration_v2_turn_item_positions",
                "orchestration_v2_projection_threads",
            ] {
                assert_eq!(
                    db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
            }
            Ok(())
        })
        .await?;
    database
        .stores
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER reject_legacy;")?;
            Ok(())
        })
        .await?;
    importer.reconcile_shells().await?;
    database.stores.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER reject_legacy BEFORE INSERT ON orchestration_v2_projection_messages WHEN NEW.message_id='thread:first:message:0001' BEGIN SELECT RAISE(FAIL,'blocked transcript'); END;")?; Ok(())
    }).await?;
    assert_eq!(
        importer.import_pending_transcripts().await?,
        LegacyV1ImportSummary {
            imported_thread_count: 1,
            imported_message_count: 2
        }
    );
    database.stores.read(|db| {
        assert_eq!(db.query_row("SELECT last_error FROM orchestration_v2_legacy_imports WHERE thread_id='thread:first'",[],|r|r.get::<_,String>(0))?,"Transcript hydration failed; retry on next open.");
        assert_eq!(db.query_row("SELECT COUNT(*) FROM orchestration_v2_turn_item_positions WHERE turn_item_id='migration:v1:turn-item:thread:first:message:0001'",[],|r|r.get::<_,i64>(0))?,0);
        Ok(())
    }).await?;
    database
        .stores
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER reject_legacy;")?;
            Ok(())
        })
        .await?;
    assert_eq!(
        importer.import_pending_transcripts().await?,
        LegacyV1ImportSummary {
            imported_thread_count: 1,
            imported_message_count: 2
        }
    );
    assert_eq!(importer.pending_thread_count().await?, 0);
    Ok(())
}

#[tokio::test]
async fn cancellation_propagates_and_absent_or_confirmed_transcripts_are_cached() -> Result<()> {
    let database = TestDatabase::memory()?;
    seed_legacy(&database.stores, "thread:cancel", 3).await?;
    importer(database.stores.clone()).reconcile_shells().await?;
    let sink = Arc::new(FakeEventCommitter::new(database.stores.clone()));
    sink.commit.push(Err(Error::Cancelled));
    let importer = LegacyV1ThreadImporter::new(
        database.stores.clone(),
        sink,
        Arc::new(ManualClock::new(NOW)),
        Arc::new(SequenceIds::new("cancel")),
    );
    assert!(matches!(
        importer.import_pending_transcripts().await,
        Err(Error::Cancelled)
    ));
    database.stores.read(|db| {
        assert!(db.query_row("SELECT last_error FROM orchestration_v2_legacy_imports WHERE thread_id='thread:cancel'",[],|r|r.get::<_,Option<String>>(0))?.is_none());
        Ok(())
    }).await?;
    let native = ThreadId("native:v2".into());
    assert_eq!(
        importer.ensure_transcript(native.clone()).await?,
        LegacyV1ImportSummary::default()
    );
    database
        .stores
        .transaction(|tx| {
            tx.execute_batch(
                "ALTER TABLE orchestration_v2_legacy_imports RENAME TO hidden_imports;",
            )?;
            Ok(())
        })
        .await?;
    assert_eq!(
        importer.ensure_transcript(native).await?,
        LegacyV1ImportSummary::default()
    );
    Ok(())
}
