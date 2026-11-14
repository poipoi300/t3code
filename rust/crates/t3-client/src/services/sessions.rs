use super::resources::Resource;
use crate::{
    ClientError,
    environment::SessionIdentity,
    platform::Platform,
    rpc::{RpcEndpoint, RpcMethod},
    types::EnvironmentId,
};
use futures::{
    FutureExt, Stream, StreamExt,
    future::{Either, LocalBoxFuture, select},
    stream::LocalBoxStream,
};
use std::{
    cell::RefCell,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

pub struct ServiceSession<R> {
    pub identity: SessionIdentity,
    pub endpoint: Rc<R>,
}
impl<R> Clone for ServiceSession<R> {
    fn clone(&self) -> Self {
        Self {
            identity: self.identity.clone(),
            endpoint: self.endpoint.clone(),
        }
    }
}
impl<R> ServiceSession<R> {
    pub(super) fn same(&self, other: &Self) -> bool {
        self.identity == other.identity && Rc::ptr_eq(&self.endpoint, &other.endpoint)
    }
}

pub struct EstablishedSessions<R> {
    previous: Option<ServiceSession<R>>,
    count: u64,
}
impl<R> Default for EstablishedSessions<R> {
    fn default() -> Self {
        Self {
            previous: None,
            count: 0,
        }
    }
}
impl<R> EstablishedSessions<R> {
    pub fn observe(
        &mut self,
        phase: QueryConnectionPhase,
        session: Option<ServiceSession<R>>,
    ) -> u64 {
        if phase == QueryConnectionPhase::Connected
            && let Some(session) = session
            && self
                .previous
                .as_ref()
                .is_none_or(|previous| !previous.same(&session))
        {
            self.previous = Some(session);
            self.count += 1;
        }
        self.count
    }
}

type Subscribe<'a, R, T> =
    Box<dyn FnMut(&ServiceSession<R>) -> LocalBoxStream<'a, Result<T, ClientError>> + 'a>;

/// Session notifications include the current value first; `None` cancels the installed stream.
pub struct EnvironmentStream<'a, R, T> {
    environment: EnvironmentId,
    sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
    current: Option<ServiceSession<R>>,
    active: Option<LocalBoxStream<'a, Result<T, ClientError>>>,
    subscribe: Subscribe<'a, R, T>,
    ended: bool,
}
impl<R, T> Unpin for EnvironmentStream<'_, R, T> {}
impl<'a, R: 'a, T: 'a> EnvironmentStream<'a, R, T> {
    pub fn new(
        environment: EnvironmentId,
        sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
        subscribe: impl FnMut(&ServiceSession<R>) -> LocalBoxStream<'a, Result<T, ClientError>> + 'a,
    ) -> Self {
        Self {
            environment,
            sessions,
            current: None,
            active: None,
            subscribe: Box::new(subscribe),
            ended: false,
        }
    }
    pub fn recovering<P: Platform + ?Sized>(
        environment: EnvironmentId,
        sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
        platform: &'a P,
        method: &'static str,
        subscribe: impl FnMut(&ServiceSession<R>) -> LocalBoxStream<'static, Result<T, ClientError>>
        + 'a,
    ) -> Self {
        let subscribe = Rc::new(RefCell::new(subscribe));
        Self::new(environment, sessions, move |session| {
            let session = session.clone();
            let subscribe = subscribe.clone();
            crate::rpc::subscription::recover_subscription(
                platform,
                method,
                move || subscribe.borrow_mut()(&session),
                Default::default(),
            )
        })
    }
}
impl<R: RpcEndpoint, T> Stream for EnvironmentStream<'_, R, T> {
    type Item = Result<T, ClientError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        // Drain replacement notices before polling old buffered frames.
        for _ in 0..32 {
            match this.sessions.as_mut().poll_next(cx) {
                Poll::Ready(Some(session)) => {
                    if session.as_ref().is_some_and(|s| {
                        s.identity.environment != this.environment
                            || s.endpoint
                                .destination()
                                .is_some_and(|id| id != this.environment)
                    }) {
                        this.active = None;
                        this.current = None;
                        return Poll::Ready(Some(Err(ClientError::EnvironmentMismatch)));
                    }
                    let same = match (&this.current, &session) {
                        (Some(a), Some(b)) => a.same(b),
                        (None, None) => true,
                        _ => false,
                    };
                    if !same {
                        this.active = None;
                        this.current = session;
                        this.active = this.current.as_ref().map(|s| (this.subscribe)(s));
                    }
                }
                Poll::Ready(None) => {
                    this.active = None;
                    this.ended = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return match &mut this.active {
                        Some(active) => match active.as_mut().poll_next(cx) {
                            Poll::Ready(None) => {
                                this.active = None;
                                Poll::Pending
                            }
                            event => event,
                        },
                        None => Poll::Pending,
                    };
                }
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

pub fn follow_rpc<M, R>(
    environment: EnvironmentId,
    sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
    input: M::Input,
) -> EnvironmentStream<'static, R, M::Output>
where
    M: RpcMethod,
    M::Input: Clone + 'static,
    M::Output: 'static,
    R: RpcEndpoint + 'static,
{
    EnvironmentStream::new(environment, sessions, move |session| {
        session.endpoint.subscribe::<M>(input.clone())
    })
}

/// Every event is applied before reading the next; this never uses a latest-value queue.
pub async fn consume_events<T>(
    mut events: impl Stream<Item = Result<T, ClientError>> + Unpin,
    mut apply: impl FnMut(T) -> Result<(), ClientError>,
) -> Result<(), ClientError> {
    while let Some(event) = events.next().await {
        apply(event?)?;
    }
    Ok(())
}

pub async fn follow_resource<M, R, P>(
    resource: &Resource<M::Output>,
    platform: &P,
    environment: EnvironmentId,
    mut sessions: LocalBoxStream<'static, Option<ServiceSession<R>>>,
    input: M::Input,
    complete: impl Fn(&M::Output) -> bool,
) -> Result<(), ClientError>
where
    M: RpcMethod,
    M::Input: Clone,
    M::Output: PartialEq,
    R: RpcEndpoint,
    P: Platform + ?Sized,
{
    let mut current: Option<ServiceSession<R>> = None;
    let mut running: Option<LocalBoxFuture<'_, Result<(), ClientError>>> = None;
    loop {
        let next = if let Some(work) = running.as_mut() {
            match select(sessions.next(), work).await {
                Either::Left((next, _)) => next,
                Either::Right((_result, _)) => {
                    running = None;
                    continue;
                }
            }
        } else {
            sessions.next().await
        };
        let Some(next) = next else {
            return Ok(());
        };
        if let (Some(a), Some(b)) = (&current, &next)
            && a.same(b)
        {
            continue;
        }
        running = None;
        resource.cancel();
        current = next;
        if let Some(session) = &current {
            if session.identity.environment != environment
                || session
                    .endpoint
                    .destination()
                    .is_some_and(|id| id != environment)
            {
                return Err(ClientError::EnvironmentMismatch);
            }
            resource.replace_session(session.identity.generation);
            resource.invalidate();
            let endpoint = session.endpoint.clone();
            let input = input.clone();
            let complete = &complete;
            running = Some(
                async move {
                    resource
                        .follow::<M, R, P>(endpoint.as_ref(), platform, input, complete)
                        .await
                }
                .boxed_local(),
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryConnectionPhase {
    Unknown,
    Connecting,
    Reconnecting,
    Connected,
    Backoff,
    Available,
    Offline,
    Blocked,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryAvailability {
    Execute,
    Wait,
    Fail(String),
}
pub fn query_availability(
    phase: QueryConnectionPhase,
    has_session: bool,
    has_answer: bool,
    failure: Option<&str>,
) -> QueryAvailability {
    use QueryConnectionPhase::*;
    match phase {
        Connected if has_session => QueryAvailability::Execute,
        Unknown | Connected => QueryAvailability::Wait,
        Connecting | Reconnecting | Backoff if has_answer || failure.is_none() => {
            QueryAvailability::Wait
        }
        Connecting | Reconnecting | Backoff | Available | Offline | Blocked => {
            QueryAvailability::Fail(failure.map(str::to_owned).unwrap_or_else(|| {
                match phase {
                    Available => "Environment is not connected.",
                    Offline => "Environment is offline.",
                    _ => "Environment is blocked.",
                }
                .into()
            }))
        }
    }
}

pub struct QueryConnection<R> {
    pub phase: QueryConnectionPhase,
    pub failure: Option<String>,
    pub session: Option<ServiceSession<R>>,
}
struct QueryObservation<'a, T, P: Platform + ?Sized> {
    observer: super::resources::ResourceSubscription<T>,
    platform: &'a P,
}
impl<T, P: Platform + ?Sized> Drop for QueryObservation<'_, T, P> {
    fn drop(&mut self) {
        self.observer.tick(self.platform.monotonic_ms());
    }
}

/// Held by a mounted reader; dropping this future cancels its request, save and renewal timer.
pub async fn drive_query<M, R, P>(
    resource: &Resource<M::Output>,
    platform: &P,
    environment: EnvironmentId,
    mut connections: LocalBoxStream<'static, QueryConnection<R>>,
    input: M::Input,
    persistence: Option<&str>,
    on_answer: impl Fn(&M::Output),
) -> Result<(), ClientError>
where
    M: RpcMethod,
    M::Input: Clone,
    M::Output: PartialEq + serde::Serialize,
    R: RpcEndpoint,
    P: Platform + ?Sized,
{
    use super::resources::QueryPersistence;
    let mut observation = QueryObservation {
        observer: resource.observe(platform.monotonic_ms()),
        platform,
    };
    let observer = &mut observation.observer;
    let _ = observer.next().await;
    if let Some(key) = persistence
        && resource.snapshot().value.is_none()
    {
        let cached = QueryPersistence { platform }
            .load(environment.clone(), key.into())
            .await;
        if let Ok(Some(cached)) = cached {
            let token = resource.begin();
            resource.finish(token, cached, platform.monotonic_ms(), false);
        }
    }
    let mut current: Option<QueryConnection<R>> = None;
    let mut work: Option<LocalBoxFuture<'_, Result<(), ClientError>>> = None;
    let mut save: Option<LocalBoxFuture<'_, Result<(), ClientError>>> = None;
    let mut refresh = true;
    loop {
        if resource.is_closed() {
            return Err(ClientError::Cancelled);
        }
        if refresh
            && work.is_none()
            && !resource.is_final()
            && let Some(connection) = &current
        {
            refresh = false;
            match query_availability(
                connection.phase,
                connection.session.is_some(),
                resource.snapshot().value.is_some(),
                connection.failure.as_deref(),
            ) {
                QueryAvailability::Execute => {
                    let session = connection.session.as_ref().expect("available session");
                    if session.identity.environment != environment
                        || session
                            .endpoint
                            .destination()
                            .is_some_and(|id| id != environment)
                    {
                        return Err(ClientError::EnvironmentMismatch);
                    }
                    resource.replace_session(session.identity.generation);
                    let endpoint = session.endpoint.clone();
                    let input = input.clone();
                    let on_answer = &on_answer;
                    work = Some(
                        async move {
                            resource
                                .execute_with::<M, R, P>(
                                    endpoint.as_ref(),
                                    platform,
                                    input,
                                    on_answer,
                                )
                                .await
                        }
                        .boxed_local(),
                    );
                }
                QueryAvailability::Wait => {}
                QueryAvailability::Fail(message) => {
                    if persistence.is_some() && resource.snapshot().value.is_some() {
                        continue;
                    }
                    let token = resource.begin();
                    resource.fail(
                        token,
                        &ClientError::InvalidPayload(message),
                        platform.monotonic_ms(),
                    );
                }
            }
        }
        let next_work = async {
            match work.as_mut() {
                Some(work) => work.await,
                None => futures::future::pending().await,
            }
        }
        .boxed_local();
        let next_save = async {
            match save.as_mut() {
                Some(save) => save.await,
                None => futures::future::pending().await,
            }
        }
        .boxed_local();
        let renewal = async {
            if !resource.is_final()
                && current
                    .as_ref()
                    .is_some_and(|connection| connection.phase == QueryConnectionPhase::Connected)
                && let Some(interval) = resource.policy.refresh_ms
            {
                platform.sleep(interval.max(1)).await;
            } else {
                futures::future::pending::<()>().await;
            }
        }
        .boxed_local();
        enum Event<R> {
            Connection(Option<QueryConnection<R>>),
            Changed,
            Work(Result<(), ClientError>),
            Saved,
            Renew,
        }
        let event = {
            let mut changed = observer.next().fuse();
            let mut connection = connections.next().fuse();
            let mut work = next_work.fuse();
            let mut saved = next_save.fuse();
            let mut renewal = renewal.fuse();
            futures::select_biased! {
                connection = connection => Event::Connection(connection),
                result = work => Event::Work(result),
                _ = saved => Event::Saved,
                _ = changed => Event::Changed,
                _ = renewal => Event::Renew,
            }
        };
        match event {
            Event::Connection(None) => return Ok(()),
            Event::Connection(Some(connection)) => {
                let first = current.is_none();
                let same_session = match (
                    current
                        .as_ref()
                        .and_then(|connection| connection.session.as_ref()),
                    &connection.session,
                ) {
                    (Some(a), Some(b)) => a.same(b),
                    (None, None) => true,
                    _ => false,
                };
                let fresh_mount = first
                    && connection.session.as_ref().is_some_and(|session| {
                        session.identity.generation == resource.session_generation()
                    });
                if !same_session || connection.phase != QueryConnectionPhase::Connected {
                    work = None;
                    save = None;
                    resource.cancel();
                    refresh = true;
                }
                if !same_session && !fresh_mount && connection.session.is_some() {
                    resource.invalidate();
                }
                if first {
                    refresh = !fresh_mount || resource.needs_refresh(platform.monotonic_ms());
                }
                current = Some(connection);
            }
            Event::Work(result) => {
                work = None;
                if result.is_ok()
                    && let (Some(key), Some(value)) = (persistence, resource.snapshot().value)
                {
                    let environment = environment.clone();
                    save = Some(
                        async move {
                            QueryPersistence { platform }
                                .save(environment, key.into(), value.as_ref())
                                .await
                        }
                        .boxed_local(),
                    );
                }
            }
            Event::Saved => save = None,
            Event::Changed if resource.is_dirty() && !resource.snapshot().loading => {
                work = None;
                refresh = true;
            }
            Event::Renew => refresh = true,
            Event::Changed => {}
        }
        observer.tick(platform.monotonic_ms());
    }
}
