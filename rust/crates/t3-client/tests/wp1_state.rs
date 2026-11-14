//! WP1 state parity regressions.
#![allow(clippy::unwrap_used)]

use futures::future::LocalBoxFuture;
use futures::{FutureExt, channel::oneshot, executor::block_on};
use serde_json::{Value, json};
use std::rc::Rc;
use std::{cell::RefCell, collections::VecDeque};
use t3_client::rpc::{RpcEndpoint, RpcMethod};
use t3_client::{
    ClientError, ClientStore,
    platform::*,
    state::{history::HistoryMeta, projection::apply_event, thread::ThreadState},
    testing::*,
    types::*,
};
use t3_client::{persistence::EnvironmentCacheStore, state::sync::*};
use t3_contracts::orchestration_v2::*;
#[derive(Default)]
struct DeferredRpc {
    replies: RefCell<VecDeque<LocalBoxFuture<'static, Result<Value, ClientError>>>>,
}
impl RpcEndpoint for DeferredRpc {
    fn call<M: RpcMethod>(
        &self,
        _: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        let reply = self.replies.borrow_mut().pop_front().unwrap();
        async move { Ok(serde_json::from_value(reply.await?)?) }.boxed_local()
    }
    fn subscribe<M: RpcMethod>(
        &self,
        _: M::Input,
    ) -> futures::stream::LocalBoxStream<'static, Result<M::Output, ClientError>> {
        panic!("unexpected subscription")
    }
}
type TestThreadSync = ThreadSync<FakePlatform, ScriptedRpc, ScriptedSnapshots>;
fn thread_sync() -> TestThreadSync {
    let store = ClientStore::default();
    let environment = EnvironmentId("env".into());
    let session = store.begin_session(environment.clone());
    ThreadSync::new(
        store,
        ThreadKey {
            environment,
            thread: ThreadId("thread-v2".into()),
        },
        session,
        Rc::new(EnvironmentCacheStore::new(FakePlatform::default())),
        Rc::new(ScriptedRpc::default()),
        Rc::new(ScriptedSnapshots::default()),
        true,
    )
}
fn install(
    sync: &TestThreadSync,
    snapshot: OrchestrationV2ThreadDetailSnapshot,
) -> Result<(), ClientError> {
    let mut raw = serde_json::to_value(snapshot)?;
    raw["kind"] = json!("snapshot");
    sync.apply(vec![serde_json::from_value(raw)?])?;
    Ok(())
}
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

#[test]
fn partial_window_drop_keeps_projection_and_watermark() -> Result<(), ClientError> {
    let mut state = ThreadState::new(ThreadId("thread-v2".into()));
    let mut snapshot = detail(1)?;
    snapshot.history_cursor = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "cursor".into(),
    )));
    snapshot.has_more_history = Some(true);
    snapshot.latest_local_turn_ordinal = Some(Some(10));
    state.install(snapshot)?;
    let previous = state.projection.clone().unwrap();
    state.apply(
        Generation(0),
        stream_event(
            5,
            event("turn-item.updated", serde_json::to_value(row(1)?.item)?)?,
        ),
    )?;
    assert!(Rc::ptr_eq(&previous, state.projection.as_ref().unwrap()));
    assert_eq!(state.history.latest_local_ordinal, Some(10));
    assert_eq!(state.sequence, 5);
    Ok(())
}

#[test]
fn compact_decode_checks_identity_counts_order_and_local_membership() -> Result<(), ClientError> {
    use t3_client::state::decoding::compact_projection;
    let id = ThreadId("thread-v2".into());
    let mut p = projection()?;
    p.visible_turn_items = vec![row(0)?, row(1)?];
    p.turn_items = vec![row(5)?.item];
    let restored = compact_projection(p.clone(), &id, true, 3, 2)?;
    assert_eq!(
        restored.turn_items,
        vec![row(0)?.item, row(1)?.item, row(5)?.item]
    );
    assert!(compact_projection(p.clone(), &id, true, 2, 2).is_err());
    assert!(compact_projection(p.clone(), &ThreadId("other".into()), true, 3, 2).is_err());
    assert!(compact_projection(p.clone(), &id, false, 1, 2).is_err());
    let mut invalid = p.clone();
    invalid.visible_turn_items[1].position = 0;
    assert!(compact_projection(invalid, &id, true, 3, 2).is_err());
    let mut invalid = p.clone();
    invalid.visible_turn_items[1].source_item_id =
        invalid.visible_turn_items[0].source_item_id.clone();
    assert!(compact_projection(invalid, &id, true, 3, 2).is_err());
    let mut invalid = p.clone();
    invalid.turn_items.push(row(0)?.item);
    assert!(compact_projection(invalid, &id, true, 4, 2).is_err());
    let mut invalid = p.clone();
    invalid.visible_turn_items[0].visibility = ProjectedTurnItemVisibility::Inherited;
    assert!(compact_projection(invalid, &id, true, 3, 2).is_err());
    Ok(())
}

#[test]
fn history_loading_is_single_flight_and_snapshot_invalidates_requests() -> Result<(), ClientError> {
    let mut state = ThreadState::new(ThreadId("thread-v2".into()));
    let mut snapshot = detail(1)?;
    snapshot.history_cursor = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "cursor".into(),
    )));
    snapshot.has_more_history = Some(true);
    state.install(snapshot.clone())?;
    let request = state.history_request().unwrap();
    assert!(state.history_request().is_none());
    state.install(snapshot)?;
    let page = serde_json::from_value(
        json!({"snapshotSequence":1,"items":[],"nextCursor":null,"hasMoreHistory":false}),
    )?;
    assert_eq!(
        state.apply_history(&request, page),
        SyncAction::StaleSession
    );
    assert_eq!(state.history.cursor.as_deref(), Some("cursor"));
    Ok(())
}

