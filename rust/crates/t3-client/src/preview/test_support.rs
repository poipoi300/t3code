//! A scriptable browser socket for previews exercised through the client service.

use std::{cell::RefCell, rc::Rc};

use futures::{FutureExt, StreamExt, channel::mpsc};
use serde_json::Value;
use t3_rpc::Frame;

use crate::platform::{HostFuture, PlatformError, SocketRequest, WebSocket};

#[derive(Clone)]
pub struct BrowserWire {
    inner: Rc<BrowserWireState>,
}

struct BrowserWireState {
    incoming: mpsc::UnboundedSender<Frame>,
    receive: RefCell<Option<mpsc::UnboundedReceiver<Frame>>>,
    sent: RefCell<Vec<Value>>,
    requests: RefCell<Vec<String>>,
}

impl Default for BrowserWire {
    fn default() -> Self {
        let (incoming, receive) = mpsc::unbounded();
        Self {
            inner: Rc::new(BrowserWireState {
                incoming,
                receive: RefCell::new(Some(receive)),
                sent: RefCell::default(),
                requests: RefCell::default(),
            }),
        }
    }
}

impl BrowserWire {
    pub fn text(&self, value: Value) -> bool {
        self.inner
            .incoming
            .unbounded_send(Frame::Text(value.to_string()))
            .is_ok()
    }

    pub fn binary(&self, bytes: Vec<u8>) -> bool {
        self.inner
            .incoming
            .unbounded_send(Frame::Binary(bytes))
            .is_ok()
    }

    pub fn requests(&self) -> Vec<String> {
        self.inner.requests.borrow().clone()
    }

    pub fn sent(&self) -> Vec<Value> {
        self.inner.sent.borrow().clone()
    }

    pub fn connect(&self, request: SocketRequest) -> Result<Box<dyn WebSocket>, PlatformError> {
        self.inner.requests.borrow_mut().push(request.url);
        let receive = self
            .inner
            .receive
            .borrow_mut()
            .take()
            .ok_or(PlatformError::Closed)?;
        Ok(Box::new(BrowserWireSocket {
            wire: self.clone(),
            receive,
        }))
    }
}

struct BrowserWireSocket {
    wire: BrowserWire,
    receive: mpsc::UnboundedReceiver<Frame>,
}

impl WebSocket for BrowserWireSocket {
    fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
        async move {
            let Frame::Text(text) = frame else {
                return Err(PlatformError::InvalidData(
                    "expected browser input JSON".into(),
                ));
            };
            let value = serde_json::from_str(&text)
                .map_err(|_| PlatformError::InvalidData("expected browser input JSON".into()))?;
            self.wire.inner.sent.borrow_mut().push(value);
            Ok(())
        }
        .boxed_local()
    }

    fn receive(&mut self) -> HostFuture<'_, Option<Frame>> {
        async move { Ok(self.receive.next().await) }.boxed_local()
    }

    fn close(&mut self) -> HostFuture<'_, ()> {
        async move {
            self.receive.close();
            Ok(())
        }
        .boxed_local()
    }
}
