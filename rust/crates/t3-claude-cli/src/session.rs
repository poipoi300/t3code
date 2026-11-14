use crate::{
    Error,
    control::*,
    message::{Decoded, Message},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    borrow::Cow, collections::HashMap, future::Future, marker::PhantomData, pin::Pin, sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const QUEUE_CAPACITY: usize = 32;

#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// Includes base64 media. A line exceeding this bound terminates the transport.
    pub max_line_bytes: usize,
    /// SDK ignores malformed non-JSON stdout diagnostics; strict mode rejects them.
    pub strict_json: bool,
    /// Replay initialize permission and dialog requests, filtered by their respective subtype.
    pub replay_pending_callbacks: bool,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            max_line_bytes: 64 * 1024 * 1024,
            strict_json: false,
            replay_pending_callbacks: true,
        }
    }
}

pub enum Event {
    /// Non-fatal compatibility issue. Raw content is available explicitly, never in error text.
    Diagnostic {
        error: Error,
        raw: Arc<Value>,
        original_bytes: Option<Vec<u8>>,
    },
    Message(Box<Message>),
    Request(Box<PendingRequest>),
    Cancelled {
        request_id: String,
    },
    /// Length only: arbitrary CLI stdout can contain credentials or user content.
    NonJson {
        bytes: usize,
    },
    UnmatchedResponse {
        request_id: String,
        raw: Value,
    },
}

pub enum PendingRequest {
    ToolPermission {
        request: Decoded<ToolPermission>,
        responder: Responder<PermissionDecision>,
    },
    Hook {
        request: Decoded<HookCallback>,
        responder: Responder<HookOutput>,
    },
    Elicitation {
        request: Decoded<Elicitation>,
        responder: Responder<ElicitationResponse>,
    },
    UserDialog {
        request: Decoded<UserDialog>,
        responder: Responder<DialogResponse>,
    },
    /// Servers registered with register_pending_mcp_server are handled by the caller.
    Mcp {
        request: Decoded<McpMessage>,
        responder: Responder<McpReply>,
    },
}

#[derive(Clone, Serialize)]
pub struct McpReply {
    pub mcp_response: Value,
}

/// The caller handles MCP JSON-RPC semantics; the client owns the outer control correlation.
pub trait McpServer: Send + Sync + 'static {
    /// Deliver JSON-RPC responses and notifications without waiting for a JSON-RPC reply.
    fn notify(&self, message: Value, cancellation: CancellationToken) -> Result<(), Error>;
    fn handle<'a>(
        &'a self,
        message: Value,
        cancellation: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Error>> + Send + 'a>>;
}

struct CallbackContext {
    cancellation: CancellationToken,
    response_slot: Mutex<Option<mpsc::OwnedPermit<WriteCommand>>>,
}
type ReplySender = oneshot::Sender<Result<ControlReply, Error>>;
struct PendingOutbound {
    reply: ReplySender,
    initialize: bool,
}
struct Core {
    writer: mpsc::Sender<WriteCommand>,
    writer_error: Mutex<Option<Error>>,
    pending: Mutex<HashMap<String, PendingOutbound>>,
    callbacks: Mutex<HashMap<String, Arc<CallbackContext>>>,
    mcp: Mutex<HashMap<String, Option<Arc<dyn McpServer>>>>,
    closed: CancellationToken,
    reason: Mutex<Option<Error>>,
    cli_version: Mutex<Option<String>>,
}
impl Core {
    fn finish(&self, error: Error) {
        let mut reason = self.reason.lock();
        if reason.is_some() {
            return;
        }
        *reason = Some(error.clone());
        for (_, pending) in self.pending.lock().drain() {
            let _ = pending.reply.send(Err(error.clone()));
        }
        for (_, callback) in self.callbacks.lock().drain() {
            callback.cancellation.cancel();
        }
        self.closed.cancel();
    }
    fn check_open(&self) -> Result<(), Error> {
        self.reason.lock().clone().map_or(Ok(()), Err)
    }
    fn remove_callback(&self, id: &str, context: &Arc<CallbackContext>) {
        let mut callbacks = self.callbacks.lock();
        if callbacks.get(id).is_some_and(|c| Arc::ptr_eq(c, context)) {
            callbacks.remove(id);
        }
    }
    async fn enqueue(
        &self,
        frame: Value,
        cancellation: Option<CancellationToken>,
    ) -> Result<oneshot::Receiver<Result<(), Error>>, Error> {
        self.check_writable()?;
        let permit = tokio::select! { biased;
            _ = self.closed.cancelled() => return Err(Error::Closed),
            permit = self.writer.reserve() => permit.map_err(|_| Error::Closed)?,
        };
        self.check_writable()?;
        let (command, rx) = Self::frame_command(frame, cancellation)?;
        permit.send(command);
        Ok(rx)
    }
    fn check_writable(&self) -> Result<(), Error> {
        self.check_open()?;
        self.writer_error.lock().clone().map_or(Ok(()), Err)
    }
    fn frame_command(
        frame: Value,
        cancellation: Option<CancellationToken>,
    ) -> Result<(WriteCommand, oneshot::Receiver<Result<(), Error>>), Error> {
        let mut bytes = serde_json::to_vec(&frame)?;
        bytes.push(b'\n');
        let (reply, rx) = oneshot::channel();
        Ok((
            WriteCommand::Frame {
                bytes,
                reply,
                cancellation,
            },
            rx,
        ))
    }
    // Outbound-future cancellation is best effort when the bounded writer queue is full.
    fn try_enqueue(&self, frame: Value, cancellation: Option<CancellationToken>) {
        if self.check_writable().is_err() {
            return;
        }
        if let Ok((command, _)) = Self::frame_command(frame, cancellation) {
            let _ = self.writer.try_send(command);
        }
    }
    async fn write(
        &self,
        frame: Value,
        cancellation: Option<CancellationToken>,
    ) -> Result<(), Error> {
        self.enqueue(frame, cancellation)
            .await?
            .await
            .map_err(|_| Error::Closed)?
    }
}
enum WriteCommand {
    Frame {
        bytes: Vec<u8>,
        reply: oneshot::Sender<Result<(), Error>>,
        cancellation: Option<CancellationToken>,
    },
    End(oneshot::Sender<Result<(), Error>>),
}

