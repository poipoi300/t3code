//! ACP Registry provider instances (TypeScript `AcpRegistryDriver.create`): the status snapshot,
//! the sign-in controller and the native session management of one configured registry agent.
//!
//! The agent that runs is the one `RegistryAgentCatalog::resolve` picks from the instance's own
//! settings (registry entry or the local command its settings carry); nothing here takes a path,
//! an argument or an environment variable from anywhere else.
use crate::{
    acp_auth::AcpAuth,
    acp_auth_state::AcpAuthenticationState,
    acp_coordinator::AcpRuntimeCoordinator,
    acp_probe::AcpNativeServices,
    acp_provider::AcpProvider,
    acp_service::{AcpResult, AcpSessionManager},
    auth::{AuthController, CredentialBinding, SessionStopper},
    host::{ProviderFactory, ProviderRuntime, ProviderSessions},
    instances::InstanceSpec,
    maintenance::MaintenanceCapabilities,
    native_provider::NativeProviderContext,
    registry_service::RegistryAgentCatalog,
    sensitive::{Sensitive, ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, RwLock, Weak},
    time::Duration,
};
use t3_contracts::{
    acp_registry::*,
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    server::ServerProvider,
};
use tokio::sync::watch;

/// One live registry instance: what the application host needs to manage its native sessions.
pub struct AcpRegistryInstance {
    pub driver: ProviderDriverKind,
    provider: Arc<AcpProvider>,
    native: Arc<AcpNativeServices>,
    confirmed: Arc<AcpAuthenticationState>,
    binding: CredentialBinding,
}

impl AcpRegistryInstance {
    /// The instance's `acpSessionManagement`.
    pub fn manager(self: &Arc<Self>) -> Arc<dyn AcpSessionManager> {
        self.clone()
    }
    /// The instance's current status snapshot, as the provider service last published it.
    pub async fn snapshot(&self) -> ServiceResult<ServerProvider> {
        self.provider.snapshot(None, false).await
    }
    /// `invalidateEnrichmentCache`: the next snapshot probes the agent again.
    async fn invalidate(&self) {
        let _ = self.provider.invalidate_models(None, true).await;
    }
}

#[async_trait]
impl AcpSessionManager for AcpRegistryInstance {
    async fn list_sessions(
        &self,
        cwd: &Path,
        cursor: Option<&str>,
    ) -> AcpResult<AcpRegistryListSessionsResult> {
        self.native.list_sessions(cwd, cursor).await
    }
    async fn delete_session(&self, cwd: &Path, session: &str) -> AcpResult<()> {
        self.native.delete_session(cwd, session).await
    }
    async fn list_providers(&self, cwd: &Path) -> AcpResult<AcpRegistryListProvidersResult> {
        self.native.list_providers(cwd).await
    }
    async fn set_provider(
        &self,
        cwd: &Path,
        input: Sensitive<AcpRegistrySetProviderInput>,
    ) -> AcpResult<()> {
        self.native.set_provider(cwd, input).await?;
        self.invalidate().await;
        Ok(())
    }
    async fn disable_provider(&self, cwd: &Path, provider: &str) -> AcpResult<()> {
        self.native.disable_provider(cwd, provider).await?;
        self.invalidate().await;
        Ok(())
    }
    async fn logout(&self, cwd: &Path) -> AcpResult<()> {
        self.native.logout(cwd).await?;
        let _ = self.confirmed.set(false).await;
        self.invalidate().await;
        let coordinator = &self.native.coordinator;
        coordinator.clear_commands(&self.native.instance_id);
        coordinator.clear_configuration(&self.native.instance_id);
        Ok(())
    }
}

/// The live registry instances by id. Each entry lives as long as the provider host keeps the
/// instance's runtime: a retired or replaced instance is no longer found.
#[derive(Default)]
pub struct AcpRegistryInstances(RwLock<BTreeMap<ProviderInstanceId, Weak<AcpRegistryInstance>>>);

impl AcpRegistryInstances {
    pub fn get(&self, id: &ProviderInstanceId) -> Option<Arc<AcpRegistryInstance>> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .and_then(Weak::upgrade)
    }
    fn insert(&self, id: ProviderInstanceId, instance: &Arc<AcpRegistryInstance>) {
        let mut map = self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.retain(|_, entry| entry.strong_count() > 0);
        map.insert(id, Arc::downgrade(instance));
    }
    fn sharing(&self, binding: &CredentialBinding) -> Vec<ProviderInstanceId> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|(id, entry)| {
                entry
                    .upgrade()
                    .filter(|instance| instance.binding == *binding)
                    .map(|_| id.clone())
            })
            .collect()
    }
}

