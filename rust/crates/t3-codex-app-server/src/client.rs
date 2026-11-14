use crate::{
    Error, Result, RpcError,
    types::*,
    wire::{Message, Notification, Params, RequestId},
};
use futures_core::Stream;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub max_frame_bytes: usize,
    pub max_server_requests: usize,
    pub event_capacity: usize,
}
impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            max_frame_bytes: 128 * 1024 * 1024,
            max_server_requests: 32,
            event_capacity: 64,
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Notification {
        raw: Notification,
        decoded: Box<ServerNotification>,
    },
    Request(Box<ServerRequest>),
    /// One diagnostic per method/category. The rejected payload is never retained.
    NotificationDecodeFailed(Error),
    FrameDecodeFailed(Error),
}

pub struct Events {
    receiver: mpsc::Receiver<(u64, Event)>,
    inner: Arc<Inner>,
    terminal_delivered: bool,
    in_flight: Option<u64>,
}
impl Events {
    /// Marks the last event handled. Polling the next event does this automatically.
    /// Call explicitly before awaiting an RPC if the consumer will not poll again yet.
    pub fn acknowledge(&mut self) {
        if let Some(sequence) = self.in_flight.take() {
            self.inner.delivered.send_replace(sequence);
        }
    }
    pub async fn recv(&mut self) -> Option<Result<Event>> {
        std::future::poll_fn(|cx| Pin::new(&mut *self).poll_next(cx)).await
    }
}
impl Stream for Events {
    type Item = Result<Event>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.acknowledge();
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some((sequence, event))) => {
                self.in_flight = Some(sequence);
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(None) if !self.terminal_delivered => {
                self.terminal_delivered = true;
                Poll::Ready(self.inner.state().failure.clone().map(Err))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
impl Drop for Events {
    fn drop(&mut self) {
        self.inner.terminate(Error::Closed);
    }
}

type PendingReply = oneshot::Sender<Result<Value>>;
struct State {
    next_id: i64,
    pending: HashMap<String, (String, PendingReply)>,
    server_requests: HashMap<RequestId, CancellationToken>,
    failure: Option<Error>,
    events: Option<mpsc::Sender<(u64, Event)>>,
    event_sequence: u64,
    warned: HashSet<String>,
}
enum Outgoing {
    Frame(Vec<u8>, Option<oneshot::Sender<Result<()>>>),
    Shutdown(oneshot::Sender<Result<()>>),
}
struct Inner {
    state: Mutex<State>,
    outgoing: mpsc::UnboundedSender<Outgoing>,
    stop: CancellationToken,
    options: ClientOptions,
    termination: watch::Sender<Option<Error>>,
    delivered: watch::Sender<u64>,
}
impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn terminate(&self, error: Error) {
        let mut state = self.state();
        if state.failure.is_some() {
            return;
        }
        state.failure = Some(error.clone());
        for (_, (_, reply)) in state.pending.drain() {
            let _ = reply.send(Err(error.clone()));
        }
        for (_, token) in state.server_requests.drain() {
            token.cancel();
        }
        state.events.take();
        self.termination.send_replace(Some(error));
        self.stop.cancel();
    }
    fn enqueue(&self, message: Message, ack: Option<oneshot::Sender<Result<()>>>) -> Result<()> {
        let bytes = message.encode()?;
        let state = self.state();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        self.outgoing
            .send(Outgoing::Frame(bytes, ack))
            .map_err(|_| Error::Closed)
    }

    async fn event(&self, event: Event) {
        let target = {
            let mut state = self.state();
            state.event_sequence += 1;
            state
                .events
                .clone()
                .map(|sender| (sender, state.event_sequence))
        };
        if let Some((sender, sequence)) = target {
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => {},
                _ = sender.send((sequence, event)) => {},
            }
        }
    }
    async fn preceding_events_processed(&self) -> Result<()> {
        let sequence = self.state().event_sequence;
        let mut delivered = self.delivered.subscribe();
        loop {
            if *delivered.borrow_and_update() >= sequence {
                return Ok(());
            }
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => return Err(Error::Closed),
                changed = delivered.changed() => { changed.map_err(|_| Error::Closed)?; }
            }
        }
    }
    fn finish_server_request(
        &self,
        id: &RequestId,
        token: &CancellationToken,
        result: std::result::Result<Value, RpcError>,
        ack: Option<oneshot::Sender<Result<()>>>,
    ) -> Result<()> {
        let bytes = Message::Response {
            id: id.clone(),
            result,
        }
        .encode()?;
        let mut state = self.state();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if token.is_cancelled() || state.server_requests.remove(id).is_none() {
            return Err(Error::RequestResolved);
        }
        token.cancel();
        self.outgoing
            .send(Outgoing::Frame(bytes, ack))
            .map_err(|_| Error::Closed)
    }
}

