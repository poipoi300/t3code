use std::{cell::RefCell, collections::HashMap, fmt::Display, rc::Rc};

use futures::{
    FutureExt, Sink, SinkExt, Stream, StreamExt,
    future::{AbortHandle, Abortable, LocalBoxFuture},
    stream::{FuturesUnordered, LocalBoxStream},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    CallEvent, Client, ClientError, Exit, Frame, Request, RequestId, Server, ServerAction,
    ServerMessage,
    codec::{self, CodecError},
    framing::Joiner,
};

/// The product's `ws` server uses its default 100 MiB incoming message limit.
pub const MAX_INCOMING_FRAME_BYTES: usize = 100 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("RPC transport: {0}")]
    Io(String),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Server(#[from] crate::ServerError),
}

pub struct AsyncClient<S, R, P = Value, A = Value, E = Value> {
    pub state: Client<P, A, E>,
    sink: S,
    incoming: R,
    joiner: Joiner,
}

impl<S, R, P, A, E> AsyncClient<S, R, P, A, E>
where
    S: Sink<Frame> + Unpin,
    S::Error: Display,
    R: Stream<Item = Result<Frame, S::Error>> + Unpin,
    P: Serialize + DeserializeOwned,
    A: DeserializeOwned,
    E: DeserializeOwned,
{
    pub fn new(sink: S, incoming: R, now_ms: u64) -> Self {
        Self {
            state: Client::new(now_ms, Default::default()),
            sink,
            incoming,
            joiner: Joiner::default(),
        }
    }

    pub async fn flush(&mut self) -> Result<(), TransportError> {
        while let Some(message) = self.state.pop_outgoing() {
            let frame = match codec::encode(&message) {
                Ok(frame) => frame,
                Err(error) => {
                    self.state
                        .disconnect(ClientError::Protocol(Value::String(error.to_string())));
                    return Err(error.into());
                }
            };
            if let Err(error) = self.sink.send(frame).await {
                self.state.disconnect(ClientError::Disconnected);
                return Err(TransportError::Io(error.to_string()));
            }
        }
        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), TransportError> {
        self.state.disconnect(ClientError::Disconnected);
        self.joiner.reset();
        self.sink
            .close()
            .await
            .map_err(|error| TransportError::Io(error.to_string()))
    }

    /// The caller supplies monotonic millisecond ticks; a pending stream disables timers.
    /// Each returned chunk is accepted by the caller before it requests another event.
    pub async fn next_event<T: Stream<Item = u64> + Unpin>(
        &mut self,
        ticks: &mut T,
        clock: impl Fn() -> u64,
    ) -> Result<Option<CallEvent<A, E, P>>, TransportError> {
        loop {
            self.flush().await?;
            if let Some(event) = self.state.pop_event() {
                if let CallEvent::Chunk { id, .. } = &event {
                    self.state.acknowledge(id.clone());
                }
                return Ok(Some(event));
            }
            if self.state.is_closed() {
                return Ok(None);
            }
            let input = self.incoming.next().fuse();
            let tick = ticks.next().fuse();
            futures::pin_mut!(input, tick);
            futures::select_biased! {
                frame = input => {
                    match frame {
                        Some(Ok(frame)) => {
                            let frame = self.joiner.feed(frame);
                            let messages = match codec::decode(&frame) {
                                Ok(messages) => messages,
                                Err(error) => {
                                    self.state.receive(ServerMessage::ClientProtocolError { error: Value::String(error.to_string()), request_ids: None }, clock());
                                    continue;
                                }
                            };
                            for message in messages { self.state.receive(message, clock()); }
                        }
                        Some(Err(error)) => {
                            self.state.disconnect(ClientError::Disconnected);
                            return Err(TransportError::Io(error.to_string()));
                        }
                        None => self.state.disconnect(ClientError::Disconnected),
                    }
                },
                now = tick => {
                    if now.is_some() { self.state.tick(clock()); }
                    else { return Err(TransportError::Io("tick stream ended; use a pending stream to disable timers".into())); }
                },
            }
        }
    }
}

pub type ResponseStream<A = Value, E = Value> = LocalBoxStream<'static, Result<Vec<A>, Exit<A, E>>>;
type WorkCell<A, E> = Rc<RefCell<Option<LocalBoxFuture<'static, Completion<A, E>>>>>;

