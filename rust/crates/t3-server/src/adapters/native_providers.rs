//! The orchestration runtime's provider adapters, built from the provider domain's instances.
//!
//! TypeScript builds each instance's orchestration adapter inside its driver
//! (`ProviderInstanceRegistry` → `CodexDriver`/`ClaudeDriver` → `createCodexAdapterV2`/...) and
//! the runtime reads them through `ProviderAdapterRegistryV2.layerFromProviderInstanceRegistry`.
//! Here the runtime starts before the provider domain (the domain retires sessions through the
//! runtime), so the registry reads a late-bound source: fixed registrations (Replay (dev),
//! adapters a host injects) from the start, and the domain's prepared instances once
//! [`ProviderSource::bind`] has run. A reconfigured instance gets a new `InstanceSpec`, hence a
//! new adapter; the domain retires the old instance's sessions itself.
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use t3_orchestration::ports::sessions_recovery::provider_adapter_registry::{
    AdapterRegistration, ProviderInstanceSource,
};

/// The registry's instance source: fixed registrations, then the bound native source.
pub struct ProviderSource {
    fixed: Vec<AdapterRegistration>,
    native: RwLock<Option<Arc<dyn ProviderInstanceSource>>>,
}

impl ProviderSource {
    pub fn new(fixed: Vec<AdapterRegistration>) -> Arc<Self> {
        Arc::new(Self {
            fixed,
            native: RwLock::new(None),
        })
    }

    /// Binds the provider domain's instances.
    pub fn bind(&self, native: Arc<dyn ProviderInstanceSource>) {
        *self.native.write().unwrap_or_else(|e| e.into_inner()) = Some(native);
    }

    /// Drops the provider domain's instances (and the domain handle they hold): afterwards no
    /// native session can open.
    pub fn unbind(&self) {
        self.native
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }

    fn native(&self) -> Option<Arc<dyn ProviderInstanceSource>> {
        self.native
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Shutdown ordering for native providers, registered right after the provider domain so it
/// runs before the domain's shutdown: the orchestration runtime stops first (admission, worker,
/// provider sessions and their processes) while the domain is alive, then the source drops the
/// domain. Orchestration's own later shutdown step is then a no-op.
pub struct NativeProvidersShutdown {
    pub orchestration: Arc<crate::adapters::orchestration::OrchestrationAdapter>,
    pub source: Arc<ProviderSource>,
}

impl crate::services::DomainRuntime for NativeProvidersShutdown {
    fn name(&self) -> &'static str {
        "native providers"
    }
    fn shutdown(&self) -> futures::future::BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            let stopped =
                crate::services::DomainRuntime::shutdown(self.orchestration.as_ref()).await;
            self.source.unbind();
            stopped
        })
    }
}

#[async_trait]
impl ProviderInstanceSource for ProviderSource {
    async fn instances(&self) -> t3_orchestration::error::Result<Vec<AdapterRegistration>> {
        let mut registrations = self.fixed.clone();
        if let Some(native) = self.native() {
            for registration in native.instances().await? {
                let id = registration.adapter.instance_id();
                if !registrations
                    .iter()
                    .any(|fixed| fixed.adapter.instance_id() == id)
                {
                    registrations.push(registration);
                }
            }
        }
        Ok(registrations)
    }
}

#[cfg(feature = "providers")]
pub use native::NativeProviders;

#[cfg(feature = "providers")]
mod native {
    use super::*;
    use std::{collections::BTreeMap, path::PathBuf, time::Duration};
    use t3_claude_cli::launch::LaunchOptions;
    use t3_contracts::{
        provider_instance::{ProviderDriverKind, ProviderInstanceId},
        provider_setup::{ProviderSetupError, ProviderSetupErrorTag},
    };
    use t3_orchestration::{
        adapters::{
            claude::{
                ClaudeAdapterOptions, ClaudeAttachments, ClaudeHostOptions,
                DirectoryClaudeAttachments, ProcessConnector, claude_registration,
            },
            codex::{
                CodexAdapterOptions, CodexAdapterV2, CodexAttachmentResolver,
                ResolvedCodexAttachment, codex_registration,
            },
        },
        api::{Clock, IdGenerator},
        provider::*,
    };
    use t3_provider::{
        domain::ProviderDomain,
        environment::{
            Environment, Platform, claude_home, claude_process_environment, codex_home_layout,
            expand_home_path, process_options,
        },
        instances::{InstanceSpec, InstanceStatus},
        sensitive::ServiceError,
    };
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    /// What every native launch shares: the server's environment, account home and cwd
    /// (TypeScript `HostProcess.Environment`, `HostProcess.HomeDirectory`, `process.cwd()`),
    /// and the attachments directory (`ServerConfig.attachmentsDir`).
    #[derive(Clone)]
    struct Host {
        environment: Environment,
        home: PathBuf,
        cwd: PathBuf,
        platform: Platform,
        attachments: PathBuf,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    }

