//! Deterministic host and server scripts; no wall-clock sleeps or network.
#[cfg(any(test, feature = "testing"))]
pub mod environment;
#[cfg(test)]
mod relay_tests;
#[cfg(test)]
mod removal_tests;
#[cfg(test)]
mod route_policy_tests;
#[cfg(test)]
mod terminal_tests;
use crate::{
    ClientError,
    platform::*,
    rpc::{RpcEndpoint, RpcMethod},
    types::{EnvironmentId, Generation, ThreadKey},
};
use futures::{
    FutureExt, StreamExt,
    channel::{mpsc, oneshot},
    future::LocalBoxFuture,
    stream::LocalBoxStream,
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    future::Future,
    pin::Pin,
    rc::{Rc, Weak},
    task::{Context, Poll},
};
#[derive(Clone, Default)]
pub struct FakePlatform {
    inner: Rc<FakeInner>,
}
type StoredValues = BTreeMap<StorageKey, Vec<u8>>;
type Sleepers = Vec<(u64, u64, oneshot::Sender<()>)>;
#[derive(Default)]
struct FakeInner {
    ordinary: RefCell<StoredValues>,
    secure: RefCell<StoredValues>,
    epoch: Cell<u64>,
    ticks: Cell<u64>,
    random: Cell<u8>,
    sleepers: RefCell<Sleepers>,
    timer_id: Cell<u64>,
    write_count: Cell<u64>,
    http: RefCell<VecDeque<Result<HttpResponse, PlatformError>>>,
    http_requests: RefCell<Vec<HttpRequest>>,
    sockets: RefCell<VecDeque<ScriptedSocket>>,
    host: RefCell<VecDeque<Result<HostReply, PlatformError>>>,
    events: RefCell<Vec<mpsc::UnboundedSender<PlatformEvent>>>,
    route_policies: RefCell<Vec<crate::navigation::RoutePolicy>>,
}
impl FakePlatform {
    /// Every route policy the client published, in order.
    pub fn route_policies(&self) -> Vec<crate::navigation::RoutePolicy> {
        self.inner.route_policies.borrow().clone()
    }
    pub fn advance(&self, milliseconds: u64) {
        self.inner.epoch.set(self.inner.epoch.get() + milliseconds);
        self.inner.ticks.set(self.inner.ticks.get() + milliseconds);
        let mut ready = Vec::new();
        self.inner.sleepers.borrow_mut().retain_mut(|(_, due, tx)| {
            if *due <= self.inner.ticks.get() {
                let (replacement, _) = oneshot::channel();
                ready.push(std::mem::replace(tx, replacement));
                false
            } else {
                true
            }
        });
        for tx in ready {
            let _ = tx.send(());
        }
    }
    pub fn push_http(&self, response: Result<HttpResponse, PlatformError>) {
        self.inner.http.borrow_mut().push_back(response);
    }
    pub fn http_requests(&self) -> Vec<HttpRequest> {
        self.inner.http_requests.borrow().clone()
    }
    pub fn pending_timers(&self) -> usize {
        self.inner.sleepers.borrow().len()
    }
    pub fn storage_write_count(&self) -> u64 {
        self.inner.write_count.get()
    }
    pub fn push_socket(&self, socket: ScriptedSocket) {
        self.inner.sockets.borrow_mut().push_back(socket);
    }
    pub fn push_host(&self, response: Result<HostReply, PlatformError>) {
        self.inner.host.borrow_mut().push_back(response);
    }
    pub fn emit(&self, event: PlatformEvent) {
        self.inner
            .events
            .borrow_mut()
            .retain(|tx| tx.unbounded_send(event.clone()).is_ok());
    }
    fn values(&self, area: StorageArea) -> &RefCell<StoredValues> {
        match area {
            StorageArea::Ordinary => &self.inner.ordinary,
            StorageArea::Secure => &self.inner.secure,
        }
    }
}
struct FakeTimer {
    owner: Weak<FakeInner>,
    id: u64,
    receiver: oneshot::Receiver<()>,
}
impl Future for FakeTimer {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        Pin::new(&mut self.receiver).poll(cx).map(|_| ())
    }
}
impl Drop for FakeTimer {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner
                .sleepers
                .borrow_mut()
                .retain(|(id, _, _)| *id != self.id);
        }
    }
}
impl Storage for FakePlatform {
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        async move {
            let mut ordinary = self.inner.ordinary.borrow_mut();
            let mut secure = self.inner.secure.borrow_mut();
            for condition in transaction.conditions {
                let values = match condition.area {
                    StorageArea::Ordinary => &ordinary,
                    StorageArea::Secure => &secure,
                };
                if values.get(&condition.key) != condition.expected.as_ref() {
                    return Ok(false);
                }
            }
            for (values, writes) in [
                (&mut ordinary, transaction.ordinary),
                (&mut secure, transaction.secure),
            ] {
                for write in writes {
                    if let Some(value) = write.value {
                        values.insert(write.key, value);
                    } else {
                        values.remove(&write.key);
                    }
                }
            }
            Ok(true)
        }
        .boxed_local()
    }
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        async move { Ok(self.values(area).borrow().get(&key).cloned()) }.boxed_local()
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        async move {
            self.inner.write_count.set(self.inner.write_count.get() + 1);
            let mut values = self.values(area).borrow_mut();
            for write in writes {
                if let Some(value) = write.value {
                    values.insert(write.key, value);
                } else {
                    values.remove(&write.key);
                }
            }
            Ok(())
        }
        .boxed_local()
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        async move {
            Ok(self
                .values(area)
                .borrow()
                .keys()
                .filter(|k| k.namespace == namespace)
                .cloned()
                .collect())
        }
        .boxed_local()
    }
}
impl Platform for FakePlatform {
    fn now_epoch_ms(&self) -> u64 {
        self.inner.epoch.get()
    }
    fn monotonic_ms(&self) -> u64 {
        self.inner.ticks.get()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        Ok((0..length)
            .map(|_| {
                let v = self.inner.random.get();
                self.inner.random.set(v.wrapping_add(1));
                v
            })
            .collect())
    }
    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        let (tx, rx) = oneshot::channel();
        let id = self.inner.timer_id.get() + 1;
        self.inner.timer_id.set(id);
        self.inner
            .sleepers
            .borrow_mut()
            .push((id, self.monotonic_ms() + milliseconds, tx));
        FakeTimer {
            owner: Rc::downgrade(&self.inner),
            id,
            receiver: rx,
        }
        .boxed_local()
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        futures::future::ready(()).boxed_local()
    }
    fn set_route_policy(&self, policy: crate::navigation::RoutePolicy) {
        self.inner.route_policies.borrow_mut().push(policy);
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        let (tx, rx) = mpsc::unbounded();
        self.inner.events.borrow_mut().push(tx);
        rx.boxed_local()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        async move {
            self.inner.http_requests.borrow_mut().push(request.clone());
            let response = self
                .inner
                .http
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(PlatformError::Network("unscripted HTTP".into())))?;
            if response.body.len() > request.max_response_bytes {
                return Err(PlatformError::TooLarge);
            }
            Ok(response)
        }
        .boxed_local()
    }
    fn websocket(&self, _request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        async move {
            let socket = self
                .inner
                .sockets
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| PlatformError::Network("unscripted socket".into()))?;
            Ok(Box::new(socket) as Box<dyn WebSocket>)
        }
        .boxed_local()
    }
    fn host(&self, _action: HostAction) -> HostFuture<'_, HostReply> {
        async move {
            self.inner
                .host
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(PlatformError::Unsupported("unscripted host action".into())))
        }
        .boxed_local()
    }
}
#[derive(Clone, Default)]
pub struct ScriptedSocket {
    pub incoming: Rc<RefCell<VecDeque<Result<t3_rpc::Frame, PlatformError>>>>,
    pub sent: Rc<RefCell<Vec<t3_rpc::Frame>>>,
    closed: Rc<Cell<bool>>,
}
impl WebSocket for ScriptedSocket {
    fn send(&mut self, frame: t3_rpc::Frame) -> HostFuture<'_, ()> {
        async move {
            if self.closed.get() {
                return Err(PlatformError::Closed);
            }
            self.sent.borrow_mut().push(frame);
            Ok(())
        }
        .boxed_local()
    }
    fn receive(&mut self) -> HostFuture<'_, Option<t3_rpc::Frame>> {
        async move {
            if self.closed.get() {
                return Ok(None);
            }
            self.incoming.borrow_mut().pop_front().transpose()
        }
        .boxed_local()
    }
    fn close(&mut self) -> HostFuture<'_, ()> {
        async move {
            self.closed.set(true);
            Ok(())
        }
        .boxed_local()
    }
}
#[derive(Clone)]
pub enum ServerDelivery {
    Shell {
        environment: EnvironmentId,
        session: Generation,
        item: t3_contracts::orchestration_v2::OrchestrationV2ShellStreamItem,
    },
    Thread {
        key: ThreadKey,
        session: Generation,
        item: t3_contracts::orchestration_v2::OrchestrationV2ThreadStreamItem,
    },
}
#[derive(Default)]
pub struct ScriptedConnection {
    pub deliveries: VecDeque<ServerDelivery>,
}
impl ScriptedConnection {
    pub fn drain(&mut self, store: &crate::ClientStore) -> Result<(), ClientError> {
        while let Some(delivery) = self.deliveries.pop_front() {
            match delivery {
                ServerDelivery::Shell {
                    environment,
                    session,
                    item,
                } => {
                    store.apply_shell(&environment, session, vec![item]);
                }
                ServerDelivery::Thread { key, session, item } => {
                    store.apply_thread(key, session, vec![item])?;
                }
            }
        }
        Ok(())
    }
}
pub struct RpcScript {
    pub method: String,
    pub input: Value,
    pub replies: Vec<Result<Value, ClientError>>,
}
#[derive(Default)]
pub struct ScriptedRpc {
    pub scripts: RefCell<VecDeque<RpcScript>>,
}
impl ScriptedRpc {
    fn take<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> Result<Vec<Result<Value, ClientError>>, ClientError> {
        let script = self
            .scripts
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| ClientError::ScriptMismatch("missing RPC script".into()))?;
        if script.method != M::NAME || script.input != serde_json::to_value(input)? {
            return Err(ClientError::ScriptMismatch(M::NAME.into()));
        }
        Ok(script.replies)
    }
}
impl RpcEndpoint for ScriptedRpc {
    fn call<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        let replies = self.take::<M>(input);
        async move {
            let mut replies = replies?;
            if replies.len() != 1 {
                return Err(ClientError::ScriptMismatch("unary reply count".into()));
            }
            Ok(serde_json::from_value(replies.remove(0)?)?)
        }
        .boxed_local()
    }
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        let replies = self.take::<M>(input).unwrap_or_else(|e| vec![Err(e)]);
        futures::stream::iter(
            replies
                .into_iter()
                .map(|v| v.and_then(|v| Ok(serde_json::from_value(v)?))),
        )
        .boxed_local()
    }
}

