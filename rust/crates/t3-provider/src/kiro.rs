use crate::{
    acp_connection::{AcpConnection, SetupHandler},
    catalog::{self, ManifestSource},
    environment::{Environment, probe_environment, process_options},
    instances::InstanceSpec,
    models,
    native_provider::{ExecutableProbe, NativeProviderContext},
    registry_service::ResolvedRegistryAgent,
    sensitive::{Sensitive, ServiceError, ServiceResult},
    service::ProviderInstanceService,
    transport::ProviderClock,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use t3_contracts::{
    acp_registry::AcpRegistryOperationErrorReason,
    model::ProviderOptionDescriptor,
    provider_instance::ProviderInstanceId,
    server::{ServerProvider, ServerProviderAuthStatus, ServerProviderModel},
};
pub const ACP_ARGS: &[&str] = &["acp", "--agent-engine=v3", "--auth-method=cli"];
pub fn default_model_id(models: &[ServerProviderModel]) -> &str {
    models
        .iter()
        .find(|model| model.slug.0 == "default")
        .and_then(|model| model.aliases.as_ref())
        .and_then(Option::as_ref)
        .and_then(|aliases| aliases.first())
        .map_or("auto", |alias| alias.0.as_str())
}
#[async_trait]
pub trait KiroMetadata: Send + Sync {
    async fn template(&self, spawn: ResolvedRegistryAgent) -> ServiceResult<Sensitive<Value>>;
}
pub struct ProcessKiroMetadata;
#[async_trait]
impl KiroMetadata for ProcessKiroMetadata {
    async fn template(&self, spawn: ResolvedRegistryAgent) -> ServiceResult<Sensitive<Value>> {
        let connection = AcpConnection::spawn(&spawn, Arc::new(SetupHandler { consent: None }))
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        let result = async {
            connection
                .initialize("t3-code-provider-probe", false)
                .await
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))?;
            connection
                .request(
                    "_kiro/config/template",
                    Sensitive::new(json!({})),
                    AcpRegistryOperationErrorReason::ProbeFailed,
                )
                .await
                .map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ))
        }
        .await;
        connection.close().await;
        result
    }
}
struct Command {
    code: Option<i32>,
    stdout: Sensitive<String>,
}
pub struct KiroProvider {
    spec: Arc<InstanceSpec>,
    context: NativeProviderContext,
    executables: Arc<dyn ExecutableProbe>,
    metadata: Arc<dyn KiroMetadata>,
    clock: Arc<dyn ProviderClock>,
    manifest: ManifestSource,
}
impl KiroProvider {
    pub fn new(
        spec: Arc<InstanceSpec>,
        context: NativeProviderContext,
        executables: Arc<dyn ExecutableProbe>,
        metadata: Arc<dyn KiroMetadata>,
        clock: Arc<dyn ProviderClock>,
        manifest: ManifestSource,
    ) -> Self {
        Self {
            spec,
            context,
            executables,
            metadata,
            clock,
            manifest,
        }
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
                .filter(|path| !path.is_empty())
                .unwrap_or("kiro-cli"),
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
        listed: Option<Vec<ServerProviderModel>>,
        agent: Option<ProviderOptionDescriptor>,
    ) -> Vec<ServerProviderModel> {
        let empty = catalog::capabilities(vec![]);
        let listed = listed.unwrap_or_else(|| {
            vec![catalog::model(
                "default",
                "Kiro default",
                Some(empty.clone()),
            )]
        });
        let custom = self.spec.config["customModels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut models = catalog::custom_models(&listed, &custom, Some(&empty));
        if let Some(agent) = agent {
            for model in &mut models {
                let mut descriptors = model
                    .capabilities
                    .as_ref()
                    .map(catalog::descriptors)
                    .unwrap_or_default()
                    .to_vec();
                if !descriptors
                    .iter()
                    .any(|descriptor| catalog::option_id(descriptor) == catalog::option_id(&agent))
                {
                    descriptors.push(agent.clone());
                    model.capabilities = Some(catalog::capabilities(descriptors));
                }
            }
        }
        models
    }
    pub fn initial_snapshot(&self) -> ServiceResult<ServerProvider> {
        let checked = self.checked()?;
        let mut value = json!({"instanceId":self.spec.instance_id,"driver":"kiro","displayName":self.spec.display_name.as_deref().unwrap_or("Kiro"),"enabled":self.spec.enabled,"installed":self.spec.enabled,"version":null,"status":if self.spec.enabled{"warning"}else{"disabled"},"auth":{"status":"unknown"},"setup":{"canAuthenticate":false,"canInstall":false},"checkedAt":checked,"models":self.models(None,None),"skills":[],"slashCommands":[{"name":"compact","description":"Summarize the conversation and reduce context usage"}],"showInteractionModeToggle":false,"supportedRuntimeModes":["approval-required","full-access"],"supportsTextGeneration":false,"continuation":{"groupKey":self.spec.default_continuation_key()},"message":if self.spec.enabled{"Checking Kiro CLI availability..."}else{"Kiro is disabled in T3 Code settings."},"versionAdvisory":crate::maintenance::version_advisory(&self.manual(),None,None,Some(&checked))});
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
    fn manual(&self) -> crate::maintenance::MaintenanceCapabilities {
        crate::maintenance::MaintenanceCapabilities {
            driver: "kiro".into(),
            package: None,
            update: None,
            install_path: None,
            manual_command: None,
        }
    }
    async fn command(&self, args: &[&str]) -> ServiceResult<Command> {
        let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        let environment = probe_environment(&self.environment());
        let mut options = process_options(self.binary(), &args, &environment);
        options.cwd = Some(self.context.cwd.clone());
        options.check_exit = false;
        options.max_output_bytes = 4 * 1024 * 1024;
        options.timeout = Some(Duration::from_millis(crate::versions::probe_timeout_ms(
            "kiro",
            10_000,
            &environment,
        )));
        let output = t3_process::run(options)
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        if output.stdout_truncated {
            return Err(ServiceError::Response);
        }
        Ok(Command {
            code: output.code,
            stdout: Sensitive::new(output.stdout_text()),
        })
    }
    async fn agents(&self) -> Option<ProviderOptionDescriptor> {
        let environment = probe_environment(&self.environment());
        let spawn = ResolvedRegistryAgent {
            agent: None,
            distribution: None,
            executable: self.binary().into(),
            args: ACP_ARGS.iter().map(|arg| (*arg).into()).collect(),
            cwd: self.context.cwd.clone(),
            environment: Sensitive::new(environment.clone()),
        };
        let template = tokio::time::timeout(
            Duration::from_millis(crate::versions::probe_timeout_ms(
                "kiro",
                10_000,
                &environment,
            )),
            self.metadata.template(spawn),
        )
        .await
        .ok()?
        .ok()?;
        models::kiro_agent_descriptor(template.expose())
    }
    async fn listed(&self) -> Option<Vec<ServerProviderModel>> {
        let listed = self
            .command(&["chat", "--list-models", "--format", "json"])
            .await
            .ok()?;
        models::kiro_models_from_list(listed.code?, listed.stdout.expose())
            .ok()
            .flatten()
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
        let version = self
            .executables
            .version(
                crate::discovery::Driver::Kiro,
                &self.binary(),
                &probe_environment(&environment),
                &self.context.cwd,
            )
            .await;
        let version = match version {
            Ok(version) => version,
            Err(_) => {
                value["status"] = json!("error");
                value["message"] = json!("Failed to run `kiro-cli --version`.");
                return serde_json::from_value(value).map_err(crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ));
            }
        };
        value["installed"] = json!(version.installed);
        value["version"] = json!(version.version);
        value["versionAdvisory"] = json!(crate::maintenance::version_advisory(
            &self.manual(),
            version.version.as_deref(),
            None,
            Some(&self.checked()?)
        ));
        if !version.installed || version.status == t3_contracts::server::ServerProviderState::Error
        {
            value["status"] = json!("error");
            value["message"] = json!(if !version.installed {
                "Kiro CLI (`kiro-cli`) is not installed or not on PATH."
            } else {
                "Kiro CLI is installed but failed to run."
            });
            return serde_json::from_value(value).map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ));
        }
        let whoami = self.command(&["whoami", "--format", "json"]).await.ok();
        let auth = models::kiro_auth_from_whoami(
            whoami.as_ref().and_then(|output| output.code),
            whoami
                .as_ref()
                .map(|output| output.stdout.expose().as_str()),
            &environment,
        )
        .map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
        value["auth"] = json!(auth);
        value
            .as_object_mut()
            .ok_or(ServiceError::Response)?
            .remove("message");
        match auth.status {
            ServerProviderAuthStatus::Unauthenticated => {
                value["status"] = json!("error");
                value["message"] =
                    json!("Kiro CLI is installed but not signed in. Run `kiro-cli login`.");
            }
            ServerProviderAuthStatus::Authenticated => {
                let (listed, agents) = tokio::join!(self.listed(), self.agents());
                value["status"] = json!("ready");
                value["models"] = json!(self.models(listed, agents));
            }
            _ => {
                value["status"] = json!("warning");
                value["message"] =
                    json!("Could not confirm the Kiro sign-in. Run `kiro-cli whoami` to check.");
            }
        }
        if let Some(advisory) = crate::versions::resolve_compatibility(
            self.manifest
                .current()
                .compatibility
                .as_deref()
                .unwrap_or_default(),
            "kiro",
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
impl ProviderInstanceService for KiroProvider {
    fn instance_id(&self) -> &ProviderInstanceId {
        &self.spec.instance_id
    }
    async fn snapshot(&self, _: Option<&str>, _: bool) -> ServiceResult<ServerProvider> {
        self.check().await
    }
    async fn maintenance(
        &self,
        _: bool,
    ) -> ServiceResult<crate::maintenance::MaintenanceCapabilities> {
        Ok(self.manual())
    }
}
