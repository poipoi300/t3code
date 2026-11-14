use crate::{
    api::ProviderRegistry,
    error::{Error, Result},
    provider::*,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use t3_contracts::{orchestration_v2::OrchestrationV2ProviderCapabilities, provider_instance::*};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct AdapterMetadata {
    pub driver: ProviderDriverKind,
    pub continuation_key: String,
    pub enabled: bool,
    pub capabilities: OrchestrationV2ProviderCapabilities,
}

#[derive(Default)]
struct CredentialState {
    changing: bool,
    generation: CancellationToken,
}

#[derive(Default)]
pub struct CredentialAccess(Mutex<CredentialState>);

impl CredentialAccess {
    pub fn begin_change(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.changing = true;
        state.generation.cancel();
    }

    pub fn end_change(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = CancellationToken::new();
        state.changing = false;
    }

    fn admit(&self) -> ProviderResult<CancellationToken> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.changing {
            return Err(ProviderError::Operation {
                operation: ProviderOperation::Open,
                detail: "This provider's sign-in is changing. Try again after it finishes.".into(),
            });
        }
        Ok(state.generation.clone())
    }
}

#[derive(Clone)]
pub struct AdapterRegistration {
    pub adapter: Arc<dyn ProviderAdapter>,
    pub continuation_key: String,
    pub enabled: bool,
    pub credential_access: Vec<Arc<CredentialAccess>>,
}

#[async_trait]
pub trait ProviderInstanceSource: Send + Sync {
    /// Each registration includes its own credential gate and gates shared with peer instances.
    async fn instances(&self) -> Result<Vec<AdapterRegistration>>;
}

pub struct AdapterRegistry {
    registrations: Vec<AdapterRegistration>,
    source: Option<Arc<dyn ProviderInstanceSource>>,
}

impl AdapterRegistry {
    pub fn from_adapters(adapters: Vec<Arc<dyn ProviderAdapter>>) -> Self {
        Self::new(
            adapters
                .into_iter()
                .map(|adapter| AdapterRegistration {
                    continuation_key: format!(
                        "{}:instance:{}",
                        adapter.driver().0,
                        adapter.instance_id().0
                    ),
                    adapter,
                    enabled: true,
                    credential_access: Vec::new(),
                })
                .collect(),
        )
    }

    pub fn new(registrations: Vec<AdapterRegistration>) -> Self {
        Self {
            registrations,
            source: None,
        }
    }

    pub fn from_instance_source(source: Arc<dyn ProviderInstanceSource>) -> Self {
        Self {
            registrations: Vec::new(),
            source: Some(source),
        }
    }

    async fn registrations(&self) -> Result<Vec<AdapterRegistration>> {
        match &self.source {
            Some(source) => source.instances().await,
            None => Ok(self.registrations.clone()),
        }
    }

    async fn registration(&self, id: &ProviderInstanceId) -> Result<AdapterRegistration> {
        self.registrations()
            .await?
            .into_iter()
            .find(|entry| entry.adapter.instance_id() == id)
            .ok_or_else(|| Error::Service {
                operation: "ProviderAdapterRegistry.get",
                detail: format!(
                    "No orchestration provider adapter is registered for {}.",
                    id.0
                ),
            })
    }

    pub async fn list(&self) -> Result<Vec<ProviderInstanceId>> {
        Ok(self
            .registrations()
            .await?
            .into_iter()
            .map(|entry| entry.adapter.instance_id().clone())
            .collect())
    }

    pub async fn metadata(&self, id: &ProviderInstanceId) -> Result<AdapterMetadata> {
        let entry = self.registration(id).await?;
        Ok(AdapterMetadata {
            driver: entry.adapter.driver(),
            continuation_key: entry.continuation_key.clone(),
            enabled: entry.enabled,
            capabilities: entry.adapter.capabilities().await?,
        })
    }
}

#[async_trait]
impl ProviderRegistry for AdapterRegistry {
    async fn metadata(&self, instance_id: ProviderInstanceId) -> Result<AdapterMetadata> {
        AdapterRegistry::metadata(self, &instance_id).await
    }

