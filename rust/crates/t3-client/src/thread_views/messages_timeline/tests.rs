use super::*;

// TypeScript casts partial payloads; Rust fixtures supply required, presentation-irrelevant fields.
fn complete_payloads(v: &mut Value) {
    match v {
        Value::Array(values) => {
            for v in values {
                complete_payloads(v);
            }
        }
        Value::Object(fields) => {
            for (key, v) in fields.iter_mut() {
                if key == "attempt" && v.is_object() {
                    for (key, default) in [
                        ("providerInstanceId", json!("fixture-provider")),
                        ("providerThreadId", json!("fixture-provider-thread")),
                        ("providerTurnId", Value::Null),
                        ("startedAt", Value::Null),
                        ("reason", json!("initial")),
                    ] {
                        v.as_object_mut().unwrap().entry(key).or_insert(default);
                    }
                }
                if matches!(key.as_str(), "projectedItem" | "subagents") {
                    let complete = |v: &mut Value| {
                        if !v.is_object() {
                            return;
                        }
                        for (key, default) in [
                            ("position", json!(0)),
                            ("visibility", json!("local")),
                            ("sourceThreadId", json!("fixture-thread")),
                            ("sourceItemId", json!("fixture-item")),
                        ] {
                            v.as_object_mut().unwrap().entry(key).or_insert(default);
                        }
                        complete_item(&mut v["item"]);
                    };
                    if let Some(values) = v.as_array_mut() {
                        for v in values {
                            complete(v);
                        }
                    } else {
                        complete(v);
                    }
                } else if key == "structuredPayload" {
                    complete_item(v);
                }
                complete_payloads(v);
            }
        }
        _ => {}
    }
}
fn complete_item(v: &mut Value) {
    if !v.is_object() {
        return;
    }
    if v["updatedAt"].is_object() {
        v["updatedAt"] = json!("2026-01-01T00:00:00Z");
    }
    loop {
        match serde_json::from_value::<t3_contracts::orchestration_v2::OrchestrationV2TurnItem>(
            v.clone(),
        ) {
            Ok(item) => {
                *v = value(&item);
                return;
            }
            Err(e) => {
                let error = e.to_string();
                let Some(field) = error
                    .strip_prefix("missing field `")
                    .and_then(|s| s.split('`').next())
                else {
                    panic!("fixture turn item: {error}: {v}");
                };
                let default = match field {
                    "ordinal" => json!(0),
                    "status" => json!("completed"),
                    "updatedAt" => json!("2026-01-01T00:00:00Z"),
                    "origin" => json!("provider_native"),
                    "driver" => json!("codex"),
                    "strategy" => json!("manual_context"),
                    "fromProviderThreadIds"
                    | "fromProviderInstanceIds"
                    | "questions"
                    | "attachments" => json!([]),
                    "input" | "prompt" | "text" => json!(""),
                    "streaming" => json!(false),
                    "runId" | "nodeId" | "providerThreadId" | "providerTurnId"
                    | "nativeItemRef" | "parentItemId" | "title" | "startedAt" | "completedAt"
                    | "targetRunId" | "childThreadId" | "result" => Value::Null,
                    "messageId"
                    | "id"
                    | "threadId"
                    | "targetThreadId"
                    | "targetProviderInstanceId"
                    | "targetModel"
                    | "contextHandoffId"
                    | "toProviderThreadId"
                    | "toProviderInstanceId"
                    | "subagentId"
                    | "providerInstanceId" => json!(format!("fixture-{field}")),
                    _ => panic!("unhandled fixture field {field}: {v}"),
                };
                v.as_object_mut().unwrap().insert(field.into(), default);
            }
        }
    }
}

