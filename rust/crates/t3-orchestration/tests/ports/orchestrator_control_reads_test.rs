#![allow(clippy::unwrap_used)]

use super::{
    command_policy_test::{change, command, context, projection, run},
    orchestrator_receipt_race_test::{Harness, Recovery},
};
use serde_json::json;
use t3_orchestration::{
    api::*,
    decision::engine::{PureDecisionEngine, required_records},
    ports::decisions::orchestrator::{derived_id, epoch_millis, iso_from_epoch_millis},
    testing::TEST_TIME,
};

#[test]
fn existing_ids_including_tombstones_cannot_be_recreated() {
    let create = command(
        json!({"type":"thread.create","commandId":"create","threadId":"thread","projectId":"project","title":"Thread","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
    );
    for deleted in [false, true] {
        let mut projection = projection();
        if deleted {
            projection.thread = change(&projection.thread, json!({"deletedAt":TEST_TIME}));
        }
        assert!(
            PureDecisionEngine
                .decide(&context(Some(projection)), &create)
                .unwrap_err()
                .to_string()
                .ends_with("Thread thread already exists. Use a new thread id.")
        );
    }
}

#[tokio::test]
async fn visits_move_forward_only_and_do_not_change_activity_time_or_load_history() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    harness.clock.set("2026-09-05T00:00:00.000Z");
    for (id, visited) in [
        ("first", "2026-09-04T00:00:00.000Z"),
        ("backwards", "2026-09-03T00:00:00.000Z"),
    ] {
        let visit = json!({"type":"thread.visit","commandId":id,"threadId":"thread","visitedAt":visited,"opened":false});
        assert!(
            required_records(&command(visit.clone()))
                .unwrap()
                .0
                .is_empty()
        );
        harness.dispatch(visit).await.unwrap();
    }
    let thread = harness
        .reader
        .thread(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        thread.last_visited_at.unwrap().0,
        "2026-09-04T00:00:00.000Z"
    );
    assert_eq!(thread.updated_at.0, TEST_TIME);
}

#[tokio::test]
async fn explicit_settlement_clears_snooze_and_replays_without_clearing_a_later_snooze() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    let snooze = |id: &str| json!({"type":"thread.snooze","commandId":id,"threadId":"thread","snoozedUntil":"2026-09-07T00:00:00.000Z"});
    harness.dispatch(snooze("snooze")).await.unwrap();
    let settle = json!({"type":"thread.settle","commandId":"settle","threadId":"thread"});
    let receipt = harness.dispatch(settle.clone()).await.unwrap();
    let settled = harness
        .reader
        .thread(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(settled.snoozed_until.flatten().is_none());
    harness.dispatch(snooze("resnooze")).await.unwrap();
    assert_eq!(harness.dispatch(settle).await.unwrap(), receipt);
    assert!(
        harness
            .reader
            .thread(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap()
            .snoozed_until
            .flatten()
            .is_some()
    );
}

#[test]
fn settlement_preconditions_preserve_blocker_priority() {
    let mut projection = projection();
    projection.runs = vec![run("active", 1, "running")];
    let settle = command(json!({"type":"thread.settle","commandId":"settle","threadId":"thread"}));
    let error = PureDecisionEngine
        .decide(&context(Some(projection.clone())), &settle)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("Thread thread is still running. Stop it before settling.")
    );
    let stale = command(
        json!({"type":"thread.settle","commandId":"settle","threadId":"thread","expectedLatestRunId":"older"}),
    );
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection)), &stale)
            .unwrap_err()
            .to_string()
            .ends_with("The thread received another run before settlement.")
    );
}

#[tokio::test]
async fn pin_reorder_cannot_restore_a_raced_unpin_and_tagging_preserves_activity_time() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    harness.clock.set("2026-09-04T00:00:00.000Z");
    harness
        .dispatch(json!({"type":"thread.pin","commandId":"pin","threadId":"thread","orderKey":"a"}))
        .await
        .unwrap();
    harness
        .dispatch(json!({"type":"thread.unpin","commandId":"unpin","threadId":"thread"}))
        .await
        .unwrap();
    assert!(harness.dispatch(json!({"type":"thread.pin.reorder","commandId":"reorder","threadId":"thread","orderKey":"b"})).await.unwrap_err().to_string().contains("is not pinned and cannot be reordered"));
    let before = harness
        .reader
        .thread(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    let tag = serde_json::from_str::<serde_json::Value>(
        include_str!("../../../../fixtures/samples/ThreadTag.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    harness.clock.set("2026-09-05T00:00:00.000Z");
    harness.dispatch(json!({"type":"thread.tag.set","commandId":"tag","threadId":"thread","tag":tag,"tagged":true})).await.unwrap();
    let after = harness
        .reader
        .thread(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.updated_at, before.updated_at);
}

#[test]
fn metadata_guards_are_checked_before_planning_and_simple_renames_need_no_transcript() {
    let rename = command(
        json!({"type":"thread.metadata.update","commandId":"rename","threadId":"thread","title":"After"}),
    );
    assert!(required_records(&rename).unwrap().0.is_empty());
    let stale = command(
        json!({"type":"thread.metadata.update","commandId":"rename","threadId":"thread","expectedWorktreePath":"/old","title":"After"}),
    );
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection())), &stale)
            .unwrap_err()
            .to_string()
            .ends_with(
                "Thread thread worktree changed before the metadata update could be applied."
            )
    );
    let mut busy = projection();
    busy.runs = vec![run("one", 1, "queued")];
    let not_empty = command(
        json!({"type":"thread.metadata.update","commandId":"rename","threadId":"thread","expectedEmpty":true,"title":"After"}),
    );
    assert!(
        PureDecisionEngine
            .decide(&context(Some(busy)), &not_empty)
            .unwrap_err()
            .to_string()
            .ends_with("Thread thread is no longer empty.")
    );
}

#[test]
fn dates_compare_offsets_and_derived_ids_match_encode_uri_component() {
    for (date, millis) in [
        ("-000001-01-01T00:00:00.000Z", -62198755200000),
        ("+010000-01-01T00:00:00.000Z", 253402300800000),
        ("+275760-09-13T00:00:00.000Z", 8640000000000000),
        ("-271821-04-20T00:00:00.000Z", -8640000000000000),
    ] {
        assert_eq!(epoch_millis(date), Some(millis), "{date}");
        assert_eq!(iso_from_epoch_millis(millis), date);
    }
    for invalid in [
        "-000000-01-01T00:00:00.000Z",
        "+275760-09-13T00:00:00.001Z",
        "2026-09-03T00:00:00+01:-1",
    ] {
        assert!(epoch_millis(invalid).is_none(), "{invalid}");
    }
    assert_eq!(
        epoch_millis("2026-09-03T00:00:00.000Z"),
        epoch_millis("2026-09-03T01:00:00+01:00")
    );
    assert!(epoch_millis("2026-02-29T00:00:00.000Z").is_none());
    assert!(epoch_millis("2024-02-29T00:00:00.000Z").is_some());
    assert_eq!(
        iso_from_epoch_millis(epoch_millis(TEST_TIME).unwrap() - 1),
        "2026-09-02T23:59:59.999Z"
    );
    assert_eq!(
        derived_id(
            "provider-thread",
            &["provider", "custom:agent", "native-thread", "a/b 🎈"]
        ),
        "provider-thread:provider:custom%3Aagent:native-thread:a%2Fb%20%F0%9F%8E%88"
    );
}

#[test]
fn thread_opens_decay_deduplicate_and_keep_only_recent_distinct_search_picks() {
    use t3_orchestration::ports::decisions::orchestrator::{
        picked_search_query, record_thread_open,
    };
    assert_eq!(
        picked_search_query(" ' Parser   Bug ' ").unwrap(),
        "parser bug"
    );
    assert_eq!(
        picked_search_query(&format!("{} BBB", "A".repeat(31))).unwrap(),
        "a".repeat(31)
    );
    assert!(picked_search_query("café").is_err());
    assert_eq!(
        record_thread_open(&serde_json::Value::Null, 0, "first"),
        json!({"openedAt":0,"score":1.0,"picks":[{"query":"first","at":0}]})
    );
    let now = 14 * 86_400_000;
    let edge = now - 21 * 86_400_000;
    let previous = json!({"openedAt":0,"score":2,"picks":[{"query":"expired","at":edge-1},{"query":"edge","at":edge},{"query":"alpha","at":now-1},{"query":"retained","at":now-2}]});
    let current = record_thread_open(&previous, now, "alpha");
    assert_eq!(
        current,
        json!({"openedAt":now,"score":2.0,"picks":[{"query":"alpha","at":now},{"query":"edge","at":edge},{"query":"retained","at":now-2}]})
    );
    let repeated = record_thread_open(&current, now + 599_999, "beta");
    assert_eq!(repeated["openedAt"], now);
    assert_eq!(repeated["score"], 2.0);
    assert_eq!(
        repeated["picks"],
        json!([{"query":"beta","at":now+599_999},{"query":"alpha","at":now},{"query":"retained","at":now-2}])
    );
    let empty = record_thread_open(&repeated, now + 599_999, "");
    assert_eq!(empty, repeated);
}

fn runtime_projection() -> OrchestrationV2ThreadProjection {
    let mut projection = projection();
    let request = json!({"id":"request","nodeId":"approval","providerTurnId":"turn","nativeRequestRef":null,"kind":"command","status":"pending","responseCapability":{"type":"live","providerSessionId":"session","supportsFeedback":true},"createdAt":TEST_TIME,"resolvedAt":null});
    projection.runtime_requests = vec![serde_json::from_value(request).unwrap()];
    projection.nodes = vec![serde_json::from_value(json!({"id":"approval","threadId":"thread","runId":"active","parentNodeId":"root","rootNodeId":"root","kind":"approval_request","status":"waiting","countsForRun":true,"providerThreadId":"provider-thread","providerTurnId":"turn","nativeItemRef":null,"runtimeRequestId":"request","checkpointScopeId":null,"startedAt":TEST_TIME,"completedAt":null})).unwrap()];
    let sample: OrchestrationV2ProviderSession = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderSession.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    projection.provider_sessions = vec![change(
        &sample,
        json!({"id":"session","providerInstanceId":"codex","driver":"codex"}),
    )];
    projection
}

#[test]
fn live_runtime_responses_record_the_answer_and_completion_before_delivery() {
    let projection = runtime_projection();
    let decision = PureDecisionEngine.decide(&context(Some(projection)), &command(json!({"type":"runtime-request.respond","commandId":"respond","threadId":"thread","requestId":"request","decision":"decline"}))).unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["runtime-request.updated", "node.updated"]
    );
    assert_eq!(events[0]["payload"]["status"], "resolved");
    assert_eq!(events[0]["payload"]["decision"], "decline");
    assert_eq!(events[0]["payload"]["resolvedAt"], TEST_TIME);
    assert_eq!(events[1]["payload"]["status"], "cancelled");
    assert_eq!(events[0]["runId"], "active");
    assert_eq!(events[0]["providerInstanceId"], "codex");
    assert_eq!(decision.effects.len(), 1);
    assert_eq!(
        decision.effects[0].id,
        "effect:respond:runtime-request.respond:request"
    );
    assert_eq!(
        serde_json::to_value(&decision.effects[0].request).unwrap(),
        json!({"type":"runtime-request.respond","providerSessionId":"session","requestId":"request","decision":"decline"})
    );
}

#[test]
fn runtime_response_rejections_preserve_precondition_order() {
    let respond = command(
        json!({"type":"runtime-request.respond","commandId":"respond","threadId":"thread","requestId":"request","decision":"decline","feedback":" "}),
    );
    for (status, capability, expected) in [
        (
            "resolved",
            json!({"type":"not_resumable","reason":"Lost process"}),
            "Runtime request request is resolved.",
        ),
        (
            "pending",
            json!({"type":"not_resumable","reason":"Lost process"}),
            "Lost process",
        ),
        (
            "pending",
            json!({"type":"live","providerSessionId":"missing","supportsFeedback":true}),
            "Feedback requires a live approval that supports it and a decline decision.",
        ),
    ] {
        let mut projection = runtime_projection();
        projection.runtime_requests[0] = change(
            &projection.runtime_requests[0],
            json!({"status":status,"responseCapability":capability}),
        );
        let error = PureDecisionEngine
            .decide(&context(Some(projection)), &respond)
            .unwrap_err();
        assert!(error.to_string().ends_with(expected), "{error}");
    }
    let mut projection = runtime_projection();
    projection.provider_sessions.clear();
    let respond = command(
        json!({"type":"runtime-request.respond","commandId":"respond","threadId":"thread","requestId":"request","decision":"accept"}),
    );
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection)), &respond)
            .unwrap_err()
            .to_string()
            .ends_with("Provider session session was not found.")
    );
}

#[test]
fn cancelling_a_message_question_does_not_enqueue_provider_work() {
    let mut projection = runtime_projection();
    projection.runtime_requests[0] = change(
        &projection.runtime_requests[0],
        json!({"kind":"user_input","responseCapability":{"type":"message"}}),
    );
    let decision = PureDecisionEngine.decide(&context(Some(projection)), &command(json!({"type":"thread.user-input.dismiss","commandId":"dismiss","threadId":"thread","requestId":"request"}))).unwrap();
    assert!(decision.effects.is_empty());
    let request = serde_json::to_value(&decision.events[0]).unwrap();
    assert_eq!(request["payload"]["decision"], "cancel");
    assert_eq!(request["payload"]["status"], "resolved");
    assert!(request.get("driver").is_none());
    assert!(request.get("providerInstanceId").is_none());
}

#[test]
fn answering_a_message_question_resolves_it_before_starting_work_and_validates_required_answers() {
    let mut projection = runtime_projection();
    projection.thread = change(&projection.thread, json!({"worktreePath":"C:/work/thread"}));
    projection.runtime_requests[0] = change(
        &projection.runtime_requests[0],
        json!({"kind":"user_input","responseCapability":{"type":"message"}}),
    );
    projection.turn_items=vec![serde_json::from_value(json!({"id":"question","threadId":"thread","runId":null,"nodeId":"approval","providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"waiting","title":null,"startedAt":TEST_TIME,"completedAt":null,"updatedAt":TEST_TIME,"type":"user_input_request","requestId":"request","questions":[{"id":"required","header":"Path","question":"Which path?","options":[],"required":true},{"id":"optional","header":"Notes","question":"Any notes?","options":[],"required":false}]})).unwrap()];
    let mut prepared = prepared_with_adapter(projection, "codex");
    prepared.inputs.provider_enabled.push((
        t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
        true,
    ));
    let send = command(
        json!({"type":"runtime-request.respond","commandId":"answer","threadId":"thread","requestId":"request","decision":"accept","answers":{"required":{"answers":[" Parser "]},"optional":""}}),
    );
    let decision = PureDecisionEngine
        .decide_prepared(&prepared, &send)
        .unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    let resolved = events
        .iter()
        .position(|event| {
            event["type"] == "runtime-request.updated" && event["payload"]["status"] == "resolved"
        })
        .unwrap();
    let started = events
        .iter()
        .position(|event| event["type"] == "run.created")
        .unwrap();
    assert!(resolved < started);
    let reply = &events
        .iter()
        .find(|event| event["type"] == "message.updated")
        .unwrap()["payload"];
    assert_eq!(reply["id"], "async-answer:request");
    assert_eq!(reply["text"], "Which path?\nParser");
    assert!(matches!(
        &decision.effects[0].request,
        t3_orchestration::effects::EffectRequest::Start { .. }
    ));
    let mut invalid = t3_contracts::stored::to_value(&send).unwrap();
    invalid["answers"]["required"] = json!("\u{feff}");
    assert!(
        PureDecisionEngine
            .decide_prepared(&prepared, &command(invalid))
            .unwrap_err()
            .to_string()
            .ends_with("Answer each question before sending.")
    );
    prepared.inputs.provider_enabled[0].1 = false;
    assert!(
        PureDecisionEngine
            .decide_prepared(&prepared, &send)
            .unwrap_err()
            .to_string()
            .ends_with("The provider for this project is disabled.")
    );
}

#[test]
fn rollback_failure_cannot_overwrite_a_newer_request() {
    let mut projection = projection();
    projection.thread = change(&projection.thread, json!({"rollbackRequestId":"new"}));
    let failure = command(
        json!({"type":"checkpoint.rollback.fail","commandId":"failure","threadId":"thread","requestId":"old","message":"Failed"}),
    );
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection.clone())), &failure)
            .unwrap_err()
            .to_string()
            .ends_with("Rollback old is no longer the thread's current rollback.")
    );
    let failure = command(
        json!({"type":"checkpoint.rollback.fail","commandId":"failure","threadId":"thread","requestId":"new","message":"Failed"}),
    );
    let decision = PureDecisionEngine
        .decide(&context(Some(projection)), &failure)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&decision.events[0]).unwrap()["payload"]["rollbackFailure"],
        json!({"requestId":"new","message":"Failed"})
    );
}

