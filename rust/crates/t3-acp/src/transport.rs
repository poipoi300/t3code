use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicU8, Ordering},
    },
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};

use crate::{
    ClientHandler, ClientOptions, Error, Event, Generation, RequestContext, RequestId, RpcError,
    SessionState, compat,
};

type Reply = oneshot::Sender<Result<Value, Error>>;

pub(crate) fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) struct Shared {
    pub pending: Mutex<HashMap<RequestId, Reply>>,
    pub prompts: Mutex<HashMap<String, Reply>>,
    pub generation: AtomicU8,
    pub sessions: Mutex<HashMap<String, SessionState>>,
    pub loads: Mutex<HashMap<String, watch::Sender<Option<tokio::time::Instant>>>>,
    pub raw: Arc<crate::raw::RawQueue>,
    pub terminal: watch::Sender<Option<Error>>,
    next_id: AtomicI64,
    outgoing: mpsc::Sender<Write>,
    events: mpsc::Sender<Delivery>,
}

struct Write {
    frame: Value,
    acknowledgement: oneshot::Sender<Result<(), Error>>,
}

impl Shared {
    pub fn generation(&self) -> Generation {
        if self.generation.load(Ordering::Acquire) != 1 {
            Generation::V2
        } else {
            Generation::V1
        }
    }

    pub fn check(&self) -> Result<(), Error> {
        match self.terminal.borrow().as_ref() {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    pub fn terminate(&self, error: Error) {
        let first = self.terminal.send_if_modified(|terminal| {
            if terminal.is_some() {
                return false;
            }
            *terminal = Some(error.clone());
            true
        });
        if !first {
            return;
        }
        for (_, reply) in lock(&self.pending).drain() {
            let _ = reply.send(Err(error.clone()));
        }
        lock(&self.sessions).clear();
        lock(&self.loads).clear();
        for (_, reply) in lock(&self.prompts).drain() {
            let _ = reply.send(Err(error.clone()));
        }
    }

    pub async fn event(&self, event: Event) -> Result<(), Error> {
        self.deliver(Delivery {
            event,
            acknowledged: None,
        })
        .await
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), Error> {
        self.check()?;
        let mut terminal = self.terminal.subscribe();
        tokio::select! {
            biased;
            _ = async { let _ = terminal.wait_for(|e| e.is_some()).await; } => self.check(),
            result = self.events.send(delivery) => result.map_err(|_| Error::Closed),
        }
    }

    async fn write(&self, frame: Value) -> Result<(), Error> {
        self.check()?;
        let mut terminal = self.terminal.subscribe();
        let (tx, rx) = oneshot::channel();
        tokio::select! {
            biased;
            _ = async { let _ = terminal.wait_for(|e| e.is_some()).await; } => self.check(),
            result = async {
                self.outgoing.send(Write { frame, acknowledgement: tx }).await.map_err(|_| Error::Closed)?;
                rx.await.map_err(|_| Error::Closed)?
            } => result,
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), Error> {
        self.write(json!({"jsonrpc":"2.0", "method":method, "params":params}))
            .await
    }

    pub async fn request(self: &Arc<Self>, method: &str, params: Value) -> Result<Value, Error> {
        self.check()?;
        // Kotlin ACP decodes request IDs as int32; never generate timestamps or u64 IDs.
        let n = self
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < i32::MAX as i64).then_some(n + 1)
            })
            .map_err(|_| Error::Schema("request ID space exhausted".into()))?;
        let id = RequestId::Number(n);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id.clone(), tx);
        let _guard = PendingGuard {
            shared: self.clone(),
            id: id.clone(),
        };
        self.check()?;
        self.write(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .await?;
        rx.await.map_err(|_| Error::Closed)?
    }
}

struct PendingGuard {
    shared: Arc<Shared>,
    id: RequestId,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        lock(&self.shared.pending).remove(&self.id);
    }
}

