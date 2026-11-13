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
