use crate::{
    catalog::ManifestSource,
    fx::{FxProvider, FxSettings},
    fx_skills,
    instances::InstanceSpec,
    native_provider::NativeProviderContext,
    sensitive::{ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    server::{ServerProvider, ServerProviderModel},
};
use tokio::sync::Mutex;
pub fn default_model_id(models: &[ServerProviderModel]) -> Option<&str> {
    models
        .iter()
        .find(|model| model.slug.0 == "default")
        .and_then(|model| model.aliases.as_ref())
        .and_then(Option::as_ref)
        .and_then(|aliases| aliases.first())
        .map(|alias| alias.0.as_str())
}
pub struct FxInstance {
    core: FxProvider,
    spec: Arc<InstanceSpec>,
    context: NativeProviderContext,
    clock: Arc<dyn ProviderClock>,
    manifest: ManifestSource,
    machine: Mutex<Option<ServerProvider>>,
}
impl FxInstance {
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        clock: Arc<dyn ProviderClock>,
        manifest: ManifestSource,
    ) -> ServiceResult<Self> {
        if spec.driver.0 != "fx" {
            return Err(ServiceError::Unsupported);
        }
        let mut settings: FxSettings =
            serde_json::from_value(spec.config.clone()).map_err(|_| ServiceError::Response)?;
        settings.enabled = spec.enabled;
        settings.binary_path =
            crate::environment::expand_home_path(&settings.binary_path, &context.home);
        let environment = spec.environment(&context.environment, &context.home, context.platform);
        let core = FxProvider {
            instance_id: spec.instance_id.clone(),
            settings,
            environment,
        };
        Ok(Self {
            core,
            spec,
            context,
            clock,
            manifest,
            machine: Mutex::new(None),
        })
    }
    fn checked(&self) -> ServiceResult<String> {
        chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .ok_or(ServiceError::Response)
    }
    fn stamp(&self, mut snapshot: ServerProvider) -> ServerProvider {
        if let Some(name) = &self.spec.display_name {
            snapshot.display_name = Some(Some(name.clone().into()));
        }
        if let Some(color) = &self.spec.accent_color {
            snapshot.accent_color = Some(Some(color.clone().into()));
        }
        snapshot.compatibility_advisory = crate::versions::resolve_compatibility(
            self.manifest
                .current()
                .compatibility
                .as_deref()
                .unwrap_or_default(),
            "fx",
            snapshot.version.as_ref().map(|version| version.0.as_str()),
            &self.context.server_version,
        );
        snapshot
    }
    pub fn initial_snapshot(&self) -> ServiceResult<ServerProvider> {
        self.core
            .initial(&self.checked()?)
            .map(|snapshot| self.stamp(snapshot))
            .map_err(|_| ServiceError::Response)
    }
    pub fn into_managed(
        self,
        options: crate::managed_provider::ManagedOptions,
    ) -> ServiceResult<Arc<crate::managed_provider::ManagedProvider>> {
        let initial = self.initial_snapshot()?;
        let manifest = self.manifest.subscribe();
        let background = options.background.clone();
        let provider =
            crate::managed_provider::ManagedProvider::new(Arc::new(self), initial, options)?;
        provider.recheck_on_manifest(manifest, background);
        Ok(provider)
    }
}
#[async_trait]
impl ProviderInstanceService for FxInstance {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    fn catalogs_per_cwd(&self) -> bool {
        true
    }
    async fn snapshot(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<ServerProvider> {
        let mut machine = self.machine.lock().await;
        if let Some(cwd) = cwd {
            if machine.is_none() {
                *machine = Some(
                    self.stamp(
                        self.core
                            .check(&self.checked()?)
                            .await
                            .map_err(|_| ServiceError::Response)?,
                    ),
                );
            }
            let base = machine.as_mut().ok_or(ServiceError::Unavailable)?;
            if !base.enabled {
                return Ok(base.clone());
            }
            if !fresh
                && base
                    .workspace_snapshots
                    .as_ref()
                    .is_some_and(|snapshots| snapshots.iter().any(|snapshot| snapshot.cwd.0 == cwd))
            {
                return Ok(base.clone());
            }
            let directory = PathBuf::from(cwd);
            let environment = self.core.environment.clone();
            let skills =
                tokio::task::spawn_blocking(move || fx_skills::discover(&directory, &environment))
                    .await
                    .map_err(|_| ServiceError::Response)?;
            let workspace=serde_json::from_value(json!({"cwd":cwd,"checkedAt":self.checked()?,"slashCommands":base.slash_commands,"skills":skills,"slashCommandsPending":false})).map_err(|_|ServiceError::Response)?;
            let snapshots = base.workspace_snapshots.get_or_insert_with(Vec::new);
            snapshots.retain(|snapshot| snapshot.cwd.0 != cwd);
            snapshots.push(workspace);
            if snapshots.len() > 16 {
                snapshots.remove(0);
            }
            return Ok(base.clone());
        }
        let mut next = self.stamp(
            self.core
                .check(&self.checked()?)
                .await
                .map_err(|_| ServiceError::Response)?,
        );
        if let Some(previous) = machine.as_ref() {
            next.workspace_snapshots = previous.workspace_snapshots.clone();
        }
        *machine = Some(next.clone());
        Ok(next)
    }
    async fn invalidate_caches(&self) -> ServiceResult<()> {
        *self.machine.lock().await = None;
        Ok(())
    }
    async fn maintenance(
        &self,
        _: bool,
    ) -> ServiceResult<crate::maintenance::MaintenanceCapabilities> {
        Ok(crate::maintenance::MaintenanceCapabilities {
            driver: "fx".into(),
            package: None,
            update: None,
            install_path: None,
            manual_command: None,
        })
    }
}