#[test]
fn shared_rows_reuse_unchanged_shell_and_timeline_rows() -> Result<(), ClientError> {
    let store = ClientStore::default();
    let environment = EnvironmentId("environment".into());
    let session = store.begin_session(environment.clone());
    let key = ThreadKey {
        environment: environment.clone(),
        thread: ThreadId("thread-v2".into()),
    };
    store.apply_shell(
        &environment,
        session,
        vec![serde_json::from_value(
            json!({"kind":"snapshot","snapshot":shell(1)?}),
        )?],
    );
    let first_shell = store.shared_shell(&environment).unwrap();
    store.apply_shell(
        &environment,
        session,
        vec![serde_json::from_value(
            json!({"kind":"snapshot","snapshot":shell(1)?}),
        )?],
    );
    let same_shell = store.shared_shell(&environment).unwrap();
    assert!(Rc::ptr_eq(&first_shell.threads, &same_shell.threads));
    assert!(Rc::ptr_eq(&first_shell.projects, &same_shell.projects));
    // A row delta replaces only its row; the store names the touched rows for the next view.
    let mut renamed = serde_json::to_value(&shell(1)?.threads[0])?;
    renamed["title"] = json!("Renamed");
    store.apply_shell(
        &environment,
        session,
        vec![serde_json::from_value(
            json!({"kind":"thread.updated","sequence":2,"location":"active","thread":renamed}),
        )?],
    );
    let renamed_shell = store.shared_shell(&environment).unwrap();
    assert_eq!(renamed_shell.threads[0].title, "Renamed");
    assert!(!Rc::ptr_eq(&same_shell.threads, &renamed_shell.threads));
    assert!(Rc::ptr_eq(&same_shell.projects, &renamed_shell.projects));
    assert_eq!(renamed_shell.snapshot_sequence, 2);
    // Any other change to the snapshot makes the next view compare every row.
    store.update_shell(&environment, session, |state| {
        let snapshot = Rc::make_mut(state.snapshot.as_mut().unwrap());
        snapshot.projects[0].title =
            t3_contracts::base_schemas::TrimmedNonEmptyString("Edited".into());
        true
    });
    let edited_shell = store.shared_shell(&environment).unwrap();
    assert_eq!(edited_shell.projects[0].title.0, "Edited");
    assert!(Rc::ptr_eq(&renamed_shell.threads, &edited_shell.threads));
    let mut snapshot = detail(1)?;
    snapshot.projection.turn_items = vec![row(0)?.item, row(1)?.item];
    snapshot.projection.visible_turn_items = vec![row(0)?, row(1)?];
    let mut raw = serde_json::to_value(snapshot)?;
    raw["kind"] = json!("snapshot");
    store.apply_thread(key.clone(), session, vec![serde_json::from_value(raw)?])?;
    let first = store.shared_thread(&key).unwrap();
    let mut item = serde_json::to_value(row(1)?.item)?;
    item["output"] = json!("new output");
    store.apply_thread(
        key.clone(),
        session,
        vec![stream_event(5, event("turn-item.updated", item)?)],
    )?;
    let next = store.shared_thread(&key).unwrap();
    assert!(Rc::ptr_eq(
        &first.visible_turn_items[0],
        &next.visible_turn_items[0]
    ));
    assert!(Rc::ptr_eq(&first.turn_items[0], &next.turn_items[0]));
    assert!(!Rc::ptr_eq(
        &first.visible_turn_items[1],
        &next.visible_turn_items[1]
    ));
    assert!(Rc::ptr_eq(&first.runs, &next.runs));
    assert!(Rc::ptr_eq(&first.thread, &next.thread));
    assert!(Rc::ptr_eq(&next, &store.shared_thread(&key).unwrap()));
    Ok(())
}

#[test]
fn steady_shells_release_together_or_at_deadline_and_reuse_state() -> Result<(), ClientError> {
    use t3_client::state::steady::*;
    let input = |id: &str, snapshot: Rc<OrchestrationV2ShellSnapshot>, status, generation| {
        SteadyShellInput {
            environment: EnvironmentId(id.into()),
            server_sequence: Some(snapshot.snapshot_sequence),
            snapshot: Some(snapshot),
            status,
            generation,
            connecting: true,
            row_actions: 0,
        }
    };
    let a = Rc::new(shell(1)?);
    let b = Rc::new(shell(2)?);
    let a_live = Rc::new(shell(3)?);
    let b_live = Rc::new(shell(4)?);
    let initial = [
        input("a", a.clone(), SyncStatus::Synchronizing, 0),
        input("b", b.clone(), SyncStatus::Synchronizing, 0),
    ];
    let mut state = step(&Rc::default(), &initial, 0, RELEASE_WINDOW_MS);
    assert!(Rc::ptr_eq(
        &state,
        &step(&state, &initial, 1, RELEASE_WINDOW_MS)
    ));
    let waiting = [
        input("a", a_live.clone(), SyncStatus::Live, 1),
        initial[1].clone(),
    ];
    state = step(&state, &waiting, 500, RELEASE_WINDOW_MS);
    assert_eq!(state.release_at, Some(2500));
    assert!(state.holding(&waiting));
    assert!(Rc::ptr_eq(
        &state,
        &step(&state, &waiting, 2499, RELEASE_WINDOW_MS)
    ));
    let released = step(&state, &waiting, 2500, RELEASE_WINDOW_MS);
    assert!(Rc::ptr_eq(
        released.shells[&EnvironmentId("a".into())]
            .shown
            .as_ref()
            .unwrap(),
        &a_live
    ));
    let together = [
        waiting[0].clone(),
        input("b", b_live.clone(), SyncStatus::Live, 1),
    ];
    state = step(&state, &together, 900, RELEASE_WINDOW_MS);
    assert!(!state.holding(&together));
    assert_eq!(state.release_at, None);
    let reconnect = [
        input("a", Rc::new(shell(8)?), SyncStatus::Synchronizing, 1),
        together[1].clone(),
    ];
    state = step(&state, &reconnect, 1000, RELEASE_WINDOW_MS);
    assert!(Rc::ptr_eq(
        state.shells[&EnvironmentId("a".into())]
            .shown
            .as_ref()
            .unwrap(),
        &a_live
    ));
    let mut user_action = reconnect;
    user_action[0].row_actions = 1;
    state = step(&state, &user_action, 1001, RELEASE_WINDOW_MS);
    assert!(!state.holding(&user_action));
    assert!(step(&state, &[], 1002, RELEASE_WINDOW_MS).shells.is_empty());
    Ok(())
}