    /// The provider domain's prepared Codex and Claude instances as adapter registrations.
    /// Other drivers have no native turn adapter in this server yet and are not listed, so a
    /// turn on them fails with the registry's "No orchestration provider adapter is
    /// registered" error rather than running on something else.
    pub struct NativeProviders {
        domain: Arc<ProviderDomain>,
        host: Host,
        built: Mutex<BTreeMap<ProviderInstanceId, (Arc<InstanceSpec>, AdapterRegistration)>>,
    }

    impl NativeProviders {
        pub fn new(
            domain: Arc<ProviderDomain>,
            context: &crate::services::DomainContext<'_>,
            clock: Arc<dyn Clock>,
            ids: Arc<dyn IdGenerator>,
        ) -> Arc<Self> {
            Arc::new(Self {
                domain,
                host: Host {
                    environment: context.environment.clone(),
                    home: context.account_home.clone(),
                    cwd: context.local.config.cwd.clone(),
                    platform: Platform::current(),
                    attachments: context.local.config.paths.attachments(),
                    clock,
                    ids,
                },
                built: Mutex::new(BTreeMap::new()),
            })
        }

        fn build(&self, spec: &Arc<InstanceSpec>) -> Option<AdapterRegistration> {
            let built = match spec.driver.0.as_str() {
                "codex" => codex(&self.host, &self.domain, spec),
                "claudeAgent" => claude(&self.host, spec),
                _ => return None,
            };
            match built {
                Ok(registration) => Some(registration),
                Err(reason) => {
                    // The reason is fixed text; paths and environment values stay out of logs.
                    tracing::warn!(instance = %spec.instance_id.0, reason, "provider instance has no turn adapter");
                    None
                }
            }
        }
    }

    #[async_trait]
    impl ProviderInstanceSource for NativeProviders {
        async fn instances(&self) -> t3_orchestration::error::Result<Vec<AdapterRegistration>> {
            let entries = self.domain.instances.list().await;
            let mut built = self.built.lock().await;
            built.retain(|id, _| entries.contains_key(id));
            let mut registrations = Vec::new();
            for (id, entry) in entries {
                let InstanceStatus::Prepared(spec) = entry.status else {
                    built.remove(&id);
                    continue;
                };
                if let Some((current, registration)) = built.get(&id)
                    && Arc::ptr_eq(current, &spec)
                {
                    registrations.push(registration.clone());
                    continue;
                }
                match self.build(&spec) {
                    Some(registration) => {
                        built.insert(id, (spec, registration.clone()));
                        registrations.push(registration);
                    }
                    None => {
                        built.remove(&id);
                    }
                }
            }
            Ok(registrations)
        }
    }

