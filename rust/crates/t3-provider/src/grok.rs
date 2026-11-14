use crate::{
    acp_connection::{AcpConnection, SetupHandler},
    catalog::{self, ManifestSource},
    environment::{Environment, probe_environment, process_options},
    grok_usage,
    instances::InstanceSpec,
    native_provider::{ExecutableProbe, NativeProviderContext},
    registry_service::ResolvedRegistryAgent,
    sensitive::{Sensitive, ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::{HttpTransport, ProviderClock},
};
use async_trait::async_trait;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    provider_policy::RuntimeMode,
    server::{
        ServerProvider, ServerProviderModel, ServerProviderSkill, ServerProviderSlashCommand,
    },
};
use tokio::sync::Mutex;
pub const DEFAULT_MODEL: &str = "grok-build";
pub fn spawn_args(mode: Option<RuntimeMode>) -> Vec<String> {
    match mode {
        None => vec!["agent", "stdio"],
        Some(RuntimeMode::Auto) => vec!["--permission-mode", "auto", "agent", "stdio"],
        Some(RuntimeMode::FullAccess) => vec!["agent", "--always-approve", "stdio"],
        _ => vec!["--permission-mode", "default", "agent", "stdio"],
    }
    .into_iter()
    .map(str::to_owned)
    .collect()
}
pub fn auth_method(environment: &Environment) -> &'static str {
    if environment
        .get("XAI_API_KEY")
        .is_some_and(|key| !key.trim().is_empty())
    {
        "xai.api_key"
    } else {
        "cached_token"
    }
}
pub fn probe_environment_for_grok(environment: &Environment) -> Environment {
    let mut environment = probe_environment(environment);
    environment.insert("GROK_DISABLE_AUTOUPDATER".into(), "1".into());
    environment
}
fn slug(value: &str) -> &str {
    let value = t3_contracts::wire::trim_js(value);
    if value.is_empty() {
        DEFAULT_MODEL
    } else {
        value
    }
}
pub struct CliModels {
    pub authenticated: Option<bool>,
    pub models: Vec<ServerProviderModel>,
}
pub fn parse_models_output(output: &str) -> CliModels {
    let authenticated = if Regex::new("(?i)you are logged in")
        .expect("login state")
        .is_match(output)
    {
        Some(true)
    } else if Regex::new("(?i)not authenticated|not logged in")
        .expect("login state")
        .is_match(output)
    {
        Some(false)
    } else {
        None
    };
    let bullet = Regex::new(r"^\s*[*-]\s+(\S+)(.*)$").expect("model bullet");
    let default = Regex::new("(?i)\\(default\\)").expect("default model");
    let mut seen = BTreeSet::new();
    let mut models = vec![];
    for line in output.lines() {
        if let Some(parts) = bullet.captures(line) {
            let id = slug(&parts[1]);
            if !seen.insert(id.to_owned()) {
                continue;
            }
            let name = id
                .split(['-', '_'])
                .map(|part| {
                    if part.eq_ignore_ascii_case("grok") {
                        "Grok"
                    } else {
                        part
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            let mut model = catalog::model(id, &name, Some(catalog::capabilities(vec![])));
            if default.is_match(&parts[2]) {
                model.is_default = Some(Some(true));
            }
            models.push(model);
        }
    }
    CliModels {
        authenticated,
        models,
    }
}
pub fn initialize_models(initialized: &Value) -> Vec<ServerProviderModel> {
    let state = &initialized["_meta"]["modelState"];
    let Some(current) = state["currentModelId"].as_str() else {
        return vec![];
    };
    let Some(models) = state["availableModels"].as_array() else {
        return vec![];
    };
    if models.iter().any(|model| {
        !model["modelId"].is_string()
            || !model["name"].is_string()
            || model
                .get("description")
                .is_some_and(|value| !value.is_string())
    }) {
        return vec![];
    }
    let mut seen = BTreeSet::new();
    models
        .iter()
        .filter_map(|native| {
            let raw = native["modelId"].as_str()?;
            let id = slug(raw);
            if !seen.insert(id) {
                return None;
            }
            let name = t3_contracts::wire::trim_js(native["name"].as_str()?);
            let mut model = catalog::model(
                id,
                if name.is_empty() { id } else { name },
                Some(crate::models::grok_capabilities(native)),
            );
            if raw.trim() == current.trim() {
                model.is_default = Some(Some(true));
            }
            Some(model)
        })
        .collect()
}
pub fn slash_commands(initialized: &Value) -> Vec<ServerProviderSlashCommand> {
    let mut commands=vec![serde_json::from_value(json!({"name":"compact","description":"Summarize the conversation and reduce context usage"})).expect("compact command")];
    for entry in initialized["_meta"]["availableCommands"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let (Some(name), Some(description)) =
            (entry["name"].as_str(), entry["description"].as_str())
        else {
            continue;
        };
        if entry
            .get("input")
            .is_some_and(|input| !input.is_null() && !input["hint"].is_string())
        {
            continue;
        }
        let name = name.trim();
        if name.is_empty()
            || name.eq_ignore_ascii_case("always-approve")
            || name.eq_ignore_ascii_case("context")
        {
            continue;
        }
        let mut value = json!({"name":name});
        if !description.trim().is_empty() {
            value["description"] = json!(description.trim());
        }
        if let Some(hint) = entry["input"]["hint"]
            .as_str()
            .map(str::trim)
            .filter(|hint| !hint.is_empty())
        {
            value["input"] = json!({"hint":hint});
        }
        if let Ok(command) = serde_json::from_value::<ServerProviderSlashCommand>(value) {
            if let Some(index) = commands
                .iter()
                .position(|previous: &ServerProviderSlashCommand| previous.name == command.name)
            {
                commands[index] = command;
            } else {
                commands.push(command);
            }
        }
    }
    commands
}
pub fn inspect_skills(value: &Value) -> ServiceResult<Vec<ServerProviderSkill>> {
    let entries = value["skills"].as_array().ok_or(ServiceError::Response)?;
    let mut skills = BTreeMap::new();
    for entry in entries {
        let (Some(name), Some(path)) = (entry["name"].as_str(), entry["source"]["path"].as_str())
        else {
            continue;
        };
        let (name, path) = (name.trim(), path.trim());
        if name.is_empty() || path.is_empty() {
            continue;
        }
        let mut value = json!({"name":name,"path":path,"enabled":entry["userInvocable"]!=false});
        for (key, raw) in [
            ("scope", &entry["source"]["type"]),
            ("description", &entry["description"]),
        ] {
            if let Some(text) = raw.as_str().map(str::trim).filter(|text| !text.is_empty()) {
                value[key] = json!(text);
            }
        }
        if let Ok(skill) = serde_json::from_value(value) {
            skills.insert(name.to_owned(), skill);
        }
    }
    Ok(skills.into_values().collect())
}
#[async_trait]
pub trait GrokMetadata: Send + Sync {
    async fn initialize(&self, spawn: ResolvedRegistryAgent) -> ServiceResult<Sensitive<Value>>;
}
pub struct ProcessGrokMetadata;
#[async_trait]
impl GrokMetadata for ProcessGrokMetadata {
    async fn initialize(&self, spawn: ResolvedRegistryAgent) -> ServiceResult<Sensitive<Value>> {
        let connection = AcpConnection::spawn(&spawn, Arc::new(SetupHandler { consent: None }))
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        let result = connection
            .initialize_with_meta(
                "t3-code-provider-probe",
                false,
                Some(json!({"clientType":"extension"})),
            )
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ));
        connection.close().await;
        result
    }
}
pub struct GrokProvider {
    spec: Arc<InstanceSpec>,
    context: NativeProviderContext,
    executables: Arc<dyn ExecutableProbe>,
    metadata: Arc<dyn GrokMetadata>,
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn ProviderClock>,
    manifest: ManifestSource,
    machine: Mutex<Option<ServerProvider>>,
    maintenance: Option<Arc<crate::maintenance_resolution::NativeMaintenance>>,
    update_checks: std::sync::atomic::AtomicBool,
    system_config: PathBuf,
}
impl GrokProvider {
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        executables: Arc<dyn ExecutableProbe>,
        metadata: Arc<dyn GrokMetadata>,
        http: Arc<dyn HttpTransport>,
        clock: Arc<dyn ProviderClock>,
        manifest: ManifestSource,
    ) -> Self {
        Self {
            spec,
            context,
            executables,
            metadata,
            http,
            clock,
            manifest,
            machine: Mutex::new(None),
            maintenance: None,
            update_checks: std::sync::atomic::AtomicBool::new(false),
            system_config: "/etc/grok".into(),
        }
    }
    pub fn with_system_config_directory(mut self, directory: PathBuf) -> Self {
        self.system_config = directory;
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
    fn environment(&self) -> Environment {
        self.spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        )
    }
    fn binary(&self) -> String {
        crate::environment::expand_home_path(
            self.spec.config["binaryPath"]
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or("grok"),
            &self.context.home,
        )
    }
    fn checked(&self) -> ServiceResult<String> {
        chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .ok_or(ServiceError::Response)
    }
    fn custom(&self) -> Vec<Value> {
        self.spec.config["customModels"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
    pub fn initial_snapshot(&self) -> ServiceResult<ServerProvider> {
        let fallback = catalog::capabilities(vec![]);
        let builtin = catalog::model(DEFAULT_MODEL, "Grok Build", Some(fallback.clone()));
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"grok","displayName":self.spec.display_name.as_deref().unwrap_or("Grok"),"enabled":self.spec.enabled,"installed":self.spec.enabled,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown"},"setup":{"canAuthenticate":false,"canInstall":false},"checkedAt":self.checked()?,"models":catalog::custom_models(&[builtin],&self.custom(),Some(&fallback)),"skills":[],"slashCommands":[],"message":if self.spec.enabled{"Checking Grok CLI availability..."}else{"Grok is disabled in T3 Code settings."},"supportsConversationRollback":false,"showInteractionModeToggle":false,"supportedRuntimeModes":["approval-required","auto","full-access"],"continuation":{"groupKey":self.spec.default_continuation_key()}});
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
    async fn command(
        &self,
        args: &[String],
        cwd: &Path,
        timeout: u64,
    ) -> ServiceResult<Option<Sensitive<String>>> {
        let mut options = process_options(
            self.binary(),
            args,
            &probe_environment_for_grok(&self.environment()),
        );
        options.cwd = Some(cwd.into());
        options.check_exit = false;
        options.max_output_bytes = 4 * 1024 * 1024;
        options.timeout = Some(Duration::from_millis(crate::versions::probe_timeout_ms(
            "grok",
            timeout,
            &self.environment(),
        )));
        let output = t3_process::run(options)
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        Ok(
            (output.code == Some(0) && !output.stdout_truncated && !output.stderr_truncated).then(
                || {
                    Sensitive::new(format!(
                        "{}\n{}",
                        output.stdout_text(),
                        output.stderr_text()
                    ))
                },
            ),
        )
    }
    async fn skills(&self, cwd: &Path) -> ServiceResult<Vec<ServerProviderSkill>> {
        let output = self
            .command(&["inspect".into(), "--json".into()], cwd, 4_000)
            .await?
            .ok_or(ServiceError::Response)?;
        let value = serde_json::from_str(output.expose()).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
        inspect_skills(&value)
    }
    async fn cli_models(&self) -> CliModels {
        self.command(&["models".into()], &self.context.cwd, 10_000)
            .await
            .ok()
            .flatten()
            .map(|output| parse_models_output(output.expose()))
            .unwrap_or(CliModels {
                authenticated: None,
                models: vec![],
            })
    }
    async fn check(&self) -> ServiceResult<ServerProvider> {
        let initial = self.initial_snapshot()?;
        if !self.spec.enabled {
            return Ok(initial);
        }
        let mut value = serde_json::to_value(initial).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
        let environment = self.environment();
        let probe_environment = probe_environment_for_grok(&environment);
        let version = match self
            .executables
            .version(
                crate::discovery::Driver::Grok,
                &self.binary(),
                &probe_environment,
                &self.context.cwd,
            )
            .await
        {
            Ok(version) => version,
            Err(_) => {
                value["status"] = json!("error");
                value["message"] = json!("Failed to execute Grok CLI health check.");
                return serde_json::from_value(value).map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ));
            }
        };
        value["installed"] = json!(version.installed);
        value["version"] = json!(version.version);
        if !version.installed || version.status == t3_contracts::server::ServerProviderState::Error
        {
            value["status"] = json!("error");
            value["message"] = json!(if !version.installed {
                "Grok CLI (`grok`) is not installed or not on PATH."
            } else if version
                .message
                .as_ref()
                .is_some_and(|message| message.contains("timed out"))
            {
                "Grok CLI is installed but timed out while running `grok --version`."
            } else {
                "Grok CLI is installed but failed to run."
            });
            return serde_json::from_value(value).map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ));
        }
        let api_key = environment
            .get("XAI_API_KEY")
            .is_some_and(|key| !key.trim().is_empty());
        let mut cli = self.cli_models().await;
        if cli.authenticated == Some(false) && !api_key {
            let recheck = self.cli_models().await;
            if recheck.authenticated.is_some() {
                cli = recheck;
            }
        }
        value["auth"] = if api_key {
            json!({"status":"authenticated","type":"api_key","label":"xAI API key"})
        } else {
            match cli.authenticated {
                Some(true) => {
                    json!({"status":"authenticated","type":"cached_token","label":"Grok account"})
                }
                Some(false) => json!({"status":"unauthenticated"}),
                None => json!({"status":"unknown"}),
            }
        };
        value["skills"] = json!(self.skills(&self.context.cwd).await.unwrap_or_default());
        let mut spawn_environment = probe_environment;
        spawn_environment.insert("GROK_OAUTH2_REFERRER".into(), "t3code".into());
        let spawn = ResolvedRegistryAgent {
            agent: None,
            distribution: None,
            executable: self.binary().into(),
            args: spawn_args(None),
            cwd: self.context.cwd.clone(),
            environment: Sensitive::new(spawn_environment),
        };
        let metadata = tokio::time::timeout(
            Duration::from_millis(crate::versions::probe_timeout_ms(
                "grok",
                20_000,
                &environment,
            )),
            self.metadata.initialize(spawn),
        )
        .await
        .ok()
        .and_then(Result::ok);
        let acp = metadata
            .as_ref()
            .map(|metadata| initialize_models(metadata.expose()))
            .unwrap_or_default();
        let discovered = if acp.is_empty() { cli.models } else { acp };
        let empty = discovered.is_empty();
        if !empty {
            value["models"] = json!(catalog::custom_models(
                &discovered,
                &self.custom(),
                Some(&catalog::capabilities(vec![]))
            ));
        }
        value
            .as_object_mut()
            .ok_or(ServiceError::Response)?
            .remove("message");
        if value["auth"]["status"] == "unauthenticated" {
            value["status"] = json!("error");
            value["message"] = json!("Grok CLI is installed but not logged in. Run `grok login`.");
        } else {
            value["status"] = json!(if metadata.is_none() || empty {
                "warning"
            } else {
                "ready"
            });
            value["slashCommands"] = json!(slash_commands(
                metadata
                    .as_ref()
                    .map(Sensitive::expose)
                    .unwrap_or(&Value::Null)
            ));
            if metadata.is_none() {
                value["message"] = json!(
                    "Grok CLI is installed but ACP initialize failed. Model options may be incomplete."
                );
            } else if empty {
                value["message"] =
                    json!("Grok CLI did not list any models. Model options may be incomplete.");
            }
        }
        if value["auth"]["status"] == "authenticated" {
            let account = grok_usage::read(
                &*self.http,
                &environment,
                &self.context.home,
                &self.system_config,
                value["checkedAt"].as_str().ok_or(ServiceError::Response)?,
            )
            .await?;
            value["usageLimits"] = json!(account.usage_limits);
            if let Some(email) = account.email {
                value["auth"]["email"] = json!(email);
            }
        }
        if let Some(advisory) = crate::versions::resolve_compatibility(
            self.manifest
                .current()
                .compatibility
                .as_deref()
                .unwrap_or_default(),
            "grok",
            version.version.as_deref(),
            &self.context.server_version,
        ) {
            value["compatibilityAdvisory"] = json!(advisory);
        }
        serde_json::from_value(value).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))
    }
}
#[async_trait]
impl ProviderInstanceService for GrokProvider {
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
    async fn snapshot(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<ServerProvider> {
        let mut machine = self.machine.lock().await;
        if let Some(cwd) = cwd {
            if machine.is_none() {
                *machine = Some(self.check().await?);
            }
            let base = machine.as_mut().ok_or(ServiceError::Unavailable)?;
            if !base.enabled {
                return Ok(base.clone());
            }
            let cached = base
                .workspace_snapshots
                .as_ref()
                .is_some_and(|snapshots| snapshots.iter().any(|snapshot| snapshot.cwd.0 == cwd));
            if cached && !fresh {
                return Ok(base.clone());
            }
            let workspace=serde_json::from_value(json!({"cwd":cwd,"checkedAt":self.checked()?,"slashCommands":base.slash_commands,"skills":self.skills(Path::new(cwd)).await?,"slashCommandsPending":false})).map_err(crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response))?;
            let snapshots = base.workspace_snapshots.get_or_insert_with(Vec::new);
            snapshots.retain(|snapshot| snapshot.cwd.0 != cwd);
            snapshots.push(workspace);
            if snapshots.len() > 16 {
                snapshots.remove(0);
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
    async fn invalidate_caches(&self) -> ServiceResult<()> {
        *self.machine.lock().await = None;
        Ok(())
    }
    async fn maintenance(
        &self,
        fresh: bool,
    ) -> ServiceResult<crate::maintenance::MaintenanceCapabilities> {
        match &self.maintenance {
            Some(maintenance) => maintenance.capabilities(fresh).await,
            None => Ok(crate::maintenance::MaintenanceCapabilities {
                driver: "grok".into(),
                package: Some("@xai-official/grok".into()),
                update: None,
                install_path: None,
                manual_command: None,
            }),
        }
    }
    async fn latest_version(&self, fresh: bool) -> ServiceResult<Option<String>> {
        match &self.maintenance {
            Some(maintenance) => maintenance.latest(fresh).await,
            None => Ok(None),
        }
    }
}
