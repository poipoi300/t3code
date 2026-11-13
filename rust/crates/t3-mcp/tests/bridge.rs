#![allow(clippy::unwrap_used)]
use axum::{Router, body::Body, extract::State, response::Response, routing::post};
use http::{HeaderMap, StatusCode};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use t3_mcp::bridge::*;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
};

struct Server(String, JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.1.abort();
    }
}
async fn serve(app: Router) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    Server(
        endpoint,
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
    )
}
fn connection(server: &Server) -> HttpConnection {
    HttpConnection::with_client(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        &server.0,
        "Bearer bridge-test",
    )
}
fn response(value: Value) -> Response {
    Response::builder()
        .header("content-type", "application/json")
        .header("mcp-session-id", "session-42")
        .body(Body::from(value.to_string()))
        .unwrap()
}
fn sse(receiver: mpsc::Receiver<String>) -> Response {
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver
            .recv()
            .await
            .map(|data| (Ok::<_, std::io::Error>(data), receiver))
    });
    Response::builder()
        .header("content-type", "Text/Event-Stream; charset=utf-8")
        .body(Body::from_stream(stream))
        .unwrap()
}

#[tokio::test]
async fn relay_preserves_foreign_payloads_unicode_headers_and_handshake_order() {
    let (seen_tx, mut seen) = mpsc::unbounded_channel::<(HeaderMap, String)>();
    let server = serve(Router::new().route("/mcp", post(move |headers: HeaderMap, body: String| {
        let seen = seen_tx.clone();
        async move {
            seen.send((headers, body.clone())).unwrap();
            let request: Value = serde_json::from_str(&body).unwrap();
            if request["method"] == "notifications/initialized" { return StatusCode::ACCEPTED.into_response(); }
            response(json!({"jsonrpc":"2.0","id":request["id"],"result":if request["method"] == "initialize" {json!({"protocolVersion":"2025-06-18"})} else {json!({"foreign":true})}}))
        }
    }))).await;
    use axum::response::IntoResponse;
    let foreign = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"foreign","arguments":{"text":"a\u{2028}b\u{2029}c🌍","future":{"unknown":[null,1,true]}}}}).to_string();
    let input = format!(
        "{{not-json}}\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}}\r\n{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\n{foreign}"
    );
    let (output, mut reader) = tokio::io::duplex(8192);
    run_stdio_bridge(
        connection(&server),
        BufReader::new(input.as_bytes()),
        output,
    )
    .await
    .unwrap();
    let mut written = String::new();
    reader.read_to_string(&mut written).await.unwrap();
    let written = written
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(written.len(), 3);
    assert!(written.iter().any(|value| value
        == &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})));
    assert!(
        written
            .iter()
            .any(|value| value["id"] == 4 && value["result"]["foreign"] == true)
    );
    for index in 0..3 {
        let (headers, body) = seen.recv().await.unwrap();
        assert_eq!(headers["authorization"], "Bearer bridge-test");
        if index == 0 {
            assert!(!headers.contains_key("mcp-session-id"));
        } else {
            assert_eq!(headers["mcp-session-id"], "session-42");
            assert_eq!(headers["mcp-protocol-version"], "2025-06-18");
        }
        if index == 2 {
            assert_eq!(body, foreign);
        }
    }
}

#[derive(Clone)]
struct Streaming {
    pending: Arc<Mutex<Option<mpsc::Sender<String>>>>,
    seen: mpsc::UnboundedSender<Value>,
    server_method: &'static str,
}
async fn streaming(
    State(state): State<Streaming>,
    axum::Json(request): axum::Json<Value>,
) -> Response {
    use axum::response::IntoResponse;
    state.seen.send(request.clone()).unwrap();
    match request["method"].as_str() {
        Some("initialize") => response(
            json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":"2025-06-18"}}),
        ),
        Some("tools/call") => {
            let (tx, rx) = mpsc::channel(4);
            tx.send(format!(
                "data: {}\r\n\r\n",
                json!({"jsonrpc":"2.0","id":"server-ping","method":state.server_method})
            ))
            .await
            .unwrap();
            *state.pending.lock().unwrap() = Some(tx);
            sse(rx)
        }
        Some("notifications/initialized") => StatusCode::ACCEPTED.into_response(),
        _ => {
            let sender = state.pending.lock().unwrap().take().unwrap();
            sender
                .send(
                    "data: {\"jsonrpc\":\"2.0\",\"id\":\"t3-cli-1\",\"result\":{\"ok\":true}}\n\n"
                        .into(),
                )
                .await
                .unwrap();
            StatusCode::ACCEPTED.into_response()
        }
    }
}

