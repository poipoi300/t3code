#![allow(clippy::unwrap_used)]
#[path = "tests_continuation.rs"]
mod continuation;
use super::*;
use crate::types::{CommandId, EnvironmentId, ThreadId, ThreadKey};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{
    base_schemas::{NodeId, RunId},
    orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
    wire::WireDateTime,
};

fn thread_json(id: &str) -> Value {
    json!({"id":id,"projectId":"project","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"model"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":id,"parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":"2026-06-20T00:00:00.000Z","updatedAt":"2026-06-20T00:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null,"latestRunId":null,"activeRunId":null,"status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,"hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0})
}
fn projection() -> OrchestrationV2ThreadProjection {
    serde_json::from_value(json!({"thread":thread_json("thread"),"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":"2026-06-20T00:00:00.000Z"})).unwrap()
}
fn run(id: &str, ordinal: u64, status: &str) -> OrchestrationV2Run {
    serde_json::from_value(json!({"id":id,"threadId":"thread","ordinal":ordinal,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"model"},"providerThreadId":null,"userMessageId":format!("message-{id}"),"rootNodeId":null,"activeAttemptId":null,"status":status,"requestedAt":"2026-06-20T00:00:00.000Z","startedAt":if status == "queued" {Value::Null} else {json!("2026-06-20T00:00:00.000Z")},"completedAt":null,"checkpointId":null,"contextHandoffId":null})).unwrap()
}
fn item(kind: &str, status: &str, extra: Value) -> OrchestrationV2TurnItem {
    let mut raw = json!({"id":"item","threadId":"thread","runId":"run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"type":kind,"status":status,"title":null,"startedAt":"2026-06-20T00:00:00.000Z","completedAt":null,"updatedAt":"2026-06-20T00:00:00.000Z"});
    if kind == "user_message" {
        raw.as_object_mut().unwrap().extend(
            json!({"createdBy":"user","creationSource":"web","inputIntent":"turn_start"})
                .as_object()
                .unwrap()
                .clone(),
        );
    }
    raw.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(raw).unwrap()
}
fn parse(date: &WireDateTime) -> i64 {
    date.0.parse().unwrap_or(0)
}
fn key(e: &str, t: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId(e.into()),
        thread: ThreadId(t.into()),
    }
}

#[test]
fn workspace_preparation_visibility_and_retry_eligibility() {
    use turn_item_presentation::*;
    let failure = |status, code| {
        item(
            "error",
            status,
            json!({"failure":{"class":"validation_error","message":"fetch failed","code":code,"retryable":false}}),
        )
    };
    assert!(!turn_item_is_workspace_preparation(&failure(
        "failed",
        "workspace_preparation_failed"
    )));
    assert!(turn_item_is_workspace_preparation(&failure(
        "cancelled",
        "workspace_preparation_failed"
    )));
    assert!(!turn_item_is_workspace_preparation(&failure(
        "cancelled",
        "provider_crashed"
    )));
    assert!(turn_item_is_workspace_preparation(&item(
        "command_execution",
        "completed",
        json!({"input":"Preparing workspace"})
    )));
    let mut r = run("run", 1, "failed");
    r.workspace_preparation = Some(Some(
        serde_json::from_value(json!({"type":"worktree","baseRef":"main"})).unwrap(),
    ));
    assert_eq!(
        workspace_preparation_retry_run_ids(
            &[r.clone()],
            &[failure("failed", "workspace_preparation_failed")]
        ),
        vec![r.id.clone()]
    );
    r.status = OrchestrationV2RunStatus::Preparing;
    assert!(
        workspace_preparation_retry_run_ids(
            &[r.clone()],
            &[failure("failed", "workspace_preparation_failed")]
        )
        .is_empty()
    );
    r.status = OrchestrationV2RunStatus::Failed;
    r.workspace_preparation = None;
    assert!(
        workspace_preparation_retry_run_ids(
            &[r.clone()],
            &[failure("failed", "workspace_preparation_failed")]
        )
        .is_empty()
    );
    assert!(
        workspace_preparation_retry_run_ids(&[r], &[failure("failed", "provider_crashed")])
            .is_empty()
    );
}

#[test]
fn lifecycle_updates_reconcile_in_order_and_fail_independently() {
    use lifecycle::*;
    let env = EnvironmentId("env".into());
    let first = CommandId("one".into());
    let second = CommandId("two".into());
    let mut life = OptimisticThreadLifecycle::default();
    let mut snapshot:OrchestrationV2ShellSnapshot=serde_json::from_value(json!({"schemaVersion":1,"snapshotSequence":0,"projects":[],"threads":[thread_json("thread")],"archivedThreads":[]})).unwrap();
    life.begin(
        env.clone(),
        ThreadId("thread".into()),
        first.clone(),
        |s, accepted| {
            let mut s = s.clone();
            s.title = if accepted { "accepted" } else { "pending" }.into();
            s
        },
    );
    life.begin(
        env.clone(),
        ThreadId("thread".into()),
        second.clone(),
        |s, _| {
            let mut s = s.clone();
            s.title.push('!');
            s
        },
    );
    assert_eq!(
        life.snapshot(&env, &snapshot).unwrap().threads[0].title,
        "pending!"
    );
    assert!(
        life.snapshot(&EnvironmentId("other".into()), &snapshot)
            .is_none()
    );
    life.accept(&env, &first, 4);
    assert_eq!(
        life.snapshot(&env, &snapshot).unwrap().threads[0].title,
        "accepted!"
    );
    life.reject(&env, &second);
    assert_eq!(life.pending_thread_ids(&env).len(), 1);
    snapshot.snapshot_sequence = 4;
    assert!(life.snapshot(&env, &snapshot).is_none());
    assert_eq!(life.pending_thread_ids(&env).len(), 1);
    life.reconcile(&env, Some(&snapshot));
    assert!(life.pending_thread_ids(&env).is_empty());
    life.begin(
        env.clone(),
        ThreadId("thread".into()),
        second.clone(),
        |s, _| s.clone(),
    );
    life.accept(&env, &second, 10);
    life.reconcile(&env, None);
    assert!(life.pending_thread_ids(&env).is_empty());
}

#[test]
fn extension_statuses_follow_selected_current_session() {
    use execution::*;
    let mut p = projection();
    p.thread.active_provider_thread_id = Some(IdOrIndex::Id(
        t3_contracts::base_schemas::ProviderThreadId("active".into()),
    ));
    p.provider_threads=vec![serde_json::from_value(json!({"id":"active","driver":"codex","providerInstanceId":"codex","providerSessionId":"session","appThreadId":"thread","ownerNodeId":null,"nativeThreadRef":null,"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":null,"lastRunOrdinal":null,"handoffIds":[],"forkedFrom":null,"createdAt":"0","updatedAt":"0"})).unwrap()];
    // Use the exported capability fixture so this test follows the wire contract's additions.
    let mut raw: Value = serde_json::from_str(
        include_str!("../../../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    raw["id"] = json!("session");
    raw["providerInstanceId"] = json!("codex");
    raw["status"] = json!("ready");
    raw["extensionStatuses"] = json!([{"key":"watch","text":"Watching"}]);
    let session: OrchestrationV2ProviderSession = serde_json::from_value(raw).unwrap();
    p.provider_sessions = vec![session.clone()];
    assert_eq!(derive_provider_extension_statuses(Some(&p)).len(), 1);
    let mut other = session.clone();
    other.id = IdOrIndex::Id(t3_contracts::base_schemas::ProviderSessionId(
        "other".into(),
    ));
    other.extension_statuses = Some(Some(vec![]));
    p.provider_sessions.push(other);
    assert_eq!(derive_provider_extension_statuses(Some(&p)).len(), 1);
    p.provider_sessions[0].status = ProviderSessionStatus::Stopped;
    assert!(derive_provider_extension_statuses(Some(&p)).is_empty());
    p.provider_sessions[0].status = ProviderSessionStatus::Error;
    assert_eq!(derive_provider_extension_statuses(Some(&p)).len(), 1);
    p.thread.model_selection.instance_id =
        t3_contracts::provider_instance::ProviderInstanceId("other".into());
    assert!(derive_provider_extension_statuses(Some(&p)).is_empty());
    assert!(derive_provider_extension_statuses(None).is_empty());
}

#[test]
fn activity_latest_and_interruptible_runs_are_distinct() {
    use execution::*;
    let mut p = projection();
    p.runs = vec![run("queued", 2, "queued"), run("active", 1, "running")];
    assert_eq!(
        derive_latest_thread_run(&p, &parse).unwrap().run_id,
        p.runs[0].id
    );
    assert_eq!(
        derive_thread_activity_run(&p, &parse).unwrap().run_id,
        p.runs[1].id
    );
    let runtime = derive_thread_structure_runtime(&p, &parse).unwrap();
    assert!(thread_runtime_has_interruptible_run(Some(&runtime)));
    assert_eq!(runtime.status, Some(OrchestrationV2RunStatus::Running));
    p.runs.pop();
    let runtime = derive_thread_structure_runtime(&p, &parse).unwrap();
    assert!(!thread_runtime_has_interruptible_run(Some(&runtime)));
}

#[test]
fn held_queues_keep_the_stopped_run_and_root_limit_error_visible() {
    use execution::*;
    let mut p = projection();
    let mut failed = run("run", 1, "failed");
    failed.completed_at = Some(WireDateTime("1".into()));
    failed.root_node_id = Some(IdOrIndex::Id(NodeId("root".into())));
    let mut held = run("held", 2, "queued");
    held.queue_held = Some(Some(true));
    let error = item(
        "error",
        "failed",
        json!({"nodeId":"root","failure":{"class":"usage_limit","message":"Plan limit reached","code":"usageLimitExceeded","retryable":null}}),
    );
    p.runs = vec![failed.clone(), held.clone()];
    p.turn_items = vec![error];
    let runtime = derive_thread_structure_runtime(&p, &parse).unwrap();
    assert_eq!(runtime.status, Some(OrchestrationV2RunStatus::Failed));
    assert_eq!(runtime.last_error.as_deref(), Some("Plan limit reached"));
    assert_eq!(
        derive_resumable_run_id(&p, Some(&runtime), &parse),
        Some(failed.id.clone())
    );
    if let OrchestrationV2TurnItem::Error(i) = &mut p.turn_items[0] {
        i.node_id = Some(IdOrIndex::Id(NodeId("child".into())));
    }
    assert_eq!(
        derive_thread_structure_runtime(&p, &parse)
            .unwrap()
            .last_error,
        None
    );
    failed.status = OrchestrationV2RunStatus::Interrupted;
    p.runs[0] = failed.clone();
    assert_eq!(
        derive_latest_thread_run(&p, &parse).unwrap().run_id,
        failed.id
    );
    p.runs = vec![held];
    assert!(derive_latest_thread_run(&p, &parse).is_none());
    assert!(derive_thread_structure_runtime(&p, &parse).is_none());
}

#[test]
fn resume_checks_pending_requests_only_for_a_continuation() {
    let request = |status| {
        serde_json::from_value(json!({"id":"request","nodeId":"node","providerTurnId":null,"nativeRequestRef":null,"kind":"command","status":status,"responseCapability":{"type":"not_resumable","reason":"ended"},"createdAt":"0","resolvedAt":null})).unwrap()
    };
    let id = IdOrIndex::Id(RunId("run".into()));
    for (requests, resumable, held, expected) in [
        (vec![], Some(&id), false, true),
        (vec![request("pending")], Some(&id), false, false),
        (vec![request("resolved")], Some(&id), false, true),
        (vec![request("pending")], Some(&id), true, false),
        (vec![request("pending")], None, true, true),
        (vec![], None, false, false),
    ] {
        assert_eq!(
            execution::can_resume_thread(&requests, resumable, held),
            expected
        );
    }
}

#[test]
fn stop_presentation_requires_unanswered_interrupt_for_selected_run() {
    use execution::*;
    let mut p = projection();
    p.runs = vec![run("run", 1, "running")];
    let request = item(
        "run_interrupt_request",
        "completed",
        json!({"message":"Stopped"}),
    );
    assert!(present_run_stop(&p).is_none());
    p.turn_items.push(request.clone());
    assert_eq!(present_run_stop(&p).unwrap().state, StopState::Stopping);
    if let OrchestrationV2TurnItem::RunInterruptRequest(r) = &mut p.turn_items[0] {
        r.unresponsive_at = Some(WireDateTime("1".into()));
    }
    assert_eq!(present_run_stop(&p).unwrap().state, StopState::Unresponsive);
    p.runs[0].status = OrchestrationV2RunStatus::Completed;
    p.runs.push(run("queued", 2, "queued"));
    assert!(present_run_stop(&p).unwrap().background);
    p.turn_items.push(item(
        "run_interrupt_result",
        "interrupted",
        json!({"message":"Stopped"}),
    ));
    assert!(present_run_stop(&p).is_none());
    p.turn_items = vec![request];
    assert!(present_run_stop(&p).is_none());
}

#[test]
fn background_stop_acceptance_waits_for_original_tasks_or_retry_deadline() {
    use execution::*;
    let thread = key("env", "thread");
    let mut r = BackgroundWorkStopRequest {
        thread: thread.clone(),
        task_ids: vec!["dev-server".into()],
        retry_at: None,
    };
    assert!(is_background_work_stop_pending(
        Some(&r),
        &thread,
        &[],
        false
    ));
    r.retry_at = Some(10000);
    assert!(is_background_work_stop_pending(
        Some(&r),
        &thread,
        &["dev-server"],
        false
    ));
    assert!(!is_background_work_stop_pending(
        Some(&r),
        &thread,
        &["next"],
        false
    ));
    assert!(!is_background_work_stop_pending(
        Some(&r),
        &thread,
        &["dev-server"],
        true
    ));
    assert!(!is_background_work_stop_pending(
        Some(&r),
        &key("else", "thread"),
        &["dev-server"],
        false
    ));
    assert!(!is_background_work_stop_pending(None, &thread, &[], false));
}

#[test]
fn goal_token_rounding_rolls_thousands_into_millions() {
    use execution_presentation::*;
    for (used, expected) in [
        (999499, "999k"),
        (999500, "1m"),
        (999999, "1m"),
        (1000000, "1m"),
    ] {
        let goal = serde_json::from_value(
            json!({"objective":"Ship","status":"active","tokensUsed":used,"tokenBudget":2000000}),
        )
        .unwrap();
        assert_eq!(
            present_provider_goal(&goal, true, |s| s.to_string()).usage,
            Some(format!("{expected} / 2m tokens"))
        );
        assert_eq!(
            present_provider_goal(&goal, false, |s| s.to_string()).title,
            "Goal set"
        );
    }
}

#[test]
fn effort_labels_preserve_legacy_ultracode_semantics() {
    let selection=serde_json::from_value(json!({"instanceId":"claudeAgent","model":"claude-opus-5-5","options":[{"id":"effort","value":"ultracode"}]})).unwrap();
    let models=vec![serde_json::from_value(json!({"slug":"claude-opus-5-5","name":"Claude Opus 5.5","isCustom":false,"capabilities":{"optionDescriptors":[{"id":"effort","label":"Reasoning","type":"select","options":[{"id":"high","label":"High","isDefault":true},{"id":"xhigh","label":"Extra High"}]},{"id":"ultracode","label":"Ultracode","type":"boolean"}]}})).unwrap()];
    assert_eq!(
        execution_presentation::format_model_selection_effort(&selection, &models, None).as_deref(),
        Some("Extra High")
    );
}

#[test]
fn sort_dates_preserve_authored_null_and_latest_activity_fallback() {
    use sort::*;
    let parse = |s: &str| s.parse::<i64>().ok();
    let mut input = ThreadSortInput {
        created_at: "1",
        updated_at: "20",
        latest_user_message_at: Some("30"),
        ..Default::default()
    };
    assert_eq!(
        get_thread_sort_timestamp(&input, SortOrder::CreatedAt, &parse),
        Some(1)
    );
    assert_eq!(
        get_thread_sort_timestamp(&input, SortOrder::LatestUserMessage, &parse),
        Some(30)
    );
    input.latest_user_authored_message_at = Some(None);
    assert_eq!(
        get_thread_sort_timestamp(&input, SortOrder::LatestUserMessage, &parse),
        Some(1)
    );
    input.latest_user_authored_message_at = None;
    input.latest_user_message_at = Some("invalid");
    input.messages = &[("user", "5"), ("assistant", "30"), ("user", "10")];
    assert_eq!(
        get_thread_sort_timestamp(&input, SortOrder::LatestUserMessage, &parse),
        Some(10)
    );
    assert_eq!(
        resolve_settled_thread_timestamp(Some("2"), Some("10"), &[Some("30")], "40", &parse),
        Some("2")
    );
    assert_eq!(
        resolve_settled_thread_timestamp(Some("invalid"), Some("10"), &[Some("30")], "40", &parse),
        Some("30")
    );
    assert_eq!(
        resolve_thread_activity_timestamp(None, &[], "40", &parse),
        Some("40")
    );
}

#[test]
fn spread_keys_are_unique_sorted_and_insertable_at_every_boundary() {
    use sort::*;
    for count in [0, 1, 650, 675, 676, 1001, 2000] {
        let keys = generate_spread_pin_order_keys(count);
        assert_eq!(keys.len(), count);
        assert_eq!(keys.iter().collect::<BTreeSet<_>>().len(), count);
        for (i, key) in keys.iter().enumerate() {
            let before = i.checked_sub(1).map(|i| keys[i].as_str());
            let between = pin_order_key_between(before, Some(key)).unwrap();
            assert!(between < *key);
            assert!(before.is_none_or(|b| between.as_str() > b));
            assert!(!key.ends_with('a'));
        }
    }
    for (a, b) in [
        (Some("a"), None),
        (Some("BAD"), None),
        (Some("z"), Some("b")),
    ] {
        assert!(pin_order_key_between(a, b).is_none());
    }
}

#[test]
fn pinned_reorder_reserves_hidden_slots_and_materializes_large_lists() {
    use sort::*;
    let reserved = pin_order_key_between(Some("f"), Some("t")).unwrap();
    let keys = BTreeMap::from([
        ("a".into(), Some("f".into())),
        ("b".into(), Some("t".into())),
        ("moved".into(), Some("z".into())),
        ("hidden".into(), Some(reserved.clone())),
    ]);
    let writes = plan_pinned_reorder(&["a".into(), "moved".into(), "b".into()], &keys, "moved");
    assert_eq!(writes.len(), 1);
    assert_ne!(writes[0].1, reserved);
    assert!(writes[0].1.as_str() > "f" && writes[0].1.as_str() < "t");
    let ids: Vec<_> = (0..1200).rev().map(|i| i.to_string()).collect();
    let writes = plan_pinned_reorder(&ids, &BTreeMap::new(), &ids[0]);
    assert_eq!(
        writes.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        ids.iter().collect::<Vec<_>>()
    );
    assert!(writes.windows(2).all(|p| p[0].1 < p[1].1));
}

#[test]
fn active_and_pinned_sort_keep_environment_ties_deterministic() {
    use sort::*;
    let keys = [key("b", "a"), key("a", "a"), key("a", "b")];
    for order in [None, Some("m")] {
        let mut rows: Vec<_> = keys
            .iter()
            .map(|key| OrderedThread {
                key,
                created_at: Some(10),
                unsettled_at: None,
                order_key: order,
            })
            .collect();
        sort_active_threads(&mut rows);
        assert_eq!(
            rows.iter().map(|r| r.key).collect::<Vec<_>>(),
            vec![&keys[1], &keys[0], &keys[2]]
        );
    }
}

#[test]
fn subagent_preview_is_bounded_unicode_aware_and_prefers_status_specific_text() {
    use subagent_display::*;
    assert_eq!(
        subagent_detail_preview(
            &OrchestrationV2TurnItemStatus::Running,
            Some("Found two\n problems"),
            Some("Reading files")
        )
        .as_deref(),
        Some("Reading files")
    );
    assert_eq!(
        subagent_detail_preview(
            &OrchestrationV2TurnItemStatus::Completed,
            Some("Found two\n problems"),
            Some("Reading files")
        )
        .as_deref(),
        Some("Found two problems")
    );
    assert_eq!(
        subagent_detail_preview(
            &OrchestrationV2TurnItemStatus::Failed,
            Some(" "),
            Some("Last progress")
        )
        .as_deref(),
        Some("Last progress")
    );
    assert!(subagent_detail_preview(&OrchestrationV2TurnItemStatus::Pending, None, None).is_none());
    for n in [279, 280, 281] {
        let text = "x".repeat(n);
        assert_eq!(
            subagent_detail_preview(&OrchestrationV2TurnItemStatus::Completed, Some(&text), None)
                .unwrap(),
            if n > 280 {
                format!("{}…", "x".repeat(280))
            } else {
                text
            }
        );
    }
    assert_eq!(
        subagent_detail_preview(
            &OrchestrationV2TurnItemStatus::Running,
            None,
            Some(&format!(" \u{a0}Reading\tfiles\n{}", "x".repeat(1_000_000)))
        )
        .unwrap(),
        format!("Reading files {}…", "x".repeat(266))
    );
    assert_eq!(
        subagent_detail_preview(
            &OrchestrationV2TurnItemStatus::Failed,
            Some(&format!("{}😀", "x".repeat(279))),
            None
        )
        .unwrap(),
        format!("{}…", "x".repeat(279))
    );
    assert_eq!(
        subagent_detail_preview(
            &OrchestrationV2TurnItemStatus::Running,
            Some(" Done "),
            Some("\u{feff}\u{2029}")
        )
        .as_deref(),
        Some("Done")
    );
    assert_eq!(
        format_subagent_display_title("Subagent: /root/parent/fix_the_bug/"),
        "Fix The Bug"
    );
}

#[test]
fn environment_presentations_preserve_order_filter_enabled_and_reuse_quiet_values() {
    use crate::environment::EnvironmentRegistration;
    use presentation::*;
    let entry = |id: &str, enabled| {
        Rc::new(EnvironmentRegistration {
            id: EnvironmentId(id.into()),
            label: id.into(),
            routes: vec![],
            enabled,
        })
    };
    let mut config: t3_contracts::server::ServerConfig = serde_json::from_str(
        include_str!("../../../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let mut views = EnvironmentPresentations::default();
    let first = entry("first", true);
    let second = entry("second", false);
    views.update([
        PresentationInput {
            entry: first.clone(),
            unsupported_reason: None,
            state: None,
            config: Some(Rc::new(config.clone())),
        },
        PresentationInput {
            entry: second.clone(),
            unsupported_reason: None,
            state: None,
            config: Some(Rc::new(config.clone())),
        },
    ]);
    assert_eq!(views.ordered_ids, vec![first.id.clone(), second.id.clone()]);
    assert_eq!(views.presentations.len(), 2);
    assert_eq!(views.enabled_with_providers.len(), 1);
    assert!(views.pull_requests_supported());
    assert_eq!(
        views.presentations[&first.id].connection.phase,
        ConnectionPhase::Connecting
    );
    assert_eq!(
        views.presentations[&second.id].connection.phase,
        ConnectionPhase::Available
    );
    let quiet = views.presentations[&first.id].clone();
    config.providers.0.clear();
    views.update([
        PresentationInput {
            entry: first.clone(),
            unsupported_reason: None,
            state: None,
            config: Some(Rc::new(config)),
        },
        PresentationInput {
            entry: second.clone(),
            unsupported_reason: Some("Server too old"),
            state: None,
            config: None,
        },
    ]);
    assert!(Rc::ptr_eq(&quiet, &views.presentations[&first.id]));
    assert_eq!(
        views.presentations[&second.id].connection.phase,
        ConnectionPhase::Unsupported
    );
    views.update([PresentationInput {
        entry: second,
        unsupported_reason: None,
        state: None,
        config: None,
    }]);
    assert!(!views.pull_requests_supported());
    assert_eq!(views.identities()[0].1, "second");
    assert_eq!(views.presentations.len(), 1);
    views.update([]);
    assert!(views.ordered_ids.is_empty());
    assert!(views.presentations.is_empty());
    assert!(views.summaries.is_empty());
}

#[test]
fn runless_provider_subagents_require_a_native_child_and_runless_root() {
    use execution::*;
    let mut p = projection();
    p.thread.creation_source = OrchestrationV2CreationSource::Provider;
    p.thread.lineage.parent_thread_id = Some(ThreadId("parent".into()));
    p.thread.lineage.relationship_to_parent = Some(AppThreadLineageRelationshipToParent::Subagent);
    let node=serde_json::from_value(json!({"id":"root","threadId":"thread","runId":null,"parentNodeId":null,"rootNodeId":"root","kind":"root_turn","status":"running","countsForRun":false,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"runtimeRequestId":null,"checkpointScopeId":null,"startedAt":"10","completedAt":null})).unwrap();
    p.nodes = vec![node];
    assert_eq!(
        derive_runless_work_started_at(&p),
        Some(WireDateTime("10".into()))
    );
    assert!(derive_thread_structure_runtime(&p, &parse).is_none());
    p.nodes[0].run_id = Some(IdOrIndex::Id(RunId("run".into())));
    assert!(derive_runless_work_started_at(&p).is_none());
    p.nodes[0].run_id = None;
    p.thread.creation_source = OrchestrationV2CreationSource::Mcp;
    assert!(derive_provider_subagent_status(&p).is_none());
}

#[test]
fn provider_turn_starting_and_ended_labels_track_current_attempt() {
    use execution::*;
    let mut p = projection();
    let mut r = run("run", 1, "running");
    r.active_attempt_id = Some(IdOrIndex::Id(t3_contracts::base_schemas::RunAttemptId(
        "attempt".into(),
    )));
    p.runs = vec![r];
    assert_eq!(
        derive_thread_activity_run(&p, &parse)
            .unwrap()
            .provider_starting_label
            .as_deref(),
        Some("Starting")
    );
    let turn=serde_json::from_value(json!({"id":"turn","providerThreadId":"provider","nodeId":"root","runAttemptId":"attempt","nativeTurnRef":null,"ordinal":1,"status":"running","startedAt":"0","completedAt":null})).unwrap();
    p.provider_turns = vec![turn];
    let summary = derive_thread_activity_run(&p, &parse).unwrap();
    assert!(!summary.turn_ended);
    assert!(summary.provider_starting_label.is_none());
    p.provider_turns[0].status = ProviderTurnStatus::Failed;
    assert!(derive_thread_activity_run(&p, &parse).unwrap().turn_ended);
    p.runs[0].work_started_at = Some(Some(WireDateTime("-100".into())));
    assert_eq!(
        derive_thread_structure_runtime(&p, &parse)
            .unwrap()
            .activity_started_at,
        Some(WireDateTime("-100".into()))
    );
}
