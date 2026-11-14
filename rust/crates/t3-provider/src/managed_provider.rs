use crate::{
    maintenance::MaintenanceCapabilities,
    sensitive::{ServiceError, ServiceResult},
    service::ProviderInstanceService,
    usage,
};
use async_trait::async_trait;
use std::{
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Duration,
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    provider_instance::ProviderInstanceId,
    provider_usage_limits::{
        ProviderUsageLimitsUpdate, ServerProviderUsageLimits, ServerProviderUsageUnavailableReason,
        ServerProviderUsageWindow,
    },
    server::{ServerProvider, ServerProviderAuthStatus, ServerProviderState},
    wire::WireDateTime,
};
use tokio::sync::{Mutex, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackgroundPolicy {
    pub server_ready: bool,
    pub demanded: bool,
    pub interval: Duration,
}
impl Default for BackgroundPolicy {
    fn default() -> Self {
        Self {
            server_ready: false,
            demanded: false,
            interval: Duration::from_secs(60),
        }
    }
}
#[async_trait]
pub trait ManagedUsage: Send + Sync {
    async fn invalidate(&self) -> ServiceResult<()>;
    async fn check(&self) -> ServiceResult<Option<ServerProviderUsageLimits>>;
}
pub struct ManagedOptions {
    pub background: watch::Receiver<BackgroundPolicy>,
    pub refresh_on_interval: fn(&ServerProvider) -> bool,
    pub usage: Option<Arc<dyn ManagedUsage>>,
}
impl ManagedOptions {
    pub fn new(background: watch::Receiver<BackgroundPolicy>) -> Self {
        Self {
            background,
            refresh_on_interval: |_| true,
            usage: None,
        }
    }
}
pub struct RuntimeUsageUpdate {
    pub windows: Vec<ServerProviderUsageWindow>,
    pub checked_at: WireDateTime,
    pub complete: bool,
    pub auth_label: Option<TrimmedNonEmptyString>,
}
struct State {
    snapshot: ServerProvider,
    generation: u64,
    usage_revision: u64,
    usage_label: Option<Option<TrimmedNonEmptyString>>,
    checked: bool,
    recheck: bool,
}
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
type UsageQueue = Arc<StdMutex<Option<Arc<()>>>>;
struct Claim {
    queue: UsageQueue,
    identity: Arc<()>,
}
impl Claim {
    fn release(&self) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &self.identity))
        {
            *queue = None;
        }
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct ManagedProvider {
    core: Arc<Core>,
    tasks: StdMutex<Vec<Task>>,
}
struct Core {
    source: Arc<dyn ProviderInstanceService>,
    state: Mutex<State>,
    refresh_gate: Mutex<()>,
    usage_queue: UsageQueue,
    usage: Option<Arc<dyn ManagedUsage>>,
    changes: watch::Sender<Option<ServerProvider>>,
}
impl ManagedProvider {
    pub fn new(
        source: Arc<dyn ProviderInstanceService>,
        initial: ServerProvider,
        options: ManagedOptions,
    ) -> ServiceResult<Arc<Self>> {
        if initial.instance_id != *source.instance_id() {
            return Err(ServiceError::Response);
        }
        let (changes, _) = watch::channel(Some(initial.clone()));
        let mut source_changes = source.subscribe_snapshots();
        let core = Arc::new(Core {
            source,
            state: Mutex::new(State {
                snapshot: initial,
                generation: 0,
                usage_revision: 0,
                usage_label: None,
                checked: false,
                recheck: false,
            }),
            refresh_gate: Mutex::new(()),
            usage_queue: Arc::new(StdMutex::new(None)),
            usage: options.usage,
            changes,
        });
        let provider = Arc::new(Self {
            core,
            tasks: StdMutex::new(vec![]),
        });
        let weak = Arc::downgrade(&provider.core);
        let worker = tokio::spawn(Core::background(
            weak,
            options.background,
            options.refresh_on_interval,
        ));
        let mut tasks = provider
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks.push(Task(worker));
        if let Some(mut changes) = source_changes.take() {
            let weak = Arc::downgrade(&provider.core);
            tasks.push(Task(tokio::spawn(async move {
                loop {
                    let value = changes.borrow_and_update().clone();
                    if let Some(value) = value {
                        let Some(provider) = weak.upgrade() else {
                            break;
                        };
                        let generation = provider.generation().await;
                        let _ = provider.publish_enrichment(generation, value).await;
                    }
                    if changes.changed().await.is_err() {
                        break;
                    }
                }
            })));
        }
        drop(tasks);
        Ok(provider)
    }
    pub async fn current(&self) -> ServerProvider {
        self.core.current().await
    }
    pub async fn is_awaiting_first_check(&self) -> bool {
        self.core.is_awaiting_first_check().await
    }
    pub async fn generation(&self) -> u64 {
        self.core.generation().await
    }
    pub async fn publish_enrichment(&self, generation: u64, snapshot: ServerProvider) -> bool {
        self.core.publish_enrichment(generation, snapshot).await
    }
    pub async fn settings_applied(
        &self,
        changed: bool,
        recheck: bool,
    ) -> ServiceResult<ServerProvider> {
        self.core.settings_applied(changed, recheck).await
    }
    pub async fn apply_usage(&self, update: RuntimeUsageUpdate) {
        self.core.apply_usage(update).await
    }
    pub async fn refresh_usage(&self) -> ServiceResult<()> {
        self.core.refresh_usage().await
    }
    pub async fn recheck_on_auth(
        &self,
        mut changes: crate::auth::AuthSubscription,
        mut background: watch::Receiver<BackgroundPolicy>,
    ) {
        let _ = changes.next().await;
        let weak = Arc::downgrade(&self.core);
        let task = tokio::spawn(async move {
            while let Some(state) = changes.next().await {
                use t3_contracts::provider_setup::ProviderAuthPhase;
                if !matches!(
                    state.expose().phase,
                    ProviderAuthPhase::Idle
                        | ProviderAuthPhase::Succeeded
                        | ProviderAuthPhase::Failed
                        | ProviderAuthPhase::Cancelled
                ) {
                    continue;
                }
                while !background.borrow_and_update().server_ready {
                    if background.changed().await.is_err() {
                        return;
                    }
                }
                let Some(provider) = weak.upgrade() else {
                    return;
                };
                let _ = provider.check(None, false, false).await;
            }
        });
        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Task(task));
    }
}
impl Core {
    async fn background(
        weak: Weak<Self>,
        mut policy: watch::Receiver<BackgroundPolicy>,
        refresh: fn(&ServerProvider) -> bool,
    ) {
        let mut initialized = false;
        loop {
            let current = *policy.borrow_and_update();
            if current.server_ready && !initialized {
                let Some(provider) = weak.upgrade() else {
                    return;
                };
                let _ = provider.check(None, false, false).await;
                initialized = true;
            }
            let Some(provider) = weak.upgrade() else {
                return;
            };
            let retry = std::mem::take(&mut provider.state.lock().await.recheck);
            drop(provider);
            let interval = if retry {
                Duration::from_secs(30)
            } else {
                current.interval
            };
            let delay = if interval.is_zero() {
                Duration::from_secs(60)
            } else {
                interval
            };
            tokio::select! {
                changed=policy.changed()=>{if changed.is_err(){return;}}
                _=tokio::time::sleep(delay)=>{
                    if !current.server_ready||!current.demanded||interval.is_zero(){continue;}
                    let Some(provider)=weak.upgrade()else{return;};
                    let snapshot=provider.current().await;
                    if !snapshot.enabled{continue;}
                    if provider.needs_usage_recovery(&snapshot){let _=provider.recover_usage().await;}else if refresh(&snapshot){let _=provider.check(None,false,!retry).await;}
                }
            }
        }
    }
    pub async fn current(&self) -> ServerProvider {
        self.state.lock().await.snapshot.clone()
    }
    pub async fn is_awaiting_first_check(&self) -> bool {
        !self.state.lock().await.checked
    }
    pub async fn generation(&self) -> u64 {
        self.state.lock().await.generation
    }
    fn publish(&self, state: &State) {
        self.changes.send_replace(Some(state.snapshot.clone()));
    }
    pub async fn publish_enrichment(&self, generation: u64, mut next: ServerProvider) -> bool {
        let mut state = self.state.lock().await;
        if generation != state.generation || next.instance_id != state.snapshot.instance_id {
            return false;
        }
        next.usage_limits = state.snapshot.usage_limits.clone();
        if let Some(label) = &state.usage_label {
            next.auth.label = label.clone().map(Some);
        }
        if state.snapshot == next {
            return false;
        }
        state.snapshot = next;
        self.publish(&state);
        true
    }
    async fn check(
        &self,
        cwd: Option<&str>,
        fresh: bool,
        defer_failure: bool,
    ) -> ServiceResult<ServerProvider> {
        let _gate = self.refresh_gate.lock().await;
        self.check_locked(cwd, fresh, defer_failure).await
    }
    async fn check_locked(
        &self,
        cwd: Option<&str>,
        fresh: bool,
        defer_failure: bool,
    ) -> ServiceResult<ServerProvider> {
        let usage_before = self.state.lock().await.usage_revision;
        let mut next = self.source.snapshot(cwd, fresh).await?;
        if next.instance_id != *self.source.instance_id() {
            return Err(ServiceError::Response);
        }
        let mut state = self.state.lock().await;
        if defer_failure
            && next.status == ServerProviderState::Error
            && next.auth.status != ServerProviderAuthStatus::Unauthenticated
            && state.snapshot.enabled
            && state.snapshot.status == ServerProviderState::Ready
        {
            state.recheck = true;
            return Ok(state.snapshot.clone());
        }
        let text = |value: &Option<Option<TrimmedNonEmptyString>>| {
            value
                .as_ref()
                .and_then(Option::as_ref)
                .map(|text| text.0.as_str().to_owned())
        };
        let email = text(&next.auth.email).map(|email| email.to_lowercase());
        let old_email = text(&state.snapshot.auth.email).map(|email| email.to_lowercase());
        let workspace = text(&next.auth.workspace_id);
        let switched = email
            .as_ref()
            .zip(old_email.as_ref())
            .is_some_and(|(email, old)| email != old)
            || workspace.is_some() && workspace != text(&state.snapshot.auth.workspace_id);
        let previous = state
            .snapshot
            .usage_limits
            .as_ref()
            .and_then(Option::as_ref)
            .cloned()
            .map(Arc::new);
        let probed = next
            .usage_limits
            .as_ref()
            .and_then(Option::as_ref)
            .cloned()
            .map(Arc::new);
        let limits = if switched {
            probed.clone()
        } else if next.usage_limits.is_none() && state.usage_revision != usage_before {
            previous.clone()
        } else {
            usage::usage_after_probe(previous.as_ref(), probed.as_ref())
        };
        if limits != probed && workspace.is_none() {
            next.auth.workspace_id = state.snapshot.auth.workspace_id.clone();
        }
        let keep_label = !switched
            && limits.as_ref().map(|limits| &limits.checked_at)
                == previous.as_ref().map(|limits| &limits.checked_at);
        if !keep_label {
            state.usage_label = None;
        }
        if let Some(label) = &state.usage_label {
            next.auth.label = label.clone().map(Some);
        }
        next.usage_limits = limits.map(|limits| Some((*limits).clone()));
        state.snapshot = next.clone();
        state.generation = state.generation.wrapping_add(1);
        state.checked = true;
        state.recheck = false;
        self.publish(&state);
        Ok(next)
    }
    pub async fn settings_applied(
        &self,
        changed: bool,
        recheck: bool,
    ) -> ServiceResult<ServerProvider> {
        let state = self.state.lock().await;
        let force = state.snapshot.enabled
            && state.snapshot.auth.status == ServerProviderAuthStatus::Unauthenticated;
        if !changed && !force {
            return Ok(state.snapshot.clone());
        }
        drop(state);
        if recheck || force {
            return self.check(None, false, false).await;
        }
        let _gate = self.refresh_gate.lock().await;
        let mut state = self.state.lock().await;
        state.generation = state.generation.wrapping_add(1);
        Ok(state.snapshot.clone())
    }
    pub async fn apply_usage(&self, update: RuntimeUsageUpdate) {
        let mut state = self.state.lock().await;
        let complete = update.complete && !update.windows.is_empty();
        let previous = state
            .snapshot
            .usage_limits
            .as_ref()
            .and_then(Option::as_ref)
            .cloned()
            .map(Arc::new);
        let limits = if complete {
            Some(Arc::new(usage::make_usage_limits(
                update.windows.clone(),
                update.checked_at.clone(),
            )))
        } else {
            usage::apply_usage_update(
                previous.as_ref(),
                &ProviderUsageLimitsUpdate {
                    windows: update.windows.clone(),
                },
                &update.checked_at.0,
            )
        };
        let label = if update.windows.is_empty() {
            None
        } else {
            update.auth_label
        };
        let relabel = (label.is_some() || complete && state.usage_label.is_some())
            && label.as_ref() != state.snapshot.auth.label.as_ref().and_then(Option::as_ref);
        if limits == previous && !relabel {
            return;
        }
        state.snapshot.usage_limits = limits.map(|limits| Some((*limits).clone()));
        if relabel {
            state.snapshot.auth.label = label.clone().map(Some);
        }
        if let Some(label) = label {
            state.usage_label = Some(Some(label));
        } else if complete {
            state.usage_label = Some(None);
        }
        state.usage_revision = state.usage_revision.wrapping_add(1);
        self.publish(&state);
    }
    pub async fn refresh_usage(&self) -> ServiceResult<()> {
        let snapshot = self.current().await;
        if !snapshot.enabled
            || snapshot
                .usage_limits
                .as_ref()
                .and_then(Option::as_ref)
                .is_none_or(|limits| {
                    limits
                        .unavailable
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some_and(|unavailable| {
                            unavailable.reason == ServerProviderUsageUnavailableReason::Unsupported
                        })
                })
        {
            return Ok(());
        }
        let claim = {
            let mut queue = self
                .usage_queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.is_some() {
                return Ok(());
            }
            let identity = Arc::new(());
            *queue = Some(identity.clone());
            Claim {
                queue: self.usage_queue.clone(),
                identity,
            }
        };
        let _gate = self.refresh_gate.lock().await;
        claim.release();
        self.source.invalidate_usage_cache().await?;
        if let Some(usage) = &self.usage {
            usage.invalidate().await?;
        }
        self.check_locked(None, false, false).await.map(|_| ())
    }
    fn needs_usage_recovery(&self, snapshot: &ServerProvider) -> bool {
        self.usage.is_some()
            && snapshot.installed
            && snapshot.status == ServerProviderState::Ready
            && snapshot.auth.status == ServerProviderAuthStatus::Authenticated
            && snapshot
                .usage_limits
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|limits| limits.unavailable.as_ref())
                .and_then(Option::as_ref)
                .is_some_and(|unavailable| {
                    unavailable.reason == ServerProviderUsageUnavailableReason::ProbeFailed
                })
    }
    async fn recover_usage(&self) -> ServiceResult<()> {
        let _gate = self.refresh_gate.lock().await;
        let reader = self.usage.as_ref().ok_or(ServiceError::Unsupported)?;
        let probed = reader.check().await?.map(Arc::new);
        let mut state = self.state.lock().await;
        let previous = state
            .snapshot
            .usage_limits
            .as_ref()
            .and_then(Option::as_ref)
            .cloned()
            .map(Arc::new);
        state.snapshot.usage_limits = usage::usage_after_probe(previous.as_ref(), probed.as_ref())
            .map(|limits| Some((*limits).clone()));
        self.publish(&state);
        Ok(())
    }
}
#[async_trait]
impl ProviderInstanceService for ManagedProvider {
    fn instance_id(&self) -> &ProviderInstanceId {
        self.core.source.instance_id()
    }
    fn subscribe_snapshots(&self) -> Option<watch::Receiver<Option<ServerProvider>>> {
        Some(self.core.changes.subscribe())
    }
    fn catalogs_per_cwd(&self) -> bool {
        self.core.source.catalogs_per_cwd()
    }
    async fn snapshot(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<ServerProvider> {
        self.core.check(cwd, fresh, false).await
    }
    async fn reload_secrets(&self) -> ServiceResult<()> {
        self.core.source.reload_secrets().await
    }
    async fn invalidate_models(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<()> {
        self.core.source.invalidate_models(cwd, fresh).await
    }
    async fn invalidate_caches(&self) -> ServiceResult<()> {
        self.core.source.invalidate_caches().await
    }
    async fn refresh_models(&self) -> ServiceResult<()> {
        self.core.source.refresh_models().await
    }
    async fn invalidate_usage_cache(&self) -> ServiceResult<()> {
        self.core.source.invalidate_usage_cache().await
    }
    async fn after_update(&self) -> ServiceResult<()> {
        self.core.source.after_update().await
    }
    async fn maintenance(&self, fresh: bool) -> ServiceResult<MaintenanceCapabilities> {
        self.core.source.maintenance(fresh).await
    }
    async fn latest_version(&self, fresh: bool) -> ServiceResult<Option<String>> {
        self.core.source.latest_version(fresh).await
    }
    async fn consume_reset_credit(
        &self,
        credit: Option<&str>,
    ) -> ServiceResult<t3_contracts::provider_usage_limits::ProviderConsumeResetCreditOutcome> {
        self.core.source.consume_reset_credit(credit).await
    }
}
