//! Shared helpers for package C's integration tests: a real server on an ephemeral port and
//! a real WebSocket driven by the `t3-rpc` client.
#![allow(clippy::unwrap_used, dead_code)]
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use t3_rpc::{CallEvent, CallKind, Client, Exit, Frame};
use t3_server::{
    adapters::config::{ConfigInputs, ServerFlags},
    runtime::RunningServer,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

pub type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub fn inputs(temp: &std::path::Path) -> ConfigInputs {
    ConfigInputs {
        flags: ServerFlags {
            base_dir: Some(temp.to_string_lossy().into_owned()),
            host: Some("127.0.0.1".into()),
            port: Some(0),
            ..Default::default()
        },
        env: Default::default(),
        home: temp.into(),
        process_cwd: temp.into(),
        bootstrap: None,
        headless: true,
        force_auto_bootstrap: Some(false),
        service_managed: false,
        cli_log_level: None,
        automatic_port: None,
        persisted_observability: Default::default(),
        static_dir: Some(temp.join("web")),
    }
}

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub runtime: RunningServer,
    pub http: reqwest::Client,
    pub base: String,
    /// `name=value` of the administrator browser session.
    pub cookie: String,
}

impl Fixture {
    pub async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("web")).unwrap();
        std::fs::write(temp.path().join("web/index.html"), "<html></html>").unwrap();
        let runtime = t3_server::runtime::start(inputs(temp.path()))
            .await
            .unwrap();
        let base = format!("http://{}", runtime.address);
        let http = reqwest::Client::new();
        let cookie = pair(&http, &base, &runtime.pairing_credential).await;
        Self {
            temp,
            runtime,
            http,
            base,
            cookie,
        }
    }
    pub fn ws_url(&self, query: &str) -> String {
        format!(
            "ws://{}/ws?orchestrationProtocol=2{query}",
            self.runtime.address
        )
    }
    /// Pairs a new browser session limited to `scopes`; returns its `name=value` cookie.
    pub async fn scoped_cookie(&self, scopes: &[&str]) -> String {
        let grant = self
            .http
            .post(format!("{}/api/auth/pairing-token", self.base))
            .header("cookie", &self.cookie)
            .json(&json!({"scopes": scopes}))
            .send()
            .await
            .unwrap();
        assert_eq!(grant.status(), 200);
        let grant = grant.json::<Value>().await.unwrap();
        pair(
            &self.http,
            &self.base,
            grant["credential"].as_str().unwrap(),
        )
        .await
    }
    pub async fn ticket(&self, cookie: &str, audience: &str) -> String {
        ticket(&self.http, &self.base, cookie, audience).await
    }
    /// Connects with a fresh RPC ticket for `cookie`.
    pub async fn connect(&self, cookie: &str, query: &str) -> (Socket, Client) {
        let ticket = self.ticket(cookie, "rpc").await;
        let (socket, _) =
            tokio_tungstenite::connect_async(self.ws_url(&format!("&wsTicket={ticket}{query}")))
                .await
                .unwrap();
        (socket, Client::new(0, Default::default()))
    }
    /// Connects through the cookie header and no ticket, with no incoming size limit.
    pub async fn connect_unlimited(&self, query: &str) -> (Socket, Client) {
        let mut request = self.ws_url(query).into_client_request().unwrap();
        request
            .headers_mut()
            .insert("cookie", self.cookie.parse().unwrap());
        let config = WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        let (socket, _) =
            tokio_tungstenite::connect_async_with_config(request, Some(config), false)
                .await
                .unwrap();
        (socket, Client::new(0, Default::default()))
    }
    pub async fn shutdown(self) {
        self.runtime.shutdown().await.unwrap();
    }
}

pub async fn pair(http: &reqwest::Client, base: &str, credential: &str) -> String {
    let response = http
        .post(format!("{base}/api/auth/browser-session"))
        .json(&json!({"credential": credential}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_string();
    cookie.split(';').next().unwrap().into()
}

pub async fn ticket(http: &reqwest::Client, base: &str, cookie: &str, audience: &str) -> String {
    let response = http
        .post(format!("{base}/api/auth/websocket-ticket/{audience}"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["ticket"]
        .as_str()
        .unwrap()
        .into()
}

pub async fn flush(socket: &mut Socket, client: &mut Client) {
    while let Some(m) = client.pop_outgoing() {
        let Frame::Text(t) = t3_rpc::codec::encode(&m).unwrap() else {
            panic!("text expected")
        };
        socket.send(Message::Text(t.into())).await.unwrap();
    }
}

/// The next frame the server sent, decoded to its envelopes.
pub async fn next_frame(socket: &mut Socket) -> Vec<Value> {
    loop {
        let value: Value = match socket.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            Message::Binary(b) => serde_json::from_slice(&b).unwrap(),
            Message::Ping(p) => {
                socket.send(Message::Pong(p)).await.unwrap();
                continue;
            }
            other => panic!("unexpected message {other:?}"),
        };
        return match value {
            Value::Array(items) => items,
            single => vec![single],
        };
    }
}

pub async fn event(socket: &mut Socket, client: &mut Client) -> CallEvent {
    loop {
        flush(socket, client).await;
        if let Some(event) = client.pop_event() {
            return event;
        }
        let message = socket.next().await.unwrap().unwrap();
        let frame = match message {
            Message::Text(t) => Frame::Text(t.to_string()),
            Message::Binary(b) => Frame::Binary(b.to_vec()),
            Message::Ping(p) => {
                socket.send(Message::Pong(p)).await.unwrap();
                continue;
            }
            other => panic!("connection closed before result: {other:?}"),
        };
        for message in t3_rpc::codec::decode(&frame).unwrap() {
            client.receive(message, 0);
        }
    }
}

pub async fn unary(socket: &mut Socket, client: &mut Client, tag: &str, payload: Value) -> Exit {
    let id = client.start(tag, payload, CallKind::Unary).unwrap();
    loop {
        match event(socket, client).await {
            CallEvent::Exit { id: actual, exit } => {
                assert_eq!(actual, id);
                return exit;
            }
            // A background provider-status update from an open config stream (see `chunk`).
            CallEvent::Chunk { id, values }
                if values
                    .iter()
                    .all(|value| value["type"] == "providerStatuses") =>
            {
                client.acknowledge(id);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// The success value of a unary call, panicking with the exit otherwise.
pub async fn unary_ok(
    socket: &mut Socket,
    client: &mut Client,
    tag: &str,
    payload: Value,
) -> Value {
    match unary(socket, client, tag, payload).await {
        Exit::Success { value } => value.unwrap_or(Value::Null),
        other => panic!("{tag} failed: {}", serde_json::to_string(&other).unwrap()),
    }
}

/// The wire form of an exit's first cause.
pub fn first_cause(exit: Exit) -> Value {
    serde_json::to_value(exit).unwrap()["cause"][0].clone()
}

/// The next stream chunk, skipping the server config stream's provider-status updates: provider
/// probes settle in the background after the snapshot, as they do in TypeScript, and no test
/// using this helper is about them (`composition_providers` asserts them with its own helper).
pub async fn chunk(socket: &mut Socket, client: &mut Client) -> Value {
    loop {
        match event(socket, client).await {
            CallEvent::Chunk { id, values } => {
                client.acknowledge(id);
                let value = values.into_iter().next().unwrap();
                if value["type"] != "providerStatuses" {
                    return value;
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
