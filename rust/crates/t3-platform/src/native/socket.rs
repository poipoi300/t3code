//! A reader task owns the receive half and buffers frames, so `receive` is cancel-safe: the RPC
//! driver drops a pending receive whenever it has a command to send.
use super::Timeouts;
use futures::{FutureExt, SinkExt, StreamExt, stream::SplitSink};
use std::{sync::Arc, time::Duration};
use t3_client::platform::{HostFuture, PlatformError, SocketRequest, WebSocket};
use t3_rpc::Frame;
use tokio::{net::TcpStream, sync::mpsc, task::JoinHandle};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{
        self, Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::SEC_WEBSOCKET_PROTOCOL},
        protocol::WebSocketConfig,
    },
};

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;
/// The server's `ws` default incoming limit; the RPC transport uses the same bound.
const MAX_MESSAGE_BYTES: usize = t3_rpc::transport::MAX_INCOMING_FRAME_BYTES;
const RECEIVE_BUFFER: usize = 64;

pub(crate) struct NativeSocket {
    sink: SplitSink<Stream, Message>,
    incoming: mpsc::Receiver<Result<Frame, PlatformError>>,
    reader: JoinHandle<()>,
    close_timeout: Duration,
    runtime: tokio::runtime::Handle,
    closed: bool,
}

fn socket_error(error: tungstenite::Error) -> PlatformError {
    match error {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            PlatformError::Closed
        }
        tungstenite::Error::Capacity(_) => PlatformError::TooLarge,
        // Handshake errors can echo the request URL, which carries the socket ticket.
        tungstenite::Error::Http(response) => {
            PlatformError::Network(format!("WebSocket upgrade rejected: {}", response.status()))
        }
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
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
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
    .map_err(|_| PlatformError::Network("WebSocket connect timed out".into()))?
    .map_err(socket_error)?;
    // Browsers fail a connection whose server picked a protocol the client did not offer.
    if let Some(selected) = response.headers().get(SEC_WEBSOCKET_PROTOCOL) {
        let selected = selected.to_str().unwrap_or_default();
        if !request.protocols.iter().any(|offered| offered == selected) {
            return Err(PlatformError::Network(
                "server selected an unoffered WebSocket protocol".into(),
            ));
        }
    }
    let (sink, mut source) = stream.split();
    let (tx, incoming) = mpsc::channel(RECEIVE_BUFFER);
    let reader = tokio::spawn(async move {
        while let Some(message) = source.next().await {
            let frame = match message {
                Ok(Message::Text(text)) => Ok(Frame::Text(text.as_str().to_owned())),
                Ok(Message::Binary(bytes)) => Ok(Frame::Binary(bytes.to_vec())),
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => continue,
                Err(error) => Err(socket_error(error)),
            };
            let failed = frame.is_err();
            if tx.send(frame).await.is_err() || failed {
                break;
            }
        }
    });
    Ok(NativeSocket {
        sink,
        incoming,
        reader,
        close_timeout: timeouts.close,
        // `connect` runs as a runtime task, so this is the platform's runtime.
        runtime: tokio::runtime::Handle::current(),
        closed: false,
    })
}

impl WebSocket for NativeSocket {
    fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
        async move {
            if self.closed {
                return Err(PlatformError::Closed);
            }
            let message = match frame {
                Frame::Text(text) => Message::text(text),
                Frame::Binary(bytes) => Message::binary(bytes),
            };
            self.sink.send(message).await.map_err(socket_error)
        }
        .boxed_local()
    }

    fn receive(&mut self) -> HostFuture<'_, Option<Frame>> {
        async move {
            if self.closed {
                return Ok(None);
            }
            self.incoming.recv().await.transpose()
        }
        .boxed_local()
    }

    /// Sends a close frame and waits for the peer's, bounded by the close timeout.
    fn close(&mut self) -> HostFuture<'_, ()> {
        async move {
            if std::mem::replace(&mut self.closed, true) {
                return Ok(());
            }
            self.incoming.close();
            let handshake = async {
                let _ = self.sink.send(Message::Close(None)).await;
                let _ = self.sink.flush().await;
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
