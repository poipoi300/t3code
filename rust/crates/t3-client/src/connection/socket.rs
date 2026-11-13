//! A retained local RPC driver over the host's cancel-safe WebSocket receive.
use super::{
    ConnectionFailure, ConnectionVia, FailureKind, SessionFactory, map_remote_environment_error,
};
use crate::{
    ClientError,
    authorization::{AuthorizedEnvironment, RemoteAuthorization},
    permissions::{EnvironmentHttpClient, SessionGrants},
    platform::{Platform, PlatformError, SocketRequest, WebSocket},
    rpc::{RpcEndpoint, RpcMethod, SessionEndpoint},
};
use futures::{
    FutureExt, Sink, SinkExt, StreamExt,
    channel::mpsc,
    future::{Either, LocalBoxFuture, select},
};
use std::{cell::RefCell, pin::Pin, rc::Rc};
use t3_contracts::{base_schemas::EnvironmentId, server::ServerConfig};
use t3_rpc::Frame;

type FrameSink = Pin<Box<dyn Sink<Frame, Error = PlatformError>>>;

struct SessionScope {
    endpoint: SessionEndpoint,
    driver: RefCell<LocalBoxFuture<'static, ()>>,
    config: RefCell<Option<Rc<ServerConfig>>>,
    authorization: RemoteAuthorization,
    identity: crate::environment::SessionIdentity,
    revision: Option<u64>,
}
impl Drop for SessionScope {
    fn drop(&mut self) {
        self.endpoint.invalidate();
    }
}

#[derive(Clone)]
pub struct PlatformSession {
    inner: Rc<SessionScope>,
}
impl PlatformSession {
    pub fn endpoint(&self) -> SessionEndpoint {
        self.inner.endpoint.clone()
    }
    pub fn config(&self) -> Option<Rc<ServerConfig>> {
        self.inner.config.borrow().clone()
    }
    pub fn invalidate(&self) {
        self.inner.endpoint.invalidate();
    }
    pub async fn run(&self) {
        futures::future::poll_fn(|cx| self.inner.driver.borrow_mut().as_mut().poll(cx)).await;
    }
    async fn initial_config(&self) -> Result<(), ClientError> {
        let operation = self.inner.endpoint.initial_config().boxed_local();
        let driver = self.run().boxed_local();
        match select(operation, driver).await {
            Either::Left((config, _)) => {
                self.inner
                    .authorization
                    .validate_session_at(&self.inner.identity, self.inner.revision)
                    .await
                    .map_err(|error| ClientError::InvalidPayload(error.to_string()))?;
                self.inner.config.replace(Some(Rc::new(config?)));
                Ok(())
            }
            Either::Right(_) => Err(ClientError::Disconnected),
        }
    }
}
impl RpcEndpoint for PlatformSession {
    fn destination(&self) -> Option<EnvironmentId> {
        self.inner.endpoint.destination()
    }
    fn call<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
        self.inner.endpoint.call::<M>(input)
    }
    fn subscribe<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> futures::stream::LocalBoxStream<'static, Result<M::Output, ClientError>> {
        self.inner.endpoint.subscribe::<M>(input)
    }
    fn command<M: RpcMethod>(
        &self,
        input: M::Input,
    ) -> LocalBoxFuture<'_, Result<M::Output, crate::rpc::RpcCallError>>
    where
        M::Output: 'static,
    {
        self.inner.endpoint.command::<M>(input)
    }
}

