use super::thread_stream_test::command;
use serde_json::{Value, json};
use t3_contracts::orchestration_v2::*;
use t3_orchestration::ports::transport::wire_projection::*;
fn item(mut value: Value, kind: &str) -> OrchestrationV2TurnItem {
    value["type"] = kind.into();
    serde_json::from_value(value).unwrap()
}
fn wire(mut value: OrchestrationV2TurnItem) -> Value {
    project_turn_item_for_wire(&mut value).unwrap();
    serde_json::to_value(value).unwrap()
}
fn detail(mut value: OrchestrationV2TurnItem) -> Value {
    project_turn_item_for_detail(&mut value).unwrap();
    serde_json::to_value(value).unwrap()
}
fn tool(output: Value) -> OrchestrationV2TurnItem {
    let mut value = command("tool", 1);
    value["input"] = json!({"pr":42});
    value["toolName"] = "mcp__github__fetch_pr".into();
    value["output"] = output;
    item(value, "dynamic_tool")
}
#[test]
fn live_tools_omit_even_small_results_but_keep_input_and_image_identity() {
    let original = tool(json!({"ok":true}));
    let projected = wire(original.clone());
    assert!(projected.get("output").is_none());
    assert_eq!(projected["outputOmitted"], true);
    assert_eq!(projected["input"], json!({"pr":42}));
    let mut value = serde_json::to_value(original).unwrap();
    value["viewedImagePath"] = "/workspace/reference.png".into();
    value["output"] = json!({"data":"private-image-data"});
    let output = wire(item(value, "dynamic_tool"));
    assert_eq!(output["viewedImagePath"], "/workspace/reference.png");
    assert!(!output.to_string().contains("private-image-data"));
}
#[test]
fn dynamic_input_limits_count_json_escaping_and_never_split_utf16_pairs() {
    for (count, truncated) in [(8191, false), (8192, true)] {
        let mut value = serde_json::to_value(tool(Value::Null)).unwrap();
        value["input"] = "\"".repeat(count).into();
        let output = wire(item(value, "dynamic_tool"));
        if truncated {
            assert_eq!(
                output["input"],
                json!({"summary":format!("{}…","\"".repeat(159)),"truncated":true})
            );
        } else {
            assert_eq!(output["input"], "\"".repeat(count));
        }
    }
    let mut value = serde_json::to_value(tool(Value::Null)).unwrap();
    value["input"] = json!({"text":format!("{}😀{}","a".repeat(149),"b".repeat(17000))});
    assert_eq!(
        wire(item(value, "dynamic_tool"))["input"]["summary"],
        format!("{{\"text\":\"{}…", "a".repeat(149))
    );
}
#[test]
fn subagent_details_truncate_at_utf8_boundaries_and_are_idempotent() {
    let mut value = command("subagent", 1);
    value["subagentId"] = "child".into();
    value["origin"] = "app_owned".into();
    value["driver"] = "codex".into();
    value["providerInstanceId"] = "codex".into();
    value["childThreadId"] = Value::Null;
    value["prompt"] = "漢".repeat(12000).into();
    value["result"] = Value::Null;
    let prefix = MAX_DETAIL_STRING_BYTES - TRUNCATION_MARKER.len();
    value["progress"] = format!("{}😀{}", "a".repeat(prefix - 1), "x".repeat(100000)).into();
    let output = wire(item(value, "subagent"));
    assert!(output["prompt"].as_str().unwrap().len() <= MAX_DETAIL_STRING_BYTES);
    assert_eq!(
        output["progress"],
        format!("{}{TRUNCATION_MARKER}", "a".repeat(prefix - 1))
    );
    assert_eq!(wire(item(output.clone(), "subagent")), output);
}
#[test]
fn command_failure_evidence_respects_explicit_exit_codes() {
    for (text, failed) in [
        ("exit code2", false),
        ("exit code::2", false),
        ("exit code 2x", false),
        ("exit code: 2", true),
        ("exited with exit code 2x", true),
        ("exited with exit code :2x", false),
    ] {
        assert_eq!(tool_output_indicates_failure(text), failed, "{text}");
    }
    let mut value = command("command", 1);
    value.as_object_mut().unwrap().remove("exitCode");
    value["output"] = "cat: missing-file: No such file or directory".into();
    let output = wire(item(value.clone(), "command_execution"));
    assert!(output.get("output").is_none());
    assert_eq!(output["outputIndicatesFailure"], true);
    assert_eq!(wire(item(output.clone(), "command_execution")), output);
    for text in [
        "logs/app.log:12: ENOENT: no such file or directory",
        "fixtures/ci.txt:3: Process exited with exit code 2",
    ] {
        value["output"] = text.into();
        value["exitCode"] = 0.into();
        let output = wire(item(value.clone(), "command_execution"));
        assert!(output.get("outputIndicatesFailure").is_none());
        assert_eq!(output["outputOmitted"], true);
    }
    value["output"] = "".into();
    value["exitCode"] = 2.into();
    assert_eq!(
        wire(item(value, "command_execution"))["outputIndicatesFailure"],
        true
    );
}
#[test]
fn file_change_bodies_are_on_demand_and_failed_diffs_stay_visible() {
    let mut value = command("file", 1);
    value["fileName"] = "src/main.ts".into();
    value["additions"] = 3.into();
    value["deletions"] = 1.into();
    value["diffStr"] = "+new code".into();
    value["oldStr"] = "old code".into();
    value["newStr"] = "new code".into();
    let output = wire(item(value.clone(), "file_change"));
    for key in ["diffStr", "oldStr", "newStr"] {
        assert!(output.get(key).is_none());
    }
    assert_eq!(output["fileName"], "src/main.ts");
    assert_eq!(output["additions"], 3);
    let output = detail(item(value.clone(), "file_change"));
    assert_eq!(output["diffStr"], "+new code");
    assert!(output.get("oldStr").is_none());
    assert!(output.get("newStr").is_none());
    value.as_object_mut().unwrap().remove("diffStr");
    let output = detail(item(value.clone(), "file_change"));
    assert_eq!(output["oldStr"], "old code");
    assert_eq!(output["newStr"], "new code");
    value["diffStr"] = "String to replace not found".into();
    value["status"] = "failed".into();
    let output = wire(item(value, "file_change"));
    assert_eq!(output["diffStr"], "String to replace not found");
    assert!(output.get("newStr").is_none());
}
#[test]
fn live_tools_retain_only_result_identities_and_failure_metadata() {
    let output = wire(tool(
        json!({"isError":true,"structuredContent":{"threadId":"child-thread","messageId":"message","text":"PRIVATE_BODY"},"content":[{"type":"text","text":"PRIVATE_BODY"}]}),
    ));
    assert_eq!(
        output["output"],
        json!({"isError":true,"threadId":"child-thread","messageId":"message"})
    );
    assert!(!output.to_string().contains("PRIVATE_BODY"));
    assert_eq!(wire(item(output.clone(), "dynamic_tool")), output);
    for sent_message in [
        "  Draft:\n\n> Hi Sam,\n\n**Keep this Markdown.**  ".to_owned(),
        "界".repeat(4000),
        "🦀".repeat(2000),
    ] {
        let result = json!({"sentMessage":sent_message,"message":"PRIVATE_ACK"});
        for envelope in [
            result.clone(),
            json!(result.to_string()),
            json!({"structuredContent":result,"content":"PRIVATE_ALTERNATIVE"}),
            json!([{"type":"text","text":result.to_string()}]),
        ] {
            let projected = wire(tool(envelope));
            assert_eq!(projected["output"], json!({"sentMessage":sent_message}));
            assert!(!projected.to_string().contains("PRIVATE_ACK"));
            assert_eq!(wire(item(projected.clone(), "dynamic_tool")), projected);
        }
    }
    for sent_message in [
        " \n\t\u{feff}".to_owned(),
        "界".repeat(4001),
        "🦀".repeat(2001),
    ] {
        assert!(
            wire(tool(json!({"sentMessage":sent_message})))
                .get("output")
                .is_none()
        );
    }
    assert_eq!(
        wire(tool(
            json!({"isError":true,"structuredContent":{"sentMessage":"Draft"}})
        ))["output"],
        json!({"isError":true,"sentMessage":"Draft"})
    );
}
#[test]
fn on_demand_tool_output_strips_image_bytes_and_keeps_markers_when_truncated() {
    let image = json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"A".repeat(600000)}});
    let output = detail(tool(json!({"content":[image]})));
    assert_eq!(
        output["output"],
        json!({"content":[{"type":"image","mimeType":"image/png"}]})
    );
    let output = detail(tool(
        json!({"content":[{"type":"text","text":"x".repeat(300000)},{"type":"image","mimeType":"image/png","data":"AAAA"}]}),
    ));
    assert_eq!(
        output["output"][1],
        json!({"type":"image","mimeType":"image/png"})
    );
    assert!(output["output"].to_string().len() < 270000);
    let output = detail(tool(json!(vec![
        json!({"type":"image","data":"AAAA","mimeType":"image/png"});
        10000
    ])));
    assert_eq!(output["output"].as_array().unwrap().len(), 9);
    // Executing WireProjection.ts gives 314913 UTF-16 units, 314915 UTF-8 bytes.
    assert_eq!(output["output"].to_string().encode_utf16().count(), 314913);
    assert_eq!(output["output"].to_string().len(), 314915);
}
#[test]
fn small_app_results_keep_images_while_oversized_and_forged_results_are_bounded() {
    let app = json!({"attachmentId":"app-html","server":"repro","tool":"chart","resourceUri":"ui://repro/chart"});
    let small = json!({"t3McpApp":app,"result":{"content":[{"type":"image","mimeType":"image/png","data":"iVBORw0KGgo="}]}});
    assert_eq!(detail(tool(small.clone()))["output"], small);
    let large = json!({"t3McpApp":app,"result":{"content":[{"type":"image","mimeType":"image/png","data":"A".repeat(600000)}]}});
    assert_eq!(
        detail(tool(large))["output"]["result"],
        json!({"content":[{"type":"image","mimeType":"image/png"}]})
    );
    let forged = json!({"t3McpApp":{"resourceUri":"x".repeat(600000)},"result":{}});
    assert!(detail(tool(forged))["output"].to_string().len() < 270000);
}

