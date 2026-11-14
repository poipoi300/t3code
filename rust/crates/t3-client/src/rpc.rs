pub mod clock;
pub mod subscription;
#[cfg(test)]
mod subscription_tests;
use crate::{
    ClientError,
    permissions::{ContractPermissionPolicy, DestinationSessions, SessionGrants},
    types::EnvironmentId,
};
use futures::{
    FutureExt, Sink, Stream, StreamExt, channel::mpsc, future::LocalBoxFuture,
    stream::LocalBoxStream,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    fmt::Display,
    pin::Pin,
    rc::{Rc, Weak},
    task::{Context, Poll, Waker},
};
use t3_contracts::rpc::WS_METHODS;
pub use t3_rpc::{AsyncClient, CallEvent, Client as ProtocolClient, ClientError as ProtocolError};
use t3_rpc::{CallKind, Cause, Exit, Frame, Request, RequestId};

pub trait RpcMethod {
    type Input: Serialize;
    type Output: DeserializeOwned;
    const NAME: &'static str;
    fn valid_error(_error: &Value) -> bool {
        false
    }
}
impl<M: t3_contracts::rpc::RpcMethod> RpcMethod for M {
    type Input = M::Payload;
    type Output = M::Success;
    const NAME: &'static str = M::TAG;
    fn valid_error(error: &Value) -> bool {
        serde_json::from_value::<M::Error>(error.clone()).is_ok()
    }
}

#[derive(thiserror::Error)]
pub enum RpcCallError<E = Value> {
    #[error("RPC rejected the request")]
    Rejected(Vec<E>),
    #[error("RPC request was not sent")]
    NotSent(ClientError),
    #[error("RPC outcome is unknown")]
    Unknown,
    #[error("RPC handler defect")]
    Defect,
    #[error("RPC call interrupted")]
    Interrupted,
    #[error("RPC response did not match the method contract")]
    InvalidResponse,
    #[error("RPC consumer overflow; reload the authoritative projection")]
    Overflow,
}
impl<E> std::fmt::Debug for RpcCallError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}
impl<E: serde::Serialize> RpcCallError<E> {
    pub fn into_client_error(self) -> ClientError {
        match self {
            Self::NotSent(error) => error,
            Self::Unknown => ClientError::Disconnected,
            Self::Defect => ClientError::InvalidPayload("RPC handler defect".into()),
            Self::Interrupted => ClientError::Cancelled,
            Self::Rejected(errors) => rejection(
                &errors
                    .iter()
                    .filter_map(|error| serde_json::to_value(error).ok())
                    .collect::<Vec<_>>(),
            ),
            Self::InvalidResponse => {
                ClientError::InvalidPayload(subscription::RPC_RESPONSE_SCHEMA_ERROR.into())
            }
            Self::Overflow => ClientError::InvalidPayload("RPC consumer overflow; resync".into()),
        }
    }
}

/// The server's reason for refusing a request: the first typed error's `message` and `detail`
/// (strings; `detail` may be absent or null).
pub fn rejection(errors: &[Value]) -> ClientError {
    let text =
        |error: &Value, field: &str| error.get(field).and_then(Value::as_str).map(str::to_owned);
    let reason = errors.iter().find_map(|error| {
        let (message, detail) = (text(error, "message"), text(error, "detail"));
        (message.is_some() || detail.is_some()).then_some((message, detail))
    });
    match reason {
        Some((message, detail)) => ClientError::Rejected {
            message: message.unwrap_or_else(|| "The server rejected the request.".into()),
            detail,
        },
        None => ClientError::Rejected {
            message: "The server rejected the request.".into(),
            detail: None,
        },
    }
}

pub trait RpcEndpoint {
    fn destination(&self) -> Option<EnvironmentId> {
        None
    }
    fn call<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>>;
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>>;

    /// A transport must opt in to recognizing definitive server rejection.
    fn command<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, RpcCallError>>
    where
        M::Output: 'static,
    {
        let call = self.call::<M>(input);
        call.map(|result| {
            result.map_err(|error| match error {
                ClientError::PermissionDenied { .. } | ClientError::Unsupported(_) => {
                    RpcCallError::NotSent(error)
                }
                _ => RpcCallError::Unknown,
            })
        })
        .boxed_local()
    }
}

/// The caller installs a fresh endpoint per authenticated connection, and drives its driver.
#[derive(Clone)]
pub struct SessionEndpoint {
    inner: Rc<RefCell<Session>>,
}
struct Session {
    environment: EnvironmentId,
    grants: SessionGrants,
    destinations: Option<Rc<RefCell<DestinationSessions>>>,
    active: bool,
    next_id: u64,
    queued_starts: usize,
    commands: mpsc::UnboundedSender<DriverCommand>,
    calls: HashMap<RequestId, Rc<RefCell<Mailbox>>>,
    config_clock: Option<clock::ConfigClock>,
}
enum DriverCommand {
    Start(Request, CallKind),
    Interrupt(RequestId),
    Close,
}
#[derive(Default)]
struct Mailbox {
    chunks: VecDeque<Vec<Value>>,
    terminal: Option<Result<Option<Value>, RpcCallError>>,
    waker: Option<Waker>,
}
const MAX_CALLS: usize = 256;
const MAX_BUFFERED_CHUNKS: usize = 8;

