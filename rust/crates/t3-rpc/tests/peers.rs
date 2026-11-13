#![allow(clippy::unwrap_used)]

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use futures::{
    FutureExt, Sink, SinkExt, StreamExt,
    channel::{mpsc, oneshot},
    executor::block_on,
    stream,
};
use serde_json::{Value, json};
use t3_rpc::{
    AsyncClient, AsyncServer, CallEvent, CallKind, ClientMessage, Exit, Frame, Handler,
    HandlerOutput, Request, RequestId, ServerMessage, codec,
};

type Sender = mpsc::UnboundedSender<Frame>;
type Receiver =
    stream::Map<mpsc::UnboundedReceiver<Frame>, fn(Frame) -> Result<Frame, mpsc::SendError>>;
type Gates = Rc<RefCell<HashMap<String, oneshot::Sender<Value>>>>;

fn channel() -> (Sender, Receiver) {
    let (tx, rx) = mpsc::unbounded();
    (tx, rx.map(Ok))
}

struct Guard {
    id: RequestId,
    dropped: Rc<RefCell<Vec<RequestId>>>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.dropped.borrow_mut().push(self.id.clone());
    }
}

struct TestHandler {
    gates: Gates,
    started: mpsc::UnboundedSender<RequestId>,
    dropped: Rc<RefCell<Vec<RequestId>>>,
    polled: Rc<Cell<usize>>,
}

impl Handler for TestHandler {
    fn handle(&mut self, request: Request) -> HandlerOutput {
        self.started.unbounded_send(request.id.clone()).unwrap();
        match request.tag.as_str() {
            "echo" => HandlerOutput::Unary(
                futures::future::ready(Exit::success(request.payload["value"].clone()))
                    .boxed_local(),
            ),
            "fail" => HandlerOutput::Unary(
                futures::future::ready(Exit::fail(json!({"code":"NO"}))).boxed_local(),
            ),
            "die" => HandlerOutput::Unary(
                futures::future::ready(Exit::die("fixture defect")).boxed_local(),
            ),
            "wait" => {
                let (tx, rx) = oneshot::channel();
                self.gates
                    .borrow_mut()
                    .insert(request.payload["key"].as_str().unwrap().into(), tx);
                let guard = Guard {
                    id: request.id,
                    dropped: self.dropped.clone(),
                };
                HandlerOutput::Unary(
                    async move {
                        let _guard = guard;
                        match rx.await {
                            Ok(value) => Exit::success(value),
                            Err(_) => Exit::interrupt(None),
                        }
                    }
                    .boxed_local(),
                )
            }
            "endless" => {
                let guard = Guard {
                    id: request.id,
                    dropped: self.dropped.clone(),
                };
                HandlerOutput::Stream(
                    stream::once(futures::future::ready(Ok(vec![json!(1)])))
                        .chain(stream::once(async move {
                            let _guard = guard;
                            futures::future::pending().await
                        }))
                        .boxed_local(),
                )
            }
            tag => {
                let responses: Vec<Result<Vec<Value>, Exit>> = match tag {
                    "numbers" => (1..=3).map(|n| Ok(vec![json!(n)])).collect(),
                    "subscribeServerConfig" => (1..=10).map(|n| Ok(vec![json!(n)])).collect(),
                    "terminal.attach" => vec![
                        Ok(vec![
                            json!({"type":"output","threadId":"thread","terminalId":"terminal","data":"é".repeat(32768)}),
                        ]),
                        Ok(vec![json!({"type":"activity","label":"complete"})]),
                    ],
                    "broken" => vec![Ok(vec![json!(1)]), Err(Exit::fail(json!({"code":"MID"})))],
                    _ => panic!("unexpected tag {tag}"),
                };
                let polled = self.polled.clone();
                HandlerOutput::Stream(
                    stream::iter(responses)
                        .inspect(move |_| polled.set(polled.get() + 1))
                        .boxed_local(),
                )
            }
        }
    }
}

