use super::bounded_snapshot_transport_test::{commit, seed, service};
use super::thread_stream_test::command;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::orchestration_v2::*;
use t3_orchestration::{
    ports::transport::command_output_hub::*,
    testing::{TEST_TIME, TestDatabase},
};

#[test]
fn native_terminal_matches_executed_typescript_fixtures() {
    use t3_orchestration::ports::transport::command_output_hub::terminal_output::*;
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/transport_terminal_output.json")).unwrap();
    for case in cases {
        let mut state = TerminalOutputState::default();
        for chunk in case["chunks"].as_array().unwrap() {
            state = append_terminal_output(
                &state,
                chunk.as_str().unwrap(),
                case["max"].as_u64().unwrap() as usize,
            );
        }
        let raw = serde_json::to_value(&state).unwrap();
        let window: usize = raw["screen"]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| {
                line.as_array()
                    .unwrap()
                    .iter()
                    .map(|run| {
                        run["text"].as_str().map_or_else(
                            || run["text"].as_array().unwrap().len(),
                            |text| text.encode_utf16().count(),
                        )
                    })
                    .sum::<usize>()
            })
            .sum();
        let actual = json!({"text":state.text,"pending":state.pending,"truncated":state.truncated,"resume":terminal_output_resume_text(&state),"plain":terminal_output_plain_text(&state.text),"row":raw["screen"]["row"],"col":raw["screen"]["col"],"pen":raw["screen"]["pen"],"saved":raw["screen"]["saved"],"windowUnits":window});
        assert_eq!(actual, case["expected"], "{}", case["name"]);
    }
}
fn target() -> OrchestrationV2SubscribeCommandOutputInput {
    serde_json::from_value(json!({"threadId":"thread","itemId":"command"})).unwrap()
}
async fn write(
    sink: &t3_orchestration::runtime::commit::EventSink,
    id: &str,
    status: &str,
    output: Option<&str>,
) {
    let mut item = command("command", 1);
    item["status"] = status.into();
    if let Some(output) = output {
        item["output"] = output.into();
    } else {
        item.as_object_mut().unwrap().remove("output");
    }
    commit(sink,vec![serde_json::from_value(json!({"id":id,"threadId":"thread","type":"turn-item.updated","occurredAt":TEST_TIME,"payload":item})).unwrap()]).await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn expanded_output_gets_initial_replace_coalesced_append_overflow_replace_and_final() {
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    seed(&fixture, &sink).await.unwrap();
    write(&sink, "start", "running", None).await;
    let hub = CommandOutputHub::new(
        service.projections,
        service.events,
        Arc::new(NativeTerminalOutputFold),
    );
    hub.append(target(), "first".into()).await.unwrap();
    let mut stream = hub.subscribe(target()).await.unwrap();
    let initial = stream.pull().await.unwrap().unwrap();
    assert_eq!(initial.text, "first");
    assert_eq!(initial.kind, CommandOutputFrameKind::Replace);
    assert!(initial.running);
    hub.append(target(), " A".into()).await.unwrap();
    hub.append(target(), " B".into()).await.unwrap();
    let append = stream.pull().await.unwrap().unwrap();
    assert_eq!(append.text, " A B");
    assert_eq!(append.kind, CommandOutputFrameKind::Append);
    hub.append(target(), "x".repeat(17000)).await.unwrap();
    let replace = stream.pull().await.unwrap().unwrap();
    assert_eq!(replace.kind, CommandOutputFrameKind::Replace);
    assert!(replace.text.starts_with("first A B"));
    hub.finish(target()).await.unwrap();
    write(&sink, "finish", "completed", None).await;
    let final_frame = stream.pull().await.unwrap().unwrap();
    assert!(!final_frame.running);
    assert_eq!(final_frame.text, replace.text);
    assert!(stream.pull().await.unwrap().is_none());
    stream.close().await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn persisted_settlement_wins_over_live_output_and_legacy_bash_is_decoded() {
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    seed(&fixture, &sink).await.unwrap();
    write(&sink, "start", "running", Some("snapshot")).await;
    let hub = CommandOutputHub::new(
        service.projections,
        service.events,
        Arc::new(NativeTerminalOutputFold),
    );
    let mut stream = hub.subscribe(target()).await.unwrap();
    assert_eq!(stream.pull().await.unwrap().unwrap().text, "snapshot");
    hub.append(target(), "uncommitted tail".into())
        .await
        .unwrap();
    let legacy = r#"{"stdout":"out","stderr":"err","interrupted":false}"#;
    write(&sink, "finish", "completed", Some(legacy)).await;
    loop {
        let frame = stream.pull().await.unwrap().unwrap();
        if !frame.running {
            assert_eq!(frame.text, "out\nerr");
            break;
        }
    }
    stream.close().await.unwrap();
}
#[tokio::test]
async fn nonexistent_and_noncommand_rows_fail_instead_of_waiting_for_output() {
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    seed(&fixture, &sink).await.unwrap();
    let hub = CommandOutputHub::new(
        service.projections,
        service.events,
        Arc::new(NativeTerminalOutputFold),
    );
    let error = hub.subscribe(target()).await.err().unwrap();
    assert!(error.to_string().contains("No such command"));
    let mut item = command("command", 1);
    item["type"] = "assistant_message".into();
    item["messageId"] = "reply".into();
    item["text"] = "text".into();
    item["streaming"] = false.into();
    item["attachments"] = json!([]);
    commit(&sink,vec![serde_json::from_value(json!({"id":"non-command","threadId":"thread","type":"turn-item.updated","occurredAt":TEST_TIME,"payload":item})).unwrap()]).await.unwrap();
    let error = hub.subscribe(target()).await.err().unwrap();
    assert!(error.to_string().contains("No such command"));
}
#[tokio::test(start_paused = true)]
async fn live_eviction_invalidates_viewer_and_finished_retention_is_bounded() {
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    seed(&fixture, &sink).await.unwrap();
    write(&sink, "start", "running", Some("persisted")).await;
    let hub = CommandOutputHub::new(
        service.projections,
        service.events,
        Arc::new(NativeTerminalOutputFold),
    );
    hub.append(target(), "old".into()).await.unwrap();
    let mut stream = hub.subscribe(target()).await.unwrap();
    stream.pull().await.unwrap();
    for i in 0..64 {
        let mut next = target();
        next.item_id = format!("other-{i}").into();
        hub.append(next, "data".into()).await.unwrap();
    }
    hub.append(target(), "new".into()).await.unwrap();
    let frame = stream.pull().await.unwrap().unwrap();
    assert_eq!(frame.kind, CommandOutputFrameKind::Replace);
    assert_eq!(frame.text, "new");
    stream.close().await.unwrap();
    hub.finish(target()).await.unwrap();
    let mut retained = hub.subscribe(target()).await.unwrap();
    assert_eq!(retained.pull().await.unwrap().unwrap().text, "new");
    retained.close().await.unwrap();
    for i in 0..16 {
        let mut next = target();
        next.item_id = format!("finished-{i}").into();
        hub.append(next.clone(), "tail".into()).await.unwrap();
        hub.finish(next).await.unwrap();
    }
    let mut evicted = hub.subscribe(target()).await.unwrap();
    assert_eq!(evicted.pull().await.unwrap().unwrap().text, "persisted");
    evicted.close().await.unwrap();
}