/// `ProviderAuthService.stopSessions` for registry agents: a credential change stops the live
/// sessions of the instance and of every registry instance sharing its credential binding.
struct BindingStopper {
    sessions: Arc<dyn ProviderSessions>,
    instances: Weak<AcpRegistryInstances>,
}

#[async_trait]
impl SessionStopper for BindingStopper {
    async fn stop(
        &self,
        instance: &ProviderInstanceId,
        binding: &CredentialBinding,
    ) -> ServiceResult<()> {
        let mut affected = BTreeSet::from([instance.clone()]);
        if let Some(instances) = self.instances.upgrade() {
            affected.extend(instances.sharing(binding));
        }
        for id in &affected {
            self.sessions.retire(id).await?;
        }
        Ok(())
    }
}

/// The instance's status source; holds the instance so the session host can find it.
struct Source(Arc<AcpRegistryInstance>);

#[async_trait]
impl ProviderInstanceService for Source {
    fn instance_id(&self) -> &ProviderInstanceId {
        self.0.provider.instance_id()
    }
    fn subscribe_snapshots(&self) -> Option<watch::Receiver<Option<ServerProvider>>> {
        self.0.provider.subscribe_snapshots()
    }
    async fn snapshot(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<ServerProvider> {
        self.0.provider.snapshot(cwd, fresh).await
    }
    async fn invalidate_models(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<()> {
        self.0.provider.invalidate_models(cwd, fresh).await
    }
    async fn invalidate_caches(&self) -> ServiceResult<()> {
        self.0.provider.invalidate_caches().await
    }
    async fn maintenance(&self, fresh: bool) -> ServiceResult<MaintenanceCapabilities> {
        self.0.provider.maintenance(fresh).await
    }
}

pub struct AcpRegistryFactory {
    pub context: NativeProviderContext,
    /// `ServerConfig.providerStatusCacheDir`: where confirmed sign-ins are remembered.
    pub cache: PathBuf,
    pub settings: Arc<t3_config::settings::SettingsService>,
    pub catalog: Arc<RegistryAgentCatalog>,
    pub coordinator: Arc<AcpRuntimeCoordinator>,
    pub clock: Arc<dyn ProviderClock>,
    pub sessions: Arc<dyn ProviderSessions>,
    pub instances: Arc<AcpRegistryInstances>,
}

#[async_trait]
impl ProviderFactory for AcpRegistryFactory {
    async fn create(&self, spec: Arc<InstanceSpec>) -> ServiceResult<ProviderRuntime> {
        if spec.driver.0 != "acpRegistry" {
            return Err(ServiceError::Unsupported);
        }
        let environment = spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        );
        // The confirmation binding hashes the declared variables (secret references, not values).
        let declared = self
            .settings
            .snapshot()
            .await
            .ok()
            .and_then(|settings| {
                settings
                    .provider_instances
                    .get(&spec.instance_id)
                    .and_then(|instance| instance.environment.clone())
            })
            .unwrap_or_default();
        let confirmed = AcpAuthenticationState::load(
            &self.cache,
            &spec.instance_id,
            &spec.config,
            &declared,
            &environment,
        )
        .await;
        let native = Arc::new(AcpNativeServices {
            instance_id: spec.instance_id.clone(),
            settings: spec.config.clone(),
            environment: Sensitive::new(environment),
            catalog: self.catalog.clone(),
            coordinator: self.coordinator.clone(),
        });
        let provider = AcpProvider::new(
            spec.clone(),
            native.clone(),
            confirmed.clone(),
            self.clock.clone(),
            self.context.cwd.clone(),
        )
        .await;
        let backend = Arc::new(AcpAuth::new(
            &native,
            spec.enabled,
            self.context.cwd.clone(),
            confirmed.clone(),
        ));
        let binding = backend.binding();
        let controller = AuthController::new(
            spec.instance_id.clone(),
            binding.clone(),
            backend,
            Arc::new(BindingStopper {
                sessions: self.sessions.clone(),
                instances: Arc::downgrade(&self.instances),
            }),
            Duration::from_secs(300),
        )
        .await;
        let instance = Arc::new(AcpRegistryInstance {
            driver: spec.driver.clone(),
            provider,
            native,
            confirmed,
            binding,
        });
        self.instances.insert(spec.instance_id.clone(), &instance);
        let initial = instance.snapshot().await?;
        Ok(ProviderRuntime {
            source: Arc::new(Source(instance)),
            initial,
            auth: Some(controller),
        })
    }
}
