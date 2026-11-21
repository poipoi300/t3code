//! A scriptable browser socket for previews exercised through the client service.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use futures::{FutureExt, StreamExt, channel::mpsc};
use serde_json::Value;
use t3_rpc::Frame;

use crate::platform::{HostFuture, PlatformError, SocketClose, SocketRequest, WebSocket};

#[derive(Clone)]
pub struct BrowserWire {
    inner: Rc<BrowserWireState>,
}

struct BrowserWireState {
    incoming: mpsc::UnboundedSender<Incoming>,
    receive: RefCell<Option<mpsc::UnboundedReceiver<Incoming>>>,
    sent: RefCell<Vec<Value>>,
    stall_sends: Cell<bool>,
    requests: RefCell<Vec<String>>,
}

enum Incoming {
    Frame(Frame),
    Close(SocketClose),
}

impl Default for BrowserWire {
    fn default() -> Self {
        let (incoming, receive) = mpsc::unbounded();
        Self {
            inner: Rc::new(BrowserWireState {
                incoming,
                receive: RefCell::new(Some(receive)),
                sent: RefCell::default(),
                stall_sends: Cell::new(false),
                requests: RefCell::default(),
            }),
        }
    }
}

impl BrowserWire {
    pub fn text(&self, value: Value) -> bool {
        self.inner
            .incoming
            .unbounded_send(Incoming::Frame(Frame::Text(value.to_string())))
            .is_ok()
    }

    pub fn binary(&self, bytes: Vec<u8>) -> bool {
        self.inner
            .incoming
            .unbounded_send(Incoming::Frame(Frame::Binary(bytes)))
            .is_ok()
    }

    pub fn peer_close(&self, code: u16, reason: &str) -> bool {
        self.inner
            .incoming
            .unbounded_send(Incoming::Close(SocketClose {
                code,
                reason: reason.into(),
            }))
            .is_ok()
    }

    #[cfg(test)]
    pub(super) fn stall_sends(&self) {
        self.inner.stall_sends.set(true);
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
            peer_close: None,
        }))
    }
}

struct BrowserWireSocket {
    wire: BrowserWire,
    receive: mpsc::UnboundedReceiver<Incoming>,
    peer_close: Option<SocketClose>,
}

impl WebSocket for BrowserWireSocket {
    fn close_info(&self) -> Option<&SocketClose> {
        self.peer_close.as_ref()
    }

    fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
        async move {
            if self.wire.inner.stall_sends.get() {
                futures::future::pending::<()>().await;
            }
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
        async move {
            match self.receive.next().await {
                Some(Incoming::Frame(frame)) => Ok(Some(frame)),
                Some(Incoming::Close(close)) => {
                    self.peer_close = Some(close);
                    Ok(None)
                }
                None => Ok(None),
            }
        }
        .boxed_local()
    }

    fn close(&mut self) -> HostFuture<'_, ()> {
        async move {
            self.receive.close();
            Ok(())
        }
        .boxed_local()
    }
}
