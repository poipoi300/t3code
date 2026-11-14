use crate::{
    catalog::{self, ManifestSource},
    environment::{Environment, probe_environment},
    instances::InstanceSpec,
    maintenance::{MaintenanceCapabilities, version_advisory},
    maintenance_resolution::NativeMaintenance,
    native_provider::{ExecutableProbe, NativeProviderContext},
    sensitive::{Sensitive, ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use t3_contracts::{provider_instance::ProviderInstanceId, server::ServerProvider};

pub fn environment(mut environment: Environment) -> Environment {
    environment.retain(|name, _| !name.eq_ignore_ascii_case("META_API_KEY"));
    environment.insert("MUSE_NO_AUTO_UPDATE".into(), "1".into());
    environment
}
pub fn approval_mode(mode: &str) -> &'static str {
    if mode == "full-access" {
        "allowAll"
    } else {
        "promptUnmatched"
    }
}
pub fn permission_profile(mode: &str) -> &'static str {
    if mode == "full-access" {
        ":unrestricted"
    } else {
        ":ask-me"
    }
}
pub fn serve_args(read_only: bool, session_logging: bool, mode: &str) -> Vec<String> {
    let mut args = vec!["serve"];
    if read_only {
        args.extend(["--disable-shell", "--disable-write"]);
        if !session_logging {
            args.push("--no-session-log");
        }
    } else {
        args.push("--trust-workspace");
        if mode == "full-access" {
            args.push("--disable-sandbox");
        }
    }
    args.into_iter().map(str::to_owned).collect()
}
pub fn initialize_params(read_only: bool) -> Value {
    json!({"clientInfo":{"name":"t3_code","title":"T3 Code","version":"1"},"capabilities":{"requestedCapabilities":if read_only{vec![]}else{vec!["sessionMcp"]}}})
}
pub struct CatalogRequest {
    pub binary: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: Sensitive<Environment>,
    pub initialize: Sensitive<Value>,
    pub startup_timeout: Duration,
}
impl CatalogRequest {
    pub async fn spawn(&self) -> ServiceResult<t3_process::SpawnedProcess> {
        let mut options = crate::environment::process_options(
            &self.binary,
            &self.args,
            self.environment.expose(),
        );
        options.cwd = Some(self.cwd.clone());
        t3_process::spawn_non_blocking(&options)
            .await
            .map_err(|_| ServiceError::Unavailable)
    }
}
impl std::fmt::Debug for CatalogRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MuseCatalogRequest([REDACTED])")
    }
}
#[async_trait]
pub trait MuseCatalog: Send + Sync {
    /// The host owns the MSP connection through initialize and model/list, including cancellation.
    async fn list(&self, request: CatalogRequest) -> ServiceResult<Sensitive<Value>>;
}
pub struct MuseProvider {
    spec: Arc<InstanceSpec>,
    context: NativeProviderContext,
    executables: Arc<dyn ExecutableProbe>,
    catalog: Arc<dyn MuseCatalog>,
    clock: Arc<dyn ProviderClock>,
    manifest: ManifestSource,
    maintenance: Option<Arc<NativeMaintenance>>,
    update_checks: std::sync::atomic::AtomicBool,
}
impl MuseProvider {
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        executables: Arc<dyn ExecutableProbe>,
        catalog: Arc<dyn MuseCatalog>,
        clock: Arc<dyn ProviderClock>,
        manifest: ManifestSource,
    ) -> Self {
        Self {
            spec,
            context,
            executables,
            catalog,
            clock,
            manifest,
            maintenance: None,
            update_checks: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn with_maintenance(
        mut self,
        maintenance: Arc<NativeMaintenance>,
        update_checks: bool,
    ) -> Self {
        self.maintenance = Some(maintenance);
        self.update_checks
            .store(update_checks, std::sync::atomic::Ordering::Relaxed);
        self
    }
    fn environment(&self) -> Environment {
        environment(probe_environment(&self.spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        )))
    }
    fn binary(&self) -> String {
        crate::environment::expand_home_path(
            self.spec.config["binaryPath"]
                .as_str()
                .filter(|path| !path.is_empty())
                .unwrap_or("muse"),
            &self.context.home,
        )
    }
    fn checked(&self) -> ServiceResult<String> {
        chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .ok_or(ServiceError::Response)
    }
    fn models(
        &self,
        models: &[t3_contracts::server::ServerProviderModel],
    ) -> Vec<t3_contracts::server::ServerProviderModel> {
        catalog::custom_models(
            models,
            self.spec.config["customModels"]
                .as_array()
                .map_or(&[], Vec::as_slice),
            Some(&crate::models::muse_capabilities(None)),
        )
    }
    fn manual(&self) -> MaintenanceCapabilities {
        MaintenanceCapabilities {
            driver: "muse".into(),
            package: None,
            update: None,
            install_path: None,
            manual_command: None,
        }
    }
    pub fn initial_snapshot(&self) -> ServiceResult<ServerProvider> {
        let checked = self.checked()?;
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"muse","displayName":self.spec.display_name.as_deref().unwrap_or("Muse Code"),"enabled":self.spec.enabled,"installed":false,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown"},"setup":{"canAuthenticate":false,"canInstall":false},"checkedAt":checked,"models":self.models(&[]),"skills":[],"slashCommands":if self.spec.enabled{json!([{"name":"compact","description":"Summarize the conversation and reduce context usage"}])}else{json!([])},"showInteractionModeToggle":false,"supportedRuntimeModes":["approval-required","full-access"],"reportsContextWindow":true,"supportsTextGeneration":true,"continuation":{"groupKey":self.spec.default_continuation_key()},"message":if self.spec.enabled{"Checking Muse Code CLI availability..."}else{"Muse Code is disabled in T3 Code settings."},"versionAdvisory":version_advisory(&self.manual(),None,None,Some(&checked))});
        if let Some(color) = &self.spec.accent_color {
            value["accentColor"] = json!(color);
        }
        serde_json::from_value(value).map_err(|_| ServiceError::Response)
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
    async fn check(&self) -> ServiceResult<ServerProvider> {
        let initial = self.initial_snapshot()?;
        if !self.spec.enabled {
            return Ok(initial);
        }
        let mut value = serde_json::to_value(initial).map_err(|_| ServiceError::Response)?;
        let environment = self.environment();
        let version = self
            .executables
            .version(
                crate::discovery::Driver::Muse,
                &self.binary(),
                &environment,
                &self.context.cwd,
            )
            .await;
        let version = match version {
            Ok(version) => version,
            Err(_) => {
                value["installed"] = json!(true);
                value["status"] = json!("error");
                value["message"] = json!(
                    "Failed to execute Muse Code CLI. Check its binary path on this T3 server host."
                );
                return serde_json::from_value(value).map_err(|_| ServiceError::Response);
            }
        };
        value["installed"] = json!(version.installed);
        value["version"] = json!(version.version);
        if !version.installed || version.status == t3_contracts::server::ServerProviderState::Error
        {
            value["status"] = json!("error");
            value["message"] = json!(if !version.installed {
                "Muse Code CLI (`muse`) was not found. Install Muse Code and run `muse login` on this T3 server host."
            } else if version
                .message
                .as_ref()
                .is_some_and(|message| message.contains("timed out"))
            {
                "Muse Code CLI version check timed out."
            } else if version
                .message
                .as_ref()
                .is_some_and(|message| message.contains("installed binary is missing"))
            {
                "Muse Code has not finished installing. Run `muse --version` on this T3 server host, then refresh its status."
            } else {
                "Muse Code CLI is installed but failed to run."
            });
        } else {
            let startup_timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
                "muse",
                8_000,
                &environment,
            ));
            let request = CatalogRequest {
                binary: self.binary(),
                args: serve_args(true, false, "approval-required"),
                cwd: self.context.cwd.clone(),
                environment: Sensitive::new(environment),
                initialize: Sensitive::new(initialize_params(true)),
                startup_timeout,
            };
            let catalog = tokio::time::timeout(
                startup_timeout.saturating_add(Duration::from_secs(4)),
                self.catalog.list(request),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|response| crate::models::muse_models(response.expose()).ok());
            match catalog {
                Some(models) => {
                    value["models"] = json!(self.models(&models));
                    value["status"] = json!(if models.is_empty() {
                        "warning"
                    } else {
                        "ready"
                    });
                    if models.is_empty() {
                        value["message"] = json!(
                            "Muse Code returned no models. Run `muse login` on this T3 server host and refresh its status."
                        );
                    } else {
                        value
                            .as_object_mut()
                            .ok_or(ServiceError::Response)?
                            .remove("message");
                    }
                }
                None => {
                    value["status"] = json!("error");
                    value["message"] = json!(
                        "Muse Code SDK could not read the model catalog. Check your Muse installation and run `muse login` on this T3 server host."
                    );
                }
            }
        }
        if let Some(advisory) = crate::versions::resolve_compatibility(
            self.manifest
                .current()
                .compatibility
                .as_deref()
                .unwrap_or_default(),
            "muse",
            version.version.as_deref(),
            &self.context.server_version,
        ) {
            value["compatibilityAdvisory"] = json!(advisory);
        }
        serde_json::from_value(value).map_err(|_| ServiceError::Response)
    }
}
#[async_trait]
impl ProviderInstanceService for MuseProvider {
    async fn configure_update_checks(&self, enabled: bool) -> ServiceResult<bool> {
        Ok(self
            .update_checks
            .swap(enabled, std::sync::atomic::Ordering::Relaxed)
            != enabled)
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    async fn snapshot(&self, _cwd: Option<&str>, _fresh: bool) -> ServiceResult<ServerProvider> {
        self.check().await
    }
    fn has_enrichment(&self) -> bool {
        self.maintenance.is_some()
    }
    async fn maintenance(&self, fresh: bool) -> ServiceResult<MaintenanceCapabilities> {
        match &self.maintenance {
            Some(maintenance) => maintenance.capabilities(fresh).await,
            None => Ok(self.manual()),
        }
    }
    async fn latest_version(&self, fresh: bool) -> ServiceResult<Option<String>> {
        match &self.maintenance {
            Some(maintenance) => maintenance.latest(fresh).await,
            None => Ok(None),
        }
    }
    async fn enrich_snapshot(&self, mut snapshot: ServerProvider) -> ServiceResult<ServerProvider> {
        let capabilities = self.maintenance(false).await?;
        let latest = if self
            .update_checks
            .load(std::sync::atomic::Ordering::Relaxed)
            && snapshot.enabled
            && snapshot.installed
            && snapshot.version.is_some()
        {
            self.latest_version(false).await?
        } else {
            None
        };
        snapshot.version_advisory = Some(version_advisory(
            &capabilities,
            snapshot.version.as_ref().map(|version| version.0.as_str()),
            latest.as_deref(),
            Some(&snapshot.checked_at.0),
        ));
        Ok(snapshot)
    }
}
