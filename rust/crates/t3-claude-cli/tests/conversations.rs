#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};
use t3_claude_cli::{
    Error, Event, McpReply, McpServer, PendingRequest, Session, SessionOptions,
    control::*,
    message::{Content, Message},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio_util::sync::CancellationToken;

struct Peer {
    read: BufReader<ReadHalf<DuplexStream>>,
    write: WriteHalf<DuplexStream>,
}
fn harness(options: SessionOptions) -> (Session, Peer) {
    let (host, peer) = tokio::io::duplex(256 * 1024);
    let (read, write) = tokio::io::split(host);
    let (peer_read, peer_write) = tokio::io::split(peer);
    (
        Session::connect(read, write, options),
        Peer {
            read: BufReader::new(peer_read),
            write: peer_write,
        },
    )
}
impl Peer {
    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.write.write_all(&bytes).await.unwrap();
    }
    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        assert!(
            self.read.read_line(&mut line).await.unwrap() > 0,
            "unexpected host EOF"
        );
        serde_json::from_str(&line).unwrap()
    }
    async fn success(&mut self, request: &Value, payload: Value) {
        self.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":payload}})).await;
    }
    async fn callback(&mut self, id: &str, request: Value) {
        self.send(json!({"type":"control_request","request_id":id,"request":request}))
            .await;
    }
}
async fn pending(session: &mut Session) -> PendingRequest {
    loop {
        match session.next_event().await.unwrap().unwrap() {
            Event::Request(request) => return *request,
            Event::Diagnostic { .. } | Event::Message(_) => continue,
            _ => panic!("expected callback"),
        }
    }
}