fn prepared_projection() -> OrchestrationV2ThreadProjection {
    let mut projection = runtime_projection();
    projection.runtime_requests.clear();
    projection.runs = vec![change(
        &run("prepared", 1, "preparing"),
        json!({"workspacePreparation":{"type":"worktree","baseRef":"main"}}),
    )];
    projection.nodes = vec![change(
        &projection.nodes[0],
        json!({"id":"node","runId":"prepared","kind":"root_turn","status":"pending","parentNodeId":null,"rootNodeId":"node","providerTurnId":null,"runtimeRequestId":null}),
    )];
    projection.attempts = vec![serde_json::from_value(json!({"id":"attempt","runId":"prepared","attemptOrdinal":1,"rootNodeId":"node","providerInstanceId":"codex","providerThreadId":"provider-thread","providerTurnId":null,"reason":"initial","status":"pending","startedAt":null,"completedAt":null})).unwrap()];
    let provider: OrchestrationV2ProviderThread = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderThread.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    projection.provider_threads = vec![change(
        &provider,
        json!({"id":"provider-thread","driver":"codex","appThreadId":"thread","providerSessionId":"session"}),
    )];
    projection.turn_items = vec![serde_json::from_value(json!({"id":"preparation","threadId":"thread","runId":"prepared","nodeId":"node","providerThreadId":"provider-thread","providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":4,"status":"running","title":"Preparing workspace","startedAt":TEST_TIME,"completedAt":null,"updatedAt":TEST_TIME,"type":"command_execution","input":"Preparing workspace"})).unwrap()];
    projection
}

#[test]
fn preparation_failure_ends_all_execution_records_and_retry_clears_stale_results() {
    let projection = prepared_projection();
    let decision = PureDecisionEngine.decide(&context(Some(projection.clone())), &command(json!({"type":"prepared-run.fail","commandId":"failure","threadId":"thread","runId":"prepared","failure":{"class":"unknown","message":"setup failed","code":null,"retryable":false}}))).unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "run-attempt.updated",
            "node.updated",
            "turn-item.updated",
            "turn-item.updated",
            "run.updated"
        ]
    );
    assert!(
        events
            .iter()
            .all(|event| event["payload"]["status"] == "failed")
    );
    assert_eq!(
        events[3]["payload"]["failure"]["code"],
        "workspace_preparation_failed"
    );
    assert_eq!(events[3]["payload"]["ordinal"], 5);
    assert_eq!(events[2]["payload"]["exitCode"], 1);
    assert!(decision.effects.is_empty());
    let mut failed = projection;
    failed.attempts[0] = serde_json::from_value(events[0]["payload"].clone()).unwrap();
    failed.nodes[0] = serde_json::from_value(events[1]["payload"].clone()).unwrap();
    failed.turn_items = vec![
        serde_json::from_value(events[2]["payload"].clone()).unwrap(),
        serde_json::from_value(events[3]["payload"].clone()).unwrap(),
    ];
    failed.runs = vec![
        serde_json::from_value(events[4]["payload"].clone()).unwrap(),
        change(&run("queued", 2, "queued"), json!({"queueHeld":true})),
    ];
    let retry = command(
        json!({"type":"prepared-run.retry","commandId":"retry","threadId":"thread","runId":"prepared"}),
    );
    let decision = PureDecisionEngine
        .decide(&context(Some(failed.clone())), &retry)
        .unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(events.len(), 6);
    assert_eq!(events[0]["payload"]["status"], "cancelled");
    assert_eq!(events[1]["payload"]["status"], "running");
    assert!(events[1]["payload"].get("exitCode").is_none());
    assert!(events[1]["payload"].get("output").is_none());
    assert!(events[1]["payload"]["completedAt"].is_null());
    assert_eq!(events[4]["payload"]["status"], "preparing");
    assert_eq!(events[5]["payload"]["queueHeld"], false);
    failed.runs.push(run("newer", 3, "running"));
    assert!(
        PureDecisionEngine
            .decide(&context(Some(failed)), &retry)
            .unwrap_err()
            .to_string()
            .ends_with("Another run is active on this thread.")
    );
}

#[test]
fn preparation_progress_requires_the_current_preparing_run_and_preserves_its_item() {
    let mut projection = prepared_projection();
    let progress = command(
        json!({"type":"prepared-run.progress","commandId":"progress","threadId":"thread","runId":"prepared","phase":"setup"}),
    );
    let decision = PureDecisionEngine
        .decide(&context(Some(projection.clone())), &progress)
        .unwrap();
    let item = serde_json::to_value(&decision.events[0]).unwrap()["payload"].clone();
    assert_eq!(item["id"], "preparation");
    assert_eq!(item["ordinal"], 4);
    assert_eq!(item["title"], "Starting setup script");
    assert_eq!(item["status"], "running");
    projection.runs[0] = change(&projection.runs[0], json!({"status":"interrupted"}));
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection)), &progress)
            .unwrap_err()
            .to_string()
            .ends_with("Run prepared is not awaiting workspace preparation.")
    );
}

#[test]
fn stop_requires_execution_records_to_end_preparation_and_holds_the_queue_either_way() {
    for complete in [false, true] {
        let mut projection = prepared_projection();
        projection.runs[0] = change(
            &projection.runs[0],
            json!({"startedAt":null,"completedAt":null}),
        );
        projection.runs.push(run("queued", 2, "queued"));
        if !complete {
            projection.nodes.clear();
            projection.provider_threads.clear();
            projection.attempts.clear();
            let interrupt = command(
                json!({"type":"run.interrupt","commandId":"interrupt-incomplete","threadId":"thread","runId":"prepared","holdQueue":true}),
            );
            assert!(
                PureDecisionEngine
                    .decide(&context(Some(projection.clone())), &interrupt)
                    .unwrap_err()
                    .to_string()
                    .contains("Run prepared is not interruptible.")
            );
        }
        let stop = command(
            json!({"type":"thread.stop","commandId":"stop-preparing","threadId":"thread","stopDelegatedTasks":false}),
        );
        let decision = PureDecisionEngine
            .decide(&context(Some(projection)), &stop)
            .unwrap();
        let events: Vec<_> = decision
            .events
            .iter()
            .map(|event| t3_contracts::stored::to_value(event).unwrap())
            .collect();
        assert_eq!(
            events.iter().any(|event| event["type"] == "run.updated"
                && event["payload"]["id"] == "prepared"
                && event["payload"]["status"] == "interrupted"),
            complete
        );
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "turn-item.updated"
                    && event["payload"]["type"] == "run_interrupt_request")
        );
        assert!(events.iter().any(|event| event["type"] == "run.updated"
            && event["payload"]["id"] == "queued"
            && event["payload"]["queueHeld"] == true));
    }
}

#[test]
fn releasing_preparation_attaches_the_root_scope_before_starting_the_provider() {
    let mut projection = prepared_projection();
    projection.thread = change(&projection.thread, json!({"worktreePath":"C:/work/thread"}));
    let decision = PureDecisionEngine.decide(&context(Some(projection)), &command(json!({"type":"prepared-run.release","commandId":"release","threadId":"thread","runId":"prepared"}))).unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "checkpoint-scope.created",
            "node.updated",
            "turn-item.updated",
            "run.updated"
        ]
    );
    assert_eq!(events[0]["payload"]["cwd"], "C:/work/thread");
    assert_eq!(events[0]["payload"]["runId"], "prepared");
    assert_eq!(
        events[1]["payload"]["checkpointScopeId"],
        events[0]["payload"]["id"]
    );
    assert_eq!(events[2]["payload"]["id"], "preparation");
    assert_eq!(events[2]["payload"]["ordinal"], 4);
    assert_eq!(events[2]["payload"]["exitCode"], 0);
    assert_eq!(events[3]["payload"]["status"], "starting");
    assert_eq!(
        serde_json::to_value(&decision.effects[0].request).unwrap(),
        json!({"type":"provider-turn.start","runId":"prepared"})
    );
}

#[test]
fn resuming_a_queue_releases_holds_but_does_not_start_a_provider_during_the_decision() {
    let mut projection = projection();
    projection.runs = vec![change(
        &run("queued", 2, "queued"),
        json!({"queueHeld":true}),
    )];
    projection.messages = vec![super::command_policy_test::message("queued")];
    let resume = command(json!({"type":"queue.resume","commandId":"resume","threadId":"thread"}));
    let decision = PureDecisionEngine
        .decide(&context(Some(projection.clone())), &resume)
        .unwrap();
    assert_eq!(decision.events.len(), 1);
    assert_eq!(
        serde_json::to_value(&decision.events[0]).unwrap()["payload"]["queueHeld"],
        false
    );
    assert!(decision.effects.is_empty());
    projection.runs.push(run("active", 3, "running"));
    let start = command(
        json!({"type":"queued-run.start","commandId":"start","threadId":"thread","runId":"queued"}),
    );
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection)), &start)
            .unwrap_err()
            .to_string()
            .ends_with("A queued user message can only be sent now while the thread is stopped.")
    );
}

#[test]
fn secret_records_answer_once_keep_their_position_and_only_saved_answers_unsnooze() {
    let mut projection = prepared_projection();
    projection.thread = change(
        &projection.thread,
        json!({"snoozedUntil":"2026-09-07T00:00:00.000Z","snoozedAt":TEST_TIME}),
    );
    let record = |id: &str, status: &str| {
        command(
            json!({"type":"secret_request.record","commandId":id,"threadId":"thread","runId":"prepared","nodeId":"node","turnItemId":"secret","label":"API key","reason":"Sign in","secretStatus":status}),
        )
    };
    let pending = PureDecisionEngine
        .decide(
            &context(Some(projection.clone())),
            &record("pending", "pending"),
        )
        .unwrap();
    let card = serde_json::to_value(&pending.events[0]).unwrap()["payload"].clone();
    assert_eq!(card["status"], "waiting");
    assert_eq!(card["ordinal"], 5);
    projection
        .turn_items
        .push(serde_json::from_value(card).unwrap());
    let saved = PureDecisionEngine
        .decide(
            &context(Some(projection.clone())),
            &record("saved", "saved"),
        )
        .unwrap();
    assert_eq!(saved.events.len(), 2);
    let saved_card = serde_json::to_value(&saved.events[1]).unwrap()["payload"].clone();
    assert_eq!(saved_card["status"], "completed");
    assert_eq!(saved_card["ordinal"], 5);
    assert!(serde_json::to_value(&saved.events[0]).unwrap()["payload"]["snoozedUntil"].is_null());
    projection.turn_items[1] = serde_json::from_value(saved_card.clone()).unwrap();
    let retried = PureDecisionEngine
        .decide(&context(Some(projection)), &record("late", "cancelled"))
        .unwrap();
    assert_eq!(retried.events.len(), 1);
    assert_eq!(
        serde_json::to_value(&retried.events[0]).unwrap()["payload"],
        saved_card
    );
}

#[test]
fn fork_decisions_resolve_the_source_run_and_preserve_the_requested_creation_time() {
    let mut source = prepared_projection();
    source.runs[0] = change(
        &source.runs[0],
        json!({"status":"completed","checkpointId":"checkpoint"}),
    );
    let mut prepared = context(None).with_inputs(DecisionInputs {
        related_projections: vec![source],
        ..Default::default()
    });
    prepared
        .context
        .ids
        .push((IdKind::ContextTransfer, "transfer".into()));
    let fork = command(
        json!({"type":"thread.fork","commandId":"fork","sourceThreadId":"thread","targetThreadId":"forked","sourcePoint":{"type":"latest_stable"},"createdBy":"user","creationSource":"web","createdAt":"2026-09-02T12:00:00.000Z"}),
    );
    let decision = PureDecisionEngine
        .decide_prepared(&prepared, &fork)
        .unwrap();
    assert_eq!(decision.events.len(), 2);
    let thread = serde_json::to_value(&decision.events[0]).unwrap();
    let transfer = serde_json::to_value(&decision.events[1]).unwrap();
    assert_eq!(thread["threadId"], "forked");
    assert_eq!(thread["occurredAt"], "2026-09-02T12:00:00.000Z");
    assert_eq!(thread["payload"]["forkedFrom"]["runId"], "prepared");
    assert_eq!(thread["payload"]["lineage"]["parentThreadId"], "thread");
    assert_eq!(transfer["payload"]["sourcePoint"]["runId"], "prepared");
    assert_eq!(transfer["payload"]["status"], "pending");
    assert!(decision.effects.is_empty());

    let mut target = projection();
    target.thread = serde_json::from_value(thread["payload"].clone()).unwrap();
    target.runs = prepared.inputs.related_projections[0].runs.clone();
    target.provider_threads = prepared.inputs.related_projections[0]
        .provider_threads
        .clone();
    target.context_transfers = vec![serde_json::from_value(transfer["payload"].clone()).unwrap()];
    let mut merge_context = context(Some(projection())).with_inputs(DecisionInputs {
        related_projections: vec![target],
        ..Default::default()
    });
    merge_context
        .context
        .ids
        .push((IdKind::ContextTransfer, "merge-transfer".into()));
    let merge = command(
        json!({"type":"thread.merge_back","commandId":"merge","sourceThreadId":"forked","targetThreadId":"thread","sourcePoint":{"type":"run","runId":"prepared"},"createdBy":"user","creationSource":"web"}),
    );
    let merged = PureDecisionEngine
        .decide_prepared(&merge_context, &merge)
        .unwrap();
    assert_eq!(merged.events.len(), 1);
    let merged_transfer = serde_json::to_value(&merged.events[0]).unwrap()["payload"].clone();
    assert_eq!(merged_transfer["sourcePoint"]["runId"], "prepared");
    assert_eq!(
        merged_transfer["basePoint"],
        transfer["payload"]["sourcePoint"]
    );
    assert_eq!(merged_transfer["sourceThreadId"], "forked");
    assert_eq!(merged_transfer["targetThreadId"], "thread");
    merge_context
        .context
        .projection
        .as_mut()
        .unwrap()
        .context_transfers = vec![serde_json::from_value(merged_transfer).unwrap()];
    let superseded = PureDecisionEngine
        .decide_prepared(&merge_context, &merge)
        .unwrap();
    assert_eq!(superseded.events.len(), 2);
    assert_eq!(
        serde_json::to_value(&superseded.events[0]).unwrap()["payload"]["status"],
        "superseded"
    );
    merge_context.context.projection.as_mut().unwrap().runs = vec![run("queued", 3, "queued")];
    assert!(PureDecisionEngine.decide_prepared(&merge_context, &merge).unwrap_err().to_string().ends_with("Finish or stop the target thread's active work and clear its queue before merging back."));
}

#[tokio::test]
async fn fork_and_created_thread_commands_commit_once_through_real_dispatch_inputs() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    let source = prepared_projection();
    let node = source.nodes[0].clone();
    let run = change(
        &source.runs[0],
        json!({"status":"completed","checkpointId":"checkpoint"}),
    );
    let events = [
        ("run.created", serde_json::to_value(run).unwrap()),
        ("node.updated", serde_json::to_value(node).unwrap()),
        ("provider-thread.updated", serde_json::to_value(&source.provider_threads[0]).unwrap()),
    ].into_iter().enumerate().map(|(index, (kind, payload))| serde_json::from_value(json!({"id":format!("seed:{index}"),"type":kind,"threadId":"thread","occurredAt":TEST_TIME,"payload":payload})).unwrap()).collect();
    harness
        .sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events,
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await
        .unwrap();
    let fork = json!({"type":"thread.fork","commandId":"fork","sourceThreadId":"thread","targetThreadId":"forked","sourcePoint":{"type":"latest_stable"},"createdBy":"user","creationSource":"web"});
    let receipt = harness.dispatch(fork.clone()).await.unwrap();
    let created = harness
        .reader
        .thread(ThreadId("forked".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        created.lineage.parent_thread_id,
        Some(ThreadId("thread".into()))
    );
    let before = harness.count_events().await;
    assert_eq!(harness.dispatch(fork).await.unwrap(), receipt);
    assert_eq!(harness.count_events().await, before);
    harness.dispatch(json!({"type":"thread.created.record","commandId":"record-fork","parentThreadId":"thread","parentRunId":"prepared","parentNodeId":"node","targetThreadId":"forked","targetRunId":null})).await.unwrap();
    let records = harness
        .reader
        .records(
            ThreadId("thread".into()),
            vec![RecordKind::TurnItems],
            RecordFilter::default(),
        )
        .await
        .unwrap();
    let cards =
        serde_json::to_value(t3_orchestration::runtime::dispatch::projection_from_records(records))
            .unwrap();
    assert_eq!(
        cards["turnItems"][0]["id"],
        "turn-item:created-thread:record-fork"
    );
    assert_eq!(cards["turnItems"][0]["targetThreadId"], "forked");
}

fn task_projection() -> OrchestrationV2ThreadProjection {
    let mut projection = prepared_projection();
    let task: OrchestrationV2Subagent = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2Subagent.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    projection.subagents = vec![change(
        &task,
        json!({"id":"task","threadId":"thread","runId":"prepared","parentNodeId":"node","origin":"app_owned","driver":"claudeAgent","providerInstanceId":"codex","childThreadId":"child","result":null,"status":"running","startedAt":TEST_TIME,"completedAt":null,"updatedAt":TEST_TIME}),
    )];
    projection
}