#[test]
fn cache_flush_serializes_saves_and_drains_newer_data_and_removal() -> Result<(), ClientError> {
    use std::cell::RefCell;
    use t3_client::state::cache::CachePersistence;
    let queue = CachePersistence::default();
    let writes = Rc::new(RefCell::new(Vec::new()));
    let (release, gate) = oneshot::channel();
    let gate = Rc::new(RefCell::new(Some(gate)));
    queue.queue(Some(1));
    let save = |value| {
        let writes = writes.clone();
        let gate = if value == Some(1) {
            gate.borrow_mut().take()
        } else {
            None
        };
        async move {
            if let Some(gate) = gate {
                gate.await.map_err(|_| ClientError::Cancelled)?;
            }
            writes.borrow_mut().push(value);
            Ok(())
        }
    };
    let mut first = Box::pin(queue.flush(save));
    assert!(first.as_mut().now_or_never().is_none());
    queue.queue(Some(2));
    queue.queue(None);
    let second_writes = writes.clone();
    let mut second = Box::pin(queue.flush(move |value| {
        second_writes.borrow_mut().push(value);
        futures::future::ready(Ok(()))
    }));
    assert!(second.as_mut().now_or_never().is_none());
    release.send(()).unwrap();
    block_on(first)?;
    block_on(second)?;
    assert_eq!(*writes.borrow(), vec![Some(1), None]);
    assert!(!queue.is_pending());
    queue.queue(Some(3));
    assert!(
        block_on(queue.flush(|_| futures::future::ready(Err(ClientError::Disconnected)))).is_err()
    );
    assert!(queue.is_pending());
    block_on(queue.flush(|_| futures::future::ready(Ok(()))))?;
    assert!(!queue.is_pending());
    Ok(())
}

#[test]
fn cache_worker_coalesces_initial_delay_and_save_cooldown() -> Result<(), ClientError> {
    use std::cell::RefCell;
    use t3_client::state::cache::CachePersistence;
    let platform = FakePlatform::default();
    let queue = CachePersistence::default();
    let writes = RefCell::new(Vec::new());
    let mut worker = Box::pin(queue.run(&platform, |value| {
        writes.borrow_mut().push(value);
        futures::future::ready(Ok(()))
    }));
    queue.queue(1);
    assert!(worker.as_mut().now_or_never().is_none());
    queue.queue(2);
    platform.advance(499);
    assert!(worker.as_mut().now_or_never().is_none());
    assert!(writes.borrow().is_empty());
    platform.advance(1);
    assert!(worker.as_mut().now_or_never().is_none());
    assert_eq!(*writes.borrow(), vec![2]);
    queue.queue(3);
    queue.queue(4);
    platform.advance(10_000);
    assert!(worker.as_mut().now_or_never().is_none());
    assert_eq!(*writes.borrow(), vec![2, 4]);
    Ok(())
}

#[test]
fn fake_transactions_compare_before_writes_and_commit_both_areas() -> Result<(), ClientError> {
    let platform = FakePlatform::default();
    let key = StorageKey {
        namespace: "test".into(),
        environment: None,
        record: "token".into(),
    };
    block_on(platform.write(
        StorageArea::Secure,
        vec![StorageWrite {
            key: key.clone(),
            value: Some(b"new-token".to_vec()),
        }],
    ))?;
    let rejected = StorageTransaction {
        conditions: vec![StorageCondition {
            area: StorageArea::Secure,
            key: key.clone(),
            expected: Some(b"old-token".to_vec()),
        }],
        ordinary: vec![StorageWrite {
            key: key.clone(),
            value: Some(b"ordinary".to_vec()),
        }],
        secure: vec![StorageWrite {
            key: key.clone(),
            value: None,
        }],
    };
    assert!(!block_on(platform.transaction(rejected.clone()))?);
    assert_eq!(
        block_on(platform.get(StorageArea::Secure, key.clone()))?,
        Some(b"new-token".to_vec())
    );
    assert_eq!(
        block_on(platform.get(StorageArea::Ordinary, key.clone()))?,
        None
    );
    let mut accepted = rejected;
    accepted.conditions[0].expected = Some(b"new-token".to_vec());
    accepted.ordinary.push(StorageWrite {
        key: key.clone(),
        value: Some(b"last-write".to_vec()),
    });
    assert!(block_on(platform.transaction(accepted))?);
    assert_eq!(
        block_on(platform.get(StorageArea::Secure, key.clone()))?,
        None
    );
    assert_eq!(
        block_on(platform.get(StorageArea::Ordinary, key))?,
        Some(b"last-write".to_vec())
    );
    Ok(())
}

#[test]
fn history_completion_preserves_concurrent_live_events_and_rejects_replaced_snapshot()
-> Result<(), ClientError> {
    let sync = thread_sync();
    let mut snapshot = detail(2)?;
    snapshot.history_cursor = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "older".into(),
    )));
    snapshot.has_more_history = Some(true);
    snapshot.latest_local_turn_ordinal = Some(Some(10));
    install(&sync, snapshot.clone())?;
    let (response, answer) = oneshot::channel();
    sync.source.push_history(
        async move { answer.await.map_err(|_| ClientError::Cancelled) }.boxed_local(),
    );
    let mut history = Box::pin(sync.load_earlier(None, None));
    assert!(history.as_mut().now_or_never().is_none());
    assert_eq!(
        block_on(sync.load_earlier(None, None))?,
        HistoryLoadResult::Busy
    );
    sync.apply(vec![stream_event(
        9,
        event("turn-item.updated", serde_json::to_value(row(15)?.item)?)?,
    )])?;
    response.send(serde_json::from_value(json!({"snapshotSequence":2,"items":[row(0)?],"nextCursor":null,"hasMoreHistory":false}))?).unwrap();
    assert_eq!(block_on(history)?, HistoryLoadResult::Loaded);
    let current = sync.store.snapshot();
    let thread = &current.environments[&sync.key.environment].threads[&sync.key.thread];
    assert_eq!(thread.sequence, 9);
    assert_eq!(
        thread.projection.as_ref().unwrap().visible_turn_items.len(),
        2
    );
    assert_eq!(thread.history.latest_local_ordinal, Some(16));
    assert!(thread.history.expanded);
    assert!(thread.cache_snapshot(true).is_none());
    install(&sync, snapshot.clone())?;
    let (response, answer) = oneshot::channel();
    sync.source.push_history(
        async move { answer.await.map_err(|_| ClientError::Cancelled) }.boxed_local(),
    );
    let mut history = Box::pin(sync.load_earlier(None, None));
    assert!(history.as_mut().now_or_never().is_none());
    install(&sync, snapshot)?;
    response.send(serde_json::from_value(json!({"snapshotSequence":2,"items":[row(0)?],"nextCursor":null,"hasMoreHistory":false}))?).unwrap();
    assert_eq!(block_on(history)?, HistoryLoadResult::Noop);
    assert!(
        sync.store.snapshot().environments[&sync.key.environment].threads[&sync.key.thread]
            .projection
            .as_ref()
            .unwrap()
            .visible_turn_items
            .is_empty()
    );
    Ok(())
}

