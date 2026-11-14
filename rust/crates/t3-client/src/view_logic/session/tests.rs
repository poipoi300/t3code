#![allow(clippy::unwrap_used)]
use super::*;
use serde_json::json;

pub(super) fn assert_typescript_case(text: &str) {
    let case: Value = serde_json::from_str(text).unwrap();
    let source = &case["input"];
    let input = TimelineEntriesInput {
        visible_turn_items: serde_json::from_value(source["visibleTurnItems"].clone()).unwrap(),
        optimistic_messages: serde_json::from_value(source["optimisticMessages"].clone()).unwrap(),
        anchored_messages: source
            .get("anchoredMessages")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_default(),
        attachment_url_by_id: source
            .get("attachmentUrlById")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_default(),
        attempts: source
            .get("attempts")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_default(),
        nodes: source
            .get("nodes")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_default(),
        plans: source
            .get("plans")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_default(),
    };
    let actual = derive_timeline_entries_from_visible_turn_items(&input);
    let mut expected = case["expected"].clone();
    // The Rust view retains the complete contract attempt, including provider provenance.
    for entry in expected.as_array_mut().unwrap() {
        if let Some(attempt) = entry.get_mut("attempt") {
            let id = attempt["id"].clone();
            if let Some(full) = input.attempts.iter().find(|a| wire_value(a)["id"] == id) {
                *attempt = wire_value(full);
            }
        }
    }
    fn canonicalize_contracts(value: &mut Value) {
        match value {
            Value::Array(values) => {
                for value in values {
                    canonicalize_contracts(value);
                }
            }
            Value::Object(object) => {
                for (key, value) in object {
                    if matches!(key.as_str(), "item" | "structuredPayload" | "toolData")
                        && let Ok(item) =
                            serde_json::from_value::<OrchestrationV2TurnItem>(value.clone())
                    {
                        *value = wire_value(&item);
                    } else {
                        canonicalize_contracts(value);
                    }
                }
            }
            _ => {}
        }
    }
    canonicalize_contracts(&mut expected);
    assert_eq!(wire_value(&actual), expected);
    assert_eq!(
        wire_value(
            &derive_timeline_entries_from_visible_turn_items_with_state(input, None).entries
        ),
        expected
    );
}

