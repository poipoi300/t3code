#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use t3_codex_app_server::{
    wire::{Message, Params, RequestId},
    *,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

type Peer = (BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>);
fn connected(options: ClientOptions) -> (Client, Events, Peer) {
    let (client, peer) = tokio::io::duplex(256 * 1024);
    let (read, write) = tokio::io::split(client);
    let (peer_read, peer_write) = tokio::io::split(peer);
    let (client, events) = Client::connect(read, write, options);
    (client, events, (BufReader::new(peer_read), peer_write))
}
async fn frame(peer: &mut Peer) -> Value {
    let mut line = String::new();
    assert!(peer.0.read_line(&mut line).await.unwrap() > 0);
    serde_json::from_str(&line).unwrap()
}
async fn emit(peer: &mut Peer, value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    peer.1.write_all(&bytes).await.unwrap();
}
async fn request_event(events: &mut Events) -> ServerRequest {
    match events.recv().await.unwrap().unwrap() {
        Event::Request(request) => *request,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn correlates_concurrent_responses_errors_and_cancelled_waiters() {
    let (client, _events, mut peer) = connected(Default::default());
    let mut pending = Vec::new();
    for method in ["model/list", "account/read", "skills/list"] {
        let client = client.clone();
        pending.push(tokio::spawn(async move {
            client.request_raw(method, Params::Missing).await
        }));
        let outbound = frame(&mut peer).await;
        assert!(outbound.get("jsonrpc").is_none());
        assert!(outbound.get("params").is_none());
    }
    emit(
        &mut peer,
        json!({"id":"2", "error":{"code":-32602,"message":"Invalid params","data":{"field":"x"}}}),
    )
    .await;
    let second = pending.remove(1).await.unwrap().unwrap_err();
    assert!(
        matches!(second, Error::Rpc { id: RequestId::String(_), method, error } if method == "account/read" && error.code == -32602.0 && error.data == Field::Value(json!({"field":"x"})))
    );
    pending[1].abort();
    assert!(pending.pop().unwrap().await.unwrap_err().is_cancelled());
    emit(&mut peer, json!({"id":3, "result":{"late":true}})).await;
    emit(&mut peer, json!({"id":1, "result":null})).await;
    assert_eq!(pending.pop().unwrap().await.unwrap().unwrap(), Value::Null);
    let cloned = client.clone();
    let next = tokio::spawn(async move {
        cloned
            .request_raw("config/read", Params::Present(Value::Null))
            .await
    });
    assert_eq!(
        frame(&mut peer).await,
        json!({"id":4,"method":"config/read","params":null})
    );
    emit(&mut peer, json!({"id":4,"result":{"config":{}}})).await;
    assert_eq!(next.await.unwrap().unwrap(), json!({"config":{}}));
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn approvals_do_not_block_notifications_responses_or_other_approvals() {
    let (client, mut events, mut peer) = connected(Default::default());
    let cloned = client.clone();
    let request = tokio::spawn(async move { cloned.model_list(&ModelListParams::default()).await });
    let outbound = frame(&mut peer).await;
    emit(&mut peer, json!({"id":"approval-1","method":"item/commandExecution/requestApproval","params":{"threadId":"t","turnId":"u","itemId":"cmd","command":"echo ok","startedAtMs":0}})).await;
    let approval = request_event(&mut events).await;
    assert!(
        matches!(approval.params(), ServerRequestParams::CommandApproval(p) if p.command == Field::Value("echo ok".into()))
    );
    emit(&mut peer, json!({"method":"item/agentMessage/delta","params":{"threadId":"t","turnId":"u","itemId":"i","delta":"hi"}})).await;
    assert!(
        matches!(events.recv().await.unwrap().unwrap(), Event::Notification { decoded, .. } if matches!(*decoded, ServerNotification::AgentMessageDelta(_)))
    );
    events.acknowledge();
    emit(
        &mut peer,
        json!({"id":outbound["id"],"result":{"data":[],"nextCursor":null}}),
    )
    .await;
    assert!(request.await.unwrap().unwrap().data.is_empty());
    assert!(!approval.is_resolved());
    let TypedServerRequest::CommandApproval(approval) = approval.into_typed() else {
        panic!("expected command approval")
    };
    approval
        .respond(ApprovalResponse {
            decision: json!("acceptForSession"),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        frame(&mut peer).await,
        json!({"id":"approval-1","result":{"decision":"acceptForSession"}})
    );
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn enforces_handler_limit_and_resolves_numeric_and_string_ids_separately() {
    let (client, mut events, mut peer) = connected(ClientOptions {
        max_server_requests: 2,
        ..Default::default()
    });
    for id in [json!(7), json!("7")] {
        emit(
            &mut peer,
            json!({"id":id,"method":"future/approval","params":null}),
        )
        .await;
    }
    let numeric = request_event(&mut events).await;
    let string = request_event(&mut events).await;
    emit(&mut peer, json!({"id":8,"method":"future/approval"})).await;
    assert_eq!(frame(&mut peer).await["error"]["code"], -32001);
    emit(
        &mut peer,
        json!({"method":"serverRequest/resolved","params":{"requestId":7,"threadId":"t"}}),
    )
    .await;
    events.recv().await.unwrap().unwrap();
    numeric.cancelled().await;
    assert!(!string.is_resolved());
    assert_eq!(reply(numeric, json!({})).await, Err(Error::RequestResolved));
    emit(&mut peer, json!({"id":9,"method":"future/approval"})).await;
    drop(request_event(&mut events).await);
    assert_eq!(
        frame(&mut peer).await,
        json!({"id":9,"error":{"code":-32601,"message":"Method not found: future/approval"}})
    );
    client.shutdown().await.unwrap();
    string.cancelled().await;
    assert!(matches!(reply(string, json!({})).await, Err(Error::Closed)));
}

#[tokio::test]
async fn decodes_fragmented_utf8_crlf_blanks_and_final_unterminated_frame() {
    let (client, mut events, mut peer) = connected(Default::default());
    let raw = json!({"method":"future/notification","params":{"delta":"é😀"}});
    let bytes = format!("\r\n  \n{}\r\n{}", raw, raw).into_bytes();
    for byte in bytes {
        peer.1.write_all(&[byte]).await.unwrap();
    }
    peer.1.shutdown().await.unwrap();
    for _ in 0..2 {
        match events.recv().await.unwrap().unwrap() {
            Event::Notification { raw: n, decoded } => {
                assert_eq!(serde_json::to_value(n).unwrap(), raw);
                assert!(matches!(*decoded, ServerNotification::Unknown(_)));
            }
            event => panic!("{event:?}"),
        }
    }
    assert_eq!(events.recv().await.unwrap().unwrap_err(), Error::Closed);
    assert!(events.recv().await.is_none());
    assert_eq!(client.await_termination().await, Error::Closed);
    assert_eq!(
        client.notify("initialized", Params::Missing).await,
        Err(Error::Closed)
    );
}

#[tokio::test]
async fn rejects_oversize_malformed_and_unroutable_frames_without_payload_diagnostics() {
    for (bytes, limit, expected) in [
        (b"12345".as_slice(), 4, Error::FrameTooLarge { limit: 4 }),
        (b"12345\n", 4, Error::FrameTooLarge { limit: 4 }),
        (b"{\"secret\":", 100, Error::Protocol("invalid JSON")),
        (
            b"{\"secret\":\"private-token\"}\n",
            100,
            Error::Protocol("missing response id"),
        ),
        (
            b"{\"id\":null,\"method\":\"x\"}\n",
            100,
            Error::Protocol("invalid request id"),
        ),
    ] {
        let (client, mut events, mut peer) = connected(ClientOptions {
            max_frame_bytes: limit,
            ..Default::default()
        });
        let cloned = client.clone();
        let pending = tokio::spawn(async move { cloned.request_raw("x", Params::Missing).await });
        frame(&mut peer).await;
        peer.1.write_all(bytes).await.unwrap();
        peer.1.shutdown().await.unwrap();
        assert!(
            matches!(events.recv().await.unwrap().unwrap(), Event::FrameDecodeFailed(error) if error == expected)
        );
        assert_eq!(events.recv().await.unwrap().unwrap_err(), Error::Closed);
        assert_eq!(pending.await.unwrap().unwrap_err(), Error::Closed);
        assert!(!format!("{expected:?}").contains("private-token"));
    }
}

#[tokio::test]
async fn bad_known_payloads_are_reported_once_and_requests_receive_invalid_params() {
    let (client, mut events, mut peer) = connected(Default::default());
    let invalid = json!({"method":"item/agentMessage/delta","params":{"delta":"private-payload"}});
    emit(&mut peer, invalid.clone()).await;
    emit(&mut peer, invalid).await;
    emit(
        &mut peer,
        json!({"method":"thread/closed","params":{"threadId":"t"}}),
    )
    .await;
    let diagnostic = events.recv().await.unwrap().unwrap();
    assert!(matches!(diagnostic, Event::NotificationDecodeFailed(_)));
    assert!(!format!("{diagnostic:?}").contains("private-payload"));
    assert!(matches!(
        events.recv().await.unwrap().unwrap(),
        Event::Notification { .. }
    ));
    emit(
        &mut peer,
        json!({"id":77,"method":"item/tool/requestUserInput","params":{}}),
    )
    .await;
    assert_eq!(frame(&mut peer).await["error"]["code"], -32602);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn initialization_is_serialized_and_tolerates_only_exact_already_initialized_error() {
    let (client, _events, mut peer) = connected(Default::default());
    let mut initializations = Vec::new();
    for _ in 0..2 {
        let client = client.clone();
        initializations.push(tokio::spawn(async move {
            client.initialize(&InitializeParams::t3("0.0.44")).await
        }));
    }
    let init = frame(&mut peer).await;
    assert_eq!(init["method"], "initialize");
    assert_eq!(
        init["params"]["capabilities"]["optOutNotificationMethods"],
        json!(["turn/diff/updated"])
    );
    emit(&mut peer, json!({"id":init["id"],"result":{"userAgent":"codex/0.159.0","codexHome":"/home/test/.codex","platformFamily":"unix","platformOs":"linux"}})).await;
    for init in initializations {
        assert_eq!(
            init.await.unwrap().unwrap().unwrap().user_agent,
            "codex/0.159.0"
        );
    }
    client.initialized().await.unwrap();
    client.initialized().await.unwrap();
    assert_eq!(frame(&mut peer).await, json!({"method":"initialized"}));
    client.shutdown().await.unwrap();
    for message in ["Already initialized", "Already initialized: wrong", "no"] {
        let (client, _events, mut peer) = connected(Default::default());
        let cloned = client.clone();
        let mut pending =
            tokio::spawn(async move { cloned.initialize(&InitializeParams::probe("test")).await });
        let mut init = frame(&mut peer).await;
        if message == "Already initialized" {
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
            emit(&mut peer, json!({"id":init["id"],"result":{"userAgent":"lost","codexHome":"/home/test/.codex","platformFamily":"unix","platformOs":"linux"}})).await;
            let cloned = client.clone();
            pending =
                tokio::spawn(
                    async move { cloned.initialize(&InitializeParams::probe("test")).await },
                );
            init = frame(&mut peer).await;
        }
        emit(
            &mut peer,
            json!({"id":init["id"],"error":{"code":-32600,"message":message}}),
        )
        .await;
        if message == "Already initialized" {
            assert!(pending.await.unwrap().unwrap().is_none());
            client.initialized().await.unwrap();
            assert_eq!(frame(&mut peer).await, json!({"method":"initialized"}));
        } else {
            assert!(pending.await.unwrap().is_err());
            let cloned = client.clone();
            let retry =
                tokio::spawn(
                    async move { cloned.initialize(&InitializeParams::probe("test")).await },
                );
            let retried = frame(&mut peer).await;
            emit(&mut peer, json!({"id":retried["id"],"result":{"userAgent":"retry-ok","codexHome":"/home/test/.codex","platformFamily":"unix","platformOs":"linux"}})).await;
            assert_eq!(
                retry.await.unwrap().unwrap().unwrap().user_agent,
                "retry-ok"
            );
            client.initialized().await.unwrap();
            assert_eq!(frame(&mut peer).await, json!({"method":"initialized"}));
        }
        client.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn failure_on_write_terminates_while_reader_remains_open() {
    let (read_client, _keep_open) = tokio::io::duplex(1024);
    let (write_client, closed_peer) = tokio::io::duplex(1024);
    drop(closed_peer);
    let (client, mut events) = Client::connect(read_client, write_client, Default::default());
    let error = client
        .request_raw("model/list", Params::Missing)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Transport {
            operation: "write",
            ..
        }
    ));
    assert_eq!(events.recv().await.unwrap().unwrap_err(), error);
    assert_eq!(client.await_termination().await, error);
}

#[test]
fn preserves_null_params_open_items_error_codes_and_turn_start_extensions() {
    assert_ne!(
        Message::decode(br#"{"method":"initialized"}"#).unwrap(),
        Message::decode(br#"{"method":"initialized","params":null}"#).unwrap()
    );
    for value in [
        json!({"id":"i","type":"imageGeneration","status":"completed","fileId":"f"}),
        json!({"id":"i","type":"reasoning"}),
        json!({"id":"i","type":"futureItem","field":true}),
        json!({"id":"i","type":"agentMessage","text":"ok","phase":"partial_answer"}),
    ] {
        let item: ThreadItem = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(item).unwrap(), value);
    }
    let params = json!({"threadId":"t","input":[{"type":"text","text":"hi"}],"collaborationMode":{"mode":"default","settings":{"model":"gpt-6-luna","reasoning_effort":"ultra"}},"additionalContext":{"x":{"kind":"untrusted","value":"text"}},"cyberAccessProgram":"daybreakBlue"});
    let typed: TurnStartParams = serde_json::from_value(params.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), params);
    let notification = ServerNotification::decode(&serde_json::from_value(json!({"method":"error","params":{"threadId":"t","turnId":"u","willRetry":false,"error":{"message":"failed","codexErrorInfo":"futureError"}}})).unwrap()).unwrap();
    assert!(
        matches!(notification, ServerNotification::Error(e) if e.error.codex_error_info == Field::Value(json!("futureError")))
    );
    let limits = json!({"primary":null,"planType":null,"secondary":{"usedPercent":10.0}});
    let typed: RateLimitSnapshot = serde_json::from_value(limits.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), limits);
}

#[test]
fn user_inputs_and_drifted_known_items_round_trip_without_losing_fields() {
    for input in [
        json!({"type":"text","text":"hi","text_elements":[{"byteRange":{"start":0,"end":2},"placeholder":"x"}]}),
        json!({"type":"image","fileId":"f","detail":"high"}),
        json!({"type":"image","url":"https://image","detail":null}),
        json!({"type":"localImage","path":"image.png","detail":"auto"}),
        json!({"type":"audio","url":"https://audio"}),
        json!({"type":"localAudio","path":"audio.wav"}),
        json!({"type":"futureInput","payload":42}),
        json!({"type":"text","text":"hi","futureField":{"value":42}}),
    ] {
        let typed: UserInput = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), input);
        let item = json!({"id":"i","type":"userMessage","content":[input]});
        let typed: ThreadItem = serde_json::from_value(item.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), item);
    }
    let item = json!({"id":"i","type":"agentMessage","text":{"future":"shape"}});
    let typed: ThreadItem = serde_json::from_value(item.clone()).unwrap();
    assert!(matches!(typed.body, ItemBody::Other { .. }));
    assert_eq!(serde_json::to_value(typed).unwrap(), item);
}

#[test]
fn permission_scope_is_optional_and_never_defaults_to_an_empty_string() {
    let value = json!({"permissions":{}});
    let typed: PermissionsApprovalResponse = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), value);
    assert_eq!(
        serde_json::to_value(PermissionsApprovalResponse {
            permissions: json!({}),
            ..Default::default()
        })
        .unwrap(),
        value
    );
}