#[test]
fn cancelled_history_request_clears_only_its_loading_flag() -> Result<(), ClientError> {
    let sync = thread_sync();
    let mut snapshot = detail(2)?;
    snapshot.history_cursor = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "older".into(),
    )));
    snapshot.has_more_history = Some(true);
    install(&sync, snapshot)?;
    sync.source
        .push_history(futures::future::pending().boxed_local());
    let mut history = Box::pin(sync.load_earlier(None, None));
    assert!(history.as_mut().now_or_never().is_none());
    assert!(
        sync.store.snapshot().environments[&sync.key.environment].threads[&sync.key.thread]
            .history
            .loading
    );
    drop(history);
    assert!(
        !sync.store.snapshot().environments[&sync.key.environment].threads[&sync.key.thread]
            .history
            .loading
    );
    Ok(())
}

#[test]
fn stale_session_http_answer_does_not_install_thread() -> Result<(), ClientError> {
    let mut sync = thread_sync();
    sync.accepts_bounded = false;
    let (reply, answer) = oneshot::channel();
    sync.source
        .push_thread(async move { answer.await.map_err(|_| ClientError::Cancelled) }.boxed_local());
    let mut follow = Box::pin(sync.follow());
    assert!(follow.as_mut().now_or_never().is_none());
    sync.store.begin_session(sync.key.environment.clone());
    reply
        .send(ThreadLoadResult::Present(Box::new(detail(7)?)))
        .ok();
    block_on(follow)?;
    assert!(
        sync.store.snapshot().environments[&sync.key.environment].threads[&sync.key.thread]
            .projection
            .is_none()
    );
    Ok(())
}

#[test]
fn bounded_cold_open_uses_socket_and_failure_retries_http() -> Result<(), ClientError> {
    let sync = thread_sync();
    sync.set_socket_snapshot_pagination(true);
    let input = json!({"threadId":"thread-v2","requestCompletionMarker":true,"acceptBoundedSnapshot":true,"acceptDeferredWork":true,"acceptCompactTurnItems":true,"acceptCompactIds":true,"acceptDedupedBodies":true});
    sync.rpc.scripts.borrow_mut().push_back(RpcScript {
        method: "orchestration.subscribeThread".into(),
        input: input.clone(),
        replies: vec![Err(ClientError::Disconnected)],
    });
    sync.source.push_thread(
        futures::future::ready(Ok(ThreadLoadResult::Present(Box::new(detail(7)?)))).boxed_local(),
    );
    let mut resumed = input;
    resumed["afterSequence"] = json!(7);
    sync.rpc.scripts.borrow_mut().push_back(RpcScript {
        method: "orchestration.subscribeThread".into(),
        input: resumed,
        replies: vec![Ok(json!({"kind":"synchronized"}))],
    });
    block_on(sync.follow())?;
    assert_eq!(sync.source.thread_requests.borrow().len(), 1);
    assert_eq!(
        sync.store.snapshot().environments[&sync.key.environment].threads[&sync.key.thread].status,
        SyncStatus::Live
    );
    Ok(())
}

#[test]
fn complete_bounded_cache_keeps_paging_evidence_and_expanded_cache_is_not_saved()
-> Result<(), ClientError> {
    let sync = thread_sync();
    let mut snapshot = detail(2)?;
    snapshot.history_cursor = Some(None);
    snapshot.has_more_history = Some(false);
    snapshot.latest_local_turn_ordinal = Some(Some(9));
    install(&sync, snapshot)?;
    block_on(sync.flush())?;
    let saved = block_on(
        sync.cache
            .load_thread(&sync.key.environment, &sync.key.thread),
    )
    .unwrap()
    .unwrap();
    assert_eq!(saved.snapshot.has_more_history, Some(false));
    assert_eq!(saved.snapshot.history_cursor, Some(None));
    sync.store.update_thread(&sync.key, sync.session, |state| {
        state.history.expanded = true;
        Rc::make_mut(state.projection.as_mut().unwrap())
            .visible_turn_items
            .push(row(4).unwrap());
        true
    });
    block_on(sync.flush())?;
    let after = block_on(
        sync.cache
            .load_thread(&sync.key.environment, &sync.key.thread),
    )
    .unwrap()
    .unwrap();
    assert_eq!(saved.snapshot, after.snapshot);
    Ok(())
}

#[test]
fn deferred_shell_links_do_not_overwrite_live_rows_or_replacement_snapshots()
-> Result<(), ClientError> {
    let store = ClientStore::default();
    let environment = EnvironmentId("env".into());
    let session = store.begin_session(environment.clone());
    let source = Rc::new(ScriptedSnapshots::default());
    let cache = Rc::new(EnvironmentCacheStore::new(FakePlatform::default()));
    let sync = ShellSync::new(
        store.clone(),
        environment.clone(),
        session,
        cache.clone(),
        Rc::new(ScriptedRpc::default()),
        source.clone(),
        None,
    );
    let (links, answer) = oneshot::channel();
    source.push_shell(
        futures::future::ready(Ok(ShellLoadResult::Present(LoadedShell {
            snapshot: shell(2)?,
            project_identities: Some("identities".into()),
            links: Some(async move { answer.await.unwrap() }.boxed_local()),
        })))
        .boxed_local(),
    );
    let (identities, fill, loaded) = block_on(sync.load_http())?;
    assert!(loaded);
    assert_eq!(identities.as_deref(), Some("identities"));
    block_on(sync.flush())?;
    assert!(block_on(cache.load_shell(&environment)).unwrap().is_none());
    let mut updated = shell(2)?.threads.remove(0);
    updated.title = "live update".into();
    sync.apply(vec![serde_json::from_value(
        json!({"kind":"thread.updated","sequence":5,"location":"active","thread":updated}),
    )?]);
    let link = serde_json::from_value(
        json!({"host":"github.com","repository":"owner/repo","number":42,"url":"https://github.com/owner/repo/pull/42","source":"manual","linkedAt":NOW,"snapshot":null,"stack":null}),
    )?;
    links
        .send(std::collections::BTreeMap::from([(
            ThreadId("thread-v2".into()),
            vec![link],
        )]))
        .unwrap();
    block_on(fill.unwrap());
    let current = store.snapshot();
    let row = &current.environments[&environment]
        .shell
        .snapshot
        .as_ref()
        .unwrap()
        .threads[0];
    assert_eq!(row.title, "live update");
    assert!(row.pull_requests.is_none());
    block_on(sync.flush())?;
    assert_eq!(
        block_on(cache.load_shell(&environment))
            .unwrap()
            .unwrap()
            .snapshot
            .snapshot_sequence,
        5
    );
    let (links, answer) = oneshot::channel();
    source.push_shell(
        futures::future::ready(Ok(ShellLoadResult::Present(LoadedShell {
            snapshot: shell(6)?,
            project_identities: None,
            links: Some(async move { answer.await.unwrap() }.boxed_local()),
        })))
        .boxed_local(),
    );
    let (_, fill, _) = block_on(sync.load_http())?;
    sync.apply(vec![serde_json::from_value(
        json!({"kind":"snapshot","snapshot":shell(1)?}),
    )?]);
    links.send(std::collections::BTreeMap::new()).unwrap();
    block_on(fill.unwrap());
    assert_eq!(
        store.snapshot().environments[&environment]
            .shell
            .resume_sequence(),
        Some(1)
    );
    Ok(())
}

