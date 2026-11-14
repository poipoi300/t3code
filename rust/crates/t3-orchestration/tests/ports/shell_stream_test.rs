use super::{
    bounded_snapshot_transport_test::{seed, service},
    thread_stream_test::projection_value,
};
use serde_json::json;
use t3_contracts::{
    application_event::ApplicationStoredEvent, base_schemas::ThreadId, orchestration_v2::*,
};
use t3_orchestration::{
    api::EventCommitter,
    ports::transport::{live_stream_budget::*, shell_stream::*},
    testing::{TEST_TIME, TestDatabase, detached_event},
};
pub fn shell(id: &str, parent: Option<&str>) -> OrchestrationV2ThreadShell {
    let mut value = projection_value(0)["thread"].clone();
    value["id"] = id.into();
    value["lineage"] = json!({"parentThreadId":parent,"relationshipToParent":parent.map(|_|"subagent"),"rootThreadId":"root"});
    for (key, value2) in [
        ("activeRunId", json!(null)),
        ("latestRunId", json!(null)),
        ("latestUserMessageAt", json!(null)),
        ("latestVisibleMessage", json!(null)),
        ("pendingRuntimeRequest", json!(null)),
        ("hasActionableProposedPlan", json!(false)),
        ("itemCount", json!(1)),
        ("visibleItemCount", json!(1)),
        ("status", json!("completed")),
    ] {
        value[key] = value2;
    }
    serde_json::from_value(value).expect("test value")
}
fn snapshot() -> OrchestrationV2ShellSnapshot {
    OrchestrationV2ShellSnapshot {
        schema_version: 1,
        snapshot_sequence: 5,
        threads: vec![],
        archived_threads: vec![],
        projects: vec![],
        older_settled: None,
    }
}

#[test]
fn identity_digests_match_typescript_sha256_and_preserve_project_order() {
    assert_eq!(
        project_identities_digest(&[]).unwrap(),
        "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU"
    );
    let project = |id: &str| {
        serde_json::from_value(json!({"id":id,"title":id,"workspaceRoot":format!("/{id}"),"repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":TEST_TIME,"updatedAt":TEST_TIME})).unwrap()
    };
    let projects = vec![project("a"), project("b")];
    let digest = project_identities_digest(&projects).unwrap();
    assert_eq!(digest, "rWkUpMToHkv4IXezTtKFu5GvSmsqGDviJtld3x1LMcI");
    assert_eq!(
        project_identities_digest(&[project("b"), project("a")]).unwrap(),
        "5rFhxJ1S4UxPbTfi97odiQpxvtuJlefd9BCdUTDJ8dY"
    );
    assert_eq!(
        resume_identity_roots(&projects, vec!["/a".into(), "/b".into()], Some(&digest)).unwrap(),
        vec!["/a", "/b"]
    );
}

