use super::ResourceKey;
use crate::{
    ClientError,
    platform::{Platform, StorageArea, StorageKey, StorageWrite},
    rpc::{RpcEndpoint, RpcMethod},
    types::{EnvironmentId, Generation, QueryState},
};
use futures::{Stream, StreamExt, future::AbortHandle};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    any::Any,
    cell::RefCell,
    collections::BTreeMap,
    pin::Pin,
    rc::{Rc, Weak},
    task::{Context, Poll, Waker},
};

#[derive(Clone)]
pub struct LocalLimiter(Rc<RefCell<LimitState>>);
struct LimitState {
    limit: usize,
    active: usize,
    next: u64,
    waiting: BTreeMap<u64, Option<Waker>>,
}
impl LocalLimiter {
    pub fn new(limit: usize) -> Self {
        Self(Rc::new(RefCell::new(LimitState {
            limit: limit.max(1),
            active: 0,
            next: 0,
            waiting: BTreeMap::new(),
        })))
    }
    pub fn acquire(&self) -> AcquirePermit {
        let id = {
            let mut state = self.0.borrow_mut();
            state.next += 1;
            let id = state.next;
            state.waiting.insert(id, None);
            id
        };
        AcquirePermit {
            limiter: self.clone(),
            id,
            acquired: false,
        }
    }
    fn wake(&self) {
        let wakers = {
            let mut state = self.0.borrow_mut();
            let available = state.limit - state.active;
            state
                .waiting
                .values_mut()
                .take(available)
                .filter_map(Option::take)
                .collect::<Vec<_>>()
        };
        for waker in wakers {
            waker.wake();
        }
    }
}
pub struct AcquirePermit {
    limiter: LocalLimiter,
    id: u64,
    acquired: bool,
}
impl std::future::Future for AcquirePermit {
    type Output = LocalPermit;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let ready = {
            let mut state = self.limiter.0.borrow_mut();
            let available = state.limit - state.active;
            let ready = state
                .waiting
                .keys()
                .take(available)
                .any(|id| *id == self.id);
            if ready {
                state.waiting.remove(&self.id);
                state.active += 1;
            } else if let Some(waker) = state.waiting.get_mut(&self.id) {
                *waker = Some(cx.waker().clone());
            }
            ready
        };
        if ready {
            self.acquired = true;
            Poll::Ready(LocalPermit(self.limiter.clone()))
        } else {
            Poll::Pending
        }
    }
}
impl Drop for AcquirePermit {
    fn drop(&mut self) {
        if !self.acquired {
            self.limiter.0.borrow_mut().waiting.remove(&self.id);
            self.limiter.wake();
        }
    }
}
pub struct LocalPermit(LocalLimiter);
impl Drop for LocalPermit {
    fn drop(&mut self) {
        self.0.0.borrow_mut().active -= 1;
        self.0.wake();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePolicy {
    pub stale_ms: u64,
    pub idle_ttl_ms: u64,
    pub refresh_ms: Option<u64>,
    pub completed_ttl_ms: Option<u64>,
}
impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            stale_ms: 30_000,
            idle_ttl_ms: 300_000,
            refresh_ms: None,
            completed_ttl_ms: None,
        }
    }
}

struct Observer {
    revision: Option<u64>,
    waker: Option<Waker>,
}
struct Inner<T> {
    state: QueryState<Rc<T>>,
    session: Generation,
    request: u64,
    answered_at: Option<u64>,
    idle_at: Option<u64>,
    readers: usize,
    dirty: bool,
    completed: bool,
    active: Option<AbortHandle>,
    observers: Vec<Weak<RefCell<Observer>>>,
    closed: bool,
}

