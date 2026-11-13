#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use t3_rpc::{CallKind, Client, ClientMessage, Exit, Request, Server, ServerMessage, codec};

fn verify(frames: Value) -> t3_process::ProcessOutput {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut options = t3_process::ProcessOptions::new("node")
        .args(["rust/tools/export/rpcTranscripts.ts", "--verify"]);
    options.cwd = Some(root);
    options.stdin = Some(serde_json::to_vec(&frames).unwrap());
    options.check_exit = false;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(t3_process::run(options))
        .unwrap()
}

#[test]
fn real_effect_decodes_every_frame_kind_generated_by_rust_and_rejects_invalid_controls() {
    let mut frames = Vec::new();
    let mut client = Client::<Value, Value, Value>::default();
    let id = client
        .start("stream", Value::Null, CallKind::Stream)
        .unwrap();
    client.acknowledge(id.clone());
    client.interrupt(&id);
    client.tick(5000);
    client.eof();
    while let Some(message) = client.pop_outgoing() {
        let t3_rpc::Frame::Text(data) = codec::encode(&message).unwrap() else {
            unreachable!()
        };
        frames.push(
            json!({"direction":"client","message":serde_json::from_str::<Value>(&data).unwrap()}),
        );
    }
    let mut server = Server::<Value, Value, Value>::new();
    server.register("stream", CallKind::Stream);
    server.register("unary", CallKind::Unary);
    server.receive(ClientMessage::Request(Request::new(
        id.clone(),
        "stream",
        Value::Null,
    )));
    server.send_chunk(&id, vec![json!(1)]).unwrap();
    server.receive(ClientMessage::Interrupt { request_id: id });
    for (index, exit) in [
        Exit::success(json!(42)),
        Exit::void(),
        Exit::fail(json!({"code":"NO"})),
        Exit::die("defect"),
        Exit::interrupt(Some(7)),
    ]
    .into_iter()
    .enumerate()
    {
        let id = t3_rpc::RequestId::from((index + 1) as u64);
        server.receive(ClientMessage::Request(Request::new(
            id.clone(),
            "unary",
            Value::Null,
        )));
        server.finish(&id, exit).unwrap();
    }
    server.receive(ClientMessage::Ping);
    server.receive(ClientMessage::Unknown(json!({"_tag":"Future"})));
    while let Some(message) = server.pop_outgoing() {
        let t3_rpc::Frame::Text(data) = codec::encode(&message).unwrap() else {
            unreachable!()
        };
        frames.push(
            json!({"direction":"server","message":serde_json::from_str::<Value>(&data).unwrap()}),
        );
    }
    frames.push(json!({"direction":"server","message":serde_json::to_value(ServerMessage::<Value>::Request(Request::new("push".into(), "push", Value::Null))).unwrap()}));
    let result = verify(json!(frames));
    assert_eq!(result.code, Some(0), "{}", result.stderr_text());
    println!("{}", result.stdout_text());
    for invalid in [
        json!({"_tag":"Exit","requestId":0,"exit":{"_tag":"Failure","cause":[{"_tag":"Interrupt"}]}}),
        json!({"_tag":"Chunk","requestId":0,"values":[]}),
    ] {
        assert_ne!(
            verify(json!([{"direction":"server","message":invalid}])).code,
            Some(0)
        );
    }
}