#[tokio::test]
async fn relay_streams_server_requests_and_bypasses_open_streams_for_replies_and_cancellation() {
    for followup in [
        json!({"jsonrpc":"2.0","id":"server-ping","result":{}}),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}),
    ] {
        let (seen, mut receiver) = mpsc::unbounded_channel();
        let server = serve(
            Router::new()
                .route("/mcp", post(streaming))
                .with_state(Streaming {
                    pending: Arc::default(),
                    seen,
                    server_method: "ping",
                }),
        )
        .await;
        let (mut input, bridge_input) = tokio::io::duplex(4096);
        let (output, bridge_output) = tokio::io::duplex(4096);
        let connection = connection(&server);
        let bridge = tokio::spawn(run_stdio_bridge(
            connection,
            BufReader::new(bridge_input),
            output,
        ));
        input
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\"}\n")
            .await
            .unwrap();
        let mut reader = BufReader::new(bridge_output);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["id"],
            "server-ping"
        );
        input
            .write_all(format!("{followup}\n").as_bytes())
            .await
            .unwrap();
        input.shutdown().await.unwrap();
        bridge.await.unwrap().unwrap();
        assert_eq!(receiver.recv().await.unwrap()["method"], "tools/call");
        assert_eq!(receiver.recv().await.unwrap(), followup);
    }
}

#[tokio::test]
async fn scoped_client_answers_streamed_ping_and_terminates_even_after_failed_handshake() {
    for (fail_initialize, server_method) in [
        (false, "ping"),
        (false, "elicitation/create"),
        (true, "ping"),
    ] {
        let (seen, mut receiver) = mpsc::unbounded_channel();
        let state = Streaming {
            pending: Arc::default(),
            seen,
            server_method,
        };
        let on_delete = state.seen.clone();
        let server = serve(Router::new().route("/mcp", post(move |State(state): State<Streaming>, request: axum::Json<Value>| async move {
            if fail_initialize && request["method"] == "initialize" {
                state.seen.send(request.0.clone()).unwrap();
                return response(json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32603,"message":"failed"}}));
            }
            streaming(State(state), request).await
        }).delete(move || {
            let seen = on_delete.clone();
            async move { seen.send(json!({"method":"DELETE"})).unwrap(); StatusCode::NO_CONTENT }
        })).with_state(state)).await;
        let result = with_tool_session(connection(&server), "fixture", |session| {
            Box::pin(async move { session.call("probe", serde_json::Map::new()).await })
        })
        .await;
        if fail_initialize {
            assert!(result.unwrap_err().message.contains("initialize failed"));
        } else {
            assert_eq!(result.unwrap(), json!({"ok":true}));
        }
        let mut sent = Vec::new();
        loop {
            let next = receiver.recv().await.unwrap();
            let deleted = next["method"] == "DELETE";
            sent.push(next);
            if deleted {
                break;
            }
        }
        if !fail_initialize {
            let expected = if server_method == "ping" {
                json!({"jsonrpc":"2.0","id":"server-ping","result":{}})
            } else {
                json!({"jsonrpc":"2.0","id":"server-ping","error":{"code":-32601,"message":"Method not found"}})
            };
            assert!(sent.contains(&expected));
        }
    }
}

#[tokio::test]
async fn cancelling_a_handshake_terminates_the_session_already_opened_by_its_headers() {
    let (seen, mut receiver) = mpsc::unbounded_channel();
    let pending = Arc::new(Mutex::new(None::<mpsc::Sender<String>>));
    let on_delete = seen.clone();
    let server = serve(Router::new().route("/mcp", post(move |axum::Json(request): axum::Json<Value>| {
        let seen = seen.clone();
        let pending = pending.clone();
        async move {
            if request["method"] == "initialize" {
                let (tx, rx) = mpsc::channel(1);
                tx.send("data: {\"jsonrpc\":\"2.0\",\"id\":\"server-ping\",\"method\":\"ping\"}\n\n".into()).await.unwrap();
                *pending.lock().unwrap() = Some(tx);
                let mut response = sse(rx);
                response.headers_mut().insert("mcp-session-id", "session-42".parse().unwrap());
                response
            } else {
                seen.send("reply").unwrap();
                axum::response::IntoResponse::into_response(StatusCode::ACCEPTED)
            }
        }
    }).delete(move |headers:HeaderMap| {
        let seen = on_delete.clone();
        async move {
            assert_eq!(headers["mcp-session-id"], "session-42");
            seen.send("DELETE").unwrap();
            StatusCode::NO_CONTENT
        }
    }))).await;
    let connection = connection(&server);
    let client = tokio::spawn(async move {
        with_tool_session(connection, "fixture", |_session| Box::pin(async { Ok(()) })).await
    });
    assert_eq!(receiver.recv().await.unwrap(), "reply");
    client.abort();
    assert!(client.await.unwrap_err().is_cancelled());
    assert_eq!(receiver.recv().await.unwrap(), "DELETE");
}

#[test]
fn terminal_output_retains_errors_images_and_explanations_while_removing_duplicate_json() {
    let structured = json!({"taskId":"task","status":"failed"});
    let explanation = json!({"type":"text","text":"The command failed."});
    let image = json!({"type":"image","data":"png","mimeType":"image/png"});
    let result = terminal_result(
        json!({"structuredContent":structured,"isError":true,"content":[
            {"type":"text","text":structured.to_string()},explanation,image
        ]}),
    );
    assert_eq!(
        result,
        json!({"structuredContent":structured,"isError":true,"content":[explanation,image]})
    );
}