#[test]
fn subagent_stop_cancels_only_unbound_app_tasks_and_uses_native_stop_identity() {
    let mut projection = task_projection();
    let stop = command(
        json!({"type":"subagent.stop","commandId":"stop-task","threadId":"thread","runId":"prepared","subagentId":"task"}),
    );
    let decision = PureDecisionEngine
        .decide(&context(Some(projection.clone())), &stop)
        .unwrap();
    let task = serde_json::to_value(&decision.events[0]).unwrap()["payload"].clone();
    assert_eq!(task["status"], "cancelled");
    assert!(task["result"].is_null());
    assert_eq!(task["completionDelivery"]["state"], "disposed");
    let mut child = super::command_policy_test::projection();
    child.thread = change(&child.thread, json!({"id":"child"}));
    child.runs = vec![change(
        &run("bound", 1, "running"),
        json!({"delegatedTaskId":"task"}),
    )];
    let prepared = context(Some(projection.clone())).with_inputs(DecisionInputs {
        related_projections: vec![child],
        ..Default::default()
    });
    assert!(
        PureDecisionEngine
            .decide_prepared(&prepared, &stop)
            .unwrap()
            .events
            .is_empty()
    );
    projection.subagents[0] = change(
        &projection.subagents[0],
        json!({"origin":"provider_native","childThreadId":null,"nativeTaskRef":{"driver":"claudeAgent","nativeId":"native-task","strength":"strong"}}),
    );
    projection.provider_threads[0] = change(
        &projection.provider_threads[0],
        json!({"driver":"claudeAgent","providerInstanceId":"codex"}),
    );
    let turn: OrchestrationV2ProviderTurn = serde_json::from_str(
        include_str!("../../../../fixtures/samples/OrchestrationV2ProviderTurn.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    projection.provider_turns = vec![change(
        &turn,
        json!({"id":"native-turn","providerThreadId":"provider-thread"}),
    )];
    let decision = PureDecisionEngine
        .decide(&context(Some(projection.clone())), &stop)
        .unwrap();
    let effect = serde_json::to_value(&decision.effects[0].request).unwrap();
    assert_eq!(effect["providerTurnId"], "native-turn");
    assert_eq!(
        effect["subagent"],
        json!({"id":"task","nativeTaskId":"native-task"})
    );
    projection.subagents[0] = change(
        &projection.subagents[0],
        json!({"nativeTaskRef":{"driver":"claudeAgent","nativeId":"native-task","strength":"weak"}}),
    );
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection)), &stop)
            .unwrap_err()
            .to_string()
            .ends_with("The requested native subagent cannot be stopped on this run.")
    );
}

#[test]
fn remote_stop_preserves_the_first_result_and_increments_its_stop_generation() {
    let mut projection = task_projection();
    let result = json!("Remote run ended");
    projection.subagents[0] = change(
        &projection.subagents[0],
        json!({"childThreadId":null,"status":"failed","result":result,"remoteChild":{"environmentId":"remote","threadId":"remote-thread","label":"Remote","stoppedThere":true,"stopCount":2}}),
    );
    let decision = PureDecisionEngine.decide(&context(Some(projection)), &command(json!({"type":"delegated_task.remote.stop","commandId":"remote-stop","parentThreadId":"thread","taskId":"task"}))).unwrap();
    let event = serde_json::to_value(&decision.events[0]).unwrap();
    assert_eq!(event["payload"]["remoteChild"]["stopCount"], 3);
    assert_eq!(event["payload"]["remoteChild"]["stoppedThere"], false);
    assert_eq!(event["payload"]["result"], result);
    assert!(event.get("providerInstanceId").is_none());
    assert!(decision.effects.is_empty());
}

#[tokio::test]
async fn adopting_a_project_folder_preserves_the_working_directory_and_rejects_live_work() {
    use t3_orchestration::stores::projects::{ProjectRow, upsert};
    let harness = Harness::new(None, false);
    let project = |id: &str, root: &str| {
        serde_json::from_value::<ProjectRow>(json!({"projectId":id,"title":id,"hasCustomTitle":false,"workspaceRoot":root,"defaultModelSelection":null,"defaultThreadEnvMode":null,"autoPull":false,"faviconPath":null,"projectIcon":null,"scripts":[],"pinnedAt":null,"archivedAt":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"deletedAt":null})).unwrap()
    };
    let projects = [
        project("project", "C:/work/main"),
        project("folder", "C:/work/thread"),
    ];
    harness
        .database
        .stores
        .transaction(move |tx| {
            for project in projects {
                upsert(tx, &project)?;
            }
            Ok(())
        })
        .await
        .unwrap();
    harness.create("thread").await.unwrap();
    harness.dispatch(json!({"type":"thread.metadata.update","commandId":"worktree","threadId":"thread","worktreePath":"C:/work/thread"})).await.unwrap();
    harness.dispatch(json!({"type":"thread.project.adopt-folder","commandId":"adopt","threadId":"thread","projectId":"folder"})).await.unwrap();
    let adopted = harness
        .reader
        .thread(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(&adopted.project_id).unwrap(), "folder");
    assert!(adopted.worktree_path.is_none());
    let mut busy = super::command_policy_test::projection();
    busy.runs = vec![run("queued", 1, "queued")];
    let prepared = context(Some(busy)).with_inputs(DecisionInputs::default());
    assert!(PureDecisionEngine.decide_prepared(&prepared, &command(json!({"type":"thread.project.adopt-folder","commandId":"busy","threadId":"thread","projectId":"missing"}))).unwrap_err().to_string().ends_with("still has work in progress. Finish or stop it first."));
}

#[test]
fn queue_resume_uses_the_latest_failed_root_error_and_current_session_error() {
    let mut projection = prepared_projection();
    projection.runs[0] = change(
        &projection.runs[0],
        json!({"status":"failed","completedAt":TEST_TIME}),
    );
    projection.runs.push(change(
        &run("queued", 2, "queued"),
        json!({"queueHeld":true}),
    ));
    let error = |id: &str, ordinal: u64, class: &str| {
        serde_json::from_value(json!({"id":id,"threadId":"thread","runId":"prepared","nodeId":"node","providerThreadId":"provider-thread","providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":ordinal,"status":"failed","title":null,"startedAt":TEST_TIME,"completedAt":TEST_TIME,"updatedAt":TEST_TIME,"type":"error","failure":{"class":class,"message":"Limit reached","code":null,"retryable":null}})).unwrap()
    };
    projection.turn_items = vec![
        error("newer", 7, "usage_limit"),
        error("older", 6, "unknown"),
    ];
    projection.provider_sessions[0] =
        change(&projection.provider_sessions[0], json!({"lastError":null}));
    let resume = command(json!({"type":"queue.resume","commandId":"resume","threadId":"thread"}));
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection.clone())), &resume)
            .unwrap_err()
            .to_string()
            .ends_with("Continue the limited thread before resuming its queue.")
    );
    projection.provider_sessions[0] = change(
        &projection.provider_sessions[0],
        json!({"lastError":"A different provider error"}),
    );
    assert_eq!(
        PureDecisionEngine
            .decide(&context(Some(projection.clone())), &resume)
            .unwrap()
            .events
            .len(),
        1
    );
    projection.provider_sessions[0] =
        change(&projection.provider_sessions[0], json!({"lastError":null}));
    projection.thread = change(
        &projection.thread,
        json!({"providerInstanceId":"other","modelSelection":{"instanceId":"other","model":"b"}}),
    );
    projection.messages = vec![change(
        &super::command_policy_test::message("queued"),
        json!({"delegatedCompletion":{"parentRunId":"prepared","generation":1,"taskIds":["task"]}}),
    )];
    assert_eq!(
        PureDecisionEngine
            .decide(&context(Some(projection.clone())), &resume)
            .unwrap()
            .events
            .len(),
        1
    );
    for field in ["archivedAt", "deletedAt"] {
        projection.thread = change(&projection.thread, json!({(field):TEST_TIME}));
        assert!(
            PureDecisionEngine
                .decide(&context(Some(projection.clone())), &resume)
                .unwrap_err()
                .to_string()
                .ends_with("Thread thread is not active.")
        );
    }
}

#[tokio::test]
async fn forks_recheck_the_source_mode_limit_before_creating_a_thread() {
    use t3_contracts::provider_policy::{ProviderInteractionMode, RuntimeMode};
    use t3_orchestration::ports::decisions::dispatch_mode_limit::DispatchModes;
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    harness.dispatch(json!({"type":"thread.runtime-mode.set","commandId":"high-mode","threadId":"thread","runtimeMode":"full-access"})).await.unwrap();
    let fork = command(
        json!({"type":"thread.fork","commandId":"fork","sourceThreadId":"thread","targetThreadId":"forked","sourcePoint":{"type":"latest_stable"},"sideChat":true,"createdBy":"user","creationSource":"web"}),
    );
    let error = harness
        .dispatcher
        .dispatch_with_limit(
            fork.clone(),
            Some(DispatchModes {
                runtime_mode: RuntimeMode::ApprovalRequired,
                interaction_mode: ProviderInteractionMode::Default,
            }),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Thread thread now runs in full-access/default mode")
    );
    assert!(
        harness
            .reader
            .thread(ThreadId("forked".into()))
            .await
            .unwrap()
            .is_none()
    );
    harness.dispatcher.dispatch(fork).await.unwrap();
    assert!(
        harness
            .reader
            .thread(ThreadId("forked".into()))
            .await
            .unwrap()
            .is_some()
    );
}

#[test]
fn reopening_pull_request_checks_clears_only_the_answer_and_keeps_watch_progress() {
    let mut projection = projection();
    let watch = json!({"startedAt":TEST_TIME,"headSha":"head","failedChecks":["lint"],"passed":false,"passedChecks":[],"remarksThrough":TEST_TIME,"remarkIds":["remark"],"conflicting":false,"wakes":2,"checksPending":false,"checksAwaitedSince":TEST_TIME});
    projection.thread = change(
        &projection.thread,
        json!({"pullRequests":[{"host":"github.com","repository":"owner/repo","number":1,"url":"https://github.com/owner/repo/pull/1","source":"manual","linkedAt":TEST_TIME,"snapshot":null,"stack":null,"watch":watch}]}),
    );
    let reopen = command(
        json!({"type":"thread.pull-request-watch.checks-reopen","commandId":"reopen-checks","threadId":"thread"}),
    );
    let decision = PureDecisionEngine
        .decide(&context(Some(projection.clone())), &reopen)
        .unwrap();
    assert_eq!(decision.events.len(), 1);
    let event = serde_json::to_value(&decision.events[0]).unwrap();
    assert_eq!(event["type"], "thread.pull-request-synced");
    let updated = &event["payload"]["pullRequests"][0]["watch"];
    assert!(updated.get("checksPending").is_none());
    for field in [
        "checksAwaitedSince",
        "wakes",
        "failedChecks",
        "remarkIds",
        "headSha",
    ] {
        assert_eq!(updated[field], watch[field]);
    }
    assert_eq!(event["payload"]["updatedAt"], TEST_TIME);
    projection.thread = serde_json::from_value(event["payload"].clone()).unwrap();
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection)), &reopen)
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn remote_requests_bind_the_parent_run_reject_stop_and_truncate_utf16_titles() {
    let mut projection = prepared_projection();
    let remote = json!({"environmentId":"remote","threadId":"remote-thread","label":"Remote"});
    let prompt = format!("{}🦀abcdefgh", "x".repeat(68));
    let request = command(
        json!({"type":"delegated_task.remote.request","commandId":"remote-request","parentThreadId":"thread","parentRunId":"prepared","parentNodeId":"node","task":prompt,"driver":"claudeAgent","modelSelection":{"instanceId":"other","model":"cheap"},"remoteChild":remote,"completionWake":"always"}),
    );
    let decision = PureDecisionEngine
        .decide(&context(Some(projection.clone())), &request)
        .unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["node.updated", "subagent.updated", "turn-item.updated"]
    );
    assert_eq!(
        events[0]["payload"]["id"],
        "node:delegated-task:remote-request"
    );
    assert_eq!(events[0]["payload"]["rootNodeId"], "node");
    assert_eq!(events[0]["payload"]["countsForRun"], false);
    assert_eq!(events[1]["payload"]["remoteChild"], remote);
    assert_eq!(events[1]["payload"]["completionWake"], "always");
    assert_eq!(
        events[2]["payload"]["title"],
        format!("{}...", "x".repeat(68))
    );
    assert_eq!(events[2]["payload"]["ordinal"], 5);
    assert_eq!(events[2]["payload"]["providerThreadId"], "provider-thread");
    assert!(
        events
            .iter()
            .all(|event| event["providerInstanceId"] == "other")
    );
    projection.turn_items.push(change(
        &projection.turn_items[0],
        json!({"type":"run_interrupt_request","message":"Stopped"}),
    ));
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection.clone())), &request)
            .unwrap_err()
            .to_string()
            .ends_with("Parent run prepared is stopping.")
    );
    projection.turn_items.pop();
    projection.runs[0] = change(&projection.runs[0], json!({"status":"completed"}));
    assert!(
        PureDecisionEngine
            .decide(&context(Some(projection.clone())), &request)
            .unwrap_err()
            .to_string()
            .ends_with("Parent run prepared is not active.")
    );
    projection.subagents = vec![change(
        &task_projection().subagents[0],
        json!({"remoteChild":remote,"result":"Done","status":"completed","childThreadId":null}),
    )];
    assert_eq!(
        PureDecisionEngine
            .decide(&context(Some(projection)), &request)
            .unwrap()
            .events
            .len(),
        3
    );
}

#[test]
fn ordinary_message_creates_a_run_attempt_scope_and_start_effect_with_adapter_identity() {
    let mut projection = projection();
    projection.thread = change(
        &projection.thread,
        json!({"worktreePath":"C:/work/project"}),
    );
    let mut prepared = context(Some(projection)).with_inputs(DecisionInputs::default());
    prepared
        .context
        .ids
        .push((IdKind::ProviderSession, "session".into()));
    let mut capabilities = super::command_policy_test::capabilities();
    capabilities
        .sessions
        .supports_multiple_provider_threads_per_session = false;
    prepared.inputs.provider_metadata.push((
        t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
        t3_orchestration::api::AdapterMetadata {
            driver: t3_contracts::provider_instance::ProviderDriverKind("actual-adapter".into()),
            continuation_key: "test".into(),
            enabled: true,
            capabilities,
        },
    ));
    let message = command(
        json!({"type":"message.dispatch","commandId":"send","threadId":"thread","messageId":"message","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
    );
    let decision = PureDecisionEngine
        .decide_prepared(&prepared, &message)
        .unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    let find = |kind: &str| &events.iter().find(|event| event["type"] == kind).unwrap()["payload"];
    let run = find("run.created");
    assert_eq!(run["id"], "run:thread:thread:ordinal:1");
    assert_eq!(run["status"], "starting");
    assert!(run["startedAt"].is_null());
    assert_eq!(find("run-attempt.created")["status"], "pending");
    assert_eq!(find("provider-thread.updated")["driver"], "actual-adapter");
    assert_eq!(
        find("provider-thread.updated")["providerSessionId"],
        "session"
    );
    assert_eq!(
        find("node.updated")["checkpointScopeId"],
        find("checkpoint-scope.created")["id"]
    );
    assert_eq!(find("checkpoint-scope.created")["cwd"], "C:/work/project");
    assert_eq!(find("message.updated")["text"], "Hello");
    assert_eq!(find("turn-item.updated")["inputIntent"], "turn_start");
    assert_eq!(
        serde_json::to_value(&decision.effects[0].request).unwrap(),
        json!({"type":"provider-turn.start","runId":run["id"]})
    );
    assert_eq!(decision.effects.len(), 1);
}

#[tokio::test]
async fn project_thread_and_first_message_commit_the_start_intent_once() {
    use std::sync::Arc;
    use t3_orchestration::{
        ports::{
            sessions_recovery::provider_adapter_registry::AdapterRegistry,
            thread_project::project_commands::{
                ProjectCommand, ProjectCommandState, plan_project_command,
            },
        },
        provider::ThreadSnapshot,
        stores::receipts::{AggregateId, CommandReceipt, ReceiptStatus},
        testing::FakeProviderAdapter,
    };
    let fixture = prepared_projection();
    let mut capabilities = super::command_policy_test::capabilities();
    capabilities
        .sessions
        .supports_multiple_provider_threads_per_session = true;
    let session = change(
        &fixture.provider_sessions[0],
        json!({"id":"provider-session:provider-instance:codex:shared","capabilities":capabilities}),
    );
    let snapshot = ThreadSnapshot {
        provider_thread: fixture.provider_threads[0].clone(),
        provider_turns: vec![],
        messages: vec![],
        reasoning_message_ids: None,
        runtime_requests: vec![],
        provider_payload: None,
    };
    let (adapter, _calls) = FakeProviderAdapter::new(
        t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
        t3_contracts::provider_instance::ProviderDriverKind("codex".into()),
        capabilities,
        session,
        snapshot,
    );
    let harness = Harness::with_providers(
        None,
        false,
        Arc::new(AdapterRegistry::from_adapters(vec![Arc::new(adapter)])),
    );
    let now = t3_contracts::wire::WireDateTime(TEST_TIME.into());
    let project = ProjectCommand::Create {
        command_id: CommandId("create-project".into()),
        project_id: ProjectId("project".into()),
        title: "Project".into(),
        workspace_root: "C:/work/project".into(),
        scripts: None,
    };
    let event = plan_project_command(
        &project,
        ProjectCommandState {
            project: None,
            workspace_owner: None,
        },
        EventId("project-created".into()),
        &now,
        |title| title.chars().count(),
    )
    .unwrap()
    .unwrap();
    let receipt = CommandReceipt {
        command_id: CommandId("create-project".into()),
        aggregate: AggregateId::Project(ProjectId("project".into())),
        command_type: "project.create".into(),
        accepted_at: TEST_TIME.into(),
        result_sequence: 0,
        status: ReceiptStatus::Accepted,
        error: None,
    };
    harness
        .dispatcher
        .commit_project(event, receipt)
        .await
        .unwrap();
    harness.create("thread").await.unwrap();
    let mut committed = harness.sink.subscribe();
    let send = json!({"type":"message.dispatch","commandId":"send","threadId":"thread","messageId":"first-message","text":"Hello","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"});
    let receipt = harness.dispatch(send.clone()).await.unwrap();
    let _first_committed = committed.recv().await.unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Accepted);
    let records = harness
        .reader
        .records(
            ThreadId("thread".into()),
            vec![
                RecordKind::Runs,
                RecordKind::Attempts,
                RecordKind::Nodes,
                RecordKind::Messages,
                RecordKind::CheckpointScopes,
            ],
            RecordFilter::default(),
        )
        .await
        .unwrap();
    let projection = t3_orchestration::runtime::dispatch::projection_from_records(records);
    assert_eq!(projection.runs.len(), 1);
    assert_eq!(
        projection.runs[0].status,
        OrchestrationV2RunStatus::Starting
    );
    assert_eq!(projection.messages[0].text, "Hello");
    assert_eq!(
        projection.nodes[0].checkpoint_scope_id.as_ref(),
        Some(&projection.checkpoint_scopes[0].id)
    );
    assert_eq!(projection.checkpoint_scopes[0].cwd.0, "C:/work/project");
    let effects = harness
        .database
        .stores
        .read(|db| t3_orchestration::stores::outbox::list_by_command(db, &CommandId("send".into())))
        .await
        .unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effects[0].request,
        t3_orchestration::effects::EffectRequest::Start {
            run_id: RunId("run:thread:thread:ordinal:1".into())
        }
    );
    let count = harness.count_events().await;
    assert_eq!(harness.dispatch(send).await.unwrap(), receipt);
    assert_eq!(harness.count_events().await, count);
}

