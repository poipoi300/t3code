use serde_json::{Value, json};
use t3_orchestration::{
    ports::acp_replay::{
        adapters::pi_adapter_v2_testkit::*, testkit::provider_replay_fixtures::replay_fixtures,
    },
    testing::replay::{ReplayEntry, ReplayTranscript},
};

#[test]
fn all_recorded_pi_and_muse_sessions_are_consumed_by_the_jsonl_controllers()
-> Result<(), Box<dyn std::error::Error>> {
    for (driver, flavor, start, marker, count) in [
        ("pi", JsonlFlavor::Pi, "process_start", "@p", 8),
        ("muse", JsonlFlavor::Muse, "host_start", "@h", 8),
    ] {
        let fixtures = replay_fixtures(driver)?;
        assert_eq!(fixtures.len(), count);
        for fixture in fixtures {
            let entries = fixture.transcript.entries.clone();
            let controller = JsonlReplayController::new(fixture.transcript, flavor)?;
            let mut processes = Vec::new();
            let mut incoming_count = 0;
            for entry in &entries {
                if let ReplayEntry::ExpectOutbound(entry) = entry {
                    let process = entry
                        .label
                        .as_ref()
                        .and_then(Option::as_ref)
                        .and_then(|label| label.0.rsplit_once(marker))
                        .and_then(|(_, ordinal)| ordinal.parse::<usize>().ok())
                        .unwrap_or(1);
                    if entry.frame["type"] == start {
                        let args: Vec<String> =
                            serde_json::from_value(entry.frame["args"].clone())?;
                        let handle = controller.spawn(&args)?;
                        assert_eq!(handle.ordinal, process);
                        processes.push(handle);
                    } else {
                        controller.receive(process, entry.frame.clone())?;
                    }
                }
                for process in &mut processes {
                    while process.incoming.try_recv().is_ok() {
                        incoming_count += 1;
                    }
                }
            }
            controller.assert_complete()?;
            assert_eq!(
                incoming_count,
                entries
                    .iter()
                    .filter(|entry| matches!(entry, ReplayEntry::EmitInbound(_)))
                    .count(),
                "{}",
                fixture.recording.file
            );
        }
    }
    Ok(())
}

fn transcript(entries: Value) -> ReplayTranscript {
    serde_json::from_value(json!({"provider":"pi","protocol":PI_RPC_REPLAY_PROTOCOL,"version":"1","scenario":"ordering","entries":entries})).unwrap()
}

#[tokio::test]
async fn pi_out_of_order_writes_bind_each_response_and_restore_another_process()
-> Result<(), Box<dyn std::error::Error>> {
    let controller = JsonlReplayController::new(
        transcript(json!([
            {"type":"expect_outbound","frame":{"type":"process_start","args":[]}},
            {"type":"expect_outbound","frame":{"type":"get_state","id":"t3-0"}},
            {"type":"expect_outbound","frame":{"type":"get_available_models","id":"t3-1"}},
            {"type":"emit_inbound","frame":{"type":"response","id":"t3-0","command":"get_state"}},
            {"type":"emit_inbound","frame":{"type":"response","id":"t3-1","command":"get_available_models"}},
            {"type":"runtime_exit","status":"success"},
            {"type":"expect_outbound","label":"process_start@p2","frame":{"type":"process_start","args":["resume"]}},
            {"type":"expect_outbound","label":"get_state@p2","frame":{"type":"get_state","id":"t3-0"}},
            {"type":"emit_inbound","label":"response:get_state@p2","frame":{"type":"response","id":"t3-0","command":"get_state"}}
        ])),
        JsonlFlavor::Pi,
    )?;
    let mut first = controller.spawn(&[])?;
    controller.receive(1, json!({"type":"get_available_models","id":"t3-102"}))?;
    assert!(first.incoming.try_recv().is_err());
    controller.receive(1, json!({"type":"get_state","id":"t3-101"}))?;
    assert_eq!(first.incoming.recv().await.unwrap()["id"], "t3-101");
    assert_eq!(first.incoming.recv().await.unwrap()["id"], "t3-102");
    assert!(first.exited.is_cancelled());
    assert!(first.incoming.recv().await.is_none());
    let mut second = controller.spawn(&["resume".into()])?;
    controller.receive(2, json!({"type":"get_state","id":"t3-201"}))?;
    assert_eq!(second.incoming.recv().await.unwrap()["id"], "t3-201");
    controller.assert_complete()?;
    Ok(())
}

#[test]
fn pi_rejects_writes_that_cross_a_turn_and_splits_multibyte_jsonl_chunks() {
    let controller = JsonlReplayController::new(
        transcript(json!([
            {"type":"expect_outbound","frame":{"type":"process_start","args":[]}},
            {"type":"expect_outbound","frame":{"type":"prompt","message":"first"}},
            {"type":"expect_outbound","frame":{"type":"prompt","message":"later"}}
        ])),
        JsonlFlavor::Pi,
    )
    .unwrap();
    let _ = controller.spawn(&[]).unwrap();
    let error = controller
        .receive(1, json!({"type":"prompt","message":"later"}))
        .unwrap_err();
    assert_eq!(error.cursor, 1);
    assert!(controller.assert_complete().is_err());
    let mut splitter = JsonLineSplitter::default();
    let bytes = "{\"text\":\"é🦀\"}\r\n\n{\"x\":1}\nunfinished".as_bytes();
    let mut lines = Vec::new();
    for chunk in bytes.chunks(1) {
        lines.extend(splitter.feed(chunk));
    }
    assert_eq!(lines, ["{\"text\":\"é🦀\"}", "{\"x\":1}"]);
    assert!(!replay_value_matches(
        &json!({"x":1}),
        &json!({"x":1,"extra":true})
    ));
    assert!(replay_value_matches(
        &json!({"x":"<any>"}),
        &json!({"x":null})
    ));
    assert_eq!(
        record_label(&json!({"type":"response","command":"get_state"}), 2).as_deref(),
        Some("response:get_state@p2")
    );
}
