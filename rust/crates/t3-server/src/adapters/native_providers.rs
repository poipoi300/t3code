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
        /// Each thread's T3 MCP server (`McpProviderSessions`), read when a session configures it.
        t3_mcp: Option<Arc<dyn t3_orchestration::provider::T3McpSessions>>,
        /// The live model manifest (`ModelCatalog.current`), read on each model compile.
        manifest: t3_provider::catalog::ManifestSource,
        /// Preview port discovery's running-agent lease (`PortDiscovery.trackAgent`).
        track_agent: Option<t3_orchestration::adapters::codex::CodexAgentTracker>,
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
            t3_mcp: Option<Arc<dyn t3_orchestration::provider::T3McpSessions>>,
            track_agent: Option<t3_orchestration::adapters::codex::CodexAgentTracker>,
        ) -> Arc<Self> {
            let manifest = domain.manifest.source();
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
                    t3_mcp,
                    manifest,
                    track_agent,
                },
                built: Mutex::new(BTreeMap::new()),
            })
        }

        fn build(&self, spec: &Arc<InstanceSpec>) -> Option<AdapterRegistration> {
            let built = match spec.driver.0.as_str() {
                "codex" => codex(&self.host, &self.domain, spec),
                "claudeAgent" => claude(&self.host, spec),
                "acpRegistry" => crate::adapters::acp_registry::turn_registration(
                    &self.domain,
                    spec,
                    crate::adapters::acp_registry::TurnHost {
                        environment: &self.host.environment,
                        home: &self.host.home,
                        cwd: &self.host.cwd,
                        platform: self.host.platform,
                        clock: self.host.clock.clone(),
                        ids: self.host.ids.clone(),
                    },
                ),
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
        let options = CodexAdapterOptions {
            t3_mcp: host.t3_mcp.clone(),
            version_gate: Some(codex_version_gate(
                host.manifest.clone(),
                spec.instance_id.clone(),
            )),
            track_agent: host.track_agent.clone(),
            ..CodexAdapterOptions::new(
                spec.instance_id.clone(),
                host.clock.clone(),
                host.ids.clone(),
            )
        };
        if config["setupMode"] == "managed" {
            let adapter = ManagedCodexAdapter {
                options,
                attachments: Some(Arc::new(CodexAttachments::new(host.attachments.clone()))),
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
        options.attachments = Some(Arc::new(CodexAttachments::new(host.attachments.clone())));
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
            None,
        );
        let home_path = text(config, "homePath");
        let claude_home = claude_home(home_path, &environment, &host.home, &host.cwd);
        // `mergeClaudeProcessEnvironment`: Claude Code reads its entrypoint from the process
        // environment, so `env.CLAUDE_CODE_ENTRYPOINT` in the instance's Claude settings (then
        // managed settings) overrides T3's. The files are untrusted: the reader caps their size,
        // ignores a file whose value is not a string, and the value is only an environment value.
        if let Some(entrypoint) = t3_provider::claude_skills::settings_environment(
            &claude_home,
            host.platform,
            &environment,
        )
        .get("CLAUDE_CODE_ENTRYPOINT")
        .and_then(serde_json::Value::as_str)
        .filter(|entrypoint| !entrypoint.is_empty())
        {
            environment.insert("CLAUDE_CODE_ENTRYPOINT".into(), entrypoint.to_owned());
        }
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
                t3_mcp: host.t3_mcp.clone(),
                compile_model: Some(claude_model_compiler(
                    host.manifest.clone(),
                    config["customModels"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                )),
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
    struct CodexAttachments(Arc<dyn ClaudeAttachments>);

    impl CodexAttachments {
        fn new(root: PathBuf) -> Self {
            Self(Arc::new(DirectoryClaudeAttachments::new(root)))
        }
    }

    #[async_trait]
    impl CodexAttachmentResolver for CodexAttachments {
        fn session_resolver(&self) -> ProviderResult<Option<Arc<dyn CodexAttachmentResolver>>> {
            let (files, _) = self.0.for_session()?;
            Ok(Some(Arc::new(Self(files))))
        }

        async fn resolve(
            &self,
            attachment: &t3_contracts::chat_attachment::ChatAttachment,
        ) -> ProviderResult<Option<ResolvedCodexAttachment>> {
            use base64::Engine;
            use t3_contracts::chat_attachment::ChatAttachment;
            let Some(original) = self.0.path(attachment) else {
                return Ok(None);
            };
            let files = self.0.clone();
            let attachment = attachment.clone();
            tokio::task::spawn_blocking(move || {
                let unreadable = || ProviderError::Protocol {
                    reason: Some(ProtocolReason::AttachmentUnreadable),
                    detail: "Failed to read saved attachment.".into(),
                    payload: None,
                };
                let path = files.reference(&original).ok_or_else(unreadable)?;
                let image_data_url = match &attachment {
                    ChatAttachment::Image(image) => {
                        let bytes = files
                            .read_reference(&path, t3_workspace::attachments::IMAGE_MAX_BYTES)
                            .ok_or_else(unreadable)?;
                        Some(format!(
                            "data:{};base64,{}",
                            image.mime_type,
                            base64::engine::general_purpose::STANDARD.encode(bytes)
                        ))
                    }
                    _ => None,
                };
                Ok(Some(ResolvedCodexAttachment {
                    extracted_text_path: files.extracted_text_path(&original),
                    path,
                    image_data_url,
                }))
            })
            .await
            .map_err(|e| ProviderError::Operation {
                operation: ProviderOperation::Start,
                detail: e.to_string(),
            })?
        }
    }

    #[cfg(test)]
    mod attachment_review_tests {
        use super::*;
        #[tokio::test]
        async fn codex_session_drop_removes_its_private_copy() {
            let tmp = tempfile::tempdir().unwrap();
            let image=serde_json::from_value(serde_json::json!({"type":"image","id":"thread-00000000-0000-4000-8000-000000000001","mimeType":"image/png","name":"a.png","sizeBytes":1})).unwrap();
            let files = DirectoryClaudeAttachments::new(tmp.path().into());
            std::fs::write(files.path(&image).unwrap(), b"x").unwrap();
            let registration: Arc<dyn CodexAttachmentResolver> =
                Arc::new(CodexAttachments(Arc::new(files)));
            let session = registration
                .session_resolver()
                .unwrap()
                .unwrap_or_else(|| registration.clone());
            let copy = session.resolve(&image).await.unwrap().unwrap().path;
            assert!(copy.is_file());
            drop(session);
            assert!(
                !copy.exists(),
                "registration retained a completed session's copy"
            );
        }
    }
    /// `CodexInstallation.compatibility` as the adapter's initialize check: the live manifest's
    /// policy for this server's version, else the bundled one; a `broken` native version
    /// refuses the session with TypeScript's setup error.
    fn codex_version_gate(
        manifest: t3_provider::catalog::ManifestSource,
        instance: ProviderInstanceId,
    ) -> t3_orchestration::adapters::codex::VersionGate {
        use t3_contracts::server::ServerProviderCompatibilityStatus;
        use t3_provider::versions::resolve_compatibility;
        Arc::new(move |version| {
            let server = env!("CARGO_PKG_VERSION");
            let current = manifest.current();
            let advisory = resolve_compatibility(
                current.compatibility.as_deref().unwrap_or_default(),
                "codex",
                Some(version),
                server,
            )
            .or_else(|| {
                let bundled = t3_provider::catalog::Manifest::bundled().ok()?;
                resolve_compatibility(
                    bundled.compatibility.as_deref().unwrap_or_default(),
                    "codex",
                    Some(version),
                    server,
                )
            });
            let Some(advisory) =
                advisory.filter(|a| a.status == ServerProviderCompatibilityStatus::Broken)
            else {
                return Ok(());
            };
            let target = advisory
                .recommended_version
                .or(advisory.recommended_range)
                .map_or_else(|| "a newer version".to_owned(), |target| target.0);
            Err(ProviderError::Setup(Box::new(ProviderSetupError {
                _tag: ProviderSetupErrorTag::Error,
                instance_id: instance.clone(),
                operation: "runtime".into(),
                detail: format!(
                    "Codex {version} does not work with this T3 Code release. Update Codex to {target}, then retry."
                ),
                cause: None,
            })))
        })
    }

    /// `ClaudeDriver`'s `modelCatalog` (the live manifest, scoped to the instance's custom
    /// models) feeding `compileClaudeModelSelection` on every compile.
    fn claude_model_compiler(
        manifest: t3_provider::catalog::ManifestSource,
        custom_models: Vec<serde_json::Value>,
    ) -> Arc<t3_orchestration::adapters::claude::ClaudeModelCompiler> {
        Arc::new(move |selection| {
            let catalog = super::claude_models::scoped_catalog(
                super::claude_models::claude_catalog(&manifest.current()),
                &custom_models,
            );
            let compiled = super::claude_models::compile(&catalog, selection);
            Ok(t3_orchestration::adapters::claude::ClaudeCompiledModel {
                model: compiled.api_model_id,
                effort: compiled.effort,
                settings: serde_json::Value::Object(compiled.settings),
            })
        })
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
            }?;
            // The session manager cancels the token it passed when it closes the session.
            tokio::spawn(async move {
                tokio::select! {
                    () = access.cancelled() => session.cancel(),
                    () = session.cancelled() => {}
                }
                drop(effective);
            });
            Ok(opened)
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

/// `compileClaudeModelSelection` (`claudeModelOptions.ts`) and `scopeClaudeModelCatalog`
/// (`ClaudeModelCatalog.ts`) over `t3-provider`'s catalogue helpers.
#[cfg(feature = "providers")]
pub mod claude_models {
    use serde_json::{Map, Value, json};
    use t3_contracts::{
        model::{ProviderOptionDescriptor, ProviderOptionSelection, ProviderOptionSelectionValue},
        model_selection::ModelSelection,
    };
    use t3_provider::{
        catalog::{self, Catalog, CatalogModel, Manifest, ModelStatus},
        claude_catalog,
    };

    #[derive(Debug, Clone, PartialEq)]
    pub struct CompiledClaudeModel {
        pub api_model_id: String,
        pub effort: Option<String>,
        /// The prompt-injected effort (`ultrathink`): TypeScript prefixes the prompt with it.
        pub prompt_effort: Option<String>,
        pub settings: Map<String, Value>,
    }

    /// `resolveClaudeModelCatalog`: the manifest's Claude catalogue, else the bundled one.
    pub fn claude_catalog(manifest: &Manifest) -> Catalog {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        manifest
            .catalog("claudeAgent", now)
            .or_else(|| Manifest::bundled().ok()?.catalog("claudeAgent", now))
            .unwrap_or(Catalog {
                models: Vec::new(),
                default_chat_model: None,
            })
    }

    /// `scopeClaudeModelCatalog`: custom models with capabilities join the catalogue, and a
    /// built-in alias that a custom slug claims stops resolving to the built-in model.
    pub fn scoped_catalog(mut catalog: Catalog, custom_models: &[Value]) -> Catalog {
        let mut entries: Vec<(String, String, Option<Value>)> = Vec::new();
        for raw in custom_models {
            let (slug, name, capabilities) = match raw {
                Value::String(slug) => (Some(slug.as_str()), None, None),
                Value::Object(record) => (
                    record.get("slug").and_then(Value::as_str),
                    record.get("name").and_then(Value::as_str),
                    record.get("capabilities").filter(|value| !value.is_null()),
                ),
                _ => continue,
            };
            let Some(slug) = slug.map(str::trim).filter(|slug| !slug.is_empty()) else {
                continue;
            };
            if entries.iter().any(|(seen, _, _)| seen == slug) {
                continue;
            }
            let name = name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(slug);
            entries.push((slug.to_owned(), name.to_owned(), capabilities.cloned()));
        }
        if entries.is_empty() {
            return catalog;
        }
        let claimed: Vec<String> = entries
            .iter()
            .map(|(slug, _, _)| slug.to_lowercase())
            .collect();
        for entry in &mut catalog.models {
            if let Some(Some(aliases)) = &mut entry.model.aliases {
                aliases.retain(|alias| !claimed.contains(&alias.0.to_lowercase()));
            }
        }
        for (slug, name, capabilities) in entries {
            // `decodeCustomModelCapabilities`: an undecodable value means no capabilities.
            let Some(capabilities) = capabilities.and_then(|value| {
                serde_json::from_value::<t3_contracts::model::ModelCapabilities>(value).ok()
            }) else {
                continue;
            };
            if catalog
                .models
                .iter()
                .any(|entry| entry.model.slug.0 == slug)
            {
                continue;
            }
            let mut model = catalog::model(
                &slug,
                &name,
                Some(catalog::capabilities(
                    catalog::descriptors(&capabilities).to_vec(),
                )),
            );
            model.is_custom = true;
            catalog.models.push(CatalogModel {
                model,
                status: ModelStatus::Current,
                adapter: None,
                profile_adapter: None,
            });
        }
        catalog
    }

    fn selection_value<'a>(
        options: &'a [ProviderOptionSelection],
        id: &str,
    ) -> Option<&'a ProviderOptionSelectionValue> {
        options
            .iter()
            .find(|option| option.id == id)
            .map(|option| &option.value)
    }

    /// `compileClaudeModelSelection`.
    pub fn compile(catalog: &Catalog, selection: &ModelSelection) -> CompiledClaudeModel {
        let options = selection
            .options
            .as_ref()
            .map(|options| options.0.as_slice())
            .unwrap_or_default();
        let capabilities = claude_catalog::resolve(catalog, &selection.model)
            .and_then(|entry| claude_catalog::ultracode(entry.model.capabilities.clone()))
            .unwrap_or_else(|| catalog::capabilities(Vec::new()));
        let descriptors = catalog::option_descriptors(&capabilities, options);
        let supports_boolean = |id: &str| {
            descriptors.iter().any(|descriptor| {
                matches!(descriptor, ProviderOptionDescriptor::Boolean(d) if d.id == id)
            })
        };
        let raw_effort = match selection_value(options, "effort") {
            Some(ProviderOptionSelectionValue::String(value)) => Some(value.as_str()),
            _ => None,
        };
        let boolean = |id: &str| match selection_value(options, id) {
            Some(ProviderOptionSelectionValue::Boolean(value)) => Some(*value),
            _ => None,
        };
        let resolved = claude_catalog::resolve_effort(catalog, Some(&selection.model), raw_effort);
        let effort =
            claude_catalog::normalize_effort(catalog, &selection.model, resolved.as_deref());
        let mut settings = Map::new();
        if supports_boolean("thinking")
            && let Some(thinking) = boolean("thinking")
        {
            settings.insert("alwaysThinkingEnabled".into(), json!(thinking));
        }
        if supports_boolean("fastMode") {
            settings.insert(
                "fastMode".into(),
                json!(boolean("fastMode").unwrap_or(false)),
            );
        }
        if let Some(ultracode) = descriptors.iter().find_map(|descriptor| match descriptor {
            ProviderOptionDescriptor::Boolean(d) if d.id == "ultracode" => {
                d.current_value.flatten()
            }
            _ => None,
        }) {
            settings.insert("ultracode".into(), json!(ultracode));
        }
        // `resolvePromptInjectedEffort`.
        let prompt_effort = raw_effort
            .map(str::trim)
            .filter(|effort| !effort.is_empty())
            .filter(|effort| {
                catalog::option_descriptors(&capabilities, &[])
                    .iter()
                    .any(|descriptor| {
                        matches!(descriptor, ProviderOptionDescriptor::Select(d)
                        if d.prompt_injected_values.as_ref().and_then(Option::as_ref)
                            .is_some_and(|values| values.iter().any(|value| value == effort)))
                    })
            })
            .map(str::to_owned);
        CompiledClaudeModel {
            api_model_id: claude_catalog::api_model_id(catalog, selection),
            effort,
            prompt_effort,
            settings,
        }
    }
}