fn input(v: &Value) -> MessagesTimelineRowsInput {
    let entries: Vec<_> = v["timelineEntries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| {
            Rc::new(
                serde_json::from_value(e.clone())
                    .unwrap_or_else(|err| panic!("entry {}: {err}", e["id"])),
            )
        })
        .collect();
    MessagesTimelineRowsInput {
        timeline_entries: entries,
        pending_approval_request_ids: strings(&v["pendingApprovalRequestIds"])
            .map(str::to_owned)
            .collect(),
        latest_run: v
            .get("latestRun")
            .filter(|v| !v.is_null())
            .map(|r| serde_json::from_value(r.clone()).expect("run")),
        running_run_id: optional_id(&v["runningRunId"]),
        expanded_run_ids: strings(&v["expandedRunIds"]).map(str::to_owned).collect(),
        expanded_attempt_ids: strings(&v["expandedAttemptIds"])
            .map(str::to_owned)
            .collect(),
        expanded_work_group_ids: strings(&v["expandedWorkGroupIds"])
            .map(str::to_owned)
            .collect(),
        is_working: v["isWorking"] == true,
        runless_work_active: v["runlessWorkActive"] == true,
        active_turn_started_at: v["activeTurnStartedAt"].as_str().map(str::to_owned),
        turn_diff_summaries: v["turnDiffSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        supports_conversation_rollback: v["supportsConversationRollback"] == true,
        thread_start_checkpoint_available: v["threadStartCheckpointAvailable"] == true,
        available_revert_turn_counts: v["availableRevertTurnCounts"]
            .as_array()
            .map(|v| v.iter().filter_map(Value::as_u64).collect()),
        worktree_setup: v.get("worktreeSetup").filter(|v| !v.is_null()).cloned(),
        tool_call_visibility: v["toolCallVisibility"].as_str().unwrap_or("showAll").into(),
        revealed_message_id: optional_id(&v["revealedMessageId"]),
    }
}
fn assert_case(case: &Value) {
    let mut case = case.clone();
    complete_payloads(&mut case);
    let args = &case["input"];
    let expected = &case["output"];
    let actual = match s(&case["function"]) {
        "deriveMessagesTimelineRows" => {
            let input = input(&args[0]);
            let rows = derive_messages_timeline_rows(&input);
            for row in &rows {
                if let MessagesTimelineRowContent::Message { message, .. }
                | MessagesTimelineRowContent::AssistantMeta { message, .. } = &row.content
                {
                    let source = input
                        .timeline_entries
                        .iter()
                        .find_map(|e| match &e.content {
                            session::TimelineContent::Message {
                                message: source, ..
                            } if source.id == message.id => Some(source),
                            _ => None,
                        })
                        .expect("message source");
                    assert!(
                        Rc::ptr_eq(message, source),
                        "timeline rows must hold the shared message"
                    );
                }
                let grouped = match &row.content {
                    MessagesTimelineRowContent::Work {
                        grouped_entries, ..
                    }
                    | MessagesTimelineRowContent::WorkLive {
                        grouped_entries, ..
                    } => Some(grouped_entries),
                    _ => None,
                };
                if let Some(grouped) = grouped {
                    for entry in grouped {
                        if let Some(projected) = &entry.projected_item {
                            let source = input
                                .timeline_entries
                                .iter()
                                .find_map(|e| match &e.content {
                                    session::TimelineContent::Work { entry: source }
                                        if source.id == entry.id =>
                                    {
                                        source.projected_item.as_ref()
                                    }
                                    _ => None,
                                })
                                .expect("projected work source");
                            assert!(
                                Rc::ptr_eq(projected, source),
                                "work rows must hold the shared projection"
                            );
                        }
                    }
                }
            }
            value(&rows)
        }
        "deriveMessagesTimelineRowsWithState" => {
            let previous =
                args.get(1)
                    .filter(|v| !v.is_null())
                    .map(|p| MessagesTimelineRowsProjection {
                        input: input(&p["input"]),
                        rows: Rc::new(
                            serde_json::from_value(p["rows"].clone()).expect("previous rows"),
                        ),
                    });
            let next =
                derive_messages_timeline_rows_with_state(&input(&args[0]), previous.as_ref());
            if let Some(previous) = &previous {
                assert_reuse(&case["identity"], &next.rows, &previous.rows);
            }
            value(&next.rows)
        }
        "computeStableMessagesTimelineRows" => {
            let previous = StableMessagesTimelineRowsState {
                result: Rc::new(
                    serde_json::from_value(args[1]["result"].clone()).expect("stable rows"),
                ),
            };
            let next = compute_stable_messages_timeline_rows(
                serde_json::from_value(args[0].clone()).expect("rows"),
                &previous,
            );
            assert_reuse(&case["identity"], &next.result, &previous.result);
            value(&next.result)
        }
        "timelineTurnFoldRunIdsByEntryId" => {
            let input = input(&args[0]);
            let entries = helpers::shown_entries(&input, &raw_entries(&input));
            let result: Vec<_> = folds::turn_folds(&entries, &input)
                .into_iter()
                .flat_map(|f| {
                    f.members
                        .into_iter()
                        .filter(move |id| f.hidden.contains(id))
                        .map(move |id| json!([id, f.run_id]))
                })
                .collect();
            json!(result)
        }
        "liveAnswerPreview" => {
            let entries: Vec<_> = serde_json::from_value(args[0].clone()).expect("preview entries");
            json!(live_answer_preview(&entries))
        }
        "resolveAssistantMessageCopyState" => value(&resolve_assistant_message_copy_state(
            args[0]["text"].as_str(),
            args[0]["showCopyButton"] == true,
            args[0]["streaming"] == true,
        )),
        "workEntryDisplayLabel" => {
            let entry = serde_json::from_value(args[0].clone()).expect("work label entry");
            json!(work_entry_display_label(
                &entry,
                args.get(1).and_then(Value::as_str)
            ))
        }
        "liveWorkEntryLabel" => {
            let entry = serde_json::from_value(args[0].clone()).expect("live work entry");
            json!(live_work_entry_label(
                &entry,
                args.get(1).and_then(Value::as_str),
                args[2] == true
            ))
        }
        name => panic!("unhandled function {name}"),
    };
    let expected = match s(&case["function"]) {
        "deriveMessagesTimelineRowsWithState" => &expected["rows"],
        "computeStableMessagesTimelineRows" => &expected["result"],
        _ => expected,
    };
    let expected = if matches!(
        s(&case["function"]),
        "deriveMessagesTimelineRows"
            | "deriveMessagesTimelineRowsWithState"
            | "computeStableMessagesTimelineRows"
    ) {
        value(
            &serde_json::from_value::<Vec<MessagesTimelineRow>>(expected.clone())
                .expect("expected rows"),
        )
    } else {
        expected.clone()
    };
    if actual != expected {
        panic!(
            "{} / {}\n{}",
            case["test"],
            case["function"],
            difference("", &actual, &expected).unwrap_or_default()
        );
    }
}
fn assert_reuse(
    identity: &Value,
    next: &Rc<Vec<Rc<MessagesTimelineRow>>>,
    previous: &Rc<Vec<Rc<MessagesTimelineRow>>>,
) {
    if identity["sameArray"] == true {
        assert!(
            Rc::ptr_eq(next, previous),
            "unchanged row array was rebuilt"
        );
    }
    for (i, reused) in identity["reused"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(index) = reused.as_i64().filter(|i| *i >= 0) {
            assert!(
                Rc::ptr_eq(&next[i], &previous[index as usize]),
                "row {i} lost identity from previous row {index}"
            );
        }
    }
}
fn difference(path: &str, a: &Value, b: &Value) -> Option<String> {
    if a == b {
        return None;
    }
    if let (Some(a), Some(b)) = (a.as_object(), b.as_object()) {
        for key in a.keys().chain(b.keys()) {
            if let Some(diff) = difference(
                &format!("{path}.{key}"),
                a.get(key).unwrap_or(&Value::Null),
                b.get(key).unwrap_or(&Value::Null),
            ) {
                return Some(diff);
            }
        }
    }
    if let (Some(a), Some(b)) = (a.as_array(), b.as_array()) {
        if a.len() != b.len() {
            return Some(format!(
                "{path}: row counts {} != {}\nactual: {}\nexpected: {}",
                a.len(),
                b.len(),
                json!(a),
                json!(b)
            ));
        }
        for (i, (a, b)) in a.iter().zip(b).enumerate() {
            if let Some(diff) = difference(&format!("{path}[{i}]"), a, b) {
                return Some(diff);
            }
        }
    }
    Some(format!("{path}: {a} != {b}"))
}
fn check_test(index: usize) {
    static CORPUS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let corpus = CORPUS.get_or_init(|| {
        serde_json::from_str(include_str!("typescript-cases.json")).expect("TypeScript corpus")
    });
    let title = &corpus["titles"][index];
    let cases: Vec<_> = corpus["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|c| c["test"] == *title)
        .collect();
    assert!(!cases.is_empty(), "no captured calls for {title}");
    for case in cases {
        assert_case(case);
    }
}
include!("test_cases.rs");