pub struct Responder<T> {
    id: String,
    core: Arc<Core>,
    context: Arc<CallbackContext>,
    tool_use_id: Option<String>,
    answered: bool,
    marker: PhantomData<T>,
}
impl<T> Responder<T> {
    pub fn request_id(&self) -> &str {
        &self.id
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.context.cancellation.clone()
    }
    pub fn is_cancelled(&self) -> bool {
        self.context.cancellation.is_cancelled()
    }
    /// Preserves response extension fields while retaining callback identity and cancellation.
    pub async fn respond_raw(mut self, value: Value) -> Result<(), Error> {
        self.answer(Ok(value)).await
    }
    pub async fn reject(mut self, error: Error) -> Result<(), Error> {
        self.answer(Err(error)).await
    }
    async fn answer(&mut self, payload: Result<Value, Error>) -> Result<(), Error> {
        if self.is_cancelled() {
            self.answered = true;
            return Err(Error::Cancelled);
        }
        let frame = match payload {
            Ok(mut payload) => {
                if let Some(id) = &self.tool_use_id {
                    payload
                        .as_object_mut()
                        .ok_or_else(|| {
                            Error::Protocol("permission response must be an object".into())
                        })?
                        .insert("toolUseID".into(), Value::String(id.clone()));
                }
                json!({"type":"control_response","response":{"subtype":"success","request_id":self.id,"response":payload}})
            }
            Err(error) => error_response(&self.id, &error),
        };
        self.core.check_writable()?;
        let (command, written) = Core::frame_command(frame, Some(self.cancellation()))?;
        let slot = self
            .context
            .response_slot
            .lock()
            .take()
            .ok_or(Error::Closed)?;
        slot.send(command);
        self.answered = true;
        let result = written.await.map_err(|_| Error::Closed)?;
        self.core.remove_callback(&self.id, &self.context);
        result
    }
}
impl<T: Serialize> Responder<T> {
    pub async fn respond(mut self, value: T) -> Result<(), Error> {
        self.answer(Ok(serde_json::to_value(value)?)).await
    }
}
impl<T> Drop for Responder<T> {
    fn drop(&mut self) {
        if !self.answered
            && !self.is_cancelled()
            && let Some(slot) = self.context.response_slot.lock().take()
            && let Ok((command, _)) = Core::frame_command(
                error_response(
                    &self.id,
                    &Error::Protocol("callback responder dropped without an answer".into()),
                ),
                Some(self.cancellation()),
            )
        {
            slot.send(command);
        }
        self.core.remove_callback(&self.id, &self.context);
    }
}

#[derive(Clone)]
pub struct ControlReply {
    /// The entire response object, including redelivery metadata and future fields.
    pub raw: Value,
    pub payload: Option<Value>,
}
impl ControlReply {
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, Error> {
        Ok(T::deserialize(serde_stacker::Deserializer::new(
            self.payload.as_ref().unwrap_or(&Value::Null),
        ))?)
    }
}

