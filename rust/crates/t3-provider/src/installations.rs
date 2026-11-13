use crate::{
    codex_installation::CodexInstallation,
    environment::{Environment, Platform},
    instances::{InstanceRegistry, InstanceStatus},
    sensitive::{ServiceError, ServiceResult},
    service::ProviderService,
    text_generators::SettingsSource,
};
use async_trait::async_trait;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use t3_contracts::{provider_instance::ProviderInstanceId, provider_setup::*};
use tokio::sync::{RwLock, watch};

#[async_trait]
pub trait ManagedInstallation: Send + Sync {
    async fn start(self: Arc<Self>) -> ServiceResult<ProviderInstallState>;
    async fn cancel(&self, operation: &str) -> ServiceResult<ProviderInstallState>;
    async fn remove(&self, protected: &[PathBuf]) -> ServiceResult<ProviderInstallState>;
    fn subscribe(&self) -> watch::Receiver<ProviderInstallState>;
    async fn check_for_update(&self);
}
#[async_trait]
impl ManagedInstallation for CodexInstallation {
    async fn start(self: Arc<Self>) -> ServiceResult<ProviderInstallState> {
        CodexInstallation::start(&self).await
    }
    async fn cancel(&self, operation: &str) -> ServiceResult<ProviderInstallState> {
        CodexInstallation::cancel(self, operation).await
    }
    async fn remove(&self, protected: &[PathBuf]) -> ServiceResult<ProviderInstallState> {
        CodexInstallation::remove(self, protected).await?;
        Ok(self.state())
    }
    fn subscribe(&self) -> watch::Receiver<ProviderInstallState> {
        CodexInstallation::subscribe(self)
    }
    async fn check_for_update(&self) {
        CodexInstallation::check_for_update(self).await;
    }
}
pub struct InstallationContext {
    pub environment: Environment,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub platform: Platform,
}
pub struct ProviderInstallations {
    instances: Arc<InstanceRegistry>,
    providers: Arc<ProviderService>,
    settings: Arc<dyn SettingsSource>,
    context: InstallationContext,
    installations: RwLock<BTreeMap<String, Arc<dyn ManagedInstallation>>>,
}
impl ProviderInstallations {
    pub fn new(
        instances: Arc<InstanceRegistry>,
        providers: Arc<ProviderService>,
        settings: Arc<dyn SettingsSource>,
        context: InstallationContext,
    ) -> Self {
        Self {
            instances,
            providers,
            settings,
            context,
            installations: RwLock::new(BTreeMap::new()),
        }
    }
    pub async fn register(&self, driver: &str, installation: Arc<dyn ManagedInstallation>) {
        self.installations
            .write()
            .await
            .insert(driver.into(), installation);
    }
    async fn installation(
        &self,
        id: &ProviderInstanceId,
        managed_only: bool,
    ) -> ServiceResult<(String, Arc<dyn ManagedInstallation>)> {
        let entry = self
            .instances
            .get(id)
            .await
            .ok_or(ServiceError::Unavailable)?;
        let InstanceStatus::Prepared(spec) = entry.status else {
            return Err(ServiceError::Unavailable);
        };
        let driver = spec.driver.0.clone();
        if !matches!(driver.as_str(), "codex" | "antigravity")
            || driver == "codex" && spec.config["setupMode"] != "managed"
        {
            return Err(ServiceError::Unsupported);
        }
        let binary = spec.config["binaryPath"]
            .as_str()
            .unwrap_or_default()
            .trim();
        if managed_only && !binary.is_empty() && !(driver == "codex" && binary == "codex") {
            return Err(ServiceError::Unsupported);
        }
        let installation = self
            .installations
            .read()
            .await
            .get(&driver)
            .cloned()
            .ok_or(ServiceError::Unavailable)?;
        Ok((driver, installation))
    }
    pub async fn start(&self, input: ProviderSetupInput) -> ServiceResult<ProviderInstallState> {
        let (_, installation) = self.installation(&input.instance_id, true).await?;
        installation.start().await
    }
    pub async fn cancel(
        &self,
        input: ProviderInstallCancelInput,
    ) -> ServiceResult<ProviderInstallState> {
        let (_, installation) = self.installation(&input.instance_id, false).await?;
        installation.cancel(&input.operation_id.0).await
    }
    pub async fn subscribe(
        &self,
        input: ProviderSetupInput,
    ) -> ServiceResult<watch::Receiver<ProviderInstallState>> {
        let (_, installation) = self.installation(&input.instance_id, false).await?;
        let subscription = installation.subscribe();
        if self
            .settings
            .settings()
            .await?
            .enable_provider_update_checks
        {
            tokio::spawn(async move {
                installation.check_for_update().await;
            });
        }
        Ok(subscription)
    }
    pub async fn remove(&self, input: ProviderSetupInput) -> ServiceResult<ProviderInstallState> {
        let (driver, installation) = self.installation(&input.instance_id, true).await?;
        let entries = self.instances.list().await;
        let mut protected = vec![];
        for entry in entries.values() {
            if let Some(binary) = entry
                .source
                .config
                .as_ref()
                .and_then(|v| v["binaryPath"].as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let binary = crate::environment::expand_home_path(binary, &self.context.home);
                protected.push(crate::environment::absolute_path(
                    std::path::Path::new(&binary),
                    &self.context.cwd,
                ));
                if let InstanceStatus::Prepared(spec) = &entry.status {
                    let environment = spec.environment(
                        &self.context.environment,
                        &self.context.home,
                        self.context.platform,
                    );
                    let options = crate::environment::process_options(&binary, &[], &environment);
                    if let Ok(resolved) =
                        t3_process::resolve_command(&options.program, &[], &options.env, false)
                    {
                        protected.push(PathBuf::from(resolved.program));
                    }
                }
            }
        }
        let state = installation.remove(&protected).await?;
        for (id, entry) in entries {
            if entry.source.driver.0 == driver {
                self.providers
                    .refresh_providers(
                        serde_json::from_value(serde_json::json!({"instanceId":id}))
                            .map_err(|_| ServiceError::Response)?,
                    )
                    .await?;
            }
        }
        Ok(state)
    }
}
