//! The ACP Registry application host (TypeScript `AcpRegistrySessions` and the handler helpers
//! in `ws.ts`): projects, imported-thread mappings, live sessions and status refreshes for the
//! provider domain's `AcpRegistryService`.
//!
//! The provider domain is built with this host and owns it, so the host holds the domain weakly
//! and is bound once the domain exists. Every working directory it hands out is a project root
//! from the projection, never a path from a client.
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, Weak},
};
use t3_contracts::{
    acp_registry::AcpRegistryOperationErrorReason as Reason,
    base_schemas::{ProjectId, ThreadId},
    orchestration_v2::OrchestrationV2ThreadHistoryOrigin,
    provider_instance::ProviderInstanceId,
};
use t3_orchestration::composition::OrchestrationRuntime;
use t3_provider::{acp_service::*, domain::ProviderDomain};

pub struct AcpHost {
    runtime: Arc<OrchestrationRuntime>,
    domain: OnceLock<Weak<ProviderDomain>>,
}

impl AcpHost {
    pub fn new(runtime: Arc<OrchestrationRuntime>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            domain: OnceLock::new(),
        })
    }

    /// Binds the provider domain built with this host.
    pub fn bind(&self, domain: &Arc<ProviderDomain>) {
        let _ = self.domain.set(Arc::downgrade(domain));
    }

    fn domain(&self) -> AcpResult<Arc<ProviderDomain>> {
        self.domain.get().and_then(Weak::upgrade).ok_or_else(|| {
            AcpError::new(
                Reason::InstanceNotFound,
                "Provider services are not available.",
            )
        })
    }
}

#[async_trait::async_trait]
impl AcpSessionHost for AcpHost {
    /// `AcpRegistrySessions.project`: a live (not deleted) project's workspace root.
    async fn project_workspace(&self, project: &ProjectId) -> AcpResult<PathBuf> {
        let id = project.clone();
        let row = self
            .runtime
            .projections
            .read_snapshot(move |db| t3_orchestration::stores::projects::get(db, &id, false))
            .await
            .map_err(|_| AcpError::new(Reason::ProjectNotFound, "The project is unavailable."))?;
        row.map(|row| PathBuf::from(row.workspace_root))
            .ok_or_else(|| AcpError::new(Reason::ProjectNotFound, "The project was not found."))
    }

    /// `AcpRegistrySessions.sessionManager`.
    async fn instance(&self, instance: &ProviderInstanceId) -> AcpResult<AcpInstance> {
        let domain = self.domain()?;
        if domain.instances.get(instance).await.is_none() {
            return Err(AcpError::new(
                Reason::InstanceNotFound,
                "The provider instance was not found.",
            ));
        }
        let Some(registry) = domain.acp_instances.get(instance) else {
            return Err(AcpError::new(
                Reason::SessionListUnsupported,
                "The provider instance does not expose ACP session management.",
            ));
        };
        let snapshot = registry.snapshot().await.map_err(|_| {
            AcpError::new(
                Reason::InstanceNotFound,
                "The provider instance is unavailable.",
            )
        })?;
        Ok(AcpInstance {
            driver: registry.driver.clone(),
            snapshot,
            manager: registry.manager(),
            // Every registry instance has a sign-in controller (`AcpRegistryDriver`'s `auth`).
            managed_auth: true,
        })
    }

