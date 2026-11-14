use serde_json::{Value, json};
use t3_orchestration::{
    ports::acp_replay::{
        adapters::open_code_adapter_v2_testkit::*,
        testkit::provider_replay_fixtures::find_replay_fixture,
    },
    testing::replay::{ReplayGate, ReplayTranscript},
};
use tokio_util::sync::CancellationToken;

fn transcript(entries: Value) -> ReplayTranscript {
    serde_json::from_value(json!({"provider":"opencode","protocol":OPENCODE_PROTOCOL,"version":"1","scenario":"sdk","entries":entries})).unwrap()
}

#[tokio::test]
async fn opencode_real_simple_sdk_recording_retargets_messages_and_streams_text()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture =
        find_replay_fixture("simple", "opencode")?.expect("registered OpenCode recording");
    let controller = OpenCodeReplayController::new(fixture.transcript, ReplayGate::default())?;
    controller.subscribe()?;
    let session = controller
        .request(
            "session.create",
            json!({"title":"Replay fixture: simple","permission":[],"directory":"/isolated"}),
        )
        .await?;
    assert_eq!(session["id"], "ses_1236c522dffeunHBidt20OIzWc");
    let response=controller.request("session.promptAsync",json!({"sessionID":session["id"],"model":{"providerID":"openai","modelID":"gpt-5.4-mini"},"agent":"build",
        "parts":[{"type":"text","text":"Respond with the following text: fixture simple ok"}],"messageID":"local-user-message"})).await?;
    assert!(response.is_null());
    let cancellation = CancellationToken::new();
    let mut text = String::new();
    let mut saw_user = false;
    while let Some(event) = controller.next_event(&cancellation).await? {
        if event["type"] == "message.updated" && event["properties"]["info"]["role"] == "user" {
            assert_eq!(event["properties"]["info"]["id"], "local-user-message");
            saw_user = true;
        }
        if event["type"] == "message.part.updated" && event["properties"]["part"]["type"] == "text"
        {
            text = event["properties"]["part"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
        }
        if event["type"] == "session.status" && event["properties"]["status"]["type"] == "idle" {
            break;
        }
    }
    assert!(saw_user);
    assert_eq!(text, "fixture simple ok");
    assert_eq!(
        controller
            .request("session.abort", json!({"sessionID":session["id"]}))
            .await?,
        true
    );
    assert_eq!(
        controller
            .request("session.children", json!({"sessionID":session["id"]}))
            .await?,
        json!([])
    );
    controller.assert_complete()?;
    Ok(())
}

#[tokio::test]
async fn opencode_sse_events_precede_the_concurrent_http_response_and_gates_cancel()
-> Result<(), Box<dyn std::error::Error>> {
    let gate = ReplayGate::new(["held".into()])?;
    let controller = OpenCodeReplayController::new(
        transcript(json!([
            {"type":"expect_outbound","frame":{"type":"event.subscribe"}},
            {"type":"expect_outbound","frame":{"type":"session.get","input":{"id":"s"}}},
            {"type":"emit_inbound","label":"held","frame":{"type":"sdk.event","event":{"type":"session.status"}}},
            {"type":"emit_inbound","frame":{"type":"sdk.response","operation":"session.get","data":{"id":"s"}}},
            {"type":"runtime_exit","status":"success"}
        ])),
        gate.clone(),
    )?;
    controller.subscribe()?;
    let cancellation = CancellationToken::new();
    let (response, event, _) = tokio::join!(
        controller.request("session.get", json!({"id":"s"})),
        controller.next_event(&cancellation),
        async {
            assert!(gate.wait_for_reached("held").await);
            gate.release("held");
        }
    );
    assert_eq!(response?["id"], "s");
    assert_eq!(event?.unwrap()["type"], "session.status");
    assert!(controller.next_event(&cancellation).await?.is_none());
    assert!(controller.exited());
    controller.assert_complete()?;

    let gate = ReplayGate::new(["held".into()])?;
    let controller = OpenCodeReplayController::new(
        transcript(
            json!([{"type":"emit_inbound","label":"held","frame":{"type":"sdk.event","event":{}}}]),
        ),
        gate.clone(),
    )?;
    let cancellation = CancellationToken::new();
    let (event, _) = tokio::join!(controller.next_event(&cancellation), async {
        assert!(gate.wait_for_reached("held").await);
        cancellation.cancel();
    });
    assert!(event?.is_none());
    assert!(!controller.finished());
    Ok(())
}

#[tokio::test]
async fn opencode_rejects_duplicate_actual_message_ids_and_missing_responses() {
    let controller=OpenCodeReplayController::new(transcript(json!([
        {"type":"expect_outbound","frame":{"type":"session.promptAsync","input":{"messageID":"one"}}},
        {"type":"expect_outbound","frame":{"type":"session.promptAsync","input":{"messageID":"two"}}}
    ])),ReplayGate::default()).unwrap();
    controller
        .expect_outbound(json!({"type":"session.promptAsync","input":{"messageID":"actual"}}))
        .unwrap();
    let error = controller
        .expect_outbound(json!({"type":"session.promptAsync","input":{"messageID":"actual"}}))
        .unwrap_err();
    assert_eq!(error.cursor, 1);
    assert_eq!(controller.assert_complete().unwrap_err().cursor, 1);
    let controller = OpenCodeReplayController::new(
        transcript(json!([{"type":"runtime_exit","status":"success"}])),
        ReplayGate::default(),
    )
    .unwrap();
    assert_eq!(
        controller.response("session.get").await.unwrap_err().detail,
        "response unavailable"
    );
    assert!(replay_value_matches(
        &json!({"nested":{"x":"<workspace>"}}),
        &json!({"nested":{"x":7,"extra":true},"extra":[]})
    ));
    assert!(!replay_value_matches(&json!([1]), &json!([1, 2])));
    assert!(!replay_value_matches(&json!(0), &json!(-0.0)));
}