    async fn list(&self) -> Result<Vec<ProviderInstanceId>> {
        AdapterRegistry::list(self).await
    }

    async fn adapter(&self, instance_id: ProviderInstanceId) -> Result<Arc<dyn ProviderAdapter>> {
        let entry = self.registration(&instance_id).await?;
        if entry.credential_access.is_empty() && self.source.is_none() {
            return Ok(entry.adapter.clone());
        }
        Ok(Arc::new(GuardedAdapter {
            adapter: entry.adapter.clone(),
            access: entry.credential_access.clone(),
            source: self.source.clone(),
        }))
    }
}

struct GuardedAdapter {
    adapter: Arc<dyn ProviderAdapter>,
    access: Vec<Arc<CredentialAccess>>,
    source: Option<Arc<dyn ProviderInstanceSource>>,
}

impl GuardedAdapter {
    async fn open(
        &self,
        input: OpenSession,
        options: Option<RuntimePolicyOptions>,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        let access = match &self.source {
            Some(source) => {
                source
                    .instances()
                    .await
                    .map_err(|error| ProviderError::Operation {
                        operation: ProviderOperation::Open,
                        detail: error.to_string(),
                    })?
                    .into_iter()
                    .find(|entry| entry.adapter.instance_id() == self.instance_id())
                    .ok_or(ProviderError::SessionStopped)?
                    .credential_access
            }
            None => self.access.clone(),
        };
        let admitted = access
            .iter()
            .map(|access| access.admit())
            .collect::<ProviderResult<Vec<_>>>()
            .map_err(|_| {
                ProviderError::Setup(Box::new(t3_contracts::provider_setup::ProviderSetupError {
                    _tag: t3_contracts::provider_setup::ProviderSetupErrorTag::Error,
                    instance_id: self.instance_id().clone(),
                    operation: "session".into(),
                    detail: "This provider's sign-in is changing. Try again after it finishes."
                        .into(),
                    cause: None,
                }))
            })?;
        let native = cancellation.child_token();
        let mut changes = JoinSet::new();
        for token in &admitted {
            let token = token.clone();
            changes.spawn(async move { token.cancelled().await });
        }
        let open = async {
            match options {
                Some(options) => {
                    self.adapter
                        .open_session_with_policy(input, options, native.clone())
                        .await
                }
                None => self.adapter.open_session(input, native.clone()).await,
            }
        };
        tokio::pin!(open);
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => { native.cancel(); open.await },
            _ = changes.join_next(), if !changes.is_empty() => { native.cancel(); open.await },
            result = &mut open => result,
        };
        changes.shutdown().await;
        if admitted.iter().any(CancellationToken::is_cancelled) {
            native.cancel();
        }
        if native.is_cancelled() {
            if let Ok(runtime) = result {
                runtime.close().await?;
            }
            return Err(ProviderError::Cancelled);
        }
        result
    }
}

#[async_trait]
impl ProviderAdapter for GuardedAdapter {
    fn instance_id(&self) -> &ProviderInstanceId {
        self.adapter.instance_id()
    }
    fn supported_runtime_modes(&self) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
        self.adapter.supported_runtime_modes()
    }
    fn driver(&self) -> ProviderDriverKind {
        self.adapter.driver()
    }
    async fn capabilities(&self) -> ProviderResult<OrchestrationV2ProviderCapabilities> {
        self.adapter.capabilities().await
    }
    async fn plan_selection_transition(
        &self,
        input: SelectionTransitionInput,
    ) -> ProviderResult<SelectionTransitionPlan> {
        self.adapter.plan_selection_transition(input).await
    }
    async fn open_session(
        &self,
        input: OpenSession,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        self.open(input, None, cancellation).await
    }

    async fn open_session_with_policy(
        &self,
        input: OpenSession,
        options: RuntimePolicyOptions,
        cancellation: CancellationToken,
    ) -> ProviderResult<Arc<dyn SessionRuntime>> {
        self.open(input, Some(options), cancellation).await
    }
}
