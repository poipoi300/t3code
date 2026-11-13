#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{future::Future, sync::Arc, task::Poll};
use t3_acp::{Client, ClientOptions, Events, RejectRequests, SessionState};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

struct Peer {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}
impl Peer {
    async fn receive(&mut self) -> Value {
        let mut line = String::new();
        assert_ne!(self.reader.read_line(&mut line).await.unwrap(), 0);
        serde_json::from_str(&line).unwrap()
    }
    async fn send(&mut self, frame: Value) {
        let mut bytes = serde_json::to_vec(&frame).unwrap();
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.unwrap();
    }
    async fn reply(&mut self, frame: &Value, result: Value) {
        self.send(json!({"id":frame["id"],"result":result})).await;
    }
}
fn connect() -> (Client, Events, Peer) {
    let (local, remote) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(local);
    let (pr, pw) = tokio::io::split(remote);
    let (c, e) = Client::connect(r, w, Arc::new(RejectRequests), ClientOptions::default());
    (
        c,
        e,
        Peer {
            reader: BufReader::new(pr),
            writer: pw,
        },
    )
}
async fn exchange<F: Future<Output = Result<Value, t3_acp::Error>>>(
    peer: &mut Peer,
    future: F,
    result: Value,
) -> Value {
    let (result, ()) = tokio::join!(future, async {
        let frame = peer.receive().await;
        peer.reply(&frame, result).await;
    });
    result.unwrap()
}

#[tokio::test]
async fn closing_session_removes_cached_state() {
    let (client, _events, mut peer) = connect();
    exchange(&mut peer,client.new_session(json!({"cwd":"/w","mcpServers":[]})),json!({"sessionId":"s","configOptions":[{"type":"boolean","configId":"x","name":"X","currentValue":true}]})).await;
    assert_ne!(client.session_state("s"), SessionState::default());
    exchange(
        &mut peer,
        client.close_session(json!({"sessionId":"s"})),
        json!({}),
    )
    .await;
    assert_eq!(client.session_state("s"), SessionState::default());
    client.shutdown().await;
}

#[tokio::test]
async fn absent_draft_config_options_normalize_to_empty_array() {
    let (client, _events, mut peer) = connect();
    let (result, ()) = tokio::join!(client.initialize(json!({})), async {
        let frame = peer.receive().await;
        peer.reply(
            &frame,
            json!({"protocolVersion":2,"info":{"name":"agent","version":"1"}}),
        )
        .await;
    });
    result.unwrap();
    let result = exchange(
        &mut peer,
        client.set_config_option(json!({"sessionId":"s","configId":"x","value":true})),
        json!({}),
    )
    .await;
    assert_eq!(result["configOptions"], json!([]));
    assert_eq!(client.session_state("s").config_options, Some(json!([])));
    client.shutdown().await;
}

#[tokio::test]
async fn v1_prompts_may_overlap_and_cancel_waits_for_both() {
    let (client, _events, mut peer) = connect();
    let (result, ()) = tokio::join!(client.initialize(json!({})), async {
        let frame = peer.receive().await;
        peer.reply(&frame, json!({"protocolVersion":1})).await;
    });
    result.unwrap();
    let first = client.prompt(json!({"sessionId":"s","prompt":[]}));
    tokio::pin!(first);
    assert!(std::future::poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await);
    let one = peer.receive().await;
    let second = client.prompt(json!({"sessionId":"s","prompt":[]}));
    tokio::pin!(second);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx).is_pending())).await,
        "v1 must permit overlapping prompts"
    );
    let two = peer.receive().await;
    let cancel = client.cancel_and_wait(json!({"sessionId":"s"}));
    tokio::pin!(cancel);
    assert!(std::future::poll_fn(|cx| Poll::Ready(cancel.as_mut().poll(cx).is_pending())).await);
    assert_eq!(peer.receive().await["method"], "session/cancel");
    peer.reply(&one, json!({"stopReason":"cancelled"})).await;
    first.await.unwrap();
    assert!(std::future::poll_fn(|cx| Poll::Ready(cancel.as_mut().poll(cx).is_pending())).await);
    peer.reply(&two, json!({"stopReason":"cancelled"})).await;
    second.await.unwrap();
    cancel.await.unwrap();
    client.shutdown().await;
}