struct Ownership(Arc<Inner>);
impl Drop for Ownership {
    fn drop(&mut self) {
        self.0.terminate(Error::Closed);
    }
}

#[derive(Clone)]
pub struct Client {
    owner: Arc<Ownership>,
    initialization: Arc<tokio::sync::Mutex<Handshake>>,
}
#[derive(Default)]
struct Handshake {
    response: Option<InitializeResponse>,
    accepted: bool,
    initialized: bool,
}

struct PendingGuard {
    inner: Arc<Inner>,
    key: String,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.inner.state().pending.remove(&self.key);
    }
}

pub struct ServerRequest {
    id: RequestId,
    method: String,
    params: ServerRequestParams,
    inner: Arc<Inner>,
    resolved: CancellationToken,
}
impl std::fmt::Debug for ServerRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerRequest")
            .field("id", &self.id)
            .field("method", &self.method)
            .field("resolved", &self.is_resolved())
            .finish_non_exhaustive()
    }
}
impl ServerRequest {
    pub fn id(&self) -> &RequestId {
        &self.id
    }
    pub fn method(&self) -> &str {
        &self.method
    }
    pub fn params(&self) -> &ServerRequestParams {
        &self.params
    }
    pub fn is_resolved(&self) -> bool {
        self.resolved.is_cancelled()
    }
    pub async fn cancelled(&self) {
        self.resolved.cancelled().await;
    }
    async fn respond_serialized<T: Serialize>(self, result: T) -> Result<()> {
        let result = match serde_json::to_value(result) {
            Ok(value) => value,
            Err(error) => {
                self.answer(Err(RpcError {
                    code: -32603.0,
                    message: "Cannot encode response".into(),
                    data: Field::Missing,
                }))
                .await?;
                return Err(Error::payload(&self.method, error));
            }
        };
        self.answer(Ok(result)).await
    }
    pub async fn reject(self, error: RpcError) -> Result<()> {
        self.answer(Err(error)).await
    }
    async fn answer(&self, result: std::result::Result<Value, RpcError>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .finish_server_request(&self.id, &self.resolved, result, Some(tx))?;
        rx.await
            .unwrap_or_else(|_| Err(self.inner.state().failure.clone().unwrap_or(Error::Closed)))
    }
}

/// A request carries the response type selected by its original wire method.
///
/// ```compile_fail
/// use t3_codex_app_server::{TypedRequest, CommandApprovalParams, ApprovalResponse, UserInputResponse};
/// async fn wrong(request: TypedRequest<CommandApprovalParams, ApprovalResponse>) {
///     request.respond(UserInputResponse::default()).await.unwrap();
/// }
/// ```
#[derive(Debug)]
pub struct TypedRequest<P, R> {
    pub params: P,
    request: ServerRequest,
    response: std::marker::PhantomData<R>,
}
impl<P, R: Serialize> TypedRequest<P, R> {
    pub fn id(&self) -> &RequestId {
        self.request.id()
    }
    pub fn method(&self) -> &str {
        self.request.method()
    }
    pub fn is_resolved(&self) -> bool {
        self.request.is_resolved()
    }
    pub async fn cancelled(&self) {
        self.request.cancelled().await;
    }
    pub async fn respond(self, result: R) -> Result<()> {
        self.request.respond_serialized(result).await
    }
    pub async fn reject(self, error: RpcError) -> Result<()> {
        self.request.reject(error).await
    }
}

