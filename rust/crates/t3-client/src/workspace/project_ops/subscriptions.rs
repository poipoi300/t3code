use super::super::vcs::{
    CheckoutKey, REFS_IDLE_TTL_MS, STATUS_IDLE_TTL_MS, VcsRefsCache, apply_status_event,
    can_use_refs_cache, refs_retry_delay_ms,
};
use super::source_control::check_destination;
use crate::{
    ClientError,
    platform::Platform,
    rpc::RpcEndpoint,
    services::resources::{Resource, ResourcePolicy, ResourceRegistry},
    types::{EnvironmentId, Generation},
};
use futures::{
    StreamExt,
    future::{AbortHandle, Abortable, Either, select},
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use t3_contracts::{git::*, rpc::*};

struct Session<R> {
    generation: Generation,
    endpoint: Option<Rc<R>>,
}
impl<R> Clone for Session<R> {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation,
            endpoint: self.endpoint.clone(),
        }
    }
}
impl<R> PartialEq for Session<R> {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
            && match (&self.endpoint, &other.endpoint) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
    }
}
struct Driver {
    id: u64,
    environment: EnvironmentId,
    abort: AbortHandle,
    expired: Box<dyn Fn(u64) -> bool>,
}
pub struct LiveVcs<R> {
    pub refs_cache: VcsRefsCache,
    resources: RefCell<ResourceRegistry>,
    sessions: RefCell<BTreeMap<EnvironmentId, Resource<Session<R>>>>,
    drivers: Rc<RefCell<BTreeMap<String, Driver>>>,
    next: Cell<u64>,
}
impl<R: RpcEndpoint + 'static> LiveVcs<R> {
    pub fn new(refs_cache: VcsRefsCache) -> Self {
        Self {
            refs_cache,
            resources: RefCell::default(),
            sessions: RefCell::default(),
            drivers: Rc::default(),
            next: Cell::new(0),
        }
    }
    fn session(&self, environment: &EnvironmentId) -> Resource<Session<R>> {
        self.sessions
            .borrow_mut()
            .entry(environment.clone())
            .or_insert_with(|| Resource::new(ResourcePolicy::default(), Generation::default()))
            .clone()
    }
    pub fn set_session(
        &self,
        environment: EnvironmentId,
        generation: Generation,
        endpoint: Option<Rc<R>>,
    ) -> Result<(), ClientError> {
        if let Some(endpoint) = &endpoint {
            check_destination(endpoint.as_ref(), &environment)
                .map_err(|e| e.into_client_error())?;
        }
        let resource = self.session(&environment);
        let next = Session {
            generation,
            endpoint,
        };
        if resource
            .snapshot()
            .value
            .as_deref()
            .is_some_and(|s| s.generation.0 > generation.0 || s == &next)
        {
            return Ok(());
        }
        self.resources
            .borrow_mut()
            .replace_session(&environment, generation);
        let token = resource.begin();
        resource.finish(token, next, 0, false);
        Ok(())
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        let session = self.session(environment);
        let generation = Generation(
            session
                .snapshot()
                .value
                .as_ref()
                .map_or(1, |value| value.generation.0.saturating_add(1)),
        );
        let token = session.begin();
        session.finish(
            token,
            Session {
                generation,
                endpoint: None,
            },
            0,
            false,
        );
        self.resources.borrow_mut().remove_environment(environment);
        self.refs_cache.remove_environment(environment);
        let keys: Vec<_> = self
            .drivers
            .borrow()
            .iter()
            .filter(|(_, driver)| &driver.environment == environment)
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            let driver = self.drivers.borrow_mut().remove(&key);
            if let Some(driver) = driver {
                driver.abort.abort();
            }
        }
    }
    pub fn list_refs(
        &self,
        environment: EnvironmentId,
        input: &VcsListRefsInput,
    ) -> Result<Resource<VcsListRefsResult>, ClientError> {
        self.resource::<WsVcsListRefsRpc>(environment, input, REFS_IDLE_TTL_MS)
    }
    pub fn branch(
        &self,
        environment: EnvironmentId,
        input: &VcsStatusInput,
    ) -> Result<Resource<VcsBranchStatus>, ClientError> {
        self.resource::<WsSubscribeVcsBranchRpc>(environment, input, STATUS_IDLE_TTL_MS)
    }
    pub fn status(
        &self,
        environment: EnvironmentId,
        input: &VcsStatusSubscriptionInput,
    ) -> Result<Resource<VcsStatusResult>, ClientError> {
        // The stream's wire event differs from the accumulated status exposed to readers.
        struct Status;
        impl crate::rpc::RpcMethod for Status {
            type Input = VcsStatusSubscriptionInput;
            type Output = VcsStatusResult;
            const NAME: &'static str = "workspace:accumulated-vcs-status";
        }
        self.resources.borrow_mut().query::<Status>(
            environment,
            input,
            ResourcePolicy {
                idle_ttl_ms: STATUS_IDLE_TTL_MS,
                ..ResourcePolicy::default()
            },
        )
    }
    pub fn resource<M: t3_contracts::rpc::RpcMethod>(
        &self,
        environment: EnvironmentId,
        input: &M::Payload,
        idle_ttl_ms: u64,
    ) -> Result<Resource<M::Success>, ClientError>
    where
        M::Success: 'static,
    {
        self.resources.borrow_mut().query::<M>(
            environment,
            input,
            ResourcePolicy {
                idle_ttl_ms,
                ..ResourcePolicy::default()
            },
        )
    }
    fn claim<T: 'static>(
        &self,
        environment: &EnvironmentId,
        key: String,
        resource: &Resource<T>,
    ) -> (DriverGuard, futures::future::AbortRegistration) {
        let (abort, registration) = AbortHandle::new_pair();
        self.next.set(self.next.get() + 1);
        let id = self.next.get();
        let resource = resource.clone();
        let old = self.drivers.borrow_mut().insert(
            key.clone(),
            Driver {
                id,
                environment: environment.clone(),
                abort,
                expired: Box::new(move |now| resource.expired(now)),
            },
        );
        if let Some(old) = old {
            old.abort.abort();
        }
        (
            DriverGuard {
                drivers: self.drivers.clone(),
                key,
                id,
            },
            registration,
        )
    }
    pub fn sweep(&self, now: u64) {
        let expired: Vec<_> = self
            .drivers
            .borrow()
            .iter()
            .filter(|(_, driver)| (driver.expired)(now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            let driver = self.drivers.borrow_mut().remove(&key);
            if let Some(driver) = driver {
                driver.abort.abort();
            }
        }
        self.resources.borrow_mut().collect(now);
    }
    pub async fn follow_refs(
        &self,
        environment: EnvironmentId,
        platform: &(impl Platform + ?Sized),
        input: VcsListRefsInput,
    ) -> Result<(), ClientError> {
        let resource = self.list_refs(environment.clone(), &input)?;
        let key = serde_json::to_string(&(&environment, "refs", &input))?;
        let (_owner, abort) = self.claim(&environment, key, &resource);
        let mut guard = ResourceGuard {
            resource: resource.clone(),
            token: None,
        };
        let driver = async {
            let target = CheckoutKey {
                environment: environment.clone(),
                cwd: input.cwd.0.clone(),
            };
            let session = self.session(&environment);
            let mut session_changes = session.observe(platform.monotonic_ms());
            let mut cache_changes = self.refs_cache.observe(&environment);
            session_changes.next().await;
            cache_changes.next().await;
            let mut changes = futures::stream::select(session_changes, cache_changes);
            let token = resource.begin();
            guard.token = Some(token);
            if let Some(cached) = self.refs_cache.load(platform, &target, &input).await {
                resource.finish(token, cached, platform.monotonic_ms(), false);
            }
            let mut attempt = 0;
            loop {
                let Some(current) = session.snapshot().value.filter(|s| s.endpoint.is_some())
                else {
                    resource.cancel();
                    changes.next().await;
                    attempt = 0;
                    continue;
                };
                let endpoint = current.endpoint.as_ref().ok_or(ClientError::Disconnected)?;
                let expected = self.refs_cache.state(&environment).revision;
                let mut request = input.clone();
                if self.refs_cache.bypass_server_snapshot(&target, expected) {
                    request.refresh = Some(Some(true));
                }
                let token = resource.begin();
                guard.token = Some(token);
                let refresh = async {
                    let refs = endpoint.call::<WsVcsListRefsRpc>(request).await?;
                    if !resource.is_current(token) {
                        return Err(ClientError::Cancelled);
                    }
                    let committed = self
                        .refs_cache
                        .commit_refresh(
                            platform,
                            &target,
                            &refs,
                            expected,
                            can_use_refs_cache(&input),
                        )
                        .await;
                    Ok(committed.then_some(refs))
                };
                let result = match select(changes.next(), Box::pin(refresh)).await {
                    Either::Left(_) => {
                        attempt = 0;
                        continue;
                    }
                    Either::Right((result, _)) => result,
                };
                match result {
                    Ok(Some(refs)) => {
                        resource.finish(token, refs, platform.monotonic_ms(), false);
                        attempt = 0;
                        changes.next().await;
                    }
                    Ok(None) | Err(ClientError::Cancelled) => {
                        attempt = 0;
                        if self.refs_cache.state(&environment).revision == expected {
                            changes.next().await;
                        }
                    }
                    Err(error) => {
                        resource.fail(token, &error, platform.monotonic_ms());
                        let delay = refs_retry_delay_ms(attempt);
                        attempt = attempt.saturating_add(1);
                        if matches!(
                            select(changes.next(), platform.sleep(delay)).await,
                            Either::Left(_)
                        ) {
                            attempt = 0;
                        }
                    }
                }
            }
        };
        Abortable::new(driver, abort)
            .await
            .unwrap_or(Err(ClientError::Cancelled))
    }
    pub async fn follow_branch(
        &self,
        environment: EnvironmentId,
        platform: &(impl Platform + ?Sized),
        input: VcsStatusInput,
    ) -> Result<(), ClientError> {
        let state = self.branch(environment.clone(), &input)?;
        self.follow::<WsSubscribeVcsBranchRpc, _>(
            environment,
            platform,
            input,
            state,
            |_, value| value,
        )
        .await
    }
    pub async fn follow_status(
        &self,
        environment: EnvironmentId,
        platform: &(impl Platform + ?Sized),
        input: VcsStatusSubscriptionInput,
    ) -> Result<(), ClientError> {
        let state = self.status(environment.clone(), &input)?;
        self.follow::<WsSubscribeVcsStatusRpc, _>(
            environment,
            platform,
            input,
            state,
            apply_status_event,
        )
        .await
    }
    pub async fn follow_subscription<M: t3_contracts::rpc::RpcMethod>(
        &self,
        environment: EnvironmentId,
        platform: &(impl Platform + ?Sized),
        input: M::Payload,
        idle_ttl_ms: u64,
    ) -> Result<(), ClientError>
    where
        M::Success: PartialEq + 'static,
        M::Payload: Clone,
    {
        let state = self.resource::<M>(environment.clone(), &input, idle_ttl_ms)?;
        self.follow::<M, _>(environment, platform, input, state, |_, value| value)
            .await
    }
    pub fn project_clones(
        &self,
        environment: EnvironmentId,
    ) -> Result<
        Resource<<WsSubscribeProjectClonesRpc as t3_contracts::rpc::RpcMethod>::Success>,
        ClientError,
    > {
        self.resource::<WsSubscribeProjectClonesRpc>(
            environment,
            &t3_contracts::project_clone::ProjectCloneSubscribeInput {},
            ResourcePolicy::default().idle_ttl_ms,
        )
    }
    pub async fn follow_project_clones(
        &self,
        environment: EnvironmentId,
        platform: &(impl Platform + ?Sized),
    ) -> Result<(), ClientError> {
        self.follow_subscription::<WsSubscribeProjectClonesRpc>(
            environment,
            platform,
            t3_contracts::project_clone::ProjectCloneSubscribeInput {},
            ResourcePolicy::default().idle_ttl_ms,
        )
        .await
    }
    pub fn worktree_setup(
        &self,
        environment: EnvironmentId,
        input: &<WsSubscribeWorktreeSetupRpc as t3_contracts::rpc::RpcMethod>::Payload,
    ) -> Result<
        Resource<<WsSubscribeWorktreeSetupRpc as t3_contracts::rpc::RpcMethod>::Success>,
        ClientError,
    > {
        self.resource::<WsSubscribeWorktreeSetupRpc>(environment, input, STATUS_IDLE_TTL_MS)
    }
    pub async fn follow_worktree_setup(
        &self,
        environment: EnvironmentId,
        platform: &(impl Platform + ?Sized),
        input: <WsSubscribeWorktreeSetupRpc as t3_contracts::rpc::RpcMethod>::Payload,
    ) -> Result<(), ClientError> {
        self.follow_subscription::<WsSubscribeWorktreeSetupRpc>(
            environment,
            platform,
            input,
            STATUS_IDLE_TTL_MS,
        )
        .await
    }
    async fn follow<M, T>(
        &self,
        environment: EnvironmentId,
        platform: &(impl Platform + ?Sized),
        input: M::Payload,
        resource: Resource<T>,
        reduce: impl Fn(Option<&T>, M::Success) -> T,
    ) -> Result<(), ClientError>
    where
        M: t3_contracts::rpc::RpcMethod,
        M::Payload: Clone,
        T: PartialEq + 'static,
    {
        let key = serde_json::to_string(&(&environment, M::TAG, &input))?;
        let (_owner, abort) = self.claim(&environment, key, &resource);
        let mut guard = ResourceGuard {
            resource: resource.clone(),
            token: None,
        };
        let driver = async {
            let session = self.session(&environment);
            let mut changes = session.observe(platform.monotonic_ms());
            changes.next().await;
            loop {
                let Some(current) = session.snapshot().value.filter(|s| s.endpoint.is_some())
                else {
                    resource.cancel();
                    changes.next().await;
                    continue;
                };
                let endpoint = current.endpoint.as_ref().ok_or(ClientError::Disconnected)?;
                let token = resource.begin();
                guard.token = Some(token);
                let mut stream = endpoint.subscribe::<M>(input.clone());
                let mut value = None;
                loop {
                    match select(changes.next(), stream.next()).await {
                        Either::Left(_) => break,
                        Either::Right((None, _)) => {
                            changes.next().await;
                            break;
                        }
                        Either::Right((Some(Err(error)), _)) => {
                            resource.fail(token, &error, platform.monotonic_ms());
                            changes.next().await;
                            break;
                        }
                        Either::Right((Some(Ok(event)), _)) => {
                            if !resource.is_current(token) {
                                break;
                            }
                            let next = reduce(value.as_deref(), event);
                            resource.finish(token, next, platform.monotonic_ms(), false);
                            value = resource.snapshot().value;
                        }
                    }
                }
            }
        };
        Abortable::new(driver, abort)
            .await
            .unwrap_or(Err(ClientError::Cancelled))
    }
}
struct ResourceGuard<T> {
    resource: Resource<T>,
    token: Option<crate::services::resources::RequestToken>,
}
impl<T> Drop for ResourceGuard<T> {
    fn drop(&mut self) {
        if self
            .token
            .is_some_and(|token| self.resource.is_current(token))
        {
            self.resource.cancel();
        }
    }
}
struct DriverGuard {
    drivers: Rc<RefCell<BTreeMap<String, Driver>>>,
    key: String,
    id: u64,
}
impl Drop for DriverGuard {
    fn drop(&mut self) {
        if self
            .drivers
            .borrow()
            .get(&self.key)
            .is_some_and(|driver| driver.id == self.id)
        {
            self.drivers.borrow_mut().remove(&self.key);
        }
    }
}