#[test]
fn compact_pull_request_update_matches_port_authority_and_preserves_metadata()
-> Result<(), ClientError> {
    let mut p = projection()?;
    let first = json!({"host":"forge.example","repository":"mount/owner/repo","number":42,"url":"https://forge.example:8443/mount/owner/repo/pulls/42","source":"manual","linkedAt":NOW,"snapshot":null,"stack":null});
    let mut second = first.clone();
    second["number"] = json!(43);
    second["url"] = json!("https://forge.example:8443/mount/owner/repo/pulls/43");
    p.thread.pull_requests = Some(Some(serde_json::from_value(json!([first, second]))?));
    let original = p.thread.clone();
    let mut update = json!({"host":"forge.example:8443","repository":"MOUNT/OWNER/REPO","number":42,"url":"https://forge.example:8443/mount/owner/repo/pulls/42","source":"agent","linkedAt":NOW,"snapshot":null,"stack":null});
    apply_event(
        &mut p,
        event("thread.pull-request-link-updated", update.clone())?,
        &HistoryMeta::default(),
    )?;
    let links = p.thread.pull_requests.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(serde_json::to_value(&links[0])?["source"], json!("agent"));
    assert_eq!(
        links[1],
        original.pull_requests.as_ref().unwrap().as_ref().unwrap()[1]
    );
    assert_eq!(p.thread.title, original.title);
    update["host"] = json!("forge.example:9443");
    update["url"] = json!("https://forge.example:9443/mount/owner/repo/pulls/42");
    apply_event(
        &mut p,
        event("thread.pull-request-link-updated", update)?,
        &HistoryMeta::default(),
    )?;
    assert_eq!(
        p.thread
            .pull_requests
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .len(),
        2
    );
    Ok(())
}

fn cookie_snapshot_source(
    platform: &Rc<FakePlatform>,
    base: &str,
) -> t3_client::state::http::HttpSnapshotSource<FakePlatform> {
    use t3_client::authorization::{AuthorizedEnvironment, HttpAuthorization, Sensitive};
    t3_client::state::http::HttpSnapshotSource {
        platform: platform.clone(),
        client: t3_client::permissions::EnvironmentHttpClient::new(
            platform.clone(),
            AuthorizedEnvironment {
                session: t3_client::environment::SessionIdentity {
                    environment: EnvironmentId("env".into()),
                    generation: Generation(1),
                    account: None,
                    dpop_thumbprint: None,
                },
                http_base_url: base.into(),
                socket_url: Sensitive("wss://remote.example/ws".into()),
                authorization: HttpAuthorization::Cookie,
                descriptor: None,
            },
            None,
            None,
            false,
        ),
    }
}

#[test]
fn bounded_http_restores_compact_rows_and_uses_cookie_credentials_and_protocol()
-> Result<(), ClientError> {
    let platform = Rc::new(FakePlatform::default());
    let source = cookie_snapshot_source(&platform, "https://remote.example/prefix");
    let mut p = projection()?;
    p.visible_turn_items = vec![row(0)?, row(1)?];
    p.turn_items = vec![row(8)?.item];
    let raw = json!({"snapshotSequence":4,"projection":p,"historyCursor":"cursor","hasMoreHistory":true,"latestLocalTurnOrdinal":2,"turnItemsOmitLocalVisible":true});
    platform.push_http(Ok(HttpResponse {
        status: 200,
        headers: Default::default(),
        body: serde_json::to_vec(&raw)?,
    }));
    let ThreadLoadResult::Present(snapshot) =
        block_on(source.thread(ThreadId("thread-v2".into())))?
    else {
        panic!("missing bounded snapshot");
    };
    assert_eq!(snapshot.projection.turn_items.len(), 3);
    assert_eq!(snapshot.history_cursor.unwrap().unwrap().0, "cursor");
    let request = platform.http_requests().pop().unwrap();
    let url = url::Url::parse(&request.url).unwrap();
    let params = url
        .query_pairs()
        .collect::<std::collections::BTreeMap<_, _>>();
    for key in [
        "compactTurnItems",
        "compactIds",
        "dedupeBodies",
        "deferWork",
    ] {
        assert_eq!(params.get(key).map(|value| value.as_ref()), Some("1"));
    }
    assert_eq!(url.path(), "/api/orchestration/threads/thread-v2/bounded");
    assert_eq!(request.credentials, Credentials::Include);
    assert_eq!(request.headers["x-t3-orchestration-protocol"], "2");
    assert_eq!(platform.pending_timers(), 0);
    Ok(())
}

#[test]
fn bounded_http_only_structured_not_found_is_missing() -> Result<(), ClientError> {
    let platform = Rc::new(FakePlatform::default());
    let source = cookie_snapshot_source(&platform, "https://remote.example");
    for (status, body, missing) in [
        (
            404,
            json!({"_tag":"EnvironmentResourceNotFoundError","code":"not_found","reason":"thread_not_found","traceId":"trace"}),
            true,
        ),
        (404, json!({"message":"not found"}), false),
        (503, json!({}), false),
        (200, json!({}), false),
    ] {
        platform.push_http(Ok(HttpResponse {
            status,
            headers: Default::default(),
            body: serde_json::to_vec(&body)?,
        }));
        let result = block_on(source.thread(ThreadId("thread-v2".into())))?;
        assert_eq!(matches!(result, ThreadLoadResult::Missing), missing);
        if !missing {
            assert!(matches!(result, ThreadLoadResult::Unavailable));
        }
    }
    Ok(())
}