#[derive(Clone)]
pub struct Client {
    core: Arc<Core>,
}
impl Client {
    pub async fn request(&self, request: OutboundRequest) -> Result<ControlReply, Error> {
        self.core.check_open()?;
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.core.pending.lock().insert(
            id.clone(),
            PendingOutbound {
                reply: tx,
                initialize: matches!(request, OutboundRequest::Initialize(_)),
            },
        );
        let mut guard = RequestGuard {
            id: id.clone(),
            core: self.core.clone(),
            queued: false,
        };
        // Register before enqueue: an in-memory peer can answer before the writer acknowledges.
        let written = self
            .core
            .enqueue(
                json!({"type":"control_request","request_id":id,"request":request}),
                None,
            )
            .await?;
        guard.queued = true;
        written.await.map_err(|_| Error::Closed)??;
        rx.await.map_err(|_| Error::Closed)?
    }
    pub async fn initialize(
        &self,
        request: Initialize,
    ) -> Result<Decoded<InitializeResult>, Error> {
        let reply = self
            .request(OutboundRequest::Initialize(Box::new(request)))
            .await?;
        let result = Decoded::<InitializeResult>::from_value(reply.payload.unwrap_or(Value::Null))?;
        if let Some(version) = &result.get().claude_code_version {
            *self.core.cli_version.lock() = Some(version.clone());
        }
        Ok(result)
    }
    /// Send only after initialize has been written. Multiple turns share this open input.
    pub async fn send_user(&self, message: UserMessage) -> Result<(), Error> {
        self.core.write(serde_json::to_value(message)?, None).await
    }
    pub async fn interrupt(&self) -> Result<ControlReply, Error> {
        self.request(OutboundRequest::Interrupt).await
    }
    pub fn cli_version(&self) -> Option<String> {
        self.core.cli_version.lock().clone()
    }
    pub fn register_mcp_server(&self, name: impl Into<String>, server: Arc<dyn McpServer>) {
        self.core.mcp.lock().insert(name.into(), Some(server));
    }
    pub fn register_pending_mcp_server(&self, name: impl Into<String>) {
        self.core.mcp.lock().insert(name.into(), None);
    }
    pub fn remove_mcp_server(&self, name: &str) {
        self.core.mcp.lock().remove(name);
    }
    pub async fn send_mcp_message(
        &self,
        server_name: String,
        message: Value,
    ) -> Result<ControlReply, Error> {
        self.request(OutboundRequest::McpMessage {
            server_name,
            message,
        })
        .await
    }
    /// Half-close stdin; continue draining stdout. Does not interrupt a running turn.
    pub async fn end_input(&self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.core
            .writer
            .send(WriteCommand::End(tx))
            .await
            .map_err(|_| Error::Closed)?;
        rx.await.map_err(|_| Error::Closed)?
    }
}
struct RequestGuard {
    id: String,
    core: Arc<Core>,
    queued: bool,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        let pending = self.core.pending.lock().remove(&self.id).is_some();
        if pending && self.queued {
            self.core.try_enqueue(
                json!({"type":"control_cancel_request","request_id":self.id}),
                None,
            );
        }
    }
}

/// Owns bounded reader/writer queues. Drain events concurrently with client operations.
/// Drain events continuously; dropping Session closes both halves and cancels every pending operation.
pub struct Session {
    client: Client,
    events: mpsc::Receiver<Result<Event, Error>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}
impl Session {
    pub fn connect<R, W>(read: R, write: W, options: SessionOptions) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (writer_tx, writer_rx) = mpsc::channel(QUEUE_CAPACITY);
        let (events_tx, events) = mpsc::channel(QUEUE_CAPACITY);
        let core = Arc::new(Core {
            writer: writer_tx,
            writer_error: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            callbacks: Mutex::new(HashMap::new()),
            mcp: Mutex::new(HashMap::new()),
            closed: CancellationToken::new(),
            reason: Mutex::new(None),
            cli_version: Mutex::new(None),
        });
        let writer = tokio::spawn(write_loop(
            write,
            writer_rx,
            core.clone(),
            events_tx.clone(),
        ));
        let reader = tokio::spawn(read_loop(read, options, core.clone(), events_tx));
        Self {
            client: Client { core },
            events,
            reader: Some(reader),
            writer: Some(writer),
        }
    }
    pub fn client(&self) -> Client {
        self.client.clone()
    }
    pub async fn next_event(&mut self) -> Option<Result<Event, Error>> {
        self.events.recv().await
    }
    /// Flush for at most five seconds, then cancel I/O and callbacks. Process exit is caller-owned.
    pub async fn shutdown(&mut self) -> Result<(), Error> {
        let flush =
            tokio::time::timeout(std::time::Duration::from_secs(5), self.client.end_input()).await;
        self.join_shutdown(match flush {
            Ok(result) => result,
            Err(_) => Ok(()),
        })
        .await
    }
    /// Caller-owned cancellation bounds a graceful flush, including a blocked writer.
    pub async fn shutdown_with_cancellation(
        &mut self,
        cancellation: CancellationToken,
    ) -> Result<(), Error> {
        let result = tokio::select! { biased;
            _ = cancellation.cancelled() => Ok(()),
            result = self.client.end_input() => result,
        };
        self.join_shutdown(result).await
    }
    async fn join_shutdown(&mut self, result: Result<(), Error>) -> Result<(), Error> {
        self.client.core.finish(Error::Closed);
        if let Some(task) = self.reader.take() {
            task.await.map_err(|e| Error::Io(e.to_string()))?;
        }
        if let Some(task) = self.writer.take() {
            task.await.map_err(|e| Error::Io(e.to_string()))?;
        }
        match result {
            Err(Error::Closed) => Ok(()),
            other => other,
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.client.core.finish(Error::Closed);
    }
}

