use super::{
    js::{self, FirstEvent, Listeners, remove_listeners},
    timer,
};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc,
    future::{Either, select},
};
use js_sys::{Array, ArrayBuffer, Uint8Array};
use t3_client::platform::{HostFuture, PlatformError, SocketRequest, WebSocket};
use t3_rpc::Frame;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{BinaryType, CloseEvent, MessageEvent};

const CLOSE_TIMEOUT_MS: u64 = 5_000;

pub(crate) struct BrowserSocket {
    socket: web_sys::WebSocket,
    window: web_sys::Window,
    incoming: mpsc::UnboundedReceiver<Result<Frame, PlatformError>>,
    listeners: Listeners,
    closed: bool,
}

pub(crate) async fn connect(
    window: web_sys::Window,
    request: SocketRequest,
) -> Result<BrowserSocket, PlatformError> {
    let url = url::Url::parse(&request.url)
        .map_err(|_| PlatformError::InvalidData("WebSocket URL".into()))?;
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err(PlatformError::InvalidData("WebSocket URL scheme".into()));
    }
    let protocols = request
        .protocols
        .iter()
        .map(|p| JsValue::from_str(p))
        .collect::<Array>();
    let socket =
        web_sys::WebSocket::new_with_str_sequence(url.as_str(), &protocols).map_err(|error| {
            PlatformError::Network(format!("WebSocket: {}", js::error_name(&error)))
        })?;
    socket.set_binary_type(BinaryType::Arraybuffer);
    let (sender, incoming) = mpsc::unbounded();
    let mut listeners: Listeners = Vec::new();
    let frames = sender.clone();
    listeners.push(js::listen(&socket, "message", move |event| {
        let Some(event) = event.dyn_ref::<MessageEvent>() else {
            return;
        };
        let data = event.data();
        let frame = if let Some(text) = data.as_string() {
            Frame::Text(text)
        } else if let Some(buffer) = data.dyn_ref::<ArrayBuffer>() {
            Frame::Binary(Uint8Array::new(buffer).to_vec())
        } else {
            return;
        };
        let _ = frames.unbounded_send(Ok(frame));
    })?);
    let errors = sender.clone();
    listeners.push(js::listen(&socket, "error", move |_| {
        let _ = errors.unbounded_send(Err(PlatformError::Network("WebSocket error".into())));
    })?);
    let mut ending = Some(sender);
    listeners.push(js::listen(&socket, "close", move |_| {
        ending.take();
    })?);
    let opened = FirstEvent::new(
        &socket,
        vec![
            ("open", Box::new(|_: &web_sys::Event| Ok(()))),
            (
                "close",
                Box::new(|event: &web_sys::Event| {
                    let code = event.dyn_ref::<CloseEvent>().map_or(0, CloseEvent::code);
                    Err(PlatformError::Network(format!(
                        "WebSocket closed during handshake ({code})"
                    )))
                }),
            ),
        ],
    )?;
    let socket = BrowserSocket {
        socket,
        window,
        incoming,
        listeners,
        closed: false,
    };
    // Dropping `socket` on failure removes the listeners and closes the connection.
    opened.await??;
    Ok(socket)
}

impl WebSocket for BrowserSocket {
    fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
        async move {
            if self.closed || self.socket.ready_state() != web_sys::WebSocket::OPEN {
                return Err(PlatformError::Closed);
            }
            match frame {
                Frame::Text(text) => self.socket.send_with_str(&text),
                Frame::Binary(bytes) => self.socket.send_with_u8_array(&bytes),
            }
            .map_err(|error| {
                PlatformError::Network(format!("WebSocket send: {}", js::error_name(&error)))
            })
        }
        .boxed_local()
    }

    fn receive(&mut self) -> HostFuture<'_, Option<Frame>> {
        async move {
            if self.closed {
                return Ok(None);
            }
            self.incoming.next().await.transpose()
        }
        .boxed_local()
    }

    fn close(&mut self) -> HostFuture<'_, ()> {
        async move {
            if std::mem::replace(&mut self.closed, true) {
                return Ok(());
            }
            let closed = FirstEvent::new(
                &self.socket,
                vec![("close", Box::new(|_: &web_sys::Event| ()))],
            )?;
            let _ = self.socket.close();
            let timeout = timer::sleep(self.window.clone(), CLOSE_TIMEOUT_MS).boxed_local();
            match select(closed, timeout).await {
                Either::Left(_) | Either::Right(_) => Ok(()),
            }
        }
        .boxed_local()
    }
}

impl Drop for BrowserSocket {
    fn drop(&mut self) {
        remove_listeners(&mut self.listeners);
        if matches!(
            self.socket.ready_state(),
            web_sys::WebSocket::CONNECTING | web_sys::WebSocket::OPEN
        ) {
            let _ = self.socket.close();
        }
    }
}