#[test]
fn scope_replacement_invalidates_old_events_and_deletion_flush_on_same_session()
-> Result<(), ClientError> {
    let first = thread_sync();
    install(&first, detail(2)?)?;
    block_on(first.flush())?;
    let successor = ThreadSync::new(
        first.store.clone(),
        first.key.clone(),
        first.session,
        first.cache.clone(),
        first.rpc.clone(),
        first.source.clone(),
        true,
    );
    install(&successor, detail(8)?)?;
    block_on(successor.flush())?;
    assert_eq!(
        first.apply(vec![stream_event(
            9,
            event("thread.deleted", thread_json())?
        )])?,
        SyncAction::StaleSession
    );
    block_on(first.flush())?;
    assert_eq!(
        block_on(
            first
                .cache
                .load_thread(&first.key.environment, &first.key.thread)
        )
        .unwrap()
        .unwrap()
        .snapshot
        .snapshot_sequence,
        8
    );
    assert_eq!(
        first.store.snapshot().environments[&first.key.environment].threads[&first.key.thread]
            .sequence,
        8
    );
    Ok(())
}

#[test]
fn pending_work_repairs_only_the_bounded_cache_and_newer_events_win() -> Result<(), ClientError> {
    for reply_kind in ["running", "missing", "terminal", "timeout"] {
        let store = ClientStore::default();
        let environment = EnvironmentId("env".into());
        let session = store.begin_session(environment.clone());
        let key = ThreadKey {
            environment,
            thread: ThreadId("thread-v2".into()),
        };
        let rpc = Rc::new(DeferredRpc::default());
        let cache = Rc::new(EnvironmentCacheStore::new(FakePlatform::default()));
        let sync = ThreadSync::new(
            store.clone(),
            key.clone(),
            session,
            cache.clone(),
            rpc.clone(),
            Rc::new(ScriptedSnapshots::default()),
            true,
        );
        let mut active = serde_json::to_value(row(0)?.item)?;
        active["status"] = json!("running");
        let mut snapshot = detail(2)?;
        snapshot
            .projection
            .turn_items
            .push(serde_json::from_value(active.clone())?);
        let mut visible = row(0)?;
        visible.item = serde_json::from_value(active.clone())?;
        snapshot.projection.visible_turn_items.push(visible);
        let mut raw = serde_json::to_value(snapshot)?;
        raw["kind"] = json!("snapshot");
        sync.apply(vec![serde_json::from_value(raw)?])?;
        block_on(sync.flush())?;
        store.update_thread(&key, session, |state| {
            state.history.expanded = true;
            Rc::make_mut(state.projection.as_mut().unwrap())
                .visible_turn_items
                .push(row(5).unwrap());
            true
        });
        let (answer, reply) = oneshot::channel();
        rpc.replies.borrow_mut().push_back(
            async move { reply.await.map_err(|_| ClientError::Cancelled) }.boxed_local(),
        );
        let mut repair = Box::pin(sync.reconcile_pending_work());
        assert!(repair.as_mut().now_or_never().is_none());
        let mut latest = active.clone();
        latest["status"] = json!("completed");
        latest["output"] = json!("latest live event");
        sync.apply(vec![stream_event(3, event("turn-item.updated", latest)?)])?;
        let unanswered = match reply_kind {
            "running" => {
                answer.send(json!({"item":active})).unwrap();
                None
            }
            "missing" => {
                answer.send(json!({"item":null})).unwrap();
                None
            }
            "terminal" => {
                active["status"] = json!("completed");
                active["output"] = json!("older lookup");
                answer.send(json!({"item":active})).unwrap();
                None
            }
            _ => {
                cache.platform().advance(10_000);
                Some(answer)
            }
        };
        block_on(repair)?;
        drop(unanswered);
        block_on(sync.flush())?;
        let saved = block_on(cache.load_thread(&key.environment, &key.thread))
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.snapshot.projection.visible_turn_items.len(),
            1,
            "{reply_kind}"
        );
        let item = serde_json::to_value(&saved.snapshot.projection.turn_items[0])?;
        assert_eq!(item["output"], json!("latest live event"), "{reply_kind}");
        assert_eq!(item["status"], json!("completed"));
        assert_eq!(saved.snapshot.snapshot_sequence, 2);
        assert_eq!(cache.platform().pending_timers(), 0);
    }
    Ok(())
}

#[test]
fn superseded_history_scope_releases_its_request_without_clearing_successor_loading()
-> Result<(), ClientError> {
    let first = thread_sync();
    let mut snapshot = detail(2)?;
    snapshot.history_cursor = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "cursor".into(),
    )));
    snapshot.has_more_history = Some(true);
    install(&first, snapshot.clone())?;
    first
        .source
        .push_history(futures::future::pending().boxed_local());
    let mut loading = Box::pin(first.load_earlier(None, None));
    assert!(loading.as_mut().now_or_never().is_none());
    let successor = ThreadSync::new(
        first.store.clone(),
        first.key.clone(),
        first.session,
        first.cache.clone(),
        first.rpc.clone(),
        first.source.clone(),
        true,
    );
    install(&successor, snapshot)?;
    first
        .store
        .update_thread(&first.key, first.session, |state| {
            state.history.loading = true;
            true
        });
    assert_eq!(block_on(loading)?, HistoryLoadResult::Noop);
    assert!(
        first.store.snapshot().environments[&first.key.environment].threads[&first.key.thread]
            .history
            .loading
    );
    Ok(())
}

#[test]
fn shell_cursor_only_updates_skip_writes_but_lower_resets_persist() -> Result<(), ClientError> {
    let store = ClientStore::default();
    let environment = EnvironmentId("env".into());
    let session = store.begin_session(environment.clone());
    let cache = Rc::new(EnvironmentCacheStore::new(FakePlatform::default()));
    let sync = ShellSync::new(
        store,
        environment.clone(),
        session,
        cache.clone(),
        Rc::new(ScriptedRpc::default()),
        Rc::new(ScriptedSnapshots::default()),
        None,
    );
    sync.apply(vec![serde_json::from_value(
        json!({"kind":"snapshot","snapshot":shell(2)?}),
    )?]);
    block_on(sync.flush())?;
    let writes = cache.platform().storage_write_count();
    sync.apply(vec![serde_json::from_value(json!({"kind":"thread.updated","sequence":9,"location":"active","thread":shell(2)?.threads[0]}))?]);
    block_on(sync.flush())?;
    assert_eq!(cache.platform().storage_write_count(), writes);
    sync.apply(vec![serde_json::from_value(
        json!({"kind":"snapshot","snapshot":shell(1)?}),
    )?]);
    block_on(sync.flush())?;
    assert!(cache.platform().storage_write_count() > writes);
    assert_eq!(
        block_on(cache.load_shell(&environment))
            .unwrap()
            .unwrap()
            .snapshot
            .snapshot_sequence,
        1
    );
    Ok(())
}