#[test]
fn run_status_is_settlement_boundary_and_stop_works_during_startup() {
    for status in [
        OrchestrationV2RunStatus::Preparing,
        OrchestrationV2RunStatus::Starting,
        OrchestrationV2RunStatus::Running,
        OrchestrationV2RunStatus::Queued,
        OrchestrationV2RunStatus::Waiting,
        OrchestrationV2RunStatus::Completed,
    ] {
        let run_id = IdOrIndex::Id(RunId("run".into()));
        let runtime = RuntimeView {
            status: Some(status.clone()),
            active_run_id: Some(run_id.clone()),
            activity_started_at: None,
        };
        let run = RunView {
            run_id,
            status: status.clone(),
            started_at: None,
            requested_at: None,
            completed_at: None,
        };
        assert!(!is_latest_run_settled(Some(&run), Some(&runtime)));
        assert!(!derive_can_interrupt_running_thread(false, Some(&runtime)));
        assert_eq!(
            derive_can_interrupt_running_thread(true, Some(&runtime)),
            status != OrchestrationV2RunStatus::Completed
        );
        assert_eq!(
            is_latest_run_settled(Some(&run), None),
            status == OrchestrationV2RunStatus::Completed
        );
    }
    assert!(!derive_can_interrupt_running_thread(true, None));
    assert!(!is_latest_run_settled(None, None));
    assert_eq!(derive_phase(None), SessionPhase::Disconnected);
}
fn image(id: &str, preview: Option<&str>) -> MessageAttachment {
    MessageAttachment {attachment:serde_json::from_value(json!({"type":"image","id":id,"name":"image.png","mimeType":"image/png","sizeBytes":42})).unwrap(),preview_url:preview.map(str::to_string)}
}
fn message() -> Rc<ChatMessage> {
    Rc::new(serde_json::from_value(json!({"id":"m","role":"user","text":"Inspect","runId":null,"createdAt":"2026-09-04T00:00:00.000Z","updatedAt":"2026-09-04T00:00:00.000Z","streaming":false,"attachments":[{"type":"image","id":"image","name":"image.png","mimeType":"image/png","sizeBytes":42}]})).unwrap())
}
#[test]
fn image_requests_preserve_gallery_order_and_exclude_local_previews() {
    let attachments = [
        image("image", None),
        image("second", None),
        image("crop", None),
        image("local", Some("blob:local")),
        image("inline", Some("data:image/png;base64,AA==")),
        image("provided", Some("https://image")),
        image("image", None),
    ];
    let result = select_message_image_resources(&attachments);
    assert_eq!(
        wire_value(&result),
        json!([{"_tag":"attachment","attachmentId":"image"},{"_tag":"attachment","attachmentId":"second"},{"_tag":"attachment","attachmentId":"crop"},{"_tag":"attachment","attachmentId":"provided"}])
    );
    assert_eq!(
        select_handoff_image_resources(
            &[message()],
            &BTreeMap::from([("m".into(), vec!["blob:local".into()])])
        )
        .len(),
        1
    );
    assert!(select_handoff_image_resources(&[message()], &BTreeMap::new()).is_empty());
}
#[test]
fn preview_projection_reuses_identity_renews_and_restores_source() {
    let source = message();
    let mut projector = MessageAttachmentPreviewProjector::default();
    assert!(Rc::ptr_eq(
        &source,
        &projector.project(source.clone(), |_| None)
    ));
    let first = projector.project(source.clone(), |_| Some("https://server.test/image".into()));
    assert!(Rc::ptr_eq(
        &first,
        &projector.project(source.clone(), |_| Some("https://server.test/image".into()))
    ));
    let renewed = projector.project(source.clone(), |_| {
        Some("https://server.test/renewed".into())
    });
    assert_ne!(renewed.attachments, first.attachments);
    assert_eq!(
        first.attachments.as_ref().unwrap()[0]
            .preview_url
            .as_deref(),
        Some("https://server.test/image")
    );
    assert!(Rc::ptr_eq(
        &source,
        &projector.project(source.clone(), |_| None)
    ));
}

fn send_to_user_item(name: &str, status: &str, output: Option<Value>) -> OrchestrationV2TurnItem {
    let mut item = json!({"id":"render","threadId":"render-thread","runId":"render-run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":2,"status":status,"title":null,"startedAt":"2026-09-04T12:00:02.000Z","completedAt":"2026-09-04T12:00:02.000Z","updatedAt":"2026-09-04T12:00:02.000Z","type":"dynamic_tool","toolName":name,"input":{"message":"Draft"}});
    if let Some(output) = output {
        item["output"] = output;
    }
    serde_json::from_value(item).unwrap()
}

fn sent_message(name: &str, output: Value) -> Option<String> {
    match send_to_user_item(name, "completed", Some(output)) {
        OrchestrationV2TurnItem::DynamicTool(tool) => sent_message_from_tool_item(&tool),
        _ => unreachable!("a dynamic tool"),
    }
}

#[test]
fn keeps_a_send_to_user_message_up_to_its_limit_even_in_3_byte_characters() {
    // toolOutput.test.ts, upstream aab388ddb1.
    let longest = "界".repeat(SEND_TO_USER_MAX_LENGTH);
    for text in ["Draft:\n\n> Hi Sam,".to_owned(), longest.clone()] {
        let reply = json!({"sentMessage": text, "message": "Shown to the user."}).to_string();
        assert_eq!(
            sent_message(
                "mcp__t3-code__send_to_user",
                json!([{"type":"text","text": reply}])
            ),
            Some(text)
        );
    }
    let over = json!({"sentMessage": format!("{longest}界")});
    assert_eq!(sent_message("mcp__t3-code__send_to_user", over), None);
    assert_eq!(
        sent_message("mcp__t3-code__send_to_user", json!({"sentMessage": "  "})),
        None
    );
    // OpenCode 2 Code Mode reports a script's nested send_to_user call as one `execute` item.
    let nested = json!({"sentMessage": "Draft", "message": "Shown to the user."}).to_string();
    assert_eq!(
        sent_message("execute", Value::String(nested)),
        Some("Draft".into())
    );
    assert_eq!(
        sent_message("mcp__t3-code__html_render", json!({"sentMessage": "Draft"})),
        None,
        "only send_to_user's own output is a sent message"
    );
    assert_eq!(
        sent_message(
            "mcp__t3-code__send_to_user",
            json!({"isError": true, "structuredContent": {"sentMessage": "Draft"}})
        ),
        None
    );
}

