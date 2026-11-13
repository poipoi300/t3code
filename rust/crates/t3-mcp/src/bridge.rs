use futures_util::{FutureExt, future::BoxFuture};
use reqwest::{Client, Method, Response};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex as AsyncMutex, oneshot},
    task::JoinSet,
};

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct BridgeError {
    pub message: String,
}

impl From<reqwest::Error> for BridgeError {
    fn from(error: reqwest::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl From<std::io::Error> for BridgeError {
    fn from(error: std::io::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl From<serde_json::Error> for BridgeError {
    fn from(error: serde_json::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

type Result<T> = std::result::Result<T, BridgeError>;

#[derive(Default)]
struct SessionState {
    id: Option<String>,
    version: Option<String>,
}

/// Shares only session headers; requests and SSE bodies remain concurrent.
#[derive(Clone)]
pub struct HttpConnection {
    client: Client,
    endpoint: String,
    authorization: String,
    state: Arc<Mutex<SessionState>>,
}

impl HttpConnection {
    pub fn new(endpoint: impl Into<String>, authorization: impl Into<String>) -> Self {
        Self::with_client(Client::new(), endpoint, authorization)
    }

    pub fn with_client(
        client: Client,
        endpoint: impl Into<String>,
        authorization: impl Into<String>,
    ) -> Self {
        Self {
            client,
            endpoint: endpoint.into(),
            authorization: authorization.into(),
            state: Arc::default(),
        }
    }

    async fn send(&self, method: Method, body: Option<String>) -> Result<Payloads> {
        let mut request = self
            .client
            .request(method, &self.endpoint)
            .header("accept", "application/json, text/event-stream")
            .header("authorization", &self.authorization);
        {
            let state = self.state.lock().expect("bridge state lock poisoned");
            if let Some(id) = &state.id {
                request = request.header("mcp-session-id", id);
            }
            if let Some(version) = &state.version {
                request = request.header("mcp-protocol-version", version);
            }
        }
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body);
        }
        let response = request.send().await?;
        if let Some(id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.state.lock().expect("bridge state lock poisoned").id = Some(id.into());
        }
        if !response.status().is_success() {
            return Err(BridgeError {
                message: format!(
                    "T3 Code MCP endpoint responded with HTTP {}.",
                    response.status().as_u16()
                ),
            });
        }
        Ok(Payloads::new(response))
    }

    fn observe(&self, payload: &Value) {
        if let Some(version) = payload["result"]["protocolVersion"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            self.state
                .lock()
                .expect("bridge state lock poisoned")
                .version = Some(version.into());
        }
    }

    pub async fn terminate(&self) {
        let has_session = self
            .state
            .lock()
            .expect("bridge state lock poisoned")
            .id
            .is_some();
        if has_session {
            let _ = self.send(Method::DELETE, None).await;
        }
    }
}

struct Payloads {
    response: Option<Response>,
    sse: bool,
    buffer: Vec<u8>,
}

impl Payloads {
    fn new(response: Response) -> Self {
        let empty = matches!(response.status().as_u16(), 202 | 204);
        let sse = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| {
                h.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .eq_ignore_ascii_case("text/event-stream")
            });
        Self {
            response: (!empty).then_some(response),
            sse,
            buffer: Vec::new(),
        }
    }

    async fn next(&mut self) -> Result<Option<Value>> {
        if !self.sse {
            let Some(response) = self.response.take() else {
                return Ok(None);
            };
            let text = response.text().await?;
            return if text.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(serde_json::from_str(&text)?))
            };
        }
        loop {
            if let Some((end, consumed)) = event_end(&self.buffer) {
                let raw = String::from_utf8_lossy(&self.buffer[..end])
                    .replace("\r\n", "\n")
                    .replace('\r', "\n");
                let data = raw
                    .split('\n')
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                self.buffer.drain(..consumed);
                if !data.is_empty() {
                    return Ok(Some(serde_json::from_str(&data)?));
                }
            } else {
                let Some(response) = self.response.as_mut() else {
                    return Ok(None);
                };
                match response.chunk().await? {
                    Some(chunk) => self.buffer.extend_from_slice(&chunk),
                    None => {
                        self.response = None;
                        return Ok(None);
                    }
                }
            }
        }
    }
}

fn newline_end(bytes: &[u8], index: usize) -> Option<usize> {
    match bytes.get(index) {
        Some(b'\n') => Some(index + 1),
        Some(b'\r') => Some(
            index
                + if bytes.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                },
        ),
        _ => None,
    }
}

fn event_end(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < bytes.len() {
        if let Some(first) = newline_end(bytes, index) {
            if let Some(second) = newline_end(bytes, first) {
                return Some((index, second));
            }
            index = first;
        } else {
            index += 1;
        }
    }
    None
}

fn rpc_error(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message.into()}})
}

async fn write<W: AsyncWrite + Unpin>(output: &AsyncMutex<W>, value: &Value) -> Result<()> {
    let mut output = output.lock().await;
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    output.write_all(&bytes).await?;
    output.flush().await?;
    Ok(())
}