impl SessionEndpoint {
    pub fn subscribe_recovering<'a, M: RpcMethod, P: crate::platform::Platform + ?Sized>(
        &'a self,
        platform: &'a P,
        input: M::Input,
        options: subscription::SubscriptionOptions,
    ) -> LocalBoxStream<'a, Result<M::Output, ClientError>>
    where
        M::Input: Clone + 'a,
        M::Output: 'a,
    {
        subscription::recover_subscription(
            platform,
            M::NAME,
            move || self.subscribe::<M>(input.clone()),
            options,
        )
    }
    pub async fn initial_config_with_clock(
        &self,
        platform: &(impl crate::platform::Platform + ?Sized),
    ) -> Result<t3_contracts::server::ServerConfig, ClientError> {
        {
            let mut inner = self.inner.borrow_mut();
            let environment = inner.environment.clone();
            inner.config_clock.get_or_insert_with(|| {
                clock::ConfigClock::new(environment, platform.now_epoch_ms() as f64)
            });
        }
        let config = self.initial_config().await?;
        if let Some(clock) = &mut self.inner.borrow_mut().config_clock {
            clock.observe(&config, platform.now_epoch_ms() as f64);
        }
        Ok(config)
    }
    pub fn to_server_clock(&self, client_ms: f64) -> f64 {
        self.inner
            .borrow()
            .config_clock
            .as_ref()
            .map_or(client_ms, |clock| clock.to_server_clock(client_ms))
    }
    pub async fn initial_config(&self) -> Result<t3_contracts::server::ServerConfig, ClientError> {
        let config = self
            .call::<t3_contracts::rpc::WsServerGetConfigRpc>(t3_contracts::rpc::Empty {})
            .await?;
        if config.environment.environment_id != self.environment() {
            self.invalidate();
            return Err(ClientError::EnvironmentMismatch);
        }
        Ok(config)
    }
    pub async fn probe(&self) -> Result<(), ClientError> {
        self.call::<t3_contracts::rpc::WsServerProbeRpc>(t3_contracts::rpc::Empty {})
            .await?;
        Ok(())
    }
    pub fn environment(&self) -> EnvironmentId {
        self.inner.borrow().environment.clone()
    }
    pub fn update_grants(&self, grants: SessionGrants) {
        self.inner.borrow_mut().grants = grants;
    }
    pub fn invalidate(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.active = false;
        let _ = inner.commands.unbounded_send(DriverCommand::Close);
    }
    pub fn available<M: RpcMethod>(&self, input: &M::Input) -> bool {
        serde_json::to_value(input)
            .ok()
            .is_some_and(|input| self.authorize(M::NAME, &input).is_ok())
    }
    fn authorize(&self, method: &str, input: &Value) -> Result<(), ClientError> {
        let inner = self.inner.borrow();
        let destinations = inner.destinations.as_ref().map(|value| value.borrow());
        let grants = match &destinations {
            Some(destinations) => destinations.grants(&inner.environment),
            None => Some(&inner.grants),
        };
        crate::permissions::authorize(
            &ContractPermissionPolicy,
            inner.active.then_some(&inner.environment),
            grants,
            method,
            input,
        )
    }
    fn start<M: RpcMethod>(
        &self,
        input: M::Input,
        kind: CallKind,
    ) -> Result<RawCall, RpcCallError> {
        let payload =
            serde_json::to_value(input).map_err(|error| RpcCallError::NotSent(error.into()))?;
        // A replaced session refuses everything, as before, but as a disconnect: nothing was
        // sent, so callers retry on the next session instead of treating it as a refusal.
        if !self.inner.borrow().active {
            return Err(RpcCallError::NotSent(ClientError::Disconnected));
        }
        self.authorize(M::NAME, &payload)
            .map_err(RpcCallError::NotSent)?;
        let descriptor = WS_METHODS
            .iter()
            .find(|m| m.tag == M::NAME)
            .ok_or(RpcCallError::InvalidResponse)?;
        if descriptor.streams != (kind == CallKind::Stream) {
            return Err(RpcCallError::NotSent(ClientError::Unsupported(
                "RPC call kind",
            )));
        }
        let mut inner = self.inner.borrow_mut();
        if inner.calls.len() >= MAX_CALLS
            || inner.queued_starts >= MAX_CALLS
            || inner.next_id > 9_007_199_254_740_991
        {
            return Err(RpcCallError::NotSent(ClientError::Unsupported(
                "RPC call limit",
            )));
        }
        let id: RequestId = inner.next_id.into();
        inner.next_id += 1;
        let mailbox = Rc::new(RefCell::new(Mailbox::default()));
        inner
            .commands
            .unbounded_send(DriverCommand::Start(
                Request::new(id.clone(), M::NAME, payload),
                kind,
            ))
            .map_err(|_| RpcCallError::NotSent(ClientError::Disconnected))?;
        inner.queued_starts += 1;
        inner.calls.insert(id.clone(), mailbox.clone());
        Ok(RawCall {
            owner: Rc::downgrade(&self.inner),
            id,
            mailbox,
            kind,
            ended: false,
        })
    }
    pub fn call_typed<M: t3_contracts::rpc::RpcMethod>(
        &self,
        input: M::Payload,
    ) -> LocalBoxFuture<'_, Result<M::Success, RpcCallError<M::Error>>> {
        let call = self.start::<M>(input, CallKind::Unary);
        async move { unary(call).await.map_err(decode_error::<M::Error>) }.boxed_local()
    }
    pub fn subscribe_typed<M: t3_contracts::rpc::RpcMethod>(
        &self,
        input: M::Payload,
    ) -> LocalBoxStream<'static, Result<M::Success, RpcCallError<M::Error>>>
    where
        M::Error: 'static,
    {
        decoded_stream(
            self.start::<M>(input, CallKind::Stream),
            decode_error::<M::Error>,
        )
    }
}
fn decode_error<E: DeserializeOwned>(error: RpcCallError) -> RpcCallError<E> {
    match error {
        RpcCallError::Rejected(errors) => {
            match errors.into_iter().map(serde_json::from_value).collect() {
                Ok(errors) => RpcCallError::Rejected(errors),
                Err(_) => RpcCallError::InvalidResponse,
            }
        }
        RpcCallError::NotSent(error) => RpcCallError::NotSent(error),
        RpcCallError::Unknown => RpcCallError::Unknown,
        RpcCallError::Defect => RpcCallError::Defect,
        RpcCallError::Interrupted => RpcCallError::Interrupted,
        RpcCallError::InvalidResponse => RpcCallError::InvalidResponse,
        RpcCallError::Overflow => RpcCallError::Overflow,
    }
}
async fn unary<A: DeserializeOwned>(
    call: Result<RawCall, RpcCallError>,
) -> Result<A, RpcCallError> {
    let mut call = call?;
    match call.next().await {
        Some(Ok(mut values)) if values.len() == 1 => {
            serde_json::from_value(values.remove(0)).map_err(|_| RpcCallError::InvalidResponse)
        }
        Some(Err(error)) => Err(error),
        _ => Err(RpcCallError::InvalidResponse),
    }
}
fn decoded_stream<A: DeserializeOwned, E: 'static>(
    call: Result<RawCall, RpcCallError>,
    map_error: fn(RpcCallError) -> E,
) -> LocalBoxStream<'static, Result<A, E>> {
    let stream: LocalBoxStream<'static, _> = match call {
        Ok(call) => call.boxed_local(),
        Err(error) => futures::stream::once(futures::future::ready(Err(error))).boxed_local(),
    };
    EndOnError(Some(
        stream
            .flat_map(|chunk| {
                futures::stream::iter(match chunk {
                    Ok(values) => values.into_iter().map(Ok).collect(),
                    Err(error) => vec![Err(error)],
                })
            })
            .map(move |value| {
                value
                    .and_then(|value| {
                        serde_json::from_value(value).map_err(|_| RpcCallError::InvalidResponse)
                    })
                    .map_err(map_error)
            }),
    ))
    .boxed_local()
}
struct EndOnError<S>(Option<S>);
impl<S, T, E> Stream for EndOnError<S>
where
    S: Stream<Item = Result<T, E>> + Unpin,
{
    type Item = Result<T, E>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let Some(stream) = &mut this.0 else {
            return Poll::Ready(None);
        };
        let next = Pin::new(stream).poll_next(cx);
        if matches!(&next, Poll::Ready(None | Some(Err(_)))) {
            this.0 = None;
        }
        next
    }
}
impl RpcEndpoint for SessionEndpoint {
    fn destination(&self) -> Option<EnvironmentId> {
        Some(self.environment())
    }
    fn call<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        let call = self.start::<M>(input, CallKind::Unary);
        async move { unary(call).await.map_err(RpcCallError::into_client_error) }.boxed_local()
    }
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
        decoded_stream(
            self.start::<M>(input, CallKind::Stream),
            RpcCallError::into_client_error,
        )
    }
    fn command<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, RpcCallError>>
    where
        M::Output: 'static,
    {
        let call = self.start::<M>(input, CallKind::Unary);
        async move {
            unary(call).await.map_err(|error| match error {
                RpcCallError::Rejected(errors) if !errors.iter().all(M::valid_error) => {
                    RpcCallError::InvalidResponse
                }
                error => error,
            })
        }
        .boxed_local()
    }
}
struct RawCall {
    owner: Weak<RefCell<Session>>,
    id: RequestId,
    mailbox: Rc<RefCell<Mailbox>>,
    kind: CallKind,
    ended: bool,
}
impl Drop for RawCall {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut inner = owner.borrow_mut();
            if inner.calls.remove(&self.id).is_some() {
                let _ = inner
                    .commands
                    .unbounded_send(DriverCommand::Interrupt(self.id.clone()));
            }
        }
    }
}
impl Stream for RawCall {
    type Item = Result<Vec<Value>, RpcCallError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        let mut mailbox = this.mailbox.borrow_mut();
        if let Some(chunk) = mailbox.chunks.pop_front() {
            return Poll::Ready(Some(Ok(chunk)));
        }
        if let Some(terminal) = mailbox.terminal.take() {
            this.ended = true;
            return Poll::Ready(match terminal {
                Ok(value) if this.kind == CallKind::Unary => {
                    Some(Ok(vec![value.unwrap_or(Value::Null)]))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            });
        }
        mailbox.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

pub struct SessionDriver<S, R> {
    client: AsyncClient<S, R>,
    inner: Rc<RefCell<Session>>,
    commands: mpsc::UnboundedReceiver<DriverCommand>,
}
pub fn session<S, R>(
    sink: S,
    incoming: R,
    environment: EnvironmentId,
    grants: SessionGrants,
    now_ms: u64,
) -> (SessionEndpoint, SessionDriver<S, R>)
where
    S: Sink<Frame> + Unpin,
    S::Error: Display,
    R: Stream<Item = Result<Frame, S::Error>> + Unpin,
{
    let (commands, receiver) = mpsc::unbounded();
    let inner = Rc::new(RefCell::new(Session {
        environment,
        grants,
        destinations: None,
        active: true,
        next_id: 0,
        queued_starts: 0,
        commands,
        calls: HashMap::new(),
        config_clock: None,
    }));
    (
        SessionEndpoint {
            inner: inner.clone(),
        },
        SessionDriver {
            client: AsyncClient::new(sink, incoming, now_ms),
            inner,
            commands: receiver,
        },
    )
}
pub fn session_for_destination<S, R>(
    sink: S,
    incoming: R,
    environment: EnvironmentId,
    destinations: Rc<RefCell<DestinationSessions>>,
    now_ms: u64,
) -> (SessionEndpoint, SessionDriver<S, R>)
where
    S: Sink<Frame> + Unpin,
    S::Error: Display,
    R: Stream<Item = Result<Frame, S::Error>> + Unpin,
{
    let grants = SessionGrants {
        scopes: Default::default(),
        valid: false,
    };
    let (endpoint, driver) = session(sink, incoming, environment, grants, now_ms);
    endpoint.inner.borrow_mut().destinations = Some(destinations);
    (endpoint, driver)
}
fn finish(mailbox: &Rc<RefCell<Mailbox>>, result: Result<Option<Value>, RpcCallError>) {
    let waker = {
        let mut mailbox = mailbox.borrow_mut();
        mailbox.terminal = Some(result);
        mailbox.waker.take()
    };
    if let Some(waker) = waker {
        waker.wake();
    }
}
impl<S, R> Drop for SessionDriver<S, R> {
    fn drop(&mut self) {
        let calls = {
            let mut inner = self.inner.borrow_mut();
            inner.active = false;
            std::mem::take(&mut inner.calls)
        };
        let mut calls: Vec<_> = calls.into_iter().collect();
        calls.sort_by(|(a, _), (b, _)| match (a, b) {
            (RequestId::Number(a), RequestId::Number(b)) => a.as_u64().cmp(&b.as_u64()),
            (RequestId::Number(_), RequestId::String(_)) => std::cmp::Ordering::Less,
            (RequestId::String(_), RequestId::Number(_)) => std::cmp::Ordering::Greater,
            (RequestId::String(a), RequestId::String(b)) => a.cmp(b),
        });
        for (_, mailbox) in calls {
            finish(&mailbox, Err(RpcCallError::Unknown));
        }
    }
}
impl<S, R> SessionDriver<S, R>
where
    S: Sink<Frame> + Unpin,
    S::Error: Display,
    R: Stream<Item = Result<Frame, S::Error>> + Unpin,
{
    pub async fn run<T: Stream<Item = u64> + Unpin>(
        mut self,
        mut ticks: T,
        clock: impl Fn() -> u64,
    ) {
        loop {
            // Sink sends must finish before selecting another command; dropping a send can lose a popped frame.
            if self.client.flush().await.is_err() {
                break;
            }
            enum Step {
                Command(Option<DriverCommand>),
                Tick(Option<u64>),
                Event(Result<Option<CallEvent>, t3_rpc::transport::TransportError>),
            }
            let step = {
                let command = self.commands.next().fuse();
                let tick = ticks.next().fuse();
                let mut disabled_ticks = futures::stream::pending();
                let event = self.client.next_event(&mut disabled_ticks, &clock).fuse();
                futures::pin_mut!(command, tick, event);
                futures::select_biased! { command = command => Step::Command(command), event = event => Step::Event(event), tick = tick => Step::Tick(tick) }
            };
            match step {
                Step::Command(Some(DriverCommand::Start(request, kind))) => {
                    self.inner.borrow_mut().queued_starts -= 1;
                    if !self.inner.borrow().calls.contains_key(&request.id) {
                        continue;
                    }
                    let id = request.id.clone();
                    // A queued request must observe grants revoked since its creation.
                    let endpoint = SessionEndpoint {
                        inner: self.inner.clone(),
                    };
                    let result = endpoint.authorize(&request.tag, &request.payload);
                    if let Err(error) = result {
                        let mailbox = self.inner.borrow_mut().calls.remove(&id);
                        if let Some(mailbox) = mailbox {
                            finish(&mailbox, Err(RpcCallError::NotSent(error)));
                        }
                    } else if self.client.state.start_request(request, kind).is_err() {
                        let mailbox = self.inner.borrow_mut().calls.remove(&id);
                        if let Some(mailbox) = mailbox {
                            finish(
                                &mailbox,
                                Err(RpcCallError::NotSent(ClientError::Disconnected)),
                            );
                        }
                    }
                }
                Step::Command(Some(DriverCommand::Interrupt(id))) => {
                    self.client.state.interrupt(&id)
                }
                Step::Command(Some(DriverCommand::Close) | None) => {
                    let _ = self.client.close().await;
                    break;
                }
                Step::Tick(Some(now)) => self.client.state.tick(now),
                Step::Tick(None) => break,
                Step::Event(Ok(Some(event))) => self.deliver(event),
                Step::Event(Ok(None) | Err(_)) => break,
            }
        }
    }
    fn deliver(&mut self, event: CallEvent) {
        match event {
            CallEvent::Chunk { id, values } => {
                let mailbox = self.inner.borrow().calls.get(&id).cloned();
                if let Some(mailbox) = mailbox {
                    let overflow = mailbox.borrow().chunks.len() >= MAX_BUFFERED_CHUNKS;
                    if overflow {
                        mailbox.borrow_mut().chunks.clear();
                        self.inner.borrow_mut().calls.remove(&id);
                        self.client.state.interrupt(&id);
                        finish(&mailbox, Err(RpcCallError::Overflow));
                    } else {
                        let wake = {
                            let mut mailbox = mailbox.borrow_mut();
                            mailbox.chunks.push_back(values);
                            mailbox.waker.take()
                        };
                        if let Some(waker) = wake {
                            waker.wake();
                        }
                    }
                }
            }
            CallEvent::Exit { id, exit } => {
                let mailbox = self.inner.borrow_mut().calls.remove(&id);
                if let Some(mailbox) = mailbox {
                    let result = match exit {
                        Exit::Success { value } => Ok(value),
                        Exit::Failure { cause } => {
                            if !cause.is_empty()
                                && cause.iter().all(|c| matches!(c, Cause::Fail { .. }))
                            {
                                Err(RpcCallError::Rejected(
                                    cause
                                        .into_iter()
                                        .filter_map(|c| match c {
                                            Cause::Fail { error } => Some(error),
                                            _ => None,
                                        })
                                        .collect(),
                                ))
                            } else if !cause.is_empty()
                                && cause.iter().all(|cause| {
                                    matches!(cause, Cause::Die { defect } if defect
                                        .as_str()
                                        .is_some_and(|d| d.starts_with("Unknown request tag: ")))
                                })
                            {
                                // Effect's RpcServer answer for a method it has no handler for:
                                // nothing ran, and the server does not serve this method.
                                Err(RpcCallError::NotSent(ClientError::Unsupported(
                                    "the server does not serve this RPC method",
                                )))
                            } else if cause.iter().any(|cause| matches!(cause, Cause::Die { .. })) {
                                Err(RpcCallError::Defect)
                            } else if !cause.is_empty()
                                && cause
                                    .iter()
                                    .all(|cause| matches!(cause, Cause::Interrupt { .. }))
                            {
                                Err(RpcCallError::Interrupted)
                            } else {
                                Err(RpcCallError::InvalidResponse)
                            }
                        }
                    };
                    finish(&mailbox, result);
                }
            }
            CallEvent::Failed { id, error } => {
                let mailbox = self.inner.borrow_mut().calls.remove(&id);
                if let Some(mailbox) = mailbox {
                    let error = match error {
                        ProtocolError::Defect(_) => RpcCallError::Defect,
                        ProtocolError::Interrupted => RpcCallError::Interrupted,
                        ProtocolError::Protocol(_) => RpcCallError::InvalidResponse,
                        _ => RpcCallError::Unknown,
                    };
                    finish(&mailbox, Err(error));
                }
            }
            CallEvent::ServerRequest(_) => {}
        }
    }
}

#[derive(thiserror::Error)]
pub enum HttpRpcError {
    #[error("environment HTTP request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
    #[error("environment HTTP request failed")]
    Fetch,
    #[error("environment HTTP response was invalid")]
    InvalidJson,
    #[error("environment HTTP endpoint returned undeclared status {status}")]
    UndeclaredStatus { status: u16 },
    #[error("environment HTTP request was rejected")]
    Rejected(Box<t3_contracts::environment_http::EnvironmentHttpCommonError>),
}
impl std::fmt::Debug for HttpRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}
pub fn remote_api_base_url(base: &str) -> Result<String, ClientError> {
    let mut url =
        url::Url::parse(base).map_err(|_| ClientError::InvalidPayload("HTTP base URL".into()))?;
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}
pub async fn execute_environment_http_request<T: DeserializeOwned>(
    platform: &(impl crate::platform::Platform + ?Sized),
    request: crate::platform::HttpRequest,
    timeout_ms: u64,
) -> Result<T, HttpRpcError> {
    use futures::future::{Either, select};
    let max_response_bytes = request.max_response_bytes;
    let response = match select(platform.http(request), platform.sleep(timeout_ms)).await {
        Either::Left((Ok(response), _)) => response,
        Either::Left((Err(_), _)) => return Err(HttpRpcError::Fetch),
        Either::Right(_) => return Err(HttpRpcError::Timeout { timeout_ms }),
    };
    if response.body.len() > max_response_bytes {
        return Err(HttpRpcError::InvalidJson);
    }
    if !(200..300).contains(&response.status) {
        return match serde_json::from_slice(&response.body) {
            Ok(error) => Err(HttpRpcError::Rejected(Box::new(error))),
            Err(_) => Err(HttpRpcError::UndeclaredStatus {
                status: response.status,
            }),
        };
    }
    serde_json::from_slice(&response.body).map_err(|_| HttpRpcError::InvalidJson)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn a_refusal_keeps_the_servers_message_and_detail() {
        let error = |value: Value| super::rejection(&[value]).to_string();
        assert_eq!(
            error(
                json!({"_tag":"OrchestrationV2DispatchCommandError","message":"Failed to dispatch orchestration V2 command","detail":"service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex."})
            ),
            "Failed to dispatch orchestration V2 command: service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex."
        );
        assert_eq!(
            error(json!({"message":"This thread is archived.","detail":null})),
            "This thread is archived."
        );
        assert_eq!(
            error(json!({"detail":"only detail"})),
            "The server rejected the request.: only detail"
        );
        assert_eq!(
            error(json!({"_tag":"Bare"})),
            "The server rejected the request."
        );
        assert!(matches!(
            RpcCallError::Rejected(vec![json!({"message":"m","detail":"d"})]).into_client_error(),
            ClientError::Rejected { message, detail: Some(detail) } if message == "m" && detail == "d"
        ));
    }

    use super::*;
    use futures::{SinkExt, executor::block_on};
    use serde_json::json;
    use t3_contracts::rpc::{
        Empty, WsGitPreparePullRequestThreadRpc, WsOrchestrationV2SubscribePromptSuggestionRpc,
        WsServerProbeRpc,
    };
    use t3_rpc::{ClientMessage, ServerMessage, codec};
    fn grants() -> SessionGrants {
        SessionGrants {
            valid: true,
            scopes: Default::default(),
        }
    }
    fn env() -> EnvironmentId {
        EnvironmentId("env".into())
    }
    fn decode_request(frame: Frame) -> Request {
        let messages: Vec<ClientMessage> = codec::decode(&frame).unwrap();
        match messages.into_iter().next().unwrap() {
            ClientMessage::Request(request) => request,
            _ => panic!("expected request"),
        }
    }
    #[test]
    fn concurrent_typed_calls_use_contract_tags_and_correlate_out_of_order_replies() {
        block_on(async {
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, driver) = session(wire, incoming, env(), grants(), 0);
            let first = endpoint.call_typed::<WsServerProbeRpc>(Empty {});
            let second = endpoint.call_typed::<WsServerProbeRpc>(Empty {});
            let controller = async {
                let a = decode_request(requests.next().await.unwrap());
                let b = decode_request(requests.next().await.unwrap());
                assert_eq!(a.tag, "server.probe");
                assert_eq!(a.payload, json!({}));
                assert_ne!(a.id, b.id);
                for id in [b.id, a.id] {
                    peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                        request_id: id,
                        exit: Exit::success(json!({})),
                    })
                    .unwrap()))
                        .await
                        .unwrap();
                }
                assert_eq!(first.await.unwrap(), Empty {});
                assert_eq!(second.await.unwrap(), Empty {});
                endpoint.invalidate();
            };
            futures::join!(driver.run(futures::stream::pending(), || 0), controller);
        });
    }
    #[test]
    fn disconnect_is_unknown_and_a_fresh_session_does_not_replay_a_request() {
        block_on(async {
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, driver) = session(wire, incoming, env(), grants(), 0);
            let call = endpoint.command::<WsServerProbeRpc>(Empty {});
            let controller = async {
                assert_eq!(decode_request(requests.next().await.unwrap()).id, 0.into());
                drop(peer);
                assert!(matches!(call.await, Err(RpcCallError::Unknown)));
                assert!(!endpoint.available::<WsServerProbeRpc>(&Empty {}));
            };
            futures::join!(driver.run(futures::stream::pending(), || 0), controller);
        });
    }
    #[test]
    fn dropping_driver_completes_pending_calls_without_polling_it_again() {
        let (wire, _) = mpsc::unbounded::<Frame>();
        let (_, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, driver) = session(wire, incoming, env(), grants(), 0);
        let call = endpoint.command::<WsServerProbeRpc>(Empty {});
        drop(driver);
        assert!(matches!(block_on(call), Err(RpcCallError::Unknown)));
    }
    #[test]
    fn stream_chunks_acknowledge_and_drop_interrupts_without_closing_other_calls() {
        block_on(async {
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (mut peer, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, driver) = session(wire, incoming, env(), grants(), 0);
            let input = serde_json::from_value(json!({"threadId":"thread"})).unwrap();
            let mut stream =
                endpoint.subscribe_typed::<WsOrchestrationV2SubscribePromptSuggestionRpc>(input);
            let controller = async {
                let request = decode_request(requests.next().await.unwrap());
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Chunk {
                    request_id: request.id.clone(),
                    values: vec![Value::Null],
                })
                .unwrap()))
                    .await
                    .unwrap();
                assert!(stream.next().await.unwrap().unwrap().is_none());
                let ack: Vec<ClientMessage> =
                    codec::decode(&requests.next().await.unwrap()).unwrap();
                assert!(
                    matches!(&ack[0], ClientMessage::Ack { request_id } if request_id == &request.id)
                );
                drop(stream);
                let interrupt: Vec<ClientMessage> =
                    codec::decode(&requests.next().await.unwrap()).unwrap();
                assert!(
                    matches!(&interrupt[0], ClientMessage::Interrupt { request_id } if request_id == &request.id)
                );
                let call = endpoint.call_typed::<WsServerProbeRpc>(Empty {});
                let request = decode_request(requests.next().await.unwrap());
                peer.send(Ok(codec::encode(&ServerMessage::<Value>::Exit {
                    request_id: request.id,
                    exit: Exit::success(json!({})),
                })
                .unwrap()))
                    .await
                    .unwrap();
                call.await.unwrap();
                endpoint.invalidate();
            };
            futures::join!(driver.run(futures::stream::pending(), || 0), controller);
        });
    }
    #[test]
    fn bounded_slow_stream_fails_explicitly_without_blocking_a_unary_reply() {
        block_on(async {
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (_, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, mut driver) = session(wire, incoming, env(), grants(), 0);
            let input = serde_json::from_value(json!({"threadId":"thread"})).unwrap();
            let mut stream =
                endpoint.subscribe_typed::<WsOrchestrationV2SubscribePromptSuggestionRpc>(input);
            for _ in 0..=MAX_BUFFERED_CHUNKS {
                driver.deliver(CallEvent::Chunk {
                    id: 0.into(),
                    values: vec![Value::Null],
                });
            }
            assert!(matches!(
                stream.next().await,
                Some(Err(RpcCallError::Overflow))
            ));
            assert!(stream.next().await.is_none());
            let call = endpoint.call_typed::<WsServerProbeRpc>(Empty {});
            driver.deliver(CallEvent::Exit {
                id: 1.into(),
                exit: Exit::success(json!({})),
            });
            assert!(call.await.is_ok());
            assert!(requests.try_recv().is_err());
        });
    }
    #[test]
    fn typed_failure_retains_domain_error_but_defects_remain_ambiguous_and_redacted() {
        block_on(async {
            let (wire, _) = mpsc::unbounded::<Frame>();
            let (_, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, mut driver) = session(wire, incoming, env(), grants(), 0);
            let call = endpoint.call_typed::<WsServerProbeRpc>(Empty {});
            let error = json!({"_tag":"EnvironmentAuthorizationError","message":"secret-marker","requiredScope":"orchestration:read"});
            driver.deliver(CallEvent::Exit {
                id: 0.into(),
                exit: Exit::fail(error),
            });
            let error = call.await.unwrap_err();
            assert!(matches!(error, RpcCallError::Rejected(_)));
            assert!(!format!("{error:?} {error}").contains("secret-marker"));
            let call = endpoint.command::<WsServerProbeRpc>(Empty {});
            driver.deliver(CallEvent::Exit {
                id: 1.into(),
                exit: Exit::die(json!("private")),
            });
            assert!(matches!(call.await, Err(RpcCallError::Defect)));
            // Effect's answer for a method the server has no handler for is not a defect: the
            // method is not served there, and nothing ran.
            let call = endpoint.command::<WsServerProbeRpc>(Empty {});
            driver.deliver(CallEvent::Exit {
                id: 2.into(),
                exit: Exit::die(json!("Unknown request tag: server.probe")),
            });
            assert!(matches!(
                call.await,
                Err(RpcCallError::NotSent(ClientError::Unsupported(_)))
            ));
        });
    }
    #[test]
    fn invalid_success_is_not_classified_as_definitive_rejection() {
        block_on(async {
            let (wire, _) = mpsc::unbounded::<Frame>();
            let (_, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, mut driver) = session(wire, incoming, env(), grants(), 0);
            let call = endpoint.command::<WsServerProbeRpc>(Empty {});
            driver.deliver(CallEvent::Exit {
                id: 0.into(),
                exit: Exit::success(json!(42)),
            });
            assert!(matches!(call.await, Err(RpcCallError::InvalidResponse)));
        });
    }
    #[test]
    fn unknown_methods_missing_sessions_and_wrong_call_kinds_send_no_requests() {
        struct Unknown;
        impl RpcMethod for Unknown {
            type Input = Value;
            type Output = Value;
            const NAME: &'static str = "unknown";
        }
        block_on(async {
            let (wire, _) = mpsc::unbounded::<Frame>();
            let (_, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let (endpoint, _driver) = session(wire, incoming, env(), grants(), 0);
            assert!(matches!(
                endpoint.command::<Unknown>(json!({})).await,
                Err(RpcCallError::NotSent(ClientError::PermissionDenied { .. }))
            ));
            assert!(matches!(
                endpoint
                    .subscribe::<WsServerProbeRpc>(Empty {})
                    .next()
                    .await,
                Some(Err(ClientError::Unsupported(_)))
            ));
            endpoint.update_grants(SessionGrants {
                valid: false,
                ..grants()
            });
            assert!(!endpoint.available::<WsServerProbeRpc>(&Empty {}));
        });
    }
    #[test]
    fn input_dependent_contract_grants_and_revocation_are_checked_again_at_execution() {
        block_on(async {
            let (wire, mut requests) = mpsc::unbounded::<Frame>();
            let (_, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
            let mut permission = grants();
            permission.scopes.insert("source-control:write".into());
            let (endpoint, driver) = session(wire, incoming, env(), permission.clone(), 0);
            let input = || {
                serde_json::from_value(
                    json!({"cwd":"/repo","mode":"worktree","threadId":"thread","reference":"1"}),
                )
                .unwrap()
            };
            assert!(!endpoint.available::<WsGitPreparePullRequestThreadRpc>(&input()));
            permission.scopes.insert("orchestration:operate".into());
            endpoint.update_grants(permission);
            assert!(endpoint.available::<WsGitPreparePullRequestThreadRpc>(&input()));
            let call = endpoint.command::<WsGitPreparePullRequestThreadRpc>(input());
            endpoint.update_grants(grants());
            let controller = async {
                assert!(matches!(
                    call.await,
                    Err(RpcCallError::NotSent(ClientError::PermissionDenied { .. }))
                ));
                assert!(requests.try_recv().is_err());
                endpoint.invalidate();
            };
            futures::join!(driver.run(futures::stream::pending(), || 0), controller);
        });
    }
    #[test]
    fn dropping_queued_calls_keeps_the_driver_command_buffer_bounded() {
        let (wire, _) = mpsc::unbounded::<Frame>();
        let (_, incoming) = mpsc::unbounded::<Result<Frame, mpsc::SendError>>();
        let (endpoint, _driver) = session(wire, incoming, env(), grants(), 0);
        for _ in 0..MAX_CALLS {
            drop(endpoint.call::<WsServerProbeRpc>(Empty {}));
        }
        assert_eq!(endpoint.inner.borrow().queued_starts, MAX_CALLS);
        assert!(matches!(
            block_on(endpoint.command::<WsServerProbeRpc>(Empty {})),
            Err(RpcCallError::NotSent(ClientError::Unsupported(_)))
        ));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod http_tests {
    use super::*;
    use crate::{
        platform::{Credentials, HttpRequest, HttpResponse, Platform, PlatformError},
        testing::FakePlatform,
    };
    use futures::executor::block_on;
    use std::collections::BTreeMap;
    fn request() -> HttpRequest {
        HttpRequest {
            method: "GET".into(),
            url: "https://host.test/api/config".into(),
            headers: BTreeMap::new(),
            body: Vec::new(),
            credentials: Credentials::SameOrigin,
            max_response_bytes: 1024,
        }
    }
    #[test]
    fn http_boundary_distinguishes_success_decode_status_network_and_declared_errors() {
        block_on(async {
            let host = FakePlatform::default();
            for (status, body, expected) in [
                (200, b"{}".to_vec(), "ok"),
                (200, b"invalid".to_vec(), "json"),
                (504, b"gateway".to_vec(), "status"),
            ] {
                host.push_http(Ok(HttpResponse {
                    status,
                    headers: BTreeMap::new(),
                    body,
                }));
                let result = execute_environment_http_request::<Value>(&host, request(), 100).await;
                match expected {
                    "ok" => assert!(result.is_ok()),
                    "json" => assert!(matches!(result, Err(HttpRpcError::InvalidJson))),
                    _ => assert!(matches!(
                        result,
                        Err(HttpRpcError::UndeclaredStatus { status: 504 })
                    )),
                }
            }
            host.push_http(Err(PlatformError::Network("secret".into())));
            let error = execute_environment_http_request::<Value>(&host, request(), 100)
                .await
                .unwrap_err();
            assert!(matches!(error, HttpRpcError::Fetch));
            assert!(!format!("{error:?} {error}").contains("secret"));
            assert_eq!(
                remote_api_base_url("https://host.test/path?q=1#fragment").unwrap(),
                "https://host.test/"
            );
        });
    }
    struct Stalled(FakePlatform);
    impl crate::platform::Storage for Stalled {
        fn get(
            &self,
            area: crate::platform::StorageArea,
            key: crate::platform::StorageKey,
        ) -> crate::platform::HostFuture<'_, Option<Vec<u8>>> {
            self.0.get(area, key)
        }
        fn write(
            &self,
            area: crate::platform::StorageArea,
            writes: Vec<crate::platform::StorageWrite>,
        ) -> crate::platform::HostFuture<'_, ()> {
            self.0.write(area, writes)
        }
        fn list(
            &self,
            area: crate::platform::StorageArea,
            namespace: String,
        ) -> crate::platform::HostFuture<'_, Vec<crate::platform::StorageKey>> {
            self.0.list(area, namespace)
        }
    }
    impl Platform for Stalled {
        fn now_epoch_ms(&self) -> u64 {
            self.0.now_epoch_ms()
        }
        fn monotonic_ms(&self) -> u64 {
            self.0.monotonic_ms()
        }
        fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
            self.0.random_bytes(length)
        }
        fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
            self.0.sleep(millis)
        }
        fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
            self.0.yield_to_host()
        }
        fn events(&self) -> LocalBoxStream<'static, crate::platform::PlatformEvent> {
            self.0.events()
        }
        fn http(&self, _request: HttpRequest) -> crate::platform::HostFuture<'_, HttpResponse> {
            futures::future::pending().boxed_local()
        }
        fn websocket(
            &self,
            request: crate::platform::SocketRequest,
        ) -> crate::platform::HostFuture<'_, Box<dyn crate::platform::WebSocket>> {
            self.0.websocket(request)
        }
        fn host(
            &self,
            action: crate::platform::HostAction,
        ) -> crate::platform::HostFuture<'_, crate::platform::HostReply> {
            self.0.host(action)
        }
    }
    #[test]
    fn http_deadline_uses_the_injected_clock() {
        block_on(async {
            let platform = Stalled(FakePlatform::default());
            let mut request = Box::pin(execute_environment_http_request::<Value>(
                &platform,
                request(),
                50,
            ));
            assert!(futures::poll!(request.as_mut()).is_pending());
            platform.0.advance(49);
            assert!(futures::poll!(request.as_mut()).is_pending());
            platform.0.advance(1);
            assert!(matches!(
                request.await,
                Err(HttpRpcError::Timeout { timeout_ms: 50 })
            ));
        });
    }
}
