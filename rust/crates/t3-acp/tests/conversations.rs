#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use std::sync::Arc;
use t3_acp::{
    Client, ClientHandler, ClientOptions, ClientRequest, Error, Event, Events, Generation,
    RejectRequests, RequestContext, RequestId, RpcError, async_trait,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

struct Peer {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}
impl Peer {
    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.unwrap();
    }
    async fn receive(&mut self) -> Value {
        let mut line = String::new();
        assert_ne!(self.reader.read_line(&mut line).await.unwrap(), 0);
        serde_json::from_str(&line).unwrap()
    }
    async fn reply(&mut self, request: &Value, result: Value) {
        self.send(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
            .await;
    }
    async fn update(&mut self, session: &str, update: Value) {
        self.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":update}})).await;
    }
}

fn connect(handler: Arc<dyn ClientHandler>) -> (Client, Events, Peer) {
    let (client_pipe, peer_pipe) = tokio::io::duplex(65536);
    let (reader, writer) = tokio::io::split(client_pipe);
    let (reader2, writer2) = tokio::io::split(peer_pipe);
    let (client, events) = Client::connect(reader, writer, handler, ClientOptions::default());
    (
        client,
        events,
        Peer {
            reader: BufReader::new(reader2),
            writer: writer2,
        },
    )
}

async fn initialize(client: &Client, peer: &mut Peer, v2: bool) {
    let c = client.clone();
    let task = tokio::spawn(async move {
        c.initialize(json!({"clientInfo":{"name":"test","version":"1"}, "clientCapabilities":{"fs":{"readTextFile":true},"terminal":true}})).await
    });
    let request = peer.receive().await;
    assert_eq!(request["params"]["protocolVersion"], 2);
    assert!(request["params"].get("capabilities").is_some());
    assert_eq!(request["params"]["clientCapabilities"]["terminal"], true);
    peer.reply(&request, if v2 { json!({"protocolVersion":2,"info":{"name":"agent","version":"1"},"capabilities":{"session":{}}}) } else { json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true}}) }).await;
    assert_eq!(
        task.await.unwrap().unwrap().generation,
        if v2 { Generation::V2 } else { Generation::V1 }
    );
}

struct Approve;
#[async_trait]
impl ClientHandler for Approve {
    async fn request(
        &self,
        request: ClientRequest,
        context: RequestContext,
    ) -> Result<Value, RpcError> {
        match request {
            ClientRequest::Permission(params) => {
                assert_eq!(params["toolCall"]["toolCallId"], "tool-1");
                assert_eq!(context.id, RequestId::Number(0));
                Ok(json!({"outcome":{"outcome":"selected","optionId":"allow"}}))
            }
            ClientRequest::Extension { method, params } => {
                Ok(json!({"method":method,"params":params}))
            }
            _ => Err(RpcError::method_not_found(&context.method)),
        }
    }
}

