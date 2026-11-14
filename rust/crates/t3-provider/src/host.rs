use crate::{
    auth::AuthController,
    instances::{InstanceRegistry, InstanceSpec, InstanceStatus, RegistryEntry},
    managed_provider::{BackgroundPolicy, ManagedOptions},
    native_probe::NativeProbe,
    native_provider::{ExecutableProbe, NativeProvider, NativeProviderContext},
    secrets::SecretResolver,
    sensitive::{ServiceError, ServiceResult},
    service::{ProviderInstanceService, ProviderService},
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use t3_contracts::{
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    server::ServerProvider,
    settings::ServerSettings,
};
use tokio::sync::{Mutex, watch};

pub struct ProviderRuntime {
    pub source: Arc<dyn ProviderInstanceService>,
    pub initial: ServerProvider,
    pub auth: Option<Arc<AuthController>>,
}
#[async_trait]
pub trait ProviderFactory: Send + Sync {
    async fn create(&self, spec: Arc<InstanceSpec>) -> ServiceResult<ProviderRuntime>;
}
#[async_trait]
pub trait ProviderSessions: Send + Sync {
    async fn retire(&self, instance: &ProviderInstanceId) -> ServiceResult<()>;
}
pub struct NativeProviderFactory {
    pub context: NativeProviderContext,
    pub protocols: Arc<dyn NativeProbe>,
    pub executables: Arc<dyn ExecutableProbe>,
    pub clock: Arc<dyn ProviderClock>,
    pub manifest: crate::catalog::ManifestSource,
    pub maintenance:
        BTreeMap<ProviderDriverKind, Arc<crate::maintenance_resolution::NativeMaintenance>>,
    pub update_checks: bool,
    pub background: watch::Receiver<BackgroundPolicy>,
    pub managed_codex: Option<Arc<dyn ProviderFactory>>,
}
pub struct AntigravityProviderFactory {
    pub context: NativeProviderContext,
    pub state_directory: std::path::PathBuf,
    pub browser_helper: std::path::PathBuf,
    pub architecture: String,
    pub installation: Arc<crate::antigravity_installation::AntigravityInstallation>,
    pub http: Arc<dyn crate::transport::HttpTransport>,
    pub stopper: Arc<dyn crate::auth::SessionStopper>,
    pub clock: Arc<dyn ProviderClock>,
    pub manifest: crate::catalog::ManifestSource,
    pub background: watch::Receiver<BackgroundPolicy>,
}
#[async_trait]
impl ProviderFactory for AntigravityProviderFactory {
    async fn create(&self, spec: Arc<InstanceSpec>) -> ServiceResult<ProviderRuntime> {
        if spec.driver.0 != "antigravity" {
            return Err(ServiceError::Unsupported);
        }
        let (profile, temp) = crate::antigravity_profile::instance_directories(
            &self.state_directory,
            &spec.instance_id.0,
        );
        let environment = spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        );
        let auth = crate::antigravity_profile::AuthConfig::from_settings(&spec.config);
        let home = crate::antigravity_profile::user_home(
            self.context.platform,
            &environment,
            &self.context.home,
        );
        let factory = Arc::new(crate::antigravity_runtime::DeferredAntigravityFactory::new(
            self.installation.clone(),
            spec.config["binaryPath"]
                .as_str()
                .filter(|binary| !binary.trim().is_empty())
                .map(|binary| crate::environment::expand_home_path(binary, &self.context.home)),
            crate::antigravity_runtime::DeferredProfileOptions {
                profile_directory: profile.clone(),
                temp_directory: temp,
                user_home: home,
                browser_helper: self.browser_helper.clone(),
                auth: auth.clone(),
                environment,
                platform: self.context.platform,
            },
        ));
        let usage = Arc::new(crate::antigravity_usage::AntigravityUsage::new(
            profile.join("antigravity-acp/acp_token.json"),
            auth.method.clone(),
            match self.context.platform {
                crate::environment::Platform::Windows => "win32",
                crate::environment::Platform::Macos => "darwin",
                crate::environment::Platform::Linux => "linux",
            }
            .into(),
            self.architecture.clone(),
            self.http.clone(),
            self.clock.clone(),
        ));
        let provider = crate::antigravity_provider::AntigravityProvider::new(
            spec.clone(),
            self.context.clone(),
            factory.clone(),
            self.clock.clone(),
            self.manifest.clone(),
            Some(usage),
        )?;
        let controller = AuthController::new(
            spec.instance_id.clone(),
            crate::auth::CredentialBinding {
                key: profile.to_string_lossy().into_owned(),
                owner: t3_contracts::provider_setup::ProviderCredentialOwner::Provider,
            },
            Arc::new(crate::antigravity_auth::AntigravityAuth {
                factory,
                provider: provider.clone(),
                config: auth,
                forwarder: Arc::new(crate::antigravity_auth::LoopbackForwarder),
            }),
            self.stopper.clone(),
            std::time::Duration::from_secs(300),
        )
        .await;
        provider.bind_auth(&controller).await;
        let initial = provider.current().await;
        let source = provider
            .into_managed(ManagedOptions::new(self.background.clone()))
            .await?;
        Ok(ProviderRuntime {
            source,
            initial,
            auth: Some(controller),
        })
    }
}
#[async_trait]
impl ProviderFactory for NativeProviderFactory {
    async fn create(&self, spec: Arc<InstanceSpec>) -> ServiceResult<ProviderRuntime> {
        if !matches!(spec.driver.0.as_str(), "codex" | "claudeAgent") {
            return Err(ServiceError::Unsupported);
        }
        if spec.driver.0 == "codex" && spec.config["setupMode"] == "managed" {
            return self
                .managed_codex
                .as_ref()
                .ok_or(ServiceError::Unavailable)?
                .create(spec)
                .await;
        }
        let mut source = NativeProvider::new(
            spec.clone(),
            self.context.clone(),
            self.protocols.clone(),
            self.executables.clone(),
            self.clock.clone(),
            self.manifest.clone(),
        );
        if let Some(maintenance) = self.maintenance.get(&spec.driver) {
            source = source.with_maintenance(maintenance.clone(), self.update_checks);
        }
        let initial = source.initial_snapshot()?;
        let source = source.into_managed(ManagedOptions::new(self.background.clone()))?;
        Ok(ProviderRuntime {
            source,
            initial,
            auth: None,
        })
    }
}
struct Parked {
    value: ServerProvider,
    changes: watch::Sender<Option<ServerProvider>>,
}
#[async_trait]
impl ProviderInstanceService for Parked {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.value.instance_id
    }
    fn subscribe_snapshots(&self) -> Option<watch::Receiver<Option<ServerProvider>>> {
        Some(self.changes.subscribe())
    }
    async fn snapshot(&self, _: Option<&str>, _: bool) -> ServiceResult<ServerProvider> {
        Ok(self.value.clone())
    }
}
#[derive(Default)]
struct State {
    live: BTreeMap<ProviderInstanceId, Arc<InstanceSpec>>,
    registered: BTreeSet<ProviderInstanceId>,
}
#[derive(Default, Debug, PartialEq, Eq)]
pub struct HostReconcile {
    pub activated: Vec<ProviderInstanceId>,
    pub retired: Vec<ProviderInstanceId>,
    pub unavailable: Vec<ProviderInstanceId>,
}
pub struct ProviderHost {
    pub registry: Arc<InstanceRegistry>,
    pub service: Arc<ProviderService>,
    factories: BTreeMap<ProviderDriverKind, Arc<dyn ProviderFactory>>,
    sessions: Arc<dyn ProviderSessions>,
    secrets: Arc<SecretResolver>,
    clock: Arc<dyn ProviderClock>,
    state: Mutex<State>,
    cache: Option<Arc<crate::status_cache::ProviderStatusCache>>,
}
impl ProviderHost {
    pub fn new(
        registry: Arc<InstanceRegistry>,
        service: Arc<ProviderService>,
        factories: BTreeMap<ProviderDriverKind, Arc<dyn ProviderFactory>>,
        sessions: Arc<dyn ProviderSessions>,
        secrets: Arc<SecretResolver>,
        clock: Arc<dyn ProviderClock>,
    ) -> Self {
        Self {
            registry,
            service,
            factories,
            sessions,
            secrets,
            clock,
            state: Mutex::new(State::default()),
            cache: None,
        }
    }
    pub fn with_status_cache(
        mut self,
        cache: Arc<crate::status_cache::ProviderStatusCache>,
    ) -> Self {
        self.cache = Some(cache);
        self
    }
    fn unavailable(
        &self,
        id: &ProviderInstanceId,
        entry: &RegistryEntry,
        message: &str,
    ) -> ServiceResult<ServerProvider> {
        let checked = chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .ok_or(ServiceError::Response)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut value = json!({"instanceId":id,"driver":entry.source.driver,"displayName":entry.source.display_name.as_ref().and_then(Option::as_ref).filter(|name|!name.trim().is_empty()).map(String::as_str).unwrap_or(&entry.source.driver.0),"enabled":false,"installed":false,"version":null,"status":"disabled","auth":{"status":"unknown"},"checkedAt":checked,"message":message,"models":[],"skills":[],"slashCommands":[],"availability":"unavailable","unavailableReason":message});
        if let Some(color) = entry.source.accent_color.as_ref().and_then(Option::as_ref) {
            value["accentColor"] = json!(color);
        }
        serde_json::from_value(value).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))
    }
    async fn park(
        &self,
        id: &ProviderInstanceId,
        entry: &RegistryEntry,
        message: &str,
    ) -> ServiceResult<()> {
        let value = self.unavailable(id, entry, message)?;
        let (changes, _) = watch::channel(Some(value.clone()));
        self.service
            .register_with_snapshot(
                Arc::new(Parked {
                    value: value.clone(),
                    changes,
                }),
                value,
            )
            .await
    }
    pub async fn reconcile(
        &self,
        settings: &dyn crate::text_generators::SettingsSource,
        rebuild: &BTreeSet<ProviderInstanceId>,
        defer_secrets: bool,
        blocked: &BTreeMap<ProviderDriverKind, String>,
    ) -> ServiceResult<HostReconcile> {
        let mut state = self.state.lock().await;
        let settings = settings.settings().await?;
        self.registry
            .reconcile(&settings, &self.secrets, rebuild, defer_secrets, blocked)
            .await;
        let changes = self.apply_desired(&mut state).await?;
        self.service
            .configure_update_checks(settings.enable_provider_update_checks)
            .await?;
        Ok(changes)
    }
    async fn apply_desired(&self, state: &mut State) -> ServiceResult<HostReconcile> {
        if let Some(cache) = &self.cache {
            self.service.set_status_cache(cache.clone()).await;
        }
        let desired = self.registry.list().await;
        let mut changes = HostReconcile::default();
        let retired=state.live.iter().filter(|(id,spec)|!desired.get(*id).is_some_and(|entry|matches!(&entry.status,InstanceStatus::Prepared(next) if Arc::ptr_eq(spec,next)))).map(|(id,_)|id.clone()).collect::<Vec<_>>();
        for id in retired {
            self.sessions
                .retire(&id)
                .await
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::StopSessions,
                ))?;
            self.service.unregister(&id).await;
            state.live.remove(&id);
            state.registered.remove(&id);
            changes.retired.push(id);
        }
        for id in state.registered.clone() {
            if !desired.contains_key(&id) {
                self.service.unregister(&id).await;
                state.registered.remove(&id);
            }
        }
        for (id, entry) in desired {
            match &entry.status {
                InstanceStatus::Prepared(spec) => {
                    if state
                        .live
                        .get(&id)
                        .is_some_and(|old| Arc::ptr_eq(old, spec))
                    {
                        continue;
                    }
                    let result = match self.factories.get(&spec.driver) {
                        Some(factory) => factory.create(spec.clone()).await,
                        None => Err(ServiceError::Unsupported),
                    };
                    match result {
                        Ok(mut runtime)
                            if runtime.initial.instance_id == id
                                && runtime.initial.driver == spec.driver
                                && runtime.source.instance_id() == &id
                                && runtime
                                    .auth
                                    .as_ref()
                                    .is_none_or(|auth| auth.instance_id() == &id) =>
                        {
                            if let Some(cache) = &self.cache
                                && let Some(cached) = cache.read(&id).await
                                && let Some(hydrated) =
                                    runtime.source.hydrate_cached_snapshot(cached).await
                            {
                                runtime.initial = hydrated;
                            }
                            if let Some(auth) = runtime.auth {
                                self.service.auth.register(id.clone(), auth).await;
                            }
                            self.service
                                .register_with_snapshot(runtime.source, runtime.initial)
                                .await?;
                            state.live.insert(id.clone(), spec.clone());
                            changes.activated.push(id.clone());
                        }
                        result => {
                            if let Ok(runtime) = result
                                && let Some(auth) = runtime.auth
                            {
                                auth.close();
                            }
                            self.park(&id, &entry, "Could not initialize this provider instance.")
                                .await?;
                            changes.unavailable.push(id.clone());
                        }
                    }
                }
                InstanceStatus::Unavailable(reason) => {
                    self.park(&id, &entry, reason.message()).await?;
                    changes.unavailable.push(id.clone());
                }
            }
            state.registered.insert(id);
        }
        Ok(changes)
    }
    pub async fn reload_secrets(
        &self,
        settings: &dyn crate::text_generators::SettingsSource,
        instance: Option<&ProviderInstanceId>,
        blocked: &BTreeMap<ProviderDriverKind, String>,
    ) -> ServiceResult<HostReconcile> {
        let mut state = self.state.lock().await;
        let settings = settings.settings().await?;
        self.registry
            .reload_secrets(&settings, &self.secrets, instance, blocked)
            .await;
        let changes = self.apply_desired(&mut state).await?;
        self.service
            .configure_update_checks(settings.enable_provider_update_checks)
            .await?;
        Ok(changes)
    }
    pub async fn apply_secret(
        &self,
        settings: &dyn crate::text_generators::SettingsSource,
        reference: &t3_contracts::secret_reference::SecretReference,
        value: &str,
        blocked: &BTreeMap<ProviderDriverKind, String>,
    ) -> ServiceResult<HostReconcile> {
        let mut state = self.state.lock().await;
        let settings = settings.settings().await?;
        self.registry
            .apply_secret(&settings, &self.secrets, reference, value, blocked)
            .await;
        let changes = self.apply_desired(&mut state).await?;
        self.service
            .configure_update_checks(settings.enable_provider_update_checks)
            .await?;
        Ok(changes)
    }
    pub async fn close(&self) -> ServiceResult<()> {
        let mut state = self.state.lock().await;
        for id in state.registered.clone() {
            if state.live.contains_key(&id) {
                self.sessions
                    .retire(&id)
                    .await
                    .map_err(crate::diagnostics::mapped(
                        concat!(module_path!(), "::error"),
                        ServiceError::StopSessions,
                    ))?;
            }
            self.service.unregister(&id).await;
            state.registered.remove(&id);
            state.live.remove(&id);
        }
        Ok(())
    }
}

#[async_trait]
impl crate::text_generators::SettingsSource for ServerSettings {
    async fn settings(&self) -> ServiceResult<ServerSettings> {
        Ok(self.clone())
    }
}