type SnapshotReplies<T> = RefCell<VecDeque<LocalBoxFuture<'static, Result<T, ClientError>>>>;
#[derive(Default)]
pub struct ScriptedSnapshots {
    shell: SnapshotReplies<crate::state::sync::ShellLoadResult>,
    thread: SnapshotReplies<crate::state::sync::ThreadLoadResult>,
    history: SnapshotReplies<t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage>,
    pub shell_requests: RefCell<Vec<crate::state::sync::ShellSnapshotOptions>>,
    pub thread_requests: RefCell<Vec<crate::types::ThreadId>>,
    pub history_requests: RefCell<Vec<crate::state::sync::HistoryQuery>>,
}

impl ScriptedSnapshots {
    pub fn push_shell(
        &self,
        reply: LocalBoxFuture<'static, Result<crate::state::sync::ShellLoadResult, ClientError>>,
    ) {
        self.shell.borrow_mut().push_back(reply);
    }
    pub fn push_thread(
        &self,
        reply: LocalBoxFuture<'static, Result<crate::state::sync::ThreadLoadResult, ClientError>>,
    ) {
        self.thread.borrow_mut().push_back(reply);
    }
    pub fn push_history(
        &self,
        reply: LocalBoxFuture<
            'static,
            Result<t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage, ClientError>,
        >,
    ) {
        self.history.borrow_mut().push_back(reply);
    }
}