#[test]
fn shows_a_sent_message_in_place_and_only_once_sent() {
    // session-logic.test.ts "shows a sent message in place through the turn fold, and only once
    // sent", upstream aab388ddb1; the fold and row order belong to the timeline rows.
    let text = "Draft reply:\n\n> Thanks, Thursday works.";
    let entries = |status: &str, output: Option<Value>| {
        let row: OrchestrationV2ProjectedTurnItem = serde_json::from_value(json!({
            "position": 2,
            "visibility": "local",
            "sourceThreadId": "render-thread",
            "sourceItemId": "render",
            "item": send_to_user_item("mcp__t3-code__send_to_user", status, output),
        }))
        .unwrap();
        derive_timeline_entries_from_visible_turn_items(&TimelineEntriesInput {
            visible_turn_items: vec![Rc::new(row)],
            ..TimelineEntriesInput::default()
        })
    };
    let sent = entries(
        "completed",
        Some(json!({"structuredContent": {"sentMessage": text}, "content": []})),
    );
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].id, "render");
    assert_eq!(
        serde_json::to_value(&sent[0].content).unwrap(),
        json!({"kind": "sent-message", "runId": "render-run", "text": text})
    );
    assert!(
        entries("running", None)
            .iter()
            .all(|entry| !matches!(entry.content, TimelineContent::SentMessage { .. }))
    );
}

#[test]
fn summarizes_send_to_user_calls_as_sent_messages() {
    use crate::work_log::{T3ToolSummaryCall, summarize_t3_tool_calls};
    let call = || T3ToolSummaryCall {
        input: json!({"message": "Draft"}),
        output: json!({"sentMessage": "Draft"}),
        outcome: "completed".into(),
    };
    assert_eq!(
        summarize_t3_tool_calls("send-to-user", &[call(), call()]).label,
        "Sent 2 messages"
    );
}

fn work_entry(value: Value) -> WorkLogEntry {
    let mut value = value;
    let object = value.as_object_mut().unwrap();
    for (key, default) in [
        ("id", json!("e")),
        ("label", json!("")),
        ("createdAt", json!("")),
        ("tone", json!("tool")),
        ("runId", Value::Null),
    ] {
        object.entry(key).or_insert(default);
    }
    serde_json::from_value(value).unwrap()
}

#[test]
fn tool_group_actions_follow_the_shared_tool_classifier() {
    // presentation.ts computeToolGroupAction over toolActivity.ts classifyToolActivity.
    use crate::work_log::tool_group_action;
    for (entry, action) in [
        // The tool's kind decides before any name.
        (json!({"toolTitle":"Run","toolData":{"kind":"execute"}}), "command"),
        (json!({"toolTitle":"Read","toolData":{"kind":"edit"}}), "edit"),
        (json!({"toolTitle":"Lookup","toolData":{"kind":"search"}}), "code-search"),
        (json!({"toolTitle":"Open","toolData":{"kind":"read"}}), "read"),
        // A nested ACP tool name counts when nothing else names the tool.
        (json!({"toolData":{"item":{"tool":"grep"}}}), "code-search"),
        (json!({"toolData":{"toolName":"Bash"}}), "command"),
        // The bare label is not a tool name.
        (json!({"label":"Read"}), "other"),
        // A namespaced name is not a token.
        (json!({"toolData":{"toolName":"files.read"}}), "other"),
        (json!({"itemType":"web_search","toolTitle":"grep src"}), "code-search"),
        (json!({"itemType":"web_search","toolTitle":"Searched the web"}), "search"),
        (json!({"command":"ls"}), "command"),
        (json!({"tone":"info"}), "update"),
    ] {
        assert_eq!(tool_group_action(&work_entry(entry.clone())), action, "{entry}");
    }
}
