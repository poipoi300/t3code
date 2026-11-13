#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use t3_rpc::{
    CallEvent, CallKind, Cause, Client, ClientError, ClientMessage, Exit, Frame, Request, Server,
    ServerAction, ServerError, ServerMessage, codec, framing,
};

#[test]
fn acknowledgements_gate_production_and_completion_at_the_product_windows() {
    for (method, limit) in [
        ("numbers", 1),
        ("subscribeServerConfig", 8),
        ("orchestration.subscribeShell", 8),
        ("orchestration.subscribeThread", 8),
        ("terminal.attach", 8),
        ("terminal.observe", 8),
    ] {
        let mut server = Server::<Value, Value, Value>::new();
        server.register(method, CallKind::Stream);
        let id = t3_rpc::RequestId::from(0_u64);
        server.receive(ClientMessage::Request(Request::new(
            id.clone(),
            method,
            Value::Null,
        )));
        server.started(&id).unwrap();
        // ACKs never accumulate credit before a chunk exists.
        for _ in 0..12 {
            server.receive(ClientMessage::Ack {
                request_id: id.clone(),
            });
        }
        for _ in 0..limit {
            assert!(server.can_poll(&id));
            server
                .send_chunk(&id, vec![json!({"type":"output","data":"x"})])
                .unwrap();
        }
        assert!(!server.can_poll(&id), "{method}");
        assert_eq!(
            server.send_chunk(&id, vec![json!(2)]),
            Err(ServerError::Backpressure)
        );
        assert_eq!(
            server.finish(&id, Exit::void()),
            Err(ServerError::Backpressure)
        );
        server.receive(ClientMessage::Ack {
            request_id: id.clone(),
        });
        assert!(server.can_poll(&id));
        server.finish(&id, Exit::void()).unwrap();
    }
    let mut server = Server::<Value, Value, Value>::new();
    server.register("terminal.attach", CallKind::Stream);
    let id = t3_rpc::RequestId::from(0_u64);
    server.receive(ClientMessage::Request(Request::new(
        id.clone(),
        "terminal.attach",
        Value::Null,
    )));
    server
        .send_chunk(&id, vec![json!({"type":"output","data":"é".repeat(32768)})])
        .unwrap();
    assert!(!server.can_poll(&id));
    server.receive(ClientMessage::Interrupt { request_id: id });
    assert_eq!(server.active_count(), 0);
}

#[test]
fn ping_timeout_uses_all_frames_and_a_strict_threshold_without_catching_up_ticks() {
    let mut client = Client::<Value, Value, Value>::default();
    let a = client.start("wait", Value::Null, CallKind::Unary).unwrap();
    let b = client
        .start("numbers", Value::Null, CallKind::Stream)
        .unwrap();
    assert_eq!(a, 0_u64.into());
    assert_eq!(b, 1_u64.into());
    client.pop_outgoing();
    client.pop_outgoing();
    client.tick(4999);
    assert!(client.pop_outgoing().is_none());
    client.tick(5000);
    assert_eq!(client.pop_outgoing(), Some(ClientMessage::Ping));
    client.tick(15000);
    assert_eq!(client.pop_outgoing(), Some(ClientMessage::Ping));
    client.receive(
        ServerMessage::Chunk {
            request_id: b.clone(),
            values: vec![json!(1)],
        },
        19999,
    );
    client.tick(20000);
    assert_eq!(client.pop_outgoing(), Some(ClientMessage::Ping));
    client.tick(34999);
    assert_eq!(client.pop_outgoing(), Some(ClientMessage::Ping));
    client.tick(39999);
    assert!(client.is_closed());
    assert_eq!(client.pending_count(), 0);
    assert!(matches!(client.pop_event(), Some(CallEvent::Chunk { .. })));
    for id in [a, b] {
        assert_eq!(
            client.pop_event(),
            Some(CallEvent::Failed {
                id,
                error: ClientError::PingTimeout
            })
        );
    }
    assert!(client.pop_outgoing().is_none());

    let mut late = Client::<Value, Value, Value>::default();
    late.tick(60000);
    assert!(!late.is_closed());
    assert_eq!(late.pop_outgoing(), Some(ClientMessage::Ping));
    late.receive(ServerMessage::Pong, 60001);
    late.tick(65000);
    assert!(!late.is_closed());
}