#[tokio::test]
async fn full_turn_streams_tools_permission_and_unknown_extensions() {
    for v2 in [false, true] {
        let (client, mut events, mut peer) = connect(Arc::new(Approve));
        let collector = tokio::spawn(async move {
            let mut kinds = Vec::new();
            let mut extension_seen = false;
            while kinds.len() < if v2 { 5 } else { 4 } {
                match events.next().await.unwrap().unwrap() {
                    Event::SessionUpdate(update) => kinds.push(update.kind().to_owned()),
                    Event::Notification { method, params } => {
                        assert_eq!(method, "_vendor/notice");
                        assert_eq!(params, json!({"future":true}));
                        extension_seen = true;
                    }
                    _ => {}
                }
            }
            events.acknowledge();
            (events, kinds, extension_seen)
        });
        initialize(&client, &mut peer, v2).await;
        let c = client.clone();
        let prompt = tokio::spawn(async move {
            c.prompt(json!({"sessionId":"s","prompt":[{"type":"text","text":"hello"}]}))
                .await
        });
        let request = peer.receive().await;
        let updates = [
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"thinking"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Read","kind":"read","status":"pending"}),
        ];
        for update in updates {
            peer.update("s", update).await;
        }
        let permission = if v2 {
            json!({"sessionId":"s","title":"Read","subject":{"type":"tool_call","toolCall":{"toolCallId":"tool-1"}},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]})
        } else {
            json!({"sessionId":"s","toolCall":{"toolCallId":"tool-1"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]})
        };
        peer.send(json!({"jsonrpc":"2.0","id":0,"method":"session/request_permission","params":permission})).await;
        let approval = peer.receive().await;
        assert_eq!(approval["id"], 0);
        assert_eq!(approval["result"]["outcome"]["optionId"], "allow");
        peer.update("s", json!({"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"file"}}]})).await;
        peer.send(json!({"jsonrpc":"2.0","id":"extension","method":"_vendor/question","params":{"future":[1,2]}})).await;
        let extension = peer.receive().await;
        assert_eq!(
            extension["result"],
            json!({"method":"_vendor/question","params":{"future":[1,2]}})
        );
        peer.send(json!({"jsonrpc":"2.0","method":"_vendor/notice","params":{"future":true}}))
            .await;
        peer.update(
            "s",
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}),
        )
        .await;
        if v2 {
            peer.update(
                "s",
                json!({"sessionUpdate":"state_update","state":"idle","stopReason":"end_turn"}),
            )
            .await;
            peer.reply(&request, json!({"messageId":"accepted"})).await;
        } else {
            peer.reply(&request, json!({"stopReason":"end_turn"})).await;
        }
        assert_eq!(prompt.await.unwrap().unwrap()["stopReason"], "end_turn");
        let (_events, kinds, extension_seen) = collector.await.unwrap();
        assert!(extension_seen);
        assert_eq!(
            &kinds[..4],
            [
                "agent_thought_chunk",
                "tool_call",
                "tool_call_update",
                "agent_message_chunk"
            ]
        );
        client.shutdown().await;
    }
}

#[tokio::test]
async fn cancel_mid_turn_waits_for_agent() {
    let (client, _events, mut peer) = connect(Arc::new(RejectRequests));
    initialize(&client, &mut peer, false).await;
    let params = json!({"sessionId":"s","prompt":[]});
    let c = client.clone();
    let input = params.clone();
    let prompt = tokio::spawn(async move { c.prompt(input).await });
    let request = peer.receive().await;

    let c = client.clone();
    let cancel = tokio::spawn(async move { c.cancel_and_wait(json!({"sessionId":"s"})).await });
    let frame = peer.receive().await;
    assert_eq!(frame["method"], "session/cancel");
    assert!(frame.get("id").is_none());
    assert!(!cancel.is_finished());
    peer.reply(&request, json!({"stopReason":"cancelled","usage":{}}))
        .await;
    assert_eq!(prompt.await.unwrap().unwrap()["stopReason"], "cancelled");
    cancel.await.unwrap().unwrap();
    client.shutdown().await;
}

#[tokio::test]
async fn load_resume_normalize_draft_mcp_and_config_ids() {
    for v2 in [false, true] {
        let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
        initialize(&client, &mut peer, v2).await;
        for load in [true, false] {
            let c = client.clone();
            let task = tokio::spawn(async move {
                let params = json!({"sessionId":"s","cwd":"/workspace","mcpServers":[{"name":"m","command":"mcp"}]});
                if load {
                    c.load_session(params).await
                } else {
                    c.resume_session(params).await
                }
            });
            let request = peer.receive().await;
            assert_eq!(
                request["method"],
                if load && !v2 {
                    "session/load"
                } else {
                    "session/resume"
                }
            );
            if v2 {
                assert_eq!(request["params"]["mcpServers"][0]["type"], "stdio");
            }
            if v2 && load {
                assert_eq!(request["params"]["replayFrom"], json!({"type":"start"}));
            }
            peer.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","_meta":{"isReplay":true},"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"old"}}}})).await;
            peer.reply(&request,json!({"configOptions":[{"type":"select","configId":"model","id":"model","name":"Model","currentValue":"m","options":[]}]})).await;
            let Event::SessionUpdate(update) = events.next().await.unwrap().unwrap() else {
                panic!()
            };
            assert!(update.is_replay());
            events.acknowledge();
            let result = task.await.unwrap().unwrap();
            assert_eq!(result["configOptions"][0]["id"], "model");
            assert_eq!(update.session_id, "s");
        }
        client.shutdown().await;
    }
}

