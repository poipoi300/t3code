use crate::sensitive::{Sensitive, ServiceError, ServiceResult};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::{
    sync::{Arc, Weak},
    time::Duration,
};
use t3_contracts::{provider_instance::ProviderInstanceId, provider_setup::*, wire::WireDateTime};
use t3_process::CancellationToken;
use tokio::{
    sync::{Mutex, mpsc, watch},
    time::Instant,
};

pub type AuthState = Sensitive<ProviderAuthState>;

#[derive(Debug, Clone, PartialEq)]
pub struct CredentialBinding {
    pub key: String,
    pub owner: ProviderCredentialOwner,
}

#[derive(Debug)]
pub enum AuthReply {
    Response(Sensitive<ProviderAuthResponse>),
    Callback(Sensitive<String>),
}

#[async_trait]
pub trait AuthBackend: Send + Sync {
    fn default_method_id(&self) -> Option<&str> {
        None
    }
    fn check_start(&self) -> ServiceResult<()> {
        Ok(())
    }
    fn check_logout(&self) -> ServiceResult<()> {
        Ok(())
    }
    fn is_logout_prompt(&self, _text: &str, _has_attachments: bool) -> bool {
        false
    }
    async fn methods(&self) -> ServiceResult<Vec<ProviderAuthMethod>>;
    async fn authenticate(&self, method: &str, context: AuthContext) -> ServiceResult<()>;
    async fn logout(&self) -> ServiceResult<Option<String>>;
    async fn remove_method(&self, _method: &str) -> ServiceResult<()> {
        Err(ServiceError::Unsupported)
    }
    async fn reconnect_profile(
        &self,
        _method: &str,
    ) -> ServiceResult<Option<Sensitive<ChatGptReconnectProfile>>> {
        Err(ServiceError::Unsupported)
    }
    async fn import_profile(
        &self,
        _profile: Sensitive<ChatGptTransferredProfile>,
    ) -> ServiceResult<()> {
        Err(ServiceError::Unsupported)
    }
    async fn validate_profile(
        &self,
        _profile: &Sensitive<ChatGptTransferredProfile>,
    ) -> ServiceResult<()> {
        Err(ServiceError::Unsupported)
    }
}

#[async_trait]
pub trait SessionStopper: Send + Sync {
    async fn stop(
        &self,
        instance: &ProviderInstanceId,
        binding: &CredentialBinding,
    ) -> ServiceResult<()>;
}