impl futures_core::Stream for Session {
    type Item = Result<Event, Error>;
    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.events.poll_recv(context)
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut write: W,
    mut commands: mpsc::Receiver<WriteCommand>,
    core: Arc<Core>,
    events: mpsc::Sender<Result<Event, Error>>,
) {
    loop {
        let command = tokio::select! { biased; _ = core.closed.cancelled() => break, v = commands.recv() => v };
        let Some(command) = command else {
            break;
        };
        let (result, end) = match command {
            WriteCommand::Frame {
                bytes,
                reply,
                cancellation,
            } => {
                if cancellation
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                {
                    let _ = reply.send(Err(Error::Cancelled));
                    continue;
                }
                let result = tokio::select! {
                    biased;
                    _ = core.closed.cancelled() => Err(Error::Closed),
                    result = async { write.write_all(&bytes).await?; write.flush().await } => result.map_err(Error::from),
                };
                let _ = reply.send(result.clone());
                (result, false)
            }
            WriteCommand::End(reply) => {
                let result = tokio::select! { biased; _ = core.closed.cancelled() => Err(Error::Closed), r = write.shutdown() => r.map_err(Error::from) };
                let _ = reply.send(result.clone());
                (result, true)
            }
        };
        if let Err(error) = result {
            *core.writer_error.lock() = Some(error.clone());
            emit(&events, &core, Err(error)).await;
            break;
        }
        if end {
            break;
        }
    }
}

async fn emit(
    events: &mpsc::Sender<Result<Event, Error>>,
    core: &Core,
    event: Result<Event, Error>,
) {
    tokio::select! { biased; _ = core.closed.cancelled() => {}, _ = events.send(event) => {} }
}
async fn diagnostic(
    events: &mpsc::Sender<Result<Event, Error>>,
    core: &Core,
    raw: Arc<Value>,
    error: Error,
    original_bytes: Option<Vec<u8>>,
) {
    emit(
        events,
        core,
        Ok(Event::Diagnostic {
            error,
            raw,
            original_bytes,
        }),
    )
    .await;
}
fn parse_frame(text: &str) -> serde_json::Result<Value> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.disable_recursion_limit();
    Value::deserialize(serde_stacker::Deserializer::new(&mut deserializer))
        .and_then(|frame| deserializer.end().map(|()| frame))
}

fn is_number_out_of_range(error: &serde_json::Error) -> bool {
    error.classify() == serde_json::error::Category::Syntax
        && error.to_string().starts_with("number out of range")
}

// JSON.parse reads a literal such as 1e400 as Infinity and keeps the message;
// serde_json rejects it. Infinity has no JSON spelling, so the nearest finite
// value stands in for it and the caller reports the substitution.
fn clamp_overflow_numbers(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    let mut in_string = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            let mut chars = text[index..].chars();
            let mut width = chars.next().map_or(1, char::len_utf8);
            if byte == b'\\' {
                width += chars.next().map_or(0, char::len_utf8);
            }
            in_string = byte != b'"';
            out.push_str(&text[index..index + width]);
            index += width;
        } else if byte == b'-' || byte.is_ascii_digit() {
            let end = bytes[index..]
                .iter()
                .position(|b| !matches!(b, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'))
                .map_or(bytes.len(), |offset| index + offset);
            let literal = &text[index..end];
            match literal.parse::<f64>() {
                Ok(value) if value.is_infinite() => {
                    out.push_str(if value.is_sign_negative() {
                        "-1.7976931348623157e308"
                    } else {
                        "1.7976931348623157e308"
                    });
                }
                _ => out.push_str(literal),
            }
            index = end;
        } else {
            in_string = byte == b'"';
            let width = text[index..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&text[index..index + width]);
            index += width;
        }
    }
    out
}