#[test]
fn interrupt_before_start_ends_preparation_and_cancels_pending_start() {
    let projection = prepared_projection();
    let decision = PureDecisionEngine.decide(&context(Some(projection.clone())), &command(json!({"type":"run.interrupt","commandId":"stop","threadId":"thread","runId":"prepared","reason":"Stop now"}))).unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    for kind in ["run.updated", "run-attempt.updated", "node.updated"] {
        assert_eq!(
            events.iter().find(|event| event["type"] == kind).unwrap()["payload"]["status"],
            "interrupted"
        );
    }
    assert!(
        events
            .iter()
            .any(|event| event["payload"]["id"] == "preparation"
                && event["payload"]["title"] == "Workspace preparation interrupted")
    );
    assert_eq!(
        decision.cancellations[0].effect_types,
        ["provider-turn.start", "provider-turn.restart"]
    );
    assert!(decision.effects.is_empty());
    let mut background = projection;
    background.runs[0] = change(&background.runs[0], json!({"ordinal":3}));
    background.provider_threads.push(change(
        &background.provider_threads[0],
        json!({"id":"background-owner","providerSessionId":"background-session"}),
    ));
    for (index, id) in [(1, "old"), (2, "newer")] {
        background.runs.push(change(
            &run(id, index, "completed"),
            json!({"providerThreadId":"background-owner"}),
        ));
        background.provider_turns.push(change(&running_projection().provider_turns[0],json!({"id":format!("background-turn:{id}"),"providerThreadId":"background-owner","runAttemptId":format!("background-attempt:{id}"),"ordinal":index})));
        background.turn_items.push(change(&background.turn_items[0],json!({"id":format!("background-item:{id}"),"runId":id,"providerThreadId":"background-owner","providerTurnId":format!("background-turn:{id}"),"input":"Background command"})));
    }
    let mut prepared = context(Some(background)).with_inputs(DecisionInputs::default());
    prepared
        .inputs
        .live_sessions
        .push((ProviderSessionId("background-session".into()), true));
    let decision=PureDecisionEngine.decide_prepared(&prepared,&command(json!({"type":"run.interrupt","commandId":"stop-background","threadId":"thread","runId":"prepared"}))).unwrap();
    assert_eq!(decision.effects.len(), 1);
    assert!(
        matches!(&decision.effects[0].request,t3_orchestration::effects::EffectRequest::Interrupt{provider_turn_id,..} if provider_turn_id.0=="background-turn:newer")
    );
}

#[test]
fn live_interrupt_waits_for_provider_completion_but_missing_session_settles_locally() {
    let mut projection = prepared_projection();
    projection.runs[0] = change(
        &projection.runs[0],
        json!({"status":"running","startedAt":TEST_TIME}),
    );
    projection.attempts[0] = change(
        &projection.attempts[0],
        json!({"status":"running","providerTurnId":"turn","startedAt":TEST_TIME}),
    );
    projection.nodes[0] = change(
        &projection.nodes[0],
        json!({"status":"running","providerTurnId":"turn","checkpointScopeId":null}),
    );
    projection.provider_sessions[0] = change(
        &projection.provider_sessions[0],
        json!({"capabilities":super::command_policy_test::capabilities()}),
    );
    projection.provider_turns = vec![serde_json::from_value(json!({"id":"turn","runAttemptId":"attempt","nodeId":"node","providerThreadId":"provider-thread","nativeTurnRef":null,"ordinal":1,"status":"running","startedAt":TEST_TIME,"completedAt":null})).unwrap()];
    let request = command(
        json!({"type":"run.interrupt","commandId":"stop","threadId":"thread","runId":"prepared"}),
    );
    for live in [true, false] {
        let prepared = context(Some(projection.clone())).with_inputs(DecisionInputs {
            live_sessions: vec![(ProviderSessionId("session".into()), live)],
            ..Default::default()
        });
        let decision = PureDecisionEngine
            .decide_prepared(&prepared, &request)
            .unwrap();
        let events: Vec<_> = decision
            .events
            .iter()
            .map(|event| t3_contracts::stored::to_value(event).unwrap())
            .collect();
        if live {
            assert_eq!(decision.effects.len(), 1);
            assert!(matches!(
                decision.effects[0].request,
                t3_orchestration::effects::EffectRequest::Interrupt { .. }
            ));
            assert!(!events.iter().any(|event| event["type"] == "run.updated"));
        } else {
            assert!(decision.effects.is_empty());
            assert!(events.iter().any(|event| event["type"] == "run.updated"
                && event["payload"]["status"] == "interrupted"));
        }
    }
    assert!(
        PureDecisionEngine
            .decide_prepared(
                &context(Some(projection)).with_inputs(DecisionInputs::default()),
                &request
            )
            .unwrap_err()
            .to_string()
            .contains("live-session observation")
    );
}

fn running_projection() -> OrchestrationV2ThreadProjection {
    let mut projection = prepared_projection();
    projection.thread = change(&projection.thread, json!({"worktreePath":"C:/work/thread"}));
    projection.runs[0] = change(
        &projection.runs[0],
        json!({"status":"running","startedAt":TEST_TIME}),
    );
    projection.attempts[0] = change(
        &projection.attempts[0],
        json!({"status":"running","providerTurnId":"turn","startedAt":TEST_TIME}),
    );
    projection.nodes[0] = change(
        &projection.nodes[0],
        json!({"status":"running","providerTurnId":"turn","checkpointScopeId":null}),
    );
    projection.provider_sessions[0] = change(
        &projection.provider_sessions[0],
        json!({"capabilities":super::command_policy_test::capabilities()}),
    );
    projection.provider_turns=vec![serde_json::from_value(json!({"id":"turn","runAttemptId":"attempt","nodeId":"node","providerThreadId":"provider-thread","nativeTurnRef":null,"ordinal":1,"status":"running","startedAt":TEST_TIME,"completedAt":null})).unwrap()];
    projection
}

#[test]
fn native_steering_preserves_attempts_and_restart_replaces_only_the_attempt() {
    for restart in [false, true] {
        let prepared = context(Some(running_projection())).with_inputs(DecisionInputs {
            live_sessions: vec![(ProviderSessionId("session".into()), true)],
            ..Default::default()
        });
        let request = command(
            json!({"type":"message.dispatch","commandId":"steer","threadId":"thread","messageId":"follow-up","text":"Use the smaller change","attachments":[],"dispatchMode":{"type":if restart {"restart_active"}else{"steer_active"},"targetRunId":"prepared"},"createdBy":"user","creationSource":"web"}),
        );
        let decision = PureDecisionEngine
            .decide_prepared(&prepared, &request)
            .unwrap();
        let events: Vec<_> = decision
            .events
            .iter()
            .map(|event| t3_contracts::stored::to_value(event).unwrap())
            .collect();
        assert!(
            events.iter().any(|event| event["type"] == "message.updated"
                && event["payload"]["runId"] == "prepared")
        );
        assert_eq!(decision.effects.len(), 1);
        if restart {
            assert!(matches!(
                decision.effects[0].request,
                t3_orchestration::effects::EffectRequest::Restart { .. }
            ));
            assert!(
                events
                    .iter()
                    .any(|event| event["type"] == "run-attempt.updated"
                        && event["payload"]["status"] == "superseded")
            );
            assert!(
                events
                    .iter()
                    .any(|event| event["type"] == "run-attempt.created"
                        && event["payload"]["attemptOrdinal"] == 2)
            );
        } else {
            assert!(matches!(
                decision.effects[0].request,
                t3_orchestration::effects::EffectRequest::Steer { .. }
            ));
            assert!(
                !events
                    .iter()
                    .any(|event| event["type"] == "run-attempt.updated")
            );
        }
    }
}

#[test]
fn a_delegated_request_creates_its_child_and_start_in_the_parent_commit() {
    let mut prepared = context(Some(running_projection())).with_inputs(DecisionInputs::default());
    let parent = prepared.context.projection.as_mut().unwrap();
    parent.thread = change(
        &parent.thread,
        json!({"linkedPullRequest":{"projectId":"project","repository":"org/repo","number":7,"url":"https://github.com/org/repo/pull/7"},"pullRequests":[{"host":"github","repository":"org/repo","number":7,"url":"https://github.com/org/repo/pull/7","source":"manual","linkedAt":TEST_TIME,"snapshot":null,"stack":null}]}),
    );
    prepared.context.ids.extend([
        (IdKind::ProviderSession, "child-session".into()),
        (IdKind::ContextTransfer, "spawn-transfer".into()),
    ]);
    prepared.inputs.provider_metadata.push((
        t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
        AdapterMetadata {
            driver: t3_contracts::provider_instance::ProviderDriverKind("actual-adapter".into()),
            continuation_key: "test".into(),
            enabled: true,
            capabilities: super::command_policy_test::capabilities(),
        },
    ));
    let request = command(
        json!({"type":"delegated_task.request","commandId":"delegate","parentThreadId":"thread","parentRunId":"prepared","parentNodeId":"node","task":"Inspect the parser","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","createdBy":"agent","creationSource":"server"}),
    );
    let decision = PureDecisionEngine
        .decide_prepared(&prepared, &request)
        .unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    let child = "thread:delegated-task:delegate";
    assert_eq!(events[0]["type"], "thread.created");
    assert_eq!(events[0]["threadId"], child);
    assert_eq!(events[0]["payload"]["lineage"]["parentThreadId"], "thread");
    assert!(events[0]["payload"]["linkedPullRequest"].is_null());
    assert_eq!(events[0]["payload"]["pullRequests"], json!([]));
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "subagent.updated"
                && event["payload"]["childThreadId"] == child)
    );
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "run.created" && event["threadId"] == child)
    );
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "context-transfer.created"
                && event["payload"]["sourcePoint"]["threadId"] == "thread")
    );
    assert_eq!(decision.effects.len(), 1);
    assert_eq!(decision.effects[0].thread_id.0, child);
}

#[test]
fn completion_acceptance_reserves_the_next_pending_sibling_without_repeating_delivery() {
    let mut projection = task_projection();
    projection.runs = vec![change(
        &run("parent", 1, "completed"),
        json!({"delegatedCompletion":{"disposition":"open","nextGeneration":2,"delivery":{"generation":1,"messageId":"wake","taskIds":["task"]}}}),
    )];
    projection.subagents[0] = change(
        &projection.subagents[0],
        json!({"runId":"parent","status":"completed","result":"Done","completionDelivery":{"state":"claimed","observedByRunId":null}}),
    );
    projection.subagents.push(change(
        &projection.subagents[0],
        json!({"id":"sibling","completionDelivery":{"state":"pending","observedByRunId":null}}),
    ));
    let mut message = super::command_policy_test::message("wake");
    message = change(
        &message,
        json!({"id":"wake","delegatedCompletion":{"parentRunId":"parent","generation":1,"taskIds":["task"]}}),
    );
    projection.messages = vec![message];
    let mut prepared = context(Some(projection)).with_inputs(DecisionInputs::default());
    prepared
        .context
        .ids
        .push((IdKind::Message, "next-wake".into()));
    let decision=PureDecisionEngine.decide_prepared(&prepared,&command(json!({"type":"notification.delivery.accept","commandId":"accept","threadId":"thread","messageId":"wake"}))).unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    assert!(events.iter().any(|event| event["payload"]["id"] == "task"
        && event["payload"]["completionDelivery"]["state"] == "delivered"
        && event["payload"]["completionDelivery"]["observedByRunId"].is_null()));
    assert!(
        events
            .iter()
            .any(|event| event["payload"]["id"] == "sibling"
                && event["payload"]["completionDelivery"]["state"] == "claimed")
    );
    let parent = &events
        .iter()
        .find(|event| event["type"] == "run.updated")
        .unwrap()["payload"];
    assert_eq!(
        parent["delegatedCompletion"]["delivery"]["taskIds"],
        json!(["sibling"])
    );
    assert_eq!(
        parent["delegatedCompletion"]["delivery"]["messageId"],
        "next-wake"
    );
    assert_eq!(parent["delegatedCompletion"]["nextGeneration"], 3);
}

#[test]
fn pull_request_mutations_use_observed_identity_and_preserve_watch_history_on_unlink() {
    use t3_orchestration::ports::decisions::orchestrator::{
        PreparedPullRequestIdentities, pull_request_mutation,
    };
    let raw = json!({"host":"ALIAS","repository":"Org/Repo","number":7});
    let canonical = json!({"host":"canonical.example","repository":"org/repo","number":7});
    let mut identities = PreparedPullRequestIdentities {
        keys: vec![
            (
                serde_json::from_value(raw.clone()).unwrap(),
                serde_json::from_value(canonical.clone()).unwrap(),
            ),
            (
                serde_json::from_value(canonical.clone()).unwrap(),
                serde_json::from_value(canonical).unwrap(),
            ),
        ],
        legacy_keys: vec![],
        canonical_urls: vec![(
            "https://alias/7".into(),
            "https://canonical.example/org/repo/pull/7".into(),
        )],
        ..Default::default()
    };
    let mut raw_link = raw.clone();
    raw_link["url"] = json!("https://alias/7");
    let canonical_key = json!({"host":"canonical.example","repository":"org/repo","number":7});
    identities
        .observe_key(
            &raw_link,
            serde_json::from_value(canonical_key.clone()).unwrap(),
        )
        .unwrap();
    let mut canonical_link = canonical_key.clone();
    canonical_link["url"] = json!("https://canonical.example/org/repo/pull/7");
    identities
        .observe_key(
            &canonical_link,
            serde_json::from_value(canonical_key).unwrap(),
        )
        .unwrap();
    let mut request = raw.clone();
    request["type"] = json!("thread.pull-request.link");
    request["source"] = json!("stack");
    request["url"] = json!("https://alias/7");
    let thread = pull_request_mutation(
        t3_contracts::stored::to_value(&projection().thread).unwrap(),
        &request,
        &t3_contracts::wire::WireDateTime(TEST_TIME.into()),
        &identities,
    )
    .unwrap();
    assert_eq!(thread["pullRequests"][0]["host"], "canonical.example");
    assert_eq!(
        thread["pullRequests"][0]["url"],
        "https://canonical.example/org/repo/pull/7"
    );
    request["type"] = json!("thread.pull-request.watch");
    request["watching"] = json!(true);
    let watching = pull_request_mutation(
        thread,
        &request,
        &t3_contracts::wire::WireDateTime(TEST_TIME.into()),
        &identities,
    )
    .unwrap();
    assert_eq!(
        watching["pullRequests"][0]["watch"]["awaitingBaseline"],
        "all"
    );
    request["type"] = json!("thread.pull-request.unlink");
    let ended = pull_request_mutation(
        watching,
        &request,
        &t3_contracts::wire::WireDateTime(TEST_TIME.into()),
        &identities,
    )
    .unwrap();
    assert_eq!(ended["pullRequests"][0]["source"], "stack-dismissed");
    assert!(ended["pullRequests"][0].get("watch").is_none());
    assert_eq!(
        ended["pullRequests"][0]["endedWatch"]["startedAt"],
        TEST_TIME
    );
}

