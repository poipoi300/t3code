#![allow(clippy::unwrap_used)]

use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};
use t3_rpc::{
    CallEvent, CallKind, Client, ClientError, ClientMessage, Exit, Frame, Request, RequestId,
    Server, ServerAction, ServerMessage, codec, framing,
};

const TRANSCRIPTS: &[&str] = &[
    include_str!("../../../fixtures/rpc-transcripts/unary-success.json"),
    include_str!("../../../fixtures/rpc-transcripts/typed-failure.json"),
    include_str!("../../../fixtures/rpc-transcripts/defect.json"),
    include_str!("../../../fixtures/rpc-transcripts/stream.json"),
    include_str!("../../../fixtures/rpc-transcripts/stream-failure.json"),
    include_str!("../../../fixtures/rpc-transcripts/interrupt-unary.json"),
    include_str!("../../../fixtures/rpc-transcripts/interrupt-stream.json"),
    include_str!("../../../fixtures/rpc-transcripts/concurrent.json"),
    include_str!("../../../fixtures/rpc-transcripts/keepalive.json"),
    include_str!("../../../fixtures/rpc-transcripts/window.json"),
    include_str!("../../../fixtures/rpc-transcripts/terminal-window.json"),
    include_str!("../../../fixtures/rpc-transcripts/disconnect.json"),
    include_str!("../../../fixtures/rpc-transcripts/discard.json"),
    include_str!("../../../fixtures/rpc-transcripts/eof.json"),
];

fn kind(tag: &str) -> CallKind {
    if matches!(
        tag,
        "numbers" | "broken" | "endless" | "subscribeServerConfig" | "terminal.attach"
    ) {
        CallKind::Stream
    } else {
        CallKind::Unary
    }
}

enum Response {
    Chunk(Vec<Value>),
    Exit(Exit),
}

fn script(request: &Request) -> VecDeque<Response> {
    match request.tag.as_str() {
        "serverInterrupt" => [Response::Exit(Exit::interrupt(None))].into(),
        "multipleCauses" => [Response::Exit(Exit::Failure { cause: vec![t3_rpc::Cause::Fail { error: json!({"code":"ONE"}) }, t3_rpc::Cause::Die { defect: json!("two") }] })].into(),
        "echo" => [Response::Exit(Exit::success(request.payload["value"].clone()))].into(),
        "fail" => [Response::Exit(Exit::fail(json!({"code":"NO"})))].into(),
        "die" => [Response::Exit(Exit::die("fixture defect"))].into(),
        "numbers" | "subscribeServerConfig" => {
            let last = if request.tag == "numbers" { 3 } else { 10 };
            (1..=last).map(|value| Response::Chunk(vec![json!(value)]))
                .chain([Response::Exit(Exit::void())]).collect()
        }
        "broken" => [Response::Chunk(vec![json!(1)]), Response::Exit(Exit::fail(json!({"code":"MID"})))].into(),
        "endless" => [Response::Chunk(vec![json!(1)])].into(),
        "wait" => VecDeque::new(),
        "terminal.attach" => [
            Response::Chunk(vec![json!({"type":"output","threadId":"thread","terminalId":"terminal","data":"é".repeat(32768)})]),
            Response::Chunk(vec![json!({"type":"activity","label":"complete"})]),
            Response::Exit(Exit::void()),
        ].into(),
        tag => panic!("unregistered fixture method {tag}"),
    }
}

