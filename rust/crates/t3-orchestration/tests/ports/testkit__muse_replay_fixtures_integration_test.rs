use serde_json::{Value, json};
use t3_orchestration::{
    ports::acp_replay::adapters::muse_adapter_v2_testkit::*,
    testing::replay::{ReplayEntry, ReplayTranscript},
};

#[tokio::test]
async fn muse_responses_rebind_client_ids_and_server_requests_keep_their_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let transcript: ReplayTranscript = serde_json::from_value(
        json!({"provider":"muse","protocol":MUSE_MSP_REPLAY_PROTOCOL,"version":"1","scenario":"ids","metadata":{"commandIds":[["command-one","command-two"]]},"entries":[
            {"type":"expect_outbound","frame":{"type":"host_start","args":[]}},
            {"type":"expect_outbound","frame":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}},
            {"type":"emit_inbound","frame":{"jsonrpc":"2.0","id":1,"result":{}}},
            {"type":"emit_inbound","frame":{"jsonrpc":"2.0","id":1,"method":"approve","params":{}}},
            {"type":"expect_outbound","frame":{"jsonrpc":"2.0","id":1,"result":"allow"}},
            {"type":"runtime_exit","status":"success"}
        ]}),
    )?;
    let controller = make_replay_controller(transcript)?;
    let mut host = controller.spawn(&[])?;
    assert_eq!(controller.mint(1)?, "command-one");
    assert_eq!(controller.mint(1)?, "command-two");
    controller.receive(
        1,
        json!({"jsonrpc":"2.0","id":91,"method":"initialize","params":{}}),
    )?;
    assert_eq!(host.incoming.recv().await.unwrap()["id"], 91);
    let request = host.incoming.recv().await.unwrap();
    assert_eq!(request["id"], 1);
    assert_eq!(request["method"], "approve");
    controller.receive(1, json!({"jsonrpc":"2.0","id":1,"result":"allow"}))?;
    assert!(host.incoming.recv().await.is_none());
    assert!(host.exited.is_cancelled());
    controller.assert_complete()?;
    Ok(())
}

#[test]
fn muse_workspace_rewriting_only_replaces_whole_outbound_placeholders() {
    let transcript:ReplayTranscript=serde_json::from_value(json!({"provider":"muse","protocol":MUSE_MSP_REPLAY_PROTOCOL,"version":"1","scenario":"workspace","metadata":{"commandIds":[]},"entries":[
        {"type":"expect_outbound","frame":{"method":"session/start","params":{"root":"<workspace>","text":"keep <workspace> embedded"}}},
        {"type":"expect_outbound","frame":{"method":"turn/start","params":{"root":"<workspace>"}}},
        {"type":"emit_inbound","frame":{"root":"<workspace>"}}
    ]})).unwrap();
    let materialized = materialize_muse_replay_workspace(transcript, "C:\\Work", "\\\\?\\C:\\Work");
    let frames: Vec<Value> = materialized
        .entries
        .iter()
        .map(|entry| match entry {
            ReplayEntry::ExpectOutbound(entry) => entry.frame.clone(),
            ReplayEntry::EmitInbound(entry) => entry.frame.clone(),
            _ => Value::Null,
        })
        .collect();
    assert_eq!(frames[0]["params"]["root"], "C:\\Work");
    assert_eq!(frames[0]["params"]["text"], "keep <workspace> embedded");
    assert_eq!(frames[1]["params"]["root"], "\\\\?\\C:\\Work");
    assert_eq!(frames[2]["root"], "<workspace>");
    assert!(make_replay_controller(serde_json::from_value(json!({"provider":"muse","protocol":MUSE_MSP_REPLAY_PROTOCOL,"version":"1","scenario":"bad","entries":[]})).unwrap()).is_err());
}