pub struct Resource<T> {
    inner: Rc<RefCell<Inner<T>>>,
    pub policy: ResourcePolicy,
}
impl<T> Clone for Resource<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            policy: self.policy,
        }
    }
}
impl<T> Resource<T> {
    pub fn new(policy: ResourcePolicy, session: Generation) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                state: QueryState {
                    value: None,
                    loading: false,
                    error: None,
                    revision: 0,
                },
                session,
                request: 0,
                answered_at: None,
                idle_at: None,
                readers: 0,
                dirty: false,
                completed: false,
                active: None,
                observers: Vec::new(),
                closed: false,
            })),
            policy,
        }
    }
    pub fn snapshot(&self) -> QueryState<Rc<T>> {
        let inner = self.inner.borrow();
        QueryState {
            value: inner.state.value.clone(),
            loading: inner.state.loading,
            error: inner.state.error.clone(),
            revision: inner.state.revision,
        }
    }
    fn change(&self, update: impl FnOnce(&mut Inner<T>)) {
        let wake = {
            let mut inner = self.inner.borrow_mut();
            update(&mut inner);
            inner.state.revision += 1;
            let revision = inner.state.revision;
            let mut wake = Vec::new();
            inner.observers.retain(|observer| {
                let Some(observer) = observer.upgrade() else {
                    return false;
                };
                let mut observer = observer.borrow_mut();
                observer.revision = Some(revision);
                if let Some(waker) = observer.waker.take() {
                    wake.push(waker);
                }
                true
            });
            wake
        };
        for waker in wake {
            waker.wake();
        }
    }
    /// The shell retains this stream while the resource is visible, and drives requested work.
    pub fn observe(&self, now: u64) -> ResourceSubscription<T> {
        let mut inner = self.inner.borrow_mut();
        let observer = Rc::new(RefCell::new(Observer {
            revision: Some(inner.state.revision),
            waker: None,
        }));
        inner.readers += 1;
        inner.idle_at = None;
        inner.observers.push(Rc::downgrade(&observer));
        ResourceSubscription {
            resource: self.clone(),
            observer,
            last_tick: now,
        }
    }
    pub fn needs_refresh(&self, now: u64) -> bool {
        let inner = self.inner.borrow();
        if inner.closed
            || inner.state.loading
            || inner.active.is_some()
            || inner.completed
            || inner.readers == 0
        {
            return false;
        }
        inner.dirty
            || (inner.state.error.is_some() && inner.state.value.is_none())
            || inner
                .answered_at
                .is_none_or(|at| now.saturating_sub(at) >= self.policy.stale_ms)
    }
    pub fn has_readers(&self) -> bool {
        self.inner.borrow().readers > 0
    }
    pub fn is_final(&self) -> bool {
        self.inner.borrow().completed
    }
    pub fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }
    pub fn session_generation(&self) -> Generation {
        self.inner.borrow().session
    }
    pub fn is_closed(&self) -> bool {
        self.inner.borrow().closed
    }
    pub fn close(&self) {
        self.cancel();
        self.change(|inner| {
            inner.closed = true;
            inner.dirty = false;
        });
    }
    pub fn interval_due(&self, now: u64) -> bool {
        let inner = self.inner.borrow();
        !inner.completed
            && !inner.state.loading
            && inner.active.is_none()
            && inner.readers > 0
            && self.policy.refresh_ms.is_some_and(|interval| {
                inner
                    .answered_at
                    .is_some_and(|at| now.saturating_sub(at) >= interval)
            })
    }
    pub fn expired(&self, now: u64) -> bool {
        let inner = self.inner.borrow();
        let ttl = if inner.completed {
            self.policy
                .completed_ttl_ms
                .unwrap_or(self.policy.idle_ttl_ms)
        } else {
            self.policy.idle_ttl_ms
        };
        inner.readers == 0
            && inner
                .idle_at
                .is_some_and(|at| now.saturating_sub(at) >= ttl)
    }
    pub fn invalidate(&self) {
        if self.is_closed() {
            return;
        }
        self.cancel();
        self.change(|inner| {
            inner.dirty = true;
            inner.completed = false;
        });
    }
    pub fn replace_session(&self, session: Generation) {
        if self.is_closed() {
            return;
        }
        if self.inner.borrow().session == session {
            return;
        }
        self.cancel();
        self.change(|inner| {
            inner.session = session;
            inner.dirty = true;
            inner.completed = false;
        });
    }
    pub fn cancel(&self) {
        let active = {
            let mut inner = self.inner.borrow_mut();
            inner.request += 1;
            inner.active.take()
        };
        if let Some(active) = active {
            active.abort();
        }
        if self.inner.borrow().state.loading {
            self.change(|inner| inner.state.loading = false);
        }
    }
    pub fn begin(&self) -> RequestToken {
        self.cancel();
        let token = {
            let inner = self.inner.borrow();
            RequestToken {
                session: inner.session,
                request: inner.request,
            }
        };
        if !self.is_closed() {
            self.change(|inner| inner.state.loading = true);
        }
        token
    }
    pub fn is_current(&self, token: RequestToken) -> bool {
        let inner = self.inner.borrow();
        !inner.closed && inner.session == token.session && inner.request == token.request
    }
    pub fn fail(&self, token: RequestToken, error: &ClientError, now: u64) -> bool {
        if !self.is_current(token) {
            return false;
        }
        self.change(|inner| {
            inner.state.loading = false;
            inner.state.error = Some(error.to_string());
            inner.active = None;
            inner.dirty = false;
            // Failures are retried on the next mount, while cached answers retain their age.
            if inner.state.value.is_none() {
                inner.answered_at = Some(now.saturating_sub(self.policy.stale_ms));
            }
        });
        true
    }
}
impl<T: PartialEq> Resource<T> {
    pub async fn run<P, F>(
        &self,
        platform: &P,
        execute: impl FnOnce(RequestToken) -> F,
    ) -> Result<(), ClientError>
    where
        P: Platform + ?Sized,
        F: std::future::Future<Output = Result<T, ClientError>>,
    {
        let token = self.begin();
        if !self.is_current(token) {
            return Err(ClientError::Cancelled);
        }
        let (abort, registration) = AbortHandle::new_pair();
        self.inner.borrow_mut().active = Some(abort);
        let mut guard = RequestGuard {
            resource: self.clone(),
            token,
            finished: false,
        };
        let result = futures::future::Abortable::new(execute(token), registration).await;
        if !self.is_current(token) {
            return Err(ClientError::Cancelled);
        }
        guard.finished = true;
        match result {
            Ok(Ok(value)) => {
                self.finish(token, value, platform.monotonic_ms(), false);
                Ok(())
            }
            Ok(Err(error)) => {
                self.fail(token, &error, platform.monotonic_ms());
                Err(error)
            }
            Err(_) => {
                self.cancel();
                Err(ClientError::Cancelled)
            }
        }
    }
    pub fn emit(&self, token: RequestToken, value: T) -> bool {
        self.publish_value(token, value, None, false)
    }
    pub fn finish(&self, token: RequestToken, value: T, now: u64, completed: bool) -> bool {
        self.publish_value(token, value, Some(now), completed)
    }
    fn publish_value(
        &self,
        token: RequestToken,
        value: T,
        at: Option<u64>,
        completed: bool,
    ) -> bool {
        if !self.is_current(token) {
            return false;
        }
        self.change(|inner| {
            if inner.state.value.as_deref() != Some(&value) {
                inner.state.value = Some(Rc::new(value));
            }
            inner.state.loading = at.is_none();
            inner.state.error = None;
            if let Some(at) = at {
                inner.answered_at = Some(at);
                inner.completed = completed;
                inner.active = None;
                inner.dirty = false;
            }
        });
        true
    }
    pub async fn execute<M, R, P>(
        &self,
        endpoint: &R,
        platform: &P,
        input: M::Input,
    ) -> Result<(), ClientError>
    where
        M: RpcMethod<Output = T>,
        R: RpcEndpoint,
        P: Platform + ?Sized,
    {
        self.execute_with::<M, R, P>(endpoint, platform, input, |_| {})
            .await
    }
    pub async fn execute_with<M, R, P>(
        &self,
        endpoint: &R,
        platform: &P,
        input: M::Input,
        on_answer: impl FnOnce(&T),
    ) -> Result<(), ClientError>
    where
        M: RpcMethod<Output = T>,
        R: RpcEndpoint,
        P: Platform + ?Sized,
    {
        let token = self.begin();
        if !self.is_current(token) {
            return Err(ClientError::Cancelled);
        }
        let (abort, registration) = AbortHandle::new_pair();
        self.inner.borrow_mut().active = Some(abort.clone());
        let mut guard = RequestGuard {
            resource: self.clone(),
            token,
            finished: false,
        };
        let result = futures::future::Abortable::new(endpoint.call::<M>(input), registration).await;
        if !self.is_current(token) {
            return Err(ClientError::Cancelled);
        }
        guard.finished = true;
        match result {
            Ok(Ok(value)) => {
                on_answer(&value);
                self.finish(token, value, platform.monotonic_ms(), false);
                Ok(())
            }
            Ok(Err(error)) => {
                self.fail(token, &error, platform.monotonic_ms());
                Err(error)
            }
            Err(_) => {
                self.cancel();
                Err(ClientError::Cancelled)
            }
        }
    }
    pub async fn follow<M, R, P>(
        &self,
        endpoint: &R,
        platform: &P,
        input: M::Input,
        complete_when: impl Fn(&T) -> bool,
    ) -> Result<(), ClientError>
    where
        M: RpcMethod<Output = T>,
        R: RpcEndpoint,
        P: Platform + ?Sized,
    {
        let token = self.begin();
        if !self.is_current(token) {
            return Err(ClientError::Cancelled);
        }
        let (abort, registration) = AbortHandle::new_pair();
        self.inner.borrow_mut().active = Some(abort.clone());
        let mut guard = RequestGuard {
            resource: self.clone(),
            token,
            finished: false,
        };
        let stream = endpoint.subscribe::<M>(input);
        let stream = futures::stream::Abortable::new(stream, registration);
        futures::pin_mut!(stream);
        while let Some(value) = stream.next().await {
            if !self.is_current(token) {
                return Err(ClientError::Cancelled);
            }
            match value {
                Ok(value) => {
                    let complete = complete_when(&value);
                    if complete {
                        self.finish(token, value, platform.monotonic_ms(), true);
                        guard.finished = true;
                        return Ok(());
                    }
                    self.emit(token, value);
                }
                Err(error) => {
                    self.fail(token, &error, platform.monotonic_ms());
                    guard.finished = true;
                    return Err(error);
                }
            }
        }
        guard.finished = true;
        if !self.is_current(token) {
            return Err(ClientError::Cancelled);
        }
        self.cancel();
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestToken {
    session: Generation,
    request: u64,
}
struct RequestGuard<T> {
    resource: Resource<T>,
    token: RequestToken,
    finished: bool,
}
impl<T> Drop for RequestGuard<T> {
    fn drop(&mut self) {
        if !self.finished && self.resource.is_current(self.token) {
            self.resource.cancel();
        }
    }
}
pub struct ResourceSubscription<T> {
    resource: Resource<T>,
    observer: Rc<RefCell<Observer>>,
    last_tick: u64,
}
impl<T> ResourceSubscription<T> {
    pub fn tick(&mut self, now: u64) {
        self.last_tick = now;
    }
}
impl<T> Stream for ResourceSubscription<T> {
    type Item = u64;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<u64>> {
        let mut observer = self.observer.borrow_mut();
        if let Some(revision) = observer.revision.take() {
            Poll::Ready(Some(revision))
        } else {
            observer.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}
impl<T> Drop for ResourceSubscription<T> {
    fn drop(&mut self) {
        let mut inner = self.resource.inner.borrow_mut();
        inner.readers -= 1;
        let observer = Rc::downgrade(&self.observer);
        inner.observers.retain(|entry| !entry.ptr_eq(&observer));
        if inner.readers == 0 {
            inner.idle_at = Some(self.last_tick);
        }
        drop(inner);
        if self.resource.policy.idle_ttl_ms == 0 {
            self.resource.cancel();
        }
    }
}

trait ErasedResource: Any {
    fn as_any(&self) -> &dyn Any;
    fn invalidate(&self);
    fn session(&self, generation: Generation);
    fn expired(&self, now: u64) -> bool;
    fn cancel(&self);
    fn close(&self);
}
impl<T: 'static> ErasedResource for Resource<T> {
    fn close(&self) {
        self.close();
    }
    fn cancel(&self) {
        self.cancel();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn invalidate(&self) {
        self.invalidate();
    }
    fn session(&self, generation: Generation) {
        self.replace_session(generation);
    }
    fn expired(&self, now: u64) -> bool {
        self.expired(now)
    }
}
#[derive(Default)]
pub struct ResourceRegistry {
    resources: BTreeMap<ResourceKey, Rc<dyn ErasedResource>>,
    sessions: BTreeMap<EnvironmentId, Generation>,
}
impl ResourceRegistry {
    pub fn disconnect(&self, environment: &EnvironmentId) {
        for (key, resource) in &self.resources {
            if &key.environment == environment {
                resource.cancel();
            }
        }
    }
    pub fn query<M: RpcMethod>(
        &mut self,
        environment: EnvironmentId,
        input: &M::Input,
        policy: ResourcePolicy,
    ) -> Result<Resource<M::Output>, ClientError>
    where
        M::Output: 'static,
    {
        let key = ResourceKey {
            environment: environment.clone(),
            resource: M::NAME.into(),
            arguments: serde_json::to_string(input)?,
        };
        if let Some(resource) = self.resources.get(&key) {
            return resource
                .as_any()
                .downcast_ref::<Resource<M::Output>>()
                .cloned()
                .ok_or_else(|| {
                    ClientError::InvalidPayload("resource method output type changed".into())
                });
        }
        let resource = Resource::new(
            policy,
            self.sessions.get(&environment).copied().unwrap_or_default(),
        );
        self.resources.insert(key, Rc::new(resource.clone()));
        Ok(resource)
    }
    pub fn replace_session(&mut self, environment: &EnvironmentId, generation: Generation) {
        self.sessions.insert(environment.clone(), generation);
        for (key, resource) in &self.resources {
            if &key.environment == environment {
                resource.session(generation);
            }
        }
    }
    pub fn invalidate(&self, environment: &EnvironmentId, method: &str) {
        for (key, resource) in &self.resources {
            if &key.environment == environment && key.resource == method {
                resource.invalidate();
            }
        }
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.resources.retain(|key, resource| {
            if &key.environment != environment {
                return true;
            }
            resource.close();
            false
        });
        self.sessions.remove(environment);
    }
    pub fn collect(&mut self, now: u64) {
        self.resources.retain(|_, resource| {
            if !resource.expired(now) {
                return true;
            }
            resource.close();
            false
        });
    }
}

pub fn query_persistence_key(input: &impl Serialize, label: &str) -> Result<String, ClientError> {
    Ok(serde_json::to_string(&(label, input))?)
}
pub struct QueryPersistence<'a, P: ?Sized> {
    pub platform: &'a P,
}
impl<P: Platform + ?Sized> QueryPersistence<'_, P> {
    fn key(environment: EnvironmentId, record: String) -> StorageKey {
        StorageKey {
            namespace: "query".into(),
            environment: Some(environment),
            record,
        }
    }
    pub async fn load<T: DeserializeOwned>(
        &self,
        environment: EnvironmentId,
        record: String,
    ) -> Result<Option<T>, ClientError> {
        let bytes = self
            .platform
            .get(
                StorageArea::Ordinary,
                Self::key(environment.clone(), record.clone()),
            )
            .await?;
        Ok(bytes
            .and_then(|bytes| serde_json::from_slice::<crate::persistence::Record<T>>(&bytes).ok())
            .filter(|stored| {
                stored.format_version == 1
                    && stored.kind == "query"
                    && stored.environment_id.as_ref() == Some(&environment)
                    && stored.record_id == record
                    && stored.payload_version == 1
            })
            .map(|stored| stored.payload))
    }
    pub async fn save(
        &self,
        environment: EnvironmentId,
        record: String,
        value: &impl Serialize,
    ) -> Result<(), ClientError> {
        let value = serde_json::to_vec(&crate::persistence::Record {
            format_version: 1,
            kind: "query".into(),
            environment_id: Some(environment.clone()),
            record_id: record.clone(),
            payload_version: 1,
            payload: value,
        })?;
        self.platform
            .write(
                StorageArea::Ordinary,
                vec![StorageWrite {
                    key: Self::key(environment, record),
                    value: Some(value),
                }],
            )
            .await?;
        Ok(())
    }
}