#[tokio::test]
async fn queue_resume_starts_the_same_queued_run_once_and_receipt_replay_is_safe() {
    use std::sync::Arc;
    use t3_orchestration::{
        ports::sessions_recovery::provider_adapter_registry::AdapterRegistry,
        provider::ThreadSnapshot, testing::FakeProviderAdapter,
    };
    let fixture = prepared_projection();
    let mut caps = super::command_policy_test::capabilities();
    caps.sessions.supports_multiple_provider_threads_per_session = true;
    let snapshot = ThreadSnapshot {
        provider_thread: fixture.provider_threads[0].clone(),
        provider_turns: vec![],
        messages: vec![],
        reasoning_message_ids: None,
        runtime_requests: vec![],
        provider_payload: None,
    };
    let (adapter, _) = FakeProviderAdapter::new(
        t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
        t3_contracts::provider_instance::ProviderDriverKind("codex".into()),
        caps,
        fixture.provider_sessions[0].clone(),
        snapshot,
    );
    let harness = Harness::with_providers(
        None,
        false,
        Arc::new(AdapterRegistry::from_adapters(vec![Arc::new(adapter)])),
    );
    harness.create("thread").await.unwrap();
    harness.dispatch(json!({"type":"thread.metadata.update","commandId":"workspace","threadId":"thread","worktreePath":"C:/work/thread"})).await.unwrap();
    for (index, mode) in [(1, "start_immediately"), (2, "queue_after_active")] {
        harness.dispatch(json!({"type":"message.dispatch","commandId":format!("send:{index}"),"threadId":"thread","messageId":format!("message:{index}"),"text":format!("Message {index}"),"attachments":[],"dispatchMode":{"type":mode},"createdBy":"user","creationSource":"web"})).await.unwrap();
    }
    let before = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.runs[1].status, OrchestrationV2RunStatus::Queued);
    let first = &before.runs[0];
    let mut ended = t3_contracts::stored::to_value(first).unwrap();
    ended["status"] = json!("completed");
    ended["completedAt"] = json!(TEST_TIME);
    let event=serde_json::from_value(json!({"id":"ended:first","threadId":"thread","type":"run.updated","occurredAt":TEST_TIME,"runId":first.id,"providerInstanceId":"codex","payload":ended})).unwrap();
    harness
        .sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events: vec![event],
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await
        .unwrap();
    let resume = json!({"type":"queue.resume","commandId":"resume","threadId":"thread"});
    let receipt = harness.dispatch(resume.clone()).await.unwrap();
    let started = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started.runs.len(), 2);
    assert_eq!(started.runs[1].id, before.runs[1].id);
    assert_eq!(started.runs[1].status, OrchestrationV2RunStatus::Starting);
    let events = harness.sink.subscribe();
    assert_eq!(harness.dispatch(resume).await.unwrap(), receipt);
    let again = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, started);
    let mut events = events;
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

fn recovery_pair() -> (PreparedDecisionContext, OrchestrationV2ThreadProjection) {
    let mut parent = task_projection();
    parent.runs[0] = change(
        &parent.runs[0],
        json!({"status":"completed","startedAt":TEST_TIME,"completedAt":TEST_TIME,"delegatedCompletion":{"disposition":"open","nextGeneration":1,"delivery":null}}),
    );
    parent.subagents[0] = change(
        &parent.subagents[0],
        json!({"completionDelivery":{"state":"pending","observedByRunId":null}}),
    );
    let mut child = projection();
    child.thread = change(
        &child.thread,
        json!({"id":"child","lineage":{"parentThreadId":"thread","relationshipToParent":"subagent","rootThreadId":"thread"},"forkedFrom":{"type":"node","threadId":"thread","nodeId":"task"}}),
    );
    child.runs = vec![change(
        &run("child-run", 1, "completed"),
        json!({"threadId":"child","startedAt":TEST_TIME,"completedAt":TEST_TIME}),
    )];
    child.messages = vec![change(
        &super::command_policy_test::message("child-run"),
        json!({"threadId":"child","role":"assistant","text":"The parser uses UTF-16 offsets."}),
    )];
    let mut prepared = context(Some(parent)).with_inputs(DecisionInputs::default());
    prepared.context.ids.extend([
        (IdKind::ContextTransfer, "result-transfer".into()),
        (IdKind::Message, "result-wake".into()),
    ]);
    (prepared, child)
}