#[test]
fn effect_frames_round_trip_byte_for_byte_and_drive_both_state_machines() {
    for transcript in TRANSCRIPTS {
        let fixture: Value = serde_json::from_str(transcript).unwrap();
        let name = fixture["name"].as_str().unwrap();
        let entries = fixture["entries"].as_array().unwrap();
        let mut client = Client::<Value, Value, Value>::default();
        let mut server = Server::<Value, Value, Value>::new();
        server.set_upgrade_headers(
            serde_json::from_value(fixture["upgradeHeaders"].clone()).unwrap(),
        );
        let mut received_headers = Vec::new();
        for tag in [
            "echo",
            "fail",
            "die",
            "wait",
            "numbers",
            "broken",
            "endless",
            "subscribeServerConfig",
            "terminal.attach",
        ] {
            server.register(tag, kind(tag));
        }
        let mut scripts = HashMap::<RequestId, VecDeque<Response>>::new();
        let mut waits = HashMap::<String, RequestId>::new();
        let mut now = 0;
        let mut interrupted = Vec::new();
        let mut disconnection_events = 0;
        let mut joiner = framing::Joiner::default();
        let mut parts = Vec::new();
        for entry in entries {
            if let Some(action) = entry["action"].as_str() {
                match action {
                    "interrupt" => {
                        let id = serde_json::from_value::<RequestId>(entry["id"].clone()).unwrap();
                        client.interrupt(&id);
                        interrupted.push(id.clone());
                        assert_eq!(
                            client.pop_event(),
                            Some(CallEvent::Failed {
                                id,
                                error: ClientError::Interrupted
                            })
                        );
                    }
                    "release" => {
                        scripts
                            .get_mut(&waits[entry["key"].as_str().unwrap()])
                            .unwrap()
                            .push_back(Response::Exit(Exit::success(entry["value"].clone())));
                    }
                    "tick" => {
                        now = entry["now"].as_u64().unwrap();
                        client.tick(now);
                    }
                    "eof" => client.eof(),
                    "release-acks" => {
                        for id in scripts.keys() {
                            assert!(
                                !server.can_poll(id),
                                "{name} must stop polling before ACK release"
                            );
                        }
                    }
                    "disconnect" => {
                        disconnection_events = client.pending_count();
                        client.disconnect(ClientError::Disconnected);
                        server.disconnect();
                        while let Some(action) = server.pop_action() {
                            assert!(matches!(
                                action,
                                ServerAction::Cancel(_) | ServerAction::Ended
                            ));
                        }
                    }
                    action => panic!("unknown action {action}"),
                }
                continue;
            }
            let frame = Frame::Text(entry["data"].as_str().unwrap().into());
            if entry["direction"] == "client" {
                let mut messages: Vec<ClientMessage> = codec::decode(&frame).unwrap();
                assert_eq!(messages.len(), 1);
                let message = messages.remove(0);
                assert_eq!(codec::encode(&message).unwrap(), frame, "{name}");
                if let ClientMessage::Request(request) = &message {
                    client
                        .start_request(request.clone(), kind(&request.tag))
                        .unwrap();
                }
                assert_eq!(
                    codec::encode(&client.pop_outgoing().unwrap()).unwrap(),
                    frame,
                    "{name}"
                );
                server.receive(message);
                while let Some(action) = server.pop_action() {
                    match action {
                        ServerAction::Dispatch(request) => {
                            received_headers.push(serde_json::to_value(&request.headers).unwrap());
                            if request.tag == "wait" {
                                waits.insert(
                                    request.payload["key"].as_str().unwrap().into(),
                                    request.id.clone(),
                                );
                            }
                            scripts.insert(request.id.clone(), script(&request));
                            server.started(&request.id).unwrap();
                        }
                        ServerAction::Cancel(id) => {
                            scripts.remove(&id);
                        }
                        ServerAction::Ended => {}
                    }
                }
            } else {
                parts.push(frame.clone());
                let partial = matches!(&frame, Frame::Text(text) if text.starts_with('\u{1e}'));
                let frame = joiner.feed(frame);
                if partial {
                    assert_eq!(frame, Frame::Text("{\"_tag\":\"Pong\"}".into()));
                    client.receive(ServerMessage::Pong, now);
                    continue;
                }
                let mut messages: Vec<ServerMessage> = codec::decode(&frame).unwrap();
                assert_eq!(messages.len(), 1);
                let message = messages.remove(0);
                assert_eq!(codec::encode(&message).unwrap(), frame, "{name}");
                let generated = server.pop_outgoing().unwrap_or_else(|| {
                    let id = match &message {
                        ServerMessage::Chunk { request_id, .. }
                        | ServerMessage::Exit { request_id, .. } => request_id,
                        _ => panic!("unsolicited frame: {message:?}"),
                    };
                    // This chooses which ready handler the executor polls, never its result.
                    match scripts.get_mut(id).unwrap().pop_front().unwrap() {
                        Response::Chunk(values) => {
                            assert!(server.can_poll(id), "{name}");
                            server.send_chunk(id, values).unwrap();
                        }
                        Response::Exit(exit) => server.finish(id, exit).unwrap(),
                    }
                    server.pop_outgoing().unwrap()
                });
                assert_eq!(framing::split(frame.clone()), parts, "{name} split frames");
                parts.clear();
                let mut expected: Value = serde_json::from_str(match &frame {
                    Frame::Text(text) => text,
                    _ => unreachable!(),
                })
                .unwrap();
                if let Some(causes) = expected
                    .get_mut("exit")
                    .and_then(|exit| exit.get_mut("cause"))
                    .and_then(Value::as_array_mut)
                {
                    for cause in causes {
                        if cause["_tag"] == "Interrupt" {
                            cause["fiberId"] = Value::Null;
                        }
                    }
                }
                assert_eq!(
                    serde_json::to_value(&generated).unwrap(),
                    expected,
                    "{name}"
                );
                let expected = match &message {
                    ServerMessage::Chunk { request_id, values } => Some(CallEvent::Chunk {
                        id: request_id.clone(),
                        values: values.clone(),
                    }),
                    ServerMessage::Exit { request_id, exit }
                        if !interrupted.contains(request_id) =>
                    {
                        Some(CallEvent::Exit {
                            id: request_id.clone(),
                            exit: exit.clone(),
                        })
                    }
                    _ => None,
                };
                let chunk_id = match &message {
                    ServerMessage::Chunk { request_id, .. } => Some(request_id.clone()),
                    _ => None,
                };
                client.receive(message, now);
                assert_eq!(client.pop_event(), expected, "{name}");
                if let Some(id) = chunk_id {
                    client.acknowledge(id);
                }
            }
        }
        for _ in 0..disconnection_events {
            assert!(matches!(
                client.pop_event(),
                Some(CallEvent::Failed {
                    error: ClientError::Disconnected,
                    ..
                })
            ));
        }
        assert!(client.pop_event().is_none(), "{name}");
        assert!(client.pop_outgoing().is_none(), "{name}");
        assert_eq!(client.pending_count(), 0, "{name}");
        assert!(server.pop_outgoing().is_none(), "{name}");
        if name == "discard" {
            server
                .finish(&0_u64.into(), Exit::success(json!("ignored")))
                .unwrap();
            assert!(server.pop_outgoing().is_none());
        }
        assert_eq!(server.active_count(), 0, "{name}");
        assert_eq!(
            json!(received_headers),
            fixture["receivedHeaders"],
            "{name} upgrade context"
        );
    }
}