#[test]
fn thread_metadata_event_table_replaces_payload_and_only_activity_bumps_time()
-> Result<(), ClientError> {
    let events = [
        "thread.created",
        "thread.archived",
        "thread.unarchived",
        "thread.deleted",
        "thread.settled",
        "thread.unsettled",
        "thread.snoozed",
        "thread.unsnoozed",
        "thread.auto-settle-set",
        "thread.pinned",
        "thread.unpinned",
        "thread.pin-reordered",
        "thread.active-reordered",
        "thread.metadata-updated",
        "thread.pull-request-synced",
        "thread.runtime-mode-updated",
        "thread.interaction-mode-updated",
        "thread.model-selection-updated",
        "thread.provider-switched",
        "thread.guest-share-updated",
        "thread.visited",
        "thread.tags-updated",
        "thread.marked-unread",
    ];
    for kind in events {
        let mut p = projection()?;
        let mut payload = thread_json();
        payload["title"] = json!("changed");
        let mut raw = serde_json::to_value(event(kind, payload)?)?;
        raw["occurredAt"] = json!("2026-07-20T00:00:00.000Z");
        apply_event(
            &mut p,
            serde_json::from_value(raw)?,
            &HistoryMeta::default(),
        )?;
        assert_eq!(p.thread.title.0, "changed", "{kind}");
        assert_eq!(
            p.updated_at.0,
            if matches!(
                kind,
                "thread.visited" | "thread.tags-updated" | "thread.marked-unread"
            ) {
                NOW
            } else {
                "2026-07-20T00:00:00.000Z"
            },
            "{kind}"
        );
    }
    Ok(())
}

#[test]
fn unchanged_enrichment_keeps_snapshot_handle_and_local_identity_clear_is_scoped()
-> Result<(), ClientError> {
    let mut state = t3_client::state::shell::ShellState::default();
    state.install(shell(3)?);
    let previous = state.snapshot.clone().unwrap();
    assert_eq!(
        state.apply(
            Generation(0),
            serde_json::from_value(
                json!({"kind":"snapshot","snapshot":shell(1)?,"resolvedRepositoryIdentityRoots":[]})
            )?
        ),
        SyncAction::None
    );
    assert!(Rc::ptr_eq(&previous, state.snapshot.as_ref().unwrap()));
    Ok(())
}

#[test]
fn shell_http_grace_cancels_unanswered_probe_and_retains_resumable_rows() -> Result<(), ClientError>
{
    let store = ClientStore::default();
    let environment = EnvironmentId("env".into());
    let session = store.begin_session(environment.clone());
    let source = Rc::new(ScriptedSnapshots::default());
    let cache = Rc::new(EnvironmentCacheStore::new(FakePlatform::default()));
    let origin = t3_client::persistence::ShellOrigin {
        server_version: "version".into(),
        database_migration: Some(1),
        omit_idle_subagents: true,
        omit_older_settled: true,
        snapshot_at: 0,
    };
    let sync = ShellSync::new(
        store,
        environment,
        session,
        cache.clone(),
        Rc::new(ScriptedRpc::default()),
        source.clone(),
        Some(origin),
    );
    sync.apply(vec![serde_json::from_value(
        json!({"kind":"snapshot","snapshot":shell(4)?}),
    )?]);
    source.push_shell(futures::future::pending().boxed_local());
    let mut load = Box::pin(sync.load_http_with_grace());
    assert!(load.as_mut().now_or_never().is_none());
    cache.platform().advance(2_999);
    assert!(load.as_mut().now_or_never().is_none());
    cache.platform().advance(1);
    let (_, fill, loaded) = block_on(load)?;
    assert!(!loaded);
    assert!(fill.is_none());
    assert_eq!(sync.resume_sequence(), Some(4));
    assert_eq!(source.shell_requests.borrow()[0].resume_from, Some(4));
    assert_eq!(cache.platform().pending_timers(), 0);
    cache.platform().advance(43_200_000);
    assert_eq!(sync.resume_sequence(), None);
    Ok(())
}

#[test]
fn thread_http_misses_retry_until_present_without_retaining_a_false_deletion()
-> Result<(), ClientError> {
    let mut sync = thread_sync();
    sync.accepts_bounded = false;
    sync.source
        .push_thread(futures::future::ready(Ok(ThreadLoadResult::Missing)).boxed_local());
    sync.source
        .push_thread(futures::future::ready(Ok(ThreadLoadResult::Missing)).boxed_local());
    sync.source.push_thread(
        futures::future::ready(Ok(ThreadLoadResult::Present(Box::new(detail(7)?)))).boxed_local(),
    );
    sync.rpc.scripts.borrow_mut().push_back(RpcScript { method: "orchestration.subscribeThread".into(), input: json!({"threadId":"thread-v2","afterSequence":7,"requestCompletionMarker":true,"acceptDedupedBodies":true}), replies: vec![Ok(json!({"kind":"synchronized"}))] });
    let mut follow = Box::pin(sync.follow());
    assert!(follow.as_mut().now_or_never().is_none());
    assert_eq!(sync.source.thread_requests.borrow().len(), 1);
    sync.cache.platform().advance(250);
    assert!(follow.as_mut().now_or_never().is_none());
    assert_eq!(sync.source.thread_requests.borrow().len(), 2);
    sync.cache.platform().advance(500);
    block_on(follow)?;
    assert_eq!(sync.source.thread_requests.borrow().len(), 3);
    assert_eq!(
        sync.store.snapshot().environments[&sync.key.environment].threads[&sync.key.thread].status,
        SyncStatus::Live
    );
    Ok(())
}

#[test]
fn targeted_history_exposes_conversation_before_activity_without_advancing_early()
-> Result<(), ClientError> {
    let sync = thread_sync();
    let mut snapshot = detail(2)?;
    snapshot.history_cursor = Some(Some(t3_contracts::base_schemas::TrimmedNonEmptyString(
        "cursor".into(),
    )));
    snapshot.has_more_history = Some(true);
    install(&sync, snapshot)?;
    sync.source.push_history(futures::future::ready(Ok(serde_json::from_value(json!({"snapshotSequence":2,"items":[row(0)?],"nextCursor":"ignored","hasMoreHistory":true}))?)).boxed_local());
    let (reply, answer) = oneshot::channel();
    sync.source.push_history(
        async move { answer.await.map_err(|_| ClientError::Cancelled) }.boxed_local(),
    );
    let mut history = Box::pin(sync.load_earlier(Some("item-0".into()), None));
    assert!(history.as_mut().now_or_never().is_none());
    let current = sync.store.snapshot();
    let state = &current.environments[&sync.key.environment].threads[&sync.key.thread];
    assert_eq!(
        state.projection.as_ref().unwrap().visible_turn_items.len(),
        1
    );
    assert_eq!(state.history.cursor.as_deref(), Some("cursor"));
    assert!(state.history.expanded && state.history.loading);
    let requests = sync.source.history_requests.borrow();
    assert_eq!(requests[0].view, Some(HistoryView::Conversation));
    assert_eq!(requests[1].view, None);
    assert_eq!(requests[0].through_entry.as_deref(), Some("item-0"));
    drop(requests);
    reply.send(serde_json::from_value(json!({"snapshotSequence":2,"items":[row(0)?,row(1)?],"nextCursor":null,"hasMoreHistory":false}))?).unwrap();
    assert_eq!(block_on(history)?, HistoryLoadResult::Loaded);
    let current = sync.store.snapshot();
    let state = &current.environments[&sync.key.environment].threads[&sync.key.thread];
    assert_eq!(
        state.projection.as_ref().unwrap().visible_turn_items.len(),
        2
    );
    assert_eq!(state.history.cursor, None);
    assert!(!state.history.loading && !state.history.has_more);
    Ok(())
}

