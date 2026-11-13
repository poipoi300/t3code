#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_acp::{
    Client, ClientHandler, ClientOptions, ClientRequest, Events, RequestContext, RpcError,
    async_trait,
};
use tokio::{
    io::{
        AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
    },
    sync::{Semaphore, mpsc},
};

fn requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "session/request_permission",
            json!({"sessionId":"s","toolCall":{"toolCallId":"t"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]}),
        ),
        (
            "fs/read_text_file",
            json!({"sessionId":"s","path":"/never-read"}),
        ),
        (
            "fs/write_text_file",
            json!({"sessionId":"s","path":"/never-write","content":"never"}),
        ),
        (
            "terminal/create",
            json!({"sessionId":"s","command":"never-execute"}),
        ),
        (
            "terminal/output",
            json!({"sessionId":"s","terminalId":"never"}),
        ),
        (
            "terminal/wait_for_exit",
            json!({"sessionId":"s","terminalId":"never"}),
        ),
        (
            "terminal/kill",
            json!({"sessionId":"s","terminalId":"never"}),
        ),
        (
            "terminal/release",
            json!({"sessionId":"s","terminalId":"never"}),
        ),
    ]
}
struct Guard(mpsc::UnboundedSender<()>);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}
struct Boundary {
    entered: mpsc::UnboundedSender<()>,
    dropped: mpsc::UnboundedSender<()>,
    gate: Semaphore,
    effects: AtomicUsize,
    panic: bool,
}
#[async_trait]
impl ClientHandler for Boundary {
    async fn request(&self, _: ClientRequest, _: RequestContext) -> Result<Value, RpcError> {
        let _guard = Guard(self.dropped.clone());
        self.entered.send(()).unwrap();
        assert!(!self.panic, "scripted handler panic before any effect");
        let _permit = self.gate.acquire().await.unwrap();
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"outcome":{"outcome":"selected","optionId":"allow"}}))
    }
}
type Connection = (
    Client,
    Events,
    BufReader<ReadHalf<DuplexStream>>,
    WriteHalf<DuplexStream>,
    Arc<Boundary>,
    mpsc::UnboundedReceiver<()>,
    mpsc::UnboundedReceiver<()>,
);

fn connect(panic: bool) -> Connection {
    let (pipe, peer) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(pipe);
    let (pr, pw) = tokio::io::split(peer);
    let (entered, rx) = mpsc::unbounded_channel();
    let (dropped, dr) = mpsc::unbounded_channel();
    let boundary = Arc::new(Boundary {
        entered,
        dropped,
        gate: Semaphore::new(0),
        effects: AtomicUsize::new(0),
        panic,
    });
    let (c, e) = Client::connect(r, w, boundary.clone(), ClientOptions::default());
    (c, e, BufReader::new(pr), pw, boundary, rx, dr)
}
async fn send(writer: &mut WriteHalf<DuplexStream>, frame: Value) {
    let mut bytes = serde_json::to_vec(&frame).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}
async fn receive(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Value {
    let mut line = String::new();
    assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn drop_shutdown_parse_failure_and_cancellation_never_approve_or_run_effects() {
    for mode in ["drop", "shutdown", "malformed", "cancel", "request-drop"] {
        let (client, mut events, mut reader, mut writer, boundary, mut entered, mut dropped) =
            connect(false);
        let requests = requests();
        for (id, (method, params)) in requests.iter().enumerate() {
            send(
                &mut writer,
                json!({"id":id,"method":method,"params":params}),
            )
            .await;
            entered.recv().await.unwrap();
        }
        if mode == "cancel" {
            client.cancel(json!({"sessionId":"s"})).await.unwrap();
            assert_eq!(
                receive(&mut reader).await,
                json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s"}})
            );
            let c = client.clone();
            let barrier = tokio::spawn(async move { c.request("_barrier", json!({})).await });
            let frame = receive(&mut reader).await;
            assert_eq!(frame["method"], "_barrier");
            assert!(frame.get("result").is_none());
            send(&mut writer, json!({"id":frame["id"],"result":null})).await;
            barrier.await.unwrap().unwrap();
        }
        if mode == "request-drop" {
            let c = client.clone();
            let pending = tokio::spawn(async move { c.request("_pending", json!({})).await });
            let frame = receive(&mut reader).await;
            assert_eq!(frame["method"], "_pending");
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
            send(
                &mut writer,
                json!({"id":frame["id"],"result":{"approved":true}}),
            )
            .await;
            let c = client.clone();
            let barrier = tokio::spawn(async move { c.request("_barrier", json!({})).await });
            let frame = receive(&mut reader).await;
            assert_eq!(frame["method"], "_barrier");
            send(&mut writer, json!({"id":frame["id"],"result":null})).await;
            barrier.await.unwrap().unwrap();
        }
        match mode {
            "drop" => {
                drop(client);
                drop(events);
            }
            "malformed" => {
                writer.write_all(b"{malformed}\n").await.unwrap();
                assert_eq!(events.next().await, Some(Err(t3_acp::Error::InvalidFrame)));
                drop(events);
                drop(client);
            }
            _ => {
                client.shutdown().await;
                drop(events);
                drop(client);
            }
        }
        for _ in 0..requests.len() {
            dropped.recv().await.unwrap();
        }
        boundary.gate.add_permits(requests.len());
        let mut output = Vec::new();
        reader.read_to_end(&mut output).await.unwrap();
        assert!(output.is_empty(), "{mode}: unexpected response");
        assert_eq!(boundary.effects.load(Ordering::SeqCst), 0, "{mode}");
    }
}

#[tokio::test]
async fn malformed_parameters_and_panics_return_only_errors_and_do_not_run_effects() {
    for panic in [false, true] {
        let (client, _events, mut reader, mut writer, boundary, mut entered, mut dropped) =
            connect(panic);
        for (id, (method, valid)) in requests().into_iter().enumerate() {
            let params = if panic {
                valid
            } else {
                json!({"private":"do not disclose"})
            };
            send(
                &mut writer,
                json!({"id":id,"method":method,"params":params}),
            )
            .await;
            let response = receive(&mut reader).await;
            assert_eq!(
                response["error"]["code"],
                if panic { -32603 } else { -32602 }
            );
            assert!(response.get("result").is_none());
            assert!(!response.to_string().contains("do not disclose"));
            if panic {
                entered.recv().await.unwrap();
                dropped.recv().await.unwrap();
            } else {
                assert!(entered.try_recv().is_err());
            }
        }
        assert_eq!(boundary.effects.load(Ordering::SeqCst), 0);
        client.shutdown().await;
    }
}