#[derive(Clone)]
pub struct PlatformSessionFactory {
    platform: Rc<dyn Platform>,
    authorization: RemoteAuthorization,
    frames: Option<mpsc::Sender<super::ConnectionEvent>>,
}
impl PlatformSessionFactory {
    pub fn new(platform: Rc<dyn Platform>, authorization: RemoteAuthorization) -> Self {
        Self {
            platform,
            authorization,
            frames: None,
        }
    }
    pub(crate) fn with_frames(mut self, frames: mpsc::Sender<super::ConnectionEvent>) -> Self {
        self.frames = Some(frames);
        self
    }
}
impl SessionFactory for PlatformSessionFactory {
    type Session = PlatformSession;
    fn open<'a>(
        &'a self,
        prepared: &'a AuthorizedEnvironment,
    ) -> LocalBoxFuture<'a, Result<PlatformSession, ConnectionFailure>> {
        async move {
            let revision = self.authorization.session_revision(&prepared.session);
            let state = EnvironmentHttpClient::new(
                self.platform.clone(),
                prepared.clone(),
                self.authorization.dpop_signer(),
                Some(self.authorization.clone()),
                false,
            )
            .fetch_session_state(None)
            .await
            .map_err(|error| map_remote_environment_error(&error, ConnectionVia::Direct))?;
            let grants = SessionGrants::from(&state);
            if !grants.valid {
                return Err(ConnectionFailure::new(
                    FailureKind::Authentication,
                    "The server did not authenticate this connection.",
                ));
            }
            self.authorization
                .validate_session_at(&prepared.session, revision)
                .await
                .map_err(|error| map_remote_environment_error(&error, ConnectionVia::Direct))?;
            let socket = self
                .platform
                .websocket(SocketRequest {
                    url: prepared.socket_url.expose().clone(),
                    protocols: Vec::new(),
                })
                .await
                .map_err(super::map_connection_platform_error)?;
            self.authorization
                .validate_session_at(&prepared.session, revision)
                .await
                .map_err(|error| map_remote_environment_error(&error, ConnectionVia::Direct))?;
            let (outgoing, outgoing_rx) = mpsc::channel(16);
            let (incoming_tx, incoming) = mpsc::channel(8);
            let sink: FrameSink = Box::pin(outgoing.sink_map_err(|_| PlatformError::Closed));
            let (endpoint, rpc) = crate::rpc::session(
                sink,
                incoming,
                prepared.session.environment.clone(),
                grants,
                self.platform.monotonic_ms(),
            );
            let ticks = futures::stream::unfold(self.platform.clone(), |platform| async move {
                platform.sleep(1_000).await;
                Some((platform.monotonic_ms(), platform))
            })
            .boxed_local();
            let clock = self.platform.clone();
            let frames = self.frames.clone();
            let generation = prepared.session.generation;
            let expiration = match &prepared.authorization {
                crate::authorization::HttpAuthorization::Dpop {
                    expires_at_epoch_ms,
                    ..
                } => Some(*expires_at_epoch_ms),
                _ => None,
            };
            let authorization = self.authorization.clone();
            let identity = prepared.session.clone();
            let driver = async move {
                let io = futures::future::join(
                    rpc.run(ticks, move || clock.monotonic_ms()),
                    pump(socket, outgoing_rx, incoming_tx, frames, generation),
                )
                .map(|_| ())
                .boxed_local();
                if let Some(expiration) = expiration {
                    let renew = async move {
                        if authorization
                            .renew_dpop_before_expiry(&identity, expiration)
                            .await
                            .is_ok()
                        {
                            futures::future::pending::<()>().await;
                        }
                    }
                    .boxed_local();
                    let _ = select(io, renew).await;
                } else {
                    io.await;
                }
            }
            .boxed_local();
            Ok(PlatformSession {
                inner: Rc::new(SessionScope {
                    endpoint,
                    driver: RefCell::new(driver),
                    config: RefCell::new(None),
                    authorization: self.authorization.clone(),
                    identity: prepared.session.clone(),
                    revision,
                }),
            })
        }
        .boxed_local()
    }
    fn ready<'a>(
        &'a self,
        session: &'a PlatformSession,
    ) -> LocalBoxFuture<'a, Result<(), ConnectionFailure>> {
        async move {
            session
                .initial_config()
                .await
                .map_err(|error| super::runtime::session_failure(&error))
        }
        .boxed_local()
    }
}