async fn read_loop<R: AsyncRead + Unpin>(
    read: R,
    options: SessionOptions,
    core: Arc<Core>,
    events: mpsc::Sender<Result<Event, Error>>,
) {
    let mut read = BufReader::new(read);
    let result = async {
        loop {
            let line = tokio::select! { biased; _ = core.closed.cancelled() => return Ok(()), line = read_line(&mut read, options.max_line_bytes) => line? };
            let Some(line) = line else { return Ok(()); };
            if line.iter().all(u8::is_ascii_whitespace) { continue; }
            let decoded = String::from_utf8_lossy(&line);
            let repaired = repair_surrogates(&decoded);
            let changed = decoded.as_bytes() != line || repaired != decoded;
            let mut parsed = parse_frame(&repaired);
            let mut clamped = false;
            if parsed.as_ref().is_err_and(is_number_out_of_range) {
                parsed = parse_frame(&clamp_overflow_numbers(&repaired));
                clamped = parsed.is_ok();
            }
            let frame = match parsed {
                Ok(frame) => Arc::new(frame),
                Err(error) if options.strict_json => return Err(Error::from(error)),
                Err(error) if decoded.trim_start().starts_with('{') => {
                    diagnostic(&events, &core, Arc::new(Value::Null), Error::from(error), Some(line)).await;
                    continue;
                }
                Err(_) => { emit(&events, &core, Ok(Event::NonJson { bytes: line.len() })).await; continue; }
            };
            if changed {
                diagnostic(&events, &core, frame.clone(), Error::Protocol("non-scalar JSON text repaired".into()), Some(line)).await;
            } else if clamped {
                diagnostic(&events, &core, frame.clone(), Error::Protocol("JSON number beyond f64 range clamped".into()), Some(line)).await;
            }
            route_frame(frame, &core, &events, &options).await;
        }
    }.await;
    match result {
        Ok(()) => core.finish(Error::Closed),
        Err(error) => {
            emit(&events, &core, Err(error.clone())).await;
            core.finish(error);
        }
    }
}

// JavaScript strings allow lone UTF-16 code units; Rust strings require scalar values.
fn repair_surrogates(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut output: Option<Vec<u8>> = None;
    let mut i = 0;
    let mut in_string = false;
    fn unit(bytes: &[u8], i: usize) -> Option<u16> {
        if bytes.get(i..i + 2)? != b"\\u" {
            return None;
        }
        let hex = std::str::from_utf8(bytes.get(i + 2..i + 6)?).ok()?;
        u16::from_str_radix(hex, 16).ok()
    }
    while i < bytes.len() {
        if bytes[i] == b'"' {
            in_string = !in_string;
        }
        if in_string && bytes[i] == b'\\' {
            if let Some(code) = unit(bytes, i) {
                if (0xd800..=0xdbff).contains(&code) {
                    if unit(bytes, i + 6).is_some_and(|low| (0xdc00..=0xdfff).contains(&low)) {
                        if let Some(output) = &mut output {
                            output.extend_from_slice(&bytes[i..i + 12]);
                        }
                        i += 12;
                        continue;
                    }
                    output
                        .get_or_insert_with(|| bytes[..i].to_vec())
                        .extend_from_slice(b"\\ufffd");
                    i += 6;
                    continue;
                }
                if (0xdc00..=0xdfff).contains(&code) {
                    output
                        .get_or_insert_with(|| bytes[..i].to_vec())
                        .extend_from_slice(b"\\ufffd");
                    i += 6;
                    continue;
                }
            }
            if let Some(output) = &mut output {
                output.push(bytes[i]);
            }
            i += 1;
            if i < bytes.len() {
                if let Some(output) = &mut output {
                    output.push(bytes[i]);
                }
                i += 1;
            }
            continue;
        }
        if let Some(output) = &mut output {
            output.push(bytes[i]);
        }
        i += 1;
    }
    match output {
        Some(output) => Cow::Owned(String::from_utf8_lossy(&output).into_owned()),
        None => Cow::Borrowed(text),
    }
}

async fn read_line<R: AsyncRead + Unpin>(
    read: &mut BufReader<R>,
    max: usize,
) -> Result<Option<Vec<u8>>, Error> {
    let mut line = Vec::new();
    loop {
        let buffer = read.fill_buf().await?;
        if buffer.is_empty() {
            return Ok(if line.is_empty() { None } else { Some(line) });
        }
        let newline = buffer.iter().position(|b| *b == b'\n');
        let n = newline.map_or(buffer.len(), |n| n + 1);
        if line.len().saturating_add(n) > max {
            return Err(Error::Protocol(
                "NDJSON line exceeds configured byte limit".into(),
            ));
        }
        line.extend_from_slice(&buffer[..n]);
        read.consume(n);
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}

fn string_field<'a>(frame: &'a Value, key: &str) -> Result<&'a str, Error> {
    frame
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Protocol(format!("missing string {key}")))
}

async fn expose_unknown(
    frame: Arc<Value>,
    core: &Core,
    events: &mpsc::Sender<Result<Event, Error>>,
    error: Error,
) {
    diagnostic(events, core, frame.clone(), error, None).await;
    emit(
        events,
        core,
        Ok(Event::Message(Box::new(Message::Unknown(frame)))),
    )
    .await;
}