    /// `threadManagement.getThreadShell` plus `AgentSessionImporter.isNativeHistoryPending`.
    async fn thread(&self, thread: &ThreadId) -> AcpResult<Option<ImportedThread>> {
        let id = thread.clone();
        self.runtime
            .projections
            .read_snapshot(move |db| {
                let Some(thread) = t3_orchestration::projections::read::thread(db, &id)? else {
                    return Ok(None);
                };
                let has_run: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_runs WHERE thread_id=?1)",
                    [&id.0],
                    |row| row.get(0),
                )?;
                let imported = thread.history_origin.flatten()
                    == Some(OrchestrationV2ThreadHistoryOrigin::V1Import);
                Ok(Some(ImportedThread {
                    native_history_pending: !imported && !has_run,
                }))
            })
            .await
            .map_err(|_| {
                AcpError::new(
                    Reason::SessionImportFailed,
                    "Could not inspect existing imported ACP sessions.",
                )
            })
    }

    /// `threadLaunch.launch` with the imported native thread, as the user, from `web`.
    async fn launch_import(&self, import: ImportLaunch) -> AcpResult<()> {
        use t3_orchestration::ports::checkpoints_forks::thread_launch_service::LaunchInput;
        let launch = import.launch;
        self.runtime
            .launch_thread(LaunchInput {
                command_id: launch.command_id,
                thread_id: launch.thread_id.flatten(),
                reuse_existing_thread: launch.reuse_existing_thread.flatten() == Some(true),
                project_id: launch.project_id,
                title: launch.title.0,
                generate_title: launch.generate_title.flatten() == Some(true),
                model_selection: launch.model_selection,
                runtime_mode: launch.runtime_mode,
                interaction_mode: launch.interaction_mode,
                workspace_strategy: launch.workspace_strategy,
                initial_message: None,
                imported_native_thread: Some(import.native),
                created_by: import.created_by,
                creation_source: launch
                    .creation_source
                    .flatten()
                    .unwrap_or(t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web),
                link_origin: None,
                delegated_from: None,
            })
            .await
            .map(|_| ())
            .map_err(|_| {
                AcpError::new(
                    Reason::SessionImportFailed,
                    "Could not create a T3 thread for the ACP session.",
                )
            })
    }

    /// `AgentSessionImporter.importNativeHistory`.
    async fn import_history(&self, thread: &ThreadId, cwd: &Path) -> AcpResult<()> {
        t3_orchestration::native_history::import_native_history(&self.runtime, thread, cwd)
            .await
            .map(|_| ())
            .map_err(|_| {
                // The cause can carry agent text; it stays out of the log and the client frame.
                tracing::warn!(thread = %thread.0, "Could not import ACP session history");
                AcpError::new(
                    Reason::SessionImportFailed,
                    "Could not import the ACP session history.",
                )
            })
    }

    /// `providerSessionManager.closeInstance`.
    async fn close_sessions(&self, instance: &ProviderInstanceId) -> AcpResult<()> {
        self.runtime
            .sessions
            .close_instance(instance)
            .await
            .map_err(|_| {
                AcpError::new(
                    Reason::ProviderConfigurationFailed,
                    "Could not stop live sessions.",
                )
            })
    }

    /// `providerRegistry.refreshInstance`: a fresh status check; it never fails the operation.
    async fn refresh(&self, instance: &ProviderInstanceId) -> AcpResult<()> {
        let domain = self.domain()?;
        let Ok(input) =
            serde_json::from_value(serde_json::json!({"instanceId": instance, "fresh": true}))
        else {
            return Ok(());
        };
        if domain.providers.refresh_providers(input).await.is_err() {
            tracing::warn!(instance = %instance.0, "ACP Registry status refresh failed");
        }
        Ok(())
    }
}

/// Registry turns (TypeScript `AcpRegistryAdapterV2Driver`): the generic ACP adapter in the
/// registry flavour, launched from the instance's resolved registry entry and bound to the
/// provider domain's runtime coordinator.
mod turns {
    use std::{path::PathBuf, sync::Arc};
    use t3_contracts::provider_instance::ProviderInstanceId;
    use t3_orchestration::{
        adapters::acp::{
            AcpRegistryAgent, AcpRuntimeHooks, AcpTransport, AcpTransportFactory,
            ProcessTransportFactory, acp_registry_adapter,
        },
        api::{Clock, IdGenerator},
        ports::sessions_recovery::provider_adapter_registry::AdapterRegistration,
        provider::{OpenSession, ProviderError, ProviderResult},
    };
    use t3_provider::{
        acp_catalog,
        acp_coordinator::AcpRuntimeCoordinator,
        domain::ProviderDomain,
        environment::{Environment, Platform, process_options},
        instances::InstanceSpec,
        registry_service::RegistryAgentCatalog,
        sensitive::Sensitive,
    };
    use tokio_util::sync::CancellationToken;

    fn launch_error(instance: &ProviderInstanceId) -> ProviderError {
        ProviderError::Setup(Box::new(t3_contracts::provider_setup::ProviderSetupError {
            _tag: t3_contracts::provider_setup::ProviderSetupErrorTag::Error,
            instance_id: instance.clone(),
            operation: "session".into(),
            detail: "Could not start the ACP agent.".into(),
            cause: None,
        }))
    }

    /// `makeAcpRegistryRuntime`: each session resolves the agent from the instance's settings
    /// (registry entry and managed installation, or the local command its settings carry) and
    /// runs it with only the resolved environment, in the thread's working directory.
    struct RegistryTransport {
        instance: ProviderInstanceId,
        catalog: Arc<RegistryAgentCatalog>,
        settings: serde_json::Value,
        environment: Sensitive<Environment>,
        cwd: PathBuf,
    }