async fn forward<W: AsyncWrite + Unpin>(
    connection: &HttpConnection,
    output: &AsyncMutex<W>,
    line: String,
    envelope: Value,
) -> Result<()> {
    let result = async {
        let mut payloads = connection.send(Method::POST, Some(line)).await?;
        while let Some(payload) = payloads.next().await? {
            let entries = match payload {
                Value::Array(entries) => entries,
                other => vec![other],
            };
            for entry in entries {
                connection.observe(&entry);
                write(output, &entry).await?;
            }
        }
        Ok::<_, BridgeError>(())
    }
    .await;
    if let Err(error) = result
        && let Some(id) = envelope.get("id")
    {
        let message = if error
            .message
            .starts_with("T3 Code MCP endpoint responded with HTTP ")
        {
            error.message
        } else {
            format!("T3 Code MCP bridge request failed: {error}")
        };
        write(output, &rpc_error(id.clone(), -32603, message)).await?;
    }
    Ok(())
}

/// Relays foreign JSON-RPC unchanged, ordering only initialization, never tool calls.
pub async fn run_stdio_bridge<R, W>(
    connection: HttpConnection,
    mut input: R,
    output: W,
) -> Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let output = Arc::new(AsyncMutex::new(output));
    let mut tasks = JoinSet::new();
    let mut barrier: Option<futures_util::future::Shared<BoxFuture<'static, ()>>> = None;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match input.read_until(b'\n', &mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&bytes)
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if line.trim().is_empty() {
            continue;
        }
        let envelope: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                write(&output, &rpc_error(Value::Null, -32700, "Parse error")).await?;
                continue;
            }
        };
        if !envelope.is_object() && !envelope.is_array() {
            continue;
        }
        let response = envelope.get("id").is_some()
            && (envelope.get("result").is_some() || envelope.get("error").is_some());
        let handshake = matches!(
            envelope["method"].as_str(),
            Some("initialize" | "notifications/initialized")
        );
        let prior = if response { None } else { barrier.clone() };
        let done = if handshake {
            let (tx, rx) = oneshot::channel();
            barrier = Some(
                async move {
                    let _ = rx.await;
                }
                .boxed()
                .shared(),
            );
            Some(tx)
        } else {
            None
        };
        let connection = connection.clone();
        let output = output.clone();
        tasks.spawn(async move {
            if let Some(prior) = prior {
                prior.await;
            }
            let result = forward(&connection, &output, line, envelope).await;
            if let Some(done) = done {
                let _ = done.send(());
            }
            result
        });
        while let Some(result) = tasks.try_join_next() {
            result.map_err(|e| BridgeError {
                message: e.to_string(),
            })??;
        }
    }
    while let Some(result) = tasks.join_next().await {
        result.map_err(|e| BridgeError {
            message: e.to_string(),
        })??;
    }
    Ok(())
}

pub struct ToolSession {
    connection: HttpConnection,
    next_id: u64,
}

/// Terminal callers display both content forms, so remove only their exact duplicate.
pub fn terminal_result(mut result: Value) -> Value {
    if let Some(structured) = result.get("structuredContent") {
        let encoded = structured.to_string();
        if let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) {
            content.retain(|part| part["type"] != "text" || part["text"] != encoded);
        }
    }
    result
}

impl ToolSession {
    async fn send(&self, message: Value) -> Result<Vec<Value>> {
        let mut payloads = self
            .connection
            .send(Method::POST, Some(message.to_string()))
            .await?;
        let mut received = Vec::new();
        while let Some(payload) = payloads.next().await? {
            self.connection.observe(&payload);
            if let Some(id) = payload.get("id")
                && payload["method"].is_string()
            {
                let reply = if payload["method"] == "ping" {
                    json!({"jsonrpc":"2.0","id":id,"result":{}})
                } else {
                    rpc_error(id.clone(), -32601, "Method not found")
                };
                Box::pin(self.send(reply)).await?;
            } else {
                received.push(payload);
            }
        }
        Ok(received)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = format!("t3-cli-{}", self.next_id);
        self.next_id += 1;
        let received = self
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        let response = received.into_iter().find(|entry| entry["id"] == id);
        match response {
            Some(value) if value.get("error").is_none() => Ok(value["result"].clone()),
            response => Err(BridgeError {
                message: format!(
                    "T3 Code MCP {method} failed{}",
                    response
                        .and_then(|v| v.get("error").cloned())
                        .map_or(".".into(), |e| format!(": {e}"))
                ),
            }),
        }
    }

    pub async fn call(
        &mut self,
        tool: &str,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<Value> {
        self.request("tools/call", json!({"name":tool,"arguments":arguments}))
            .await
    }
}

/// Registers cleanup before the handshake so cancellation also terminates opened sessions.
pub async fn with_tool_session<T, F>(
    connection: HttpConnection,
    client_name: &str,
    operation: F,
) -> Result<T>
where
    F: for<'a> FnOnce(&'a mut ToolSession) -> BoxFuture<'a, Result<T>>,
{
    struct Cleanup(Option<HttpConnection>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(connection) = self.0.take()
                && let Ok(runtime) = tokio::runtime::Handle::try_current()
            {
                runtime.spawn(async move {
                    connection.terminate().await;
                });
            }
        }
    }
    let mut cleanup = Cleanup(Some(connection.clone()));
    let mut session = ToolSession {
        connection,
        next_id: 0,
    };
    let result = async {
        session.request("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":client_name,"version":"0.0.0"}})).await?;
        session.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await?;
        operation(&mut session).await
    }.await;
    session.connection.terminate().await;
    cleanup.0 = None;
    result
}