async fn route_frame(
    frame: Arc<Value>,
    core: &Arc<Core>,
    events: &mpsc::Sender<Result<Event, Error>>,
    options: &SessionOptions,
) {
    let envelope_keys: &[&str] = match frame.get("type").and_then(Value::as_str) {
        Some("control_request") => &["type", "request_id", "request"],
        Some("control_response") => &["type", "response"],
        Some("control_cancel_request") => &["type", "request_id"],
        _ => &[],
    };
    if !envelope_keys.is_empty()
        && frame
            .as_object()
            .is_some_and(|map| map.keys().any(|key| !envelope_keys.contains(&key.as_str())))
    {
        diagnostic(
            events,
            core,
            frame.clone(),
            Error::Protocol("unrecognized control envelope fields".into()),
            None,
        )
        .await;
    }
    match frame.get("type").and_then(Value::as_str) {
        Some("control_response") => {
            let Some(response) = frame.get("response") else {
                expose_unknown(
                    frame,
                    core,
                    events,
                    Error::Protocol("missing control response".into()),
                )
                .await;
                return;
            };
            let Ok(id) = string_field(response, "request_id") else {
                expose_unknown(
                    frame,
                    core,
                    events,
                    Error::Protocol("missing response id".into()),
                )
                .await;
                return;
            };
            let id = id.to_owned();
            let result = match response.get("subtype").and_then(Value::as_str) {
                Some("success") => Ok(ControlReply {
                    raw: response.clone(),
                    payload: response.get("response").cloned(),
                }),
                Some("error") if response.get("error").is_some_and(Value::is_string) => {
                    Err(Error::Cli {
                        request_id: id.clone(),
                        message: response["error"].as_str().unwrap_or_default().into(),
                    })
                }
                _ => Err(Error::Protocol(
                    "unrecognized control response verdict".into(),
                )),
            };
            let malformed = matches!(result, Err(Error::Protocol(_)));
            let pending = core.pending.lock().remove(&id);
            let initialize_issue = pending.as_ref().is_some_and(|p| p.initialize)
                && result.as_ref().is_ok_and(|reply| {
                    reply.payload.as_ref().is_none_or(|payload| {
                        InitializeResult::deserialize(serde_stacker::Deserializer::new(payload))
                            .map_or(true, |value| !value.extra.is_empty())
                    })
                });
            let replay =
                options.replay_pending_callbacks && pending.as_ref().is_some_and(|p| p.initialize);
            if let Some(pending) = pending {
                let _ = pending.reply.send(result);
            } else {
                emit(
                    events,
                    core,
                    Ok(Event::UnmatchedResponse {
                        request_id: id,
                        raw: frame.as_ref().clone(),
                    }),
                )
                .await;
            }
            if initialize_issue {
                expose_unknown(
                    frame.clone(),
                    core,
                    events,
                    Error::Protocol("unrecognized initialize result data".into()),
                )
                .await;
            }
            if malformed {
                expose_unknown(
                    frame.clone(),
                    core,
                    events,
                    Error::Protocol("malformed control response".into()),
                )
                .await;
            }
            if replay {
                for (key, subtype) in [
                    ("pending_permission_requests", "can_use_tool"),
                    ("pending_user_dialog_requests", "request_user_dialog"),
                ] {
                    if let Some(list) = response.get(key).and_then(Value::as_array) {
                        for pending in list {
                            if pending
                                .get("request")
                                .and_then(|r| r.get("subtype"))
                                .and_then(Value::as_str)
                                != Some(subtype)
                            {
                                expose_unknown(
                                    Arc::new(pending.clone()),
                                    core,
                                    events,
                                    Error::Protocol("ineligible redelivery entry".into()),
                                )
                                .await;
                                continue;
                            }
                            let mut pending = pending.clone();
                            if subtype == "can_use_tool"
                                && let Some(request) =
                                    pending.get_mut("request").and_then(Value::as_object_mut)
                            {
                                request.remove("server_policy_ask_only");
                            }
                            route_callback(Arc::new(pending), core, events).await;
                        }
                    } else if response.get(key).is_some() {
                        diagnostic(
                            events,
                            core,
                            frame.clone(),
                            Error::Protocol("invalid redelivery list".into()),
                            None,
                        )
                        .await;
                    }
                }
            }
        }
        Some("control_request") => route_callback(frame, core, events).await,
        Some("control_cancel_request") => {
            if let Ok(id) = string_field(&frame, "request_id") {
                if let Some(callback) = core.callbacks.lock().remove(id) {
                    callback.cancellation.cancel();
                }
                emit(
                    events,
                    core,
                    Ok(Event::Cancelled {
                        request_id: id.into(),
                    }),
                )
                .await;
            } else {
                expose_unknown(
                    frame,
                    core,
                    events,
                    Error::Protocol("invalid cancellation".into()),
                )
                .await;
            }
        }
        _ => {
            let message = Message::from_raw(frame.clone())
                .unwrap_or_else(|_| Message::Unknown(frame.clone()));
            if message.has_unknown() {
                diagnostic(
                    events,
                    core,
                    frame,
                    Error::Protocol("unrecognized message data".into()),
                    None,
                )
                .await;
            }
            if let Some(version) = message.cli_version() {
                *core.cli_version.lock() = Some(version.into());
            }
            emit(events, core, Ok(Event::Message(Box::new(message)))).await;
        }
    }
}