struct Policy(std::sync::Mutex<std::collections::HashMap<String, String>>);
#[t3_acp::async_trait]
impl t3_acp::ClientHandler for Policy {
    async fn request(
        &self,
        request: t3_acp::ClientRequest,
        _: t3_acp::RequestContext,
    ) -> Result<Value, t3_acp::RpcError> {
        let t3_acp::ClientRequest::Permission(params) = request else {
            panic!("unexpected effect")
        };
        let id = params["toolCall"]["toolCallId"].as_str().unwrap();
        assert_eq!(
            self.0.lock().unwrap().get(id).map(String::as_str),
            Some("read"),
            "permission policy must see the preceding tool kind"
        );
        Ok(json!({"outcome":{"outcome":"selected","optionId":"allow"}}))
    }
}

#[tokio::test]
async fn scripted_tools_permissions_trailing_text_and_prompt_replies_are_ordered() {
    for batch in [false, true] {
        let (local, remote) = tokio::io::duplex(65536);
        let (r, w) = tokio::io::split(local);
        let (pr, pw) = tokio::io::split(remote);
        let policy = Arc::new(Policy(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )));
        let (client, mut events) = Client::connect(r, w, policy.clone(), ClientOptions::default());
        let mut peer = Peer {
            reader: BufReader::new(pr),
            writer: pw,
        };
        let (initialized, ()) = tokio::join!(client.initialize(json!({})), async {
            let frame = peer.receive().await;
            peer.reply(&frame, json!({"protocolVersion":1})).await;
        });
        initialized.unwrap();
        let text = Arc::new(std::sync::Mutex::new(String::new()));
        let mut prompts = Vec::new();
        let mut requests = Vec::new();
        for n in 0..3 {
            let c = client.clone();
            let observed = text.clone();
            prompts.push(tokio::spawn(async move {
                c.prompt(json!({"sessionId":format!("s{n}"),"prompt":[]}))
                    .await
                    .unwrap();
                assert!(
                    observed.lock().unwrap().contains(&format!("tail{n}")),
                    "prompt resolved before its trailing text was handled"
                );
            }));
            requests.push(peer.receive().await);
        }
        for (n, request) in requests.iter().enumerate() {
            let frames = [
                json!({"method":"session/update","params":{"sessionId":format!("s{n}"),"update":{"sessionUpdate":"tool_call","toolCallId":format!("t{n}"),"kind":"read","title":"Read","status":"pending"}}}),
                json!({"id":n,"method":"session/request_permission","params":{"sessionId":format!("s{n}"),"toolCall":{"toolCallId":format!("t{n}")},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]}}),
                json!({"method":"session/update","params":{"sessionId":format!("s{n}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("head{n}")}}}}),
                json!({"method":"session/update","params":{"sessionId":format!("s{n}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("tail{n}")}}}}),
                json!({"id":request["id"],"result":{"stopReason":"end_turn"}}),
            ];
            if batch {
                peer.send(json!(frames)).await;
            } else {
                for frame in frames {
                    peer.send(frame).await;
                }
            }
        }
        let mut updates = 0;
        while updates < 9 {
            if let t3_acp::Event::SessionUpdate(update) = events.next().await.unwrap().unwrap() {
                if update.kind() == "tool_call" {
                    policy.0.lock().unwrap().insert(
                        update.update["toolCallId"].as_str().unwrap().into(),
                        update.update["kind"].as_str().unwrap().into(),
                    );
                } else {
                    let block: t3_acp::v1::schema::ContentBlock =
                        serde_json::from_value(update.update["content"].clone()).unwrap();
                    let t3_acp::v1::schema::ContentBlock::Text(block) = block else {
                        panic!()
                    };
                    text.lock().unwrap().push_str(&block.text);
                }
                updates += 1;
            }
        }
        events.acknowledge();
        for prompt in prompts {
            prompt.await.unwrap();
        }
        for _ in 0..3 {
            let reply = peer.receive().await;
            let reply = if batch { &reply[0] } else { &reply };
            assert_eq!(reply["result"]["outcome"]["optionId"], "allow");
        }
        client.shutdown().await;
    }
}

struct Blocked {
    entered: tokio::sync::mpsc::UnboundedSender<Value>,
    gate: Arc<tokio::sync::Semaphore>,
}
#[t3_acp::async_trait]
impl t3_acp::ClientHandler for Blocked {
    async fn request(
        &self,
        request: t3_acp::ClientRequest,
        _: t3_acp::RequestContext,
    ) -> Result<Value, t3_acp::RpcError> {
        let params = match request {
            t3_acp::ClientRequest::Extension { params, .. } => params,
            t3_acp::ClientRequest::ReadTextFile(params) => params["token"].clone(),
            _ => panic!(),
        };
        self.entered.send(params).unwrap();
        let _permit = self.gate.acquire().await.unwrap();
        Ok(json!({"ok":true}))
    }
}

#[tokio::test]
async fn duplicate_ids_are_ignored_handlers_are_capped_and_id_can_be_reused_after_flush() {
    let (local, remote) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(local);
    let (pr, pw) = tokio::io::split(remote);
    let (tx, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (client, mut events) = Client::connect(
        r,
        w,
        Arc::new(Blocked {
            entered: tx,
            gate: gate.clone(),
        }),
        ClientOptions {
            max_concurrent_handlers: 1,
            ..Default::default()
        },
    );
    let mut peer = Peer {
        reader: BufReader::new(pr),
        writer: pw,
    };
    peer.send(json!({"id":7,"method":"fs/read_text_file","params":{"sessionId":"s","path":"/never-read","token":1}})).await;
    assert_eq!(entered.recv().await, Some(json!(1)));
    peer.send(json!({"id":7,"method":"fs/read_text_file","params":{"sessionId":"s","path":"/never-read","token":2}})).await;
    assert_eq!(
        events.next().await.unwrap().unwrap(),
        t3_acp::Event::DuplicateRequest {
            id: t3_acp::RequestId::Number(7)
        }
    );
    assert!(entered.try_recv().is_err());
    peer.send(json!({"id":8,"method":"fs/read_text_file","params":{"sessionId":"s","path":"/never-read","token":3}})).await;
    assert_eq!(peer.receive().await["error"]["code"], -32000);
    assert!(entered.try_recv().is_err());
    gate.add_permits(1);
    let reply = peer.receive().await;
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["result"]["ok"], true);
    while events.next().await.unwrap().unwrap()
        != (t3_acp::Event::ResponseSent {
            id: t3_acp::RequestId::Number(7),
        })
    {}
    peer.send(json!({"id":7,"method":"fs/read_text_file","params":{"sessionId":"s","path":"/never-read","token":4}})).await;
    assert_eq!(entered.recv().await, Some(json!(4)));
    assert_eq!(peer.receive().await["id"], 7);
    client.shutdown().await;
}

#[tokio::test]
async fn oversized_unterminated_lines_are_bounded() {
    let (local, mut peer) = tokio::io::duplex(1024);
    let (r, w) = tokio::io::split(local);
    let (client, mut events) = Client::connect(
        r,
        w,
        Arc::new(RejectRequests),
        ClientOptions {
            max_line_bytes: 64,
            ..Default::default()
        },
    );
    peer.write_all(&[b' '; 65]).await.unwrap();
    assert_eq!(
        events.next().await,
        Some(Err(t3_acp::Error::LineTooLong { limit: 64 }))
    );
    client.shutdown().await;
}

#[tokio::test]
async fn consumer_stall_backpressures_agent_without_losing_updates_or_closing() {
    let (local, remote) = tokio::io::duplex(256);
    let (r, w) = tokio::io::split(local);
    let (pr, mut pw) = tokio::io::split(remote);
    let (client, mut events) = Client::connect(
        r,
        w,
        Arc::new(RejectRequests),
        ClientOptions {
            event_capacity: 2,
            ..Default::default()
        },
    );
    {
        let producer = async {
            for n in 0..40 {
                let mut data = serde_json::to_vec(&json!({"method":"_tick","params":n})).unwrap();
                data.push(b'\n');
                pw.write_all(&data).await.unwrap();
            }
        };
        tokio::pin!(producer);
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(producer.as_mut().poll(cx).is_pending())).await
        );
        let consume = async {
            for n in 0..40 {
                assert_eq!(
                    events.next().await.unwrap().unwrap(),
                    t3_acp::Event::Notification {
                        method: "_tick".into(),
                        params: json!(n)
                    }
                );
            }
            events.acknowledge();
        };
        tokio::join!(producer, consume);
    }
    let mut peer = Peer {
        reader: BufReader::new(pr),
        writer: pw,
    };
    assert_eq!(
        exchange(
            &mut peer,
            client.request("_healthy", json!({})),
            json!(true)
        )
        .await,
        json!(true)
    );
    client.shutdown().await;
}

#[test]
fn numeric_ids_share_javascript_number_identity_without_colliding_with_strings() {
    use std::collections::HashSet;
    for (a, b) in [
        (json!(1), json!(1.0)),
        (json!(0), json!(-0.0)),
        (json!(9007199254740992_u64), json!(9007199254740993_u64)),
    ] {
        let a: t3_acp::RequestId = serde_json::from_value(a).unwrap();
        let b: t3_acp::RequestId = serde_json::from_value(b).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.identity(), b.identity());
        assert_eq!(HashSet::from([a, b]).len(), 1);
    }
}

#[tokio::test]
async fn unsolicited_session_updates_do_not_allocate_cache_entries() {
    let (client, mut events, mut peer) = connect();
    peer.send(json!({"method":"session/update","params":{"sessionId":"unsolicited","update":{"sessionUpdate":"config_option_update","configOptions":[{"type":"boolean","configId":"x","name":"X","currentValue":true}]}}})).await;
    events.next().await.unwrap().unwrap();
    assert_eq!(client.session_state("unsolicited"), SessionState::default());
    client.shutdown().await;
}

struct ObservedReader {
    pipe: ReadHalf<DuplexStream>,
    remaining: usize,
    reached: Option<tokio::sync::oneshot::Sender<()>>,
}
impl tokio::io::AsyncRead for ObservedReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = std::pin::Pin::new(&mut self.pipe).poll_read(cx, buf);
        self.remaining = self.remaining.saturating_sub(buf.filled().len() - before);
        if self.remaining == 0
            && let Some(reached) = self.reached.take()
        {
            let _ = reached.send(());
        }
        result
    }
}

#[tokio::test]
async fn final_unterminated_response_survives_handler_completion_mid_line() {
    let (local, remote) = tokio::io::duplex(4096);
    let (r, w) = tokio::io::split(local);
    let (pr, pw) = tokio::io::split(remote);
    let mut peer = Peer {
        reader: BufReader::new(pr),
        writer: pw,
    };
    let hold = json!({"id":7,"method":"fs/read_text_file","params":{"sessionId":"s","path":"/never-read","token":0}});
    let hold_length = serde_json::to_vec(&hold).unwrap().len() + 1;
    let (reached, partial_read) = tokio::sync::oneshot::channel();
    let reader = ObservedReader {
        pipe: r,
        remaining: hold_length + 8,
        reached: Some(reached),
    };
    let (tx, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (client, _events) = Client::connect(
        reader,
        w,
        Arc::new(Blocked {
            entered: tx,
            gate: gate.clone(),
        }),
        ClientOptions::default(),
    );
    peer.send(hold).await;
    entered.recv().await.unwrap();
    let c = client.clone();
    let response = tokio::spawn(async move { c.request("_final", json!({})).await });
    let request = peer.receive().await;
    let bytes =
        serde_json::to_vec(&json!({"id":request["id"],"result":{"complete":true}})).unwrap();
    peer.writer.write_all(&bytes[..8]).await.unwrap();
    partial_read.await.unwrap();
    gate.add_permits(1);
    assert_eq!(peer.receive().await["id"], 7);
    peer.writer.write_all(&bytes[8..]).await.unwrap();
    peer.writer.shutdown().await.unwrap();
    assert_eq!(response.await.unwrap().unwrap(), json!({"complete":true}));
    client.shutdown().await;
}

#[tokio::test]
async fn fractional_and_large_ids_missing_results_and_unroutable_frames_are_recoverable() {
    let (client, mut events, mut peer) = connect();
    assert_eq!(client.generation(), t3_acp::Generation::V2);
    for id in [json!(1.0), json!(1.5), json!(18446744073709551615_u64)] {
        peer.send(json!({"id":id,"method":"_unsupported","params":{}}))
            .await;
        let response = peer.receive().await;
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], -32601);
    }
    let c = client.clone();
    let empty = tokio::spawn(async move { c.request("_empty", json!({})).await });
    let frame = peer.receive().await;
    peer.send(
        json!([42,{}, {"id":"leaked","error":false}, {"id":frame["id"].as_i64().unwrap() as f64}]),
    )
    .await;
    assert_eq!(empty.await.unwrap().unwrap(), Value::Null);
    let mut skipped = 0;
    while skipped < 3 {
        if events.next().await.unwrap().unwrap() == t3_acp::Event::UnroutableFrame {
            skipped += 1;
        }
    }
    let c = client.clone();
    let bad = tokio::spawn(async move { c.request("_bad", json!({})).await });
    let frame = peer.receive().await;
    peer.send(json!({"id":frame["id"],"error":"private malformed error"}))
        .await;
    assert_eq!(bad.await.unwrap().unwrap_err(), t3_acp::Error::InvalidFrame);
    assert_eq!(
        exchange(
            &mut peer,
            client.request("_still_healthy", json!({})),
            json!(true)
        )
        .await,
        json!(true)
    );
    client.shutdown().await;
}

struct RetainedClient {
    client: std::sync::Mutex<Option<Client>>,
    dropped: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}
impl Drop for RetainedClient {
    fn drop(&mut self) {
        if let Some(tx) = self.dropped.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }
}
#[t3_acp::async_trait]
impl t3_acp::ClientHandler for RetainedClient {
    async fn request(
        &self,
        _: t3_acp::ClientRequest,
        _: t3_acp::RequestContext,
    ) -> Result<Value, t3_acp::RpcError> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn dropping_event_stream_breaks_a_handler_retaining_a_client_clone() {
    let (local, mut remote) = tokio::io::duplex(1024);
    let (r, w) = tokio::io::split(local);
    let (tx, dropped) = tokio::sync::oneshot::channel();
    let handler = Arc::new(RetainedClient {
        client: std::sync::Mutex::new(None),
        dropped: std::sync::Mutex::new(Some(tx)),
    });
    let (client, events) = Client::connect(r, w, handler.clone(), ClientOptions::default());
    *handler.client.lock().unwrap() = Some(client.clone());
    drop(handler);
    drop(client);
    drop(events);
    dropped.await.unwrap();
    let mut output = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut remote, &mut output)
        .await
        .unwrap();
    assert!(output.is_empty());
}
