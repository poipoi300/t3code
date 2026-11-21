use std::{cell::RefCell, rc::Rc};

use super::{
    js::{self, FirstEvent, Listeners, remove_listeners},
    timer,
};
use crate::socket_ingress::{self, BufferedReceiver, BufferedSender, Incoming, Item, Limits};
use futures::{
    FutureExt,
    future::{Either, select},
};
use js_sys::{Array, ArrayBuffer, JsString, Uint8Array};
use t3_client::platform::{HostFuture, PlatformError, SocketClose, SocketRequest, WebSocket};
use t3_rpc::Frame;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{BinaryType, CloseEvent, MessageEvent};

const CLOSE_TIMEOUT_MS: u64 = 5_000;

pub(crate) struct BrowserSocket {
    socket: web_sys::WebSocket,
    window: web_sys::Window,
    incoming: Option<BufferedReceiver>,
    events: Rc<RefCell<BrowserEvents>>,
    peer_close: Option<SocketClose>,
}

struct BrowserEvents {
    ingress: Option<BufferedSender>,
    listeners: Listeners,
}

impl BrowserEvents {
    fn finish(&mut self, terminal: Option<Item>) {
        if let (Some(mut sender), Some(item)) = (self.ingress.take(), terminal) {
            let _ = sender.send(item);
        }
        remove_listeners(&mut self.listeners);
    }
}

fn close_status(event: &web_sys::Event) -> SocketClose {
    event
        .dyn_ref::<CloseEvent>()
        .map_or(SocketClose::ABNORMAL, |event| SocketClose {
            code: event.code(),
            reason: event.reason(),
        })
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
    let limits = Limits::for_url(&url);
    let (ingress, incoming) = socket_ingress::buffered(limits.preview);
    let browser = BrowserSocket {
        socket,
        window,
        incoming: Some(incoming),
        events: Rc::new(RefCell::new(BrowserEvents {
            ingress: Some(ingress),
            listeners: Vec::new(),
        })),
        peer_close: None,
    };
    let socket = &browser.socket;
    let events = Rc::downgrade(&browser.events);
    let connection = socket.clone();
    let message = js::listen(socket, "message", move |event| {
        let (Some(events), Some(event)) = (events.upgrade(), event.dyn_ref::<MessageEvent>())
        else {
            return;
        };
        let data = event.data();
        let frame = if let Some(text) = data.dyn_ref::<JsString>() {
            limits.check(text.length() as usize, false).and_then(|()| {
                let text = String::from(text);
                limits.check(text.len(), false).map(|()| Frame::Text(text))
            })
        } else if let Some(buffer) = data.dyn_ref::<ArrayBuffer>() {
            let bytes = Uint8Array::new(buffer);
            limits
                .check(bytes.length() as usize, true)
                .map(|()| Frame::Binary(bytes.to_vec()))
        } else {
            return;
        };
        let mut events = events.borrow_mut();
        let result = frame.and_then(|frame| {
            events
                .ingress
                .as_mut()
                .ok_or(PlatformError::Closed)?
                .send(Ok(Incoming::Frame(frame)))
        });
        if let Err(error) = result {
            events.finish(Some(Err(error)));
            let _ = connection.close();
        }
    })?;
    browser.events.borrow_mut().listeners.push(message);
    let connection = socket.clone();
    let error = js::listen(socket, "error", move |_| {
        // The browser dispatches close after error. Keep its listener to receive the code.
        if connection.ready_state() < web_sys::WebSocket::CLOSING {
            let _ = connection.close();
        }
    })?;
    browser.events.borrow_mut().listeners.push(error);
    let events = Rc::downgrade(&browser.events);
    let close = js::listen(socket, "close", move |event| {
        if let Some(events) = events.upgrade() {
            events
                .borrow_mut()
                .finish(Some(Ok(Incoming::Close(close_status(&event)))));
        }
    })?;
    browser.events.borrow_mut().listeners.push(close);
    let opened = FirstEvent::new(
        socket,
        vec![
            ("open", Box::new(|_: &web_sys::Event| Ok(()))),
            (
                "close",
                Box::new(|event: &web_sys::Event| {
                    Err(PlatformError::SocketHandshakeClosed {
                        close: close_status(event),
                        http_status: None,
                    })
                }),
            ),
        ],
    )?;
    opened.await??;
    Ok(browser)
}