#[derive(Debug)]
pub enum TypedServerRequest {
    CommandApproval(TypedRequest<CommandApprovalParams, ApprovalResponse>),
    FileChangeApproval(TypedRequest<FileChangeApprovalParams, FileChangeApprovalResponse>),
    PermissionsApproval(TypedRequest<PermissionsApprovalParams, PermissionsApprovalResponse>),
    UserInput(TypedRequest<UserInputParams, UserInputResponse>),
    Elicitation(TypedRequest<ElicitationParams, ElicitationResponse>),
    LegacyExecApproval(TypedRequest<LegacyExecApprovalParams, LegacyExecApprovalResponse>),
    LegacyPatchApproval(TypedRequest<LegacyPatchApprovalParams, LegacyPatchApprovalResponse>),
    Unknown(TypedRequest<Params, Value>),
}
impl ServerRequest {
    pub fn into_typed(mut self) -> TypedServerRequest {
        let params = std::mem::replace(
            &mut self.params,
            ServerRequestParams::Unknown {
                method: String::new(),
                params: Params::Missing,
            },
        );
        macro_rules! typed {
            ($variant:ident, $params:expr) => {
                TypedServerRequest::$variant(TypedRequest {
                    params: $params,
                    request: self,
                    response: std::marker::PhantomData,
                })
            };
        }
        match params {
            ServerRequestParams::CommandApproval(p) => typed!(CommandApproval, p),
            ServerRequestParams::FileChangeApproval(p) => typed!(FileChangeApproval, p),
            ServerRequestParams::PermissionsApproval(p) => typed!(PermissionsApproval, p),
            ServerRequestParams::UserInput(p) => typed!(UserInput, p),
            ServerRequestParams::Elicitation(p) => typed!(Elicitation, p),
            ServerRequestParams::LegacyExecApproval(p) => typed!(LegacyExecApproval, p),
            ServerRequestParams::LegacyPatchApproval(p) => typed!(LegacyPatchApproval, p),
            ServerRequestParams::Unknown { params, .. } => typed!(Unknown, params),
        }
    }
}

impl Drop for ServerRequest {
    fn drop(&mut self) {
        if !self.is_resolved() {
            let _ = self.inner.finish_server_request(
                &self.id,
                &self.resolved,
                Err(RpcError::method_not_found(&self.method)),
                None,
            );
        }
    }
}