async fn route_callback(
    frame: Arc<Value>,
    core: &Arc<Core>,
    events: &mpsc::Sender<Result<Event, Error>>,
) {
    let Ok(id) = string_field(&frame, "request_id") else {
        expose_unknown(
            frame,
            core,
            events,
            Error::Protocol("invalid callback id".into()),
        )
        .await;
        return;
    };
    let id = id.to_owned();
    let request = frame.get("request").cloned().unwrap_or(Value::Null);
    let subtype = request.get("subtype").and_then(Value::as_str);
    if matches!(
        subtype,
        Some(
            "remote_tool_call"
                | "remote_plumbing_call"
                | "remote_tools_probe"
                | "remote_tools_reannounce"
        )
    ) {
        expose_unknown(
            frame,
            core,
            events,
            Error::Unsupported("remote tool delivery".into()),
        )
        .await;
        return;
    }
    let context = Arc::new(CallbackContext {
        cancellation: core.closed.child_token(),
        response_slot: Mutex::new(None),
    });
    {
        let mut callbacks = core.callbacks.lock();
        if callbacks.contains_key(&id) {
            return;
        }
        callbacks.insert(id.clone(), context.clone());
    }
    // Reserve the reply before publishing a callback so Drop can always fail it closed.
    let slot = tokio::select! { biased;
        _ = core.closed.cancelled() => return,
        slot = core.writer.clone().reserve_owned() => { let Ok(slot) = slot else {
            core.remove_callback(&id, &context);
            expose_unknown(frame, core, events, Error::Closed).await;
            return;
        }; slot },
    };
    *context.response_slot.lock() = Some(slot);
    let callback = decode_callback(&id, request, core, &context);
    let extra = match &callback {
        Ok(PendingRequest::ToolPermission { request, .. }) => Some(&request.get().extra),
        Ok(PendingRequest::Hook { request, .. }) => Some(&request.get().extra),
        Ok(PendingRequest::Elicitation { request, .. }) => Some(&request.get().extra),
        Ok(PendingRequest::UserDialog { request, .. }) => Some(&request.get().extra),
        Ok(PendingRequest::Mcp { request, .. }) => Some(&request.get().extra),
        Err(_) => None,
    };
    if extra.is_some_and(|extra| extra.keys().any(|key| key != "subtype")) {
        diagnostic(
            events,
            core,
            frame.clone(),
            Error::Protocol("unrecognized callback fields".into()),
            None,
        )
        .await;
    }
    match callback {
        Ok(PendingRequest::Mcp { request, responder }) => {
            let server = core.mcp.lock().get(&request.get().server_name).cloned();
            if let Some(Some(server)) = server {
                tokio::spawn(async move {
                    let cancellation = responder.cancellation();
                    let message = request.get().message.clone();
                    let rpc_request = message.get("method").is_some()
                        && message.get("id").is_some_and(|id| !id.is_null());
                    if !rpc_request {
                        match server.notify(message, cancellation) {
                            Ok(()) => {
                                let _ = responder
                                    .respond(McpReply {
                                        mcp_response: json!({"jsonrpc":"2.0","result":{},"id":0}),
                                    })
                                    .await;
                            }
                            Err(error) => {
                                let _ = responder.reject(error).await;
                            }
                        }
                        return;
                    }
                    let reply = tokio::select! { biased; _ = cancellation.cancelled() => return, reply = server.handle(message.clone(), cancellation.clone()) => reply };
                    match reply {
                        Ok(reply)
                            if reply.get("id") == message.get("id")
                                && reply.get("jsonrpc") == Some(&json!("2.0")) =>
                        {
                            let _ = responder
                                .respond(McpReply {
                                    mcp_response: reply,
                                })
                                .await;
                        }
                        Ok(_) => {
                            let _ = responder
                                .reject(Error::Protocol("MCP response id/protocol mismatch".into()))
                                .await;
                        }
                        Err(error) => {
                            let _ = responder.reject(error).await;
                        }
                    }
                });
            } else {
                emit(
                    events,
                    core,
                    Ok(Event::Request(Box::new(PendingRequest::Mcp {
                        request,
                        responder,
                    }))),
                )
                .await;
            }
        }
        Ok(pending) => {
            if let PendingRequest::ToolPermission { request, .. } = &pending
                && request
                    .get()
                    .permission_suggestions
                    .as_ref()
                    .is_some_and(|list| {
                        list.iter()
                            .any(|v| serde_json::from_value::<PermissionUpdate>(v.clone()).is_err())
                    })
            {
                diagnostic(
                    events,
                    core,
                    frame,
                    Error::Protocol("unrecognized permission suggestion".into()),
                    None,
                )
                .await;
            }
            emit(events, core, Ok(Event::Request(Box::new(pending)))).await;
        }
        Err(error) => {
            if let Some(slot) = context.response_slot.lock().take()
                && let Ok((command, _)) = Core::frame_command(
                    error_response(&id, &error),
                    Some(context.cancellation.clone()),
                )
            {
                slot.send(command);
            }
            core.remove_callback(&id, &context);
            expose_unknown(frame, core, events, error).await;
        }
    }
}