struct Published {
    owner: Option<String>,
    state: ProviderAuthState,
}
struct Active {
    id: String,
    owner: String,
    expires: Instant,
    cancel: CancellationToken,
    replies: mpsc::Sender<(AuthReply, tokio::sync::oneshot::Sender<ServiceResult<()>>)>,
    responds: bool,
    completes: bool,
    responding: bool,
}
struct State {
    published: Published,
    active: Option<Active>,
    stopping: bool,
    sessions: Vec<CancellationToken>,
}
struct Inner {
    domain: tokio::sync::RwLock<BindingDomain>,
    state: Mutex<State>,
    changes: watch::Sender<u64>,
    cancel: CancellationToken,
}
#[derive(Default)]
struct BindingState {
    changing: Vec<Weak<Inner>>,
    sessions: Vec<CancellationToken>,
}
type BindingDomain = Arc<Mutex<BindingState>>;
impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct AuthContext {
    inner: Weak<Inner>,
    pub flow_id: String,
    pub expires_at_ms: i64,
    pub return_url: Option<String>,
    pub callback_mode: Option<ProviderAuthCallbackMode>,
    pub cancellation: CancellationToken,
    replies: mpsc::Receiver<(AuthReply, tokio::sync::oneshot::Sender<ServiceResult<()>>)>,
}
impl std::fmt::Debug for AuthContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthContext([REDACTED])")
    }
}
impl AuthContext {
    pub async fn commit(
        &self,
        operation: impl std::future::Future<Output = ServiceResult<()>> + Send + 'static,
    ) -> ServiceResult<()> {
        let inner = self.inner.upgrade().ok_or(ServiceError::Inactive)?;
        let flow = self.flow_id.clone();
        tokio::spawn(async move {
            let mut state = inner.state.lock().await;
            if inner.cancel.is_cancelled()
                || state
                    .active
                    .as_ref()
                    .is_none_or(|active| active.id != flow || active.cancel.is_cancelled())
            {
                return Err(ServiceError::Inactive);
            }
            state.published.state.phase = ProviderAuthPhase::Verifying;
            state.published.state.authorization_url = None;
            state.published.state.interaction = Some(None);
            inner
                .changes
                .send_modify(|value| *value = value.wrapping_add(1));
            let result = operation.await;
            state.active = None;
            state.published.state.phase = if result.is_ok() {
                ProviderAuthPhase::Succeeded
            } else {
                ProviderAuthPhase::Failed
            };
            state.published.state.expires_at = None;
            state.published.state.message = Some(
                result
                    .err()
                    .map_or_else(|| "Sign-in complete.".into(), |error| error.to_string()),
            );
            inner
                .changes
                .send_modify(|value| *value = value.wrapping_add(1));
            result
        })
        .await
        .map_err(|_| ServiceError::Storage)?
    }
    pub async fn interaction(
        &self,
        interaction: ProviderAuthInteraction,
        responds: bool,
        completes: bool,
    ) -> ServiceResult<()> {
        let interaction = serde_json::from_value(
            serde_json::to_value(interaction).map_err(|_| ServiceError::Response)?,
        )
        .map_err(|_| ServiceError::Response)?;
        let Some(inner) = self.inner.upgrade() else {
            return Err(ServiceError::Inactive);
        };
        let mut state = inner.state.lock().await;
        if let Some(active) = state.active.as_mut().filter(|a| a.id == self.flow_id) {
            active.responds = responds;
            active.completes = completes;
            let url = match &interaction {
                ProviderAuthInteraction::Browser { url, .. }
                | ProviderAuthInteraction::DeviceCode { url, .. } => Some(url.0.clone()),
                _ => None,
            };
            state.published.state.phase = ProviderAuthPhase::Waiting;
            state.published.state.authorization_url = url;
            state.published.state.interaction = Some(Some(interaction));
            state.published.state.message = Some("Complete sign-in to continue.".into());
            inner.changes.send_modify(|v| *v = v.wrapping_add(1));
        }
        Ok(())
    }
    pub async fn verifying(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut state = inner.state.lock().await;
        if let Some(active) = state.active.as_mut().filter(|a| a.id == self.flow_id) {
            active.responds = false;
            active.completes = false;
            state.published.state.phase = ProviderAuthPhase::Verifying;
            state.published.state.interaction = Some(None);
            state.published.state.authorization_url = None;
            state.published.state.message = Some("Checking provider sign-in.".into());
            inner.changes.send_modify(|v| *v = v.wrapping_add(1));
        }
    }
    pub async fn message(&self, message: &'static str) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut state = inner.state.lock().await;
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.flow_id)
        {
            state.published.state.message = Some(message.into());
            inner
                .changes
                .send_modify(|version| *version = version.wrapping_add(1));
        }
    }
    pub async fn receive(&mut self) -> ServiceResult<AuthDelivery> {
        tokio::select! {
            _ = self.cancellation.cancelled() => Err(ServiceError::Cancelled),
            reply = self.replies.recv() => reply.map(|(reply, accepted)|AuthDelivery { reply, accepted: Some(accepted) }).ok_or(ServiceError::Cancelled),
        }
    }
}
pub struct AuthDelivery {
    pub reply: AuthReply,
    accepted: Option<tokio::sync::oneshot::Sender<ServiceResult<()>>>,
}
impl AuthDelivery {
    pub fn finish(mut self, result: ServiceResult<()>) {
        if let Some(sender) = self.accepted.take() {
            let _ = sender.send(result);
        }
    }
}