#[tokio::test]
async fn every_outbound_control_has_exact_shape_and_correlated_response() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    let cases = vec![
        (
            OutboundRequest::Initialize(Box::new(Initialize {
                title: Some("thread".into()),
                prompt_suggestions: Some(false),
                ..Default::default()
            })),
            json!({"subtype":"initialize","title":"thread","promptSuggestions":false}),
        ),
        (
            OutboundRequest::McpSetServers {
                servers: BTreeMap::from([(
                    "t3-code".into(),
                    json!({"type":"http","url":"https://test","headers":{"Authorization":"private"}}),
                )]),
            },
            json!({"subtype":"mcp_set_servers","servers":{"t3-code":{"type":"http","url":"https://test","headers":{"Authorization":"private"}}}}),
        ),
        (
            OutboundRequest::SetModel {
                model: "sonnet".into(),
            },
            json!({"subtype":"set_model","model":"sonnet"}),
        ),
        (
            OutboundRequest::ApplyFlagSettings {
                settings: FlagSettings {
                    effort_level: Some("high".into()),
                    fast_mode: None,
                    ultracode: Some(false),
                },
            },
            json!({"subtype":"apply_flag_settings","settings":{"effortLevel":"high","fastMode":null,"ultracode":false}}),
        ),
        (
            OutboundRequest::SetPermissionMode {
                mode: PermissionMode::DontAsk,
            },
            json!({"subtype":"set_permission_mode","mode":"dontAsk"}),
        ),
        (
            OutboundRequest::StopTask {
                task_id: "task".into(),
            },
            json!({"subtype":"stop_task","task_id":"task"}),
        ),
        (
            OutboundRequest::BackgroundTasks {
                tool_use_id: "tool".into(),
            },
            json!({"subtype":"background_tasks","tool_use_id":"tool"}),
        ),
        (OutboundRequest::McpStatus, json!({"subtype":"mcp_status"})),
        (
            OutboundRequest::McpReadResource {
                server_name: "server".into(),
                uri: "ui://test".into(),
            },
            json!({"subtype":"mcp_read_resource","serverName":"server","uri":"ui://test"}),
        ),
        (
            OutboundRequest::McpCall {
                tool: "mcp__s__tool".into(),
                arguments: Default::default(),
                result_not_for_model: true,
            },
            json!({"subtype":"mcp_call","tool":"mcp__s__tool","arguments":{},"result_not_for_model":true}),
        ),
        (
            OutboundRequest::ListPermissionRules,
            json!({"subtype":"list_permission_rules"}),
        ),
        (OutboundRequest::Interrupt, json!({"subtype":"interrupt"})),
        (
            OutboundRequest::GetUsage {
                skip_behaviors: true,
            },
            json!({"subtype":"get_usage","skip_behaviors":true}),
        ),
        (
            OutboundRequest::GetSettings,
            json!({"subtype":"get_settings"}),
        ),
        (
            OutboundRequest::McpMessage {
                server_name: "local".into(),
                message: json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"}),
            },
            json!({"subtype":"mcp_message","server_name":"local","message":{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}}),
        ),
    ];
    for (request, expected) in cases {
        let host = client.request(request);
        let script = async {
            let frame = peer.recv().await;
            assert_eq!(frame["type"], "control_request");
            assert_eq!(frame["request"], expected);
            assert!(!frame["request_id"].as_str().unwrap().is_empty());
            peer.success(&frame, json!({"marker":expected["subtype"]}))
                .await;
        };
        let (reply, ()) = tokio::join!(host, script);
        assert_eq!(
            reply.unwrap().payload.unwrap()["marker"],
            expected["subtype"]
        );
    }
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn controls_remain_concurrent_while_a_permission_waits_and_correlate_in_reverse_order() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    peer.callback("permission",json!({"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"native-tool","input":{"command":"pwd"},"future":null})).await;
    let PendingRequest::ToolPermission { request, responder } = pending(&mut session).await else {
        panic!()
    };
    assert_eq!(request.get().input["command"], "pwd");
    assert!(request.raw().get("future").is_some());
    let a = client.request(OutboundRequest::SetModel {
        model: "haiku".into(),
    });
    let b = client.request(OutboundRequest::McpStatus);
    let script = async {
        let first = peer.recv().await;
        let second = peer.recv().await;
        assert_ne!(first["request_id"], second["request_id"]);
        peer.success(&second, json!({"which":second["request"]["subtype"]}))
            .await;
        peer.send(json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working"}]}})).await;
        peer.success(&first, json!({"which":first["request"]["subtype"]}))
            .await;
    };
    let (a, b, ()) = tokio::join!(a, b, script);
    assert_eq!(a.unwrap().payload.unwrap()["which"], "set_model");
    assert_eq!(b.unwrap().payload.unwrap()["which"], "mcp_status");
    assert!(matches!(
        session.next_event().await.unwrap().unwrap(),
        Event::Message(_)
    ));
    responder
        .respond(PermissionDecision::Allow {
            updated_input: request.get().input.clone(),
            updated_permissions: Some(vec![PermissionUpdate::AddRules {
                rules: vec![PermissionRule {
                    tool_name: "Bash".into(),
                    rule_content: None,
                }],
                behavior: RuleBehavior::Allow,
                destination: "session".into(),
            }]),
            decision_classification: Some("user_permanent".into()),
        })
        .await
        .unwrap();
    let reply = peer.recv().await;
    assert_eq!(reply["response"]["request_id"], "permission");
    assert_eq!(
        reply["response"]["response"],
        json!({"behavior":"allow","updatedInput":{"command":"pwd"},"updatedPermissions":[{"type":"addRules","rules":[{"toolName":"Bash"}],"behavior":"allow","destination":"session"}],"decisionClassification":"user_permanent","toolUseID":"native-tool"})
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn hook_elicitation_dialog_and_manual_mcp_callbacks_answer_with_their_wire_payloads() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    session.client().register_pending_mcp_server("manual");
    peer.callback("hook",json!({"subtype":"hook_callback","callback_id":"hook_0","input":{"hook_event_name":"SubagentStart"}})).await;
    let PendingRequest::Hook { request, responder } = pending(&mut session).await else {
        panic!()
    };
    assert_eq!(request.get().callback_id, "hook_0");
    responder
        .respond(HookOutput {
            hook_specific_output: Some(
                json!({"hookEventName":"SubagentStart","additionalContext":"device instructions"}),
            ),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        peer.recv().await["response"]["response"],
        json!({"hookSpecificOutput":{"hookEventName":"SubagentStart","additionalContext":"device instructions"}})
    );
    peer.callback("form",json!({"subtype":"elicitation","mcp_server_name":"server","message":"Consent?","mode":"form","requested_schema":{"type":"object","properties":{}}})).await;
    let PendingRequest::Elicitation { request, responder } = pending(&mut session).await else {
        panic!()
    };
    assert_eq!(request.get().mode.as_deref(), Some("form"));
    responder
        .respond(ElicitationResponse::Accept {
            content: Some(BTreeMap::new()),
        })
        .await
        .unwrap();
    assert_eq!(
        peer.recv().await["response"]["response"],
        json!({"action":"accept","content":{}})
    );
    peer.callback("dialog",json!({"subtype":"request_user_dialog","dialog_kind":"resume_return","payload":{"sessionAgeMinutes":10,"estimatedTokens":2000}})).await;
    let PendingRequest::UserDialog { request, responder } = pending(&mut session).await else {
        panic!()
    };
    assert_eq!(request.get().payload["estimatedTokens"], 2000);
    responder
        .respond(DialogResponse::Completed {
            result: ResumeChoice::Continue,
        })
        .await
        .unwrap();
    assert_eq!(
        peer.recv().await["response"]["response"],
        json!({"behavior":"completed","result":"continue"})
    );
    peer.callback("mcp",json!({"subtype":"mcp_message","server_name":"manual","message":{"jsonrpc":"2.0","id":"nested","method":"tools/list"}})).await;
    let PendingRequest::Mcp { request, responder } = pending(&mut session).await else {
        panic!()
    };
    responder.respond(McpReply {mcp_response:json!({"jsonrpc":"2.0","id":request.get().message["id"],"result":{"tools":[]}})}).await.unwrap();
    assert_eq!(
        peer.recv().await["response"]["response"],
        json!({"mcp_response":{"jsonrpc":"2.0","id":"nested","result":{"tools":[]}}})
    );
    session.shutdown().await.unwrap();
}

struct EchoServer {
    name: &'static str,
    deliveries: std::sync::Mutex<Vec<Value>>,
}
impl EchoServer {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            deliveries: Default::default(),
        }
    }
}
impl McpServer for EchoServer {
    fn notify(&self, message: Value, _cancellation: CancellationToken) -> Result<(), Error> {
        assert!(
            !(message.get("method").is_some() && message.get("id").is_some_and(|id| !id.is_null()))
        );
        self.deliveries.lock().unwrap().push(message);
        Ok(())
    }
    fn handle<'a>(
        &'a self,
        message: Value,
        _cancellation: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Error>> + Send + 'a>> {
        Box::pin(async move {
            assert!(
                message.get("method").is_some()
                    && message.get("id").is_some_and(|id| !id.is_null()),
                "one-way MCP delivery must not enter the request handler"
            );
            Ok(
                json!({"jsonrpc":"2.0","id":message["id"],"result":{"server":self.name,"method":message["method"]}}),
            )
        })
    }
}
#[tokio::test]
async fn registered_mcp_servers_route_identical_ids_by_server_and_ack_notifications() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    client.register_mcp_server("a", Arc::new(EchoServer::new("a")));
    client.register_mcp_server("b", Arc::new(EchoServer::new("b")));
    for (id, server, method) in [("one", "a", "initialize"), ("two", "b", "tools/list")] {
        peer.callback(id,json!({"subtype":"mcp_message","server_name":server,"message":{"jsonrpc":"2.0","id":12,"method":method}})).await;
    }
    let mut replies = BTreeMap::new();
    for _ in 0..2 {
        let reply = peer.recv().await;
        replies.insert(
            reply["response"]["request_id"].as_str().unwrap().to_owned(),
            reply["response"]["response"]["mcp_response"].clone(),
        );
    }
    assert_eq!(replies["one"]["result"]["server"], "a");
    assert_eq!(replies["two"]["result"]["server"], "b");
    for (id, server) in [("one", "a"), ("two", "b")] {
        peer.callback(id,json!({"subtype":"mcp_message","server_name":server,"message":{"jsonrpc":"2.0","method":"notifications/initialized"}})).await;
        assert_eq!(
            peer.recv().await["response"]["response"],
            json!({"mcp_response":{"jsonrpc":"2.0","result":{},"id":0}})
        );
    }
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn callback_cancellation_suppresses_late_answers_and_duplicate_inflight_ids() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let request =
        json!({"subtype":"can_use_tool","tool_name":"Write","tool_use_id":"tool","input":{}});
    peer.callback("p", request.clone()).await;
    peer.callback("p", request).await;
    let PendingRequest::ToolPermission { responder, .. } = pending(&mut session).await else {
        panic!()
    };
    let cancellation = responder.cancellation();
    peer.send(json!({"type":"control_cancel_request","request_id":"p"}))
        .await;
    cancellation.cancelled().await;
    assert!(
        matches!(session.next_event().await.unwrap().unwrap(),Event::Cancelled{request_id} if request_id=="p")
    );
    assert_eq!(
        responder
            .respond(PermissionDecision::Allow {
                updated_input: Default::default(),
                updated_permissions: None,
                decision_classification: None
            })
            .await,
        Err(Error::Cancelled)
    );
    peer.callback("next", json!({"subtype":"future_callback"}))
        .await;
    let response = peer.recv().await;
    assert_eq!(response["response"]["request_id"], "next");
    assert_eq!(response["response"]["subtype"], "error");
    assert!(
        response["response"]["error"]
            .as_str()
            .unwrap()
            .contains("unsupported")
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropped_outbound_future_sends_cancel_and_eof_rejects_all_pending_controls() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    let mut request = Box::pin(client.request(OutboundRequest::GetSettings));
    let sent = tokio::select! { sent=peer.recv()=>sent, result=&mut request=>panic!("early result {result:?}") };
    drop(request);
    assert_eq!(
        peer.recv().await,
        json!({"type":"control_cancel_request","request_id":sent["request_id"]})
    );
    let host = client.request(OutboundRequest::GetUsage {
        skip_behaviors: true,
    });
    let script = async {
        let _ = peer.recv().await;
        peer.write.shutdown().await.unwrap();
    };
    let (reply, ()) = tokio::join!(host, script);
    assert!(matches!(reply, Err(Error::Closed)));
    assert!(session.next_event().await.is_none());
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn cli_errors_missing_payload_malformed_callbacks_and_dropped_responders_do_not_hang() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    let host = client.request(OutboundRequest::SetModel {
        model: "absent".into(),
    });
    let script = async {
        let request = peer.recv().await;
        peer.send(json!({"type":"control_response","response":{"subtype":"error","request_id":request["request_id"],"error":"unknown model","error_code":"unknown_model"}})).await;
    };
    let (reply, ()) = tokio::join!(host, script);
    assert!(matches!(reply,Err(Error::Cli{message,..}) if message=="unknown model"));
    let host = client.interrupt();
    let script = async {
        let request = peer.recv().await;
        peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"]}})).await;
    };
    let (reply, ()) = tokio::join!(host, script);
    assert!(reply.unwrap().payload.is_none());
    peer.callback("malformed", json!({"subtype":"can_use_tool","input":{}}))
        .await;
    assert_eq!(peer.recv().await["response"]["subtype"], "error");
    peer.callback(
        "forgotten",
        json!({"subtype":"hook_callback","callback_id":"missing","input":{}}),
    )
    .await;
    drop(pending(&mut session).await);
    let response = peer.recv().await;
    assert_eq!(response["response"]["request_id"], "forgotten");
    assert_eq!(response["response"]["subtype"], "error");
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn initialize_redelivers_pending_callbacks_and_records_actual_cli_version() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    let host = client.initialize(Initialize::default());
    let script = async {
        let request = peer.recv().await;
        peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{"commands":[],"models":[],"claude_code_version":"2.9.future"},"pending_permission_requests":[{"request_id":"p","request":{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":"t","input":{}}}],"pending_user_dialog_requests":[{"request_id":"d","request":{"subtype":"request_user_dialog","dialog_kind":"resume_return","payload":{}}}],"pending_elicitation_requests":[{"request_id":"e","request":{"subtype":"elicitation","mcp_server_name":"s","message":"ok?"}}]}})).await;
    };
    let (result, ()) = tokio::join!(host, script);
    assert_eq!(
        result.unwrap().get().claude_code_version.as_deref(),
        Some("2.9.future")
    );
    assert_eq!(client.cli_version().as_deref(), Some("2.9.future"));
    for id in ["p", "d"] {
        drop(pending(&mut session).await);
        let response = peer.recv().await;
        assert_eq!(response["response"]["request_id"], id);
        assert_eq!(response["response"]["subtype"], "error");
    }
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn streaming_input_accepts_steers_and_media_and_keeps_reading_after_a_result() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    let blocks = vec![
        image_block("image/png", b"png").unwrap(),
        pdf_block(b"pdf").unwrap(),
        serde_json::from_value(json!({"type":"text","text":"/skill do it"})).unwrap(),
    ];
    let mut message = UserMessage::new(Content::Blocks(blocks));
    message.priority = Some(Priority::Now);
    message.uuid = Some("caller-message-id".into());
    let uuid = message.uuid.clone();
    client.send_user(message).await.unwrap();
    let frame = peer.recv().await;
    assert_eq!(frame["uuid"], uuid.as_deref().unwrap());
    assert_eq!(frame["priority"], "now");
    assert_eq!(frame["parent_tool_use_id"], Value::Null);
    assert_eq!(
        frame["message"]["content"][0],
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"cG5n"}})
    );
    assert_eq!(
        frame["message"]["content"][1],
        json!({"type":"document","source":{"type":"base64","media_type":"application/pdf","data":"cGRm"}})
    );
    for _ in 0..2 {
        peer.send(json!({"type":"result","subtype":"success","is_error":false}))
            .await;
        assert!(
            matches!(session.next_event().await.unwrap().unwrap(),Event::Message(message) if matches!(*message,Message::Result(_)))
        );
        client
            .send_user(UserMessage::new(Content::Text("next".into())))
            .await
            .unwrap();
        assert_eq!(peer.recv().await["type"], "user");
    }
    assert!(UserMessage::new(Content::Text("x".into())).uuid.is_none());
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn framing_handles_split_utf8_crlf_diagnostics_and_final_unterminated_line() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let bytes =
        "diagnostic\r\n\r\n{\"type\":\"future\",\"text\":\"café 🦀\"}\r\n{\"type\":\"keep_alive\"}"
            .as_bytes();
    for byte in bytes {
        peer.write.write_all(&[*byte]).await.unwrap();
    }
    peer.write.shutdown().await.unwrap();
    assert!(matches!(
        session.next_event().await.unwrap().unwrap(),
        Event::NonJson { .. }
    ));
    assert!(matches!(
        session.next_event().await.unwrap().unwrap(),
        Event::Diagnostic { .. }
    ));
    let Event::Message(message) = session.next_event().await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(message.raw()["text"], "café 🦀");
    assert!(
        matches!(session.next_event().await.unwrap().unwrap(),Event::Message(message) if matches!(*message,Message::KeepAlive(_)))
    );
    assert!(session.next_event().await.is_none());
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversized_lines_fail_but_malformed_known_frames_are_nonfatal() {
    let (mut session, mut peer) = harness(SessionOptions {
        max_line_bytes: 32,
        ..Default::default()
    });
    peer.write
        .write_all(b"12345678901234567890123456789012345")
        .await
        .unwrap();
    assert!(matches!(
        session.next_event().await.unwrap(),
        Err(Error::Protocol(_))
    ));
    session.shutdown().await.unwrap();
    let (mut session, mut peer) = harness(SessionOptions::default());
    peer.send(json!({"type":"assistant"})).await;
    assert!(matches!(
        session.next_event().await.unwrap().unwrap(),
        Event::Diagnostic { .. }
    ));
    assert!(
        matches!(session.next_event().await.unwrap().unwrap(), Event::Message(message) if matches!(*message, Message::Unknown(_)))
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_drop_cancels_callbacks_and_finishes_outbound_waiters() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    peer.callback(
        "hook",
        json!({"subtype":"hook_callback","callback_id":"h","input":{}}),
    )
    .await;
    let PendingRequest::Hook { responder, .. } = pending(&mut session).await else {
        panic!()
    };
    let cancellation = responder.cancellation();
    let request = client.request(OutboundRequest::McpStatus);
    let script = async {
        let _ = peer.recv().await;
        drop(session);
        cancellation.cancelled().await;
    };
    let (result, ()) = tokio::join!(request, script);
    assert_eq!(result.unwrap_err(), Error::Closed);
    assert!(responder.is_cancelled());
}

#[tokio::test]
async fn incoming_callback_cancellation_does_not_cancel_an_outbound_request_with_the_same_id() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    let host = client.request(OutboundRequest::GetSettings);
    let script = async {
        let request = peer.recv().await;
        let id = request["request_id"].as_str().unwrap();
        peer.callback(
            id,
            json!({"subtype":"hook_callback","callback_id":"hook","input":{}}),
        )
        .await;
        peer.send(json!({"type":"control_cancel_request","request_id":id}))
            .await;
        peer.success(&request, json!({"effective":{"effortLevel":"high"}}))
            .await;
    };
    let (result, ()) = tokio::join!(host, script);
    assert_eq!(
        result
            .unwrap()
            .decode::<SettingsResult>()
            .unwrap()
            .effective
            .unwrap()["effortLevel"],
        "high"
    );
    let PendingRequest::Hook { responder, .. } = pending(&mut session).await else {
        panic!()
    };
    assert!(responder.is_cancelled());
    assert!(matches!(
        session.next_event().await.unwrap().unwrap(),
        Event::Cancelled { .. }
    ));
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn unknown_mcp_server_is_rejected_and_input_half_close_keeps_stdout_alive() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    peer.callback("mcp", json!({"subtype":"mcp_message","server_name":"missing","message":{"jsonrpc":"2.0","id":1,"method":"initialize"}})).await;
    assert_eq!(peer.recv().await["response"]["subtype"], "error");
    assert!(matches!(
        session.next_event().await.unwrap().unwrap(),
        Event::Diagnostic { .. }
    ));
    assert!(
        matches!(session.next_event().await.unwrap().unwrap(), Event::Message(message) if matches!(*message, Message::Unknown(_)))
    );
    session.client().end_input().await.unwrap();
    let mut line = String::new();
    assert_eq!(peer.read.read_line(&mut line).await.unwrap(), 0);
    peer.send(json!({"type":"system","subtype":"init","session_id":"s","model":"m","claude_code_version":"2.1.test"})).await;
    assert!(matches!(
        session.next_event().await.unwrap().unwrap(),
        Event::Message(_)
    ));
    assert_eq!(session.client().cli_version().as_deref(), Some("2.1.test"));
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn javascript_strings_survive_in_results_controls_and_permissions() {
    // JSON.stringify("🦀".slice(0, 1)) and invalid bytes decoded by Node's UTF-8 reader.
    for text in [
        br"\ud83e".as_slice(),
        br"\udd80".as_slice(),
        b"\xff".as_slice(),
        b"\xf0\x9f".as_slice(),
    ] {
        let (mut session, mut peer) = harness(SessionOptions::default());
        let mut bytes =
            br#"{"type":"result","subtype":"success","is_error":false,"result":""#.to_vec();
        bytes.extend_from_slice(text);
        bytes.extend_from_slice(b"\"}\n");
        peer.write.write_all(&bytes).await.unwrap();
        let Event::Diagnostic {
            original_bytes,
            raw,
            ..
        } = session.next_event().await.unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(original_bytes.as_deref(), Some(bytes.as_slice()));
        assert_eq!(raw["result"], "�");
        let Event::Message(message) = session.next_event().await.unwrap().unwrap() else {
            panic!()
        };
        let Message::Result(result) = *message else {
            panic!("result vanished")
        };
        assert_eq!(result.get().result.as_deref(), Some("�"));

        let client = session.client();
        let script = async {
            let sent = peer.recv().await;
            let prefix = format!(
                r#"{{"type":"control_response","response":{{"subtype":"success","request_id":{},"response":{{"text":""#,
                sent["request_id"]
            );
            let mut bytes = prefix.into_bytes();
            bytes.extend_from_slice(text);
            bytes.extend_from_slice(b"\"}}}\n");
            peer.write.write_all(&bytes).await.unwrap();
            bytes
        };
        let (reply, bytes) = tokio::join!(client.interrupt(), script);
        assert_eq!(reply.unwrap().payload.unwrap()["text"], "�");
        let Event::Diagnostic { original_bytes, .. } = session.next_event().await.unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(original_bytes.as_deref(), Some(bytes.as_slice()));

        let mut bytes=br#"{"type":"control_request","request_id":"p","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"t","input":{"command":""#.to_vec();
        bytes.extend_from_slice(text);
        bytes.extend_from_slice(b"\"}}}\n");
        peer.write.write_all(&bytes).await.unwrap();
        let Event::Diagnostic { original_bytes, .. } = session.next_event().await.unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(original_bytes.as_deref(), Some(bytes.as_slice()));
        let PendingRequest::ToolPermission { request, responder } = pending(&mut session).await
        else {
            panic!("permission vanished")
        };
        assert_eq!(request.get().input["command"], "�");
        drop(responder);
        assert_eq!(peer.recv().await["response"]["subtype"], "error");
        session.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn tolerated_frames_do_not_close_the_transport_or_unrelated_controls() {
    for bad in [
        json!({"type":"assistant","message":"wrong"}),
        json!({"type":17,"content":"raw"}),
        json!({"type":"system","subtype":"init","model":42}),
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":7}}}),
    ] {
        let (mut session, mut peer) = harness(SessionOptions::default());
        let client = session.client();
        let script = async {
            let sent = peer.recv().await;
            peer.send(bad.clone()).await;
            peer.success(&sent, json!({"alive":true})).await;
            peer.write.shutdown().await.unwrap();
        };
        let (reply, ()) = tokio::join!(client.interrupt(), script);
        assert_eq!(reply.unwrap().payload.unwrap()["alive"], true);
        let mut found = false;
        while let Some(event) = session.next_event().await {
            if let Ok(Event::Message(message)) = event {
                assert_eq!(message.raw(), &bad);
                found = true;
            }
        }
        assert!(found);
        session.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn malformed_control_verdict_fails_only_its_request() {
    for verdict in [
        json!({"subtype":"future","error":"new"}),
        json!({"subtype":"error","error":42}),
    ] {
        let (mut session, mut peer) = harness(SessionOptions::default());
        let client = session.client();
        let script = async {
            let first = peer.recv().await;
            let second = peer.recv().await;
            let mut response = verdict.clone();
            response["request_id"] = first["request_id"].clone();
            peer.send(json!({"type":"control_response","response":response}))
                .await;
            peer.success(&second, json!({"alive":true})).await;
            peer.write.shutdown().await.unwrap();
        };
        let (first, second, ()) = tokio::join!(
            client.interrupt(),
            client.request(OutboundRequest::McpStatus),
            script
        );
        assert!(first.is_err());
        assert_eq!(second.unwrap().payload.unwrap()["alive"], true);
        session.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn future_suggestions_are_presented_without_granting_permission() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let suggestions = json!([
        {"type":"futureSuggestion","secret":null},
        {"type":"setMode","mode":"futureMode","destination":"session"},
        {"type":"addRules","rules":[],"behavior":"futureBehavior","destination":"session"}
    ]);
    peer.callback("p",json!({"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"t","input":{},"permission_suggestions":suggestions})).await;
    let Event::Diagnostic { raw, .. } = session.next_event().await.unwrap().unwrap() else {
        panic!("missing suggestion diagnostic")
    };
    assert_eq!(raw["request"]["permission_suggestions"], suggestions);
    let PendingRequest::ToolPermission { request, responder } = pending(&mut session).await else {
        panic!("unknown suggestions hid the prompt")
    };
    assert_eq!(
        serde_json::to_value(&request.get().permission_suggestions).unwrap(),
        suggestions
    );
    drop(responder);
    let response = peer.recv().await;
    assert_eq!(response["response"]["request_id"], "p");
    assert_eq!(response["response"]["subtype"], "error");
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn raw_permission_responses_preserve_unknown_suggestions_and_native_identity() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let updates = json!([
        {"type":"futureScopedRule","scope":{"command":"pwd"},"opaque":null,"destination":"session"},
        {"type":"setMode","mode":"futureRestrictedMode","destination":"session"}
    ]);
    assert!(serde_json::from_value::<PermissionUpdate>(updates[0].clone()).is_err());
    for (id, supplied_tool_id) in [("raw", "raw-tool"), ("raw-again", "unrelated-tool")] {
        peer.callback(id, json!({"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"raw-tool","input":{"command":"pwd"}})).await;
        let PendingRequest::ToolPermission { responder, .. } = pending(&mut session).await else {
            panic!("expected tool permission")
        };
        let payload = json!({
            "behavior":"allow", "updatedInput":{"command":"pwd"},
            "updatedPermissions":updates, "toolUseID":supplied_tool_id,
            "decisionClassification":"user_permanent", "futureResult":{"nullable":null,"enabled":false}
        });
        responder.respond_raw(payload.clone()).await.unwrap();
        let mut expected = payload;
        expected["toolUseID"] = json!("raw-tool");
        assert_eq!(
            peer.recv().await,
            json!({"type":"control_response","response":{
                "subtype":"success","request_id":id,"response":expected
            }})
        );
    }
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_responses_and_null_ids_are_one_way_deliveries() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let server = Arc::new(EchoServer::new("s"));
    session.client().register_mcp_server("s", server.clone());
    for message in [
        json!({"jsonrpc":"2.0","id":2,"result":{}}),
        json!({"jsonrpc":"2.0","method":"notification","id":null}),
    ] {
        peer.callback(
            "m",
            json!({"subtype":"mcp_message","server_name":"s","message":message.clone()}),
        )
        .await;
        assert_eq!(
            peer.recv().await["response"]["response"],
            json!({"mcp_response":{"jsonrpc":"2.0","result":{},"id":0}})
        );
        assert_eq!(server.deliveries.lock().unwrap().last(), Some(&message));
    }
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn initialize_filters_redelivery_and_survives_malformed_entries() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    let script = async {
        let sent = peer.recv().await;
        peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":sent["request_id"],"response":{},"pending_permission_requests":[null,{"request_id":"wrong","request":{"subtype":"hook_callback","callback_id":"h","input":{}}},{"request_id":"p","request":{"subtype":"can_use_tool","tool_name":"Read","tool_use_id":"t","input":{}}}]}})).await;
    };
    let (reply, ()) = tokio::join!(client.initialize(Initialize::default()), script);
    assert!(
        reply.is_ok(),
        "one malformed replay must not discard initialize"
    );
    // Skip diagnostics while waiting for the sole eligible callback.
    loop {
        if let Ok(Event::Request(request)) = session.next_event().await.unwrap() {
            let PendingRequest::ToolPermission { responder, .. } = *request else {
                panic!("wrong redelivery subtype dispatched")
            };
            drop(responder);
            break;
        }
    }
    let reply = peer.recv().await;
    assert_eq!(reply["response"]["request_id"], "p");
    assert_eq!(reply["response"]["subtype"], "error");
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn stdin_failure_keeps_unread_stdout_alive() {
    struct Broken;
    impl tokio::io::AsyncWrite for Broken {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
        fn poll_flush(
            self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    let (read, mut peer) = tokio::io::duplex(1024);
    let mut session = Session::connect(read, Broken, SessionOptions::default());
    assert!(session.client().interrupt().await.is_err());
    peer.write_all(
        b"{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"final\"}\n",
    )
    .await
    .unwrap();
    peer.shutdown().await.unwrap();
    let mut found = false;
    while let Some(event) = session.next_event().await {
        if let Ok(Event::Message(message)) = event {
            found |= matches!(*message, Message::Result(_));
        }
    }
    assert!(found, "writer failure discarded stdout");
    session.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_releases_a_writer_blocked_on_an_unread_pipe() {
    let (mut session, _peer) = harness(SessionOptions::default());
    let client = session.client();
    let mut sending =
        Box::pin(client.send_user(UserMessage::new(Content::Text("x".repeat(1024 * 1024)))));
    // Poll the producer until it is queued; the peer never reads its end.
    std::future::poll_fn(|cx| {
        assert!(sending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(60), session.shutdown())
        .await
        .expect("shutdown hung behind a blocked stdin write")
        .unwrap();
    assert!(sending.await.is_err());
}

#[tokio::test]
async fn remote_tool_controls_are_left_unanswered() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    for subtype in [
        "remote_tool_call",
        "remote_plumbing_call",
        "remote_tools_probe",
        "remote_tools_reannounce",
    ] {
        peer.callback(subtype, json!({"subtype":subtype})).await;
    }
    peer.callback("barrier", json!({"subtype":"unsupported"}))
        .await;
    let response = peer.recv().await;
    assert_eq!(response["response"]["request_id"], "barrier");
    assert_eq!(response["response"]["subtype"], "error");
    session.shutdown().await.unwrap();
}

fn allow() -> PermissionDecision {
    PermissionDecision::Allow {
        updated_input: Default::default(),
        updated_permissions: None,
        decision_classification: None,
    }
}
fn permission(tool: &str) -> Value {
    json!({"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":tool,"input":{"command":"pwd"}})
}
#[tokio::test]
async fn permissions_fail_closed_on_drop_malformed_unknown_and_shutdown() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    peer.callback("drop", permission("tool-drop")).await;
    drop(pending(&mut session).await);
    let reply = peer.recv().await;
    assert_eq!(reply["response"]["request_id"], "drop");
    assert_eq!(reply["response"]["subtype"], "error");
    for (id, request) in [
        (
            "malformed",
            json!({"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":42,"input":{}}),
        ),
        (
            "unknown",
            json!({"subtype":"future_permission","tool_name":"Bash","tool_use_id":"t","input":{}}),
        ),
        (
            "missing-subtype",
            json!({"tool_name":"Bash","tool_use_id":"t","input":{}}),
        ),
        ("missing-body", Value::Null),
    ] {
        peer.callback(id, request.clone()).await;
        let reply = peer.recv().await;
        assert_eq!(reply["response"]["request_id"], id);
        assert_eq!(reply["response"]["subtype"], "error");
        assert!(matches!(
            session.next_event().await.unwrap().unwrap(),
            Event::Diagnostic { .. }
        ));
        let Event::Message(message) = session.next_event().await.unwrap().unwrap() else {
            panic!()
        };
        assert!(matches!(*message, Message::Unknown(_)));
        assert_eq!(message.raw()["request"], request);
    }
    peer.callback("shutdown", permission("tool-shutdown")).await;
    let PendingRequest::ToolPermission { responder, .. } = pending(&mut session).await else {
        panic!()
    };
    let cancellation = responder.cancellation();
    session.shutdown().await.unwrap();
    assert!(cancellation.is_cancelled());
    assert_eq!(responder.respond(allow()).await, Err(Error::Cancelled));
    let mut remaining = String::new();
    assert_eq!(
        peer.read.read_line(&mut remaining).await.unwrap(),
        0,
        "shutdown sent a permission response without consent"
    );
}

#[tokio::test]
async fn concurrent_permissions_correlate_reverse_answers_and_ignore_duplicate_delivery() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    peer.callback("a", permission("tool-a")).await;
    peer.callback("a", permission("tool-evil-duplicate")).await;
    peer.callback("b", permission("tool-b")).await;
    let PendingRequest::ToolPermission {
        responder: first, ..
    } = pending(&mut session).await
    else {
        panic!()
    };
    let PendingRequest::ToolPermission {
        responder: second, ..
    } = pending(&mut session).await
    else {
        panic!()
    };
    assert_eq!(first.request_id(), "a");
    assert_eq!(second.request_id(), "b");
    second
        .respond(PermissionDecision::Deny {
            message: "no".into(),
            interrupt: None,
            decision_classification: None,
        })
        .await
        .unwrap();
    let reply = peer.recv().await;
    assert_eq!(reply["response"]["request_id"], "b");
    assert_eq!(reply["response"]["response"]["behavior"], "deny");
    assert_eq!(reply["response"]["response"]["toolUseID"], "tool-b");
    first.respond(allow()).await.unwrap();
    let reply = peer.recv().await;
    assert_eq!(reply["response"]["request_id"], "a");
    assert_eq!(reply["response"]["response"]["behavior"], "allow");
    assert_eq!(reply["response"]["response"]["toolUseID"], "tool-a");
    session.shutdown().await.unwrap();
    let mut remaining = String::new();
    assert_eq!(
        peer.read.read_line(&mut remaining).await.unwrap(),
        0,
        "duplicate prompt produced an extra reply"
    );
}

#[tokio::test]
async fn redelivery_needs_a_fresh_explicit_answer_and_deduplicates_inflight_permissions() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    peer.callback("p", permission("original-tool")).await;
    let PendingRequest::ToolPermission { responder, .. } = pending(&mut session).await else {
        panic!()
    };
    let client = session.client();
    let script = async {
        let sent = peer.recv().await;
        peer.send(json!({"type":"control_response","response":{"subtype":"success","request_id":sent["request_id"],"response":{},"pending_permission_requests":[{"request_id":"p","request":permission("redelivered-tool")}]}})).await;
        peer.send(json!({"type":"keep_alive"})).await;
    };
    let (reply, ()) = tokio::join!(client.initialize(Initialize::default()), script);
    reply.unwrap();
    assert!(
        matches!(session.next_event().await.unwrap().unwrap(),Event::Message(message) if matches!(*message, Message::KeepAlive(_)))
    );
    drop(responder);
    let reply = peer.recv().await;
    assert_eq!(reply["response"]["request_id"], "p");
    assert_eq!(reply["response"]["subtype"], "error");
    peer.callback("p", permission("fresh-tool")).await;
    let PendingRequest::ToolPermission { responder, .. } = pending(&mut session).await else {
        panic!()
    };
    drop(responder);
    assert_eq!(peer.recv().await["response"]["subtype"], "error");
    session.shutdown().await.unwrap();
    let mut remaining = String::new();
    assert_eq!(peer.read.read_line(&mut remaining).await.unwrap(), 0);
}

#[tokio::test]
async fn malformed_json_is_a_nonfatal_protocol_diagnostic_and_future_fields_stay_raw() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let bytes = b"  {\"type\":\"result\",\"result\":\"unterminated\n";
    peer.write.write_all(bytes).await.unwrap();
    let Event::Diagnostic {
        original_bytes,
        error,
        ..
    } = session.next_event().await.unwrap().unwrap()
    else {
        panic!()
    };
    assert!(matches!(error, Error::Decode { .. }));
    assert_eq!(original_bytes.as_deref(), Some(bytes.as_slice()));
    peer.send(json!({"type":"result","subtype":"success","is_error":false,"future_field":{"x":1}}))
        .await;
    let Event::Diagnostic { raw, .. } = session.next_event().await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(raw["future_field"], json!({"x":1}));
    assert!(
        matches!(session.next_event().await.unwrap().unwrap(),Event::Message(message) if matches!(*message,Message::Result(_)))
    );
    peer.callback("future-field",json!({"subtype":"can_use_tool","tool_name":"Read","tool_use_id":"t","input":{},"future_permission_mode":"future"})).await;
    let Event::Diagnostic { raw, .. } = session.next_event().await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(raw["request"]["future_permission_mode"], "future");
    drop(pending(&mut session).await);
    assert_eq!(peer.recv().await["response"]["subtype"], "error");
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn caller_cancellation_releases_shutdown_behind_a_blocked_write() {
    let (mut session, _peer) = harness(SessionOptions::default());
    let client = session.client();
    let mut sending =
        Box::pin(client.send_user(UserMessage::new(Content::Text("x".repeat(1024 * 1024)))));
    std::future::poll_fn(|cx| {
        assert!(sending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    session
        .shutdown_with_cancellation(cancellation)
        .await
        .unwrap();
    assert!(sending.await.is_err());
}

#[tokio::test]
async fn json_parse_overflow_numbers_remain_raw_without_dropping_a_result() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    peer.write.write_all("{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"future_number\":1e400,\"negative\":-1e400,\"exact\":12.5,\"note\":\"\u{e9} \\\"1e400\\\" -1e400\"}\n".as_bytes()).await.unwrap();
    peer.write.shutdown().await.unwrap();
    let mut found = false;
    while let Some(event) = session.next_event().await {
        if let Ok(Event::Message(message)) = event {
            assert!(matches!(*message, Message::Result(_)));
            let raw = message.raw();
            assert_eq!(raw["future_number"].as_f64(), Some(f64::MAX));
            assert_eq!(raw["negative"].as_f64(), Some(f64::MIN));
            assert_eq!(raw["exact"].as_f64(), Some(12.5));
            assert_eq!(raw["note"], "\u{e9} \"1e400\" -1e400");
            found = true;
        }
    }
    assert!(found, "JSON.parse accepts overflow numbers");
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_permission_cannot_answer_a_redelivery_with_the_same_id() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    peer.callback("p", permission("old-tool")).await;
    let PendingRequest::ToolPermission { responder: old, .. } = pending(&mut session).await else {
        panic!()
    };
    peer.send(json!({"type":"control_cancel_request","request_id":"p"}))
        .await;
    assert!(
        matches!(session.next_event().await.unwrap().unwrap(),Event::Cancelled{request_id} if request_id=="p")
    );
    peer.callback("p", permission("new-tool")).await;
    let PendingRequest::ToolPermission { responder: new, .. } = pending(&mut session).await else {
        panic!()
    };
    assert_eq!(old.respond(allow()).await, Err(Error::Cancelled));
    assert!(!new.is_cancelled());
    drop(new);
    let response = peer.recv().await;
    assert_eq!(response["response"]["request_id"], "p");
    assert_eq!(response["response"]["subtype"], "error");
    session.shutdown().await.unwrap();
    let mut remaining = String::new();
    assert_eq!(peer.read.read_line(&mut remaining).await.unwrap(), 0);
}

#[tokio::test]
async fn future_permission_modes_are_raw_with_a_nonfatal_diagnostic() {
    for subtype in ["init", "status"] {
        let (mut session, mut peer) = harness(SessionOptions::default());
        let raw = if subtype == "init" {
            json!({"type":"system","subtype":subtype,"session_id":"s","model":"sonnet","permissionMode":"futureMode"})
        } else {
            json!({"type":"system","subtype":subtype,"permissionMode":"futureMode"})
        };
        peer.send(raw.clone()).await;
        peer.write.shutdown().await.unwrap();
        let mut diagnosed = false;
        let mut surfaced = false;
        while let Some(event) = session.next_event().await {
            match event.unwrap() {
                Event::Diagnostic { raw, .. } => {
                    assert_eq!(raw["permissionMode"], "futureMode");
                    diagnosed = true;
                }
                Event::Message(message) => {
                    assert_eq!(message.raw(), &raw);
                    surfaced = true;
                }
                _ => panic!(),
            }
        }
        assert!(diagnosed, "unknown permission mode had no diagnostic");
        assert!(surfaced);
        session.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn stdin_failure_does_not_reject_a_previously_written_control() {
    struct FailAfterFirstFlush {
        inner: WriteHalf<DuplexStream>,
        flushed: bool,
    }
    impl tokio::io::AsyncWrite for FailAfterFirstFlush {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            if self.flushed {
                return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }
            Pin::new(&mut self.inner).poll_write(cx, bytes)
        }
        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let result = Pin::new(&mut self.inner).poll_flush(cx);
            if matches!(result, std::task::Poll::Ready(Ok(()))) {
                self.flushed = true;
            }
            result
        }
        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }
    let (host, remote) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(host);
    let (peer_read, peer_write) = tokio::io::split(remote);
    let mut session = Session::connect(
        read,
        FailAfterFirstFlush {
            inner: write,
            flushed: false,
        },
        SessionOptions::default(),
    );
    let client = session.client();
    let mut peer = Peer {
        read: BufReader::new(peer_read),
        write: peer_write,
    };
    let script = async {
        let sent = peer.recv().await;
        assert!(client.interrupt().await.is_err());
        peer.success(&sent, json!({"written_before_failure":true}))
            .await;
        peer.write.shutdown().await.unwrap();
    };
    let (first, ()) = tokio::join!(client.request(OutboundRequest::GetSettings), script);
    assert_eq!(
        first.unwrap().payload.unwrap()["written_before_failure"],
        true
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_initialize_payload_surfaces_raw_and_fails_only_initialize() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let client = session.client();
    let script = async {
        let sent = peer.recv().await;
        peer.success(&sent, json!({"commands":"future-shape"}))
            .await;
        peer.write.shutdown().await.unwrap();
    };
    let (reply, ()) = tokio::join!(client.initialize(Initialize::default()), script);
    assert!(reply.is_err());
    let mut diagnosed = false;
    while let Some(event) = session.next_event().await {
        if let Event::Diagnostic { raw, .. } = event.unwrap() {
            assert_eq!(raw["response"]["response"]["commands"], "future-shape");
            diagnosed = true;
        }
    }
    assert!(
        diagnosed,
        "initialize's typed decoder swallowed the raw payload"
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn future_control_envelope_fields_are_exposed_with_diagnostics() {
    for mut raw in [
        json!({"type":"control_cancel_request","request_id":"p"}),
        json!({"type":"control_response","response":{"subtype":"success","request_id":"p","response":{}}}),
        json!({"type":"control_request","request_id":"p","request":permission("t")}),
    ] {
        let (mut session, mut peer) = harness(SessionOptions::default());
        raw["future_envelope"] = json!({"content":"preserve"});
        peer.send(raw.clone()).await;
        let Event::Diagnostic { raw: seen, .. } = session.next_event().await.unwrap().unwrap()
        else {
            panic!("missing envelope diagnostic")
        };
        assert_eq!(seen.as_ref(), &raw);
        let event = session.next_event().await.unwrap().unwrap();
        if let Event::Request(request) = event {
            drop(request);
            assert_eq!(peer.recv().await["response"]["subtype"], "error");
        }
        session.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn json_parse_deep_values_do_not_discard_a_result() {
    let (mut session, mut peer) = harness(SessionOptions::default());
    let depth = 256;
    let line = format!(
        "{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"future_nested\":{}0{}}}\n",
        "[".repeat(depth),
        "]".repeat(depth)
    );
    peer.write.write_all(line.as_bytes()).await.unwrap();
    peer.write.shutdown().await.unwrap();
    let mut found = false;
    while let Some(event) = session.next_event().await {
        if let Ok(Event::Message(message)) = event {
            assert!(matches!(*message, Message::Result(_)));
            let mut value = &message.raw()["future_nested"];
            for _ in 0..depth {
                value = &value[0];
            }
            assert_eq!(value, 0);
            found = true;
        }
    }
    assert!(found, "JSON.parse accepts deeply nested values");
    session.shutdown().await.unwrap();
}