impl WebSocket for BrowserSocket {
    fn close_info(&self) -> Option<&SocketClose> {
        self.peer_close.as_ref()
    }

    fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
        async move {
            if self.incoming.is_none() || self.socket.ready_state() != web_sys::WebSocket::OPEN {
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
            let Some(incoming) = self.incoming.as_mut() else {
                return Ok(None);
            };
            match incoming.next().await {
                Some(Ok(Incoming::Frame(frame))) => Ok(Some(frame)),
                item => {
                    self.incoming = None;
                    if let Some(Incoming::Close(close)) = item.transpose()? {
                        self.peer_close = Some(close);
                    }
                    Ok(None)
                }
            }
        }
        .boxed_local()
    }

    fn close(&mut self) -> HostFuture<'_, ()> {
        async move {
            self.incoming = None;
            self.events.borrow_mut().finish(None);
            if self.socket.ready_state() >= web_sys::WebSocket::CLOSING {
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
        self.events.borrow_mut().finish(None);
        if matches!(
            self.socket.ready_state(),
            web_sys::WebSocket::CONNECTING | web_sys::WebSocket::OPEN
        ) {
            let _ = self.socket.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen::prelude::wasm_bindgen;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen(inline_js = "
        const original = globalThis.WebSocket;
        let last;
        export function installSocket() {
            globalThis.WebSocket = class extends EventTarget {
                static CONNECTING = 0;
                static OPEN = 1;
                static CLOSING = 2;
                static CLOSED = 3;
                readyState = 0;
                listeners = new Set();
                constructor(url) {
                    super();
                    last = this;
                    queueMicrotask(() => {
                        if (url.endsWith('/refuse')) {
                            this.readyState = 3;
                            this.dispatchEvent(new Event('error'));
                            this.dispatchEvent(new CloseEvent('close', {code: 1006}));
                            return;
                        }
                        this.readyState = 1;
                        this.dispatchEvent(new Event('open'));
                    });
                }
                addEventListener(name, callback) {
                    super.addEventListener(name, callback);
                    this.listeners.add(callback);
                }
                removeEventListener(name, callback) {
                    super.removeEventListener(name, callback);
                    this.listeners.delete(callback);
                }
                close() {
                    this.readyState = 2;
                    queueMicrotask(() => {
                        this.readyState = 3;
                        this.dispatchEvent(new CloseEvent('close', {code: 1000}));
                    });
                }
            };
        }
        export function restoreSocket() { globalThis.WebSocket = original; }
        export function endSocket(kind, code, reason) {
            if (kind === 'close') {
                last.readyState = 3;
                last.dispatchEvent(new CloseEvent(kind, {code, reason}));
            } else {
                last.dispatchEvent(new Event(kind));
            }
        }
        export function listenerCount() { return last.listeners.size; }
        export function sendBinary(bytes) {
            last.dispatchEvent(new MessageEvent('message', {data: new ArrayBuffer(bytes)}));
        }
        export function sendText(character, count) {
            last.dispatchEvent(new MessageEvent('message', {data: character.repeat(count)}));
        }
    ")]
    extern "C" {
        #[wasm_bindgen(js_name = installSocket)]
        fn install_socket();
        #[wasm_bindgen(js_name = restoreSocket)]
        fn restore_socket();
        #[wasm_bindgen(js_name = endSocket)]
        fn end_socket(kind: &str, code: u16, reason: &str);
        #[wasm_bindgen(js_name = listenerCount)]
        fn listener_count() -> usize;
        #[wasm_bindgen(js_name = sendBinary)]
        fn send_binary(bytes: usize);
        #[wasm_bindgen(js_name = sendText)]
        fn send_text(character: &str, count: usize);
    }

    struct RestoreSocket;
    impl Drop for RestoreSocket {
        fn drop(&mut self) {
            restore_socket();
        }
    }

    #[wasm_bindgen_test]
    async fn browser_socket_terminal_events_and_ingress() {
        install_socket();
        let _restore = RestoreSocket;
        // Node supplies EventTarget and CloseEvent. Its global also exposes the timers.
        let window: web_sys::Window = js_sys::global().unchecked_into();
        for code in [1000, 1008, 4401, 4404, 4503] {
            let mut socket = test_socket(&window).await;
            send_binary(1);
            end_socket("close", code, "peer reason");
            assert_eq!(
                socket.receive().now_or_never(),
                Some(Ok(Some(Frame::Binary(vec![0]))))
            );
            assert_eq!(socket.receive().now_or_never(), Some(Ok(None)));
            let close = socket.close_info().expect("peer close information");
            assert_eq!((close.code, close.reason.as_str()), (code, "peer reason"));
            assert_eq!(listener_count(), 0);
        }
        let mut socket = test_socket(&window).await;
        end_socket("error", 0, "");
        end_socket("close", 1006, "");
        assert_eq!(socket.receive().now_or_never(), Some(Ok(None)));
        assert_eq!(
            socket.close_info(),
            Some(&SocketClose {
                code: 1006,
                reason: String::new()
            })
        );
        assert_eq!(listener_count(), 0);
        let failure = connect(
            window.clone(),
            SocketRequest {
                url: "wss://host/refuse".into(),
                protocols: vec![],
            },
        )
        .await;
        assert!(
            matches!(failure, Err(PlatformError::SocketHandshakeClosed { close, http_status: None })
            if close.code == 1006 && close.reason.is_empty())
        );
        assert_eq!(listener_count(), 0);

        let mut socket = test_socket(&window).await;
        socket.close().await.unwrap();
        socket.close().await.unwrap();
        assert_eq!(socket.receive().now_or_never(), Some(Ok(None)));
        assert_eq!(listener_count(), 0);

        for overflow in [
            || send_binary(12 * 1024 * 1024 + 1),
            || send_text("a", 256 * 1024 + 1),
            || send_text("é", 128 * 1024 + 1),
        ] {
            let mut socket = test_socket(&window).await;
            overflow();
            assert!(matches!(
                socket.receive().now_or_never(),
                Some(Err(PlatformError::TooLarge))
            ));
            assert_eq!(socket.receive().now_or_never(), Some(Ok(None)));
            assert_eq!(listener_count(), 0);
        }
    }

    #[wasm_bindgen_test]
    async fn preview_control_burst_keeps_stream_and_latest_image() {
        install_socket();
        let _restore = RestoreSocket;
        let mut socket = test_socket(&js_sys::global().unchecked_into()).await;
        for i in 1..=64 {
            send_text("x", i);
        }
        for i in 1..=64 {
            assert_eq!(
                socket.receive().now_or_never(),
                Some(Ok(Some(Frame::Text("x".repeat(i)))))
            );
        }
        assert_eq!(socket.receive().now_or_never(), None);
        send_binary(1);
        send_text("between", 1);
        send_binary(2);
        assert_eq!(
            socket.receive().now_or_never(),
            Some(Ok(Some(Frame::Text("between".into()))))
        );
        assert_eq!(
            socket.receive().now_or_never(),
            Some(Ok(Some(Frame::Binary(vec![0; 2]))))
        );
        assert_eq!(socket.receive().now_or_never(), None);
        send_text("live", 1);
        assert_eq!(
            socket.receive().now_or_never(),
            Some(Ok(Some(Frame::Text("live".into()))))
        );
    }

    async fn test_socket(window: &web_sys::Window) -> BrowserSocket {
        connect(
            window.clone(),
            SocketRequest {
                url: "wss://host/api/preview-stream/ws".into(),
                protocols: vec![],
            },
        )
        .await
        .unwrap()
    }
}