pub struct AuthController {
    instance: ProviderInstanceId,
    pub binding: CredentialBinding,
    backend: Arc<dyn AuthBackend>,
    stopper: Arc<dyn SessionStopper>,
    inner: Arc<Inner>,
    timeout: Duration,
}
impl Drop for AuthController {
    fn drop(&mut self) {
        self.inner.cancel.cancel();
    }
}
impl AuthController {
    pub fn instance_id(&self) -> &ProviderInstanceId {
        &self.instance
    }
    pub async fn new(
        instance: ProviderInstanceId,
        binding: CredentialBinding,
        backend: Arc<dyn AuthBackend>,
        stopper: Arc<dyn SessionStopper>,
        timeout: Duration,
    ) -> Arc<Self> {
        let methods = backend.methods().await.unwrap_or_default();
        let state = ProviderAuthState {
            instance_id: instance.clone(),
            phase: ProviderAuthPhase::Idle,
            flow_id: None,
            authorization_url: None,
            expires_at: None,
            message: None,
            methods: Some(methods),
            interaction: Some(None),
            credential_owner: Some(binding.owner.clone()),
        };
        let (changes, _) = watch::channel(0);
        Arc::new(Self {
            instance,
            binding,
            backend,
            stopper,
            timeout,
            inner: Arc::new(Inner {
                domain: tokio::sync::RwLock::new(Arc::new(Mutex::new(BindingState::default()))),
                state: Mutex::new(State {
                    published: Published { owner: None, state },
                    active: None,
                    stopping: false,
                    sessions: vec![],
                }),
                changes,
                cancel: CancellationToken::new(),
            }),
        })
    }
    pub async fn is_changing(&self) -> bool {
        let state = self.inner.state.lock().await;
        state.stopping || state.active.is_some()
    }
    async fn binding_guard(&self) -> ServiceResult<tokio::sync::OwnedMutexGuard<BindingState>> {
        let domain = self.inner.domain.read().await;
        let mut guard = domain.clone().lock_owned().await;
        guard.changing.retain(|owner| owner.strong_count() > 0);
        for owner in &guard.changing {
            if let Some(other) = owner.upgrade()
                && !Arc::ptr_eq(&other, &self.inner)
                && !other.cancel.is_cancelled()
            {
                let state = other.state.lock().await;
                if state.stopping || state.active.is_some() {
                    return Err(ServiceError::SharedBinding);
                }
            }
        }
        Ok(guard)
    }
    fn claim_binding(&self, domain: &mut BindingState) {
        let owner = Arc::downgrade(&self.inner);
        if !domain.changing.iter().any(|other| other.ptr_eq(&owner)) {
            domain.changing.push(owner);
        }
    }
    async fn stop_binding(inner: &Inner) {
        let domain = inner.domain.read().await;
        for session in domain.lock().await.sessions.drain(..) {
            session.cancel();
        }
    }
    pub async fn snapshot(&self, owner: &str) -> AuthState {
        let state = self.inner.state.lock().await;
        let mut published = state.published.state.clone();
        if state.published.owner.as_ref().is_some_and(|o| o != owner) {
            published.flow_id = None;
            published.authorization_url = None;
            published.expires_at = None;
            published.interaction = Some(None);
            if state.active.is_some() {
                published.message = Some("Sign-in is in progress in another client.".into());
            }
        }
        Sensitive::new(published)
    }
    pub fn subscribe(self: &Arc<Self>, owner: String) -> AuthSubscription {
        AuthSubscription {
            current: DirectSubscription {
                controller: self.clone(),
                owner,
                changes: self.inner.changes.subscribe(),
                initial: true,
            },
            registry: None,
        }
    }
    pub async fn start(
        &self,
        owner: &str,
        method: Option<&str>,
        return_url: Option<String>,
        callback_mode: Option<ProviderAuthCallbackMode>,
        now_ms: i64,
    ) -> ServiceResult<AuthState> {
        if self.inner.cancel.is_cancelled() {
            return Err(ServiceError::Unavailable);
        }
        self.backend.check_start()?;
        let mut domain = self.binding_guard().await?;
        let mut state = self.inner.state.lock().await;
        if state.active.as_ref().is_some_and(|a| a.owner == owner) {
            return Ok(Sensitive::new(state.published.state.clone()));
        }
        if state.stopping || state.active.is_some() {
            return Err(ServiceError::Busy);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let expiry = now_ms.saturating_add(self.timeout.as_millis().min(i64::MAX as u128) as i64);
        let iso = chrono::DateTime::from_timestamp_millis(expiry)
            .ok_or(ServiceError::Expired)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let cancellation = self.inner.cancel.child_token();
        let (tx, rx) = mpsc::channel(1);
        state.active = Some(Active {
            id: id.clone(),
            owner: owner.into(),
            expires: Instant::now() + self.timeout,
            cancel: cancellation.clone(),
            replies: tx,
            responds: false,
            completes: false,
            responding: false,
        });
        self.claim_binding(&mut domain);
        drop(domain);
        state.published.owner = Some(owner.into());
        let snapshot = &mut state.published.state;
        snapshot.phase = ProviderAuthPhase::Starting;
        snapshot.flow_id = Some(id.clone().into());
        snapshot.expires_at = Some(WireDateTime(iso));
        snapshot.message = Some("Starting sign-in.".into());
        snapshot.authorization_url = None;
        snapshot.interaction = Some(None);
        let initial = Sensitive::new(snapshot.clone());
        self.inner.changes.send_modify(|v| *v = v.wrapping_add(1));
        let context = AuthContext {
            inner: Arc::downgrade(&self.inner),
            flow_id: id.clone(),
            expires_at_ms: expiry,
            return_url,
            callback_mode,
            cancellation: cancellation.clone(),
            replies: rx,
        };
        let backend = self.backend.clone();
        let stopper = self.stopper.clone();
        let instance = self.instance.clone();
        let binding = self.binding.clone();
        let method = method.map(str::to_owned);
        let timeout = self.timeout;
        let inner = Arc::downgrade(&self.inner);
        let stop_inner = inner.clone();
        tokio::spawn(async move {
            let work = async {
                let methods = backend.methods().await?;
                let method = method
                    .or_else(|| backend.default_method_id().map(str::to_owned))
                    .or_else(|| methods.first().map(|m| m.id.0.clone()))
                    .ok_or(ServiceError::Method)?;
                if !methods.iter().any(|m| m.id.0 == method) {
                    return Err(ServiceError::Method);
                }
                let stopped = stopper.stop(&instance, &binding).await;
                if let Some(inner) = stop_inner.upgrade() {
                    Self::stop_binding(&inner).await;
                    Self::stop_owned(&mut *inner.state.lock().await);
                }
                stopped?;
                backend.authenticate(&method, context).await
            };
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return,
                result = tokio::time::timeout(timeout, work) => result.unwrap_or(Err(ServiceError::Expired)),
            };
            let methods = backend.methods().await.ok();
            if let Some(inner) = inner.upgrade() {
                let mut state = inner.state.lock().await;
                if state.active.as_ref().is_some_and(|a| a.id == id) {
                    state.active = None;
                    let published = &mut state.published.state;
                    published.phase = if result.is_ok() {
                        ProviderAuthPhase::Succeeded
                    } else {
                        ProviderAuthPhase::Failed
                    };
                    published.authorization_url = None;
                    published.interaction = Some(None);
                    published.expires_at = None;
                    if let Some(methods) = methods {
                        published.methods = Some(methods);
                    }
                    published.message = Some(
                        result
                            .err()
                            .map_or_else(|| "Sign-in complete.".into(), |e| e.to_string()),
                    );
                    inner.changes.send_modify(|v| *v = v.wrapping_add(1));
                }
            }
        });
        Ok(initial)
    }
    async fn deliver(
        &self,
        owner: &str,
        flow: &str,
        interaction: Option<(&str, &ProviderAuthResponse)>,
        reply: AuthReply,
    ) -> ServiceResult<AuthState> {
        let (tx, done) = tokio::sync::oneshot::channel();
        {
            let mut state = self.inner.state.lock().await;
            let active = state
                .active
                .as_ref()
                .filter(|a| a.id == flow && a.owner == owner && a.expires > Instant::now())
                .ok_or(ServiceError::Inactive)?;
            if active.responding {
                return Err(ServiceError::ResponseBusy);
            }
            if let Some((id, response)) = interaction {
                let expected = state
                    .published
                    .state
                    .interaction
                    .as_ref()
                    .and_then(Option::as_ref)
                    .ok_or(ServiceError::Interaction)?;
                let matches = match (expected, response) {
                    (
                        ProviderAuthInteraction::Browser { id: current, .. },
                        ProviderAuthResponse::Browser { .. },
                    )
                    | (
                        ProviderAuthInteraction::Terminal { id: current, .. },
                        ProviderAuthResponse::Terminal { .. },
                    )
                    | (
                        ProviderAuthInteraction::Credentials { id: current, .. },
                        ProviderAuthResponse::Credentials { .. },
                    ) => current.0 == id,
                    _ => false,
                };
                if !matches || !active.responds {
                    return Err(ServiceError::Interaction);
                }
            } else if !active.completes {
                return Err(ServiceError::Interaction);
            }
            let active = state.active.as_mut().ok_or(ServiceError::Inactive)?;
            active
                .replies
                .try_send((reply, tx))
                .map_err(|_| ServiceError::ResponseBusy)?;
            active.responding = true;
        }
        let inner = self.inner.clone();
        let flow = flow.to_owned();
        let result = tokio::spawn(async move {
            let result = done.await.unwrap_or(Err(ServiceError::Inactive));
            let mut state = inner.state.lock().await;
            if let Some(active) = state.active.as_mut().filter(|a| a.id == flow) {
                active.responding = false;
                inner.changes.send_modify(|v| *v = v.wrapping_add(1));
            }
            result
        })
        .await
        .map_err(|_| ServiceError::Inactive)?;
        result?;
        Ok(self.snapshot(owner).await)
    }
    pub async fn respond(
        &self,
        owner: &str,
        input: ProviderAuthRespondInput,
    ) -> ServiceResult<AuthState> {
        let response = input.response;
        self.deliver(
            owner,
            &input.flow_id.0,
            Some((&input.interaction_id.0, &response)),
            AuthReply::Response(Sensitive::new(response.clone())),
        )
        .await
    }
    pub async fn complete(
        &self,
        owner: &str,
        input: ProviderAuthCompleteInput,
    ) -> ServiceResult<AuthState> {
        self.deliver(
            owner,
            &input.flow_id.0,
            None,
            AuthReply::Callback(Sensitive::new(input.callback_url.0)),
        )
        .await
    }
    pub async fn cancel(&self, owner: &str, flow: &str) -> ServiceResult<AuthState> {
        let mut state = self.inner.state.lock().await;
        let active = state
            .active
            .as_ref()
            .filter(|a| a.id == flow && a.owner == owner && a.expires > Instant::now())
            .ok_or(ServiceError::Inactive)?;
        active.cancel.cancel();
        state.active = None;
        let published = &mut state.published.state;
        published.phase = ProviderAuthPhase::Cancelled;
        published.authorization_url = None;
        published.interaction = Some(None);
        published.expires_at = None;
        published.message = Some("Sign-in cancelled.".into());
        let result = Sensitive::new(published.clone());
        self.inner.changes.send_modify(|v| *v = v.wrapping_add(1));
        Ok(result)
    }
    pub async fn logout(self: &Arc<Self>) -> ServiceResult<AuthState> {
        let controller = self.clone();
        tokio::spawn(async move { controller.logout_inner().await })
            .await
            .map_err(|_| ServiceError::Storage)?
    }
    async fn logout_inner(&self) -> ServiceResult<AuthState> {
        self.backend.check_logout()?;
        let mut domain = self.binding_guard().await?;
        {
            let mut state = self.inner.state.lock().await;
            if state.stopping {
                return Err(ServiceError::Busy);
            }
            state.stopping = true;
            self.claim_binding(&mut domain);
            if let Some(active) = state.active.take() {
                active.cancel.cancel();
            }
        }
        drop(domain);
        let stopped = self.stopper.stop(&self.instance, &self.binding).await;
        Self::stop_binding(&self.inner).await;
        Self::stop_owned(&mut *self.inner.state.lock().await);
        let result = async {
            stopped?;
            self.backend.logout().await
        }
        .await;
        let methods = self.backend.methods().await.ok();
        let mut state = self.inner.state.lock().await;
        state.stopping = false;
        state.published.owner = None;
        let published = &mut state.published.state;
        published.phase = if result.is_ok() {
            ProviderAuthPhase::Idle
        } else {
            ProviderAuthPhase::Failed
        };
        published.flow_id = None;
        published.authorization_url = None;
        published.expires_at = None;
        published.interaction = Some(None);
        if let Some(methods) = methods {
            published.methods = Some(methods);
        }
        published.message = Some(match &result {
            Ok(message) => message.clone().unwrap_or_else(|| "Signed out.".into()),
            Err(_) => "Could not sign out. Try again.".into(),
        });
        self.inner.changes.send_modify(|v| *v = v.wrapping_add(1));
        result?;
        Ok(Sensitive::new(published.clone()))
    }
    pub async fn remove_method(self: &Arc<Self>, method: &str) -> ServiceResult<AuthState> {
        let controller = self.clone();
        let method = method.to_owned();
        tokio::spawn(async move {
            {
                let mut domain = controller.binding_guard().await?;
                let mut state = controller.inner.state.lock().await;
                if state.stopping || state.active.is_some() {
                    return Err(ServiceError::Busy);
                }
                state.stopping = true;
                controller.claim_binding(&mut domain);
            }
            let result = controller.backend.remove_method(&method).await;
            let methods = controller.backend.methods().await.ok();
            let mut state = controller.inner.state.lock().await;
            state.stopping = false;
            if let Some(methods) = methods {
                state.published.state.methods = Some(methods);
            }
            controller
                .inner
                .changes
                .send_modify(|v| *v = v.wrapping_add(1));
            result?;
            Ok(Sensitive::new(state.published.state.clone()))
        })
        .await
        .map_err(|_| ServiceError::Storage)?
    }
    pub async fn invalidate(&self) {
        let mut state = self.inner.state.lock().await;
        if state.active.is_some() || state.stopping {
            return;
        }
        Self::stop_owned(&mut state);
        state.published.owner = None;
        state.published.state.phase = ProviderAuthPhase::Idle;
        state.published.state.flow_id = None;
        state.published.state.message = Some("This provider's shared sign-in changed.".into());
        self.inner.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
    pub fn close(&self) {
        self.inner.cancel.cancel();
    }
    fn stop_owned(state: &mut State) {
        for session in state.sessions.drain(..) {
            session.cancel();
        }
    }
    pub async fn admit_access(&self) -> ServiceResult<CredentialAccess> {
        if self.inner.cancel.is_cancelled() {
            return Err(ServiceError::Unavailable);
        }
        let mut domain = self.binding_guard().await?;
        let mut state = self.inner.state.lock().await;
        if state.active.is_some() || state.stopping {
            return Err(ServiceError::Busy);
        }
        let cancellation = CancellationToken::new();
        state.sessions.retain(|s| !s.is_cancelled());
        state.sessions.push(cancellation.clone());
        domain.sessions.retain(|s| !s.is_cancelled());
        domain.sessions.push(cancellation.clone());
        Ok(CredentialAccess { cancellation })
    }
    pub async fn adopt_credentials(
        self: &Arc<Self>,
        profile: Sensitive<ChatGptTransferredProfile>,
    ) -> ServiceResult<AuthState> {
        let controller = self.clone();
        tokio::spawn(async move {
            controller.backend.validate_profile(&profile).await?;
            let mut domain = controller.binding_guard().await?;
            {
                let mut state = controller.inner.state.lock().await;
                if state.active.is_some() || state.stopping {
                    return Err(ServiceError::Busy);
                }
                state.stopping = true;
                controller.claim_binding(&mut domain);
                Self::stop_owned(&mut state);
            }
            drop(domain);
            let result = async {
                let stopped = controller
                    .stopper
                    .stop(&controller.instance, &controller.binding)
                    .await;
                Self::stop_binding(&controller.inner).await;
                stopped?;
                controller.backend.import_profile(profile).await
            }
            .await;
            let methods = controller.backend.methods().await.ok();
            let mut state = controller.inner.state.lock().await;
            state.stopping = false;
            state.published.owner = None;
            let published = &mut state.published.state;
            published.phase = if result.is_ok() {
                ProviderAuthPhase::Succeeded
            } else {
                ProviderAuthPhase::Failed
            };
            published.flow_id = None;
            published.authorization_url = None;
            published.interaction = Some(None);
            published.expires_at = None;
            if let Some(methods) = methods {
                published.methods = Some(methods);
            }
            published.message = result.err().map(|e| e.to_string());
            controller
                .inner
                .changes
                .send_modify(|v| *v = v.wrapping_add(1));
            result?;
            Ok(Sensitive::new(published.clone()))
        })
        .await
        .map_err(|_| ServiceError::Storage)?
    }
}

#[derive(Debug)]
pub struct CredentialAccess {
    pub cancellation: CancellationToken,
}
impl Drop for CredentialAccess {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

struct DirectSubscription {
    controller: Arc<AuthController>,
    owner: String,
    changes: watch::Receiver<u64>,
    initial: bool,
}
impl DirectSubscription {
    async fn next(&mut self) -> Option<AuthState> {
        if self.controller.inner.cancel.is_cancelled() {
            return None;
        }
        if self.initial {
            self.initial = false;
        } else {
            tokio::select! {_=self.controller.inner.cancel.cancelled()=>return None,changed=self.changes.changed()=>changed.ok()?}
        }
        Some(self.controller.snapshot(&self.owner).await)
    }
}
struct RegistrySubscription {
    controllers: Arc<tokio::sync::RwLock<BTreeMap<ProviderInstanceId, Arc<AuthController>>>>,
    changes: watch::Receiver<u64>,
    instance: ProviderInstanceId,
}
pub struct AuthSubscription {
    current: DirectSubscription,
    registry: Option<RegistrySubscription>,
}
impl AuthSubscription {
    pub async fn next(&mut self) -> Option<AuthState> {
        let Some(registry) = self.registry.as_mut() else {
            return self.current.next().await;
        };
        loop {
            tokio::select! {biased;
                changed=registry.changes.changed()=>{
                    changed.ok()?;
                    let controller=registry.controllers.read().await.get(&registry.instance).cloned()?;
                    self.current=DirectSubscription {changes:controller.inner.changes.subscribe(),controller,owner:self.current.owner.clone(),initial:true};
                }
                value=self.current.next()=>{
                    let controllers = registry.controllers.read().await;
                    let controller = controllers.get(&registry.instance)?.clone();
                    if !Arc::ptr_eq(&controller, &self.current.controller) {
                        self.current = DirectSubscription {
                            changes: controller.inner.changes.subscribe(), controller,
                            owner: self.current.owner.clone(), initial: true,
                        };
                        continue;
                    }
                    if registry.changes.has_changed().unwrap_or(false) { continue; }
                    return value;
                }
            }
        }
    }
}

pub struct AuthService {
    controllers: Arc<tokio::sync::RwLock<BTreeMap<ProviderInstanceId, Arc<AuthController>>>>,
    changes: watch::Sender<u64>,
    credential_changes: Mutex<()>,
}
impl Default for AuthService {
    fn default() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            controllers: Arc::new(tokio::sync::RwLock::new(BTreeMap::new())),
            changes,
            credential_changes: Mutex::new(()),
        }
    }
}
impl AuthService {
    pub async fn try_handle_prompt_command(
        &self,
        id: &ProviderInstanceId,
        text: &str,
        has_attachments: bool,
    ) -> ServiceResult<bool> {
        let Ok(controller) = self.controller(id).await else {
            return Ok(false);
        };
        if !controller.backend.is_logout_prompt(text, has_attachments) {
            return Ok(false);
        }
        let _gate = self.credential_changes.lock().await;
        let current = self.controller(id).await?;
        if !current.backend.is_logout_prompt(text, has_attachments) {
            return Ok(false);
        }
        self.check_binding(&current).await?;
        current.logout().await?;
        Ok(true)
    }
    pub async fn register(&self, id: ProviderInstanceId, controller: Arc<AuthController>) {
        let mut controllers = self.controllers.write().await;
        if let Some(other) = controllers
            .values()
            .find(|other| other.binding == controller.binding)
        {
            let domain = other.inner.domain.read().await.clone();
            let mut current = controller.inner.domain.write().await;
            if !Arc::ptr_eq(&current, &domain) {
                let mut previous = current.lock().await;
                let mut shared = domain.lock().await;
                shared.sessions.append(&mut previous.sessions);
                shared.changing.append(&mut previous.changing);
                drop(shared);
                drop(previous);
                *current = domain.clone();
            }
        }
        if let Some(old) = controllers.insert(id, controller.clone())
            && !Arc::ptr_eq(&old, &controller)
        {
            old.close();
        }
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
    pub async fn unregister(&self, id: &ProviderInstanceId) {
        if let Some(old) = self.controllers.write().await.remove(id) {
            old.close();
        }
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
    pub async fn close(&self) {
        let controllers = std::mem::take(&mut *self.controllers.write().await);
        for controller in controllers.into_values() {
            controller.close();
        }
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
    async fn controller(&self, id: &ProviderInstanceId) -> ServiceResult<Arc<AuthController>> {
        self.controllers
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or(ServiceError::Unsupported)
    }
    async fn check_binding(&self, current: &AuthController) -> ServiceResult<()> {
        for (id, other) in self.controllers.read().await.iter() {
            if *id != current.instance
                && other.binding == current.binding
                && other.is_changing().await
            {
                return Err(ServiceError::SharedBinding);
            }
        }
        Ok(())
    }
    pub async fn start(
        &self,
        input: ProviderAuthStartInput,
        owner: &str,
        now_ms: i64,
    ) -> ServiceResult<AuthState> {
        let _gate = self.credential_changes.lock().await;
        let auth = self.controller(&input.instance_id).await?;
        self.check_binding(&auth).await?;
        auth.start(
            owner,
            input.method_id.as_ref().map(|s| s.0.as_str()),
            input.return_url,
            input.callback_mode,
            now_ms,
        )
        .await
    }
    pub async fn respond(
        &self,
        input: ProviderAuthRespondInput,
        owner: &str,
    ) -> ServiceResult<AuthState> {
        self.controller(&input.instance_id)
            .await?
            .respond(owner, input)
            .await
    }
    pub async fn complete(
        &self,
        input: ProviderAuthCompleteInput,
        owner: &str,
    ) -> ServiceResult<AuthState> {
        self.controller(&input.instance_id)
            .await?
            .complete(owner, input)
            .await
    }
    pub async fn cancel(
        &self,
        input: ProviderAuthCancelInput,
        owner: &str,
    ) -> ServiceResult<AuthState> {
        self.controller(&input.instance_id)
            .await?
            .cancel(owner, &input.flow_id.0)
            .await
    }
    pub async fn logout(&self, input: ProviderSetupInput) -> ServiceResult<AuthState> {
        let _gate = self.credential_changes.lock().await;
        let auth = self.controller(&input.instance_id).await?;
        self.check_binding(&auth).await?;
        let result = auth.logout().await?;
        for (id, other) in self.controllers.read().await.iter() {
            if *id != input.instance_id && other.binding == auth.binding {
                other.invalidate().await;
            }
        }
        Ok(result)
    }
    pub async fn remove_method(
        &self,
        input: ProviderAuthRemoveMethodInput,
    ) -> ServiceResult<AuthState> {
        let _gate = self.credential_changes.lock().await;
        let auth = self.controller(&input.instance_id).await?;
        self.check_binding(&auth).await?;
        auth.remove_method(&input.method_id.0).await
    }
    pub async fn subscribe(
        &self,
        input: ProviderSetupInput,
        owner: String,
    ) -> ServiceResult<AuthSubscription> {
        let changes = self.changes.subscribe();
        let mut subscription = self.controller(&input.instance_id).await?.subscribe(owner);
        subscription.registry = Some(RegistrySubscription {
            controllers: self.controllers.clone(),
            changes,
            instance: input.instance_id,
        });
        Ok(subscription)
    }
    pub async fn reconnect_profile(
        &self,
        input: ChatGptReconnectProfileInput,
    ) -> ServiceResult<Option<Sensitive<ChatGptReconnectProfile>>> {
        self.controller(&input.instance_id)
            .await?
            .backend
            .reconnect_profile(&input.method_id)
            .await
    }
    pub async fn import_profile(
        &self,
        input: ChatGptImportProfileInput,
    ) -> ServiceResult<AuthState> {
        let _gate = self.credential_changes.lock().await;
        let auth = self.controller(&input.instance_id).await?;
        self.check_binding(&auth).await?;
        auth.adopt_credentials(Sensitive::new(input.profile)).await
    }
}