impl Client {
    pub fn connect<R, W>(reader: R, writer: W, options: ClientOptions) -> (Self, Events)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, outgoing_rx) = mpsc::unbounded_channel();
        let (events, events_rx) = mpsc::channel(options.event_capacity.max(1));
        let (delivered, _) = watch::channel(0);
        let (termination, _) = watch::channel(None);
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                next_id: 1,
                pending: HashMap::new(),
                server_requests: HashMap::new(),
                failure: None,
                events: Some(events),
                event_sequence: 0,
                warned: HashSet::new(),
            }),
            outgoing,
            stop: CancellationToken::new(),
            options,
            termination,
            delivered,
        });
        tokio::spawn(read_loop(reader, inner.clone()));
        tokio::spawn(write_loop(writer, outgoing_rx, inner.clone()));
        (
            Self {
                owner: Arc::new(Ownership(inner.clone())),
                initialization: Arc::new(tokio::sync::Mutex::new(Handshake::default())),
            },
            Events {
                receiver: events_rx,
                inner,
                terminal_delivered: false,
                in_flight: None,
            },
        )
    }

    /// Dropping this future removes only its local waiter; Codex has no RPC cancel.
    pub async fn request_raw(&self, method: &str, params: Params) -> Result<Value> {
        let inner = &self.owner.0;
        let (tx, rx) = oneshot::channel();
        let (id, guard) = {
            let mut state = inner.state();
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            let id = state.next_id;
            state.next_id = id
                .checked_add(1)
                .ok_or(Error::Protocol("request id exhausted"))?;
            let key = id.to_string();
            state.pending.insert(key.clone(), (method.into(), tx));
            (
                RequestId::Integer(id),
                PendingGuard {
                    inner: inner.clone(),
                    key,
                },
            )
        };
        inner.enqueue(
            Message::Request {
                id,
                method: method.into(),
                params,
            },
            None,
        )?;
        let result = rx
            .await
            .unwrap_or_else(|_| Err(inner.state().failure.clone().unwrap_or(Error::Closed)));
        drop(guard);
        result
    }

    pub async fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R> {
        let params = serde_json::to_value(params).map_err(|e| Error::payload(method, e))?;
        crate::error::decode_payload(
            method,
            &self.request_raw(method, Params::Present(params)).await?,
        )
    }

    pub async fn notify(&self, method: &str, params: Params) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.owner.0.enqueue(
            Message::Notification(Notification {
                method: method.into(),
                params,
            }),
            Some(tx),
        )?;
        rx.await.unwrap_or_else(|_| {
            Err(self
                .owner
                .0
                .state()
                .failure
                .clone()
                .unwrap_or(Error::Closed))
        })
    }

    /// Serialized once per connection, including across clones; retryable on failure.
    /// Returns no server metadata when recovering an interrupted handshake via
    /// Codex's exact `Already initialized` error.
    pub async fn initialize(
        &self,
        params: &InitializeParams,
    ) -> Result<Option<InitializeResponse>> {
        let mut handshake = self.initialization.lock().await;
        if !handshake.accepted {
            match self.request("initialize", params).await {
                Ok(response) => handshake.response = Some(response),
                Err(Error::Rpc { error, .. })
                    if error.code == -32600.0 && error.message == "Already initialized" =>
                {
                    handshake.response = None;
                }
                Err(error) => return Err(error),
            }
            handshake.accepted = true;
        }
        Ok(handshake.response.clone())
    }

    /// Call after the provider's version compatibility gate has accepted the server.
    pub async fn initialized(&self) -> Result<()> {
        let mut handshake = self.initialization.lock().await;
        if !handshake.accepted {
            return Err(Error::Protocol("initialize has not completed"));
        }
        if !handshake.initialized {
            self.notify("initialized", Params::Missing).await?;
            handshake.initialized = true;
        }
        Ok(())
    }

    pub async fn await_termination(&self) -> Error {
        let mut termination = self.owner.0.termination.subscribe();
        loop {
            if let Some(error) = termination.borrow_and_update().clone() {
                return error;
            }
            if termination.changed().await.is_err() {
                return Error::Closed;
            }
        }
    }

    /// Flushes queued writes, closes the writer, then stops the reader and all waiters.
    /// There is no native `shutdown` method; the process host owns exit/kill deadlines.
    pub async fn shutdown(&self) -> Result<()> {
        if let Some(error) = self.owner.0.state().failure.clone() {
            return Err(error);
        }
        let (tx, rx) = oneshot::channel();
        self.owner
            .0
            .outgoing
            .send(Outgoing::Shutdown(tx))
            .map_err(|_| Error::Closed)?;
        rx.await.unwrap_or_else(|_| {
            Err(self
                .owner
                .0
                .state()
                .failure
                .clone()
                .unwrap_or(Error::Closed))
        })
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut outgoing: mpsc::UnboundedReceiver<Outgoing>,
    inner: Arc<Inner>,
) {
    loop {
        let next = tokio::select! { biased; _ = inner.stop.cancelled() => break, next = outgoing.recv() => next };
        let Some(next) = next else {
            break;
        };
        let (result, ack, shutdown) = match next {
            Outgoing::Frame(bytes, ack) => {
                let result = tokio::select! {
                    biased;
                    _ = inner.stop.cancelled() => break,
                    result = async { writer.write_all(&bytes).await?; writer.flush().await } => result,
                };
                (result, ack, false)
            }
            Outgoing::Shutdown(ack) => {
                let result = tokio::select! { biased; _ = inner.stop.cancelled() => break, result = writer.shutdown() => result };
                (result, Some(ack), true)
            }
        };
        let result = result.map_err(|e| Error::Transport {
            operation: "write",
            message: e.to_string(),
        });
        if let Err(error) = &result {
            inner.terminate(error.clone());
        }
        let failed = result.is_err();
        if let Some(ack) = ack {
            let _ = ack.send(result);
        }
        if shutdown || failed {
            inner.terminate(Error::Closed);
            break;
        }
    }
}