#[tokio::test]
async fn legacy_initialize_fixture_uses_shape_instead_of_protocol_number() {
    let (client, _events, mut peer) = connect(Arc::new(RejectRequests));
    let c = client.clone();
    let task = tokio::spawn(async move { c.initialize(json!({})).await });
    let request = peer.receive().await;
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/antigravity-initialize.json")).unwrap();
    peer.reply(&request, fixture.clone()).await;
    let negotiated = task.await.unwrap().unwrap();
    assert_eq!(negotiated.generation, Generation::V1);
    assert_eq!(negotiated.raw, fixture);
    let c = client.clone();
    let auth =
        tokio::spawn(async move { c.authenticate(json!({"methodId":"oauth-personal"})).await });
    let request = peer.receive().await;
    assert_eq!(request["method"], "authenticate");
    peer.reply(&request, json!({})).await;
    auth.await.unwrap().unwrap();
    client.shutdown().await;
}

#[tokio::test]
async fn failed_v2_idle_keeps_error_and_waits_for_prompt_ack() {
    let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
    initialize(&client, &mut peer, true).await;
    let c = client.clone();
    let prompt = tokio::spawn(async move { c.prompt(json!({"sessionId":"s","prompt":[]})).await });
    let request = peer.receive().await;
    let error = json!({"code":-32000,"message":"Missing Authentication header","data":{"x":true}});
    peer.update(
        "s",
        json!({"sessionUpdate":"state_update","state":"idle","stopReason":"error","error":error}),
    )
    .await;
    let _ = events.next().await.unwrap().unwrap();
    events.acknowledge();
    assert!(!prompt.is_finished());
    peer.reply(&request, json!({})).await;
    assert_eq!(
        prompt.await.unwrap().unwrap(),
        json!({"stopReason":"error","error":error})
    );
    client.shutdown().await;
}

#[tokio::test]
async fn protocol_batches_preserve_exact_ids_errors_and_handler_panics() {
    struct Handler;
    #[async_trait]
    impl ClientHandler for Handler {
        async fn request(&self, _: ClientRequest, c: RequestContext) -> Result<Value, RpcError> {
            match c.method.as_str() {
                "_ok" => Ok(json!({"ok":true})),
                "_bad" => Err(RpcError {
                    code: -32602,
                    message: "bad input".into(),
                    data: Some(json!({"field":"name"})),
                }),
                _ => panic!("handler defect"),
            }
        }
    }
    let (client, mut events, mut peer) = connect(Arc::new(Handler));
    peer.send(json!([
        {"jsonrpc":"2.0","id":0,"method":"_ok","params":{}},
        {"jsonrpc":"2.0","id":"0","method":"_bad","params":{}},
        {"jsonrpc":"2.0","id":"$t3:jsonrpc:number:0","method":"_panic","params":{}}
    ]))
    .await;
    let response = peer.receive().await;
    assert_eq!(
        response[0],
        json!({"jsonrpc":"2.0","id":0,"result":{"ok":true}})
    );
    assert_eq!(response[1]["id"], "0");
    assert_eq!(response[1]["error"]["data"], json!({"field":"name"}));
    assert_eq!(response[2]["error"]["code"], -32603);
    for id in [
        RequestId::Number(0),
        RequestId::String("0".into()),
        RequestId::String("$t3:jsonrpc:number:0".into()),
    ] {
        assert_eq!(
            events.next().await.unwrap().unwrap(),
            Event::ResponseSent { id }
        );
    }
    client.shutdown().await;
}

#[tokio::test]
async fn unknown_string_reply_never_completes_numeric_request() {
    let (client, _events, mut peer) = connect(Arc::new(RejectRequests));
    let c = client.clone();
    let request = tokio::spawn(async move { c.request("_echo", json!({})).await });
    let frame = peer.receive().await;
    let id = frame["id"].as_i64().unwrap();
    assert!((1..=i32::MAX as i64).contains(&id));
    peer.send(json!([
        {"jsonrpc":"2.0","id":id.to_string(),"result":{"wrong":true}},
        {"jsonrpc":"2.0","id":"agent-internal","result":{}},
        {"jsonrpc":"2.0","id":id,"result":{"correct":true}}
    ]))
    .await;
    assert_eq!(request.await.unwrap().unwrap(), json!({"correct":true}));
    client.shutdown().await;
}