fn tapped(
    tx: Sender,
    log: Rc<RefCell<Vec<Frame>>>,
) -> impl Sink<Frame, Error = mpsc::SendError> + Unpin {
    tx.with(move |frame: Frame| {
        log.borrow_mut().push(frame.clone());
        futures::future::ready(Ok(frame))
    })
}

#[test]
fn rust_async_peers_cover_every_effect_scenario_over_in_memory_channels() {
    for scenario in [
        "unary-success",
        "typed-failure",
        "defect",
        "stream",
        "stream-failure",
        "interrupt-unary",
        "interrupt-stream",
        "concurrent",
        "keepalive",
        "window",
        "terminal-window",
        "disconnect",
        "discard",
        "eof",
    ] {
        block_on(async {
            let (client_tx, server_rx) = channel();
            let (server_tx, client_rx) = channel();
            let gates = Gates::default();
            let dropped = Rc::new(RefCell::new(Vec::new()));
            let (started, mut started_rx) = mpsc::unbounded();
            let outbound = Rc::new(RefCell::new(Vec::new()));
            let inbound = Rc::new(RefCell::new(Vec::new()));
            let handler = TestHandler {
                gates: gates.clone(),
                started,
                dropped: dropped.clone(),
                polled: Rc::new(Cell::new(0)),
            };
            let mut server =
                AsyncServer::new(tapped(server_tx, inbound.clone()), server_rx, handler);
            server.set_frame_parts(true);
            for method in ["echo", "fail", "die", "wait"] {
                server.state.register(method, CallKind::Unary);
            }
            for method in [
                "numbers",
                "broken",
                "endless",
                "subscribeServerConfig",
                "terminal.attach",
            ] {
                server.state.register(method, CallKind::Stream);
            }
            let mut client = AsyncClient::<_, _, Value, Value, Value>::new(
                tapped(client_tx, outbound.clone()),
                client_rx,
                0,
            );
            let task = async move {
                let mut ticks = stream::pending();
                match scenario {
                    "interrupt-stream" => {
                        let id = client
                            .state
                            .start("endless", Value::Null, CallKind::Stream)
                            .unwrap();
                        assert!(
                            matches!(client.next_event(&mut ticks, || 0).await.unwrap(), Some(CallEvent::Chunk { id: actual, .. }) if actual == id)
                        );
                        client.state.interrupt(&id);
                        client.flush().await.unwrap();
                    }
                    "concurrent" => {
                        let a = client
                            .state
                            .start("wait", json!({"key":"a"}), CallKind::Unary)
                            .unwrap();
                        let numbers = client
                            .state
                            .start("numbers", Value::Null, CallKind::Stream)
                            .unwrap();
                        let b = client
                            .state
                            .start("wait", json!({"key":"b"}), CallKind::Unary)
                            .unwrap();
                        client.flush().await.unwrap();
                        for _ in 0..3 {
                            started_rx.next().await.unwrap();
                        }
                        gates
                            .borrow_mut()
                            .remove("b")
                            .unwrap()
                            .send(json!("second"))
                            .unwrap();
                        let mut completed = Vec::new();
                        let mut values = Vec::new();
                        while !completed.contains(&b) {
                            match client.next_event(&mut ticks, || 0).await.unwrap().unwrap() {
                                CallEvent::Chunk { id, values: chunk } => {
                                    assert_eq!(id, numbers);
                                    values.extend(chunk);
                                }
                                CallEvent::Exit { id, .. } => {
                                    assert_ne!(id, a);
                                    completed.push(id);
                                }
                                event => panic!("unexpected {event:?}"),
                            }
                        }
                        gates
                            .borrow_mut()
                            .remove("a")
                            .unwrap()
                            .send(json!("first"))
                            .unwrap();
                        while completed.len() < 3 {
                            match client.next_event(&mut ticks, || 0).await.unwrap().unwrap() {
                                CallEvent::Chunk { id, values: chunk } => {
                                    assert_eq!(id, numbers);
                                    values.extend(chunk);
                                }
                                CallEvent::Exit { id, .. } => completed.push(id),
                                event => panic!("unexpected {event:?}"),
                            }
                        }
                        assert_eq!(values, vec![json!(1), json!(2), json!(3)]);
                    }
                    "interrupt-unary" | "disconnect" | "eof" => {
                        let id = client
                            .state
                            .start("wait", json!({"key":"a"}), CallKind::Unary)
                            .unwrap();
                        client.flush().await.unwrap();
                        started_rx.next().await.unwrap();
                        if scenario == "eof" {
                            client.state.eof();
                            client.flush().await.unwrap();
                            gates
                                .borrow_mut()
                                .remove("a")
                                .unwrap()
                                .send(json!("done"))
                                .unwrap();
                            assert_eq!(
                                client.next_event(&mut ticks, || 0).await.unwrap(),
                                Some(CallEvent::Exit {
                                    id,
                                    exit: Exit::success(json!("done"))
                                })
                            );
                        } else if scenario == "interrupt-unary" {
                            client.state.interrupt(&id);
                            client.flush().await.unwrap();
                            assert!(matches!(
                                client.next_event(&mut ticks, || 0).await.unwrap(),
                                Some(CallEvent::Failed { .. })
                            ));
                        } else {
                            let stream = client
                                .state
                                .start("endless", Value::Null, CallKind::Stream)
                                .unwrap();
                            assert!(matches!(
                                client.next_event(&mut ticks, || 0).await.unwrap(),
                                Some(CallEvent::Chunk { .. })
                            ));
                            assert_eq!(stream, 1_u64.into());
                            client.flush().await.unwrap();
                        }
                    }
                    _ => {
                        let (method, payload, kind, expected_exit, chunk_count) = match scenario {
                            "unary-success" => (
                                "echo",
                                json!({"value":"hello 🌍"}),
                                CallKind::Unary,
                                Exit::success(json!("hello 🌍")),
                                0,
                            ),
                            "typed-failure" => (
                                "fail",
                                Value::Null,
                                CallKind::Unary,
                                Exit::fail(json!({"code":"NO"})),
                                0,
                            ),
                            "defect" => (
                                "die",
                                Value::Null,
                                CallKind::Unary,
                                Exit::die("fixture defect"),
                                0,
                            ),
                            "stream" => ("numbers", Value::Null, CallKind::Stream, Exit::void(), 3),
                            "stream-failure" => (
                                "broken",
                                Value::Null,
                                CallKind::Stream,
                                Exit::fail(json!({"code":"MID"})),
                                1,
                            ),
                            "window" => (
                                "subscribeServerConfig",
                                Value::Null,
                                CallKind::Stream,
                                Exit::void(),
                                10,
                            ),
                            "terminal-window" => (
                                "terminal.attach",
                                Value::Null,
                                CallKind::Stream,
                                Exit::void(),
                                2,
                            ),
                            "discard" => (
                                "echo",
                                json!({"value":"ignored"}),
                                CallKind::Unary,
                                Exit::void(),
                                0,
                            ),
                            "keepalive" => {
                                client.state.tick(5000);
                                (
                                    "echo",
                                    json!({"value":"alive"}),
                                    CallKind::Unary,
                                    Exit::success(json!("alive")),
                                    0,
                                )
                            }
                            _ => panic!("unexpected scenario"),
                        };
                        let id = if scenario == "discard" || scenario == "unary-success" {
                            let mut request = Request::new(0_u64.into(), method, payload);
                            if scenario == "discard" {
                                request.discard = Some(t3_rpc::True);
                            } else {
                                request.headers.push(("x-fixture".into(), "yes".into()));
                            }
                            client.state.start_request(request, kind).unwrap()
                        } else {
                            client.state.start(method, payload, kind).unwrap()
                        };
                        let mut chunks = 0;
                        loop {
                            match client.next_event(&mut ticks, || 0).await.unwrap().unwrap() {
                                CallEvent::Chunk { id: actual, .. } => {
                                    assert_eq!(actual, id);
                                    chunks += 1;
                                }
                                CallEvent::Exit { id: actual, exit } => {
                                    assert_eq!(actual, id);
                                    assert_eq!(exit, expected_exit);
                                    break;
                                }
                                event => panic!("unexpected {event:?}"),
                            }
                        }
                        assert_eq!(chunks, chunk_count, "{scenario}");
                    }
                }
                client.close().await.unwrap();
                client
            };
            let (result, _client) = futures::join!(server.run(), task);
            assert!(result.is_ok(), "{scenario}: {result:?}");
            assert_eq!(server.state.active_count(), 0);
            if scenario != "keepalive" {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("../../fixtures/rpc-transcripts/{scenario}.json"));
                let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                for (direction, log) in [("client", &outbound), ("server", &inbound)] {
                    let expected: Vec<Frame> = fixture["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|entry| entry["direction"] == direction)
                        .map(|entry| Frame::Text(entry["data"].as_str().unwrap().into()))
                        .collect();
                    assert_eq!(
                        normalized_tap(&log.borrow()),
                        normalized_tap(&expected),
                        "{scenario} {direction}"
                    );
                }
            }
            if matches!(
                scenario,
                "interrupt-unary" | "interrupt-stream" | "disconnect"
            ) {
                let id = if scenario == "disconnect" { 1 } else { 0 };
                assert!(
                    dropped.borrow().contains(&RequestId::from(id as u64)),
                    "{scenario}"
                );
            }
            if scenario == "keepalive" {
                assert!(
                    outbound
                        .borrow()
                        .iter()
                        .any(|frame| codec::decode::<ClientMessage>(frame).unwrap()
                            == vec![ClientMessage::Ping])
                );
                assert!(
                    inbound
                        .borrow()
                        .iter()
                        .any(|frame| codec::decode::<ServerMessage>(frame).unwrap()
                            == vec![ServerMessage::Pong])
                );
            }
        });
    }
}

