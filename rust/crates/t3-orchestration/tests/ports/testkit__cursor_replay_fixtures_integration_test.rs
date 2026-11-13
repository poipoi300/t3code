use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use t3_orchestration::{
    ports::acp_replay::{
        adapters::cursor_adapter_v2_testkit::*,
        testkit::provider_replay_fixtures::find_replay_fixture,
    },
    testing::replay::ReplayTranscript,
};

fn transcript(entries: Value) -> ReplayTranscript {
    serde_json::from_value(json!({"provider":"cursor","protocol":CURSOR_PROTOCOL,"version":"1","scenario":"runner","entries":entries})).unwrap()
}
fn outbound(frame: Value) -> Value {
    json!({"type":"expect_outbound","frame":frame})
}
fn inbound(frame: Value) -> Value {
    json!({"type":"emit_inbound","frame":frame})
}
fn setup_frames() -> Vec<Value> {
    vec![
        outbound(json!({"type":"agent.open","operation":"create","options":{}})),
        inbound(json!({"type":"agent.opened","agentId":"agent"})),
        outbound(json!({"type":"run.start","message":"hello","options":{}})),
        inbound(json!({"type":"run.started","agentId":"agent","runId":"run"})),
    ]
}

#[tokio::test]
async fn cursor_simple_recording_streams_to_completion() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = find_replay_fixture("simple", "cursor")?.expect("registered Cursor recording");
    let runner = CursorReplayRunner::new(fixture.transcript)?;
    let session=runner.open("create",None,&json!({"model":{"id":"composer-2.5"},"name":"Replay fixture: simple","mode":"agent","local":{"cwd":"/tmp/isolated","autoReview":false,"settingSources":["project","user","team","mdm","plugins"],"sandboxOptions":{"enabled":false},"enableAgentRetries":true}}))?;
    let run = session.send(
        json!("Respond with the following text: fixture simple ok"),
        &json!({"model":{"id":"composer-2.5"},"mode":"agent"}),
    )?;
    let updates = Arc::new(Mutex::new(Vec::new()));
    let result = run
        .wait(|update| {
            updates.lock().unwrap().push(update);
            async { Ok(()) }
        })
        .await?;
    assert_eq!(result["status"], "finished");
    assert_eq!(result["result"], "fixture simple ok");
    assert!(
        updates
            .lock()
            .unwrap()
            .iter()
            .any(|update| update["type"] == "text-delta")
    );
    session.close()?;
    runner.assert_complete()?;
    Ok(())
}

#[tokio::test]
async fn cursor_steering_and_cancel_wake_the_waiting_run_without_polling()
-> Result<(), Box<dyn std::error::Error>> {
    let mut frames = setup_frames();
    frames.extend([
        outbound(json!({"type":"run.steer","runId":"run","text":"change"})),
        inbound(json!({"type":"run.steered","runId":"run","outcome":{"status":"accepted"}})),
        outbound(json!({"type":"run.cancel","runId":"run"})),
        inbound(json!({"type":"run.completed","result":{"id":"run","status":"cancelled"}})),
        outbound(json!({"type":"agent.close","agentId":"agent"})),
    ]);
    let runner = CursorReplayRunner::new(transcript(json!(frames)))?;
    let session = runner.open("create", None, &json!({}))?;
    let run = session.send(json!("hello"), &json!({}))?;
    let (wait, steer) = tokio::join!(run.wait(|_| async { Ok(()) }), async {
        let ack = run.steer("change").await?;
        assert_eq!(ack["status"], "accepted");
        run.cancel()
    });
    steer?;
    assert_eq!(wait?["status"], "cancelled");
    session.close()?;
    runner.assert_complete()?;
    Ok(())
}

#[tokio::test]
async fn cursor_failures_are_sticky_and_keep_the_recorded_cursor() {
    let mut frames = setup_frames();
    frames.push(inbound(
        json!({"type":"interaction.update","runId":"wrong-run","update":{}}),
    ));
    let runner = CursorReplayRunner::new(transcript(json!(frames))).unwrap();
    let session = runner.open("create", None, &json!({})).unwrap();
    let run = session.send(json!("hello"), &json!({})).unwrap();
    let error = run.wait(|_| async { Ok(()) }).await.unwrap_err();
    assert_eq!(error.kind, CursorReplayErrorKind::Mismatch);
    assert_eq!(error.cursor, 4);
    assert_eq!(runner.assert_complete().unwrap_err().cursor, 4);
    assert_eq!(run.cancel().unwrap_err().cursor, 4);

    let mut frames = setup_frames();
    frames.push(inbound(
        json!({"type":"interaction.update","runId":"run","update":{"type":"text-delta"}}),
    ));
    let runner = CursorReplayRunner::new(transcript(json!(frames))).unwrap();
    let session = runner.open("create", None, &json!({})).unwrap();
    let run = session.send(json!("hello"), &json!({})).unwrap();
    let error = run
        .wait(|_| async { Err(json!("callback failed")) })
        .await
        .unwrap_err();
    assert_eq!(error.kind, CursorReplayErrorKind::Runtime);
    assert_eq!(error.cursor, 4);
    assert_eq!(error.actual, Some(json!("callback failed")));
}

#[test]
fn cursor_dynamic_handoff_normalization_and_recording_redaction_preserve_user_text() {
    let left = json!({"text":["Context handoff (restart):\nsummary A\n\nUser message:\nkeep me"]});
    let right = json!({"text":["Context handoff (restart):\nsummary B\n\nUser message:\nkeep me"]});
    assert_eq!(normalize_frame(&left), normalize_frame(&right));
    assert_ne!(
        normalize_frame(&left),
        normalize_frame(
            &json!({"text":["Context handoff (restart):\nsummary A\n\nUser message:\nchanged"]})
        )
    );
    assert_eq!(
        sanitize_replay_value(
            &json!({"/private/key":["/private/value",7]}),
            &[("/private", "<cwd>"), ("", "bad")]
        ),
        json!({"<cwd>/key":["<cwd>/value",7]})
    );
    let runner = CursorReplayRunner::new(transcript(json!([]))).unwrap();
    assert!(runner.assert_complete().is_ok());
    assert_eq!(
        runner.open("create", None, &json!({})).err().unwrap().kind,
        CursorReplayErrorKind::Exhausted
    );
}