#[test]
fn recorded_effect_ping_timeout_fails_the_rust_call_at_the_same_tick() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/rpc-transcripts/ping-timeout.json"
    ))
    .unwrap();
    let mut client = Client::<Value, Value, Value>::default();
    let mut now = 0;
    let mut dropping = false;
    for entry in fixture["entries"].as_array().unwrap() {
        match entry["action"].as_str() {
            Some("tick") => {
                now = entry["now"].as_u64().unwrap();
                client.tick(now);
            }
            Some("drop-server-frames") => dropping = true,
            Some("expect-ping-timeout") => {
                assert!(client.is_closed());
                assert_eq!(
                    client.pop_event(),
                    Some(CallEvent::Failed {
                        id: 0_u64.into(),
                        error: ClientError::PingTimeout
                    })
                );
            }
            Some("disconnect") => {}
            Some(action) => panic!("unknown {action}"),
            None => {
                let frame = Frame::Text(entry["data"].as_str().unwrap().into());
                if entry["direction"] == "client" {
                    let message = codec::decode::<ClientMessage>(&frame).unwrap().remove(0);
                    if let ClientMessage::Request(request) = message {
                        client.start_request(request, CallKind::Unary).unwrap();
                    }
                    assert_eq!(
                        codec::encode(&client.pop_outgoing().unwrap()).unwrap(),
                        frame
                    );
                } else if !dropping {
                    for message in codec::decode(&frame).unwrap() {
                        client.receive(message, now);
                    }
                }
            }
        }
    }
    assert_eq!(client.pending_count(), 0);
    assert!(client.pop_outgoing().is_none());
}

