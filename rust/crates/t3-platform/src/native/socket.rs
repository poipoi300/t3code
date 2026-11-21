//! A reader task owns the receive half and buffers frames, so `receive` is cancel-safe: the RPC
//! driver drops a pending receive whenever it has a command to send.
use super::Timeouts;
use crate::socket_ingress::{self, BufferedReceiver, BufferedSender, Incoming, Item, Limits};
use futures::{FutureExt, SinkExt, StreamExt, stream::SplitSink};
use std::{sync::Arc, time::Duration};
use t3_client::platform::{HostFuture, PlatformError, SocketClose, SocketRequest, WebSocket};
use t3_rpc::Frame;
use tokio::{
    net::TcpStream,
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{
        self, Message,
        client::IntoClientRequest,
        error::ProtocolError,
        http::{HeaderValue, header::SEC_WEBSOCKET_PROTOCOL},
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WriteHalf = Mutex<SplitSink<Stream, Message>>;
const RECEIVE_BUFFER: usize = 64;

enum ReceiveQueue {
    Rpc(mpsc::Receiver<Item>),
    Preview(BufferedReceiver),
}

enum SendQueue {
    Rpc(mpsc::Sender<Item>),
    Preview(BufferedSender),
}

impl SendQueue {
    async fn send(&mut self, item: Item) -> Result<(), PlatformError> {
        match self {
            Self::Rpc(sender) => sender.send(item).await.map_err(|_| PlatformError::Closed),
            Self::Preview(sender) => sender.send(item),
        }
    }
}

pub(crate) struct NativeSocket {
    sink: Arc<WriteHalf>,
    incoming: Option<ReceiveQueue>,
    reader: JoinHandle<()>,
    close_timeout: Duration,
    runtime: tokio::runtime::Handle,
    closed: bool,
    peer_close: Option<SocketClose>,
}

fn socket_error(error: tungstenite::Error) -> PlatformError {
    match error {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            PlatformError::Closed
        }
        tungstenite::Error::Capacity(_) => PlatformError::TooLarge,
        tungstenite::Error::Url(_) => PlatformError::InvalidData("WebSocket URL".into()),
        other => PlatformError::Network(format!("WebSocket failed: {}", error_kind(&other))),
    }
}
fn error_kind(error: &tungstenite::Error) -> &'static str {
    match error {
        tungstenite::Error::Io(_) => "I/O error",
        tungstenite::Error::Tls(_) => "TLS error",
        tungstenite::Error::Protocol(_) => "protocol error",
        tungstenite::Error::Utf8(_) => "invalid UTF-8",
        _ => "transport error",
    }
}

fn handshake_closed(http_status: Option<u16>) -> PlatformError {
    PlatformError::SocketHandshakeClosed {
        close: SocketClose::ABNORMAL,
        http_status,
    }
}

fn receive_error(error: tungstenite::Error) -> Item {
    use std::io::ErrorKind;
    match error {
        tungstenite::Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)
        | tungstenite::Error::ConnectionClosed
        | tungstenite::Error::AlreadyClosed => Ok(Incoming::Close(SocketClose::ABNORMAL)),
        tungstenite::Error::Io(ref error)
            if matches!(
                error.kind(),
                ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::BrokenPipe
            ) =>
        {
            Ok(Incoming::Close(SocketClose::ABNORMAL))
        }
        other => Err(socket_error(other)),
    }
}

async fn send_close(sink: &WriteHalf, frame: Option<CloseFrame>) {
    let mut sink = sink.lock().await;
    let _ = sink.send(Message::Close(frame)).await;
    let _ = sink.flush().await;
}