    #[async_trait::async_trait]
    impl AcpTransportFactory for RegistryTransport {
        async fn connect(
            &self,
            input: &OpenSession,
            cancellation: CancellationToken,
        ) -> ProviderResult<AcpTransport> {
            let cwd = input
                .runtime_policy
                .cwd
                .clone()
                .unwrap_or_else(|| self.cwd.clone());
            let resolved = self
                .catalog
                .resolve(&self.settings, &cwd, self.environment.expose())
                .await
                .map_err(|_| launch_error(&self.instance))?;
            let mut options = process_options(
                &resolved.executable,
                &resolved.args,
                resolved.environment.expose(),
            );
            options.cwd = Some(cwd);
            options.timeout = None;
            ProcessTransportFactory { options }
                .connect(input, cancellation)
                .await
        }
    }

    /// The flavour's coordinator callbacks (`onUrlElicitation`, `onAvailableCommandsUpdate`,
    /// `onSessionConfigurationUpdate`, `withRuntimeStartup`).
    struct CoordinatorHooks {
        coordinator: Arc<AcpRuntimeCoordinator>,
        instance: ProviderInstanceId,
        startup_key: String,
    }

    #[async_trait::async_trait]
    impl AcpRuntimeHooks for CoordinatorHooks {
        async fn url_consent(&self, _: &str, url: &str, message: &str) -> bool {
            // The one consent rule (`acp_catalog::consent_action`): web address without
            // credentials, cleaned message, an id the server mints.
            let Some(action) = acp_catalog::consent_action(url, message) else {
                return false;
            };
            self.coordinator
                .request_url_auth(self.instance.clone(), Sensitive::new(action))
                .await
        }
        fn available_commands(&self, commands: &[serde_json::Value]) {
            self.coordinator
                .publish_commands(self.instance.clone(), acp_catalog::commands(commands));
        }
        fn session_configuration(&self, configuration: &serde_json::Value) {
            self.coordinator.publish_configuration(
                self.instance.clone(),
                acp_catalog::live_configuration(configuration),
            );
        }
        fn startup(&self) -> Option<Box<dyn std::any::Any + Send>> {
            Some(Box::new(
                self.coordinator.foreground_startup(&self.startup_key),
            ))
        }
    }

    /// The host values every native launch shares (`native_providers.rs`' `Host`).
    pub struct TurnHost<'a> {
        pub environment: &'a Environment,
        pub home: &'a std::path::Path,
        pub cwd: &'a std::path::Path,
        pub attachments: Option<t3_orchestration::adapters::acp::AcpAttachmentSource>,
        pub platform: Platform,
        pub clock: Arc<dyn Clock>,
        pub ids: Arc<dyn IdGenerator>,
    }

    /// The instance's turn adapter. Like TypeScript, an instance whose settings name no agent
    /// (or whose launch fields a managed ceiling locked) still has one, so admission refusals
    /// come first and a turn that gets through fails at launch with a typed setup error.
    pub fn registration(
        domain: &Arc<ProviderDomain>,
        spec: &Arc<InstanceSpec>,
        host: TurnHost<'_>,
    ) -> Result<AdapterRegistration, &'static str> {
        let config = &spec.config;
        let local = config["source"] == "local";
        let agent = AcpRegistryAgent {
            agent_id: if local {
                String::new()
            } else {
                t3_contracts::wire::trim_js(config["agentId"].as_str().unwrap_or_default()).into()
            },
            local_command: local.then(|| {
                config["commandPath"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
            }),
        };
        let mut environment = spec.environment(host.environment, host.home, host.platform);
        if agent.is_claude() {
            // Claude Agent runs ACP v2 unless the instance environment decides.
            environment
                .entry("CLAUDE_AGENT_ACP_EXPERIMENTAL_V2".into())
                .or_insert_with(|| "1".into());
        }
        let startup_key = if local {
            format!("local:{}", spec.instance_id.0)
        } else {
            agent.agent_id.clone()
        };
        let adapter = acp_registry_adapter(
            spec.instance_id.clone(),
            &agent,
            Arc::new(RegistryTransport {
                instance: spec.instance_id.clone(),
                catalog: domain.registry_catalog.clone(),
                settings: config.clone(),
                environment: Sensitive::new(environment),
                cwd: host.cwd.into(),
            }),
            Some(Arc::new(CoordinatorHooks {
                coordinator: domain.coordinator.clone(),
                instance: spec.instance_id.clone(),
                startup_key,
            })),
            host.clock,
            host.ids,
        )
        .with_attachments(host.attachments);
        Ok(AdapterRegistration {
            adapter: Arc::new(adapter),
            continuation_key: spec.default_continuation_key(),
            enabled: spec.enabled,
            credential_access: Vec::new(),
        })
    }
}
pub use turns::{TurnHost, registration as turn_registration};