pub(crate) struct Runtime {
    pub shared: Arc<Shared>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Runtime {
    pub async fn shutdown(&self) {
        self.shared.terminate(Error::Closed);
        let tasks = std::mem::take(&mut *lock(&self.tasks));
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shared.terminate(Error::Closed);
        for task in lock(&self.tasks).drain(..) {
            task.abort();
        }
    }
}

pub(crate) fn connect<R, W>(
    reader: R,
    writer: W,
    handler: Arc<dyn ClientHandler>,
    options: &ClientOptions,
) -> (Arc<Runtime>, mpsc::Receiver<Delivery>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (outgoing, writes) = mpsc::channel(256);
    let (events, receiver) = mpsc::channel(options.event_capacity.max(1));
    let (terminal, _) = watch::channel(None);
    let shared = Arc::new(Shared {
        pending: Mutex::new(HashMap::new()),
        prompts: Mutex::new(HashMap::new()),
        generation: AtomicU8::new(0),
        sessions: Mutex::new(HashMap::new()),
        loads: Mutex::new(HashMap::new()),
        raw: Arc::new(crate::raw::RawQueue::default()),
        terminal,
        next_id: AtomicI64::new(1 << 30),
        outgoing,
        events,
    });
    let tasks = vec![
        tokio::spawn(write_loop(writer, writes, shared.clone())),
        tokio::spawn(read_loop(
            reader,
            shared.clone(),
            handler,
            options.max_line_bytes,
            options.max_concurrent_handlers.max(1),
        )),
    ];
    (
        Arc::new(Runtime {
            shared,
            tasks: Mutex::new(tasks),
        }),
        receiver,
    )
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut writes: mpsc::Receiver<Write>,
    shared: Arc<Shared>,
) {
    let mut terminal = shared.terminal.subscribe();
    loop {
        let write = tokio::select! {
            biased;
            _ = async { let _ = terminal.wait_for(|e| e.is_some()).await; } => break,
            write = writes.recv() => match write { Some(write) => write, None => break },
        };
        let result = tokio::select! {
            biased;
            _ = async { let _ = terminal.wait_for(|e| e.is_some()).await; } => break,
            result = async {
            let mut bytes = serde_json::to_vec(&write.frame).map_err(|_| Error::InvalidFrame)?;
            bytes.push(b'\n');
            writer.write_all(&bytes).await.map_err(|e| Error::Io {
                operation: "write",
                detail: e.to_string(),
            })?;
            writer.flush().await.map_err(|e| Error::Io {
                operation: "flush",
                detail: e.to_string(),
            })
            } => result,
        };
        if let Err(error) = &result {
            shared.terminate(error.clone());
        }
        let failed = result.is_err();
        let _ = write.acknowledgement.send(result);
        if failed {
            break;
        }
    }
}

pub(crate) struct Delivery {
    pub event: Event,
    pub acknowledged: Option<oneshot::Sender<()>>,
}

#[derive(Default)]
struct Dispatch {
    barrier: Option<oneshot::Receiver<()>>,
    active: HashSet<RequestId>,
    jobs: JoinSet<(RequestId, Value)>,
    batches: HashMap<RequestId, usize>,
    responses: HashMap<usize, Vec<(RequestId, Option<Value>)>>,
    next_batch: usize,
}

impl Dispatch {
    async fn barrier(&mut self) -> Result<(), Error> {
        if let Some(acknowledged) = self.barrier.take() {
            acknowledged.await.map_err(|_| Error::Closed)?;
        }
        Ok(())
    }

    async fn event(&mut self, shared: &Shared, event: Event) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        shared
            .deliver(Delivery {
                event,
                acknowledged: Some(tx),
            })
            .await?;
        self.barrier = Some(rx);
        Ok(())
    }