#[tokio::test]
async fn fragmented_utf8_final_response_and_malformed_lines() {
    let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
    let c = client.clone();
    let task = tokio::spawn(async move { c.request("_echo", json!({})).await });
    let request = peer.receive().await;
    let bytes =
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":request["id"],"result":{"text":"é🦀"}}))
            .unwrap();
    for byte in bytes {
        peer.writer.write_all(&[byte]).await.unwrap();
    }
    peer.writer.shutdown().await.unwrap();
    assert_eq!(task.await.unwrap().unwrap(), json!({"text":"é🦀"}));
    assert_eq!(events.next().await, Some(Err(Error::Closed)));
    assert!(events.next().await.is_none());
    assert_eq!(
        client.request("_after", json!({})).await.unwrap_err(),
        Error::Closed
    );
    client.shutdown().await;
    for (bytes, error) in [
        (b"{broken}\n".as_slice(), Error::InvalidFrame),
        (b"{truncated".as_slice(), Error::Closed),
    ] {
        let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
        peer.writer.write_all(bytes).await.unwrap();
        peer.writer.shutdown().await.unwrap();
        assert_eq!(events.next().await, Some(Err(error)));
        client.shutdown().await;
    }
}

#[tokio::test(start_paused = true)]
async fn load_replay_idle_is_session_local_and_captures_new_options() {
    let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
    initialize(&client, &mut peer, false).await;
    let c = client.clone();
    let load = tokio::spawn(async move {
        c.load_session(json!({"sessionId":"s","cwd":"/w","mcpServers":[]}))
            .await
    });
    let request = peer.receive().await;
    peer.update(
        "other",
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ongoing"}}),
    )
    .await;
    let _ = events.next().await.unwrap().unwrap();
    events.acknowledge();
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    assert!(!load.is_finished());
    peer.update("s",json!({"sessionUpdate":"config_option_update","configOptions":[{"id":"model","currentValue":"new"}]})).await;
    let _ = events.next().await.unwrap().unwrap();
    events.acknowledge();
    let result = load.await.unwrap().unwrap();
    assert_eq!(result["_meta"]["t3SessionLoadReady"], "replay_idle");
    assert_eq!(result["configOptions"][0]["currentValue"], "new");
    peer.reply(&request, json!({"configOptions":[]})).await;
    let c = client.clone();
    let echo = tokio::spawn(async move { c.request("_barrier", json!({})).await });
    let frame = peer.receive().await;
    peer.reply(&frame, json!({})).await;
    echo.await.unwrap().unwrap();
    assert_eq!(
        client.session_state("s").config_options.unwrap()[0]["currentValue"],
        "new"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn newer_config_notification_wins_over_stale_response() {
    let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
    initialize(&client, &mut peer, false).await;
    let c = client.clone();
    let set = tokio::spawn(async move {
        c.set_config_option(json!({"sessionId":"s","configId":"model","value":"new"}))
            .await
    });
    let frame = peer.receive().await;
    let options = json!([{"id":"model","currentValue":"new"}]);
    peer.update(
        "s",
        json!({"sessionUpdate":"config_option_update","configOptions":options}),
    )
    .await;
    let _ = events.next().await.unwrap().unwrap();
    events.acknowledge();
    peer.reply(&frame, json!({"configOptions":[]})).await;
    set.await.unwrap().unwrap();
    assert_eq!(client.session_state("s").config_options, Some(options));
    client.shutdown().await;
}

#[tokio::test]
async fn future_variants_structured_diffs_and_null_names_are_preserved() {
    let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
    initialize(&client, &mut peer, false).await;
    let updates = [
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"chart","points":[1,2]}}),
        json!({"sessionUpdate":"timeline_update","entries":[1]}),
        json!({"sessionUpdate":"tool_call_update","toolCallId":"t","name":null,"content":[{"type":"diff","changes":[{"operation":"move","path":"/new","oldPath":"/old","fileType":"text"}],"patch":{"format":"git_patch","text":"rename"}}]}),
        json!({"sessionUpdate":"notice","severity":"warning","title":"MCP unavailable"}),
        json!({"sessionUpdate":"notice","severity":"info"}),
        json!({"sessionUpdate":"future_message_chunk","futurePayload":[1,2]}),
    ];
    for update in &updates {
        peer.update("s", update.clone()).await;
    }
    for (index, original) in updates.iter().enumerate() {
        let Event::SessionUpdate(update) = events.next().await.unwrap().unwrap() else {
            panic!()
        };
        assert_eq!(update.raw["update"], *original);
        if index == 1 {
            assert!(!update.is_known());
        }
        if index == 2 {
            assert!(update.update.get("name").is_none());
            assert_eq!(update.update["content"], original["content"]);
        }
        if index == 4 {
            assert_eq!(update.kind(), "_t3_unknown");
        }
    }
    client.shutdown().await;
}