    fn text<'a>(config: &'a serde_json::Value, key: &str) -> &'a str {
        config[key].as_str().unwrap_or_default()
    }

    /// `createCodexAdapterV2` (direct) or the managed runtime (`setupMode: "managed"`).
    fn codex(
        host: &Host,
        domain: &Arc<ProviderDomain>,
        spec: &Arc<InstanceSpec>,
    ) -> Result<AdapterRegistration, &'static str> {
        let environment = spec.environment(&host.environment, &host.home, host.platform);
        let config = &spec.config;
        // CodexDriver: an empty `homePath` falls back to the instance environment's CODEX_HOME.
        let configured = text(config, "homePath");
        let home_path = if t3_contracts::wire::trim_js(configured).is_empty() {
            environment
                .get("CODEX_HOME")
                .map(String::as_str)
                .unwrap_or(configured)
        } else {
            configured
        };
        let layout = codex_home_layout(
            home_path,
            text(config, "shadowHomePath"),
            &host.home,
            &host.cwd,
        );
        let options = CodexAdapterOptions::new(
            spec.instance_id.clone(),
            host.clock.clone(),
            host.ids.clone(),
        );
        if config["setupMode"] == "managed" {
            let adapter = ManagedCodexAdapter {
                options,
                attachments: Some(Arc::new(CodexAttachments(DirectoryClaudeAttachments::new(
                    host.attachments.clone(),
                )))),
                domain: Arc::downgrade(domain),
            };
            return Ok(AdapterRegistration {
                adapter: Arc::new(adapter),
                continuation_key: layout.continuation_key,
                enabled: spec.enabled,
                credential_access: Vec::new(),
            });
        }
        t3_provider::homes::materialize_codex_shadow(&layout)
            .map_err(|_| "Failed to materialize the Codex shadow home.")?;
        let mut options = options;
        options.attachments = Some(Arc::new(CodexAttachments(DirectoryClaudeAttachments::new(
            host.attachments.clone(),
        ))));
        options.process = codex_process(
            &expand_home_path(text(config, "binaryPath"), &host.home),
            layout
                .effective_home
                .as_ref()
                .map(|home| home.to_string_lossy().into_owned()),
            text(config, "launchArgs"),
            environment,
            host,
        );
        Ok(codex_registration(
            options,
            layout.continuation_key,
            spec.enabled,
            Vec::new(),
        ))
    }

    /// `layerAppServerClientFactory.open`: `codex app-server` with the instance environment,
    /// the T3 originator unless the instance sets one, and CODEX_HOME for an explicit home.
    fn codex_process(
        binary_path: &str,
        effective_home: Option<String>,
        launch_args: &str,
        environment: Environment,
        host: &Host,
    ) -> t3_process::ProcessOptions {
        use t3_codex_app_server::launch::{
            LaunchInput, Platform as LaunchPlatform, app_server_launch,
        };
        // The program is resolved by `t3_process::spawn` (PATH, PATHEXT and batch quoting), so
        // the description is built for a plain `codex` and only its arguments and environment
        // are used.
        let launch = app_server_launch(LaunchInput {
            platform: if host.platform == Platform::Windows {
                LaunchPlatform::Windows
            } else {
                LaunchPlatform::Posix
            },
            binary_path: "codex".into(),
            resolved_executable: None,
            home_directory: host.home.to_string_lossy().into_owned(),
            effective_codex_home: effective_home,
            launch_args: Some(launch_args.into()),
            host_environment: environment,
            instance_environment: Vec::new(),
            cwd: None,
            extend_env: None,
        });
        let program = if binary_path.is_empty() {
            "codex"
        } else {
            binary_path
        };
        let mut process = process_options(program, &launch.args, &launch.environment);
        process.timeout = None;
        process.termination_grace = launch.force_kill_after;
        process
    }

    /// `createClaudeAdapterV2`: the Claude process environment, CLAUDE_CONFIG_DIR for an
    /// explicit home, and an executable the SDK can spawn without a shell.
    fn claude(host: &Host, spec: &Arc<InstanceSpec>) -> Result<AdapterRegistration, &'static str> {
        let config = &spec.config;
        let mut environment = claude_process_environment(
            &host.environment,
            &spec.variables,
            &host.home,
            host.platform,
            config["entrypoint"].as_str(),
        );
        let home_path = text(config, "homePath");
        let claude_home = claude_home(home_path, &environment, &host.home, &host.cwd);
        if !t3_contracts::wire::trim_js(home_path).is_empty() {
            environment.insert(
                "CLAUDE_CONFIG_DIR".into(),
                claude_home.to_string_lossy().into_owned(),
            );
        }
        let binary = match text(config, "binaryPath") {
            "" => "claude".to_owned(),
            configured => expand_home_path(configured, &host.home),
        };
        let binary = claude_sdk_executable(binary, &environment, host.platform);
        let launch = LaunchOptions {
            cli_path: Some(binary),
            env: environment,
            ..Default::default()
        };
        let options = ClaudeAdapterOptions {
            instance_id: spec.instance_id.clone(),
            launch,
            connector: Arc::new(ProcessConnector),
            clock: host.clock.clone(),
            ids: host.ids.clone(),
            new_native_session_id: Arc::new(|| uuid::Uuid::new_v4().to_string()),
            mcp_servers: BTreeMap::new(),
            transcript_root: Some(claude_home.join("projects")),
            attachments: Arc::new(DirectoryClaudeAttachments::new(host.attachments.clone())),
            host: ClaudeHostOptions {
                home_directory: Some(host.home.clone()),
                settings_directory: Some(claude_home.clone()),
                attachment_directory: Some(host.attachments.clone()),
                ..Default::default()
            },
        };
        Ok(claude_registration(
            options,
            format!("claude:home:{}", claude_home.display()),
            spec.enabled,
            Vec::new(),
        ))
    }

    /// `resolveClaudeSdkExecutablePath`: on Windows the SDK spawns without a shell, so a bare
    /// name is resolved through PATH and a launcher shim is replaced by the package entry
    /// beside it (`bin/claude.exe`, else `cli.js`).
    fn claude_sdk_executable(
        binary: String,
        environment: &Environment,
        platform: Platform,
    ) -> String {
        if platform != Platform::Windows {
            return binary;
        }
        let patch = environment
            .iter()
            .map(|(key, value)| (key.into(), Some(value.into())))
            .collect();
        let resolved = t3_process::resolve_command(binary.as_ref(), &[], &patch, false)
            .map(|command| PathBuf::from(command.program))
            .unwrap_or_else(|_| PathBuf::from(&binary));
        let shim = resolved
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "cmd" | "bat" | "ps1"
                )
            });
        if !shim {
            return resolved.to_string_lossy().into_owned();
        }
        let Some(directory) = resolved.parent() else {
            return binary;
        };
        let package = directory.join("node_modules/@anthropic-ai/claude-code");
        for candidate in [package.join("bin/claude.exe"), package.join("cli.js")] {
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
        tracing::warn!(
            "Claude launcher shim resolved but no known package entry was found next to it; the Claude Agent SDK cannot spawn launcher scripts directly."
        );
        binary
    }

    /// Codex reads attachments by saved path; images also travel inline as a data URL
    /// (`CodexAdapterV2` `resolveAttachmentPath` + `readFile`).
    struct CodexAttachments(DirectoryClaudeAttachments);

    #[async_trait]
    impl CodexAttachmentResolver for CodexAttachments {
        async fn resolve(
            &self,
            attachment: &t3_contracts::chat_attachment::ChatAttachment,
        ) -> ProviderResult<Option<ResolvedCodexAttachment>> {
            use base64::Engine;
            use t3_contracts::chat_attachment::ChatAttachment;
            let Some(path) = self.0.path(attachment) else {
                return Ok(None);
            };
            let image_data_url = match attachment {
                ChatAttachment::Image(image) => match tokio::fs::read(&path).await {
                    Ok(bytes) => Some(format!(
                        "data:{};base64,{}",
                        image.mime_type,
                        base64::engine::general_purpose::STANDARD.encode(bytes)
                    )),
                    Err(_) => {
                        return Err(ProviderError::Protocol {
                            reason: Some(ProtocolReason::AttachmentUnreadable),
                            detail: format!("Failed to read attachment '{}'.", image.id),
                            payload: None,
                        });
                    }
                },
                _ => None,
            };
            Ok(Some(ResolvedCodexAttachment {
                extracted_text_path: self.0.extracted_text_path(&path),
                path,
                image_data_url,
            }))
        }
    }

    fn setup_error(instance: &ProviderInstanceId, detail: &str) -> ProviderError {
        ProviderError::Setup(Box::new(ProviderSetupError {
            _tag: ProviderSetupErrorTag::Error,
            instance_id: instance.clone(),
            operation: "session".into(),
            detail: detail.into(),
            cause: None,
        }))
    }

    /// Managed Codex (`CodexManagedProvider.resolveRuntime`): each session resolves the
    /// installed executable, the signed-in profile and its token helper, and holds the
    /// executable lease and credential access until the session ends. A sign-in change cancels
    /// the access, which stops the session's process.
    struct ManagedCodexAdapter {
        options: CodexAdapterOptions,
        attachments: Option<Arc<dyn CodexAttachmentResolver>>,
        domain: std::sync::Weak<ProviderDomain>,
    }

    impl ManagedCodexAdapter {
        async fn open(
            &self,
            input: OpenSession,
            policy: Option<RuntimePolicyOptions>,
            cancellation: CancellationToken,
        ) -> ProviderResult<Arc<dyn SessionRuntime>> {
            let instance = &self.options.instance_id;
            let runtime = match self.domain.upgrade() {
                Some(domain) => domain.managed_codex.runtime(instance).await,
                None => None,
            }
            .ok_or_else(|| {
                setup_error(instance, "This provider instance is no longer available.")
            })?;
            let effective = runtime.resolve().await.map_err(|error| match error {
                ServiceError::Busy | ServiceError::SharedBinding => setup_error(
                    instance,
                    "This provider's sign-in is changing. Try again after it finishes.",
                ),
                other => setup_error(instance, &other.to_string()),
            })?;
            let mut options = self.options.clone();
            options.attachments = self.attachments.clone();
            let mut environment = effective.environment.clone();
            environment
                .entry("CODEX_INTERNAL_ORIGINATOR_OVERRIDE".into())
                .or_insert_with(|| "T3 Code".into());
            let mut args = vec![
                "app-server".to_owned(),
                "-c".into(),
                "thread_unload_delay_secs=0".into(),
            ];
            args.extend(t3_codex_app_server::launch::tokenize_cli_args(
                &effective.launch_args,
            ));
            options.process =
                process_options(&effective.executable.executable.path, &args, &environment);
            options.process.timeout = None;
            options.process.termination_grace = Duration::from_secs(2);
            let session = cancellation.child_token();
            let access = effective.cancellation();
            let adapter = CodexAdapterV2::new(options);
            let opened = match policy {
                Some(policy) => {
                    adapter
                        .open_session_with_policy(input, policy, session.clone())
                        .await
                }
                None => adapter.open_session(input, session.clone()).await,
            };
            if opened.is_err() {
                return opened;
            }
            // The session manager cancels the token it passed when it closes the session.
            tokio::spawn(async move {
                tokio::select! {
                    () = access.cancelled() => session.cancel(),
                    () = session.cancelled() => {}
                }
                drop(effective);
            });
            opened
        }
    }

    #[async_trait]
    impl ProviderAdapter for ManagedCodexAdapter {
        fn instance_id(&self) -> &ProviderInstanceId {
            &self.options.instance_id
        }
        fn driver(&self) -> ProviderDriverKind {
            ProviderDriverKind("codex".into())
        }
        fn supported_runtime_modes(
            &self,
        ) -> Option<Vec<t3_contracts::provider_policy::RuntimeMode>> {
            CodexAdapterV2::new(self.options.clone()).supported_runtime_modes()
        }
        async fn capabilities(
            &self,
        ) -> ProviderResult<t3_contracts::orchestration_v2::OrchestrationV2ProviderCapabilities>
        {
            CodexAdapterV2::new(self.options.clone())
                .capabilities()
                .await
        }
        async fn plan_selection_transition(
            &self,
            input: SelectionTransitionInput,
        ) -> ProviderResult<SelectionTransitionPlan> {
            CodexAdapterV2::new(self.options.clone())
                .plan_selection_transition(input)
                .await
        }
        async fn open_session(
            &self,
            input: OpenSession,
            cancellation: CancellationToken,
        ) -> ProviderResult<Arc<dyn SessionRuntime>> {
            self.open(input, None, cancellation).await
        }
        async fn open_session_with_policy(
            &self,
            input: OpenSession,
            options: RuntimePolicyOptions,
            cancellation: CancellationToken,
        ) -> ProviderResult<Arc<dyn SessionRuntime>> {
            self.open(input, Some(options), cancellation).await
        }
    }
}