pub enum HandlerOutput<A = Value, E = Value> {
    Unary(LocalBoxFuture<'static, Exit<A, E>>),
    Stream(ResponseStream<A, E>),
}

/// Method schemas, authorization, and payload validation belong at this boundary.
pub trait Handler<P = Value, A = Value, E = Value> {
    fn handle(&mut self, request: Request<P>) -> HandlerOutput<A, E>;
}

enum Completion<A, E> {
    Chunk(Vec<A>, ResponseStream<A, E>),
    Exit(Exit<A, E>),
}

struct Job<A, E> {
    id: RequestId,
    generation: u64,
    completion: Result<Completion<A, E>, futures::future::Aborted>,
}

struct Running<A, E> {
    generation: u64,
    abort: AbortHandle,
    // Cancellation drops the handler immediately, before another executor poll.
    work: WorkCell<A, E>,
}

pub struct AsyncServer<S, R, H, P = Value, A = Value, E = Value> {
    pub state: Server<P, A, E>,
    sink: S,
    incoming: R,
    handler: H,
    jobs: FuturesUnordered<LocalBoxFuture<'static, Job<A, E>>>,
    aborts: HashMap<RequestId, Running<A, E>>,
    parked: HashMap<RequestId, ResponseStream<A, E>>,
    generation: u64,
    split_frames: bool,
}

impl<S, R, H, P, A, E> AsyncServer<S, R, H, P, A, E>
where
    S: Sink<Frame> + Unpin,
    S::Error: Display,
    R: Stream<Item = Result<Frame, S::Error>> + Unpin,
    H: Handler<P, A, E>,
    P: Serialize + DeserializeOwned,
    A: Serialize + 'static,
    E: Serialize + 'static,
{
    pub fn new(sink: S, incoming: R, handler: H) -> Self {
        Self {
            state: Server::new(),
            sink,
            incoming,
            handler,
            jobs: FuturesUnordered::new(),
            aborts: HashMap::new(),
            parked: HashMap::new(),
            generation: 0,
            split_frames: false,
        }
    }

    /// Enable only when the peer negotiated `frameParts=1` at connection time.
    pub fn set_frame_parts(&mut self, enabled: bool) {
        self.split_frames = enabled;
    }

    fn schedule(&mut self, id: RequestId, work: LocalBoxFuture<'static, Completion<A, E>>) {
        let (abort, registration) = AbortHandle::new_pair();
        let generation = self.generation;
        self.generation = self.generation.wrapping_add(1);
        let work = Rc::new(RefCell::new(Some(work)));
        self.aborts.insert(
            id.clone(),
            Running {
                generation,
                abort,
                work: work.clone(),
            },
        );
        let work = futures::future::poll_fn(move |cx| match work.borrow_mut().as_mut() {
            Some(work) => work.as_mut().poll(cx),
            None => std::task::Poll::Pending,
        });
        self.jobs.push(
            async move {
                Job {
                    id,
                    generation,
                    completion: Abortable::new(
                        async move {
                            match std::panic::AssertUnwindSafe(work).catch_unwind().await {
                                Ok(completion) => completion,
                                Err(panic) => Completion::Exit(Exit::die(panic_message(panic))),
                            }
                        },
                        registration,
                    )
                    .await,
                }
            }
            .boxed_local(),
        );
    }

    fn schedule_stream(&mut self, id: RequestId, mut stream: ResponseStream<A, E>) {
        self.schedule(
            id,
            async move {
                loop {
                    match stream.next().await {
                        Some(Ok(values)) if values.is_empty() => continue,
                        Some(Ok(values)) => return Completion::Chunk(values, stream),
                        Some(Err(exit)) => return Completion::Exit(exit),
                        None => return Completion::Exit(Exit::void()),
                    }
                }
            }
            .boxed_local(),
        );
    }

    fn actions(&mut self) -> Result<(), TransportError> {
        while let Some(action) = self.state.pop_action() {
            match action {
                ServerAction::Dispatch(request) => {
                    let id = request.id.clone();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        self.handler.handle(request)
                    }));
                    self.state.started(&id)?;
                    match result {
                        Err(panic) => self.state.fail_defect(&id, panic_message(panic))?,
                        Ok(HandlerOutput::Unary(future)) => self.schedule(
                            id,
                            async move { Completion::Exit(future.await) }.boxed_local(),
                        ),
                        Ok(HandlerOutput::Stream(stream)) => self.schedule_stream(id, stream),
                    }
                }
                ServerAction::Cancel(id) => {
                    self.cancel_work(&id);
                }
                ServerAction::Ended => {}
            }
        }
        Ok(())
    }

    fn cancel_work(&mut self, id: &RequestId) {
        if let Some(running) = self.aborts.remove(id) {
            running.work.borrow_mut().take();
            running.abort.abort();
        }
        self.parked.remove(id);
    }

    fn disconnect(&mut self) {
        self.state.disconnect();
        self.jobs.clear();
        self.aborts.clear();
        self.parked.clear();
    }

    async fn flush(&mut self) -> Result<(), TransportError> {
        while let Some(message) = self.state.pop_outgoing() {
            let frame = match codec::encode(&message) {
                Ok(frame) => frame,
                Err(error) => {
                    let id = match message {
                        ServerMessage::Chunk { request_id, .. } => {
                            self.cancel_work(&request_id);
                            self.state.fail_defect(&request_id, error.to_string())?;
                            continue;
                        }
                        ServerMessage::Exit { request_id, .. } => request_id,
                        _ => return Err(error.into()),
                    };
                    codec::encode(&ServerMessage::<Value>::Exit {
                        request_id: id,
                        exit: Exit::die(error.to_string()),
                    })?
                }
            };
            let parts = if self.split_frames {
                crate::framing::split(frame)
            } else {
                vec![frame]
            };
            for part in parts {
                if let Err(error) = self.sink.send(part).await {
                    return Err(TransportError::Io(error.to_string()));
                }
            }
        }
        Ok(())
    }

    /// Drives one incoming frame or handler completion. Returns false after disconnect.
    pub async fn step(&mut self) -> Result<bool, TransportError> {
        let result = self.step_inner().await;
        if result.is_err() {
            self.disconnect();
        }
        result
    }

    async fn step_inner(&mut self) -> Result<bool, TransportError> {
        if self.state.is_closed() {
            return Ok(false);
        }
        self.actions()?;
        self.flush().await?;
        let incoming = self.incoming.next().fuse();
        let job = if self.jobs.is_empty() {
            futures::future::Either::Left(futures::future::pending())
        } else {
            futures::future::Either::Right(self.jobs.next())
        }
        .fuse();
        futures::pin_mut!(incoming, job);
        futures::select! {
            frame = incoming => {
                match frame {
                    Some(Ok(frame)) => {
                        let bytes = match &frame { Frame::Text(text) => text.len(), Frame::Binary(bytes) => bytes.len() };
                        if bytes > MAX_INCOMING_FRAME_BYTES { return Err(TransportError::Io(format!("RPC frame exceeds the {MAX_INCOMING_FRAME_BYTES} byte incoming message limit"))); }
                        match codec::decode(&frame) {
                        Ok(messages) => for message in messages {
                            let ack = match &message { crate::ClientMessage::Ack { request_id } => Some(request_id.clone()), _ => None };
                            self.state.receive(message);
                            if let Some(id) = ack
                                && self.state.can_poll(&id)
                                && let Some(stream) = self.parked.remove(&id) {
                                self.schedule_stream(id, stream);
                            }
                        },
                        Err(error) => {
                            let frame = codec::encode(&ServerMessage::<Value>::Defect { defect: Value::String(error.to_string()) })?;
                            self.sink.send(frame).await.map_err(|e| TransportError::Io(e.to_string()))?;
                        }
                        }
                    },
                    Some(Err(error)) => return Err(TransportError::Io(error.to_string())),
                    None => { self.disconnect(); return Ok(false); }
                }
            },
            completion = job => {
                if let Some(Job { id, generation, completion: Ok(completion) }) = completion
                    && self.aborts.get(&id).is_some_and(|running| running.generation == generation) {
                    self.aborts.remove(&id);
                    match completion {
                        Completion::Chunk(values, stream) => {
                            match self.state.send_chunk(&id, values) {
                                Ok(()) => {
                                    if self.state.can_poll(&id) { self.schedule_stream(id, stream); }
                                    else { self.parked.insert(id, stream); }
                                },
                                Err(error) => self.state.fail_defect(&id, error.to_string())?,
                            }
                        }
                        Completion::Exit(exit) => self.state.finish(&id, exit)?,
                    }
                }
            },
        }
        self.actions()?;
        self.flush().await?;
        Ok(true)
    }

    pub async fn run(&mut self) -> Result<(), TransportError> {
        while self.step().await? {}
        Ok(())
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).into()
    } else {
        "RPC handler panicked".into()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{CallKind, ClientMessage};
    struct Streams;
    impl Handler for Streams {
        fn handle(&mut self, _: Request) -> HandlerOutput {
            HandlerOutput::Stream(futures::stream::iter([Ok(vec![Value::Null])]).boxed_local())
        }
    }

    #[test]
    fn unrelated_frames_do_not_scan_parked_streams() {
        futures::executor::block_on(async {
            let (mut input, reader) = futures::channel::mpsc::unbounded::<
                Result<Frame, futures::channel::mpsc::SendError>,
            >();
            let (writer, _output) = futures::channel::mpsc::unbounded();
            let mut server = AsyncServer::new(writer, reader, Streams);
            server.state.register("stream", CallKind::Stream);
            for id in 0..64_u64 {
                server.state.receive(ClientMessage::Request(Request::new(
                    id.into(),
                    "stream",
                    Value::Null,
                )));
                server.step().await.unwrap();
            }
            server.state.poll_checks.set(0);
            input
                .send(Ok(codec::encode(&ClientMessage::<Value>::Ping).unwrap()))
                .await
                .unwrap();
            server.step().await.unwrap();
            assert_eq!(server.state.poll_checks.get(), 0);
        });
    }
}
