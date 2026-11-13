#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use t3_acp::{
    Client, ClientHandler, ClientOptions, ClientRequest, Event, RequestContext, RpcError,
    async_trait,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct RecordedResponses(Mutex<VecDeque<(String, ClientRequest, Value)>>);
#[async_trait]
impl ClientHandler for RecordedResponses {
    async fn request(
        &self,
        actual: ClientRequest,
        context: RequestContext,
    ) -> Result<Value, RpcError> {
        let (method, expected, result) = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("unrecorded client request");
        assert_eq!(context.method, method);
        assert_eq!(actual, expected, "normalized handler parameters");
        assert!(
            matches!(context.id, t3_acp::RequestId::String(ref id) if id.starts_with("fixture-"))
        );
        Ok(result)
    }
}

#[tokio::test]
async fn replay_typescript_registry_transcripts_over_memory_pipes() {
    for (name, source) in [
        (
            "acp_elicitation",
            include_str!("../fixtures/acp_elicitation.ndjson"),
        ),
        (
            "acp_titled_multiselect",
            include_str!("../fixtures/acp_titled_multiselect.ndjson"),
        ),
        (
            "malformed_session_update",
            include_str!("../fixtures/malformed_session_update.ndjson"),
        ),
        ("simple", include_str!("../fixtures/simple.ndjson")),
        (
            "tool_call_read_only",
            include_str!("../fixtures/tool_call_read_only.ndjson"),
        ),
        ("todo_list", include_str!("../fixtures/todo_list.ndjson")),
        (
            "turn_interrupt",
            include_str!("../fixtures/turn_interrupt.ndjson"),
        ),
        ("multi_turn", include_str!("../fixtures/multi_turn.ndjson")),
    ] {
        let original = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").join(format!("apps/server/src/orchestration-v2/testkit/fixtures/{name}/registry_transcript.ndjson"))
        ).unwrap();
        assert_eq!(
            source.replace("\r\n", "\n"),
            original.replace("\r\n", "\n"),
            "fixture drift: {name}"
        );
        let rows: Vec<Value> = source
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let responses = Arc::new(RecordedResponses(Mutex::new(
            rows.iter()
                .filter(|row| {
                    row["type"] == "expect_outbound" && row["frame"]["kind"] == "response"
                })
                .map(|row| {
                    (
                        row["frame"]["method"].as_str().unwrap().into(),
                        {
                            let method = row["frame"]["method"].as_str().unwrap();
                            let params = rows
                                .iter()
                                .find(|candidate| {
                                    candidate["type"] == "emit_inbound"
                                        && candidate["frame"]["kind"] == "request"
                                        && candidate["frame"]["method"] == method
                                })
                                .unwrap()["frame"]["params"]
                                .clone();
                            assert_eq!(method, "elicitation/create");
                            let typed: t3_acp::v1::schema::CreateElicitationRequest =
                                serde_json::from_value(params.clone()).unwrap();
                            assert_eq!(serde_json::to_value(typed).unwrap(), params);
                            ClientRequest::Elicitation(params)
                        },
                        row["frame"]["result"].clone(),
                    )
                })
                .collect(),
        )));
        let (pipe, peer) = tokio::io::duplex(65536);
        let (client_reader, w) = tokio::io::split(pipe);
        let (peer_reader, mut w2) = tokio::io::split(peer);
        let mut reader = BufReader::new(peer_reader);
        let (client, mut events) = Client::connect(
            client_reader,
            w,
            responses.clone(),
            ClientOptions::default(),
        );
        let mut pending: HashMap<String, VecDeque<Value>> = HashMap::new();
        let mut tasks: HashMap<
            String,
            VecDeque<tokio::task::JoinHandle<Result<Value, t3_acp::Error>>>,
        > = HashMap::new();
        let mut next_id = 0;

        for row in &rows {
            let frame = &row["frame"];
            if row["type"] == "expect_outbound" {
                let method = frame["method"].as_str().unwrap().to_owned();
                match frame["kind"].as_str().unwrap() {
                    "request" => {
                        let c = client.clone();
                        let m = method.clone();
                        let mut params = frame["params"].clone();
                        if m == "initialize" {
                            for (key, value) in [
                                ("clientInfo", json!({"name":"replay","version":"1"})),
                                ("info", json!({"name":"replay","version":"1"})),
                                ("clientCapabilities", json!({})),
                                ("capabilities", json!({})),
                            ] {
                                if params[key] == "<any>" {
                                    params[key] = value;
                                }
                            }
                        }
                        if m == "session/new" {
                            if params["cwd"] == "<workspace>" {
                                params["cwd"] = json!("/workspace");
                            }
                            if params["mcpServers"] == "<any>" {
                                params["mcpServers"] = json!([]);
                            }
                        }
                        tasks
                            .entry(method.clone())
                            .or_default()
                            .push_back(tokio::spawn(async move {
                                match m.as_str() {
                                    "initialize" => c.initialize(params).await.map(|n| n.response),
                                    "session/new" => c.new_session(params).await,
                                    "session/prompt" => c.prompt(params).await,
                                    _ => c.request(&m, params).await,
                                }
                            }));
                        let mut line = String::new();
                        assert_ne!(reader.read_line(&mut line).await.unwrap(), 0, "{name}");
                        let actual: Value = serde_json::from_str(&line).unwrap();
                        assert_eq!(actual["method"], method, "{name}");
                        assert_recorded_params(&frame["params"], &actual["params"]);
                        match method.as_str() {
                            "initialize" => {
                                let typed: t3_acp::v1::schema::InitializeRequest =
                                    serde_json::from_value(actual["params"].clone()).unwrap();
                                assert_eq!(
                                    serde_json::to_value(typed).unwrap()["protocolVersion"],
                                    2
                                );
                                assert_eq!(
                                    actual["params"]["clientCapabilities"]["fs"],
                                    json!({"readTextFile":false,"writeTextFile":false})
                                );
                                assert_eq!(
                                    actual["params"]["clientCapabilities"]["terminal"],
                                    false
                                );
                            }
                            "session/new" => {
                                let typed: t3_acp::v1::schema::NewSessionRequest =
                                    serde_json::from_value(actual["params"].clone()).unwrap();
                                assert_eq!(serde_json::to_value(typed).unwrap(), actual["params"]);
                            }
                            "session/prompt" => {
                                let typed: t3_acp::v1::schema::PromptRequest =
                                    serde_json::from_value(actual["params"].clone()).unwrap();
                                assert_eq!(serde_json::to_value(typed).unwrap(), actual["params"]);
                            }
                            _ => {}
                        }
                        pending
                            .entry(method)
                            .or_default()
                            .push_back(actual["id"].clone());
                    }
                    "notification" => {
                        client
                            .notify(&method, frame["params"].clone())
                            .await
                            .unwrap();
                        let mut line = String::new();
                        reader.read_line(&mut line).await.unwrap();
                        let actual: Value = serde_json::from_str(&line).unwrap();
                        assert_eq!(actual["method"], method);
                        assert_recorded_params(&frame["params"], &actual["params"]);
                    }
                    "response" => {
                        let mut line = String::new();
                        reader.read_line(&mut line).await.unwrap();
                        let actual: Value = serde_json::from_str(&line).unwrap();
                        assert_eq!(actual["result"], frame["result"], "{name}");
                    }
                    other => panic!("unsupported recorded outbound {other}"),
                }
            } else if row["type"] == "emit_inbound" {
                let method = frame["method"].as_str().unwrap();
                let mut wire = json!({"jsonrpc":"2.0"});
                match frame["kind"].as_str().unwrap() {
                    "response" => {
                        wire["id"] = pending.get_mut(method).unwrap().pop_front().unwrap();
                        if let Some(error) = frame.get("error") {
                            wire["error"] = error.clone();
                        } else {
                            wire["result"] = frame["result"].clone();
                        }
                    }
                    "request" => {
                        next_id += 1;
                        wire["id"] = json!(format!("fixture-{next_id}"));
                        wire["method"] = json!(method);
                        wire["params"] = frame["params"].clone();
                    }
                    "notification" => {
                        wire["method"] = json!(method);
                        wire["params"] = frame["params"].clone();
                    }
                    other => panic!("unsupported recorded inbound {other}"),
                }
                let mut bytes = serde_json::to_vec(&wire).unwrap();
                bytes.push(b'\n');
                w2.write_all(&bytes).await.unwrap();
                if frame["kind"] == "notification" && method == "session/update" {
                    loop {
                        match events.next().await.unwrap().unwrap() {
                            Event::SessionUpdate(update) => {
                                assert_ne!(row["label"], "usage.malformed");
                                assert_eq!(
                                    update.session_id,
                                    frame["params"]["sessionId"].as_str().unwrap()
                                );
                                assert!(update.is_known(), "{name}: {}", update.kind());
                                assert_eq!(update.raw, frame["params"]);
                                assert_eq!(
                                    update.update, frame["params"]["update"],
                                    "{name}: normalized content"
                                );
                                let typed: t3_acp::v1::schema::SessionUpdate =
                                    serde_json::from_value(update.update.clone()).unwrap();
                                let mut encoded = serde_json::to_value(typed).unwrap();
                                // The SDK omits the v1 default pending status on tool-call serialization.
                                if update.kind() == "tool_call" && encoded.get("status").is_none() {
                                    encoded["status"] = json!("pending");
                                }
                                assert_eq!(
                                    encoded, frame["params"]["update"],
                                    "{name}: typed content"
                                );
                                break;
                            }
                            Event::InvalidNotification { method } => {
                                assert_eq!(method, "session/update");
                                assert_eq!(row["label"], "usage.malformed");
                                break;
                            }
                            Event::ResponseSent { .. } => {}
                            other => panic!("unexpected fixture event: {other:?}"),
                        }
                    }
                    events.acknowledge();
                }
                if frame["kind"] == "response" {
                    tasks
                        .get_mut(method)
                        .unwrap()
                        .pop_front()
                        .unwrap()
                        .await
                        .unwrap()
                        .unwrap();
                }
            }
        }
        assert!(tasks.values().all(VecDeque::is_empty));
        assert!(responses.0.lock().unwrap().is_empty());
        client.shutdown().await;
    }
}

fn assert_recorded_params(expected: &Value, actual: &Value) {
    match expected {
        Value::String(s) if s == "<any>" => assert!(!actual.is_null()),
        Value::String(s) if s == "<workspace>" => assert_eq!(actual, "/workspace"),
        Value::Object(fields) => {
            assert_eq!(
                actual.as_object().unwrap().len(),
                fields.len(),
                "fixture parameter keys"
            );
            for (key, value) in fields {
                assert_recorded_params(value, &actual[key]);
            }
        }
        _ => assert_eq!(actual, expected),
    }
}