pub(crate) async fn connect(
    request: SocketRequest,
    tls: Arc<rustls::ClientConfig>,
    timeouts: Timeouts,
) -> Result<NativeSocket, PlatformError> {
    let url = url::Url::parse(&request.url)
        .map_err(|_| PlatformError::InvalidData("WebSocket URL".into()))?;
    if !matches!(url.scheme(), "ws" | "wss") || url.host_str().is_none() {
        return Err(PlatformError::InvalidData("WebSocket URL scheme".into()));
    }
    let mut upgrade = url
        .as_str()
        .into_client_request()
        .map_err(|_| PlatformError::InvalidData("WebSocket URL".into()))?;
    if !request.protocols.is_empty() {
        upgrade.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_str(&request.protocols.join(", "))
                .map_err(|_| PlatformError::InvalidData("WebSocket protocol".into()))?,
        );
    }
    let limits = Limits::for_url(&url);
    // Tungstenite has one assembly limit for both opcodes, so allow the larger
    // JPEG size here and enforce the text limit before copying or queuing it.
    let config = WebSocketConfig::default()
        .max_message_size(Some(limits.binary))
        .max_frame_size(Some(limits.binary));
    let (stream, response) = tokio::time::timeout(
        timeouts.connect,
        tokio_tungstenite::connect_async_tls_with_config(
            upgrade,
            Some(config),
            false,
            Some(Connector::Rustls(tls)),
        ),
    )
    .await
    .map_err(|_| handshake_closed(None))?
    .map_err(|error| {
        // Upgrade errors can carry the ticket URL. Preserve only the numeric status.
        let status = match error {
            tungstenite::Error::Http(response) => Some(response.status().as_u16()),
            _ => None,
        };
        handshake_closed(status)
    })?;
    // Browsers fail a connection whose server picked a protocol the client did not offer.
    if let Some(selected) = response.headers().get(SEC_WEBSOCKET_PROTOCOL) {
        let selected = selected.to_str().unwrap_or_default();
        if !request.protocols.iter().any(|offered| offered == selected) {
            return Err(handshake_closed(None));
        }
    }
    let (sink, mut source) = stream.split();
    let sink = Arc::new(Mutex::new(sink));
    let (mut tx, incoming) = if limits.preview {
        let (tx, rx) = socket_ingress::buffered(true);
        (SendQueue::Preview(tx), ReceiveQueue::Preview(rx))
    } else {
        let (tx, rx) = mpsc::channel(RECEIVE_BUFFER);
        (SendQueue::Rpc(tx), ReceiveQueue::Rpc(rx))
    };
    let reader_sink = sink.clone();
    let reader = tokio::spawn(async move {
        loop {
            let frame = match source.next().await {
                None => Ok(Incoming::Close(SocketClose::ABNORMAL)),
                Some(Ok(Message::Text(text))) => limits
                    .check(text.len(), false)
                    .map(|()| Incoming::Frame(Frame::Text(text.as_str().to_owned()))),
                Some(Ok(Message::Binary(bytes))) => {
                    Ok(Incoming::Frame(Frame::Binary(bytes.to_vec())))
                }
                Some(Ok(Message::Close(close))) => Ok(Incoming::Close(close.map_or(
                    SocketClose {
                        code: 1005,
                        reason: String::new(),
                    },
                    |close| SocketClose {
                        code: close.code.into(),
                        reason: close.reason.to_string(),
                    },
                ))),
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                Some(Err(error)) => receive_error(error),
            };
            if matches!(frame, Err(PlatformError::TooLarge)) {
                let close = CloseFrame {
                    code: CloseCode::Size,
                    reason: "message too large".into(),
                };
                let _ = tokio::time::timeout(timeouts.close, send_close(&reader_sink, Some(close)))
                    .await;
            }
            let terminal = !matches!(frame, Ok(Incoming::Frame(_)));
            match tx.send(frame).await {
                Ok(()) if !terminal => continue,
                Err(PlatformError::SocketBacklogOverflow) => {
                    let _ =
                        tokio::time::timeout(timeouts.close, send_close(&reader_sink, None)).await;
                    let _ = tx.send(Err(PlatformError::SocketBacklogOverflow)).await;
                }
                _ => {}
            }
            break;
        }
    });
    Ok(NativeSocket {
        sink,
        incoming: Some(incoming),
        reader,
        close_timeout: timeouts.close,
        // `connect` runs as a runtime task, so this is the platform's runtime.
        runtime: tokio::runtime::Handle::current(),
        closed: false,
        peer_close: None,
    })
}

impl WebSocket for NativeSocket {
    fn close_info(&self) -> Option<&SocketClose> {
        self.peer_close.as_ref()
    }

    fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
        async move {
            if self.incoming.is_none() {
                return Err(PlatformError::Closed);
            }
            let message = match frame {
                Frame::Text(text) => Message::text(text),
                Frame::Binary(bytes) => Message::binary(bytes),
            };
            self.sink
                .lock()
                .await
                .send(message)
                .await
                .map_err(socket_error)
        }
        .boxed_local()
    }

    fn receive(&mut self) -> HostFuture<'_, Option<Frame>> {
        async move {
            let item = match &mut self.incoming {
                Some(ReceiveQueue::Rpc(queue)) => queue.recv().await,
                Some(ReceiveQueue::Preview(queue)) => queue.next().await,
                None => None,
            };
            match item {
                Some(Ok(Incoming::Frame(frame))) => Ok(Some(frame)),
                terminal => {
                    self.incoming = None;
                    if let Some(Incoming::Close(close)) = terminal.transpose()? {
                        self.peer_close = Some(close);
                    }
                    Ok(None)
                }
            }
        }
        .boxed_local()
    }

    /// Sends a close frame and waits for the peer's, bounded by the close timeout.
    fn close(&mut self) -> HostFuture<'_, ()> {
        async move {
            if std::mem::replace(&mut self.closed, true) {
                return Ok(());
            }
            self.incoming = None;
            let handshake = async {
                send_close(&self.sink, None).await;
                let _ = (&mut self.reader).await;
            };
            // The timer binds to the runtime even when the caller polls from another executor.
            let bounded = {
                let _runtime = self.runtime.enter();
                tokio::time::timeout(self.close_timeout, handshake)
            };
            let _ = bounded.await;
            self.reader.abort();
            Ok(())
        }
        .boxed_local()
    }
}

impl Drop for NativeSocket {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