#[tokio::test(start_paused = true)]
async fn subscription_orders_snapshot_completion_replay_and_live_then_cancels_on_drop() {
    use super::bounded_snapshot_transport_test::commit;
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    let base = seed(&fixture, &sink).await.unwrap();
    let input = serde_json::from_value(json!({"requestCompletionMarker":true})).unwrap();
    let receivers = sink.subscribe_shared().unwrap();
    let count = receivers.len();
    assert_eq!(count, 0);
    let mut stream = service
        .subscribe_shell(input, Default::default())
        .await
        .unwrap();
    let first = stream.pull().await.unwrap().unwrap();
    let OrchestrationV2ShellStreamItem::Snapshot(frame) = &first[0].value else {
        panic!()
    };
    assert_eq!(frame.snapshot.snapshot_sequence, base);
    assert_eq!(frame.snapshot.threads.len(), 1);
    assert!(matches!(
        stream.pull().await.unwrap().unwrap()[0].value,
        OrchestrationV2ShellStreamItem::Synchronized(_)
    ));
    let sequence = commit(&sink, vec![detached_event("shell-live", "thread").unwrap()])
        .await
        .unwrap();
    assert_eq!(stream.pull().await.unwrap().unwrap()[0].sequence, sequence);
    stream.close().await.unwrap();
    let mut resumed = service
        .subscribe_shell(
            serde_json::from_value(json!({"afterSequence":base,"requestCompletionMarker":true}))
                .unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    let replay = resumed.pull().await.unwrap().unwrap();
    assert!(matches!(
        replay[0].value,
        OrchestrationV2ShellStreamItem::ThreadUpdated(_)
    ));
    assert_eq!(
        serde_json::to_value(&replay[0].value).unwrap()["sequence"],
        sequence
    );
    assert!(matches!(
        resumed.pull().await.unwrap().unwrap()[0].value,
        OrchestrationV2ShellStreamItem::Synchronized(_)
    ));
    resumed.close().await.unwrap();
}

#[tokio::test]
async fn foreign_ahead_and_pruned_cursors_require_http_snapshot_and_end() {
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    let head = seed(&fixture, &sink).await.unwrap();
    for after in [head + 1, head + 1001] {
        let mut stream = service.subscribe_shell(serde_json::from_value(json!({"afterSequence":after,"httpSnapshotFallback":true,"requestCompletionMarker":true})).unwrap(),Default::default()).await.unwrap();
        assert!(matches!(
            stream.pull().await.unwrap().unwrap()[0].value,
            OrchestrationV2ShellStreamItem::SnapshotRequired(_)
        ));
        assert!(stream.pull().await.unwrap().is_none());
        stream.close().await.unwrap();
    }
    fixture.stores.transaction(move |tx| {tx.execute("INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES('event-log-pruned-through',2,?1,?2)",(head as i64,TEST_TIME))?;Ok(())}).await.unwrap();
    let mut stream = service
        .subscribe_shell(
            serde_json::from_value(json!({"afterSequence":0,"httpSnapshotFallback":true})).unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        stream.pull().await.unwrap().unwrap()[0].value,
        OrchestrationV2ShellStreamItem::SnapshotRequired(_)
    ));
    stream.close().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shell_subscriber_charges_projected_bytes_and_rejects_oversized_live_rows() {
    use super::bounded_snapshot_transport_test::commit;
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    seed(&fixture, &sink).await.unwrap();
    let mut stream = service
        .subscribe_shell(
            serde_json::from_value(json!({})).unwrap(),
            LiveStreamLimits {
                max_items: 100,
                max_serialized_bytes: 200,
            },
        )
        .await
        .unwrap();
    stream.pull().await.unwrap();
    commit(
        &sink,
        vec![detached_event("oversized-shell", "thread").unwrap()],
    )
    .await
    .unwrap();
    assert!(
        stream
            .pull()
            .await
            .unwrap_err()
            .to_string()
            .contains("buffer is full")
    );
    stream.close().await.unwrap();
}

#[test]
fn subscriber_index_keeps_settled_ties_pins_snoozes_and_hidden_child_ancestors() {
    use t3_orchestration::api::ActiveShellIndexEntry;
    let rows: Vec<_> = (0..30)
        .map(|i| ActiveShellIndexEntry {
            id: format!("t{i}").into(),
            project_id: "project".to_owned().into(),
            lineage: shell("root", None).lineage,
            settled_override: Some(AppThreadSettledOverride::Settled),
            settled_at: Some(t3_contracts::wire::WireDateTime(format!(
                "2026-10-{:02}T00:00:00.000Z",
                i + 1
            ))),
            pinned_at: None,
            snoozed_until: None,
            may_be_live: false,
            has_remote_tasks: false,
        })
        .collect();
    let options = ShellSubscriberOptions {
        omit_idle_subagents: true,
        omit_older_settled: true,
    };
    assert_eq!(shell_subscriber_thread_ids(&rows, options).len(), 25);
    let mut rows = rows;
    rows[0].pinned_at = Some(Some(t3_contracts::wire::WireDateTime(TEST_TIME.into())));
    rows[1].snoozed_until = Some(Some(t3_contracts::wire::WireDateTime(TEST_TIME.into())));
    rows[2].has_remote_tasks = true;
    rows[3].may_be_live = true;
    rows[29].lineage.side_chat = Some(Some(true));
    rows[29].lineage.parent_thread_id = Some("t4".to_owned().into());
    rows[29].lineage.root_thread_id = "t4".to_owned().into();
    assert_eq!(shell_subscriber_thread_ids(&rows, options).len(), 30);
}
fn removed(sequence: u64, id: &str, archive: bool) -> OrchestrationV2ShellStreamItem {
    OrchestrationV2ShellStreamItem::ThreadRemoved(Box::new(ShellStreamItemThreadRemoved {
        sequence,
        location: if archive {
            ShellStreamItemThreadUpdatedLocation::Archive
        } else {
            ShellStreamItemThreadUpdatedLocation::Active
        },
        thread_id: ThreadId(id.into()),
    }))
}
#[test]
fn omitted_children_remain_known_and_nested_live_ancestors_are_kept() {
    let mut running = shell("running", Some("root"));
    running.activity_run_status = Some(Some(ThreadShellActivityRunStatus::Running));
    let mut queued = shell("queued", Some("root"));
    queued.status = OrchestrationV2ShellThreadStatus::Queued;
    let input = vec![
        shell("root", None),
        shell("idle", Some("root")),
        running,
        queued,
        shell("middle", Some("root")),
        shell("leaf", Some("middle")),
        shell("remote", Some("root")),
    ];
    let remote = [ThreadId("remote".into())].into_iter().collect();
    let (kept, ids) = active_shell_rows_for_subscriber(input, true, &remote);
    assert_eq!(
        kept.iter().map(|s| s.id.0.as_str()).collect::<Vec<_>>(),
        vec!["root", "running", "queued", "middle", "remote"]
    );
    assert!(ids.contains(&ThreadId("idle".into())));
    assert!(ids.contains(&ThreadId("leaf".into())));
}
#[test]
fn shell_event_retention_removes_transcript_bodies_and_keeps_subagent_state_changes() {
    let mut ids = [ThreadId("child".into())].into_iter().collect();
    for thread in ["parent", "child"] {
        let stored=ApplicationStoredEvent::Orchestration(Box::new(OrchestrationV2StoredEvent{sequence:1,command_id:None,event:serde_json::from_value(json!({"id":"turn-item","threadId":thread,"type":"turn-item.updated","occurredAt":TEST_TIME,"payload":super::thread_stream_test::command("command",1)})).unwrap()}));
        assert_eq!(
            should_publish_shell_application_event(&stored, &mut ids),
            thread == "parent"
        );
        let projected = serde_json::to_string(&to_shell_application_event(&stored)).unwrap();
        assert!(!projected.contains("private output"));
        assert!(projected.len() < 100);
    }
    let mut thread = projection_value(0)["thread"].clone();
    thread["lineage"]["relationshipToParent"] = "subagent".into();
    let created=ApplicationStoredEvent::Orchestration(Box::new(OrchestrationV2StoredEvent{sequence:2,command_id:None,event:serde_json::from_value(json!({"id":"new-child","threadId":"new-child","type":"thread.created","occurredAt":TEST_TIME,"payload":thread})).unwrap()}));
    assert!(should_publish_shell_application_event(&created, &mut ids));
    assert!(ids.contains(&ThreadId("new-child".into())));
    let detached = ApplicationStoredEvent::Orchestration(Box::new(OrchestrationV2StoredEvent {
        sequence: 3,
        command_id: None,
        event: detached_event("state", "child").unwrap(),
    }));
    assert!(should_publish_shell_application_event(&detached, &mut ids));
}
#[tokio::test]
async fn shell_coalescing_is_separate_per_location_and_delivered_states_remain_charged() {
    let buffer = LiveBuffer::new(LiveStreamLimits {
        max_items: 4,
        ..Default::default()
    });
    let first = removed(1, "same", false);
    buffer
        .push(
            SharedLiveItem::new(first.clone(), 1).unwrap(),
            shell_delta_key(&first),
        )
        .unwrap();
    let _held = buffer.pull().await.unwrap().unwrap();
    for value in [
        removed(3, "same", false),
        removed(4, "same", true),
        removed(5, "other", false),
        removed(10, "same", false),
        removed(11, "same", true),
    ] {
        let sequence = match &value {
            OrchestrationV2ShellStreamItem::ThreadRemoved(v) => v.sequence,
            _ => 0,
        };
        buffer
            .push(
                SharedLiveItem::new(value.clone(), sequence).unwrap(),
                shell_delta_key(&value),
            )
            .unwrap();
    }
    assert_eq!(buffer.usage().retained_items, 4);
    let delivered = buffer.pull().await.unwrap().unwrap();
    assert_eq!(
        delivered.iter().map(|v| v.sequence).collect::<Vec<_>>(),
        vec![5, 10, 11]
    );
    let one = LiveBuffer::new(LiveStreamLimits {
        max_items: 1,
        ..Default::default()
    });
    one.push(
        SharedLiveItem::new(first.clone(), 1).unwrap(),
        shell_delta_key(&first),
    )
    .unwrap();
    let _held = one.pull().await.unwrap();
    let next = removed(2, "same", false);
    assert!(
        one.push(
            SharedLiveItem::new(next.clone(), 2).unwrap(),
            shell_delta_key(&next)
        )
        .is_err()
    );
}
#[test]
fn authoritative_snapshot_always_precedes_metadata_only_enrichment() {
    let mut base = snapshot();
    base.threads = vec![shell("active", None)];
    base.archived_threads = vec![shell("archived", None)];
    base.projects=serde_json::from_value(json!([{"id":"project-a","title":"A","workspaceRoot":"/workspace/a","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":TEST_TIME,"updatedAt":TEST_TIME},{"id":"project-b","title":"B","workspaceRoot":"/workspace/b","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":TEST_TIME,"updatedAt":TEST_TIME}])).unwrap();
    let items = shell_stream_items_from_initial_snapshot(
        base.clone(),
        vec!["/workspace/a".into(), "/workspace/a".into()],
    );
    assert_eq!(items.len(), 2);
    let OrchestrationV2ShellStreamItem::Snapshot(initial) = &items[0] else {
        panic!()
    };
    assert_eq!(initial.snapshot, base);
    assert!(initial.resolved_repository_identity_roots.is_none());
    let OrchestrationV2ShellStreamItem::Snapshot(enriched) = &items[1] else {
        panic!()
    };
    assert!(enriched.snapshot.threads.is_empty());
    assert!(enriched.snapshot.archived_threads.is_empty());
    assert_eq!(enriched.snapshot.projects.len(), 1);
    assert_eq!(
        enriched.resolved_repository_identity_roots,
        Some(vec!["/workspace/a".into()])
    );
    assert!(
        serde_json::to_string(enriched).unwrap().len()
            < serde_json::to_string(initial).unwrap().len()
    );
}
#[test]
fn enrichment_dedupes_sequence_only_repeats_and_survives_thread_deltas() {
    let mut filter = ShellEnrichmentDeduper::default();
    let initial = OrchestrationV2ShellStreamItem::Snapshot(Box::new(ShellStreamItemSnapshot {
        snapshot: snapshot(),
        resolved_repository_identity_roots: None,
    }));
    let marked = enrichment_snapshot(snapshot(), vec!["/workspace/a".into()]);
    assert!(filter.accept(&initial));
    assert!(filter.accept(&initial));
    assert!(filter.accept(&marked));
    let mut later = snapshot();
    later.snapshot_sequence = 10;
    let repeated = enrichment_snapshot(later, vec!["/workspace/a".into()]);
    assert!(!filter.accept(&repeated));
    assert!(filter.accept(&removed(11, "thread", false)));
    assert!(!filter.accept(&repeated));
    let removed =
        OrchestrationV2ShellStreamItem::ProjectRemoved(Box::new(ShellStreamItemProjectRemoved {
            sequence: 12,
            project_id: "project".to_owned().into(),
        }));
    assert!(filter.accept(&removed));
    assert!(filter.accept(&repeated));
    assert!(filter.accept(&enrichment_snapshot(
        snapshot(),
        vec!["/workspace/a".into(), "/workspace/b".into()]
    )));
}
#[test]
fn unchanged_shells_resend_at_five_seconds_and_reset_after_removal() {
    let mut filter = UnchangedThreadShellFilter::default();
    let update =
        shell_stream_item_from_thread_shell(1, ThreadId("a".into()), Some(shell("a", None)));
    assert!(filter.accept(&update, 0));
    assert!(!filter.accept(&update, 4999));
    assert!(filter.accept(&update, 5000));
    assert!(filter.accept(&removed(3, "a", false), 5001));
    assert!(filter.accept(&update, 5002));
    let mut changed = shell("a", None);
    changed.title = "Renamed".into();
    assert!(filter.accept(
        &shell_stream_item_from_thread_shell(4, changed.id.clone(), Some(changed)),
        5003
    ));
}
#[tokio::test]
async fn shell_resume_honors_gap_pruning_bytes_and_the_head_exception() {
    let fixture = TestDatabase::memory().unwrap();
    let (_service, sink) = service(&fixture);
    let head = seed(&fixture, &sink).await.unwrap();
    assert!(
        can_resume_shell_from(&fixture.stores, 0, head)
            .await
            .unwrap()
    );
    assert!(
        !can_resume_shell_from(&fixture.stores, head + 1, head)
            .await
            .unwrap()
    );
    assert!(
        !can_resume_shell_from(&fixture.stores, 0, 1001)
            .await
            .unwrap()
    );
    fixture.stores.transaction(|tx|{tx.execute("INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES('event-log-pruned-through',2,1000,?1)",[TEST_TIME])?;Ok(())}).await.unwrap();
    assert!(
        can_resume_shell_from(&fixture.stores, head, head)
            .await
            .unwrap()
    );
    assert!(
        !can_resume_shell_from(&fixture.stores, 0, head)
            .await
            .unwrap()
    );
}