#[test]
fn delegated_recovery_waits_for_restart_then_commits_the_result_and_wake_once() {
    let (prepared, child) = recovery_pair();
    let result = PureDecisionEngine
        .decide_delegated_recovery(
            &prepared,
            Some(&child),
            None,
            None,
            &[RunId("child-run".into())],
        )
        .unwrap();
    assert!(result.events.is_empty());
    let result = PureDecisionEngine
        .decide_delegated_recovery(
            &prepared,
            Some(&child),
            None,
            Some(&RunId("child-run".into())),
            &[RunId("child-run".into())],
        )
        .unwrap();
    let events: Vec<_> = result
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    assert_eq!(events[0]["type"], "subagent.updated");
    assert_eq!(events[0]["payload"]["status"], "completed");
    assert_eq!(
        events[0]["payload"]["result"],
        "The parser uses UTF-16 offsets."
    );
    assert_eq!(
        events[0]["payload"]["completionDelivery"]["state"],
        "claimed"
    );
    let cohort = &events
        .iter()
        .find(|event| event["type"] == "run.updated")
        .unwrap()["payload"]["delegatedCompletion"];
    assert_eq!(cohort["delivery"]["messageId"], "result-wake");
    assert_eq!(cohort["delivery"]["taskIds"], json!(["task"]));
    let transfer = events
        .iter()
        .find(|event| event["type"] == "context-transfer.created")
        .unwrap();
    assert_eq!(transfer["payload"]["sourcePoint"]["runId"], "child-run");
    assert_eq!(transfer["payload"]["targetRunId"], "prepared");
    let mut replay = prepared.clone();
    let projection = replay.context.projection.as_mut().unwrap();
    projection.subagents[0] = serde_json::from_value(events[0]["payload"].clone()).unwrap();
    projection
        .context_transfers
        .push(serde_json::from_value(transfer["payload"].clone()).unwrap());
    assert!(
        PureDecisionEngine
            .decide_delegated_recovery(&replay, Some(&child), None, None, &[])
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn completion_run_finalization_redelivers_cancelled_claims_but_keeps_delivered_tasks_delivered() {
    for cancelled in [false, true] {
        let (mut prepared, _) = recovery_pair();
        let projection = prepared.context.projection.as_mut().unwrap();
        projection.runs[0] = change(
            &projection.runs[0],
            json!({"delegatedCompletion":{"disposition":"open","nextGeneration":2,"delivery":{"generation":1,"messageId":"delivery-input","taskIds":["task"]}}}),
        );
        projection.subagents[0] = change(
            &projection.subagents[0],
            json!({"status":"completed","result":"Done","completionDelivery":{"state":"claimed","observedByRunId":null}}),
        );
        projection.runs.push(change(
            &run(
                "delivery",
                2,
                if cancelled { "cancelled" } else { "completed" },
            ),
            json!({"userMessageId":"delivery-input","completedAt":TEST_TIME}),
        ));
        projection.messages.push(change(&super::command_policy_test::message("delivery"),json!({"id":"delivery-input","delegatedCompletion":{"parentRunId":"prepared","generation":1,"taskIds":["task"]}})));
        let result = PureDecisionEngine
            .decide_delegated_recovery(&prepared, None, Some(&RunId("delivery".into())), None, &[])
            .unwrap();
        let events: Vec<_> = result
            .events
            .iter()
            .map(|event| t3_contracts::stored::to_value(event).unwrap())
            .collect();
        let task = &events
            .iter()
            .find(|event| event["type"] == "subagent.updated")
            .unwrap()["payload"];
        assert_eq!(
            task["completionDelivery"]["state"],
            if cancelled { "claimed" } else { "delivered" }
        );
        let cohort = &events
            .iter()
            .find(|event| event["type"] == "run.updated")
            .unwrap()["payload"]["delegatedCompletion"];
        if cancelled {
            assert_eq!(cohort["delivery"]["generation"], 2);
            assert_eq!(cohort["delivery"]["messageId"], "result-wake");
            assert_eq!(cohort["nextGeneration"], 3);
        } else {
            assert!(cohort["delivery"].is_null());
            assert_eq!(cohort["nextGeneration"], 2);
        }
    }
}

fn prepared_with_adapter(
    projection: OrchestrationV2ThreadProjection,
    driver: &str,
) -> PreparedDecisionContext {
    let mut prepared = context(Some(projection)).with_inputs(DecisionInputs::default());
    prepared
        .context
        .ids
        .push((IdKind::ProviderSession, "new-session".into()));
    prepared.inputs.provider_metadata.push((
        t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
        AdapterMetadata {
            driver: t3_contracts::provider_instance::ProviderDriverKind(driver.into()),
            continuation_key: "test".into(),
            enabled: true,
            capabilities: super::command_policy_test::capabilities(),
        },
    ));
    prepared
        .inputs
        .live_sessions
        .push((ProviderSessionId("session".into()), true));
    prepared
}

#[test]
fn native_goals_use_the_adapter_identity_and_queue_a_control_before_interrupting() {
    let request = command(
        json!({"type":"message.dispatch","commandId":"goal","threadId":"thread","messageId":"goal-message","text":"/goal inspect the parser","attachments":[],"dispatchMode":{"type":"steer_active","targetRunId":"prepared"},"createdBy":"user","creationSource":"web"}),
    );
    let prepared = prepared_with_adapter(running_projection(), "codex");
    let result = PureDecisionEngine
        .decide_prepared(&prepared, &request)
        .unwrap();
    let events: Vec<_> = result
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    let run = events
        .iter()
        .find(|event| event["type"] == "run.created")
        .unwrap();
    assert_eq!(run["payload"]["status"], "queued");
    assert_eq!(run["payload"]["queuePosition"], 1);
    assert!(result.effects.iter().any(|effect| matches!(
        effect.request,
        t3_orchestration::effects::EffectRequest::Interrupt { .. }
    )));
    assert!(!result.effects.iter().any(|effect| matches!(
        effect.request,
        t3_orchestration::effects::EffectRequest::Steer { .. }
    )));
    let mut pause = t3_contracts::stored::to_value(&request).unwrap();
    pause["text"] = json!("/goal pause");
    pause["dispatchMode"] = json!({"type":"queue_after_active"});
    let claude = prepared_with_adapter(running_projection(), "claudeAgent");
    assert!(
        PureDecisionEngine
            .decide_prepared(&claude, &command(pause))
            .unwrap_err()
            .to_string()
            .ends_with(
                "Claude goals cannot be paused or resumed. Send /goal clear to remove the goal."
            )
    );
    // A configured instance named codex may use an adapter that has no native goals.
    let other = prepared_with_adapter(running_projection(), "cursor");
    let error = PureDecisionEngine
        .decide_prepared(&other, &request)
        .unwrap_err();
    assert!(error.to_string().ends_with(
        "Goal commands must run as a separate turn. Queue it or wait for the active turn to finish."
    ));
}

#[test]
fn a_prestart_steer_is_reserved_for_the_target_and_a_restart_can_fall_back_to_queue() {
    for fallback in [false, true] {
        let mut projection = if fallback {
            running_projection()
        } else {
            prepared_projection()
        };
        projection.thread = change(&projection.thread, json!({"worktreePath":"C:/work/thread"}));
        if fallback {
            projection.provider_sessions[0]
                .capabilities
                .turns
                .supports_steering_by_interrupt_restart = false;
        }
        let prepared = prepared_with_adapter(projection, "codex");
        let request = command(
            json!({"type":"message.dispatch","commandId":"reserved","threadId":"thread","messageId":"reserved-message","text":"Another task","attachments":[],"dispatchMode":{"type":if fallback {"restart_active"}else{"steer_active"},"targetRunId":"prepared"},"createdBy":"user","creationSource":"web"}),
        );
        let result = PureDecisionEngine
            .decide_prepared(&prepared, &request)
            .unwrap();
        let events: Vec<_> = result
            .events
            .iter()
            .map(|event| t3_contracts::stored::to_value(event).unwrap())
            .collect();
        let run = &events
            .iter()
            .find(|event| event["type"] == "run.created")
            .unwrap()["payload"];
        assert_eq!(run["status"], "queued");
        if fallback {
            assert_eq!(run["queuePosition"], 1);
            assert!(result.effects.iter().any(|effect| matches!(
                effect.request,
                t3_orchestration::effects::EffectRequest::Interrupt { .. }
            )));
        } else {
            assert_eq!(run["steerWhenReadyRunId"], "prepared");
            assert!(result.effects.is_empty());
        }
    }
}

#[test]
fn child_followups_create_a_new_parent_task_and_bind_only_the_new_run() {
    let (mut prepared, mut child) = recovery_pair();
    let mut parent = prepared.context.projection.take().unwrap();
    parent.subagents[0] = change(
        &parent.subagents[0],
        json!({"result":"Original task complete","status":"completed"}),
    );
    child.thread = change(&child.thread, json!({"worktreePath":"C:/work/child"}));
    prepared = prepared_with_adapter(child, "codex");
    prepared.inputs.related_projections.push(parent);
    prepared
        .inputs
        .next_item_ordinals
        .push((ThreadId("thread".into()), None, 42));
    let request = command(
        json!({"type":"message.dispatch","commandId":"followup","threadId":"child","messageId":"followup-input","text":"Now add the test","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
    );
    let result = PureDecisionEngine
        .decide_prepared(&prepared, &request)
        .unwrap();
    let events: Vec<_> = result
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    let task = events
        .iter()
        .find(|event| event["type"] == "subagent.updated")
        .unwrap();
    assert_eq!(task["threadId"], "thread");
    assert_eq!(task["payload"]["id"], "node:delegated-task:followup");
    assert_eq!(task["payload"]["runId"], "prepared");
    assert!(task["payload"]["result"].is_null());
    assert_eq!(task["payload"]["status"], "running");
    let bind = events
        .iter()
        .find(|event| event["type"] == "run.updated" && event["threadId"] == "child")
        .unwrap();
    assert_eq!(bind["payload"]["delegatedTaskId"], task["payload"]["id"]);
    assert_ne!(bind["payload"]["id"], "child-run");
    assert_eq!(
        events
            .iter()
            .find(|event| event["type"] == "turn-item.updated" && event["threadId"] == "thread")
            .unwrap()["payload"]["ordinal"],
        42
    );
}

#[test]
fn stop_cancels_only_idle_unbound_orphans_after_observing_no_restart() {
    use t3_orchestration::ports::decisions::orchestrator::idle_orphan_tasks;
    let (prepared, child) = recovery_pair();
    let parent = prepared.context.projection.unwrap();
    let mut orphan = child.clone();
    orphan.thread = change(
        &orphan.thread,
        json!({"forkedFrom":{"type":"node","nodeId":"original-task"}}),
    );
    let now = t3_contracts::wire::WireDateTime(TEST_TIME.into());
    assert!(
        idle_orphan_tasks(
            &parent,
            &[child],
            Some(&vec![(ThreadId("child".into()), vec![])]),
            &now
        )
        .unwrap()
        .is_empty()
    );
    let observations = vec![(ThreadId("child".into()), vec![RunId("child-run".into())])];
    assert!(
        idle_orphan_tasks(
            &parent,
            std::slice::from_ref(&orphan),
            Some(&observations),
            &now
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        idle_orphan_tasks(&parent, std::slice::from_ref(&orphan), None, &now)
            .unwrap_err()
            .to_string()
            .contains("restart continuation observations")
    );
    let cancelled = idle_orphan_tasks(
        &parent,
        &[orphan],
        Some(&vec![(ThreadId("child".into()), vec![])]),
        &now,
    )
    .unwrap();
    assert_eq!(cancelled.len(), 1);
    let task = t3_contracts::stored::to_value(&cancelled[0]).unwrap();
    assert_eq!(task["status"], "cancelled");
    assert!(task["result"].is_null());
    assert_eq!(task["completionDelivery"]["state"], "disposed");
}

#[test]
fn wake_duration_inherits_the_latest_started_work_only_for_automatic_or_child_wakes() {
    use t3_orchestration::ports::decisions::orchestrator::wake_work_started_at;
    let mut projection = task_projection();
    projection.runs = vec![
        change(
            &run("old", 1, "completed"),
            json!({"startedAt":"2026-09-01T00:00:00.000Z","workStartedAt":"2026-08-31T00:00:00.000Z"}),
        ),
        run("queued", 2, "queued"),
    ];
    let trigger = json!({"createdBy":"agent","creationSource":"mcp","senderThreadId":"child"});
    assert_eq!(
        wake_work_started_at(&projection, &trigger, "immediate")
            .unwrap()
            .unwrap()
            .0,
        "2026-08-31T00:00:00.000Z"
    );
    assert!(
        wake_work_started_at(&projection, &trigger, "explicit-queue")
            .unwrap()
            .is_none()
    );
    projection.subagents.clear();
    assert!(
        wake_work_started_at(&projection, &trigger, "immediate")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        wake_work_started_at(&projection, &trigger, "automatic-queue")
            .unwrap()
            .unwrap()
            .0,
        "2026-08-31T00:00:00.000Z"
    );
    assert!(
        wake_work_started_at(
            &projection,
            &json!({"createdBy":"user","creationSource":"web"}),
            "immediate"
        )
        .unwrap()
        .is_none()
    );
}

async fn message_harness() -> Harness {
    use std::sync::Arc;
    use t3_orchestration::{
        ports::sessions_recovery::provider_adapter_registry::AdapterRegistry,
        provider::ThreadSnapshot, testing::FakeProviderAdapter,
    };
    let fixture = prepared_projection();
    let caps = super::command_policy_test::capabilities();
    let snapshot = ThreadSnapshot {
        provider_thread: fixture.provider_threads[0].clone(),
        provider_turns: vec![],
        messages: vec![],
        reasoning_message_ids: None,
        runtime_requests: vec![],
        provider_payload: None,
    };
    let (adapter, _calls) = FakeProviderAdapter::new(
        t3_contracts::provider_instance::ProviderInstanceId("codex".into()),
        t3_contracts::provider_instance::ProviderDriverKind("codex".into()),
        caps.clone(),
        change(&fixture.provider_sessions[0], json!({"capabilities":caps})),
        snapshot,
    );
    let harness = Harness::with_providers(
        None,
        false,
        Arc::new(AdapterRegistry::from_adapters(vec![Arc::new(adapter)])),
    );
    harness.create("thread").await.unwrap();
    harness.dispatch(json!({"type":"thread.metadata.update","commandId":"workspace","threadId":"thread","worktreePath":"C:/work/thread"})).await.unwrap();
    harness
}

#[tokio::test]
async fn a_committed_terminal_starts_the_waiting_run_and_replaying_the_update_does_not_start_twice()
{
    let harness = message_harness().await;
    for (index, mode) in [(1, "start_immediately"), (2, "queue_after_active")] {
        harness.dispatch(json!({"type":"message.dispatch","commandId":format!("send:{index}"),"threadId":"thread","messageId":format!("input:{index}"),"text":"Work","attachments":[],"dispatchMode":{"type":mode},"createdBy":"user","creationSource":"web"})).await.unwrap();
    }
    let before = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    let mut run = t3_contracts::stored::to_value(&before.runs[0]).unwrap();
    run["status"] = json!("completed");
    run["completedAt"] = json!(TEST_TIME);
    let event:OrchestrationV2DomainEvent=serde_json::from_value(json!({"id":"terminal:first","threadId":"thread","type":"run.updated","runId":run["id"],"occurredAt":TEST_TIME,"payload":run})).unwrap();
    harness
        .sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events: vec![event.clone()],
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await
        .unwrap();
    let stored = OrchestrationV2StoredEvent {
        sequence: 1,
        command_id: Some(CommandId("provider:first".into())),
        event,
    };
    harness.dispatcher.handle_run_update(&stored).await.unwrap();
    let after = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.runs[1].status, OrchestrationV2RunStatus::Starting);
    assert_eq!(after.runs[1].id, before.runs[1].id);
    harness.dispatcher.handle_run_update(&stored).await.unwrap();
    assert_eq!(
        harness
            .reader
            .projection(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap(),
        after
    );
}

#[tokio::test]
async fn implementing_a_plan_loads_it_by_identity_and_marks_it_completed_in_the_message_commit() {
    let harness = message_harness().await;
    let send = command(
        json!({"type":"message.dispatch","commandId":"implement","threadId":"thread","messageId":"implement-input","text":"Implement the plan","attachments":[],"sourcePlanRef":{"threadId":"thread","planId":"plan"},"dispatchMode":{"type":"defer_start"},"createdBy":"user","creationSource":"web"}),
    );
    assert!(
        !required_records(&send)
            .unwrap()
            .0
            .contains(&RecordKind::Plans)
    );
    let event=serde_json::from_value(json!({"id":"plan:event","threadId":"thread","type":"plan.updated","occurredAt":TEST_TIME,"payload":{"id":"plan","threadId":"thread","runId":null,"nodeId":"plan-node","kind":"proposed_plan","status":"active","markdown":"# Plan\nBuild the parser."}})).unwrap();
    harness
        .sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events: vec![event],
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await
        .unwrap();
    harness.dispatcher.dispatch(send).await.unwrap();
    let plan = harness
        .reader
        .plan(ThreadId("thread".into()), PlanId("plan".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        t3_contracts::stored::to_value(&plan).unwrap()["status"],
        "completed"
    );
}

#[tokio::test]
async fn deletion_cascades_to_side_chats_atomically_and_replay_keeps_their_tombstones() {
    let harness = Harness::new(None, false);
    harness.create("thread").await.unwrap();
    harness.create("side").await.unwrap();
    let side = harness
        .reader
        .thread(ThreadId("side".into()))
        .await
        .unwrap()
        .unwrap();
    let side = change(
        &side,
        json!({"lineage":{"parentThreadId":"thread","relationshipToParent":"fork","rootThreadId":"thread","sideChat":true}}),
    );
    let transfer = json!({"id":"side-transfer","type":"fork","sourceThreadId":"thread","targetThreadId":"side","sourcePoint":{"threadId":"thread"},"basePoint":null,"sourceProviderInstanceId":"codex","targetProviderInstanceId":"codex","targetRunId":null,"status":"consumed","resolution":null,"createdBy":"user","error":null,"createdAt":TEST_TIME,"updatedAt":TEST_TIME,"consumedAt":TEST_TIME});
    let events=vec![serde_json::from_value(json!({"id":"side:lineage","type":"thread.metadata-updated","threadId":"side","occurredAt":TEST_TIME,"payload":side})).unwrap(),serde_json::from_value(json!({"id":"side:transfer","type":"context-transfer.created","threadId":"thread","occurredAt":TEST_TIME,"payload":transfer})).unwrap()];
    harness
        .sink
        .commit(CommitRequest {
            receipt: None,
            decision: Decision {
                events,
                ..Default::default()
            },
            guard: CommitGuard::None,
        })
        .await
        .unwrap();
    let delete = json!({"type":"thread.delete","commandId":"delete","threadId":"thread"});
    let receipt = harness.dispatch(delete.clone()).await.unwrap();
    for id in ["thread", "side"] {
        assert!(
            harness
                .reader
                .thread(ThreadId(id.into()))
                .await
                .unwrap()
                .unwrap()
                .deleted_at
                .is_some()
        );
    }
    assert_eq!(harness.dispatch(delete).await.unwrap(), receipt);
    let events = harness
        .sink
        .events_by_command(CommandId("delete".into()))
        .await
        .unwrap();
    let deleted: Vec<_> = events
        .iter()
        .filter_map(|stored| {
            let event = t3_contracts::stored::to_value(&stored.event).unwrap();
            (event["type"] == "thread.deleted").then(|| event["threadId"].clone())
        })
        .collect();
    assert_eq!(
        deleted,
        json!(["thread", "side"]).as_array().unwrap().clone()
    );
}

#[tokio::test]
async fn editing_a_queued_goal_reads_only_its_user_message_and_ignores_obsolete_assistant_payloads()
{
    let harness = message_harness().await;
    for index in 1..=2 {
        harness.dispatch(json!({"type":"message.dispatch","commandId":format!("queue:{index}"),"threadId":"thread","messageId":format!("queued-input:{index}"),"text":"Work","attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"user","creationSource":"web"})).await.unwrap();
    }
    harness.database.stores.transaction(|db| {
        db.execute("INSERT INTO orchestration_v2_projection_messages (message_id,thread_id,run_id,node_id,role,streaming,created_at,updated_at,payload_json) VALUES ('obsolete-assistant','thread',NULL,NULL,'assistant',0,?1,?1,'{}')",[TEST_TIME])?;
        Ok(())
    }).await.unwrap();
    let queued = harness
        .reader
        .query_records(ProjectionRecordQuery {
            thread_id: ThreadId("thread".into()),
            fields: vec![RecordKind::Runs],
            filter: RecordFilter::default(),
            messages: MessageFilter::default(),
            run_statuses: Some(vec![OrchestrationV2RunStatus::Queued]),
            include_latest_run: false,
        })
        .await
        .unwrap();
    let queued = t3_orchestration::runtime::dispatch::projection_from_records(queued)
        .runs
        .remove(0);
    let id = t3_contracts::stored::to_value(&queued.id).unwrap();
    harness.dispatch(json!({"type":"queued-run.edit","commandId":"edit-goal","threadId":"thread","runId":id,"text":"/goal pause"})).await.unwrap();
    let message = harness
        .reader
        .run_message(
            ThreadId("thread".into()),
            serde_json::from_value(id).unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(message.text, "/goal pause");
}

#[test]
fn rollback_validates_provider_ownership_and_missing_files_before_scheduling_work() {
    let mut projection = running_projection();
    projection.thread = change(
        &projection.thread,
        json!({"activeProviderThreadId":"provider-thread"}),
    );
    let scope:OrchestrationV2CheckpointScope=serde_json::from_value(json!({"id":"scope","threadId":"thread","runId":"prepared","nodeId":"node","parentScopeId":null,"providerThreadId":"provider-thread","kind":"root_run","ordinalWithinParent":0,"advancesAppRunCount":true,"cwd":"C:/work/thread","createdAt":TEST_TIME})).unwrap();
    projection.checkpoint_scopes = vec![scope];
    projection.checkpoints=vec![serde_json::from_value(json!({"id":"checkpoint","threadId":"thread","scopeId":"scope","runId":"prepared","nodeId":"node","parentCheckpointId":null,"ordinalWithinScope":1,"appRunOrdinal":1,"ref":"refs/t3/checkpoint","status":"missing","files":[],"capturedAt":TEST_TIME})).unwrap()];
    let request = command(
        json!({"type":"checkpoint.rollback","commandId":"rollback","threadId":"thread","checkpointId":"checkpoint","scopeId":"scope","restoreFiles":false}),
    );
    let prepared = prepared_with_adapter(projection.clone(), "codex");
    let decision = PureDecisionEngine
        .decide_prepared(&prepared, &request)
        .unwrap();
    assert_eq!(decision.events.len(), 2);
    assert_eq!(decision.effects.len(), 1);
    assert!(matches!(
        decision.effects[0].request,
        t3_orchestration::effects::EffectRequest::Rollback {
            restore_files: Some(false),
            ..
        }
    ));
    let metadata = t3_contracts::stored::to_value(&decision.events[0]).unwrap();
    assert_eq!(metadata["payload"]["rollbackRequestId"], "rollback");
    let mut wrong = projection.clone();
    wrong.provider_turns[0] = change(
        &wrong.provider_turns[0],
        json!({"providerThreadId":"another-owner"}),
    );
    assert!(
        PureDecisionEngine
            .decide_prepared(&prepared_with_adapter(wrong, "codex"), &request)
            .unwrap_err()
            .to_string()
            .contains("belongs to provider thread another-owner")
    );
    let restore = command(
        json!({"type":"checkpoint.rollback","commandId":"restore","threadId":"thread","checkpointId":"checkpoint","scopeId":"scope","restoreFiles":true}),
    );
    assert!(
        PureDecisionEngine
            .decide_prepared(&prepared, &restore)
            .unwrap_err()
            .to_string()
            .contains("is missing and cannot be restored")
    );
}

async fn completed_delegate_harness() -> (Harness, ThreadId, RunId) {
    let harness = message_harness().await;
    harness.dispatch(json!({"type":"message.dispatch","commandId":"parent-start","threadId":"thread","messageId":"parent-input","text":"Work","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"})).await.unwrap();
    let parent = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    harness.dispatch(json!({"type":"delegated_task.request","commandId":"delegate-sqlite","parentThreadId":"thread","parentRunId":parent.runs[0].id,"parentNodeId":parent.runs[0].root_node_id,"task":"Inspect the parser","completionWake":"always","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","createdBy":"agent","creationSource":"server"})).await.unwrap();
    let child_id = ThreadId("thread:delegated-task:delegate-sqlite".into());
    let child = harness
        .reader
        .projection(child_id.clone())
        .await
        .unwrap()
        .unwrap();
    let mut run = t3_contracts::stored::to_value(&child.runs[0]).unwrap();
    run["status"] = json!("completed");
    run["startedAt"] = json!(TEST_TIME);
    run["completedAt"] = json!(TEST_TIME);
    let result = json!({"id":"child-result","threadId":child_id,"runId":run["id"],"nodeId":run["rootNodeId"],"role":"assistant","text":"Use UTF-16 offsets.","attachments":[],"streaming":false,"createdBy":"agent","creationSource":"provider","createdAt":TEST_TIME,"updatedAt":TEST_TIME});
    harness.sink.commit(CommitRequest{receipt:None,decision:Decision{events:vec![serde_json::from_value(json!({"id":"child:completed","threadId":child_id,"type":"run.updated","runId":run["id"],"occurredAt":TEST_TIME,"payload":run})).unwrap(),serde_json::from_value(json!({"id":"child:reply","threadId":child_id,"type":"message.updated","runId":run["id"],"nodeId":run["rootNodeId"],"occurredAt":TEST_TIME,"payload":result})).unwrap()],..Default::default()},guard:CommitGuard::None}).await.unwrap();
    let child_run = serde_json::from_value(run["id"].clone()).unwrap();
    (harness, child_id, child_run)
}

#[tokio::test]
async fn delegated_recovery_commits_the_child_result_and_offers_only_its_current_cohort() {
    let (harness, child_id, child_run) = completed_delegate_harness().await;
    let (sender, mut offers) = tokio::sync::mpsc::channel(4);
    harness
        .dispatcher
        .set_continuation_requests(sender)
        .unwrap();
    let operations = t3_orchestration::runtime::dispatch::DispatchThreadOperations {
        base: std::sync::Arc::new(t3_orchestration::testing::FakeThreadOperations::default()),
        dispatcher: harness.dispatcher.clone(),
    };
    operations
        .recover_delegated_task(child_id.clone(), Some(child_run.clone()), None)
        .await
        .unwrap();
    let parent = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        t3_contracts::stored::to_value(&parent.subagents[0]).unwrap()["completionDelivery"]["state"],
        "claimed"
    );
    let offer = offers.recv().await.unwrap();
    let completion = offer.delegated_completion.unwrap();
    assert_eq!(offer.thread_id, ThreadId("thread".into()));
    assert_eq!(completion.generation, 1);
    assert_eq!(
        parent.subagents[0].result.as_deref(),
        Some("Use UTF-16 offsets.")
    );
    assert_eq!(
        parent.subagents[0].status,
        OrchestrationV2TurnItemStatus::Completed
    );
    let transfers: Vec<_> = parent
        .context_transfers
        .iter()
        .map(|transfer| t3_contracts::stored::to_value(transfer).unwrap())
        .filter(|transfer| transfer["type"] == "subagent_result")
        .collect();
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0]["sourcePoint"]["runId"], json!(child_run));
    harness
        .dispatcher
        .recover_delegated_task(child_id, None, None)
        .await
        .unwrap();
    let again = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.context_transfers, parent.context_transfers);
}

#[tokio::test]
async fn startup_recovers_good_children_and_contains_a_bad_child_before_offering_completions() {
    use std::sync::Arc;
    use t3_orchestration::{runtime::dispatch::DelegatedRecoveryPhase, testing::SequenceIds};
    let (harness, good_child, _) = completed_delegate_harness().await;
    let parent = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    harness.dispatch(json!({"type":"delegated_task.request","commandId":"bad-delegate","parentThreadId":"thread","parentRunId":parent.runs[0].id,"parentNodeId":parent.runs[0].root_node_id,"task":"Bad result","completionWake":"always","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","createdBy":"agent","creationSource":"server"})).await.unwrap();
    let bad_child = ThreadId("thread:delegated-task:bad-delegate".into());
    let child = harness
        .reader
        .projection(bad_child.clone())
        .await
        .unwrap()
        .unwrap();
    let ended = change(
        &child.runs[0],
        json!({"status":"completed","completedAt":TEST_TIME}),
    );
    harness.sink.commit(CommitRequest { receipt: None, decision: Decision { events: vec![serde_json::from_value(json!({"id":"bad-ended","threadId":bad_child,"type":"run.updated","runId":ended.id,"occurredAt":TEST_TIME,"payload":t3_contracts::stored::to_value(&ended).unwrap()})).unwrap()], ..Default::default() }, guard: CommitGuard::None }).await.unwrap();
    let corrupt_id = bad_child.0.clone();
    harness.database.stores.transaction(move |tx| {
        tx.execute("UPDATE orchestration_v2_projection_runs SET payload_json=json_remove(payload_json,'$.id') WHERE thread_id=?1", [&corrupt_id])?;
        Ok(())
    }).await.unwrap();
    let mut continuations = harness
        .dispatcher
        .prepare_continuations(
            harness.reader.clone(),
            Arc::new(SequenceIds::new("startup")),
            std::num::NonZeroUsize::new(8).unwrap(),
        )
        .unwrap();
    let report = harness.dispatcher.recover_delegated_tasks().await;
    assert_eq!(report.subagent_result_threads, 2);
    assert_eq!(report.completion_threads, 1);
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        report.failures[0].phase,
        DelegatedRecoveryPhase::SubagentResults
    );
    assert_eq!(report.failures[0].thread_id.as_ref(), Some(&bad_child));
    let parent = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    let task = parent
        .subagents
        .iter()
        .find(|task| {
            task.child_thread_id
                == Some(t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                    good_child.clone(),
                ))
        })
        .unwrap();
    assert_eq!(task.result.as_deref(), Some("Use UTF-16 offsets."));
    let offer = continuations.requests.recv().await.unwrap();
    assert_eq!(offer.thread_id, ThreadId("thread".into()));
    assert_eq!(offer.delegated_completion.as_ref().unwrap().generation, 1);
    assert_eq!(continuations.service.process(&offer).await.unwrap(), t3_orchestration::ports::sessions_recovery::provider_continuation_service::ContinuationOutcome::Settled);
    let delivered = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(delivered.messages.iter().any(|message| {
        message.id
            == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                offer
                    .delegated_completion
                    .as_ref()
                    .unwrap()
                    .message_id
                    .clone(),
            )
    }));
    continuations.service.shutdown().await;
}

#[tokio::test]
async fn startup_and_effect_repair_finalize_only_the_current_completion_and_replay_without_writes()
{
    use std::sync::Arc;
    use t3_orchestration::{
        runtime::dispatch::DispatchThreadOperations,
        testing::{FakeThreadOperations, SequenceIds},
    };
    for startup in [false, true] {
        let (harness, child, child_run) = completed_delegate_harness().await;
        let mut continuations = harness
            .dispatcher
            .prepare_continuations(
                harness.reader.clone(),
                Arc::new(SequenceIds::new("repair")),
                std::num::NonZeroUsize::new(8).unwrap(),
            )
            .unwrap();
        let operations = DispatchThreadOperations {
            base: Arc::new(FakeThreadOperations::default()),
            dispatcher: harness.dispatcher.clone(),
        };
        operations
            .recover_delegated_task(child, Some(child_run), None)
            .await
            .unwrap();
        let offer = continuations.requests.recv().await.unwrap();
        assert_eq!(continuations.service.process(&offer).await.unwrap(), t3_orchestration::ports::sessions_recovery::provider_continuation_service::ContinuationOutcome::Settled);
        let before = harness
            .reader
            .projection(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap();
        let delivery_message = &offer.delegated_completion.as_ref().unwrap().message_id;
        let delivery = before
            .runs
            .iter()
            .find(|run| {
                run.user_message_id
                    == t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(
                        delivery_message.clone(),
                    )
            })
            .unwrap();
        let ended = change(
            delivery,
            json!({"status":"completed","completedAt":TEST_TIME}),
        );
        let id: RunId =
            serde_json::from_value(t3_contracts::stored::to_value(&ended.id).unwrap()).unwrap();
        harness.sink.commit(CommitRequest { receipt: None, decision: Decision { events: vec![serde_json::from_value(json!({"id":"delivery:ended","threadId":"thread","type":"run.updated","runId":id,"occurredAt":TEST_TIME,"payload":t3_contracts::stored::to_value(&ended).unwrap()})).unwrap()], ..Default::default() }, guard: CommitGuard::None }).await.unwrap();
        if startup {
            let report = harness.dispatcher.recover_delegated_tasks().await;
            assert!(report.failures.is_empty(), "{report:?}");
            assert_eq!(report.completion_threads, 1);
        } else {
            operations
                .recover_delegated_task(ThreadId("thread".into()), None, Some(id.clone()))
                .await
                .unwrap();
        }
        let after = harness
            .reader
            .projection(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            t3_contracts::stored::to_value(&after.subagents[0]).unwrap()["completionDelivery"]["state"],
            "delivered"
        );
        assert!(t3_contracts::stored::to_value(&after.runs[0]).unwrap()["delegatedCompletion"]["delivery"].is_null());
        let events = harness.count_events().await;
        operations
            .recover_delegated_task(ThreadId("thread".into()), None, Some(id))
            .await
            .unwrap();
        assert_eq!(harness.count_events().await, events);
        continuations.service.shutdown().await;
    }
}

#[test]
fn delegated_question_notices_steer_a_running_turn_only_when_steering_preserves_its_tools() {
    for interrupts_tools in [false, true] {
        let mut projection = running_projection();
        projection.provider_sessions[0]
            .capabilities
            .turns
            .active_steering_interrupts_tools = Some(Some(interrupts_tools));
        let prepared = prepared_with_adapter(projection, "codex");
        let request = command(
            json!({"type":"message.dispatch","commandId":"task-question","threadId":"thread","messageId":"question-notice","text":"The delegated task has a question.","notification":{"source":{"kind":"delegated_task","taskIds":["task"],"childThreadId":"child"},"outcome":"updated","summary":"Task question"},"attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":"server"}),
        );
        let decision = PureDecisionEngine
            .decide_prepared(&prepared, &request)
            .unwrap();
        let events: Vec<_> = decision
            .events
            .iter()
            .map(|event| t3_contracts::stored::to_value(event).unwrap())
            .collect();
        if interrupts_tools {
            assert!(
                events.iter().any(|event| event["type"] == "run.created"
                    && event["payload"]["status"] == "queued")
            );
            assert!(decision.effects.is_empty());
        } else {
            assert!(!events.iter().any(|event| event["type"] == "run.created"));
            assert_eq!(decision.effects.len(), 1);
            assert!(matches!(
                decision.effects[0].request,
                t3_orchestration::effects::EffectRequest::Steer {
                    model_selection: None,
                    ..
                }
            ));
        }
    }
}

#[test]
fn followup_task_status_tracks_held_queued_and_started_child_runs_without_replacing_results() {
    let (prepared, mut child) = recovery_pair();
    child.runs[0] = change(
        &child.runs[0],
        json!({"delegatedTaskId":"task","status":"queued","queueHeld":true,"startedAt":null,"completedAt":null}),
    );
    let decision = PureDecisionEngine
        .decide_delegated_sync(&prepared, &child, &RunId("child-run".into()))
        .unwrap();
    assert_eq!(
        t3_contracts::stored::to_value(&decision.events[0]).unwrap()["payload"]["status"],
        "idle"
    );
    child.runs[0] = change(
        &child.runs[0],
        json!({"status":"starting","queueHeld":false}),
    );
    assert!(
        PureDecisionEngine
            .decide_delegated_sync(&prepared, &child, &RunId("child-run".into()))
            .unwrap()
            .events
            .is_empty()
    );
    let mut completed = prepared;
    completed.context.projection.as_mut().unwrap().subagents[0] = change(
        &completed.context.projection.as_ref().unwrap().subagents[0],
        json!({"result":"Already published"}),
    );
    child.runs[0] = change(&child.runs[0], json!({"status":"queued","queueHeld":true}));
    assert!(
        PureDecisionEngine
            .decide_delegated_sync(&completed, &child, &RunId("child-run".into()))
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn task_question_wakes_need_positive_observation_before_being_cancelled_as_stale() {
    use t3_orchestration::ports::decisions::orchestrator::stale_question_wake;
    let parent = task_projection();
    let mut child = runtime_projection();
    child.thread = change(&child.thread, json!({"id":"child"}));
    child.runtime_requests[0] = change(&child.runtime_requests[0], json!({"kind":"user_input"}));
    let request = t3_contracts::stored::to_value(&child.runtime_requests[0]).unwrap();
    let message = change(
        &super::command_policy_test::message("notice"),
        json!({"id":format!("message:delegated-task-question:thread:{}",request["id"].as_str().unwrap()),"notification":{"source":{"kind":"delegated_task","taskIds":["task"],"childThreadId":"child"},"outcome":"updated","summary":"Task question"}}),
    );
    assert!(!stale_question_wake(&parent, &message, None).unwrap());
    assert!(!stale_question_wake(&parent, &message, Some(&child)).unwrap());
    child.runtime_requests.clear();
    assert!(stale_question_wake(&parent, &message, Some(&child)).unwrap());
    let mut unknown = parent.clone();
    unknown.subagents.clear();
    assert!(!stale_question_wake(&unknown, &message, Some(&child)).unwrap());
}

#[test]
fn host_identity_observations_distinguish_authorities_and_legacy_links_use_the_compatibility_epoch()
{
    use t3_orchestration::ports::decisions::orchestrator::{
        PreparedPullRequestIdentities, pull_request_links_of,
    };
    let mut identities = PreparedPullRequestIdentities::default();
    let key = json!({"host":"github","repository":"org/repo","number":1});
    for host in ["github.com", "enterprise.example"] {
        let mut raw = key.clone();
        raw["url"] = json!(format!("https://{host}/org/repo/pull/1"));
        let canonical = json!({"host":host,"repository":"org/repo","number":1});
        identities
            .observe_key(&raw, serde_json::from_value(canonical.clone()).unwrap())
            .unwrap();
        assert_eq!(identities.key(&raw).unwrap(), canonical);
    }
    assert!(identities.key(&json!({"host":"github","repository":"org/repo","number":1,"url":"https://unknown.example/org/repo/pull/1"})).is_err());
    let legacy = json!({"projectId":"project","repository":"org/repo","number":1,"url":"https://github.com/org/repo/pull/1"});
    identities.legacy_keys.push((
        serde_json::from_value(legacy.clone()).unwrap(),
        serde_json::from_value(json!({"host":"github.com","repository":"org/repo","number":1}))
            .unwrap(),
    ));
    let mut thread = t3_contracts::stored::to_value(&projection().thread).unwrap();
    thread["linkedPullRequest"] = legacy;
    assert!(
        pull_request_links_of(&thread, &identities)
            .unwrap()
            .is_empty()
    );
    thread.as_object_mut().unwrap().shift_remove("pullRequests");
    let links = pull_request_links_of(&thread, &identities).unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["linkedAt"], "1970-01-01T00:00:00.000Z");
    assert_eq!(links[0]["url"], "https://github.com/org/repo/pull/1");
}

#[test]
fn queued_pull_request_wakes_drop_stale_members_and_rebuild_the_notice_for_current_members() {
    use t3_orchestration::ports::decisions::orchestrator::{
        PreparedPullRequestIdentities, PullRequestWatchWakeState, current_pull_request_watch_wake,
    };
    let mut identities = PreparedPullRequestIdentities::default();
    let mut projection = projection();
    let mut links = Vec::new();
    let mut stamps = Vec::new();
    for number in 1..=2 {
        let key = json!({"host":"github.com","repository":"org/repo","number":number});
        let typed = serde_json::from_value(key.clone()).unwrap();
        identities
            .keys
            .push((serde_json::from_value(key.clone()).unwrap(), typed));
        let mut link = key.clone();
        link["url"] = json!(format!("https://github.com/org/repo/pull/{number}"));
        link["source"] = json!("manual");
        link["linkedAt"] = json!(TEST_TIME);
        link["snapshot"] = serde_json::Value::Null;
        link["stack"] = serde_json::Value::Null;
        link["watch"] = json!({"startedAt":TEST_TIME,"headSha":"current","failedChecks":[],"passed":false,"passedChecks":[],"conflicting":false,"remarksThrough":TEST_TIME,"remarkIds":[],"wakes":1});
        identities
            .observe_key(&link, serde_json::from_value(key.clone()).unwrap())
            .unwrap();
        links.push(link);
        let mut stamp = key;
        stamp["watchStartedAt"] = json!(TEST_TIME);
        stamp["headSha"] = json!(if number == 1 { "old" } else { "current" });
        stamp["section"] = json!({"text":format!("PR {number} update"),"outcome":"updated","summary":format!("PR {number}")});
        stamps.push(stamp);
    }
    projection.thread = change(&projection.thread, json!({"pullRequests":links}));
    let message = change(
        &super::command_policy_test::message("watch"),
        json!({"text":"Combined notice","notification":{"source":{"kind":"monitor","pullRequest":stamps[0],"otherPullRequests":[stamps[1]]},"outcome":"updated","summary":"Both PRs"}}),
    );
    let PullRequestWatchWakeState::Updated(current) =
        current_pull_request_watch_wake(&projection, &message, &identities).unwrap()
    else {
        panic!("a stale member must be removed")
    };
    assert_eq!(current["text"], "PR 2 update");
    assert_eq!(
        current["notification"]["source"]["pullRequest"]["number"],
        2
    );
    assert!(
        current["notification"]["source"]["pullRequest"]
            .get("section")
            .is_none()
    );
    assert!(
        current["notification"]["source"]
            .get("otherPullRequests")
            .is_none()
    );
    let mut thread = t3_contracts::stored::to_value(&projection.thread).unwrap();
    thread["pullRequests"][1]["watch"]["headSha"] = json!("newer");
    projection.thread = serde_json::from_value(thread).unwrap();
    assert!(matches!(
        current_pull_request_watch_wake(&projection, &message, &identities).unwrap(),
        PullRequestWatchWakeState::Stale
    ));
}

#[test]
fn native_steering_uses_the_selected_model_for_its_switch_marker_when_selection_is_implicit() {
    let mut projection = running_projection();
    projection.thread = change(
        &projection.thread,
        json!({"modelSelection":{"instanceId":"codex","model":"b"}}),
    );
    let mut input = t3_contracts::stored::to_value(&projection.turn_items[0]).unwrap();
    input["type"] = json!("user_message");
    input["messageId"] =
        t3_contracts::stored::to_value(&projection.runs[0].user_message_id).unwrap();
    input["text"] = json!("Original prompt");
    input["attachments"] = json!([]);
    input["createdBy"] = json!("user");
    input["creationSource"] = json!("web");
    input["inputIntent"] = json!("turn_start");
    input["status"] = json!("completed");
    projection.turn_items = vec![serde_json::from_value(input).unwrap()];
    let mut prepared = prepared_with_adapter(projection, "codex");
    prepared.inputs.selection_transition =
        Some(t3_orchestration::provider::SelectionTransitionPlan::ApplyOnNextTurn);
    let decision=PureDecisionEngine.decide_prepared(&prepared,&command(json!({"type":"message.dispatch","commandId":"implicit-model-steer","threadId":"thread","messageId":"new-input","text":"Continue with the smaller change","attachments":[],"dispatchMode":{"type":"steer_active","targetRunId":"prepared"},"createdBy":"user","creationSource":"web"}))).unwrap();
    let item = decision
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .find(|event| {
            event["type"] == "turn-item.updated" && event["payload"]["type"] == "user_message"
        })
        .unwrap();
    assert_eq!(item["payload"]["modelSwitch"]["to"]["model"], "b");
}

struct GatedLineageInputs {
    base: t3_orchestration::runtime::dispatch::ProjectionCommandInputs,
    entered: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
    gated: std::sync::atomic::AtomicBool,
}

#[tokio::test]
async fn the_first_background_completion_is_unread_even_when_it_ends_in_the_dispatch_millisecond() {
    let harness = message_harness().await;
    let at = epoch_millis(TEST_TIME).unwrap() + 1000;
    harness.clock.set(&iso_from_epoch_millis(at));
    harness.dispatch(json!({"type":"message.dispatch","commandId":"unread-first","threadId":"thread","messageId":"unread-first-input","text":"Background work","attachments":[],"dispatchMode":{"type":"defer_start"},"createdBy":"agent","creationSource":"mcp"})).await.unwrap();
    let projection = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    harness.dispatch(json!({"type":"run.interrupt","commandId":"unread-first-end","threadId":"thread","runId":t3_contracts::stored::to_value(&projection.runs[0].id).unwrap()})).await.unwrap();
    let shell = harness
        .reader
        .shell(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        shell
            .last_visited_at
            .as_ref()
            .and_then(Option::as_ref)
            .map(|date| date.0.as_str()),
        Some(iso_from_epoch_millis(at - 1).as_str())
    );
    assert_eq!(
        shell.latest_run_completed_at.flatten().unwrap().0,
        iso_from_epoch_millis(at)
    );
    harness.clock.set(&iso_from_epoch_millis(at + 1000));
    harness.dispatch(json!({"type":"message.dispatch","commandId":"unread-second","threadId":"thread","messageId":"unread-second-input","text":"More background work","attachments":[],"dispatchMode":{"type":"defer_start"},"createdBy":"agent","creationSource":"mcp"})).await.unwrap();
    assert_eq!(
        harness
            .reader
            .shell(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap()
            .last_visited_at,
        shell.last_visited_at
    );
}

#[tokio::test]
async fn marking_unread_retires_only_expired_snoozes_and_preserves_every_other_thread_field() {
    for seconds in [60, 3600] {
        let harness = message_harness().await;
        let at = epoch_millis(TEST_TIME).unwrap();
        harness.dispatch(json!({"type":"message.dispatch","commandId":"snooze-send","threadId":"thread","messageId":"snooze-input","text":"Background work","attachments":[],"dispatchMode":{"type":"defer_start"},"createdBy":"agent","creationSource":"mcp"})).await.unwrap();
        let projection = harness
            .reader
            .projection(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap();
        harness.dispatch(json!({"type":"run.interrupt","commandId":"snooze-end","threadId":"thread","runId":t3_contracts::stored::to_value(&projection.runs[0].id).unwrap()})).await.unwrap();
        let wake = iso_from_epoch_millis(at + 1000 + seconds * 1000);
        harness.clock.set(&iso_from_epoch_millis(at + 1000));
        harness.dispatch(json!({"type":"thread.snooze","commandId":"snooze","threadId":"thread","snoozedUntil":wake})).await.unwrap();
        harness.clock.set(&iso_from_epoch_millis(at + 121000));
        harness.dispatch(json!({"type":"thread.visit","commandId":"dismiss-wake","threadId":"thread","visitedAt":iso_from_epoch_millis(at+121000)})).await.unwrap();
        let mut expected = t3_contracts::stored::to_value(
            &harness
                .reader
                .thread(ThreadId("thread".into()))
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        expected["lastVisitedAt"] = json!(iso_from_epoch_millis(at - 1));
        if seconds == 60 {
            expected["snoozedUntil"] = serde_json::Value::Null;
            expected["snoozedAt"] = serde_json::Value::Null;
            expected["lastSnoozeWakeAt"] = json!(wake);
        }
        harness
            .dispatch(
                json!({"type":"thread.mark-unread","commandId":"mark-unread","threadId":"thread"}),
            )
            .await
            .unwrap();
        assert_eq!(
            t3_contracts::stored::to_value(
                &harness
                    .reader
                    .thread(ThreadId("thread".into()))
                    .await
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn dispatch_records_success_failure_and_receipt_replay_once_each() {
    use t3_orchestration::runtime::dispatch::DispatchOutcome;
    let harness = Harness::new(None, false);
    let (metrics, mut observations) = tokio::sync::mpsc::unbounded_channel();
    harness
        .dispatcher
        .set_dispatch_observer(std::sync::Arc::new(
            super::orchestrator_receipt_race_test::DispatchMetrics(metrics),
        ))
        .unwrap();
    harness.create("thread").await.unwrap();
    assert_eq!(
        observations.recv().await.unwrap(),
        ("thread.create".into(), DispatchOutcome::Success)
    );
    let pin = json!({"type":"thread.pin","commandId":"metrics-pin","threadId":"thread"});
    harness.dispatch(pin.clone()).await.unwrap();
    harness.dispatch(pin).await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            observations.recv().await.unwrap(),
            ("thread.pin".into(), DispatchOutcome::Success)
        );
    }
    assert!(harness.dispatch(json!({"type":"thread.create","commandId":"metrics-duplicate","threadId":"thread","projectId":"project","title":"Duplicate","modelSelection":{"instanceId":"codex","model":"a"},"runtimeMode":"approval-required","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"})).await.is_err());
    assert_eq!(
        observations.recv().await.unwrap(),
        ("thread.create".into(), DispatchOutcome::Failure)
    );
    assert!(observations.try_recv().is_err());
}

#[tokio::test]
async fn a_running_provider_turn_does_not_decode_unrelated_finished_run_payloads() {
    let harness = message_harness().await;
    harness.dispatch(json!({"type":"message.dispatch","commandId":"old-run","threadId":"thread","messageId":"old-input","text":"Earlier work","attachments":[],"dispatchMode":{"type":"defer_start"},"createdBy":"user","creationSource":"web"})).await.unwrap();
    let projection = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    let run = t3_contracts::stored::to_value(&projection.runs[0]).unwrap();
    harness.database.stores.transaction(|db| {
        db.execute("UPDATE orchestration_v2_projection_runs SET status='completed',payload_json='{}' WHERE thread_id='thread'",[])?;
        Ok(())
    }).await.unwrap();
    let event = serde_json::from_value(json!({"id":"new-running-turn","threadId":"thread","type":"provider-turn.updated","runId":run["id"],"occurredAt":TEST_TIME,"payload":{"id":"turn","providerThreadId":run["providerThreadId"],"nodeId":run["rootNodeId"],"runAttemptId":run["activeAttemptId"],"nativeTurnRef":{"driver":"codex","nativeId":"native-turn","strength":"strong"},"ordinal":1,"status":"running","startedAt":TEST_TIME,"completedAt":null}})).unwrap();
    harness
        .dispatcher
        .handle_provider_turn_update(&OrchestrationV2StoredEvent {
            sequence: 1,
            command_id: None,
            event,
        })
        .await
        .unwrap();
}

#[test]
fn queued_start_failure_republishes_the_prompt_and_ends_only_the_still_queued_execution() {
    let mut projection = prepared_projection();
    projection.runs[0] = change(
        &projection.runs[0],
        json!({"status":"queued","queuePosition":1}),
    );
    projection.turn_items[0] = change(
        &projection.turn_items[0],
        json!({"type":"user_message","messageId":"queued-input","text":"Keep this prompt","attachments":[],"createdBy":"user","creationSource":"web","inputIntent":"turn_start","status":"completed"}),
    );
    let mut prepared = prepared_with_adapter(projection, "codex");
    prepared
        .inputs
        .next_item_ordinals
        .push((ThreadId("thread".into()), None, 50));
    let failure: OrchestrationV2ProviderFailure = serde_json::from_value(json!({"class":"unknown","message":"Native provider offline","code":"queued_start_failed","retryable":null})).unwrap();
    let decision = PureDecisionEngine
        .decide_queued_failure(&prepared, &RunId("prepared".into()), failure.clone())
        .unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "turn-item.updated",
            "run-attempt.updated",
            "node.updated",
            "turn-item.updated",
            "run.updated"
        ]
    );
    assert_eq!(events[0]["payload"]["text"], "Keep this prompt");
    assert_eq!(
        events[3]["payload"]["failure"],
        t3_contracts::stored::to_value(&failure).unwrap()
    );
    assert_eq!(events[3]["payload"]["ordinal"], 50);
    assert_eq!(events[4]["payload"]["status"], "failed");
    assert!(events[4]["payload"]["queuePosition"].is_null());
    assert!(decision.effects.is_empty());
    prepared.context.projection.as_mut().unwrap().runs[0] = change(
        &prepared.context.projection.as_ref().unwrap().runs[0],
        json!({"status":"running"}),
    );
    assert!(
        PureDecisionEngine
            .decide_queued_failure(&prepared, &RunId("prepared".into()), failure)
            .unwrap()
            .events
            .is_empty()
    );
}

struct BrokenQueuedInputs(t3_orchestration::runtime::dispatch::ProjectionCommandInputs);

#[test]
fn prepared_handoff_is_committed_with_the_new_run_and_its_final_session_binding() {
    use t3_orchestration::decision::engine::PreparedMessageContext;
    let mut projection = projection();
    projection.thread = change(
        &projection.thread,
        json!({"worktreePath":"C:/work/thread","activeProviderThreadId":"unavailable-source"}),
    );
    let mut prepared = prepared_with_adapter(projection, "codex");
    let provider = change(
        &prepared_projection().provider_threads[0],
        json!({"id":"handoff-target","providerSessionId":null,"firstRunOrdinal":null,"lastRunOrdinal":null,"handoffIds":[]}),
    );
    let handoff = serde_json::from_value(json!({"id":"handoff","threadId":"thread","targetRunId":"run:thread:thread:ordinal:1","fromProviderThreadIds":["unavailable-source"],"toProviderThreadId":"handoff-target","coveredRunOrdinals":{"from":1,"to":1},"strategy":"manual_context","status":"ready","summaryMessageId":null,"summaryText":"Earlier work","createdByProviderInstanceId":"codex","createdAt":TEST_TIME,"updatedAt":TEST_TIME})).unwrap();
    prepared.inputs.message_context = Some(PreparedMessageContext {
        provider_thread: provider,
        handoff: Some(handoff),
        transfers: vec![],
        released_sessions: vec![],
    });
    let send = command(
        json!({"type":"message.dispatch","commandId":"handoff-send","threadId":"thread","messageId":"handoff-input","text":"Continue","attachments":[],"dispatchMode":{"type":"start_immediately"},"createdBy":"user","creationSource":"web"}),
    );
    let decision = PureDecisionEngine
        .decide_prepared(&prepared, &send)
        .unwrap();
    let events: Vec<_> = decision
        .events
        .iter()
        .map(|event| t3_contracts::stored::to_value(event).unwrap())
        .collect();
    let run = &events
        .iter()
        .find(|event| event["type"] == "run.created")
        .unwrap()["payload"];
    let provider = &events
        .iter()
        .find(|event| event["type"] == "provider-thread.updated")
        .unwrap()["payload"];
    assert_eq!(run["contextHandoffId"], "handoff");
    assert_eq!(run["providerThreadId"], "handoff-target");
    assert_eq!(provider["providerSessionId"], "new-session");
    assert_eq!(provider["firstRunOrdinal"], 1);
    assert_eq!(provider["lastRunOrdinal"], 1);
    assert_eq!(provider["handoffIds"], json!(["handoff"]));
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "context-handoff.updated")
            .count(),
        1
    );
    let mut malformed = prepared.clone();
    malformed.inputs.message_context.as_mut().unwrap().handoff = Some(change(
        prepared
            .inputs
            .message_context
            .as_ref()
            .unwrap()
            .handoff
            .as_ref()
            .unwrap(),
        json!({"targetRunId":"another-run"}),
    ));
    assert!(
        PureDecisionEngine
            .decide_prepared(&malformed, &send)
            .unwrap_err()
            .to_string()
            .contains("does not belong to the run")
    );
}

#[tokio::test]
async fn interrupt_ignores_settled_tool_leaves_and_completed_transcript_items() {
    let harness = message_harness().await;
    harness.dispatch(json!({"type":"message.dispatch","commandId":"interrupt-read-send","threadId":"thread","messageId":"interrupt-read-input","text":"Work","attachments":[],"dispatchMode":{"type":"defer_start"},"createdBy":"user","creationSource":"web"})).await.unwrap();
    let projection = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    let run = t3_contracts::stored::to_value(&projection.runs[0]).unwrap()["id"].clone();
    harness.database.stores.transaction(|db| {
        db.execute("INSERT INTO orchestration_v2_projection_nodes (node_id,thread_id,run_id,parent_node_id,root_node_id,kind,status,payload_json) VALUES ('settled-leaf','thread','old-completed-run','old-root','old-root','tool_call','completed','{}')",[])?;
        db.execute("INSERT INTO orchestration_v2_projection_turn_items (turn_item_id,thread_id,ordinal,type,status,updated_at,payload_json) VALUES ('settled-reply','thread',99,'assistant_message','completed',?1,'{}')",[TEST_TIME])?;
        Ok(())
    }).await.unwrap();
    harness.dispatch(json!({"type":"run.interrupt","commandId":"interrupt-read","threadId":"thread","runId":run})).await.unwrap();
    let runs = harness
        .reader
        .query_records(ProjectionRecordQuery {
            thread_id: ThreadId("thread".into()),
            fields: vec![RecordKind::Runs],
            filter: RecordFilter::default(),
            messages: MessageFilter::default(),
            run_statuses: None,
            include_latest_run: false,
        })
        .await
        .unwrap();
    assert_eq!(
        t3_orchestration::runtime::dispatch::projection_from_records(runs).runs[0].status,
        OrchestrationV2RunStatus::Interrupted
    );
}

#[async_trait::async_trait]
impl t3_orchestration::runtime::dispatch::CommandInputs for BrokenQueuedInputs {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        Some(self.0.reader.as_ref())
    }
    async fn lock_keys(
        &self,
        command: &OrchestrationV2ServerCommand,
    ) -> t3_orchestration::error::Result<Vec<ThreadId>> {
        self.0.lock_keys(command).await
    }
    async fn prepare(
        &self,
        command: &OrchestrationV2ServerCommand,
    ) -> t3_orchestration::error::Result<t3_orchestration::runtime::dispatch::PreparedCommand> {
        if t3_contracts::stored::to_value(command)?["type"] == "message.dispatch" {
            return Err(t3_orchestration::provider::ProviderError::Operation {
                operation: t3_orchestration::provider::ProviderOperation::Capabilities,
                detail: "Native provider offline".into(),
            }
            .into());
        }
        self.0.prepare(command).await
    }
}

struct OfflineFailureFormatter;
impl t3_orchestration::runtime::dispatch::QueueStartFailureFormatter for OfflineFailureFormatter {
    fn failure(
        &self,
        _cause: &t3_orchestration::error::Error,
    ) -> t3_orchestration::error::Result<OrchestrationV2ProviderFailure> {
        Ok(serde_json::from_value(
            json!({"class":"unknown","message":"Native provider offline","code":"queued_start_failed","retryable":null}),
        )?)
    }
}

#[tokio::test]
async fn a_failed_queued_preparation_settles_in_sqlite_and_replaying_the_drain_is_idempotent() {
    use std::sync::Arc;
    use t3_orchestration::{
        runtime::dispatch::*,
        testing::{FakeProviderRegistry, SequenceIds},
    };
    let harness = message_harness().await;
    for (index, mode) in [(1, "start_immediately"), (2, "queue_after_active")] {
        harness.dispatch(json!({"type":"message.dispatch","commandId":format!("failure-send:{index}"),"threadId":"thread","messageId":format!("failure-input:{index}"),"text":"Work","attachments":[],"dispatchMode":{"type":mode},"createdBy":"user","creationSource":"web"})).await.unwrap();
    }
    let before = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    let run = change(
        &before.runs[0],
        json!({"status":"completed","completedAt":TEST_TIME}),
    );
    harness.sink.commit(CommitRequest { receipt: None, decision:Decision { events:vec![serde_json::from_value(json!({"id":"failure-terminal","threadId":"thread","type":"run.updated","runId":t3_contracts::stored::to_value(&run).unwrap()["id"],"occurredAt":TEST_TIME,"payload":t3_contracts::stored::to_value(&run).unwrap()})).unwrap()], ..Default::default() }, guard:CommitGuard::None }).await.unwrap();
    let dispatcher = Dispatcher::with_command_lanes(
        DispatchServices {
            database: harness.database.stores.clone(),
            inputs: Arc::new(BrokenQueuedInputs(ProjectionCommandInputs {
                reader: harness.reader.clone(),
                providers: Arc::new(FakeProviderRegistry::default()),
                clock: harness.clock.clone(),
                ids: Arc::new(SequenceIds::new("queued-failure")),
            })),
            engine: Arc::new(PureDecisionEngine),
            committer: harness.sink.clone(),
            recovery: Arc::new(Recovery::default()),
            clock: harness.clock.clone(),
        },
        None,
        harness.dispatcher.command_lanes().unwrap(),
    );
    dispatcher
        .set_queue_start_failure_formatter(Arc::new(OfflineFailureFormatter))
        .unwrap();
    dispatcher
        .resume_queue(ThreadId("thread".into()), None)
        .await
        .unwrap();
    let after = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.runs[1].status, OrchestrationV2RunStatus::Failed);
    let errors: Vec<_> = after
        .turn_items
        .iter()
        .map(|item| t3_contracts::stored::to_value(item).unwrap())
        .filter(|item| item["type"] == "error")
        .collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["failure"]["code"], "queued_start_failed");
    dispatcher
        .resume_queue(ThreadId("thread".into()), None)
        .await
        .unwrap();
    assert_eq!(
        harness
            .reader
            .projection(ThreadId("thread".into()))
            .await
            .unwrap()
            .unwrap(),
        after
    );
}
#[async_trait::async_trait]
impl t3_orchestration::runtime::dispatch::CommandInputs for GatedLineageInputs {
    fn reader(&self) -> Option<&dyn ProjectionReader> {
        Some(self.base.reader.as_ref())
    }
    async fn lock_keys(
        &self,
        command: &OrchestrationV2ServerCommand,
    ) -> t3_orchestration::error::Result<Vec<ThreadId>> {
        if t3_contracts::stored::to_value(command)?["commandId"] == "ordered-user"
            && !self.gated.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.base.lock_keys(command).await
    }
    async fn prepare(
        &self,
        command: &OrchestrationV2ServerCommand,
    ) -> t3_orchestration::error::Result<t3_orchestration::runtime::dispatch::PreparedCommand> {
        self.base.prepare(command).await
    }
}

#[tokio::test]
async fn a_user_message_keeps_run_one_while_its_lineage_read_yields_to_a_later_agent_send() {
    use std::sync::{Arc, atomic::AtomicBool};
    use t3_orchestration::{
        runtime::dispatch::*,
        testing::{FakeProviderRegistry, SequenceIds},
    };
    let harness = message_harness().await;
    let providers = Arc::new(FakeProviderRegistry::default());
    for _ in 0..2 {
        providers.metadata.push(Ok(AdapterMetadata {
            driver: t3_contracts::provider_instance::ProviderDriverKind("codex".into()),
            continuation_key: "test".into(),
            enabled: true,
            capabilities: super::command_policy_test::capabilities(),
        }));
    }
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let inputs = Arc::new(GatedLineageInputs {
        base: ProjectionCommandInputs {
            reader: harness.reader.clone(),
            providers,
            clock: harness.clock.clone(),
            ids: Arc::new(SequenceIds::new("ordered")),
        },
        entered: entered.clone(),
        release: release.clone(),
        gated: AtomicBool::new(false),
    });
    let recovery = Arc::new(Recovery::default());
    let dispatcher = Dispatcher::new(
        DispatchServices {
            database: harness.database.stores.clone(),
            inputs,
            engine: Arc::new(PureDecisionEngine),
            committer: harness.sink.clone(),
            recovery,
            clock: harness.clock.clone(),
        },
        None,
    );
    let user = {
        let dispatcher = dispatcher.clone();
        tokio::spawn(async move {
            dispatcher.dispatch(command(json!({"type":"message.dispatch","commandId":"ordered-user","threadId":"thread","messageId":"user-first","text":"User first","attachments":[],"dispatchMode":{"type":"defer_start"},"createdBy":"user","creationSource":"web"}))).await
        })
    };
    entered.notified().await;
    let (attempted, arrived) = tokio::sync::oneshot::channel();
    let agent = tokio::spawn(async move {
        attempted.send(()).unwrap();
        dispatcher.dispatch(command(json!({"type":"message.dispatch","commandId":"ordered-agent","threadId":"thread","messageId":"agent-second","text":"Agent second","attachments":[],"dispatchMode":{"type":"queue_after_active"},"createdBy":"agent","creationSource":"mcp"}))).await
    });
    arrived.await.unwrap();
    release.notify_one();
    user.await.unwrap().unwrap();
    agent.await.unwrap().unwrap();
    let projection = harness
        .reader
        .projection(ThreadId("thread".into()))
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<_> = projection
        .runs
        .iter()
        .map(|run| t3_contracts::stored::to_value(&run.user_message_id).unwrap())
        .collect();
    assert_eq!(
        ids,
        json!(["user-first", "agent-second"])
            .as_array()
            .unwrap()
            .clone()
    );
}
