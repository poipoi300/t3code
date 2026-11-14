use crate::{sensitive::Sensitive, transport::ProviderClock};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use t3_contracts::{
    acp_registry::{
        AcpRegistryAcceptUrlAuthInput, AcpRegistryProbeModel, AcpRegistryUrlAuthAction,
    },
    model::ProviderOptionDescriptor,
    provider_instance::ProviderInstanceId,
    server::{ServerProviderSkill, ServerProviderSlashCommand},
};
use t3_process::CancellationToken;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};

#[derive(Clone, Debug, PartialEq)]
pub struct AvailableCommands {
    pub slash_commands: Vec<ServerProviderSlashCommand>,
    pub skills: Vec<ServerProviderSkill>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LiveConfiguration {
    pub models: Vec<AcpRegistryProbeModel>,
    pub current_model_id: Option<String>,
    pub config_options: Vec<ProviderOptionDescriptor>,
    pub model_option_ids: Option<Vec<String>>,
}
struct Publication<T> {
    current: Option<Arc<T>>,
    listeners: Vec<mpsc::UnboundedSender<Option<Arc<T>>>>,
}
impl<T> Default for Publication<T> {
    fn default() -> Self {
        Self {
            current: None,
            listeners: vec![],
        }
    }
}
impl<T> Publication<T> {
    fn publish(&mut self, value: Option<T>) {
        self.current = value.map(Arc::new);
        self.listeners
            .retain(|listener| listener.send(self.current.clone()).is_ok());
    }
    fn subscribe(&mut self) -> mpsc::UnboundedReceiver<Option<Arc<T>>> {
        let (send, receive) = mpsc::unbounded_channel();
        if self.current.is_some() {
            let _ = send.send(self.current.clone());
        }
        self.listeners.push(send);
        receive
    }
}
struct Foreground {
    count: usize,
    started: CancellationToken,
}
struct Pending {
    action: Sensitive<AcpRegistryUrlAuthAction>,
    consent: Option<oneshot::Sender<bool>>,
    expires: i64,
    identity: Arc<()>,
}
#[derive(Default)]
struct State {
    foreground: BTreeMap<String, Foreground>,
    commands: BTreeMap<ProviderInstanceId, Publication<AvailableCommands>>,
    configuration: BTreeMap<ProviderInstanceId, Publication<LiveConfiguration>>,
    actions: BTreeMap<ProviderInstanceId, Publication<Sensitive<AcpRegistryUrlAuthAction>>>,
    pending: BTreeMap<ProviderInstanceId, Pending>,
}
struct Inner {
    state: Mutex<State>,
    clock: Arc<dyn ProviderClock>,
}
impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
pub struct ForegroundStartup {
    inner: Arc<Inner>,
    agent: String,
}
impl Drop for ForegroundStartup {
    fn drop(&mut self) {
        let mut state = self.inner.state();
        if let Some(active) = state.foreground.get_mut(&self.agent) {
            active.count = active.count.saturating_sub(1);
            if active.count == 0 {
                state.foreground.remove(&self.agent);
            }
        }
    }
}
struct AuthRequest {
    inner: Arc<Inner>,
    instance: ProviderInstanceId,
    identity: Arc<()>,
}
impl Drop for AuthRequest {
    fn drop(&mut self) {
        let mut state = self.inner.state();
        if state
            .pending
            .get(&self.instance)
            .is_some_and(|pending| Arc::ptr_eq(&pending.identity, &self.identity))
        {
            state.pending.remove(&self.instance);
            state
                .actions
                .entry(self.instance.clone())
                .or_default()
                .publish(None);
        }
    }
}
pub struct AcpRuntimeCoordinator {
    inner: Arc<Inner>,
    pub session_mutation: Arc<AsyncMutex<()>>,
}
impl AcpRuntimeCoordinator {
    pub fn new(clock: Arc<dyn ProviderClock>) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                clock,
            }),
            session_mutation: Arc::new(AsyncMutex::new(())),
        }
    }
    pub fn foreground_startup(&self, agent: &str) -> ForegroundStartup {
        let mut state = self.inner.state();
        let active = state
            .foreground
            .entry(agent.into())
            .or_insert_with(|| Foreground {
                count: 0,
                started: CancellationToken::new(),
            });
        active.count += 1;
        active.started.cancel();
        ForegroundStartup {
            inner: self.inner.clone(),
            agent: agent.into(),
        }
    }
    pub async fn background_probe<T, E>(
        &self,
        agent: &str,
        probe: impl Future<Output = Result<T, E>>,
    ) -> Result<Option<T>, E> {
        let started = {
            let mut state = self.inner.state();
            let active = state
                .foreground
                .entry(agent.into())
                .or_insert_with(|| Foreground {
                    count: 0,
                    started: CancellationToken::new(),
                });
            if active.count > 0 {
                return Ok(None);
            }
            if active.started.is_cancelled() {
                active.started = CancellationToken::new();
            }
            active.started.clone()
        };
        tokio::select! {biased;_=started.cancelled()=>Ok(None),result=probe=>result.map(Some)}
    }
    pub fn publish_commands(&self, instance: ProviderInstanceId, commands: AvailableCommands) {
        self.inner
            .state()
            .commands
            .entry(instance)
            .or_default()
            .publish(Some(commands));
    }
    pub fn commands(&self, instance: &ProviderInstanceId) -> Option<Arc<AvailableCommands>> {
        self.inner.state().commands.get(instance)?.current.clone()
    }
    pub fn clear_commands(&self, instance: &ProviderInstanceId) {
        if let Some(value) = self.inner.state().commands.get_mut(instance) {
            value.current = None;
        }
    }
    pub fn subscribe_commands(
        &self,
        instance: ProviderInstanceId,
    ) -> mpsc::UnboundedReceiver<Option<Arc<AvailableCommands>>> {
        self.inner
            .state()
            .commands
            .entry(instance)
            .or_default()
            .subscribe()
    }
    pub fn publish_configuration(
        &self,
        instance: ProviderInstanceId,
        configuration: LiveConfiguration,
    ) {
        self.inner
            .state()
            .configuration
            .entry(instance)
            .or_default()
            .publish(Some(configuration));
    }
    pub fn configuration(&self, instance: &ProviderInstanceId) -> Option<Arc<LiveConfiguration>> {
        self.inner
            .state()
            .configuration
            .get(instance)?
            .current
            .clone()
    }
    pub fn clear_configuration(&self, instance: &ProviderInstanceId) {
        if let Some(value) = self.inner.state().configuration.get_mut(instance) {
            value.current = None;
        }
    }
    pub fn subscribe_configuration(
        &self,
        instance: ProviderInstanceId,
    ) -> mpsc::UnboundedReceiver<Option<Arc<LiveConfiguration>>> {
        self.inner
            .state()
            .configuration
            .entry(instance)
            .or_default()
            .subscribe()
    }
    pub fn url_auth_action(
        &self,
        instance: &ProviderInstanceId,
    ) -> Option<Sensitive<AcpRegistryUrlAuthAction>> {
        self.inner
            .state()
            .pending
            .get(instance)
            .map(|pending| pending.action.clone())
    }
    pub fn subscribe_url_auth(
        &self,
        instance: ProviderInstanceId,
    ) -> mpsc::UnboundedReceiver<Option<Arc<Sensitive<AcpRegistryUrlAuthAction>>>> {
        self.inner
            .state()
            .actions
            .entry(instance)
            .or_default()
            .subscribe()
    }
    pub async fn request_url_auth(
        &self,
        instance: ProviderInstanceId,
        action: Sensitive<AcpRegistryUrlAuthAction>,
    ) -> bool {
        let identity = Arc::new(());
        let (receive, _guard) = {
            let now = self.inner.clock.now_ms();
            let expires = now.saturating_add(600_000);
            let mut action = action.into_inner();
            let date = |ms| {
                chrono::DateTime::from_timestamp_millis(ms)
                    .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            };
            action.created_at = date(now);
            action.expires_at = date(expires);
            let action = Sensitive::new(action);
            let (send, receive) = oneshot::channel();
            let mut state = self.inner.state();
            if let Some(mut previous) = state.pending.insert(
                instance.clone(),
                Pending {
                    action: action.clone(),
                    consent: Some(send),
                    expires,
                    identity: identity.clone(),
                },
            ) && let Some(consent) = previous.consent.take()
            {
                let _ = consent.send(false);
            }
            state
                .actions
                .entry(instance.clone())
                .or_default()
                .publish(Some(action));
            (
                receive,
                AuthRequest {
                    inner: self.inner.clone(),
                    instance,
                    identity,
                },
            )
        };
        matches!(
            tokio::time::timeout(Duration::from_secs(600), receive).await,
            Ok(Ok(true))
        )
    }
    pub fn accept_url_auth(&self, input: &AcpRegistryAcceptUrlAuthInput) -> bool {
        let mut state = self.inner.state();
        let Some(pending) = state
            .pending
            .get_mut(&input.instance_id)
            .filter(|pending| pending.action.expose().elicitation_id == input.elicitation_id)
        else {
            return false;
        };
        let accepted = self.inner.clock.now_ms() < pending.expires;
        let Some(consent) = pending.consent.take() else {
            return false;
        };
        let _ = consent.send(accepted);
        accepted
    }
}
