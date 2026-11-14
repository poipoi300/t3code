use crate::{
    account_usage,
    catalog::{self, ManifestSource},
    claude_catalog,
    discovery::{self, Driver, VersionProbe},
    environment::{Environment, Platform},
    instances::InstanceSpec,
    native_probe::{NativeProbe, ProbeLaunch},
    probe_models,
    sensitive::{ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    server::{ServerProvider, ServerProviderModel},
};
use tokio::sync::Mutex;

#[async_trait]
pub trait ExecutableProbe: Send + Sync {
    async fn version(
        &self,
        driver: Driver,
        binary: &str,
        environment: &Environment,
        cwd: &Path,
    ) -> ServiceResult<VersionProbe>;
}
pub struct ProcessExecutableProbe;
#[async_trait]
impl ExecutableProbe for ProcessExecutableProbe {
    async fn version(
        &self,
        driver: Driver,
        binary: &str,
        environment: &Environment,
        cwd: &Path,
    ) -> ServiceResult<VersionProbe> {
        discovery::probe_version(driver, Some(binary), environment, Some(cwd))
            .await
            .map_err(|_| ServiceError::Response)
    }
}
#[derive(Clone)]
pub struct NativeProviderContext {
    pub environment: Environment,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub platform: Platform,
    pub managed_mcp: bool,
    pub server_version: String,
}
pub struct NativeProvider {
    spec: Arc<InstanceSpec>,
    context: NativeProviderContext,
    protocols: Arc<dyn NativeProbe>,
    executables: Arc<dyn ExecutableProbe>,
    clock: Arc<dyn ProviderClock>,
    manifest: ManifestSource,
    machine: Mutex<Option<ServerProvider>>,
    probe_cache: Arc<crate::claude_probe_cache::ClaudeProbeCache>,
    reset_client: Option<Arc<crate::reset_credits::ClaudeResetClient>>,
    reset_coordinator: Arc<crate::reset_credits::ResetCreditCoordinator>,
    maintenance: Option<Arc<crate::maintenance_resolution::NativeMaintenance>>,
    update_checks: std::sync::atomic::AtomicBool,
}
impl NativeProvider {
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        protocols: Arc<dyn NativeProbe>,
        executables: Arc<dyn ExecutableProbe>,
        clock: Arc<dyn ProviderClock>,
        manifest: impl Into<ManifestSource>,
    ) -> Self {
        let environment = spec.environment(&context.environment, &context.home, context.platform);
        let configured = spec.config["homePath"].as_str().unwrap_or_default();
        let config =
            crate::environment::claude_home(configured, &environment, &context.home, &context.cwd);
        let account = if !configured.trim().is_empty()
            || environment
                .get("CLAUDE_CONFIG_DIR")
                .is_some_and(|s| !s.trim().is_empty())
        {
            config.join(".claude.json")
        } else {
            context.home.join(".claude.json")
        };
        let probe_cache = Arc::new(crate::claude_probe_cache::ClaudeProbeCache::new(
            protocols,
            clock.clone(),
            account,
        ));
        Self {
            spec,
            context,
            protocols: probe_cache.clone(),
            probe_cache,
            executables,
            clock,
            manifest: manifest.into(),
            machine: Mutex::new(None),
            reset_client: None,
            reset_coordinator: crate::reset_credits::ResetCreditCoordinator::shared(),
            maintenance: None,
            update_checks: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn with_reset_client(
        mut self,
        client: Arc<crate::reset_credits::ClaudeResetClient>,
    ) -> Self {
        self.reset_client = Some(client);
        self
    }
    pub fn with_reset_coordinator(
        mut self,
        coordinator: Arc<crate::reset_credits::ResetCreditCoordinator>,
    ) -> Self {
        self.reset_coordinator = coordinator;
        self
    }
    pub fn with_maintenance(
        mut self,
        maintenance: Arc<crate::maintenance_resolution::NativeMaintenance>,
        update_checks: bool,
    ) -> Self {
        self.maintenance = Some(maintenance);
        self.update_checks
            .store(update_checks, std::sync::atomic::Ordering::Relaxed);
        self
    }
    pub fn initial_snapshot(&self) -> ServiceResult<ServerProvider> {
        let driver = Driver::from_kind(&self.spec.driver.0)
            .filter(|driver| matches!(driver, Driver::Codex | Driver::Claude))
            .ok_or(ServiceError::Unsupported)?;
        let now = self.clock.now_ms();
        let checked = chrono::DateTime::from_timestamp_millis(now)
            .ok_or(ServiceError::Response)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let manifest = self.manifest.current();
        let catalog = manifest.catalog(driver.kind(), now);
        let custom = self.spec.config["customModels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let built_in = catalog
            .map(|catalog| {
                catalog
                    .models
                    .into_iter()
                    .map(|entry| entry.model)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let models = catalog::custom_models(&built_in, &custom, None);
        let name = if driver == Driver::Codex {
            "Codex"
        } else {
            "Claude"
        };
        let message = if self.spec.enabled {
            format!("{name} provider status has not been checked in this session yet.")
        } else {
            format!("{name} is disabled in T3 Code settings.")
        };
        let mut value = json!({"installed":false,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown"},"skills":[],"slashCommands":[],"message":message});
        self.stamp(&mut value, &checked, &models);
        let environment = self.spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        );
        let home = self.spec.config["homePath"].as_str().unwrap_or_default();
        value["continuation"] = json!({"groupKey":if driver==Driver::Codex{crate::environment::codex_home_layout(home,self.spec.config["shadowHomePath"].as_str().unwrap_or_default(),&self.context.home,&self.context.cwd).continuation_key}else{format!("claude:home:{}",crate::environment::claude_home(home,&environment,&self.context.home,&self.context.cwd).display())}});
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
    fn stamp(&self, value: &mut Value, checked: &str, models: &[ServerProviderModel]) {
        value["instanceId"] = json!(self.spec.instance_id);
        value["driver"] = json!(self.spec.driver);
        value["displayName"] = json!(self.spec.display_name.as_deref().unwrap_or(
            if self.spec.driver.0 == "codex" {
                "Codex"
            } else {
                "Claude"
            }
        ));
        if let Some(color) = self.spec.accent_color.as_ref() {
            value["accentColor"] = json!(color);
        }
        value["continuation"] = json!({"groupKey":self.spec.default_continuation_key()});
        value["enabled"] = json!(self.spec.enabled);
        value["checkedAt"] = json!(checked);
        value["models"] = json!(models);
        value["showInteractionModeToggle"] = json!(true);
        value["reportsContextWindow"] = json!(true);
        value["supportedRuntimeModes"] = json!([
            "approval-required",
            "auto-accept-edits",
            "auto",
            "full-access"
        ]);
        value["setup"] = json!({"canAuthenticate":false,"canInstall":false});
    }
    async fn check(
        &self,
        cwd: Option<&str>,
        machine: Option<&ServerProvider>,
    ) -> ServiceResult<(ServerProvider, bool)> {
        let workspace = cwd.is_some();
        let mut pending = false;
        let driver = Driver::from_kind(&self.spec.driver.0)
            .filter(|d| matches!(d, Driver::Codex | Driver::Claude))
            .ok_or(ServiceError::Unsupported)?;
        if driver == Driver::Codex && self.spec.config["setupMode"] == "managed" {
            return Err(ServiceError::Unsupported);
        }
        let now = self.clock.now_ms();
        let checked = chrono::DateTime::from_timestamp_millis(now)
            .ok_or(ServiceError::Response)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let manifest = self.manifest.current();
        let catalog = manifest.catalog(driver.kind(), now);
        let custom = self
            .spec
            .config
            .get("customModels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let builtins = catalog
            .as_ref()
            .map(|c| c.models.iter().map(|e| e.model.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut models = catalog::custom_models(&builtins, &custom, None);
        let mut value = json!({"installed":false,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown"},"models":models});
        self.stamp(&mut value, &checked, &models);
        if !self.spec.enabled {
            return Ok((
                serde_json::from_value(value).map_err(|_| ServiceError::Response)?,
                false,
            ));
        }
        let configured = self.spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        );
        let config_text = |key: &str| self.spec.config[key].as_str().unwrap_or_default();
        let mut environment = if driver == Driver::Claude {
            crate::environment::claude_process_environment(
                &self.context.environment,
                &self.spec.variables,
                &self.context.home,
                self.context.platform,
                self.spec.config["entrypoint"].as_str(),
            )
        } else {
            configured
        };
        if driver == Driver::Claude {
            let home = crate::environment::claude_home(
                config_text("homePath"),
                &environment,
                &self.context.home,
                &self.context.cwd,
            );
            if !config_text("homePath").trim().is_empty() {
                environment.insert(
                    "CLAUDE_CONFIG_DIR".into(),
                    home.to_string_lossy().into_owned(),
                );
            }
            value["continuation"] = json!({"groupKey":format!("claude:home:{}",home.display())});
        } else {
            let layout = crate::environment::codex_home_layout(
                config_text("homePath"),
                config_text("shadowHomePath"),
                &self.context.home,
                &self.context.cwd,
            );
            value["continuation"] = json!({"groupKey":layout.continuation_key});
            if let Some(home) = layout.effective_home {
                environment.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
            }
        }
        let binary = config_text("binaryPath");
        let binary = if binary.trim().is_empty() {
            driver.binary().ok_or(ServiceError::Unsupported)?.to_owned()
        } else {
            crate::environment::expand_home_path(binary, &self.context.home)
        };
        let cwd = cwd
            .map(PathBuf::from)
            .unwrap_or_else(|| self.context.cwd.clone());
        let version = if let Some(machine) = machine {
            VersionProbe {
                installed: machine.installed,
                version: machine.version.as_ref().map(|v| v.0.clone()),
                status: t3_contracts::server::ServerProviderState::Ready,
                message: None,
                resolved_executable: None,
            }
        } else {
            self.executables
                .version(driver, &binary, &environment, &cwd)
                .await?
        };
        value["installed"] = json!(version.installed);
        value["version"] = json!(version.version);
        value["status"] = json!(version.status);
        if let Some(message) = version.message {
            value["message"] = json!(message);
        }
        if !version.installed || version.status == t3_contracts::server::ServerProviderState::Error
        {
            return Ok((
                serde_json::from_value(value).map_err(|_| ServiceError::Response)?,
                false,
            ));
        }
        if driver == Driver::Claude {
            let config = crate::environment::claude_home(
                config_text("homePath"),
                &environment,
                &self.context.home,
                &self.context.cwd,
            );
            value["skills"] = json!(crate::claude_skills::discover(
                crate::claude_skills::ClaudeSkillContext {
                    config: &config,
                    custom_home: !config_text("homePath").trim().is_empty(),
                    cwd: workspace.then_some(cwd.as_path()),
                    home: &self.context.home,
                    environment: &environment,
                    platform: self.context.platform,
                    launch_args: config_text("launchArgs")
                }
            ));
        } else {
            let layout = crate::environment::codex_home_layout(
                config_text("homePath"),
                config_text("shadowHomePath"),
                &self.context.home,
                &self.context.cwd,
            );
            crate::homes::materialize_codex_shadow(&layout)?;
        }
        if workspace && driver == Driver::Claude && !cwd.exists() {
            value["slashCommands"] = json!([{"name":"compact","description":"Summarize the conversation and reduce context usage"}]);
            value["models"] = json!(models);
            return Ok((
                serde_json::from_value(value).map_err(|_| ServiceError::Response)?,
                false,
            ));
        }
        let mut launch = ProbeLaunch {
            executable: binary,
            environment: environment.clone(),
            cwd: cwd.clone(),
            launch_args: config_text("launchArgs").into(),
            managed_mcp: self.context.managed_mcp,
            include_status: !workspace,
            gateway_discovery_grace: Duration::ZERO,
        };
        if driver == Driver::Claude && !workspace {
            let config = crate::environment::claude_home(
                config_text("homePath"),
                &environment,
                &self.context.home,
                &self.context.cwd,
            );
            let mut discovery: std::collections::BTreeMap<String, Value> = environment
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect();
            discovery.extend(crate::claude_skills::settings_environment(
                &config,
                self.context.platform,
                &environment,
            ));
            launch.gateway_discovery_grace =
                crate::native_probe::claude_gateway_discovery_grace(&discovery);
        }
        let result = if driver == Driver::Codex {
            self.protocols.codex(launch).await
        } else {
            self.protocols.claude(launch).await
        };
        match result {
            Err(_) => {
                pending = true;
                value["status"] = json!("warning");
                value["message"] = json!(
                    "The provider could not verify its account status. Refresh to try again."
                );
            }
            Ok(raw) => {
                let raw = raw.expose();
                if driver == Driver::Codex {
                    let (auth, status) =
                        probe_models::codex_auth(&raw["account"], raw.get("rateLimits"))?;
                    value["auth"] = json!(auth);
                    if let Some(endpoint) = raw
                        .get("config")
                        .and_then(|config| probe_models::codex_custom_endpoint(&config["config"]))
                    {
                        value["auth"]["endpoint"] = json!(endpoint);
                    }
                    value["status"] = json!(status);
                    if let Some(agent) = raw["initialize"]["userAgent"]
                        .as_str()
                        .and_then(|s| s.split_once('/'))
                        .and_then(|(_, v)| v.split_whitespace().next())
                    {
                        value["version"] = json!(agent);
                    }
                    if let Some(skills) = raw.get("skills") {
                        value["skills"] =
                            json!(probe_models::codex_skills(skills, &cwd.to_string_lossy())?);
                    }
                    value["slashCommands"] = json!([{"name":"compact","description":"Summarize the conversation and reduce context usage"},{"name":"goal","description":"Set a goal Codex keeps working toward until it is done","input":{"hint":"Objective, or pause, resume, clear"}},{"name":"feedback","description":"Send this thread and Codex logs to OpenAI","input":{"hint":"Describe the issue (optional)"}}]);
                    if let Some(reported) = raw.get("models").and_then(Value::as_array) {
                        let reported = probe_models::codex_models(reported)?;
                        models = catalog::custom_models(
                            &reported,
                            &custom,
                            reported.iter().find_map(|m| m.capabilities.as_ref()),
                        );
                        if let Some(catalog) = catalog.as_ref() {
                            models = catalog::apply_catalog(
                                &models,
                                Some(catalog),
                                catalog::codex_family,
                            );
                            value["updateRequiredModels"] = json!(catalog::codex_update_required(
                                &models,
                                Some(catalog),
                                version.version.as_deref()
                            ));
                        }
                    } else {
                        value["status"] = json!("warning");
                        value["message"] = json!("Codex did not return its model catalog in time.");
                    }
                    if status == "error" {
                        value["message"] =
                            json!("Codex is not authenticated. Run `codex login` and try again.");
                    }
                    probe_models::codex_configured_defaults(
                        &mut models,
                        raw.get("config").map(|v| &v["config"]),
                        &raw["account"],
                    );
                    let usage = if raw["nativeUsage"].as_bool() != Some(true) && status != "error" {
                        account_usage::unavailable(&checked, "unsupported")?
                    } else if let Some(rates) = raw.get("rateLimits") {
                        account_usage::codex_limits(rates, &checked)?
                    } else {
                        account_usage::unavailable(&checked, "probeFailed")?
                    };
                    value["usageLimits"] = json!(usage);
                } else {
                    let init = &raw["initialize"];
                    let config = crate::environment::claude_home(
                        config_text("homePath"),
                        &environment,
                        &self.context.home,
                        &self.context.cwd,
                    );
                    let settings_env = crate::claude_skills::settings_environment_for_workspace(
                        &config,
                        Some(&cwd),
                        self.context.platform,
                        &environment,
                    );
                    let base_url = settings_env
                        .get("ANTHROPIC_BASE_URL")
                        .and_then(Value::as_str)
                        .or_else(|| environment.get("ANTHROPIC_BASE_URL").map(String::as_str));
                    let endpoint = probe_models::claude_custom_endpoint(base_url);
                    let custom_endpoint = endpoint.is_some();
                    let (auth, status) =
                        probe_models::claude_auth(&init["account"], custom_endpoint)?;
                    value["auth"] = json!(auth);
                    if value["auth"]["type"] == "customEndpoint"
                        && let Some(endpoint) = endpoint
                    {
                        value["auth"]["endpoint"] = json!(endpoint);
                    }
                    if let Some(workspace) = raw["workspaceId"].as_str() {
                        value["auth"]["workspaceId"] = json!(workspace);
                    }
                    value["status"] = json!(status);
                    if status == "error" {
                        value["message"] = json!(
                            "Claude Code is not authenticated. Run `claude auth login` and try again."
                        );
                    }
                    if let Some(catalog) = catalog.as_ref() {
                        models =
                            claude_catalog::models_for_version(catalog, version.version.as_deref());
                        models.extend(claude_catalog::reported(
                            catalog,
                            &custom,
                            init.get("models")
                                .and_then(Value::as_array)
                                .map(Vec::as_slice)
                                .unwrap_or_default(),
                        ));
                        models = catalog::custom_models(&models, &custom, None);
                        for model in &mut models {
                            if model.is_custom {
                                model.capabilities =
                                    claude_catalog::ultracode(model.capabilities.take());
                            }
                        }
                        claude_catalog::cap_effort(catalog, &mut models, &raw["settings"]);
                        value["updateRequiredModels"] = json!(claude_catalog::update_required(
                            catalog,
                            version.version.as_deref()
                        ));
                    }
                    value["usageLimits"] = json!(if let Some(usage) = raw.get("usage") {
                        account_usage::claude_limits(
                            usage,
                            raw["usageCheckedAt"].as_str().unwrap_or(&checked),
                        )?
                        .0
                    } else {
                        account_usage::unavailable(&checked, "probeFailed")?
                    });
                    if !workspace
                        && init["account"]["subscriptionType"]
                            .as_str()
                            .is_some_and(|s| !s.is_empty())
                        && value["usageLimits"].get("unavailable").is_none()
                        && let (Some(client), Some(version)) =
                            (self.reset_client.as_ref(), version.version.as_deref())
                    {
                        let config = crate::environment::claude_home(
                            config_text("homePath"),
                            &environment,
                            &self.context.home,
                            &self.context.cwd,
                        );
                        if let Some(credits) = client.read(&config, version).await {
                            value["usageLimits"]["resetCredits"] = json!(credits);
                        }
                    }
                    let mut commands = vec![
                        json!({"name":"compact","description":"Summarize the conversation and reduce context usage"}),
                    ];
                    for command in init
                        .get("commands")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(name) = command["name"]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            if commands.iter().any(|c| {
                                c["name"]
                                    .as_str()
                                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
                            }) {
                                continue;
                            }
                            let mut next = json!({"name":name});
                            if let Some(description) = command["description"]
                                .as_str()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                            {
                                next["description"] = json!(description);
                            }
                            if let Some(hint) = command["argumentHint"]
                                .as_str()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                            {
                                next["input"] = json!({"hint":hint});
                            }
                            commands.push(next);
                        }
                    }
                    value["slashCommands"] = json!(commands);
                }
            }
        }
        value["models"] = json!(models);
        let version = value["version"].as_str();
        value["compatibilityAdvisory"] = json!(crate::versions::resolve_compatibility(
            manifest.compatibility.as_deref().unwrap_or_default(),
            driver.kind(),
            version,
            &self.context.server_version
        ));
        if value["compatibilityAdvisory"].is_null() {
            value
                .as_object_mut()
                .ok_or(ServiceError::Response)?
                .remove("compatibilityAdvisory");
        }
        Ok((
            serde_json::from_value(value).map_err(|_| ServiceError::Response)?,
            pending,
        ))
    }
}
#[async_trait]
impl ProviderInstanceService for NativeProvider {
    async fn configure_update_checks(&self, enabled: bool) -> ServiceResult<bool> {
        Ok(self
            .update_checks
            .swap(enabled, std::sync::atomic::Ordering::Relaxed)
            != enabled)
    }
    fn has_enrichment(&self) -> bool {
        self.maintenance.is_some()
    }
    async fn enrich_snapshot(&self, mut snapshot: ServerProvider) -> ServiceResult<ServerProvider> {
        if let Some(maintenance) = &self.maintenance {
            let capabilities = maintenance.capabilities(false).await?;
            let latest = if self
                .update_checks
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                maintenance.latest(false).await?
            } else {
                None
            };
            snapshot.version_advisory = Some(crate::maintenance::version_advisory(
                &capabilities,
                snapshot.version.as_ref().map(|version| version.0.as_str()),
                latest.as_deref(),
                Some(&snapshot.checked_at.0),
            ));
        }
        Ok(snapshot)
    }
    async fn invalidate_usage_cache(&self) -> ServiceResult<()> {
        self.probe_cache.invalidate().await;
        Ok(())
    }
    async fn invalidate_caches(&self) -> ServiceResult<()> {
        self.probe_cache.invalidate().await;
        if let Some(machine) = self.machine.lock().await.as_mut() {
            machine.workspace_snapshots = None;
        }
        Ok(())
    }
    async fn maintenance(
        &self,
        fresh: bool,
    ) -> ServiceResult<crate::maintenance::MaintenanceCapabilities> {
        self.maintenance
            .as_ref()
            .ok_or(ServiceError::Unsupported)?
            .capabilities(fresh)
            .await
    }
    async fn latest_version(&self, fresh: bool) -> ServiceResult<Option<String>> {
        match self.maintenance.as_ref() {
            Some(maintenance) => maintenance.latest(fresh).await,
            None => Ok(None),
        }
    }
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
                *machine = Some(self.check(None, None).await?.0);
            }
            let base = machine.as_mut().ok_or(ServiceError::Unavailable)?;
            if base.enabled {
                if !fresh
                    && base
                        .workspace_snapshots
                        .as_ref()
                        .is_some_and(|snapshots| snapshots.iter().any(|s| s.cwd.0 == cwd))
                {
                    return Ok(base.clone());
                }
                let exists = Path::new(cwd).exists();
                let keeps_catalog = exists || self.spec.driver.0 == "claudeAgent";
                let (draft, pending) = if keeps_catalog {
                    self.check(Some(cwd), Some(base)).await?
                } else {
                    (base.clone(), false)
                };
                let snapshot = t3_contracts::server::ServerProviderWorkspaceSnapshot {
                    cwd: cwd.to_owned().into(),
                    checked_at: draft.checked_at,
                    slash_commands: if keeps_catalog {
                        draft.slash_commands
                    } else {
                        vec![]
                    },
                    slash_commands_pending: Some(Some(pending)),
                    skills: if keeps_catalog { draft.skills } else { vec![] },
                    option_descriptors: None,
                    models: None,
                    models_pending: None,
                };
                let snapshots = base.workspace_snapshots.get_or_insert_with(Vec::new);
                snapshots.retain(|s| s.cwd.0 != cwd);
                snapshots.push(snapshot);
                if snapshots.len() > 16 {
                    snapshots.remove(0);
                }
            }
            return Ok(base.clone());
        }
        let mut next = self.check(None, None).await?.0;
        if let Some(previous) = machine.as_ref() {
            next.workspace_snapshots = previous.workspace_snapshots.clone();
            if previous.auth == next.auth {
                let old = previous
                    .usage_limits
                    .as_ref()
                    .and_then(Option::as_ref)
                    .cloned()
                    .map(Arc::new);
                let probe = next
                    .usage_limits
                    .as_ref()
                    .and_then(Option::as_ref)
                    .cloned()
                    .map(Arc::new);
                next.usage_limits = crate::usage::usage_after_probe(old.as_ref(), probe.as_ref())
                    .map(|v| Some(v.as_ref().clone()));
            }
        }
        *machine = Some(next.clone());
        Ok(next)
    }
    async fn invalidate_models(&self, _: Option<&str>, fresh: bool) -> ServiceResult<()> {
        if fresh {
            self.probe_cache.invalidate().await;
        }
        Ok(())
    }
    async fn refresh_models(&self) -> ServiceResult<()> {
        self.probe_cache.invalidate().await;
        Ok(())
    }
    async fn consume_reset_credit(
        &self,
        credit: Option<&str>,
    ) -> ServiceResult<t3_contracts::provider_usage_limits::ProviderConsumeResetCreditOutcome> {
        use t3_contracts::provider_usage_limits::ProviderConsumeResetCreditOutcome as Outcome;
        if !self.spec.enabled {
            return Err(ServiceError::Disabled);
        }
        let mut environment = self.spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        );
        let configured = self.spec.config["homePath"].as_str().unwrap_or_default();
        let outcome = match self.spec.driver.0.as_str() {
            "codex" => {
                if self.spec.config["setupMode"] == "managed" {
                    return Err(ServiceError::Unsupported);
                }
                let layout = crate::environment::codex_home_layout(
                    configured,
                    self.spec.config["shadowHomePath"]
                        .as_str()
                        .unwrap_or_default(),
                    &self.context.home,
                    &self.context.cwd,
                );
                let account = layout
                    .effective_home
                    .as_ref()
                    .unwrap_or(&layout.shared_home);
                if let Some(home) = layout.effective_home.as_ref() {
                    environment.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
                }
                let binary = self.spec.config["binaryPath"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("codex");
                let launch = ProbeLaunch {
                    executable: crate::environment::expand_home_path(binary, &self.context.home),
                    environment,
                    cwd: self.context.cwd.clone(),
                    launch_args: self.spec.config["launchArgs"]
                        .as_str()
                        .unwrap_or_default()
                        .into(),
                    managed_mcp: self.context.managed_mcp,
                    include_status: true,
                    gateway_discovery_grace: Duration::ZERO,
                };
                self.reset_coordinator
                    .redeem(
                        &account.to_string_lossy(),
                        |key| self.protocols.consume_codex_reset(launch, key),
                        |_| false,
                    )
                    .await?
            }
            "claudeAgent" => {
                let current = self.snapshot(None, false).await?;
                let credits = current
                    .usage_limits
                    .as_ref()
                    .and_then(Option::as_ref)
                    .and_then(|u| u.reset_credits.as_ref())
                    .and_then(Option::as_ref);
                let grant = if let Some(credit) = credit {
                    credits
                        .filter(|credits| {
                            credits
                                .options
                                .as_ref()
                                .and_then(Option::as_ref)
                                .is_some_and(|options| {
                                    options.iter().any(|o| o.credit_id.0 == credit)
                                })
                        })
                        .map(|_| credit)
                } else {
                    credits
                        .and_then(|c| c.next_credit_id.as_ref())
                        .and_then(Option::as_ref)
                        .map(|s| s.0.as_str())
                };
                let (Some(grant), Some(version)) = (grant, current.version.as_ref()) else {
                    return Ok(Outcome::NoCredit);
                };
                let client = self
                    .reset_client
                    .as_ref()
                    .ok_or(ServiceError::Unsupported)?;
                let config = crate::environment::claude_home(
                    configured,
                    &environment,
                    &self.context.home,
                    &self.context.cwd,
                );
                let account = if !configured.trim().is_empty()
                    || environment
                        .get("CLAUDE_CONFIG_DIR")
                        .is_some_and(|s| !s.trim().is_empty())
                {
                    config.join(".claude.json")
                } else {
                    self.context.home.join(".claude.json")
                };
                let account_key = config.to_string_lossy().into_owned();
                self.reset_coordinator
                    .redeem(
                        &account_key,
                        |key| async move {
                            client
                                .consume(&config, &account, &version.0, grant, &key)
                                .await
                        },
                        |error| error.settled(),
                    )
                    .await
                    .map_err(|_| ServiceError::Response)?
            }
            _ => return Err(ServiceError::Unsupported),
        };
        let before = self
            .machine
            .lock()
            .await
            .as_ref()
            .and_then(|p| p.usage_limits.as_ref())
            .and_then(Option::as_ref)
            .map(|u| u.checked_at.clone());
        for attempt in 0..4 {
            self.probe_cache.invalidate().await;
            let snapshot = self.snapshot(None, true).await?;
            let usage = snapshot.usage_limits.as_ref().and_then(Option::as_ref);
            if outcome!=Outcome::Reset || usage.is_some_and(|u|Some(&u.checked_at)!=before.as_ref() && u.unavailable.as_ref().and_then(Option::as_ref).is_none_or(|e|e.reason!=t3_contracts::provider_usage_limits::ServerProviderUsageUnavailableReason::ProbeFailed)){return Ok(outcome);}
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            }
        }
        Err(ServiceError::Response)
    }
}