fn responder<T>(
    id: &str,
    core: &Arc<Core>,
    context: &Arc<CallbackContext>,
    tool_use_id: Option<String>,
) -> Responder<T> {
    Responder {
        id: id.into(),
        core: core.clone(),
        context: context.clone(),
        tool_use_id,
        answered: false,
        marker: PhantomData,
    }
}
fn decode_callback(
    id: &str,
    request: Value,
    core: &Arc<Core>,
    context: &Arc<CallbackContext>,
) -> Result<PendingRequest, Error> {
    Ok(match string_field(&request, "subtype")? {
        "can_use_tool" => {
            let request = Decoded::<ToolPermission>::from_value(request)?;
            let tool_id = request.get().tool_use_id.clone();
            PendingRequest::ToolPermission {
                request,
                responder: responder(id, core, context, Some(tool_id)),
            }
        }
        "hook_callback" => PendingRequest::Hook {
            request: Decoded::from_value(request)?,
            responder: responder(id, core, context, None),
        },
        "elicitation" => PendingRequest::Elicitation {
            request: Decoded::from_value(request)?,
            responder: responder(id, core, context, None),
        },
        "request_user_dialog" => PendingRequest::UserDialog {
            request: Decoded::from_value(request)?,
            responder: responder(id, core, context, None),
        },
        "mcp_message" => {
            let request = Decoded::<McpMessage>::from_value(request)?;
            if !core.mcp.lock().contains_key(&request.get().server_name) {
                return Err(Error::Unsupported(format!(
                    "MCP server {}",
                    request.get().server_name
                )));
            }
            PendingRequest::Mcp {
                request,
                responder: responder(id, core, context, None),
            }
        }
        other => return Err(Error::Unsupported(other.into())),
    })
}
fn error_response(id: &str, error: &Error) -> Value {
    json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":error.to_string()}})
}

crate::redacted_debug!(McpReply, ControlReply);

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[tokio::test]
    async fn reader_backpressures_instead_of_accumulating_all_frames() {
        let session = Session::connect(
            tokio::io::empty(),
            tokio::io::sink(),
            SessionOptions::default(),
        );
        let (events, mut receiver) = mpsc::channel(QUEUE_CAPACITY);
        let bytes = b"{\"type\":\"keep_alive\"}\n".repeat(100);
        let mut reading = Box::pin(read_loop(
            bytes.as_slice(),
            SessionOptions::default(),
            session.client.core.clone(),
            events,
        ));
        std::future::poll_fn(|cx| {
            assert!(
                reading.as_mut().poll(cx).is_pending(),
                "reader accumulated the whole stream without backpressure"
            );
            assert!(receiver.len() <= QUEUE_CAPACITY);
            std::task::Poll::Ready(())
        })
        .await;
        let mut count = 0;
        loop {
            while let Ok(event) = receiver.try_recv() {
                assert!(matches!(event, Ok(Event::Message(_))));
                count += 1;
            }
            let done = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(reading.as_mut().poll(cx).is_ready())
            })
            .await;
            if done {
                break;
            }
        }
        while let Ok(event) = receiver.try_recv() {
            assert!(matches!(event, Ok(Event::Message(_))));
            count += 1;
        }
        assert_eq!(count, 100);
    }

    #[tokio::test]
    async fn dropped_permission_keeps_a_reserved_error_slot_under_backpressure() {
        let (host, peer) = tokio::io::duplex(16 * 1024);
        let (read, write) = tokio::io::split(host);
        let mut session = Session::connect(read, write, SessionOptions::default());
        let (events, mut received) = mpsc::channel(QUEUE_CAPACITY);
        route_callback(Arc::new(json!({"type":"control_request","request_id":"p","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"t","input":{}}})), &session.client.core, &events).await;
        let Ok(Ok(Event::Request(permission))) = received.try_recv() else {
            panic!()
        };
        for _ in 0..QUEUE_CAPACITY - 1 {
            let Ok((command, _)) = Core::frame_command(json!({"type":"filler"}), None) else {
                panic!()
            };
            assert!(session.client.core.writer.try_send(command).is_ok());
        }
        assert_eq!(session.client.core.writer.capacity(), 0);
        drop(permission);
        let mut read = BufReader::new(peer);
        for _ in 0..QUEUE_CAPACITY - 1 {
            let mut line = String::new();
            assert!(read.read_line(&mut line).await.is_ok());
            assert!(line.contains("filler"));
        }
        let mut line = String::new();
        assert!(read.read_line(&mut line).await.is_ok());
        let Ok(reply) = serde_json::from_str::<Value>(&line) else {
            panic!()
        };
        assert_eq!(reply["response"]["subtype"], "error");
        assert_eq!(reply["response"]["request_id"], "p");
        assert!(session.shutdown().await.is_ok());
    }
}