    async fn finish(&mut self, shared: &Shared, id: RequestId, frame: Value) -> Result<(), Error> {
        if let Some(batch) = self.batches.remove(&id) {
            let responses = self.responses.get_mut(&batch).ok_or(Error::InvalidFrame)?;
            if let Some((_, response)) = responses.iter_mut().find(|(key, _)| *key == id) {
                *response = Some(frame);
            }
            if responses.iter().any(|(_, response)| response.is_none()) {
                return Ok(());
            }
            let responses = self.responses.remove(&batch).ok_or(Error::InvalidFrame)?;
            shared
                .write(Value::Array(
                    responses.iter().filter_map(|(_, v)| v.clone()).collect(),
                ))
                .await?;
            for (id, _) in responses {
                self.active.remove(&id);
                shared.event(Event::ResponseSent { id }).await?;
            }
        } else {
            shared.write(frame).await?;
            self.active.remove(&id);
            shared.event(Event::ResponseSent { id }).await?;
        }
        Ok(())
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    reader: R,
    shared: Arc<Shared>,
    handler: Arc<dyn ClientHandler>,
    max_line_bytes: usize,
    max_handlers: usize,
) {
    let mut reader = BufReader::new(reader);
    let mut terminal = shared.terminal.subscribe();
    let mut dispatch = Dispatch::default();
    let mut bytes = Vec::new();
    let result = async {
        loop {
            // fill_buf is cancellation safe; partial lines remain in our own buffer.
            let chunk = tokio::select! {
                biased;
                _ = async { let _ = terminal.wait_for(|e| e.is_some()).await; } => return Ok(()),
                result = dispatch.jobs.join_next(), if !dispatch.jobs.is_empty() => {
                    if let Some(Ok((id, frame))) = result {
                        dispatch.finish(&shared, id, frame).await?;
                    }
                    continue;
                },
                chunk = reader.fill_buf() => chunk.map_err(|error| Error::Io { operation: "read", detail: error.to_string() })?,
            };
            let eof = chunk.is_empty();
            let length = chunk.iter().position(|b| *b == b'\n').map_or(chunk.len(), |i| i + 1);
            if bytes.len().saturating_add(length) > max_line_bytes {
                return Err(Error::LineTooLong { limit: max_line_bytes });
            }
            bytes.extend_from_slice(&chunk[..length]);
            reader.consume(length);
            let complete = bytes.last() == Some(&b'\n');
            if !complete && !eof {
                continue;
            }
            if bytes.iter().all(u8::is_ascii_whitespace) {
                bytes.clear();
                if eof { return Err(Error::Closed); }
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let frame = match serde_json::from_str::<Value>(&text) {
                Ok(frame) => frame,
                Err(_) if eof => return Err(Error::Closed),
                Err(_) => return Err(Error::InvalidFrame),
            };
            let line_length = text.encode_utf16().count() - usize::from(complete);
            tokio::select! {
                biased;
                _ = async { let _ = terminal.wait_for(|e| e.is_some()).await; } => return Ok(()),
                result = dispatch_frame(frame, line_length, &shared, &handler, &mut dispatch, max_handlers) => result?,
            }
            bytes.clear();
            if eof { return Err(Error::Closed); }
        }
    }.await;
    if let Err(error) = result {
        shared.terminate(error);
    }
}

async fn dispatch_frame(
    frame: Value,
    line_length: usize,
    shared: &Arc<Shared>,
    handler: &Arc<dyn ClientHandler>,
    dispatch: &mut Dispatch,
    max_handlers: usize,
) -> Result<(), Error> {
    let batch = frame.is_array();
    let frames = match frame {
        Value::Array(frames) => frames,
        other => vec![other],
    };
    let batch_id = dispatch.next_batch;
    dispatch.next_batch += 1;
    if batch {
        let ids = frames
            .iter()
            .filter_map(|frame| {
                frame.get("method")?.as_str()?;
                let id = wire_id(frame.get("id")?)?;
                if matches!(&id, RequestId::String(s) if s.is_empty())
                    || dispatch.active.contains(&id)
                {
                    return None;
                }
                Some((id, None))
            })
            .fold(Vec::new(), |mut ids, item| {
                if !ids.iter().any(|(id, _)| *id == item.0) {
                    ids.push(item);
                }
                ids
            });
        if !ids.is_empty() {
            for (id, _) in &ids {
                dispatch.batches.insert(id.clone(), batch_id);
            }
            dispatch.responses.insert(batch_id, ids);
        }
    }
    for frame in frames {
        let Some(object) = frame.as_object() else {
            shared.event(Event::UnroutableFrame).await?;
            continue;
        };
        if let Some(method) = object.get("method").and_then(Value::as_str) {
            let params = object
                .get("params")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or(Value::Null);
            let id = object.get("id").and_then(wire_id);
            if id.is_none() || matches!(&id, Some(RequestId::String(s)) if s.is_empty()) {
                if object
                    .get("id")
                    .is_some_and(|v| !v.is_null() && !v.is_string() && !v.is_number())
                {
                    shared.event(Event::UnroutableFrame).await?;
                } else {
                    notification(method, params, line_length, shared, dispatch).await?;
                }
                continue;
            }
            dispatch.barrier().await?;
            let id = id.ok_or(Error::InvalidFrame)?;
            if !dispatch.active.insert(id.clone()) {
                // Effect RpcServer.handleRequest interrupts duplicate in-flight requests without replying.
                shared.event(Event::DuplicateRequest { id }).await?;
                continue;
            }
            if dispatch.jobs.len() >= max_handlers || dispatch.active.len() > max_handlers {
                dispatch
                    .finish(
                        shared,
                        id.clone(),
                        response(
                            &id,
                            Err(RpcError {
                                code: -32000,
                                message: "Too many concurrent requests".into(),
                                data: None,
                            }),
                        ),
                    )
                    .await?;
                continue;
            }
            let context = RequestContext {
                id: id.clone(),
                method: method.to_owned(),
            };
            let request = compat::client_request(method, params, shared.generation(), &id);
            let inline = matches!(&request, Ok(crate::ClientRequest::Extension { .. }));
            let inline_id = id.clone();
            let handler = handler.clone();
            let method = method.to_owned();
            dispatch.jobs.spawn(async move {
                // The nested task contains panics, including a panic before the handler's first await.
                let mut job = JoinSet::new();
                job.spawn(async move {
                    match request {
                        Ok(request) => handler.request(request, context).await.map(|mut result| {
                            if matches!(
                                method.as_str(),
                                "session/elicitation" | "_session/elicitation"
                            ) {
                                let meta = result.as_object_mut().and_then(|r| r.remove("_meta"));
                                let mut wrapped = json!({"action":result});
                                if let Some(meta) = meta {
                                    wrapped["_meta"] = meta;
                                }
                                wrapped
                            } else {
                                result
                            }
                        }),
                        Err(error) => Err(error),
                    }
                });
                let result = match job.join_next().await {
                    Some(Ok(result)) => result,
                    _ => Err(RpcError::internal()),
                };
                (id.clone(), response(&id, result))
            });
            if inline {
                while let Some(result) = dispatch.jobs.join_next().await {
                    if let Ok((id, frame)) = result {
                        let finished = id == inline_id;
                        dispatch.finish(shared, id, frame).await?;
                        if finished {
                            break;
                        }
                    }
                }
            }
        } else {
            dispatch.barrier().await?;
            let Some(id) = object.get("id").and_then(wire_id) else {
                shared.event(Event::UnroutableFrame).await?;
                continue;
            };
            let reply = lock(&shared.pending).remove(&id);
            let Some(reply) = reply else {
                shared.event(Event::UnroutableFrame).await?;
                continue;
            };
            let result = if let Some(error) = object.get("error").filter(|v| !v.is_null()) {
                serde_json::from_value(error.clone())
                    .map(Error::Rpc)
                    .map_or(Err(Error::InvalidFrame), Err)
            } else {
                // Effect's undefined result decodes as an absent value; raw JSON uses null.
                Ok(object.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = reply.send(result);
        }
    }
    Ok(())
}

fn wire_id(value: &Value) -> Option<RequestId> {
    match value {
        Value::Null => Some(RequestId::String(String::new())),
        Value::String(s) => Some(RequestId::String(s.clone())),
        Value::Number(n) => Some(
            n.as_i64()
                .map(RequestId::Number)
                .unwrap_or_else(|| RequestId::Numeric(n.clone())),
        ),
        _ => None,
    }
}

fn response(id: &RequestId, result: Result<Value, RpcError>) -> Value {
    match result {
        Ok(result) => json!({"jsonrpc":"2.0", "id":id, "result":result}),
        Err(error) => json!({"jsonrpc":"2.0", "id":id, "error":error}),
    }
}

async fn notification(
    method: &str,
    params: Value,
    line_length: usize,
    shared: &Shared,
    dispatch: &mut Dispatch,
) -> Result<(), Error> {
    if method != "session/update" {
        let event = Event::Notification {
            method: method.to_owned(),
            params,
        };
        dispatch.event(shared, event.clone()).await?;
        shared.raw.push(event, line_length);
        return Ok(());
    }
    let Some(update) = compat::session_update(params, shared.generation()) else {
        return dispatch
            .event(
                shared,
                Event::InvalidNotification {
                    method: method.to_owned(),
                },
            )
            .await;
    };
    if update.counts_as_replay_activity()
        && let Some(load) = lock(&shared.loads).get(&update.session_id)
    {
        load.send_replace(Some(tokio::time::Instant::now()));
    }
    if update.kind() == "config_option_update" {
        let mut sessions = lock(&shared.sessions);
        if let Some(state) = sessions.get_mut(&update.session_id) {
            state.revision += 1;
            state.config_options = update.update.get("configOptions").cloned();
        }
    }
    if update.kind() == "current_mode_update" {
        let mut sessions = lock(&shared.sessions);
        if let Some(state) = sessions.get_mut(&update.session_id) {
            state.modes.get_or_insert_with(|| json!({}))["currentModeId"] =
                update.update["currentModeId"].clone();
        }
    }
    let completion = if update.kind() == "state_update" && update.update["state"] == "idle" {
        let mut result = json!({"stopReason":update.update.get("stopReason").filter(|v| !v.is_null()).cloned().unwrap_or(json!("end_turn"))});
        for key in ["usage", "error", "_meta"] {
            if let Some(value) = update.update.get(key) {
                result[key] = value.clone();
            }
        }
        lock(&shared.prompts)
            .remove(&update.session_id)
            .map(|tx| (tx, result))
    } else {
        None
    };
    let event = Event::SessionUpdate(update);
    dispatch.event(shared, event.clone()).await?;
    shared.raw.push(event, line_length);
    if let Some((tx, result)) = completion {
        dispatch.barrier().await?;
        let _ = tx.send(Ok(result));
    }
    Ok(())
}

#[cfg(test)]
mod regression {
    use super::*;
    use crate::RejectRequests;
    use std::{future::Future, task::Poll};

    fn shared(capacity: usize) -> (Arc<Shared>, mpsc::Receiver<Delivery>, mpsc::Receiver<Write>) {
        let (outgoing, writes) = mpsc::channel(256);
        let (events, receiver) = mpsc::channel(capacity);
        let (terminal, _) = watch::channel(None);
        (
            Arc::new(Shared {
                pending: Mutex::new(HashMap::new()),
                prompts: Mutex::new(HashMap::new()),
                generation: AtomicU8::new(1),
                sessions: Mutex::new(HashMap::new()),
                loads: Mutex::new(HashMap::new()),
                raw: Arc::new(crate::raw::RawQueue::default()),
                terminal,
                next_id: AtomicI64::new(1),
                outgoing,
                events,
            }),
            receiver,
            writes,
        )
    }

    async fn pending<F: Future>(future: std::pin::Pin<&mut F>) {
        let mut future = future;
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await
        );
    }

    #[tokio::test]
    async fn preceding_notification_blocks_response_completion() {
        let (shared, mut events, _writes) = shared(4);
        let (tx, mut rx) = oneshot::channel();
        lock(&shared.pending).insert(RequestId::Number(1), tx);
        let handler: Arc<dyn ClientHandler> = Arc::new(RejectRequests);
        let mut dispatch = Dispatch::default();
        let future = dispatch_frame(
            json!([
                {"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"tail"}}}},
                {"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}
            ]),
            100,
            &shared,
            &handler,
            &mut dispatch,
            4,
        );
        tokio::pin!(future);
        pending(future.as_mut()).await;
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        events
            .recv()
            .await
            .unwrap()
            .acknowledged
            .unwrap()
            .send(())
            .unwrap();
        future.await.unwrap();
        assert_eq!(rx.await.unwrap().unwrap()["stopReason"], "end_turn");
    }

    #[tokio::test]
    async fn full_event_buffer_does_not_terminate_session() {
        let (shared, mut events, _writes) = shared(1);
        shared
            .event(Event::Notification {
                method: "first".into(),
                params: Value::Null,
            })
            .await
            .unwrap();
        let future = shared.event(Event::Notification {
            method: "second".into(),
            params: Value::Null,
        });
        tokio::pin!(future);
        pending(future.as_mut()).await;
        assert_eq!(shared.check(), Ok(()));
        events.recv().await.unwrap();
        future.await.unwrap();
        assert!(
            matches!(events.recv().await.unwrap().event, Event::Notification { method, .. } if method == "second")
        );
    }

    #[tokio::test]
    async fn tolerant_frames_do_not_kill_connection() {
        let handler: Arc<dyn ClientHandler> = Arc::new(RejectRequests);
        for frame in [
            json!({"method":"_notice"}),
            json!({"id":null,"method":"_notice"}),
            json!({"id":1.5,"method":"fs/read_text_file"}),
            json!({"id":1}),
            json!({"id":1,"error":"bad"}),
            json!([]),
            json!(42),
            json!({"id":18446744073709551615_u64,"result":{}}),
        ] {
            let (shared, _events, _writes) = shared(16);
            let mut dispatch = Dispatch::default();
            assert_eq!(
                dispatch_frame(frame.clone(), 100, &shared, &handler, &mut dispatch, 4).await,
                Ok(()),
                "{frame}"
            );
        }
    }

    #[tokio::test]
    async fn extension_handler_runs_inline_before_routing_the_next_message() {
        let (shared, _events, _writes) = shared(8);
        let handler: Arc<dyn ClientHandler> = Arc::new(RejectRequests);
        let mut dispatch = Dispatch::default();
        let future = dispatch_frame(
            json!({"id":7,"method":"_extension","params":{}}),
            100,
            &shared,
            &handler,
            &mut dispatch,
            4,
        );
        tokio::pin!(future);
        pending(future.as_mut()).await;
    }

    #[tokio::test]
    async fn malformed_reply_fails_only_matching_request() {
        let (shared, _events, _writes) = shared(8);
        let (one, mut rx) = oneshot::channel();
        let (two, _other) = oneshot::channel();
        lock(&shared.pending).insert(RequestId::Number(1), one);
        lock(&shared.pending).insert(RequestId::Number(2), two);
        let handler: Arc<dyn ClientHandler> = Arc::new(RejectRequests);
        let mut dispatch = Dispatch::default();
        assert_eq!(
            dispatch_frame(
                json!({"jsonrpc":"2.0","id":1,"error":"bad"}),
                100,
                &shared,
                &handler,
                &mut dispatch,
                4
            )
            .await,
            Ok(())
        );
        assert_eq!(rx.try_recv().unwrap(), Err(Error::InvalidFrame));
        assert_eq!(lock(&shared.pending).len(), 1);
        assert_eq!(shared.check(), Ok(()));
    }
}
