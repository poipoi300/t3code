use crate::{
    catalog::{self, ManifestSource},
    instances::InstanceSpec,
    native_provider::NativeProviderContext,
    opencode_catalog,
    opencode_http::{HttpModels, OpenCodeError, OpenCodeHttp},
    opencode_loader::OpenCodeModelLoader,
    opencode_runtime::OpenCodeRuntime,
    opencode_workspace::{self, WorkspaceEvents},
    sensitive::{ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use t3_contracts::{provider_instance::ProviderInstanceId, server::ServerProvider};
use tokio::sync::Mutex;

pub struct OpenCodeProvider {
    spec: Arc<InstanceSpec>,
    context: NativeProviderContext,
    runtime: OpenCodeRuntime,
    events: Arc<dyn WorkspaceEvents>,
    clock: Arc<dyn ProviderClock>,
    manifest: ManifestSource,
    machine: Mutex<Option<ServerProvider>>,
    models: Mutex<Option<(Arc<OpenCodeHttp>, Arc<OpenCodeModelLoader>)>>,
    unsupported_usage: Mutex<Option<String>>,
    maintenance: Option<[Arc<crate::maintenance_resolution::NativeMaintenance>; 2]>,
    service_replacement: Option<Arc<crate::opencode_service::OpenCodeServiceReplacement>>,
    update_checks: std::sync::atomic::AtomicBool,
}
impl OpenCodeProvider {
    pub fn initial_snapshot(&self) -> ServiceResult<ServerProvider> {
        let fallback = opencode_catalog::fallback_capabilities();
        let message = if self.spec.enabled {
            "OpenCode provider status has not been checked in this session yet."
        } else if self.runtime.external() {
            "OpenCode is disabled in T3 Code settings. A server URL is configured."
        } else {
            "OpenCode is disabled in T3 Code settings."
        };
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"opencode","displayName":self.spec.display_name.as_deref().unwrap_or("OpenCode"),"enabled":self.spec.enabled,"installed":false,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown"},"models":catalog::custom_models(&[],&self.custom(),Some(&fallback)),"checkedAt":self.checked()?,"continuation":{"groupKey":self.spec.default_continuation_key()},"supportsConversationRollback":false,"showInteractionModeToggle":false,"message":message});
        if let Some(color) = &self.spec.accent_color {
            value["accentColor"] = json!(color);
        }
        serde_json::from_value(value).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))
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
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        runtime: OpenCodeRuntime,
        events: Arc<dyn WorkspaceEvents>,
        clock: Arc<dyn ProviderClock>,
        manifest: impl Into<ManifestSource>,
    ) -> Self {
        Self {
            spec,
            context,
            runtime,
            events,
            clock,
            manifest: manifest.into(),
            machine: Mutex::new(None),
            models: Mutex::new(None),
            unsupported_usage: Mutex::new(None),
            maintenance: None,
            service_replacement: None,
            update_checks: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn with_service_replacement(
        mut self,
        replacement: Arc<crate::opencode_service::OpenCodeServiceReplacement>,
    ) -> Self {
        self.service_replacement = Some(replacement);
        self
    }
    pub fn with_maintenance(
        mut self,
        v1: Arc<crate::maintenance_resolution::NativeMaintenance>,
        v2: Arc<crate::maintenance_resolution::NativeMaintenance>,
    ) -> Self {
        self.maintenance = Some([v1, v2]);
        self
    }
    async fn maintenance_resolver(
        &self,
    ) -> Option<&Arc<crate::maintenance_resolution::NativeMaintenance>> {
        if !self.spec.enabled {
            return None;
        }
        let version = if self.runtime.local_server() {
            self.runtime.installed_version().await.ok().flatten()
        } else {
            None
        };
        let version = match version {
            Some(version) => version,
            None => self.runtime.probe().await.ok()?,
        };
        self.maintenance
            .as_ref()
            .map(|resolvers| &resolvers[usize::from(version.generation == 2)])
    }
    fn checked(&self) -> ServiceResult<String> {
        chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .ok_or(ServiceError::Response)
    }
    fn environment(&self) -> crate::environment::Environment {
        self.spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        )
    }
    fn custom(&self) -> Vec<Value> {
        self.spec.config["customModels"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
    async fn loader(&self, connection: Arc<OpenCodeHttp>) -> Arc<OpenCodeModelLoader> {
        let mut cached = self.models.lock().await;
        if let Some((http, loader)) = cached.as_ref()
            && Arc::ptr_eq(http, &connection)
        {
            return loader.clone();
        }
        let source = Arc::new(HttpModels {
            connection: connection.clone(),
            directory: self.context.cwd.to_string_lossy().into_owned(),
            strict_names: false,
        });
        let loader = Arc::new(OpenCodeModelLoader::new(source, &self.environment()));
        *cached = Some((connection, loader.clone()));
        loader
    }
    fn failure(&self, value: &mut Value, error: OpenCodeError, inventory: bool) {
        value["status"] = json!("error");
        value["installed"] = json!(self.runtime.external() || inventory);
        value["message"] = json!(if self.runtime.external() {
            match error {
                OpenCodeError::Authentication => {
                    "OpenCode server rejected authentication. Check the server URL and password."
                }
                OpenCodeError::Unavailable | OpenCodeError::Timeout => {
                    "Couldn't reach the configured OpenCode server. Check that the server is running and the URL is correct."
                }
                _ => "Failed to connect to the configured OpenCode server.",
            }
        } else if inventory {
            "Failed to load OpenCode provider inventory."
        } else {
            "OpenCode CLI (`opencode`) is not installed or not on PATH."
        });
        if self.runtime.external() && error == OpenCodeError::Authentication {
            value["auth"]["status"] = json!("unauthenticated");
        }
    }
    async fn check(&self) -> ServiceResult<ServerProvider> {
        let fallback = opencode_catalog::fallback_capabilities();
        let custom = self.custom();
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"opencode","displayName":self.spec.display_name.as_deref().unwrap_or("OpenCode"),"enabled":self.spec.enabled,"installed":false,"version":null,"status":"warning","auth":{"status":"unknown"},"models":catalog::custom_models(&[],&custom,Some(&fallback)),"checkedAt":self.checked()?,"continuation":{"groupKey":self.spec.default_continuation_key()},"supportsConversationRollback":false,"showInteractionModeToggle":false});
        value["usageLimits"] = json!(crate::opencode_usage::unavailable(
            value["checkedAt"].as_str().ok_or(ServiceError::Response)?,
            "unsupported",
            None
        ));
        if let Some(color) = self.spec.accent_color.as_ref() {
            value["accentColor"] = json!(color);
        }
        if !self.spec.enabled {
            value["status"] = json!("disabled");
            value["message"] = json!(if self.runtime.external() {
                "OpenCode is disabled in T3 Code settings. A server URL is configured."
            } else {
                "OpenCode is disabled in T3 Code settings."
            });
        } else {
            let installed = if self.runtime.local_server() {
                self.runtime.installed_version().await.ok().flatten()
            } else {
                None
            };
            match self.runtime.probe().await {
                Err(error) => {
                    self.failure(&mut value, error, false);
                    value["usageLimits"] = json!(crate::opencode_usage::unavailable(
                        value["checkedAt"].as_str().ok_or(ServiceError::Response)?,
                        "probeFailed",
                        Some("OpenCode could not read workspace usage.")
                    ));
                }
                Ok(probed) => {
                    if probed.generation == 1 {
                        value["usageLimits"] = json!(
                            crate::opencode_usage::legacy(
                                &*self.runtime.transport(),
                                &self.environment(),
                                &self.context.home,
                                true,
                                self.runtime.external(),
                                value["checkedAt"].as_str().ok_or(ServiceError::Response)?
                            )
                            .await
                        );
                    }
                    value["installed"] = json!(true);
                    value["version"] = json!(probed.version);
                    if !self.runtime.external() || installed.is_some() {
                        value["installedVersion"] = json!(
                            installed
                                .as_ref()
                                .map(|version| version.version.as_str())
                                .unwrap_or(&probed.version)
                        );
                    }
                    if probed.generation == 1
                        && crate::versions::compare_versions(&probed.version, "1.14.19").is_lt()
                    {
                        value["status"] = json!("error");
                        value["message"] = json!(format!(
                            "OpenCode v{} is too old. Upgrade to v1.14.19 or newer.",
                            probed.version
                        ));
                    } else {
                        match self.runtime.acquire().await {
                            Err(error) => self.failure(&mut value, error, true),
                            Ok(connection) => {
                                value["version"] = json!(connection.version.version);
                                if connection.version.generation == 2 {
                                    let checked = value["checkedAt"]
                                        .as_str()
                                        .ok_or(ServiceError::Response)?
                                        .to_owned();
                                    let cached = self.unsupported_usage.lock().await.as_deref()
                                        == Some(connection.version.version.as_str());
                                    let limits = if cached {
                                        None
                                    } else {
                                        crate::opencode_usage::server(
                                            &connection.http,
                                            &self.context.cwd.to_string_lossy(),
                                            &checked,
                                        )
                                        .await
                                    };
                                    if limits.is_none() {
                                        *self.unsupported_usage.lock().await =
                                            Some(connection.version.version.clone());
                                    }
                                    value["usageLimits"] = json!(match limits {
                                        Some(limits) => limits,
                                        None =>
                                            crate::opencode_usage::legacy(
                                                &*self.runtime.transport(),
                                                &self.environment(),
                                                &self.context.home,
                                                true,
                                                self.runtime.external(),
                                                &checked
                                            )
                                            .await,
                                    });
                                    value["showInteractionModeToggle"] = json!(true);
                                    value["supportedRuntimeModes"] = json!([
                                        "approval-required",
                                        "auto-accept-edits",
                                        "auto",
                                        "full-access"
                                    ]);
                                    value["slashCommands"] =
                                        json!(opencode_catalog::commands(&[], true)?);
                                    match self.loader(connection.http.clone()).await.load().await {
                                        Ok(models) => {
                                            let ready = !models.is_empty();
                                            value["status"] =
                                                json!(if ready { "ready" } else { "warning" });
                                            value["auth"]["status"] = json!(if ready {
                                                "authenticated"
                                            } else {
                                                "unknown"
                                            });
                                            value["models"] = json!(opencode_catalog::v2_models(
                                                &models, &custom
                                            ));
                                            value["message"] = json!(if ready {
                                                format!(
                                                    "OpenCode {} lists {} model{}.",
                                                    connection.version.version,
                                                    models.len(),
                                                    if models.len() == 1 { "" } else { "s" }
                                                )
                                            } else {
                                                "OpenCode 2 is running, but it did not list any models yet.".into()
                                            });
                                        }
                                        Err(_) => {
                                            value["status"] = json!("error");
                                            value["message"] =
                                                json!("OpenCode could not load its model list.");
                                        }
                                    }
                                } else {
                                    let timeout =
                                        Duration::from_millis(crate::versions::probe_timeout_ms(
                                            "opencode",
                                            10_000,
                                            &self.environment(),
                                        ));
                                    match connection
                                        .http
                                        .inventory_v1(&self.context.cwd.to_string_lossy(), timeout)
                                        .await
                                    {
                                        Err(error) => self.failure(&mut value, error, true),
                                        Ok(inventory) => {
                                            let models = opencode_catalog::v1_models(
                                                &inventory.providers,
                                                &inventory.agents,
                                                &custom,
                                            )?;
                                            let connected = inventory.providers["connected"]
                                                .as_array()
                                                .map_or(0, Vec::len);
                                            let ready = connected > 0 && models.is_some();
                                            value["status"] =
                                                json!(if ready { "ready" } else { "warning" });
                                            value["auth"] = json!({"status":if ready{"authenticated"}else{"unknown"},"type":"opencode"});
                                            if let Some(models) = models {
                                                value["models"] = json!(models);
                                            }
                                            value["skills"] = json!(opencode_catalog::skills(
                                                &inventory.skills,
                                                false
                                            )?);
                                            value["slashCommands"] =
                                                json!(opencode_catalog::commands(
                                                    &inventory.commands,
                                                    false
                                                )?);
                                            value["message"] = json!(if connected > 0 && !ready {
                                                "OpenCode upstream providers are connected, but it did not list any models yet.".into()
                                            } else if connected > 0 {
                                                format!(
                                                    "{connected} upstream provider{} connected through {}.",
                                                    if connected == 1 { "" } else { "s" },
                                                    if self.runtime.external() {
                                                        "the configured OpenCode server"
                                                    } else {
                                                        "OpenCode"
                                                    }
                                                )
                                            } else if self.runtime.external() {
                                                "Connected to the configured OpenCode server, but it did not report any connected upstream providers.".into()
                                            } else {
                                                "OpenCode is available, but it did not report any connected upstream providers.".into()
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(advisory) = crate::versions::resolve_compatibility(
            self.manifest
                .current()
                .compatibility
                .as_deref()
                .unwrap_or_default(),
            "opencode",
            value["version"].as_str(),
            &self.context.server_version,
        ) {
            value["compatibilityAdvisory"] = json!(advisory);
        }
        serde_json::from_value(value).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))
    }
    async fn workspace(&self, base: &mut ServerProvider, cwd: &str) -> ServiceResult<()> {
        let connection = self.runtime.acquire().await.map_err(ServiceError::from)?;
        let environment = self.environment();
        let timeout = Duration::from_millis(crate::versions::probe_timeout_ms(
            "opencode",
            10_000,
            &environment,
        ));
        let custom = self.custom();
        let (models, skills, commands, pending) = if connection.version.generation == 2 {
            let directory =
                crate::environment::absolute_path(std::path::Path::new(cwd), &self.context.cwd)
                    .to_string_lossy()
                    .into_owned();
            let source = Arc::new(HttpModels {
                connection: connection.http.clone(),
                directory: cwd.into(),
                strict_names: true,
            });
            let loader = OpenCodeModelLoader::new(source, &environment);
            let (models, workspace) = tokio::join!(
                loader.read_settled(),
                opencode_workspace::load(&connection.http, &*self.events, &directory, &environment)
            );
            let (skills, commands) = workspace.map_err(ServiceError::from)?;
            let pending = commands.is_empty().then_some(true);
            (
                models.map(|models| opencode_catalog::v2_models(&models, &custom)),
                opencode_catalog::skills(&skills, true)?,
                opencode_catalog::commands(&commands, true)?,
                if pending.is_some() { None } else { Some(false) },
            )
        } else {
            let (providers, agents, skills, commands) = tokio::join!(
                connection.http.get("/provider", Some(cwd), 1, timeout),
                connection.http.get("/agent", Some(cwd), 1, timeout),
                connection.http.get("/skill", Some(cwd), 1, timeout),
                connection.http.get("/command", Some(cwd), 1, timeout)
            );
            let models = match (providers, agents) {
                (Ok(providers), Ok(agents)) => agents.as_array().and_then(|agents| {
                    opencode_catalog::v1_models(&providers, agents, &custom)
                        .ok()
                        .flatten()
                }),
                _ => None,
            };
            let skills = skills.map_err(ServiceError::from)?;
            let skills = skills.as_array().ok_or(ServiceError::Response)?;
            let pending = match commands.as_ref() {
                Ok(_) => Some(false),
                Err(OpenCodeError::Timeout) => Some(true),
                _ => None,
            };
            let commands = commands
                .ok()
                .and_then(|commands| commands.as_array().cloned())
                .unwrap_or_default();
            (
                models,
                opencode_catalog::skills(skills, false)?,
                opencode_catalog::commands(&commands, false)?,
                pending,
            )
        };
        let previous = base
            .workspace_snapshots
            .as_ref()
            .and_then(|workspaces| workspaces.iter().find(|workspace| workspace.cwd.0 == cwd));
        let mut snapshot =
            json!({"cwd":cwd,"checkedAt":self.checked()?,"slashCommands":commands,"skills":skills});
        if let Some(pending) = pending {
            snapshot["slashCommandsPending"] = json!(pending);
        }
        match models {
            Some(models) => {
                if models != base.models {
                    snapshot["models"] = json!(models);
                }
            }
            None => {
                snapshot["modelsPending"] = json!(true);
                if let Some(models) = previous.and_then(|workspace| workspace.models.as_ref()) {
                    snapshot["models"] = json!(models);
                }
            }
        }
        let snapshot = serde_json::from_value(snapshot).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
        let workspaces = base.workspace_snapshots.get_or_insert_with(Vec::new);
        workspaces.retain(|workspace| workspace.cwd.0 != cwd);
        workspaces.push(snapshot);
        if workspaces.len() > 16 {
            workspaces.remove(0);
        }
        Ok(())
    }
}
#[async_trait]
impl ProviderInstanceService for OpenCodeProvider {
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
        snapshot.version_advisory = Some(crate::maintenance::version_advisory(
            &capabilities,
            snapshot.version.as_ref().map(|version| version.0.as_str()),
            latest.as_deref(),
            Some(&snapshot.checked_at.0),
        ));
        Ok(snapshot)
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
                *machine = Some(self.check().await?);
            }
            let base = machine.as_mut().ok_or(ServiceError::Unavailable)?;
            if base.enabled
                && (fresh
                    || !base.workspace_snapshots.as_ref().is_some_and(|workspaces| {
                        workspaces.iter().any(|workspace| workspace.cwd.0 == cwd)
                    }))
            {
                self.workspace(base, cwd).await?;
            }
            return Ok(base.clone());
        }
        let mut next = self.check().await?;
        if let Some(previous) = machine.as_ref() {
            next.workspace_snapshots = previous.workspace_snapshots.clone();
        }
        *machine = Some(next.clone());
        Ok(next)
    }
    async fn invalidate_models(&self, _: Option<&str>, _: bool) -> ServiceResult<()> {
        self.models.lock().await.take();
        Ok(())
    }
    async fn invalidate_caches(&self) -> ServiceResult<()> {
        self.runtime.forget().await?;
        self.models.lock().await.take();
        if let Some(machine) = self.machine.lock().await.as_mut() {
            machine.workspace_snapshots = None;
        }
        Ok(())
    }
    async fn after_update(&self) -> ServiceResult<()> {
        let replaced = if let Some(replacement) = &self.service_replacement
            && let Some(server) = self.spec.config["serverUrl"]
                .as_str()
                .filter(|url| !url.trim().is_empty())
        {
            replacement
                .replace(
                    self.spec.config["binaryPath"]
                        .as_str()
                        .unwrap_or("opencode"),
                    server,
                    &self.environment(),
                )
                .await
        } else {
            Ok(())
        };
        let forgotten = self.invalidate_caches().await;
        replaced.and(forgotten)
    }
    async fn maintenance(
        &self,
        fresh: bool,
    ) -> ServiceResult<crate::maintenance::MaintenanceCapabilities> {
        match self.maintenance_resolver().await {
            Some(resolver) => resolver.capabilities(fresh).await,
            None => Ok(crate::maintenance::MaintenanceCapabilities {
                driver: "opencode".into(),
                package: None,
                update: None,
                install_path: None,
                manual_command: None,
            }),
        }
    }
    async fn latest_version(&self, fresh: bool) -> ServiceResult<Option<String>> {
        match self.maintenance_resolver().await {
            Some(resolver) => resolver.latest(fresh).await,
            None => Ok(None),
        }
    }
}