#[test]
fn handoff_transcripts_are_absent_from_cards_and_live_events() {
    let mut value = command("handoff", 1);
    value["contextHandoffId"] = "handoff".into();
    value["fromProviderThreadIds"] = json!([]);
    value["toProviderThreadId"] = "target".into();
    value["fromProviderInstanceIds"] = json!([]);
    value["toProviderInstanceId"] = "codex".into();
    value["strategy"] = "full_thread_summary".into();
    value["summary"] = "PRIVATE_HANDOFF_TRANSCRIPT".into();
    let original = item(value, "handoff");
    assert!(wire(original.clone()).get("summary").is_none());
    assert!(detail(original.clone()).get("summary").is_none());
    let mut event=serde_json::from_value(json!({"id":"handoff-event","threadId":"thread","type":"turn-item.updated","occurredAt":"2026-09-08T00:00:00.000Z","payload":original})).unwrap();
    project_domain_event_for_wire(&mut event).unwrap();
    assert!(
        !serde_json::to_string(&event)
            .unwrap()
            .contains("PRIVATE_HANDOFF_TRANSCRIPT")
    );
    assert!(
        serde_json::to_string(&original)
            .unwrap()
            .contains("PRIVATE_HANDOFF_TRANSCRIPT")
    );
}

#[test]
fn checkpoint_cards_cap_files_keep_totals_and_dedupe_after_the_record() {
    let files:Vec<_>=(0..250).map(|i|json!({"path":format!("vendor/file-{i}.ts"),"kind":"modified","additions":2,"deletions":1})).collect();
    let mut record:OrchestrationV2DomainEvent=serde_json::from_value(json!({"id":"checkpoint-event","threadId":"thread","type":"checkpoint.captured","occurredAt":"2026-09-08T00:00:00.000Z","payload":{"id":"checkpoint","threadId":"thread","scopeId":"scope","runId":null,"nodeId":"node","parentCheckpointId":null,"ordinalWithinScope":1,"appRunOrdinal":null,"ref":"refs/t3/checkpoints/large","status":"ready","files":files,"capturedAt":"2026-09-08T00:00:00.000Z"}})).unwrap();
    let mut card = command("checkpoint-card", 1);
    card["checkpointId"] = "checkpoint".into();
    card["scopeId"] = "scope".into();
    card["files"] = json!(files);
    let mut card = item(card, "checkpoint");
    project_domain_event_for_wire(&mut record).unwrap();
    project_turn_item_for_wire(&mut card).unwrap();
    let mut deduper = LiveBodyDeduper::default();
    let mut record = thread_stream_event_item(OrchestrationV2StoredEvent {
        sequence: 1,
        command_id: None,
        event: record,
    });
    deduper.apply(&mut record);
    deduper.dedupe_item(&mut card);
    let card = serde_json::to_value(card).unwrap();
    assert_eq!(card["files"], json!([]));
    assert_eq!(card["filesOmitted"], true);
    assert_eq!(
        card["filesTotal"],
        json!({"count":250,"additions":500,"deletions":250})
    );
    let OrchestrationV2ThreadStreamItem::Event(record) = record else {
        panic!()
    };
    let record = serde_json::to_value(record.event).unwrap();
    assert_eq!(record["payload"]["files"].as_array().unwrap().len(), 200);
}