fn normalized_tap(frames: &[Frame]) -> std::collections::BTreeMap<String, Vec<Value>> {
    let mut joiner = t3_rpc::framing::Joiner::default();
    let mut grouped = std::collections::BTreeMap::<String, Vec<Value>>::new();
    for frame in frames {
        let partial = matches!(frame, Frame::Text(text) if text.starts_with('\u{1e}'));
        let frame = joiner.feed(frame.clone());
        if partial {
            continue;
        }
        for mut message in codec::decode::<Value>(&frame).unwrap() {
            if let Some(object) = message.as_object_mut() {
                for key in ["traceId", "spanId", "sampled"] {
                    object.remove(key);
                }
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
            let key = message
                .get("requestId")
                .or_else(|| message.get("id"))
                .unwrap_or(&message["_tag"])
                .to_string();
            grouped.entry(key).or_default().push(message);
        }
    }
    grouped
}

#[test]
fn async_server_drops_interrupted_futures_before_writing_the_exit_and_never_pulls_a_blocked_stream()
{
    block_on(async {
        let (mut tx, rx) = channel();
        let (out, mut inbound) = channel();
        let (started, _started_rx) = mpsc::unbounded();
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let polled = Rc::new(Cell::new(0));
        let handler = TestHandler {
            gates: Gates::default(),
            started,
            dropped: dropped.clone(),
            polled: polled.clone(),
        };
        let mut server = AsyncServer::new(out, rx, handler);
        server.state.register("wait", CallKind::Unary);
        server.state.register("numbers", CallKind::Stream);
        let id = RequestId::from(0_u64);
        tx.send(
            codec::encode(&ClientMessage::Request(Request::new(
                id.clone(),
                "wait",
                json!({"key":"a"}),
            )))
            .unwrap(),
        )
        .await
        .unwrap();
        server.step().await.unwrap();
        assert!(dropped.borrow().is_empty());
        tx.send(
            codec::encode(&ClientMessage::<Value>::Interrupt {
                request_id: id.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
        server.step().await.unwrap();
        assert_eq!(*dropped.borrow(), vec![id.clone()]);
        assert!(matches!(
            codec::decode::<ServerMessage>(&inbound.next().await.unwrap().unwrap())
                .unwrap()
                .as_slice(),
            [ServerMessage::Exit { .. }]
        ));
        server.step().await.unwrap();
        tx.send(
            codec::encode(&ClientMessage::Request(Request::new(
                id.clone(),
                "numbers",
                Value::Null,
            )))
            .unwrap(),
        )
        .await
        .unwrap();
        server.step().await.unwrap();
        server.step().await.unwrap();
        assert_eq!(polled.get(), 1);
        inbound.next().await.unwrap().unwrap();
        for _ in 0..3 {
            tx.send(codec::encode(&ClientMessage::<Value>::Ping).unwrap())
                .await
                .unwrap();
            server.step().await.unwrap();
            assert_eq!(polled.get(), 1);
            assert_eq!(
                codec::decode::<ServerMessage>(&inbound.next().await.unwrap().unwrap()).unwrap(),
                vec![ServerMessage::Pong]
            );
        }
        tx.send(codec::encode(&ClientMessage::<Value>::Ack { request_id: id }).unwrap())
            .await
            .unwrap();
        server.step().await.unwrap();
        server.step().await.unwrap();
        assert_eq!(polled.get(), 2);
    });
}

#[test]
fn async_client_samples_injected_time_on_frames_and_drives_keepalive_without_a_runtime_timer() {
    block_on(async {
        let (client_tx, mut server_rx) = channel();
        let (mut server_tx, client_rx) = channel();
        let (tick_tx, mut ticks) = mpsc::unbounded();
        let now = Rc::new(Cell::new(0));
        let server_clock = now.clone();
        let mut client = AsyncClient::<_, _, Value, Value, Value>::new(client_tx, client_rx, 0);
        let id = client
            .state
            .start("numbers", Value::Null, CallKind::Stream)
            .unwrap();
        let task = async {
            assert_eq!(
                client.next_event(&mut ticks, || now.get()).await.unwrap(),
                Some(CallEvent::Chunk {
                    id: id.clone(),
                    values: vec![json!(1)]
                })
            );
            assert_eq!(
                client.next_event(&mut ticks, || now.get()).await.unwrap(),
                Some(CallEvent::Failed {
                    id: id.clone(),
                    error: t3_rpc::ClientError::PingTimeout
                })
            );
        };
        let peer = async {
            assert!(matches!(
                codec::decode::<ClientMessage>(&server_rx.next().await.unwrap().unwrap())
                    .unwrap()
                    .as_slice(),
                [ClientMessage::Request(_)]
            ));
            server_clock.set(14000);
            server_tx
                .send(
                    codec::encode(&ServerMessage::<Value, Value>::Chunk {
                        request_id: id.clone(),
                        values: vec![json!(1)],
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
            server_clock.set(15000);
            tick_tx.unbounded_send(15000).unwrap();
            assert_eq!(
                codec::decode::<ClientMessage>(&server_rx.next().await.unwrap().unwrap()).unwrap(),
                vec![ClientMessage::Ack {
                    request_id: id.clone()
                }]
            );
            assert_eq!(
                codec::decode::<ClientMessage>(&server_rx.next().await.unwrap().unwrap()).unwrap(),
                vec![ClientMessage::Ping]
            );
            server_clock.set(20000);
            tick_tx.unbounded_send(20000).unwrap();
            assert_eq!(
                codec::decode::<ClientMessage>(&server_rx.next().await.unwrap().unwrap()).unwrap(),
                vec![ClientMessage::Ping]
            );
            server_clock.set(35000);
            tick_tx.unbounded_send(35000).unwrap();
        };
        futures::join!(task, peer);
    });
}

#[test]
fn malformed_frames_fail_calls_but_both_transports_remain_usable() {
    block_on(async {
        let (mut tx, rx) = channel();
        let (out, mut replies) = channel();
        let (started, _) = mpsc::unbounded();
        let mut server = AsyncServer::new(
            out,
            rx,
            TestHandler {
                gates: Gates::default(),
                started,
                dropped: Default::default(),
                polled: Default::default(),
            },
        );
        tx.send(Frame::Text("{".into())).await.unwrap();
        assert!(server.step().await.is_ok());
        assert!(!server.state.is_closed());
        assert!(matches!(
            codec::decode::<ServerMessage>(&replies.next().await.unwrap().unwrap())
                .unwrap()
                .as_slice(),
            [ServerMessage::Defect { .. }]
        ));
        tx.send(Frame::Text("{\"_tag\":\"Ping\"}".into()))
            .await
            .unwrap();
        server.step().await.unwrap();
        assert_eq!(
            codec::decode::<ServerMessage>(&replies.next().await.unwrap().unwrap()).unwrap(),
            vec![ServerMessage::Pong]
        );
    });
}

#[test]
fn malformed_server_frame_fails_calls_without_closing_the_client() {
    block_on(async {
        let (out, _requests) = channel();
        let (mut tx, rx) = channel();
        let mut client = AsyncClient::<_, _, Value, Value, Value>::new(out, rx, 0);
        let id = client
            .state
            .start("wait", Value::Null, CallKind::Unary)
            .unwrap();
        tx.send(Frame::Text("{".into())).await.unwrap();
        assert!(
            matches!(client.next_event(&mut stream::pending(), || 0).await.unwrap(), Some(CallEvent::Failed { id: actual, error: t3_rpc::ClientError::Protocol(_) }) if actual == id)
        );
        assert!(!client.state.is_closed());
        let id = client
            .state
            .start("echo", Value::Null, CallKind::Unary)
            .unwrap();
        tx.send(
            codec::encode(&ServerMessage::<Value>::Exit {
                request_id: id.clone(),
                exit: Exit::void(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
        assert!(
            matches!(client.next_event(&mut stream::pending(), || 0).await.unwrap(), Some(CallEvent::Exit { id: actual, .. }) if actual == id)
        );
    });
}

#[test]
fn buffered_stale_ticks_are_wakeups_using_the_same_clock_as_frames() {
    block_on(async {
        let (out, mut requests) = channel();
        let (_tx, rx) = channel();
        let mut client = AsyncClient::<_, _, Value, Value, Value>::new(out, rx, 0);
        client
            .state
            .start("wait", Value::Null, CallKind::Unary)
            .unwrap();
        let mut ticks = stream::iter([5000, 10000, 15000, 20000]).chain(stream::pending());
        assert!(
            client
                .next_event(&mut ticks, || 5000)
                .now_or_never()
                .is_none()
        );
        assert!(!client.state.is_closed());
        assert!(requests.next().now_or_never().is_some());
        assert_eq!(
            codec::decode::<ClientMessage>(
                &requests.next().now_or_never().unwrap().unwrap().unwrap()
            )
            .unwrap(),
            vec![ClientMessage::Ping]
        );
        assert!(requests.next().now_or_never().is_none());
    });
}

struct FaultHandler(&'static str);
impl Handler for FaultHandler {
    fn handle(&mut self, _: Request) -> HandlerOutput {
        match self.0 {
            "construct" => panic!("construction failure"),
            "future" => HandlerOutput::Unary(async { panic!("future failure") }.boxed_local()),
            "poll" => {
                HandlerOutput::Stream(stream::poll_fn(|_| panic!("stream failure")).boxed_local())
            }
            "kind" => HandlerOutput::Stream(stream::iter([Ok(vec![json!(1)])]).boxed_local()),
            _ => unreachable!(),
        }
    }
}

fn fault_stays_in_request(kind: &'static str) {
    block_on(async {
        let (mut tx, rx) = channel();
        let (out, mut replies) = channel();
        let mut server = AsyncServer::new(out, rx, FaultHandler(kind));
        server.state.register(
            "fault",
            if kind == "poll" {
                CallKind::Stream
            } else {
                CallKind::Unary
            },
        );
        tx.send(
            codec::encode(&ClientMessage::Request(Request::new(
                0_u64.into(),
                "fault",
                Value::Null,
            )))
            .unwrap(),
        )
        .await
        .unwrap();
        let result = std::panic::AssertUnwindSafe(async {
            server.step().await.unwrap();
            if kind != "construct" {
                server.step().await.unwrap();
            }
        })
        .catch_unwind()
        .await;
        assert!(result.is_ok(), "handler {kind} escaped the call");
        assert!(!server.state.is_closed());
        assert_eq!(server.state.active_count(), 0);
        assert!(matches!(
            codec::decode::<ServerMessage>(&replies.next().await.unwrap().unwrap())
                .unwrap()
                .as_slice(),
            [ServerMessage::Exit {
                exit: Exit::Failure { .. },
                ..
            }]
        ));
        tx.send(codec::encode(&ClientMessage::<Value>::Ping).unwrap())
            .await
            .unwrap();
        server.step().await.unwrap();
        assert_eq!(
            codec::decode::<ServerMessage>(&replies.next().await.unwrap().unwrap()).unwrap(),
            vec![ServerMessage::Pong]
        );
    });
}

#[test]
fn handler_construction_panics_are_request_defects() {
    fault_stays_in_request("construct");
}
#[test]
fn handler_future_panics_are_request_defects() {
    fault_stays_in_request("future");
}
#[test]
fn handler_stream_panics_are_request_defects() {
    fault_stays_in_request("poll");
}
#[test]
fn wrong_handler_output_kind_is_a_request_defect() {
    fault_stays_in_request("kind");
}

struct Unencodable;
impl serde::Serialize for Unencodable {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("fixture encoding error"))
    }
}
struct EncodingHandler(bool);
impl Handler<Value, Unencodable> for EncodingHandler {
    fn handle(&mut self, _: Request) -> HandlerOutput<Unencodable> {
        if self.0 {
            HandlerOutput::Stream(stream::iter([Ok(vec![Unencodable])]).boxed_local())
        } else {
            HandlerOutput::Unary(futures::future::ready(Exit::success(Unencodable)).boxed_local())
        }
    }
}

#[test]
fn encoding_failures_are_confined_to_the_request() {
    for stream in [false, true] {
        block_on(async {
            let (mut tx, rx) = channel();
            let (out, mut replies) = channel();
            let mut server = AsyncServer::new(out, rx, EncodingHandler(stream));
            server.state.register(
                "bad",
                if stream {
                    CallKind::Stream
                } else {
                    CallKind::Unary
                },
            );
            tx.send(
                codec::encode(&ClientMessage::Request(Request::new(
                    0_u64.into(),
                    "bad",
                    Value::Null,
                )))
                .unwrap(),
            )
            .await
            .unwrap();
            server.step().await.unwrap();
            assert!(server.step().await.is_ok());
            assert!(!server.state.is_closed());
            assert_eq!(server.state.active_count(), 0);
            assert!(matches!(
                codec::decode::<ServerMessage>(&replies.next().await.unwrap().unwrap())
                    .unwrap()
                    .as_slice(),
                [ServerMessage::Exit {
                    exit: Exit::Failure { .. },
                    ..
                }]
            ));
        });
    }
}

#[test]
fn server_enforces_the_product_websocket_message_limit_before_json_parsing() {
    block_on(async {
        let (mut tx, rx) = channel();
        let (out, _replies) = channel();
        let mut server = AsyncServer::new(out, rx, FaultHandler("construct"));
        tx.send(Frame::Text(" ".repeat(100 * 1024 * 1024 + 1)))
            .await
            .unwrap();
        assert!(server.step().await.is_err());
        assert!(server.state.is_closed());
    });
}