async fn read_loop<R: AsyncRead + Unpin>(mut reader: R, inner: Arc<Inner>) {
    let mut chunk = [0; 8192];
    let mut frame = Vec::new();
    let mut discarding = false;
    let result: Result<()> = async {
        loop {
            let length = tokio::select! { biased; _ = inner.stop.cancelled() => return Ok(()), read = reader.read(&mut chunk) => read.map_err(|e| Error::Transport { operation: "read", message: e.to_string() })? };
            if length == 0 {
                if !frame.is_empty() { route_frame(&frame, &inner).await?; }
                return Err(Error::Closed);
            }
            let mut start = 0;
            while start < length {
                let end = chunk[start..length].iter().position(|b| *b == b'\n').map(|n| start + n).unwrap_or(length);
                if discarding {
                    discarding = end == length;
                    start = end + 1;
                    continue;
                }
                if frame.len() + end - start > inner.options.max_frame_bytes {
                    frame.clear();
                    discarding = end == length;
                    inner.event(Event::FrameDecodeFailed(Error::FrameTooLarge { limit: inner.options.max_frame_bytes })).await;
                    start = end + 1;
                    continue;
                }
                frame.extend_from_slice(&chunk[start..end]);
                if end < length {
                    if frame.last() == Some(&b'\r') { frame.pop(); }
                    route_frame(&frame, &inner).await?;
                    frame.clear();
                }
                start = end + 1;
            }
        }
    }.await;
    if let Err(error) = result {
        inner.terminate(error);
    }
}

async fn route_frame(frame: &[u8], inner: &Arc<Inner>) -> Result<()> {
    if frame.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let message = match Message::decode(frame) {
        Ok(message) => message,
        Err(error) => {
            inner.preceding_events_processed().await?;
            if let Ok(Value::Object(fields)) =
                serde_json::from_str::<Value>(&String::from_utf8_lossy(frame))
                && !fields.get("method").is_some_and(Value::is_string)
                && let Some(id) = fields
                    .get("id")
                    .and_then(|id| serde_json::from_value::<RequestId>(id.clone()).ok())
                && let Some((_, pending)) = inner.state().pending.remove(&id.to_string())
            {
                let _ = pending.send(Err(error));
                return Ok(());
            }
            inner.event(Event::FrameDecodeFailed(error)).await;
            return Ok(());
        }
    };
    match message {
        Message::Response { id, result } => {
            inner.preceding_events_processed().await?;
            let pending = inner.state().pending.remove(&id.to_string());
            if let Some((method, reply)) = pending {
                let _ = reply.send(result.map_err(|error| Error::Rpc {
                    id,
                    method,
                    error: Box::new(error),
                }));
            }
        }
        Message::Notification(raw) => {
            // Cancellation precedes payload decoding in the TypeScript protocol as well.
            if raw.method == "serverRequest/resolved"
                && let Params::Present(params) = &raw.params
                && let Some(id) = params.get("requestId")
                && let Ok(id) = serde_json::from_value::<RequestId>(id.clone())
                && let Some(token) = inner.state().server_requests.remove(&id)
            {
                token.cancel();
            }
            match ServerNotification::decode(&raw) {
                Ok(decoded) => {
                    inner
                        .event(Event::Notification {
                            raw,
                            decoded: Box::new(decoded),
                        })
                        .await;
                }
                Err(error) => {
                    let key = match &error {
                        Error::Payload {
                            method, category, ..
                        } => format!("{method}:{category}"),
                        _ => raw.method,
                    };
                    let warn = inner.state().warned.insert(key);
                    if warn {
                        inner.event(Event::NotificationDecodeFailed(error)).await;
                    }
                }
            }
        }
        Message::Request { id, method, params } => {
            if let Some(old) = inner.state().server_requests.remove(&id) {
                old.cancel();
            }
            let decoded = match ServerRequestParams::decode(&method, params) {
                Ok(decoded) => decoded,
                Err(_) => {
                    inner.enqueue(
                        Message::Response {
                            id,
                            result: Err(RpcError::invalid_params()),
                        },
                        None,
                    )?;
                    return Ok(());
                }
            };
            let token = CancellationToken::new();
            let admission = {
                let mut state = inner.state();
                if state.server_requests.len() >= inner.options.max_server_requests {
                    false
                } else {
                    state.server_requests.insert(id.clone(), token.clone());
                    true
                }
            };
            if !admission {
                inner.enqueue(
                    Message::Response {
                        id,
                        result: Err(RpcError {
                            code: -32001.0,
                            message: "Too many Codex requests are already active.".into(),
                            data: Field::Missing,
                        }),
                    },
                    None,
                )?;
            } else {
                inner
                    .event(Event::Request(Box::new(ServerRequest {
                        id,
                        method,
                        params: decoded,
                        inner: inner.clone(),
                        resolved: token,
                    })))
                    .await;
            }
        }
    }
    Ok(())
}

