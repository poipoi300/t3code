use super::bounded_snapshot_transport_test::{commit, seed, service};
use super::thread_stream_test::command;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::orchestration_v2::*;
use t3_orchestration::{
    error::Result,
    ports::transport::command_output_hub::*,
    testing::{TEST_TIME, TestDatabase},
};

struct PlainTextFold;
impl TerminalOutputFold for PlainTextFold {
    fn empty(&self) -> Value {
        json!("")
    }
    fn append(&self, state: &Value, chunk: &str) -> Result<Value> {
        Ok(json!(format!("{}{chunk}", state.as_str().unwrap())))
    }
    fn normalize(&self, text: &str) -> Result<TerminalText> {
        Ok(TerminalText {
            text: text.into(),
            truncated: false,
        })
    }
    fn resume(&self, state: &Value) -> Result<TerminalText> {
        self.normalize(state.as_str().unwrap())
    }
    fn settled(&self, state: &Value) -> Result<TerminalText> {
        self.resume(state)
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
    let hub = CommandOutputHub::new(service.projections, service.events, Arc::new(PlainTextFold));
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
    let hub = CommandOutputHub::new(service.projections, service.events, Arc::new(PlainTextFold));
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
    let hub = CommandOutputHub::new(service.projections, service.events, Arc::new(PlainTextFold));
    let error = hub.subscribe(target()).await.err().unwrap();
    assert!(error.to_string().contains("No such command"));
}
#[tokio::test(start_paused = true)]
async fn live_eviction_invalidates_viewer_and_finished_retention_is_bounded() {
    let fixture = TestDatabase::memory().unwrap();
    let (service, sink) = service(&fixture);
    seed(&fixture, &sink).await.unwrap();
    write(&sink, "start", "running", Some("persisted")).await;
    let hub = CommandOutputHub::new(service.projections, service.events, Arc::new(PlainTextFold));
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
}