#[tokio::test]
async fn duplicate_id_replaces_only_its_handle_and_never_approves() {
    let (client, mut events, mut peer) = connected(Default::default());
    for _ in 0..2 {
        emit(&mut peer, json!({"id":7,"method":"future/approval"})).await;
    }
    let old = request_event(&mut events).await;
    let new = request_event(&mut events).await;
    assert!(old.is_resolved());
    assert_eq!(
        reply(old, json!({"decision":"accept"})).await,
        Err(Error::RequestResolved)
    );
    drop(new);
    assert_eq!(frame(&mut peer).await["error"]["code"], -32601);
    client.notify("barrier", Params::Missing).await.unwrap();
    assert_eq!(frame(&mut peer).await, json!({"method":"barrier"}));
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn bad_reply_fails_only_the_correlated_request_and_bad_frames_are_skipped() {
    let (client, mut events, mut peer) = connected(Default::default());
    let c = client.clone();
    let one = tokio::spawn(async move { c.request_raw("one", Params::Missing).await });
    frame(&mut peer).await;
    let c = client.clone();
    let two = tokio::spawn(async move { c.request_raw("two", Params::Missing).await });
    frame(&mut peer).await;
    peer.1
        .write_all(b"broken JSON\n42\n{\"id\":1,\"error\":false}\n")
        .await
        .unwrap();
    // Drain diagnostics concurrently: their delivery also orders the valid reply.
    let drain = tokio::spawn(async move { while events.recv().await.is_some() {} });
    emit(&mut peer, json!({"id":2,"result":"ok"})).await;
    assert!(matches!(
        one.await.unwrap(),
        Err(Error::Protocol("invalid RPC error"))
    ));
    assert_eq!(two.await.unwrap().unwrap(), json!("ok"));
    client.shutdown().await.unwrap();
    drain.await.unwrap();
}

#[test]
fn accepts_javascript_numeric_ids_and_error_codes_and_lossy_utf8() {
    for source in ["1.0", "1.5", "9223372036854775808", "1e40"] {
        let bytes = format!(r#"{{"id":{source},"error":{{"code":-1.5,"message":"failed"}}}}"#);
        assert!(Message::decode(bytes.as_bytes()).is_ok(), "{source}");
    }
    let message = Message::decode(b"{\"method\":\"future\",\"params\":\"\xff\"}").unwrap();
    assert!(
        matches!(message, Message::Notification(n) if n.params == Params::Present(json!("\u{fffd}")))
    );
}

#[tokio::test]
async fn initialize_leaves_the_version_gate_before_initialized() {
    let (client, _events, mut peer) = connected(Default::default());
    let c = client.clone();
    let init = tokio::spawn(async move { c.initialize(&InitializeParams::probe("test")).await });
    let request = frame(&mut peer).await;
    emit(&mut peer, json!({"id":request["id"],"result":{"userAgent":"version","codexHome":"/tmp/test","platformFamily":"unix","platformOs":"linux"}})).await;
    init.await.unwrap().unwrap();
    client.notify("versionGate", Params::Missing).await.unwrap();
    assert_eq!(frame(&mut peer).await, json!({"method":"versionGate"}));
    client.shutdown().await.unwrap();
}

#[test]
fn payload_errors_record_a_field_path_without_the_value() {
    let params = Params::Present(json!({"threadId":"t","input":"private-input"}));
    let error = params.decode::<TurnStartParams>("turn/start").unwrap_err();
    assert!(format!("{error}").contains("input"));
    assert!(!format!("{error:?}").contains("private-input"));
}

#[tokio::test]
async fn oversized_frame_is_bounded_and_does_not_end_the_session() {
    let (client, mut events, mut peer) = connected(ClientOptions {
        max_frame_bytes: 32,
        ..Default::default()
    });
    let c = client.clone();
    let request = tokio::spawn(async move { c.request_raw("x", Params::Missing).await });
    frame(&mut peer).await;
    peer.1.write_all(b"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n{\"id\":1,\"result\":42}\n").await.unwrap();
    assert!(matches!(
        events.recv().await.unwrap().unwrap(),
        Event::FrameDecodeFailed(Error::FrameTooLarge { limit: 32 })
    ));
    events.acknowledge();
    assert_eq!(request.await.unwrap().unwrap(), json!(42));
    client.shutdown().await.unwrap();
}

async fn reply(request: ServerRequest, value: Value) -> Result<()> {
    let TypedServerRequest::Unknown(request) = request.into_typed() else {
        panic!("expected extension request")
    };
    request.respond(value).await
}

async fn approve(request: ServerRequest) -> Result<()> {
    match request.into_typed() {
        TypedServerRequest::CommandApproval(r) => {
            r.respond(ApprovalResponse {
                decision: json!("accept"),
                ..Default::default()
            })
            .await
        }
        TypedServerRequest::FileChangeApproval(r) => {
            r.respond(FileChangeApprovalResponse {
                decision: json!("accept"),
                ..Default::default()
            })
            .await
        }
        TypedServerRequest::PermissionsApproval(r) => {
            r.respond(PermissionsApprovalResponse {
                permissions: json!({"network":{"enabled":true}}),
                ..Default::default()
            })
            .await
        }
        other => panic!("unexpected approval: {other:?}"),
    }
}

#[tokio::test]
async fn permission_boundary_never_approves_on_drop_cancellation_shutdown_malformed_or_reused_id() {
    for method in [
        "item/commandExecution/requestApproval",
        "item/fileChange/requestApproval",
        "item/permissions/requestApproval",
    ] {
        let (client, mut events, mut peer) = connected(Default::default());
        let params = json!({"threadId":"t","turnId":"u","itemId":"i","startedAtMs":0,"cwd":"/tmp","permissions":{"network":{"enabled":true}}});
        let request = json!({"id":7,"method":method,"params":params});
        let malformed = json!({"id":7,"method":method,"params":{"private":"do not approve"}});
        emit(&mut peer, malformed.clone()).await;
        assert_eq!(frame(&mut peer).await["error"]["code"], -32602);

        emit(&mut peer, request.clone()).await;
        drop(request_event(&mut events).await);
        assert_eq!(frame(&mut peer).await["error"]["code"], -32601);

        emit(&mut peer, request.clone()).await;
        let cancelled = request_event(&mut events).await;
        emit(
            &mut peer,
            json!({"method":"serverRequest/resolved","params":{"requestId":7,"threadId":"t"}}),
        )
        .await;
        events.recv().await.unwrap().unwrap();
        assert_eq!(approve(cancelled).await, Err(Error::RequestResolved));

        emit(&mut peer, request.clone()).await;
        let old = request_event(&mut events).await;
        emit(&mut peer, request.clone()).await;
        let replacement = request_event(&mut events).await;
        assert_eq!(approve(old).await, Err(Error::RequestResolved));
        drop(replacement);
        assert_eq!(frame(&mut peer).await["error"]["code"], -32601);

        emit(&mut peer, request.clone()).await;
        let old = request_event(&mut events).await;
        emit(&mut peer, malformed).await;
        assert_eq!(frame(&mut peer).await["error"]["code"], -32602);
        assert_eq!(approve(old).await, Err(Error::RequestResolved));

        emit(&mut peer, request.clone()).await;
        let cancelled_task = request_event(&mut events).await;
        let (entered, started) = tokio::sync::oneshot::channel();
        let handler = tokio::spawn(async move {
            let _held = cancelled_task;
            entered.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started.await.unwrap();
        handler.abort();
        assert!(handler.await.unwrap_err().is_cancelled());
        assert_eq!(frame(&mut peer).await["error"]["code"], -32601);

        client
            .notify("boundaryBarrier", Params::Missing)
            .await
            .unwrap();
        assert_eq!(frame(&mut peer).await, json!({"method":"boundaryBarrier"}));
        emit(&mut peer, request.clone()).await;
        let at_shutdown = request_event(&mut events).await;
        client.shutdown().await.unwrap();
        assert_eq!(approve(at_shutdown).await, Err(Error::Closed));
        let mut tail = String::new();
        assert_eq!(peer.0.read_line(&mut tail).await.unwrap(), 0);

        let (client, mut events, mut peer) = connected(Default::default());
        emit(&mut peer, request).await;
        let at_drop = request_event(&mut events).await;
        drop(client);
        at_drop.cancelled().await;
        assert_eq!(approve(at_drop).await, Err(Error::Closed));
        assert_eq!(peer.0.read_line(&mut tail).await.unwrap(), 0);
    }
}

#[test]
fn rpc_errors_preserve_explicit_null_data() {
    let wire = json!({"id":1,"error":{"code":-1.5,"message":"failed","data":null}});
    let message = Message::decode(&serde_json::to_vec(&wire).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&message.encode().unwrap()).unwrap(),
        wire
    );
}

#[test]
fn all_open_unions_preserve_unknown_and_drifted_known_shapes() {
    macro_rules! other {
        ($t:ident, $value:expr) => {{
            let value = $value;
            let typed: $t = serde_json::from_value(value.clone()).unwrap();
            assert!(matches!(typed, $t::Other { .. }));
            assert_eq!(serde_json::to_value(typed).unwrap(), value);
        }};
    }
    for kind in ["futureAccount", "chatgpt"] {
        other!(Account, json!({"type":kind,"email":42,"extension":"value"}));
    }
    for kind in ["futureStatus", "active"] {
        other!(
            ThreadStatus,
            json!({"type":kind,"activeFlags":{},"extension":"value"})
        );
    }
    for kind in ["futureChange", "update"] {
        other!(
            FileChangeKind,
            json!({"type":kind,"move_path":42,"extension":"value"})
        );
    }
    for kind in ["futureMode", "form"] {
        other!(
            ElicitationParams,
            json!({"mode":kind,"serverName":42,"extension":"value"})
        );
    }
    let body: FunctionCallOutputBody = serde_json::from_value(json!({"future":"output"})).unwrap();
    assert!(matches!(body, FunctionCallOutputBody::Other(_)));
    assert_eq!(
        serde_json::to_value(body).unwrap(),
        json!({"future":"output"})
    );
}

#[tokio::test]
async fn malformed_resolved_notification_still_invalidates_its_exact_handle() {
    let (client, mut events, mut peer) = connected(Default::default());
    emit(&mut peer, json!({"id":7,"method":"future/approval"})).await;
    let request = request_event(&mut events).await;
    emit(
        &mut peer,
        json!({"method":"serverRequest/resolved","params":{"requestId":7}}),
    )
    .await;
    assert!(matches!(
        events.recv().await.unwrap().unwrap(),
        Event::NotificationDecodeFailed(_)
    ));
    assert!(request.is_resolved());
    assert_eq!(
        reply(request, json!({"decision":"accept"})).await,
        Err(Error::RequestResolved)
    );
    client.notify("barrier", Params::Missing).await.unwrap();
    assert_eq!(frame(&mut peer).await, json!({"method":"barrier"}));
    client.shutdown().await.unwrap();
}