async fn pump(
    mut socket: Box<dyn WebSocket>,
    mut outgoing: mpsc::Receiver<Frame>,
    mut incoming: mpsc::Sender<Result<Frame, PlatformError>>,
    mut frames: Option<mpsc::Sender<super::ConnectionEvent>>,
    generation: crate::types::Generation,
) {
    let mut pending = None;
    loop {
        enum Step {
            Send(Option<Frame>),
            Receive(Result<Option<Frame>, PlatformError>),
            Deliver(Result<(), mpsc::SendError>),
        }
        let step = if pending.is_some() {
            let ready = futures::future::poll_fn(|cx| incoming.poll_ready_unpin(cx));
            match select(outgoing.next(), ready).await {
                Either::Left((frame, _)) => Step::Send(frame),
                Either::Right((result, _)) => Step::Deliver(result),
            }
        } else {
            // Hosts promise cancel-safe receive; a send always finishes before another receive.
            match select(outgoing.next(), socket.receive()).await {
                Either::Left((frame, _)) => Step::Send(frame),
                Either::Right((frame, _)) => Step::Receive(frame),
            }
        };
        match step {
            Step::Send(Some(frame)) => {
                if socket.send(frame).await.is_err() {
                    break;
                }
            }
            Step::Send(None) | Step::Receive(Ok(None) | Err(_)) | Step::Deliver(Err(_)) => break,
            Step::Receive(Ok(Some(frame))) => {
                if let Some(frames) = &mut frames {
                    let _ = frames.try_send(super::ConnectionEvent::Frame { generation });
                }
                pending = Some(Ok(frame));
            }
            Step::Deliver(Ok(())) => {
                if let Some(frame) = pending.take()
                    && incoming.start_send_unpin(frame).is_err()
                {
                    break;
                }
            }
        }
    }
    let _ = socket.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::HostFuture;
    use std::cell::Cell;

    struct Socket {
        receive: mpsc::UnboundedReceiver<Frame>,
        sent: Rc<RefCell<Vec<Frame>>>,
        closed: Rc<Cell<bool>>,
    }
    impl WebSocket for Socket {
        fn send(&mut self, frame: Frame) -> HostFuture<'_, ()> {
            async move {
                self.sent.borrow_mut().push(frame);
                Ok(())
            }
            .boxed_local()
        }
        fn receive(&mut self) -> HostFuture<'_, Option<Frame>> {
            async move { Ok(self.receive.next().await) }.boxed_local()
        }
        fn close(&mut self) -> HostFuture<'_, ()> {
            async move {
                self.closed.set(true);
                Ok(())
            }
            .boxed_local()
        }
    }

    #[test]
    fn sends_and_close_progress_while_incoming_is_backpressured() {
        let (receive, receive_rx) = mpsc::unbounded();
        for index in 0..8 {
            assert!(
                receive
                    .unbounded_send(Frame::Text(index.to_string()))
                    .is_ok()
            );
        }
        let sent = Rc::new(RefCell::new(Vec::new()));
        let closed = Rc::new(Cell::new(false));
        let socket = Box::new(Socket {
            receive: receive_rx,
            sent: sent.clone(),
            closed: closed.clone(),
        });
        let (mut outgoing, outgoing_rx) = mpsc::channel(1);
        let (incoming, mut incoming_rx) = mpsc::channel(0);
        let mut driver = Box::pin(pump(
            socket,
            outgoing_rx,
            incoming,
            None,
            crate::types::Generation(1),
        ));
        assert!(driver.as_mut().now_or_never().is_none());
        assert!(outgoing.try_send(Frame::Text("request".into())).is_ok());
        assert!(driver.as_mut().now_or_never().is_none());
        assert_eq!(*sent.borrow(), vec![Frame::Text("request".into())]);
        assert!(
            matches!(incoming_rx.next().now_or_never(), Some(Some(Ok(Frame::Text(value)))) if value == "0")
        );
        drop(outgoing);
        assert!(driver.as_mut().now_or_never().is_some());
        assert!(closed.get());
    }
}
