//! Ports sync invariants from shell-sync, threads-sync, shellReducer,
//! orchestrationV2Projection and threadHistoryMerge TypeScript tests.
use futures::{FutureExt, StreamExt};
use serde_json::{Value, json};
use std::rc::Rc;
use t3_client::{
    ClientError, ClientStore,
    platform::*,
    state::{
        history::{HistoryMeta, WorkPage, merge_history},
        items,
        projection::apply_event,
        shell::ShellState,
        thread::ThreadState,
    },
    testing::*,
    types::*,
};
use t3_contracts::{orchestration_v2::*, orchestration_v2_id_table::IdOrIndex};
const NOW: &str = "2026-06-20T00:00:00.000Z";
fn thread_json() -> Value {
    json!({"id":"thread-v2","projectId":"project-v2","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"thread-v2","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})
}
fn projection() -> Result<OrchestrationV2ThreadProjection, ClientError> {
    Ok(serde_json::from_value(
        json!({"thread":thread_json(),"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":NOW}),
    )?)
}
fn shell(sequence: u64) -> Result<OrchestrationV2ShellSnapshot, ClientError> {
    let mut thread = thread_json();
    thread.as_object_mut().ok_or_else(||ClientError::InvalidPayload("test object".into()))?.extend(json!({"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0}).as_object().ok_or_else(||ClientError::InvalidPayload("test object".into()))?.clone());
    Ok(serde_json::from_value(
        json!({"schemaVersion":1,"snapshotSequence":sequence,"threads":[thread],"archivedThreads":[],"projects":[{"id":"project-v2","title":"Project","workspaceRoot":"/workspace/project","repositoryIdentity":null,"defaultModelSelection":null,"scripts":[],"createdAt":NOW,"updatedAt":NOW}]}),
    )?)
}
fn row(index: u64) -> Result<OrchestrationV2ProjectedTurnItem, ClientError> {
    let id = format!("item-{index}");
    Ok(serde_json::from_value(
        json!({"position":index,"visibility":"local","sourceThreadId":"thread-v2","sourceItemId":id,"item":{"id":id,"type":"command_execution","threadId":"thread-v2","runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":index+1,"status":"completed","title":format!("Command {index}"),"input":"pwd","output":"done","exitCode":0,"startedAt":NOW,"completedAt":NOW,"updatedAt":NOW}}),
    )?)
}
fn event(kind: &str, payload: Value) -> Result<OrchestrationV2DomainEvent, ClientError> {
    Ok(serde_json::from_value(
        json!({"type":kind,"id":"event-v2","threadId":"thread-v2","occurredAt":NOW,"payload":payload}),
    )?)
}
fn stream_event(
    sequence: u64,
    event: OrchestrationV2DomainEvent,
) -> OrchestrationV2ThreadStreamItem {
    OrchestrationV2ThreadStreamItem::Event(Box::new(ThreadStreamItemMember3 {
        kind: ThreadStreamItemMember3Kind::Event,
        sequence,
        event,
        ids: None,
    }))
}
fn detail(sequence: u64) -> Result<OrchestrationV2ThreadDetailSnapshot, ClientError> {
    Ok(OrchestrationV2ThreadDetailSnapshot {
        snapshot_sequence: sequence,
        projection: projection()?,
        history_cursor: None,
        has_more_history: None,
        latest_local_turn_ordinal: None,
    })
}
fn run(status: &str) -> Value {
    json!({"id":"run-v2","threadId":"thread-v2","ordinal":1,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"providerThreadId":null,"userMessageId":"message-v2","rootNodeId":null,"activeAttemptId":null,"status":status,"requestedAt":NOW,"startedAt":NOW,"completedAt":NOW,"checkpointId":null,"contextHandoffId":null})
}
#[test]
fn shell_filtered_sequences_dedup_and_authoritative_reset() -> Result<(), ClientError> {
    let mut state = ShellState::default();
    state.begin_session(Generation(1));
    state.install(shell(7)?);
    let remove = |sequence| {
        serde_json::from_value(
            json!({"kind":"thread.removed","sequence":sequence,"location":"active","threadId":"thread-v2"}),
        )
    };
    assert_eq!(state.apply(Generation(1), remove(19)?), SyncAction::Applied);
    assert_eq!(
        state.apply(Generation(1), remove(18)?),
        SyncAction::Duplicate
    );
    assert_eq!(
        state.snapshot.as_ref().map(|s| s.snapshot_sequence),
        Some(19)
    );
    assert_eq!(state.status, SyncStatus::Synchronizing);
    assert_eq!(
        state.removed_at.get(&ThreadId("thread-v2".into())),
        Some(&19)
    );
    state.install(shell(4)?);
    assert!(state.removed_at.is_empty());
    assert_eq!(state.resume_sequence(), Some(4));
    assert_eq!(
        state.apply(Generation(0), remove(30)?),
        SyncAction::StaleSession
    );
    assert_eq!(state.resume_sequence(), Some(4));
    Ok(())
}
#[test]
fn completion_required_and_gap_keeps_rows_until_snapshot() -> Result<(), ClientError> {
    let mut state = ShellState::default();
    state.install(shell(5)?);
    assert_eq!(state.status, SyncStatus::Synchronizing);
    state.apply(
        Generation(0),
        serde_json::from_value(json!({"kind":"synchronized"}))?,
    );
    assert_eq!(state.status, SyncStatus::Live);
    let old = state.snapshot.clone();
    assert_eq!(
        state.apply(
            Generation(0),
            serde_json::from_value(json!({"kind":"snapshot-required"}))?
        ),
        SyncAction::ReloadSnapshot
    );
    assert_eq!(old, state.snapshot);
    assert_eq!(state.resume_sequence(), None);
    state.apply(
        Generation(0),
        serde_json::from_value(json!({"kind":"synchronized"}))?,
    );
    assert_ne!(state.status, SyncStatus::Live);
    state.install(shell(2)?);
    state.apply(
        Generation(0),
        serde_json::from_value(json!({"kind":"synchronized"}))?,
    );
    assert_eq!(state.resume_sequence(), Some(2));
    assert_eq!(state.status, SyncStatus::Live);
    Ok(())
}
#[test]
fn archive_delta_removes_both_rows() -> Result<(), ClientError> {
    let mut state = ShellState::default();
    let mut snapshot = shell(0)?;
    snapshot.archived_threads = snapshot.threads.clone();
    let row = snapshot.threads[0].clone();
    state.install(snapshot);
    state.apply(
        Generation(0),
        OrchestrationV2ShellStreamItem::ThreadUpdated(Box::new(ShellStreamItemThreadUpdated {
            sequence: 2,
            location: ShellStreamItemThreadUpdatedLocation::Archive,
            thread: row,
        })),
    );
    assert!(
        state
            .snapshot
            .as_ref()
            .is_some_and(|s| s.threads.is_empty() && s.archived_threads.is_empty())
    );
    Ok(())
}
#[test]
fn enrichment_changes_identity_without_structure_or_cursor() -> Result<(), ClientError> {
    let mut state = ShellState::default();
    let mut original = shell(10)?;
    original.projects[0].repository_identity = Some(Some(serde_json::from_value(
        json!({"canonicalKey":"github.com/t3/code","locator":{"source":"git-remote","remoteName":"origin","remoteUrl":"https://github.com/t3/code"}}),
    )?));
    // Explicit null enrichment is authoritative even when its structure/cursor is stale.
    state.install(original);
    let mut missing = shell(11)?;
    missing.projects[0].repository_identity = None;
    state.install(missing);
    assert!(state.snapshot.as_ref().is_some_and(|s| {
        s.projects[0]
            .repository_identity
            .as_ref()
            .and_then(|i| i.as_ref())
            .is_some()
    }));
    let mut next = shell(2)?;
    next.threads.clear();
    state.apply(
        Generation(0),
        OrchestrationV2ShellStreamItem::Snapshot(Box::new(ShellStreamItemSnapshot {
            snapshot: next,
            resolved_repository_identity_roots: Some(vec!["/workspace/project".into()]),
        })),
    );
    assert_eq!(state.resume_sequence(), Some(11));
    assert_eq!(state.snapshot.as_ref().map(|s| s.threads.len()), Some(1));
    assert!(
        state
            .snapshot
            .as_ref()
            .is_some_and(|s| s.projects[0].repository_identity == Some(None))
    );
    Ok(())
}
#[test]
fn unknown_thread_events_advance_resume_cursor_but_not_projection() -> Result<(), ClientError> {
    let mut t = ThreadState::new(ThreadId("thread-v2".into()));
    t.install(detail(7)?)?;
    let original = t.projection.clone();
    t.apply(
        Generation(0),
        OrchestrationV2ThreadStreamItem::UnknownEvent {
            sequence: 99,
            event_type: "future.event".into(),
        },
    )?;
    assert_eq!(original, t.projection);
    assert_eq!(t.resume_sequence(), Some(99));
    assert_eq!(
        t.apply(
            Generation(0),
            OrchestrationV2ThreadStreamItem::UnknownEvent {
                sequence: 50,
                event_type: "future.old".into()
            }
        )?,
        SyncAction::Duplicate
    );
    Ok(())
}
#[test]
fn old_session_and_deleted_thread_cannot_resurrect() -> Result<(), ClientError> {
    let mut t = ThreadState::new(ThreadId("thread-v2".into()));
    t.install(detail(7)?)?;
    t.begin_session(Generation(2));
    let mut renamed = thread_json();
    renamed["title"] = json!("renamed");
    assert_eq!(
        t.apply(
            Generation(1),
            stream_event(10, event("thread.metadata-updated", renamed.clone())?)
        )?,
        SyncAction::StaleSession
    );
    t.apply(
        Generation(2),
        stream_event(8, event("thread.deleted", thread_json())?),
    )?;
    assert_eq!(t.status, SyncStatus::Deleted);
    assert!(t.projection.is_none());
    t.apply(
        Generation(2),
        stream_event(9, event("thread.metadata-updated", renamed)?),
    )?;
    assert!(t.projection.is_none());
    t.install(detail(1)?)?;
    assert!(t.projection.is_some());
    assert_eq!(t.sequence, 1);
    Ok(())
}
#[test]
fn visited_event_does_not_bump_activity() -> Result<(), ClientError> {
    let mut p = projection()?;
    let old = p.updated_at.clone();
    let raw = json!({"type":"thread.visited","id":"event-visit","threadId":"thread-v2","occurredAt":"2026-06-21T00:00:00.000Z","payload":thread_json()});
    apply_event(
        &mut p,
        serde_json::from_value(raw)?,
        &HistoryMeta::default(),
    )?;
    assert_eq!(p.updated_at, old);
    Ok(())
}

#[test]
fn local_items_order_and_history_preserves_live_updates() -> Result<(), ClientError> {
    let mut p = projection()?;
    for n in [4, 1, 3] {
        apply_event(
            &mut p,
            event("turn-item.updated", serde_json::to_value(row(n)?.item)?)?,
            &HistoryMeta::default(),
        )?;
    }
    let mut changed = row(1)?;
    if let OrchestrationV2TurnItem::CommandExecution(v) = &mut changed.item {
        v.output = Some(Some("new live output".into()));
    }
    apply_event(
        &mut p,
        event(
            "turn-item.updated",
            serde_json::to_value(changed.item.clone())?,
        )?,
        &HistoryMeta::default(),
    )?;
    merge_history(
        &mut p,
        vec![row(0)?, row(1)?, row(2)?, row(3)?, row(3)?],
        None,
    );
    assert_eq!(
        p.visible_turn_items
            .iter()
            .map(|r| items::ordinal(&r.item))
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(p.visible_turn_items[1].item, changed.item);
    assert_eq!(p.turn_items.len(), 5);
    assert_eq!(
        p.visible_turn_items
            .iter()
            .map(|r| r.position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    p.visible_turn_items.remove(1);
    merge_history(&mut p, vec![row(1)?], None);
    assert!(!p.visible_turn_items.iter().any(|r| {
        r.source_item_id
            == row(1)
                .map(|r| r.source_item_id)
                .unwrap_or(IdOrIndex::Index(99))
    }));
    Ok(())
}
#[test]
fn work_page_uses_source_bounds_and_updates_anchor() -> Result<(), ClientError> {
    let mut p = projection()?;
    let mut anchor = row(1)?;
    items::set_deferred(
        &mut anchor.item,
        Some(OrchestrationV2DeferredWork {
            cursor: t3_contracts::base_schemas::TrimmedNonEmptyString("work".into()),
            item_count: 4,
            last_ordinal: 5,
        }),
    );
    p.turn_items = vec![anchor.item.clone(), row(4)?.item];
    p.visible_turn_items = vec![anchor.clone(), row(4)?];
    let mut wrong = row(2)?;
    wrong.source_thread_id = IdOrIndex::Id(ThreadId("other".into()));
    merge_history(
        &mut p,
        vec![row(0)?, row(1)?, row(2)?, row(3)?, wrong, row(9)?],
        Some(WorkPage {
            anchor: anchor.source_item_id,
            next_cursor: None,
        }),
    );
    assert_eq!(
        p.visible_turn_items
            .iter()
            .map(|r| items::ordinal(&r.item))
            .collect::<Vec<_>>(),
        vec![2, 3, 4, 5]
    );
    assert!(items::deferred(&p.visible_turn_items[0].item).is_none());
    Ok(())
}
#[test]
fn history_request_rejects_old_cursor_and_old_snapshot_but_keeps_live_edits()
-> Result<(), ClientError> {
    let mut t = ThreadState::new(ThreadId("thread-v2".into()));
    let mut snapshot = detail(7)?;
    snapshot.history_cursor = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "page-1".into(),
    )));
    snapshot.has_more_history = Some(true);
    t.install(snapshot.clone())?;
    let request = t
        .history_request()
        .ok_or_else(|| ClientError::InvalidPayload("missing request".into()))?;
    let page = || OrchestrationV2ThreadHistoryPage {
        snapshot_sequence: 7,
        items: vec![],
        next_cursor: None,
        has_more_history: false,
        id_table: None,
        ids: None,
    };
    t.apply(
        Generation(0),
        stream_event(
            8,
            event("thread.metadata-updated", {
                let mut v = thread_json();
                v["title"] = json!("new title");
                v
            })?,
        ),
    )?;
    assert_eq!(t.apply_history(&request, page()), SyncAction::Applied);
    assert_eq!(
        t.projection.as_ref().map(|p| p.thread.title.0.as_str()),
        Some("new title")
    );
    assert_eq!(t.sequence, 8);
    assert_eq!(t.apply_history(&request, page()), SyncAction::StaleSession);
    t.install(snapshot)?;
    assert_eq!(t.apply_history(&request, page()), SyncAction::StaleSession);
    Ok(())
}
#[test]
fn partial_window_drops_old_work_but_accepts_active_run() -> Result<(), ClientError> {
    let mut p = projection()?;
    let history = HistoryMeta {
        has_more: true,
        latest_local_ordinal: Some(10),
        ..HistoryMeta::default()
    };
    assert!(!apply_event(
        &mut p,
        event("turn-item.updated", serde_json::to_value(row(0)?.item)?)?,
        &history
    )?);
    assert!(p.turn_items.is_empty());
    apply_event(&mut p, event("run.created", run("running"))?, &history)?;
    let mut payload = serde_json::to_value(row(0)?.item)?;
    payload["runId"] = json!("run-v2");
    apply_event(&mut p, event("turn-item.updated", payload)?, &history)?;
    assert_eq!(p.visible_turn_items.len(), 1);
    apply_event(&mut p, event("run.updated", run("rolled_back"))?, &history)?;
    assert!(p.visible_turn_items.is_empty());
    Ok(())
}
#[test]
fn environment_isolation_batch_notifications_and_late_session() -> Result<(), ClientError> {
    let store = ClientStore::default();
    let a = EnvironmentId("a".into());
    let b = EnvironmentId("b".into());
    let ga = store.begin_session(a.clone());
    let gb = store.begin_session(b.clone());
    let mut changes = store.subscribe();
    assert!(changes.next().now_or_never().flatten().is_some());
    let snapshot_item = |s| {
        OrchestrationV2ShellStreamItem::Snapshot(Box::new(ShellStreamItemSnapshot {
            snapshot: s,
            resolved_repository_identity_roots: None,
        }))
    };
    let before = store.snapshot();
    store.apply_shell(
        &a,
        ga,
        vec![snapshot_item(shell(1)?), snapshot_item(shell(2)?)],
    );
    store.apply_shell(&b, gb, vec![snapshot_item(shell(7)?)]);
    let changed = changes
        .next()
        .now_or_never()
        .flatten()
        .ok_or_else(|| ClientError::InvalidPayload("missing notification".into()))?;
    assert_eq!(changed.revision, before.revision + 2);
    assert!(changes.next().now_or_never().is_none());
    let old = store.snapshot();
    let fresh = store.begin_session(a.clone());
    assert_ne!(fresh, ga);
    assert_eq!(
        store.apply_shell(&a, ga, vec![snapshot_item(shell(99)?)]),
        SyncAction::StaleSession
    );
    assert_eq!(old.environments[&a].shell.resume_sequence(), Some(2));
    assert_eq!(
        store.snapshot().environments[&b].shell.resume_sequence(),
        Some(7)
    );
    assert!(Rc::ptr_eq(
        &old.environments[&b],
        &store.snapshot().environments[&b]
    ));
    Ok(())
}
#[test]
fn fake_platform_manual_timers_secure_separation_and_scripted_server() -> Result<(), ClientError> {
    let host = FakePlatform::default();
    let mut timer = host.sleep(10);
    assert!(timer.as_mut().now_or_never().is_none());
    host.advance(9);
    assert!(timer.as_mut().now_or_never().is_none());
    host.advance(1);
    assert!(timer.now_or_never().is_some());
    futures::executor::block_on(async {
        let key = StorageKey {
            namespace: "token".into(),
            environment: None,
            record: "one".into(),
        };
        host.write(
            StorageArea::Secure,
            vec![StorageWrite {
                key: key.clone(),
                value: Some(b"secret".to_vec()),
            }],
        )
        .await?;
        assert_eq!(host.get(StorageArea::Ordinary, key.clone()).await?, None);
        assert_eq!(
            host.get(StorageArea::Secure, key).await?,
            Some(b"secret".to_vec())
        );
        Ok::<_, ClientError>(())
    })?;
    let store = ClientStore::default();
    let environment = EnvironmentId("a".into());
    let session = store.begin_session(environment.clone());
    let mut connection = ScriptedConnection::default();
    connection.deliveries.push_back(ServerDelivery::Shell {
        environment: environment.clone(),
        session,
        item: OrchestrationV2ShellStreamItem::Snapshot(Box::new(ShellStreamItemSnapshot {
            snapshot: shell(8)?,
            resolved_repository_identity_roots: None,
        })),
    });
    connection.drain(&store)?;
    assert_eq!(
        store.snapshot().environments[&environment]
            .shell
            .resume_sequence(),
        Some(8)
    );
    Ok(())
}

#[test]
fn bounded_compact_ids_and_omitted_rows_restore_before_reduction() -> Result<(), ClientError> {
    let mut p = projection()?;
    p.visible_turn_items = vec![row(0)?];
    let raw = json!({"snapshotSequence":9,"projection":p,"historyCursor":"next","hasMoreHistory":true,"latestLocalTurnOrdinal":1,"turnItemsOmitLocalVisible":true,"idTable":true});
    let mut encoded = t3_contracts::orchestration_v2_id_table::encode_record_id_table(
        &raw,
        t3_contracts::orchestration_v2_id_table::RecordContainer::Key("projection"),
    )?;
    encoded
        .as_object_mut()
        .ok_or_else(|| ClientError::InvalidPayload("fixture".into()))?
        .remove("idTable");
    let snapshot = t3_client::state::decoding::bounded_snapshot(encoded)?;
    assert_eq!(
        snapshot.projection.thread.id,
        IdOrIndex::Id(ThreadId("thread-v2".into()))
    );
    assert_eq!(snapshot.projection.turn_items.len(), 1);
    assert_eq!(
        snapshot.projection.turn_items[0],
        snapshot.projection.visible_turn_items[0].item
    );
    let mut t = ThreadState::new(ThreadId("thread-v2".into()));
    t.install(snapshot)?;
    assert!(t.history.partial());
    assert_eq!(t.sequence, 9);
    Ok(())
}
#[test]
fn queued_user_input_is_hidden_until_run_starts() -> Result<(), ClientError> {
    let mut p = projection()?;
    let h = HistoryMeta::default();
    apply_event(&mut p, event("run.created", run("queued"))?, &h)?;
    let mut input = serde_json::to_value(row(0)?.item)?;
    input["type"] = json!("user_message");
    input["runId"] = json!("run-v2");
    input["messageId"] = json!("message-v2");
    input["inputIntent"] = json!("queued_turn");
    input["text"] = json!("queued");
    input["attachments"] = json!([]);
    input["createdBy"] = json!("user");
    input["creationSource"] = json!("web");
    apply_event(&mut p, event("turn-item.updated", input)?, &h)?;
    assert_eq!(p.turn_items.len(), 1);
    assert!(p.visible_turn_items.is_empty());
    apply_event(&mut p, event("run.updated", run("running"))?, &h)?;
    assert_eq!(p.visible_turn_items.len(), 1);
    apply_event(&mut p, event("run.updated", run("cancelled"))?, &h)?;
    assert!(p.visible_turn_items.is_empty());
    Ok(())
}
#[test]
fn streaming_replacement_restores_bodies_and_keeps_deferred_cursor() -> Result<(), ClientError> {
    let mut p = projection()?;
    let h = HistoryMeta::default();
    let mut input = serde_json::to_value(row(0)?.item)?;
    input["type"] = json!("assistant_message");
    input["streaming"] = json!(false);
    input["messageId"] = json!("message-v2");
    input["text"] = json!("live text");
    let message = json!({"id":"message-v2","threadId":"thread-v2","runId":null,"nodeId":null,"createdBy":"agent","creationSource":"provider","streaming":false,"role":"assistant","text":"live text","attachments":[],"createdAt":NOW,"updatedAt":NOW});
    apply_event(&mut p, event("message.updated", message)?, &h)?;
    input["deferredWork"] = json!({"cursor":"work","itemCount":3,"lastOrdinal":5});
    apply_event(&mut p, event("turn-item.updated", input.clone())?, &h)?;
    input
        .as_object_mut()
        .ok_or_else(|| ClientError::InvalidPayload("fixture".into()))?
        .remove("deferredWork");
    input["text"] = json!("");
    input["textOmitted"] = json!(true);
    apply_event(&mut p, event("turn-item.updated", input)?, &h)?;
    assert_eq!(p.visible_turn_items.len(), 1);
    assert!(items::deferred(&p.turn_items[0]).is_some());
    if let OrchestrationV2TurnItem::AssistantMessage(v) = &p.turn_items[0] {
        assert_eq!(v.text, "live text");
        assert!(v.text_omitted.is_none());
    } else {
        return Err(ClientError::InvalidPayload("unexpected item".into()));
    }
    Ok(())
}
#[test]
fn token_usage_survives_terminal_provider_update() -> Result<(), ClientError> {
    let mut p = projection()?;
    let mut payload = json!({"id":"turn","providerThreadId":"provider-thread","nodeId":"node","runAttemptId":null,"nativeTurnRef":null,"ordinal":1,"status":"running","startedAt":NOW,"completedAt":null,"tokenUsage":{"usedTokens":50000,"maxTokens":100000,"updatedAt":NOW}});
    apply_event(
        &mut p,
        event("provider-turn.updated", payload.clone())?,
        &HistoryMeta::default(),
    )?;
    let usage = p.provider_turns[0].token_usage.clone();
    payload
        .as_object_mut()
        .ok_or_else(|| ClientError::InvalidPayload("fixture".into()))?
        .remove("tokenUsage");
    payload["status"] = json!("completed");
    apply_event(
        &mut p,
        event("provider-turn.updated", payload)?,
        &HistoryMeta::default(),
    )?;
    assert_eq!(p.provider_turns.len(), 1);
    assert_eq!(p.provider_turns[0].token_usage, usage);
    Ok(())
}

#[test]
fn disconnect_invalidates_inflight_session_and_preserves_cache() -> Result<(), ClientError> {
    let store = ClientStore::default();
    let env = EnvironmentId("env".into());
    let session = store.begin_session(env.clone());
    store.apply_shell(
        &env,
        session,
        vec![OrchestrationV2ShellStreamItem::Snapshot(Box::new(
            ShellStreamItemSnapshot {
                snapshot: shell(7)?,
                resolved_repository_identity_roots: None,
            },
        ))],
    );
    assert_eq!(store.disconnect(&env, session), SyncAction::Applied);
    let current = store.snapshot();
    assert_eq!(current.environments[&env].shell.status, SyncStatus::Cached);
    assert_eq!(current.environments[&env].shell.resume_sequence(), Some(7));
    assert_eq!(
        store.apply_shell(
            &env,
            session,
            vec![serde_json::from_value(json!({"kind":"synchronized"}))?]
        ),
        SyncAction::StaleSession
    );
    assert_eq!(store.snapshot().revision, current.revision);
    Ok(())
}
