#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    task::{Context, Poll, Waker},
};
use t3_acp::{
    Client, ClientHandler, ClientOptions, ClientRequest, Error, Event, RequestContext, RequestId,
    RpcError, async_trait,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream},
    sync::mpsc,
};

struct Gate {
    state: AtomicU8,
    waker: Mutex<Option<Waker>>,
}
impl Gate {
    fn release(&self, state: u8) {
        self.state.store(state, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}
struct Writer {
    pipe: DuplexStream,
    gate: Arc<Gate>,
    started: Option<mpsc::UnboundedSender<()>>,
}
impl AsyncWrite for Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.pipe).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(started) = self.started.take() {
            started.send(()).unwrap();
        }
        *self.gate.waker.lock().unwrap() = Some(cx.waker().clone());
        match self.gate.state.load(Ordering::Acquire) {
            0 => Poll::Pending,
            1 => Pin::new(&mut self.pipe).poll_flush(cx),
            _ => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "scripted flush failure",
            ))),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.pipe).poll_shutdown(cx)
    }
}
struct Handler;
#[async_trait]
impl ClientHandler for Handler {
    async fn request(&self, _: ClientRequest, _: RequestContext) -> Result<Value, RpcError> {
        Ok(json!({"ok":true}))
    }
}

#[tokio::test]
async fn response_acknowledges_only_after_flush_and_failed_flush_terminates() {
    for success in [true, false] {
        let (incoming, mut agent) = tokio::io::duplex(4096);
        let (outgoing, output) = tokio::io::duplex(4096);
        let gate = Arc::new(Gate {
            state: AtomicU8::new(0),
            waker: Mutex::new(None),
        });
        let (tx, mut started) = mpsc::unbounded_channel();
        let writer = Writer {
            pipe: outgoing,
            gate: gate.clone(),
            started: Some(tx),
        };
        let (client, mut events) = Client::connect(
            incoming,
            writer,
            Arc::new(Handler),
            ClientOptions::default(),
        );
        agent
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"_echo\",\"params\":{}}\n")
            .await
            .unwrap();
        started.recv().await.unwrap();
        let mut output = BufReader::new(output);
        let mut line = String::new();
        output.read_line(&mut line).await.unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["result"], json!({"ok":true}));
        let mut next = Box::pin(events.next());
        assert!(std::future::poll_fn(|cx| Poll::Ready(next.as_mut().poll(cx).is_pending())).await);
        gate.release(if success { 1 } else { 2 });
        let event = next.await.unwrap();
        if success {
            assert_eq!(
                event.unwrap(),
                Event::ResponseSent {
                    id: RequestId::Number(0)
                }
            );
        } else {
            assert!(matches!(
                event,
                Err(Error::Io {
                    operation: "flush",
                    ..
                })
            ));
        }
        client.shutdown().await;
    }
}