macro_rules! methods {
    ($($name:ident ($params:ty) -> $response:ty = $method:literal;)*) => { impl Client {
        $(pub async fn $name(&self, params: &$params) -> Result<$response> { self.request($method, params).await })*
    } };
}
methods! {
    thread_start(ThreadStartParams) -> ThreadResponse = "thread/start";
    thread_resume(ThreadResumeParams) -> ThreadResponse = "thread/resume";
    thread_read(ThreadReadParams) -> ThreadResponse = "thread/read";
    thread_fork(ThreadForkParams) -> ThreadResponse = "thread/fork";
    thread_turns_list(ThreadTurnsListParams) -> ThreadTurnsListResponse = "thread/turns/list";
    thread_revert(ThreadRevertParams) -> ThreadResponse = "thread/revert";
    thread_archive(ThreadIdParams) -> Empty = "thread/archive";
    thread_unarchive(ThreadIdParams) -> ThreadResponse = "thread/unarchive";
    thread_unsubscribe(ThreadIdParams) -> Empty = "thread/unsubscribe";
    thread_inject_items(ThreadInjectItemsParams) -> Empty = "thread/inject_items";
    thread_compact_start(ThreadIdParams) -> Empty = "thread/compact/start";
    turn_start(TurnStartParams) -> TurnResponse = "turn/start";
    turn_steer(TurnSteerParams) -> TurnSteerResponse = "turn/steer";
    turn_interrupt(TurnInterruptParams) -> Empty = "turn/interrupt";
    thread_goal_get(ThreadIdParams) -> ThreadGoalResponse = "thread/goal/get";
    thread_goal_set(ThreadGoalSetParams) -> ThreadGoalResponse = "thread/goal/set";
    thread_goal_clear(ThreadIdParams) -> ThreadGoalClearResponse = "thread/goal/clear";
    config_read(ConfigReadParams) -> ConfigReadResponse = "config/read";
    feedback_upload(FeedbackUploadParams) -> FeedbackUploadResponse = "feedback/upload";
    model_list(ModelListParams) -> ModelListResponse = "model/list";
    account_read(GetAccountParams) -> GetAccountResponse = "account/read";
    account_rate_limit_reset_credit_consume(ConsumeResetCreditParams) -> ConsumeResetCreditResponse = "account/rateLimitResetCredit/consume";
    skills_list(SkillsListParams) -> SkillsListResponse = "skills/list";
    mcp_server_status_list(ListMcpServerStatusParams) -> ListMcpServerStatusResponse = "mcpServerStatus/list";
    mcp_server_tool_call(McpServerToolCallParams) -> McpServerToolCallResponse = "mcpServer/tool/call";
    mcp_resource_read(McpResourceReadParams) -> McpResourceReadResponse = "mcpServer/resource/read";
    thread_sample(ThreadSampleParams) -> ThreadSampleResponse = "thread/sample";
    background_terminals_list(BackgroundTerminalsListParams) -> BackgroundTerminalsListResponse = "thread/backgroundTerminals/list";
    background_terminals_terminate(BackgroundTerminalsTerminateParams) -> BackgroundTerminalsTerminateResponse = "thread/backgroundTerminals/terminate";
}
impl Client {
    pub async fn account_rate_limits_read(&self) -> Result<GetAccountRateLimitsResponse> {
        self.request("account/rateLimits/read", &Value::Null).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;
    use std::{future::Future, task::Waker};

    #[tokio::test]
    async fn unencodable_reply_is_an_error_only_for_its_request() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        struct Unencodable;
        impl Serialize for Unencodable {
            fn serialize<S: serde::Serializer>(
                &self,
                _: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("private-payload"))
            }
        }
        let (read, _keep_open) = tokio::io::duplex(1024);
        let (write, peer) = tokio::io::duplex(1024);
        let (client, mut events) = Client::connect(read, write, Default::default());
        let mut peer = BufReader::new(peer);
        let mut pending = std::pin::pin!(client.request_raw("x", Params::Missing));
        assert!(
            pending
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        let mut line = String::new();
        peer.read_line(&mut line).await.unwrap();
        route_frame(br#"{"id":7,"method":"future/approval"}"#, &client.owner.0)
            .await
            .unwrap();
        let Event::Request(request) = events.recv().await.unwrap().unwrap() else {
            panic!("expected request")
        };
        let error = request.respond_serialized(Unencodable).await.unwrap_err();
        events.acknowledge();
        assert!(matches!(error, Error::Payload { .. }));
        assert!(!format!("{error:?}").contains("private-payload"));
        line.clear();
        peer.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"id":7,"error":{"code":-32603,"message":"Cannot encode response"}})
        );
        route_frame(br#"{"id":1,"result":"ok"}"#, &client.owner.0)
            .await
            .unwrap();
        assert_eq!(pending.await.unwrap(), json!("ok"));
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn response_waits_until_preceding_notification_is_handled() {
        let (read, _peer) = tokio::io::duplex(1024);
        let (client, mut events) = Client::connect(read, tokio::io::sink(), Default::default());
        let mut request = std::pin::pin!(client.request_raw("x", Params::Missing));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(request.as_mut().poll(&mut cx).is_pending());
        let transcript = b"{\"method\":\"future/notification\"}\n{\"id\":1,\"result\":{}}\n";
        let mut response = std::pin::pin!(read_loop(transcript.as_slice(), client.owner.0.clone()));
        assert!(response.as_mut().poll(&mut cx).is_pending());
        assert!(request.as_mut().poll(&mut cx).is_pending());
        assert!(matches!(
            events.recv().await.unwrap().unwrap(),
            Event::Notification { .. }
        ));
        assert!(response.as_mut().poll(&mut cx).is_pending());
        assert!(request.as_mut().poll(&mut cx).is_pending());
        events.acknowledge();
        assert!(response.as_mut().poll(&mut cx).is_ready());
        assert_eq!(request.await.unwrap(), json!({}));
        assert_eq!(client.await_termination().await, Error::Closed);
    }

    #[tokio::test]
    async fn slow_consumer_backpressures_instead_of_growing_or_terminating() {
        let (read, _peer) = tokio::io::duplex(1024);
        let (client, mut events) = Client::connect(read, tokio::io::sink(), Default::default());

        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Script {
            remaining: usize,
            read: Arc<AtomicUsize>,
        }
        impl AsyncRead for Script {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _: &mut Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.remaining == 0 {
                    return Poll::Pending;
                }
                self.remaining -= 1;
                self.read.fetch_add(1, Ordering::SeqCst);
                buf.put_slice(b"{\"method\":\"future/notification\"}\n");
                Poll::Ready(Ok(()))
            }
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let mut script = std::pin::pin!(read_loop(
            Script {
                remaining: 66,
                read: reads.clone()
            },
            client.owner.0.clone()
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(script.as_mut().poll(&mut cx).is_pending());
        assert_eq!(reads.load(Ordering::SeqCst), 65);
        assert_eq!(events.receiver.len(), 64);
        assert!(client.owner.0.state().failure.is_none());
        events.recv().await.unwrap().unwrap();
        assert!(script.as_mut().poll(&mut cx).is_pending());
        assert_eq!(reads.load(Ordering::SeqCst), 66);
        assert_eq!(events.receiver.len(), 64);
        for _ in 0..64 {
            events.recv().await.unwrap().unwrap();
        }
        assert!(script.as_mut().poll(&mut cx).is_pending());
        events.recv().await.unwrap().unwrap();
        client.shutdown().await.unwrap();
        script.await;
    }
}