#[test]
fn per_call_defects_interrupts_notifications_discard_and_eof_keep_their_scope() {
    let mut client = Client::<Value, Value, Value>::default();
    let a = client.start("a", Value::Null, CallKind::Unary).unwrap();
    let b = client.start("b", Value::Null, CallKind::Unary).unwrap();
    assert_eq!(
        client.start_request(
            Request::new(a.clone(), "duplicate", Value::Null),
            CallKind::Unary
        ),
        Err(ClientError::DuplicateId)
    );
    client.receive(
        ServerMessage::Exit {
            request_id: a.clone(),
            exit: Exit::die("one"),
        },
        0,
    );
    assert_eq!(client.pending_count(), 1);
    client.receive(
        ServerMessage::Defect {
            defect: json!("all"),
        },
        0,
    );
    assert_eq!(client.pending_count(), 0);
    assert!(matches!(client.pop_event(), Some(CallEvent::Exit { id, .. }) if id == a));
    assert_eq!(
        client.pop_event(),
        Some(CallEvent::Failed {
            id: b,
            error: ClientError::Defect(json!("all"))
        })
    );
    assert!(client.start("after", Value::Null, CallKind::Unary).is_ok());

    let mut server = Server::<Value, Value, Value>::new();
    server.register("echo", CallKind::Unary);
    server.receive(ClientMessage::Request(Request::new(
        "missing".into(),
        "unknown",
        Value::Null,
    )));
    assert_eq!(
        server.pop_outgoing(),
        Some(ServerMessage::Exit {
            request_id: "missing".into(),
            exit: Exit::die("Unknown request tag: unknown")
        })
    );
    let mut notification = Request::new("".into(), "echo", Value::Null);
    notification.is_notification = Some(t3_rpc::True);
    server.receive(ClientMessage::Request(notification.clone()));
    server.receive(ClientMessage::Request(notification));
    let mut ids = Vec::new();
    while let Some(ServerAction::Dispatch(request)) = server.pop_action() {
        ids.push(request.id);
    }
    assert_eq!(
        ids,
        vec![
            "@effect/rpc/notification/0".into(),
            "@effect/rpc/notification/1".into()
        ]
    );
    for id in ids {
        server.finish(&id, Exit::void()).unwrap();
    }
    assert!(server.pop_outgoing().is_none());

    let mut discard = Request::new("discard".into(), "echo", Value::Null);
    discard.discard = Some(t3_rpc::True);
    server.receive(ClientMessage::Request(discard));
    assert!(server.pop_outgoing().is_none());
    server.started(&"discard".into()).unwrap();
    assert_eq!(
        server.pop_outgoing(),
        Some(ServerMessage::Exit {
            request_id: "discard".into(),
            exit: Exit::void()
        })
    );
    server.receive(ClientMessage::Eof);
    server.receive(ClientMessage::Request(Request::new(
        "late".into(),
        "echo",
        Value::Null,
    )));
    assert_eq!(server.active_count(), 1);
    server
        .finish(&"discard".into(), Exit::die("silent failure"))
        .unwrap();
    assert!(server.pop_outgoing().is_none());
    assert!(matches!(
        server.pop_action(),
        Some(ServerAction::Dispatch(_))
    ));
    assert!(matches!(server.pop_action(), Some(ServerAction::Ended)));
}