impl crate::state::sync::SnapshotSource for ScriptedSnapshots {
    fn shell(
        &self,
        options: crate::state::sync::ShellSnapshotOptions,
    ) -> LocalBoxFuture<'_, Result<crate::state::sync::ShellLoadResult, ClientError>> {
        self.shell_requests.borrow_mut().push(options);
        self.shell.borrow_mut().pop_front().unwrap_or_else(|| {
            futures::future::ready(Err(ClientError::ScriptMismatch(
                "missing shell snapshot script".into(),
            )))
            .boxed_local()
        })
    }
    fn thread(
        &self,
        thread: crate::types::ThreadId,
    ) -> LocalBoxFuture<'_, Result<crate::state::sync::ThreadLoadResult, ClientError>> {
        self.thread_requests.borrow_mut().push(thread);
        self.thread.borrow_mut().pop_front().unwrap_or_else(|| {
            futures::future::ready(Err(ClientError::ScriptMismatch(
                "missing thread snapshot script".into(),
            )))
            .boxed_local()
        })
    }
    fn history(
        &self,
        query: crate::state::sync::HistoryQuery,
    ) -> LocalBoxFuture<
        '_,
        Result<t3_contracts::orchestration_v2::OrchestrationV2ThreadHistoryPage, ClientError>,
    > {
        self.history_requests.borrow_mut().push(query);
        self.history.borrow_mut().pop_front().unwrap_or_else(|| {
            futures::future::ready(Err(ClientError::ScriptMismatch(
                "missing history snapshot script".into(),
            )))
            .boxed_local()
        })
    }
}