#[tokio::test]
async fn client_served_files_terminals_elicitation_and_mcp_are_scriptable() {
    struct Script(tokio::sync::mpsc::UnboundedSender<(ClientRequest, RequestContext)>);
    #[async_trait]
    impl ClientHandler for Script {
        async fn request(
            &self,
            request: ClientRequest,
            context: RequestContext,
        ) -> Result<Value, RpcError> {
            self.0.send((request.clone(), context)).unwrap();
            Ok(match request {
                ClientRequest::ReadTextFile(_) => json!({"content":"file contents"}),
                ClientRequest::TerminalCreate(_) => json!({"terminalId":"term"}),
                ClientRequest::TerminalOutput(_) => json!({"output":"out","truncated":false}),
                ClientRequest::TerminalWaitForExit(_) => json!({"exitCode":0}),
                ClientRequest::Elicitation(_) => {
                    json!({"action":"accept","content":{"choice":"a"},"_meta":{"answer":true}})
                }
                ClientRequest::McpConnect(_) => json!({"connectionId":"mcp"}),
                ClientRequest::McpMessage(_) => json!({"result":{"tools":[]}}),
                _ => json!({}),
            })
        }
    }
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let (client, _events, mut peer) = connect(Arc::new(Script(sent)));
    let form = json!({"sessionId":"s","mode":"form","message":"Choose","requestedSchema":{"type":"object","properties":{"choice":{"type":"string","enum":["a"]}}}});
    let methods = [
        (
            "fs/read_text_file",
            json!({"sessionId":"s","path":"/file","line":1,"limit":10,"vendor":true}),
        ),
        (
            "fs/write_text_file",
            json!({"sessionId":"s","path":"/file","content":"contents"}),
        ),
        (
            "terminal/create",
            json!({"sessionId":"s","command":"shell","args":[],"env":[],"cwd":"/w"}),
        ),
        (
            "terminal/output",
            json!({"sessionId":"s","terminalId":"term"}),
        ),
        (
            "terminal/wait_for_exit",
            json!({"sessionId":"s","terminalId":"term"}),
        ),
        (
            "terminal/kill",
            json!({"sessionId":"s","terminalId":"term"}),
        ),
        (
            "terminal/release",
            json!({"sessionId":"s","terminalId":"term"}),
        ),
        ("elicitation/create", form.clone()),
        ("session/elicitation", form.clone()),
        ("_session/elicitation", form),
        ("mcp/connect", json!({"serverId":"tools"})),
        (
            "mcp/message",
            json!({"connectionId":"mcp","method":"tools/list"}),
        ),
        ("mcp/disconnect", json!({"connectionId":"mcp"})),
        (
            "mcp/message",
            json!({"serverId":"tools","requestId":"inner","method":"tools/list"}),
        ),
    ];
    for (index, (method, params)) in methods.into_iter().enumerate() {
        peer.send(json!({"jsonrpc":"2.0","id":index,"method":method,"params":params}))
            .await;
        let response = peer.receive().await;
        assert!(response.get("error").is_none(), "{method}: {response}");
        assert_eq!(response["id"], index);
        let (request, context) = received.recv().await.unwrap();
        assert_eq!(context.method, method);
        if let ClientRequest::ReadTextFile(actual) = request {
            assert_eq!(actual, params);
        }
        if matches!(method, "session/elicitation" | "_session/elicitation") {
            assert_eq!(response["result"]["action"]["action"], "accept");
            assert_eq!(response["result"]["_meta"], json!({"answer":true}));
        }
    }
    peer.send(json!({"jsonrpc":"2.0","id":"bad","method":"fs/read_text_file","params":{"private":"secret"}})).await;
    let error = peer.receive().await;
    assert_eq!(error["error"]["code"], -32602);
    assert!(!error.to_string().contains("secret"));
    assert!(received.try_recv().is_err());
    client.shutdown().await;
}