#[test]
fn history_controller_old_finalizer_cannot_unregister_successor() -> Result<(), ClientError> {
    use std::cell::Cell;
    use t3_client::state::history::{HistoryController, HistoryHandler};
    struct Handler(Rc<Cell<u32>>);
    impl HistoryHandler for Handler {
        fn load_earlier(
            &self,
            _: Option<String>,
            _: Option<String>,
        ) -> LocalBoxFuture<'_, Result<HistoryLoadResult, ClientError>> {
            self.0.set(self.0.get() + 1);
            futures::future::ready(Ok(HistoryLoadResult::Loaded)).boxed_local()
        }
        fn reconcile_pending_work(&self) -> LocalBoxFuture<'_, Result<(), ClientError>> {
            futures::future::ready(Ok(())).boxed_local()
        }
    }
    let controller = HistoryController::default();
    let key = ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread".into()),
    };
    let calls = Rc::new(Cell::new(0));
    let first = controller.register(key.clone(), Rc::new(Handler(Rc::new(Cell::new(0)))));
    let successor = controller.register(key.clone(), Rc::new(Handler(calls.clone())));
    drop(first);
    assert_eq!(
        block_on(controller.load_earlier(&key, None, None))?,
        HistoryLoadResult::Loaded
    );
    assert_eq!(calls.get(), 1);
    drop(successor);
    assert_eq!(
        block_on(controller.load_earlier(&key, None, None))?,
        HistoryLoadResult::Noop
    );
    Ok(())
}

#[test]
fn unknown_shell_delivery_does_not_consume_cursor_or_block_later_known_events()
-> Result<(), ClientError> {
    let store = ClientStore::default();
    let environment = EnvironmentId("env".into());
    let session = store.begin_session(environment.clone());
    let rpc = Rc::new(ScriptedRpc::default());
    rpc.scripts.borrow_mut().push_back(RpcScript {
        method: "orchestration.subscribeShell".into(), input: json!({"requestCompletionMarker":true}),
        replies: vec![Ok(json!({"kind":"snapshot","snapshot":shell(0)?})), Ok(json!({"kind":"future.event","sequence":99})), Ok(json!({"kind":"thread.removed","sequence":5,"location":"active","threadId":"thread-v2"})), Ok(json!({"kind":"synchronized"}))],
    });
    let source = Rc::new(ScriptedSnapshots::default());
    source.push_shell(futures::future::ready(Ok(ShellLoadResult::Unavailable)).boxed_local());
    let sync = ShellSync::new(
        store.clone(),
        environment.clone(),
        session,
        Rc::new(EnvironmentCacheStore::new(FakePlatform::default())),
        rpc,
        source,
        None,
    );
    block_on(sync.follow())?;
    let snapshot = store.snapshot();
    let shell = &snapshot.environments[&environment].shell;
    assert_eq!(shell.status, SyncStatus::Live);
    assert_eq!(shell.resume_sequence(), Some(5));
    assert!(shell.snapshot.as_ref().unwrap().threads.is_empty());
    Ok(())
}

#[test]
fn environment_subscriptions_wake_only_for_their_environment() {
    use futures::{FutureExt, StreamExt};
    let store = ClientStore::default();
    let a = EnvironmentId("a".into());
    let b = EnvironmentId("b".into());
    let mut only_a = store.subscribe_environment(&a);
    let mut everything = store.subscribe();
    assert!(
        only_a.next().now_or_never().flatten().is_some(),
        "initial revision"
    );
    assert!(everything.next().now_or_never().flatten().is_some());
    let session = store.begin_session(b.clone());
    store.update_thread(
        &ThreadKey {
            environment: b.clone(),
            thread: ThreadId("t".into()),
        },
        session,
        |_| true,
    );
    assert!(
        only_a.next().now_or_never().is_none(),
        "b's changes do not wake a"
    );
    assert!(everything.next().now_or_never().flatten().is_some());
    store.begin_session(a.clone());
    let change = only_a.next().now_or_never().flatten().unwrap();
    assert!(
        change
            .scopes
            .contains(&t3_client::store::Scope::Environment(a))
    );
}

#[test]
fn a_visit_record_changes_only_the_read_state() -> Result<(), ClientError> {
    let mut p = projection()?;
    let before = p.clone();
    let visit = |payload: Value| event("thread.visit-recorded", payload);
    assert!(apply_event(
        &mut p,
        visit(json!({"lastVisitedAt":"2026-06-21T00:00:00.000Z"}))?,
        &HistoryMeta::default()
    )?);
    assert_eq!(
        p.thread.last_visited_at.as_ref().map(|at| at.0.as_str()),
        Some("2026-06-21T00:00:00.000Z")
    );
    assert_eq!(p.thread.opens, before.thread.opens, "absent opens are kept");
    assert_eq!(p.thread.title, before.thread.title);
    assert_eq!(p.updated_at, before.updated_at, "a visit is not activity");
    let opens = json!({"openedAt":5,"score":1,"picks":[]});
    apply_event(
        &mut p,
        visit(json!({"lastVisitedAt":null,"opens":opens}))?,
        &HistoryMeta::default(),
    )?;
    assert_eq!(p.thread.last_visited_at, None);
    assert_eq!(serde_json::to_value(&p.thread.opens)?, opens);
    assert!(
        !apply_event(
            &mut p,
            visit(json!({"lastVisitedAt":null}))?,
            &HistoryMeta::default()
        )?,
        "the same read state changes nothing"
    );
    Ok(())
}