#[cfg(not(target_arch = "wasm32"))]
mod async_replay {
    use super::*;
    use futures::{
        FutureExt, StreamExt,
        channel::{mpsc, oneshot},
        executor::block_on,
        stream,
    };
    use std::{cell::RefCell, rc::Rc};
    use t3_rpc::{AsyncServer, Handler, HandlerOutput};

    type Gates = Rc<RefCell<HashMap<String, oneshot::Sender<Value>>>>;
    type Polls = Rc<RefCell<HashMap<RequestId, usize>>>;
    struct Guard(RequestId, Rc<RefCell<Vec<RequestId>>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.1.borrow_mut().push(self.0.clone());
        }
    }
    struct FixtureHandler {
        gates: Gates,
        polls: Polls,
        dropped: Rc<RefCell<Vec<RequestId>>>,
    }
    impl Handler for FixtureHandler {
        fn handle(&mut self, request: Request) -> HandlerOutput {
            let guard = Guard(request.id.clone(), self.dropped.clone());
            if request.tag == "wait" {
                let (tx, rx) = oneshot::channel();
                self.gates
                    .borrow_mut()
                    .insert(request.payload["key"].as_str().unwrap().into(), tx);
                return HandlerOutput::Unary(
                    async move {
                        let _guard = guard;
                        Exit::success(rx.await.unwrap())
                    }
                    .boxed_local(),
                );
            }
            let mut responses = script(&request);
            if kind(&request.tag) == CallKind::Unary {
                let Response::Exit(exit) = responses.pop_front().unwrap() else {
                    unreachable!()
                };
                return HandlerOutput::Unary(
                    async move {
                        let _guard = guard;
                        exit
                    }
                    .boxed_local(),
                );
            }
            let polls = self.polls.clone();
            let id = request.id;
            let responses = stream::iter(responses).map(move |response| {
                let _ = &guard;
                *polls.borrow_mut().entry(id.clone()).or_default() += 1;
                match response {
                    Response::Chunk(values) => Ok(values),
                    Response::Exit(exit) => Err(exit),
                }
            });
            if request.tag == "endless" {
                HandlerOutput::Stream(responses.chain(stream::pending()).boxed_local())
            } else {
                HandlerOutput::Stream(responses.boxed_local())
            }
        }
    }

    fn normalize(mut message: Value) -> Value {
        if message["_tag"] == "Defect" {
            return json!({"_tag":"Defect"});
        }
        if let Some(cause) = message
            .get_mut("exit")
            .and_then(|exit| exit.get_mut("cause"))
            .and_then(Value::as_array_mut)
        {
            for reason in cause {
                if reason["_tag"] == "Interrupt" {
                    reason["fiberId"] = Value::Null;
                }
            }
        }
        message
    }

    #[test]
    fn real_async_server_replays_all_effect_socket_transcripts_and_withholds_acks() {
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/rpc-transcripts");
        let mut paths: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        for path in paths {
            let fixture: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let Some(name) = fixture["name"].as_str() else {
                continue;
            };
            block_on(async {
                let (input, reader) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
                let consumed = Rc::new(std::cell::Cell::new(0));
                let count = consumed.clone();
                let reader = reader.inspect(move |_| count.set(count.get() + 1));
                let (writer, mut output) = mpsc::unbounded::<Frame>();
                let gates = Gates::default();
                let polls = Polls::default();
                let dropped = Rc::new(RefCell::new(Vec::new()));
                let mut server = AsyncServer::new(
                    writer,
                    reader,
                    FixtureHandler {
                        gates: gates.clone(),
                        polls: polls.clone(),
                        dropped: dropped.clone(),
                    },
                );
                server.set_frame_parts(true);
                server.state.set_upgrade_headers(
                    serde_json::from_value(fixture["upgradeHeaders"].clone()).unwrap(),
                );
                for method in [
                    "echo",
                    "fail",
                    "die",
                    "wait",
                    "numbers",
                    "broken",
                    "endless",
                    "subscribeServerConfig",
                    "terminal.attach",
                    "serverInterrupt",
                    "multipleCauses",
                ] {
                    server.state.register(method, kind(method));
                }
                let mut actual = Vec::new();
                let mut expected = Vec::new();
                let mut actual_joiner = framing::Joiner::default();
                let mut expected_joiner = framing::Joiner::default();
                let mut closed = false;
                let mut split_parts = 0;
                for entry in fixture["entries"].as_array().unwrap() {
                    if let Some(action) = entry["action"].as_str() {
                        match action {
                            "release" => {
                                gates
                                    .borrow_mut()
                                    .remove(entry["key"].as_str().unwrap())
                                    .unwrap()
                                    .send(entry["value"].clone())
                                    .unwrap();
                            }
                            "release-acks" => {
                                let limit = if name == "window" { 8 } else { 1 };
                                assert_eq!(polls.borrow().values().sum::<usize>(), limit, "{name}");
                                assert!(
                                    server.step().now_or_never().is_none(),
                                    "{name} should be waiting for ACK"
                                );
                            }
                            "interrupt" => {}
                            "disconnect" => {
                                closed = true;
                            }
                            "tick" | "eof" | "drop-server-frames" | "expect-ping-timeout" => {}
                            action => panic!("unknown {action}"),
                        }
                        continue;
                    }
                    let frame = Frame::Text(entry["data"].as_str().unwrap().into());
                    if entry["direction"] == "client" {
                        let before = consumed.get();
                        input.unbounded_send(Ok(frame)).unwrap();
                        while consumed.get() == before {
                            server
                                .step()
                                .now_or_never()
                                .unwrap_or_else(|| panic!("{name}: input not consumed"))
                                .unwrap();
                        }
                    } else {
                        let is_partial = entry["data"].as_str().unwrap().starts_with('\u{1e}');
                        let expected_frame = expected_joiner.feed(frame);
                        if !is_partial {
                            expected.push(normalize(
                                codec::decode::<Value>(&expected_frame).unwrap().remove(0),
                            ));
                        }
                        let frame = loop {
                            if let Some(frame) = output.next().now_or_never() {
                                break frame.unwrap();
                            }
                            server
                                .step()
                                .now_or_never()
                                .unwrap_or_else(|| panic!("{name}: no ready response"))
                                .unwrap();
                        };
                        if matches!(&frame, Frame::Text(text) if text.starts_with('\u{1e}')) {
                            split_parts += 1;
                        }
                        let is_partial =
                            matches!(&frame, Frame::Text(text) if text.starts_with('\u{1e}'));
                        let joined = actual_joiner.feed(frame);
                        if !is_partial {
                            actual.push(normalize(
                                codec::decode::<Value>(&joined).unwrap().remove(0),
                            ));
                        }
                    }
                }
                if closed {
                    drop(input);
                    server.run().await.unwrap();
                } else {
                    while let Some(result) = server.step().now_or_never() {
                        result.unwrap();
                    }
                }
                while let Some(Some(frame)) = output.next().now_or_never() {
                    let joined = actual_joiner.feed(frame);
                    actual.push(normalize(
                        codec::decode::<Value>(&joined).unwrap().remove(0),
                    ));
                }
                // Different ready handlers may interleave; each call's frame order is fixed.
                let group = |frames: Vec<Value>| {
                    let mut grouped = std::collections::BTreeMap::<String, Vec<Value>>::new();
                    for frame in frames {
                        grouped
                            .entry(
                                frame
                                    .get("requestId")
                                    .map_or_else(|| frame["_tag"].to_string(), Value::to_string),
                            )
                            .or_default()
                            .push(frame);
                    }
                    grouped
                };
                assert_eq!(group(actual), group(expected), "{name}");
                assert_eq!(server.state.active_count(), 0, "{name}");
                if name == "terminal-window" {
                    assert_eq!(split_parts, 1);
                }
                if name == "interrupt-stream" {
                    assert!(dropped.borrow().contains(&0_u64.into()));
                }
            });
        }
    }
}
