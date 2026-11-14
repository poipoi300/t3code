use crate::{
    catalog::{self, ManifestSource},
    environment::{Environment, probe_environment, process_options},
    instances::InstanceSpec,
    maintenance::{MaintenanceCapabilities, version_advisory},
    maintenance_resolution::NativeMaintenance,
    native_provider::{ExecutableProbe, NativeProviderContext},
    pi_catalog,
    pi_launch::{self, LaunchOptions},
    pi_probe::{PiProbe, PiProbeError, ProbeKind},
    sensitive::{ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    server::{ServerProvider, ServerProviderModel, ServerProviderState},
};
use tokio::sync::Mutex;
pub struct PiProvider {
    spec: Arc<InstanceSpec>,
    context: NativeProviderContext,
    executables: Arc<dyn ExecutableProbe>,
    probe: Arc<dyn PiProbe>,
    clock: Arc<dyn ProviderClock>,
    manifest: ManifestSource,
    maintenance: Option<Arc<NativeMaintenance>>,
    update_checks: std::sync::atomic::AtomicBool,
    machine: Mutex<Option<ServerProvider>>,
}
impl PiProvider {
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        executables: Arc<dyn ExecutableProbe>,
        probe: Arc<dyn PiProbe>,
        clock: Arc<dyn ProviderClock>,
        manifest: ManifestSource,
    ) -> Self {
        Self {
            spec,
            context,
            executables,
            probe,
            clock,
            manifest,
            maintenance: None,
            update_checks: std::sync::atomic::AtomicBool::new(false),
            machine: Mutex::new(None),
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
        probe_environment(&self.spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        ))
    }
    fn binary(&self) -> String {
        crate::environment::expand_home_path(
            self.spec.config["binaryPath"]
                .as_str()
                .filter(|path| !path.is_empty())
                .unwrap_or("pi"),
            &self.context.home,
        )
    }
    fn checked(&self) -> ServiceResult<String> {
        chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .ok_or(ServiceError::Response)
    }
    fn manual(&self) -> MaintenanceCapabilities {
        MaintenanceCapabilities {
            driver: "pi".into(),
            package: Some("@earendil-works/pi-coding-agent".into()),
            update: None,
            install_path: None,
            manual_command: None,
        }
    }
    fn models(&self, discovered: Vec<ServerProviderModel>) -> Vec<ServerProviderModel> {
        let empty = catalog::capabilities(vec![]);
        let mut models = vec![catalog::model("default", "Pi default", Some(empty.clone()))];
        models.extend(discovered);
        catalog::custom_models(
            &models,
            self.spec.config["customModels"]
                .as_array()
                .map_or(&[], Vec::as_slice),
            Some(&empty),
        )
    }
    pub fn initial_snapshot(&self) -> ServiceResult<ServerProvider> {
        let checked = self.checked()?;
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"pi","displayName":self.spec.display_name.as_deref().unwrap_or("Pi"),"enabled":self.spec.enabled,"installed":self.spec.enabled,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown"},"setup":{"canAuthenticate":false,"canInstall":false},"checkedAt":checked,"models":self.models(vec![]),"skills":[],"slashCommands":[],"showInteractionModeToggle":false,"supportedRuntimeModes":["approval-required","auto-accept-edits","full-access"],"reportsContextWindow":true,"supportsTextGeneration":true,"continuation":{"groupKey":self.spec.default_continuation_key()},"message":if self.spec.enabled{"Checking Pi CLI availability..."}else{"Pi is disabled in T3 Code settings."},"versionAdvisory":version_advisory(&self.manual(),None,None,Some(&checked))});
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
    fn launch(
        &self,
        args: &[String],
        cwd: &std::path::Path,
        text_generation: bool,
    ) -> pi_launch::RpcLaunch {
        pi_launch::build_launch(LaunchOptions {
            binary: &self.binary(),
            args,
            cwd,
            environment: &self.environment(),
            session_path: None,
            ephemeral: true,
            short_lived: true,
            disable_extensions: text_generation,
            disable_tools: text_generation,
            extension_path: None,
            runtime_mode: None,
            mcp: None,
        })
    }
    async fn unavailable(
        &self,
        version: &str,
        args: &[String],
        provider: &str,
        id: &str,
    ) -> String {
        let label = format!("Pi default ({provider}/{id})");
        let unavailable = format!(
            "{label} uses {provider}, which has no available models in Pi. Choose another model, or check that provider's setup and the Pi launch arguments."
        );
        if crate::versions::compare_versions(version, "0.84.1").is_lt()
            || args.iter().any(|arg| arg == "--api-key")
        {
            return unavailable;
        }
        let environment = self.environment();
        let args = [
            "auth",
            "check",
            "--provider",
            provider,
            "--json",
            "--no-refresh",
        ]
        .map(str::to_owned);
        let mut options = process_options(self.binary(), &args, &environment);
        options.cwd = Some(self.context.cwd.clone());
        options.timeout = Some(Duration::from_millis(crate::versions::probe_timeout_ms(
            "pi",
            10_000,
            &environment,
        )));
        options.check_exit = false;
        options.max_output_bytes = 256 * 1024;
        let missing = t3_process::run(options)
            .await
            .ok()
            .filter(|output| !output.stdout_truncated)
            .and_then(|output| serde_json::from_str::<Value>(&output.stdout_text()).ok())
            .is_some_and(|value| value["reason"] == "credentials_not_configured");
        if missing {
            format!(
                "{label} has no credentials configured for {provider}. Choose another model, or run `pi` in a terminal and use /login or configure its API key."
            )
        } else {
            unavailable
        }
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
                crate::discovery::Driver::Pi,
                &self.binary(),
                &environment,
                &self.context.cwd,
            )
            .await;
        let version = match version {
            Ok(version) => version,
            Err(_) => {
                value["status"] = json!("error");
                value["message"] = json!("Failed to execute Pi CLI health check.");
                return serde_json::from_value(value).map_err(|_| ServiceError::Response);
            }
        };
        value["installed"] = json!(version.installed);
        value["version"] = json!(version.version);
        if !version.installed || version.status == ServerProviderState::Error {
            value["status"] = json!("error");
            value["message"] = json!(if !version.installed {
                "Pi CLI (`pi`) is not installed or not on PATH. Install with `npm install -g --ignore-scripts @earendil-works/pi-coding-agent`."
            } else if version
                .message
                .as_ref()
                .is_some_and(|message| message.contains("timed out"))
            {
                "Pi CLI is installed but timed out while running `pi --version`."
            } else if version.version.is_none() {
                "T3 Code could not determine the Pi version. Pi 0.80.5 or newer is required."
            } else {
                "Pi CLI is installed but failed to run or is older than the required Pi 0.80.5."
            });
            return serde_json::from_value(value).map_err(|_| ServiceError::Response);
        }
        let args = pi_launch::resolve_launch_args(
            self.spec.config["launchArgs"].as_str().unwrap_or_default(),
        );
        let args = match args {
            Ok(args) => args,
            Err(_) => {
                value["status"] = json!("error");
                value["message"] = json!(
                    "Pi launch arguments are invalid or override options controlled by T3 Code."
                );
                return serde_json::from_value(value).map_err(|_| ServiceError::Response);
            }
        };
        let timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
            "pi",
            30_000,
            &environment,
        ));
        let text = self.probe.read(
            self.launch(args.expose(), &self.context.cwd, true),
            ProbeKind::Models,
        );
        let inventory = self.probe.read(
            self.launch(args.expose(), &self.context.cwd, false),
            ProbeKind::Inventory,
        );
        let discovery = tokio::time::timeout(timeout, async {
            let (text, inventory) = tokio::join!(
                tokio::time::timeout(Duration::from_secs(5), text),
                inventory
            );
            let inventory = inventory.map_err(|_| ServiceError::Response)?;
            let inventory = inventory.expose();
            let mut models = pi_catalog::models(&inventory.models, &inventory.state)?;
            let text = match text {
                Ok(Ok(text)) => pi_catalog::models(&text.expose().models, &Value::Null)
                    .ok()
                    .map(|models| {
                        models
                            .into_iter()
                            .map(|model| model.slug.0)
                            .collect::<BTreeSet<_>>()
                    }),
                Ok(Err(PiProbeError::Exit)) => Some(BTreeSet::new()),
                _ => None,
            };
            if let Some(text) = &text {
                for model in &mut models {
                    if !text.contains(&model.slug.0) {
                        model.supports_text_generation = Some(Some(false));
                    }
                }
            }
            Ok::<_, ServiceError>((
                models,
                inventory.state.clone(),
                pi_catalog::commands(&inventory.commands),
                text.is_some_and(|text| text.is_empty()),
            ))
        })
        .await;
        value["status"] = json!("ready");
        match discovery {
            Err(_) => {
                value["message"] = json!(
                    "Pi is available, but model and command discovery needs interactive input. The live session will handle it."
                )
            }
            Ok(Err(_)) => {
                value["message"] = json!(
                    "Pi is available, but T3 Code could not refresh its models and commands. The live session will retry startup."
                )
            }
            Ok(Ok((models, state, (commands, skills), text_unavailable))) => {
                let authenticated = !models.is_empty();
                let active = &state["model"];
                let unavailable = match (active["provider"].as_str(), active["id"].as_str()) {
                    (Some(provider), Some(id))
                        if !models.iter().any(|model| {
                            model
                                .sub_provider
                                .as_ref()
                                .and_then(Option::as_ref)
                                .is_some_and(|sub| sub.0 == provider)
                        }) =>
                    {
                        Some((provider, id))
                    }
                    _ => None,
                };
                value["auth"] = json!({"status":if authenticated{"authenticated"}else{"unauthenticated"},"type":"pi"});
                value["status"] = json!(if authenticated { "ready" } else { "warning" });
                if !authenticated {
                    value["message"] = json!(
                        "Pi has no usable models. Run `pi` in a terminal and use /login, or configure an API key in ~/.pi/agent."
                    );
                } else if let Some((provider, id)) = unavailable {
                    value["message"] = json!(
                        self.unavailable(
                            version.version.as_deref().unwrap_or_default(),
                            args.expose(),
                            provider,
                            id
                        )
                        .await
                    );
                } else {
                    value
                        .as_object_mut()
                        .ok_or(ServiceError::Response)?
                        .remove("message");
                }
                value["models"] = json!(self.models(models));
                value["skills"] = json!(skills);
                value["slashCommands"] = json!(pi_catalog::builtin_commands(commands));
                if text_unavailable {
                    value["supportsTextGeneration"] = json!(false);
                }
            }
        }
        if let Some(advisory) = crate::versions::resolve_compatibility(
            self.manifest
                .current()
                .compatibility
                .as_deref()
                .unwrap_or_default(),
            "pi",
            version.version.as_deref(),
            &self.context.server_version,
        ) {
            value["compatibilityAdvisory"] = json!(advisory);
        }
        serde_json::from_value(value).map_err(|_| ServiceError::Response)
    }
}
#[async_trait]
impl ProviderInstanceService for PiProvider {
    async fn configure_update_checks(&self, enabled: bool) -> ServiceResult<bool> {
        Ok(self
            .update_checks
            .swap(enabled, std::sync::atomic::Ordering::Relaxed)
            != enabled)
    }
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    fn catalogs_per_cwd(&self) -> bool {
        true
    }
    async fn snapshot(&self, cwd: Option<&str>, _fresh: bool) -> ServiceResult<ServerProvider> {
        let mut machine = self.machine.lock().await;
        if let Some(cwd) = cwd.filter(|_| self.spec.enabled) {
            let mut snapshot = match machine.clone() {
                Some(snapshot) => snapshot,
                None => self.check().await?,
            };
            let args = pi_launch::resolve_launch_args(
                self.spec.config["launchArgs"].as_str().unwrap_or_default(),
            )?;
            let environment = self.environment();
            let timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
                "pi",
                30_000,
                &environment,
            ));
            let inventory = tokio::time::timeout(
                timeout,
                self.probe.read(
                    self.launch(args.expose(), std::path::Path::new(cwd), false),
                    ProbeKind::Commands,
                ),
            )
            .await
            .map_err(|_| ServiceError::Response)?
            .map_err(|_| ServiceError::Response)?;
            let (commands, skills) = pi_catalog::commands(&inventory.expose().commands);
            let workspace=serde_json::from_value(json!({"cwd":cwd,"models":snapshot.models,"slashCommands":pi_catalog::builtin_commands(commands),"skills":skills,"checkedAt":self.checked()?})).map_err(|_|ServiceError::Response)?;
            let workspaces = snapshot.workspace_snapshots.get_or_insert_default();
            workspaces.retain(|workspace| workspace.cwd.0 != cwd);
            workspaces.insert(0, workspace);
            workspaces.truncate(16);
            *machine = Some(snapshot.clone());
            Ok(snapshot)
        } else {
            let mut snapshot = self.check().await?;
            snapshot.workspace_snapshots = machine
                .as_ref()
                .and_then(|snapshot| snapshot.workspace_snapshots.clone());
            *machine = Some(snapshot.clone());
            Ok(snapshot)
        }
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