#[test]
fn stored_events_share_the_same_wire_projection_without_retaining_old_events() {
    use std::sync::Arc;
    let cache = WireEventCache::default();
    let original=Arc::new(OrchestrationV2StoredEvent{sequence:1,command_id:None,event:serde_json::from_value(json!({"id":"shared","threadId":"thread","type":"turn-item.updated","occurredAt":"2026-09-08T00:00:00.000Z","payload":tool(json!({"private":"body"}))})).unwrap()});
    let projected = cache.project(&original).unwrap();
    assert!(!Arc::ptr_eq(&original, &projected));
    assert!(Arc::ptr_eq(&projected, &cache.project(&original).unwrap()));
    let weak = Arc::downgrade(&original);
    drop(original);
    assert!(weak.upgrade().is_none());
    let detached = Arc::new(OrchestrationV2StoredEvent {
        sequence: 2,
        command_id: None,
        event: t3_orchestration::testing::detached_event("detached", "thread").unwrap(),
    });
    assert!(Arc::ptr_eq(&detached, &cache.project(&detached).unwrap()));
}

#[test]
fn live_body_dedupe_follows_delivery_order_and_forgets_after_32_records() {
    use t3_orchestration::ports::transport::thread_stream::coalesce_live_updated_events;
    fn message(sequence: u64, id: &str, text: &str) -> OrchestrationV2ThreadStreamItem {
        serde_json::from_value(json!({"kind":"event","sequence":sequence,"event":{"id":format!("event-{sequence}"),"threadId":"thread","type":"message.updated","occurredAt":"2026-09-08T00:00:00.000Z","payload":{"id":id,"threadId":"thread","runId":null,"nodeId":null,"role":"assistant","text":text,"attachments":[],"streaming":true,"createdBy":"agent","creationSource":"provider","createdAt":"2026-09-08T00:00:00.000Z","updatedAt":"2026-09-08T00:00:00.000Z"}}})).unwrap()
    }
    fn reply(sequence: u64, id: &str, text: &str) -> OrchestrationV2ThreadStreamItem {
        let mut value = command("reply", 1);
        value["type"] = "assistant_message".into();
        value["messageId"] = id.into();
        value["text"] = text.into();
        value["streaming"] = true.into();
        value["status"] = "running".into();
        serde_json::from_value(json!({"kind":"event","sequence":sequence,"event":{"id":format!("event-{sequence}"),"threadId":"thread","type":"turn-item.updated","occurredAt":"2026-09-08T00:00:00.000Z","payload":value}})).unwrap()
    }
    let mut deduper = LiveBodyDeduper::default();
    let mut events = coalesce_live_updated_events(vec![
        message(1, "message", "a"),
        reply(2, "message", "a"),
        message(3, "message", "ab"),
        reply(4, "message", "ab"),
    ]);
    assert_eq!(events.len(), 2);
    for event in &mut events {
        deduper.apply(event);
    }
    let value = serde_json::to_value(&events[1]).unwrap();
    assert_eq!(value["sequence"], 4);
    assert_eq!(value["event"]["payload"]["text"], "");
    assert_eq!(value["event"]["payload"]["textOmitted"], true);
    let mut deduper = LiveBodyDeduper::default();
    let mut first = reply(1, "message", "a");
    deduper.apply(&mut first);
    assert_eq!(
        serde_json::to_value(first).unwrap()["event"]["payload"]["text"],
        "a"
    );
    for index in 0..33 {
        deduper.apply(&mut message(index + 2, &format!("m{index}"), "body"));
    }
    let mut forgotten = reply(40, "m0", "body");
    deduper.apply(&mut forgotten);
    assert_eq!(
        serde_json::to_value(forgotten).unwrap()["event"]["payload"]["text"],
        "body"
    );
    let mut remembered = reply(41, "m32", "body");
    deduper.apply(&mut remembered);
    assert_eq!(
        serde_json::to_value(remembered).unwrap()["event"]["payload"]["textOmitted"],
        true
    );
}