#[tokio::test]
async fn writer_failure_fails_pending_and_future_calls_and_notifications() {
    let (incoming, _agent_writer) = tokio::io::duplex(4096);
    let (outgoing, agent_reader) = tokio::io::duplex(4096);
    drop(agent_reader);
    let (client, mut events) = Client::connect(
        incoming,
        outgoing,
        Arc::new(RejectRequests),
        ClientOptions::default(),
    );
    let error = client.request("_first", json!({})).await.unwrap_err();
    assert!(matches!(
        error,
        Error::Io {
            operation: "write",
            ..
        }
    ));
    assert_eq!(
        client.request("_later", json!({})).await.unwrap_err(),
        error
    );
    assert_eq!(client.notify("_later", json!({})).await.unwrap_err(), error);
    assert_eq!(events.next().await, Some(Err(error)));
    client.shutdown().await;
}

#[tokio::test]
async fn raw_stream_is_bounded_replayable_and_ends_with_connection() {
    let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
    for n in 0..40 {
        peer.send(json!({"jsonrpc":"2.0","method":"_event","params":{"n":n}}))
            .await;
    }
    for _ in 0..40 {
        events.next().await.unwrap().unwrap();
    }
    let mut raw = client.raw_notifications();
    client.shutdown().await;
    for n in 8..40 {
        assert_eq!(
            raw.next().await,
            Some(Event::Notification {
                method: "_event".into(),
                params: json!({"n":n})
            })
        );
    }
    assert!(raw.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn load_without_replay_times_out_without_poisoning_other_requests() {
    let (client, _events, mut peer) = connect(Arc::new(RejectRequests));
    let c = client.clone();
    let load = tokio::spawn(async move {
        c.load_session(json!({"sessionId":"s","cwd":"/w","mcpServers":[]}))
            .await
    });
    let old = peer.receive().await;
    assert_eq!(
        load.await.unwrap().unwrap_err(),
        Error::Timeout("session load")
    );
    peer.reply(&old, json!({})).await;
    let c = client.clone();
    let echo = tokio::spawn(async move { c.request("_echo", json!({})).await });
    let request = peer.receive().await;
    peer.reply(&request, json!({"ok":true})).await;
    assert_eq!(echo.await.unwrap().unwrap(), json!({"ok":true}));
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn cancellation_timeout_keeps_the_prompt_registered_until_settled() {
    let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
    let c = client.clone();
    let prompt = tokio::spawn(async move { c.prompt(json!({"sessionId":"s","prompt":[]})).await });
    let request = peer.receive().await;
    let c = client.clone();
    let cancel = tokio::spawn(async move { c.cancel_and_wait(json!({"sessionId":"s"})).await });
    assert_eq!(peer.receive().await["method"], "session/cancel");
    assert_eq!(
        cancel.await.unwrap().unwrap_err(),
        Error::Timeout("cancellation")
    );
    assert!(matches!(
        client.prompt(json!({"sessionId":"s","prompt":[]})).await,
        Err(Error::PromptActive(_))
    ));
    peer.reply(&request, json!({"stopReason":"cancelled"}))
        .await;
    peer.update(
        "s",
        json!({"sessionUpdate":"state_update","state":"idle","stopReason":"cancelled"}),
    )
    .await;
    events.next().await.unwrap().unwrap();
    events.acknowledge();
    prompt.await.unwrap().unwrap();
    client.shutdown().await;
}

#[tokio::test]
async fn aborted_request_is_cleaned_without_an_interrupt_rpc_and_late_reply_is_ignored() {
    let (client, _events, mut peer) = connect(Arc::new(RejectRequests));
    let c = client.clone();
    let task = tokio::spawn(async move { c.request("_slow", json!({})).await });
    let old = peer.receive().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    peer.reply(&old, json!({"late":true})).await;
    let c = client.clone();
    let next = tokio::spawn(async move { c.request("_next", json!({})).await });
    let frame = peer.receive().await;
    assert_eq!(frame["method"], "_next");
    peer.reply(&frame, json!({"ok":true})).await;
    assert_eq!(next.await.unwrap().unwrap(), json!({"ok":true}));
    client.shutdown().await;
}

#[tokio::test]
async fn auth_options_modes_models_and_generation_guards_match_wire() {
    for v2 in [false, true] {
        let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
        initialize(&client, &mut peer, v2).await;
        if v2 {
            assert!(matches!(
                client.set_model(json!({})).await,
                Err(Error::Rpc(RpcError { code: -32601, .. }))
            ));
            assert!(matches!(
                client.set_mode(json!({})).await,
                Err(Error::Rpc(RpcError { code: -32601, .. }))
            ));
        } else {
            assert!(matches!(
                client.list_providers(json!({})).await,
                Err(Error::Rpc(RpcError { code: -32601, .. }))
            ));
        }
        assert!(matches!(
            client.delete_session(json!({"sessionId":"s"})).await,
            Err(Error::Rpc(RpcError { code: -32601, .. }))
        ));
        let c = client.clone();
        let auth = tokio::spawn(async move { c.authenticate(json!({"methodId":"login"})).await });
        let frame = peer.receive().await;
        assert_eq!(
            frame["method"],
            if v2 { "auth/login" } else { "authenticate" }
        );
        peer.reply(&frame, json!({})).await;
        auth.await.unwrap().unwrap();
        if !v2 {
            for (model, expected, params) in [
                (
                    true,
                    "session/set_model",
                    json!({"sessionId":"s","modelId":"model"}),
                ),
                (
                    false,
                    "session/set_mode",
                    json!({"sessionId":"s","modeId":"plan"}),
                ),
            ] {
                let c = client.clone();
                let task = tokio::spawn(async move {
                    if model {
                        c.set_model(params).await
                    } else {
                        c.set_mode(params).await
                    }
                });
                let frame = peer.receive().await;
                assert_eq!(frame["method"], expected);
                peer.reply(&frame, json!({})).await;
                task.await.unwrap().unwrap();
            }
        }
        let c = client.clone();
        let config = tokio::spawn(async move {
            c.set_config_option(json!({"sessionId":"s","configId":"auto","value":true}))
                .await
        });
        let frame = peer.receive().await;
        if v2 {
            assert_eq!(frame["params"]["type"], "boolean");
        }
        peer.reply(&frame, json!({"configOptions":[]})).await;
        config.await.unwrap().unwrap();
        peer.update("s",json!({"sessionUpdate":"available_commands_update","availableCommands":[{"name":"plan","description":"Plan","input":if v2{json!({"type":"text","hint":"Task"})}else{json!({"hint":"Task"})}}]})).await;
        let Event::SessionUpdate(update) = events.next().await.unwrap().unwrap() else {
            panic!()
        };
        assert_eq!(
            update.update["availableCommands"][0]["input"]["type"],
            "text"
        );
        client.shutdown().await;
    }
}

#[tokio::test]
async fn draft_auth_keeps_registry_env_var_methods_and_never_invents_agent_login() {
    let (client, _events, mut peer) = connect(Arc::new(RejectRequests));
    let c = client.clone();
    let task = tokio::spawn(async move { c.initialize(json!({})).await });
    let frame = peer.receive().await;
    peer.reply(&frame,json!({"protocolVersion":2,"info":{"name":"a","version":"1"},"authMethods":[{"methodId":"future","type":"future_auth","name":"Future"},{"methodId":"api_key","type":"env_var","name":"API key","vars":[{"name":"KEY","label":"Key"}],"link":"https://example.test/keys"}]})).await;
    let n = task.await.unwrap().unwrap();
    assert_eq!(n.raw["authMethods"].as_array().unwrap().len(), 2);
    assert_eq!(n.response["authMethods"].as_array().unwrap().len(), 1);
    assert_eq!(n.response["authMethods"][0]["id"], "api_key");
    assert_eq!(n.response["authMethods"][0]["vars"][0]["name"], "KEY");
    client.shutdown().await;
}

#[tokio::test]
async fn malformed_known_notifications_are_skipped_without_payload_disclosure() {
    let (client, mut events, mut peer) = connect(Arc::new(RejectRequests));
    for update in [
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"private"}}),
        json!({"sessionUpdate":"available_commands_update","availableCommands":[123]}),
        json!({"sessionUpdate":"usage_update","cost":{"currency":"private"}}),
        json!({"sessionUpdate":"current_mode_update","currentModeId":123}),
    ] {
        peer.update("s", update).await;
    }
    peer.update(
        "s",
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"okay"}}),
    )
    .await;
    for _ in 0..4 {
        assert_eq!(
            events.next().await.unwrap().unwrap(),
            Event::InvalidNotification {
                method: "session/update".into()
            }
        );
    }
    let Event::SessionUpdate(update) = events.next().await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(update.update["content"]["text"], "okay");
    client.shutdown().await;
}
