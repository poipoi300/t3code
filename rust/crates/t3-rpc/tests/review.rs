#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use t3_rpc::{
    CallEvent, CallKind, Client, ClientError, ClientMessage, Exit, Frame, Request, RequestId,
    Server, ServerMessage, codec,
};

#[test]
fn interrupts_always_encode_the_required_fiber_id() {
    let exit = Exit::<Value, Value>::interrupt(None);
    assert_eq!(
        serde_json::to_value(exit).unwrap(),
        json!({"_tag":"Failure","cause":[{"_tag":"Interrupt","fiberId":null}]})
    );
}

#[test]
fn undecodable_responses_complete_only_the_affected_call() {
    for response in [
        json!({"_tag":"Exit","requestId":0,"exit":{"_tag":"Success","value":"wrong"}}),
        json!({"_tag":"Exit","requestId":0,"exit":{"_tag":"Future"}}),
        json!({"_tag":"Exit","requestId":0,"exit":{"_tag":"Failure","cause":[{"_tag":"Future"}]}}),
        json!({"_tag":"Exit","requestId":0,"exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":"wrong"}]}}),
        json!({"_tag":"Chunk","requestId":0,"values":[]}),
        json!({"_tag":"Chunk","requestId":0,"values":"wrong"}),
        json!({"_tag":"Chunk","requestId":0,"values":["wrong"]}),
    ] {
        let mut client = Client::<Value, u64, u64>::default();
        let id = client
            .start("numbers", Value::Null, CallKind::Stream)
            .unwrap();
        client.start("other", Value::Null, CallKind::Unary).unwrap();
        client.pop_outgoing();
        client.pop_outgoing();
        let messages = codec::decode(&Frame::Text(response.to_string())).unwrap();
        for message in messages {
            client.receive(message, 0);
        }
        assert!(
            matches!(client.pop_event(), Some(CallEvent::Failed { id: actual, error: ClientError::Protocol(_) }) if actual == id),
            "{response}"
        );
        assert_eq!(client.pending_count(), 1);
        assert!(
            matches!(client.pop_outgoing(), Some(ClientMessage::Interrupt { request_id }) if request_id == id)
        );
    }
}

#[test]
fn eof_keeps_ping_and_defects_alive_and_resets_after_drain() {
    let mut server = Server::<Value, Value, Value>::new();
    server.register("wait", CallKind::Unary);
    let id = 0_u64.into();
    server.receive(ClientMessage::Request(Request::new(
        id,
        "wait",
        Value::Null,
    )));
    server.receive(ClientMessage::Eof);
    server.receive(ClientMessage::Ping);
    assert_eq!(server.pop_outgoing(), Some(ServerMessage::Pong));
    server.receive(ClientMessage::Unknown(json!({"_tag":"Future"})));
    assert!(matches!(
        server.pop_outgoing(),
        Some(ServerMessage::Defect { .. })
    ));
    server.finish(&0_u64.into(), Exit::void()).unwrap();
    server.pop_outgoing();
    server.receive(ClientMessage::Request(Request::new(
        1_u64.into(),
        "wait",
        Value::Null,
    )));
    assert_eq!(server.active_count(), 1);
}

#[test]
fn interrupts_for_inactive_ids_are_silent() {
    let mut server = Server::<Value, Value, Value>::new();
    server.register("echo", CallKind::Unary);
    for completed in [false, true] {
        if completed {
            server.receive(ClientMessage::Request(Request::new(
                0_u64.into(),
                "echo",
                Value::Null,
            )));
            server.finish(&0_u64.into(), Exit::void()).unwrap();
            server.pop_outgoing();
        }
        server.receive(ClientMessage::Interrupt {
            request_id: 0_u64.into(),
        });
        assert!(server.pop_outgoing().is_none());
    }
}

#[test]
fn malformed_requests_with_valid_ids_have_request_scoped_failures() {
    for request in [
        json!({"_tag":"Request","id":0,"tag":"echo","headers":[]}),
        json!({"_tag":"Request","id":0,"payload":null,"headers":[]}),
        json!({"_tag":"Request","id":0,"tag":"echo","payload":null,"headers":42}),
    ] {
        let mut server = Server::<Value, Value, Value>::new();
        server.register("echo", CallKind::Unary);
        for message in codec::decode(&Frame::Text(request.to_string())).unwrap() {
            server.receive(message);
        }
        assert!(
            matches!(server.pop_outgoing(), Some(ServerMessage::Exit { request_id, exit: Exit::Failure { .. } }) if request_id == 0_u64.into()),
            "{request}"
        );
    }
}

#[test]
fn chunks_queued_before_interruption_are_not_acknowledged_afterwards() {
    let mut client = Client::<Value, Value, Value>::default();
    let id = client
        .start("stream", Value::Null, CallKind::Stream)
        .unwrap();
    client.pop_outgoing();
    client.receive(
        ServerMessage::Chunk {
            request_id: id.clone(),
            values: vec![json!(1)],
        },
        0,
    );
    client.interrupt(&id);
    client.pop_outgoing();
    client.acknowledge(id);
    assert!(client.pop_outgoing().is_none());
}

#[test]
fn numeric_ids_have_javascript_map_identity_and_hashing() {
    use std::collections::HashSet;
    let integer: RequestId = serde_json::from_str("1").unwrap();
    let float: RequestId = serde_json::from_str("1.0").unwrap();
    assert_eq!(integer, float);
    assert!(HashSet::from([integer]).contains(&float));
    assert_eq!(
        serde_json::from_str::<RequestId>("-0.0").unwrap(),
        0_u64.into()
    );
    assert_ne!(float, "1".into());
}

#[test]
fn upgrade_headers_are_prepended_and_retained_when_a_request_has_its_own_headers() {
    let mut server = Server::<Value, Value, Value>::new();
    server.register("echo", CallKind::Unary);
    server.set_upgrade_headers(vec![("x-upgrade".into(), "first".into())]);
    let mut request = Request::new(0_u64.into(), "echo", Value::Null);
    request.headers.push(("x-upgrade".into(), "last".into()));
    server.receive(ClientMessage::Request(request));
    let Some(t3_rpc::ServerAction::Dispatch(request)) = server.pop_action() else {
        panic!("not dispatched")
    };
    assert_eq!(
        request.headers,
        vec![
            ("x-upgrade".into(), "first".into()),
            ("x-upgrade".into(), "last".into())
        ]
    );
}

#[test]
fn exact_three_byte_frame_boundary_matches_typescript_fast_path() {
    assert_eq!(
        t3_rpc::framing::split(Frame::Text("界".repeat(16384))).len(),
        1
    );
}