#[test]
fn codec_retains_optional_null_fiber_ids_unknown_tags_and_batched_binary_frames() {
    for cause in [
        json!({"_tag":"Interrupt"}),
        json!({"_tag":"Interrupt","fiberId":null}),
        json!({"_tag":"Interrupt","fiberId":12}),
    ] {
        let decoded: Cause = serde_json::from_value(cause.clone()).unwrap();
        let mut expected = cause;
        expected["fiberId"] = expected.get("fiberId").cloned().unwrap_or(Value::Null);
        assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
    }
    let raw = json!({"_tag":"Future","field":[1,null]});
    let message: ClientMessage = serde_json::from_value(raw.clone()).unwrap();
    assert!(matches!(message, ClientMessage::Unknown(_)));
    assert_eq!(serde_json::to_value(message).unwrap(), raw);
    let messages: Vec<ClientMessage> = codec::decode(&Frame::Binary(
        br#"[{"_tag":"Ping"},{"_tag":"Ack","requestId":"str"}]"#.to_vec(),
    ))
    .unwrap();
    assert_eq!(
        messages,
        vec![
            ClientMessage::Ping,
            ClientMessage::Ack {
                request_id: "str".into()
            }
        ]
    );
    assert!(codec::decode::<ClientMessage>(&Frame::Text("{".into())).is_err());
    assert!(codec::decode::<ClientMessage>(&Frame::Binary(vec![255])).is_err());
    let mut binary = br#"{"_tag":"Request","id":0,"tag":"echo","payload":""#.to_vec();
    binary.push(255);
    binary.extend_from_slice(br#"","headers":[]}"#);
    let messages = codec::decode::<ClientMessage>(&Frame::Binary(binary)).unwrap();
    assert!(
        matches!(&messages[0], ClientMessage::Request(request) if request.payload == json!("�"))
    );
}

#[test]
fn frame_parts_preserve_unicode_boundaries_binary_passthrough_and_liveness() {
    for text in [
        "x".repeat(49150),
        "x".repeat(49151),
        "x".repeat(49152),
        "界".repeat(16384),
        "🌍éa".repeat(30000),
    ] {
        let frame = Frame::Text(text.clone());
        let parts = framing::split(frame.clone());
        let mut joiner = framing::Joiner::default();
        for (index, part) in parts.iter().enumerate() {
            let Frame::Text(part_text) = part else {
                panic!("text expected")
            };
            assert!(part_text.len() <= framing::FRAME_PART_BYTES);
            let joined = joiner.feed(part.clone());
            if index + 1 == parts.len() {
                assert_eq!(joined, frame);
            } else {
                assert_eq!(joined, Frame::Text("{\"_tag\":\"Pong\"}".into()));
            }
        }
    }
    let mut joiner = framing::Joiner::default();
    joiner.feed(Frame::Text("\u{1e}unfinished".into()));
    assert_eq!(
        joiner.feed(Frame::Binary(vec![1, 2])),
        Frame::Binary(vec![1, 2])
    );
    joiner.reset();
    assert_eq!(
        joiner.feed(Frame::Text("new connection".into())),
        Frame::Text("new connection".into())
    );
}

#[test]
fn terminal_byte_windows_charge_every_variable_field_and_both_envelopes_at_the_boundary() {
    for (event, variable_bytes) in [
        (
            json!({"type":"output","threadId":"é","terminalId":"界","data":"🌍"}),
            9,
        ),
        (
            json!({"type":"error","threadId":"é","terminalId":"界","message":"🌍"}),
            9,
        ),
        (
            json!({"type":"activity","threadId":"é","terminalId":"界","label":"🌍"}),
            9,
        ),
        (
            json!({"type":"snapshot","snapshot":{"threadId":"é","terminalId":"界","history":"🌍","cwd":"a","worktreePath":"b","label":"c"}}),
            12,
        ),
        (json!({"type":"exit","threadId":"é","terminalId":"界"}), 5),
    ] {
        for delta in [0, 1] {
            let mut server = Server::<Value, Value, Value>::new();
            server.register("terminal.attach", CallKind::Stream);
            let id = 0_u64.into();
            server.receive(ClientMessage::Request(Request::new(
                id,
                "terminal.attach",
                Value::Null,
            )));
            // One chunk envelope and two event envelopes, plus the row's UTF-8 fields.
            let padding = 65536 - 3 * 128 - variable_bytes - delta;
            server
                .send_chunk(
                    &0_u64.into(),
                    vec![
                        event.clone(),
                        json!({"type":"output","data":"x".repeat(padding)}),
                    ],
                )
                .unwrap();
            assert_eq!(
                server.can_poll(&0_u64.into()),
                delta == 1,
                "{event}, delta={delta}"
            );
        }
    }
}

#[test]
fn frame_parts_match_the_product_splitter_corpus() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/rpc-transcripts/framing-cases.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let frame = Frame::Text(case["frame"].as_str().unwrap().into());
        let parts: Vec<Frame> = case["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| Frame::Text(part.as_str().unwrap().into()))
            .collect();
        assert_eq!(framing::split(frame.clone()), parts);
        let mut joiner = framing::Joiner::default();
        let mut joined = None;
        for part in parts {
            joined = Some(joiner.feed(part));
        }
        assert_eq!(joined, Some(frame));
    }
}
