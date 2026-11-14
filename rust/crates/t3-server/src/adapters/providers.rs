//! Provider composition and authority at the server boundary.
use crate::services::{DomainContext, DomainRuntime, Domains};
use futures::{StreamExt, future::BoxFuture};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    provider_setup::{ProviderSetupError, ProviderSetupErrorTag},
    server::*,
};
use t3_provider::{
    auth::{CredentialBinding, SessionStopper},
    domain::*,
    host::ProviderSessions,
    managed_provider::BackgroundPolicy,
    native_probe::NativeProbe,
    native_provider::{ExecutableProbe, NativeProviderContext},
    sensitive::{ServiceError, ServiceResult},
    transport::{HttpTransport, ProviderClock, ReqwestTransport, SystemClock},
};
use tokio::sync::{Mutex, watch};

pub struct ProviderEffects {
    pub http: Arc<dyn HttpTransport>,
    pub clock: Arc<dyn ProviderClock>,
    pub protocols: Arc<dyn NativeProbe>,
    pub executables: Arc<dyn ExecutableProbe>,
    pub codex: CodexInstallEffects,
    pub maintenance_probe: Option<Arc<dyn t3_provider::maintenance_resolution::MaintenanceProbe>>,
    pub maintenance_executor: Option<Arc<dyn t3_provider::service::MaintenanceExecutor>>,
    pub registry_fetch: Option<Arc<dyn t3_provider::registry::RegistryFetch>>,
    pub registry_commands: Option<Arc<dyn t3_provider::registry_installation::RegistryCommands>>,
    pub registry_downloads: Option<Arc<dyn t3_provider::registry_installation::RegistryDownloads>>,
}
pub struct ProvidersAdapter {
    pub domain: Arc<ProviderDomain>,
    /// The environment's outbound HTTP transport, shared with the usage rate fetcher.
    pub http: Arc<dyn HttpTransport>,
    settings: Arc<t3_config::settings::SettingsService>,
    clock: Arc<dyn ProviderClock>,
    authority: Arc<crate::workspaces::EnvironmentWorkspaces>,
    readiness: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl ProvidersAdapter {
    pub async fn start(
        context: &DomainContext<'_>,
        domains: &Domains,
        effects: Option<Arc<ProviderEffects>>,
    ) -> crate::Result<Arc<Self>> {
        let runtime = domains
            .get::<crate::adapters::orchestration::OrchestrationAdapter>()
            .and_then(|adapter| adapter.composed().cloned())
            .ok_or_else(|| {
                crate::Error::Service(
                    "Provider services require the shared orchestration session manager.".into(),
                )
            })?;
        let authority = domains
            .get::<crate::workspaces::EnvironmentWorkspaces>()
            .ok_or_else(|| {
                crate::Error::Service(
                    "Provider services require the shared workspace authority.".into(),
                )
            })?;
        let managed_mcp = if effects.is_some() {
            false
        } else {
            let path = if cfg!(windows) {
                "C:/Program Files/ClaudeCode/managed-mcp.json"
            } else if cfg!(target_os = "macos") {
                "/Library/Application Support/ClaudeCode/managed-mcp.json"
            } else {
                "/etc/claude-code/managed-mcp.json"
            };
            tokio::fs::try_exists(path).await.unwrap_or(false)
        };
        let native = NativeProviderContext {
            environment: context.environment.clone(),
            home: context.account_home.clone(),
            cwd: context.local.config.cwd.clone(),
            platform: t3_provider::environment::Platform::current(),
            managed_mcp,
            server_version: env!("CARGO_PKG_VERSION").into(),
        };
        let paths = &context.local.config.paths;
        let domain_paths = DomainPaths {
            base: paths.base_dir.clone(),
            state: paths.state_dir.clone(),
            cache: paths.caches(),
            secrets: paths.secrets(),
            broker_parent: std::env::temp_dir(),
        };
        let effects = match effects {
            Some(effects) => effects,
            None => {
                let (protocols, executables) =
                    DomainPorts::native_probes(native.server_version.clone());
                Arc::new(ProviderEffects {
                    http: Arc::new(ReqwestTransport::new().map_err(start_error)?),
                    clock: Arc::new(SystemClock),
                    protocols,
                    executables,
                    codex: CodexInstallEffects::native(&native, &domain_paths)
                        .map_err(start_error)?,
                    maintenance_probe: None,
                    maintenance_executor: None,
                    registry_fetch: None,
                    registry_commands: None,
                    registry_downloads: None,
                })
            }
        };
        let limits = context
            .local
            .managed_policy
            .as_ref()
            .and_then(|policy| policy.limits.as_ref());
        let policy = Arc::new(
            t3_provider::policy::ManagedProviders::new(
                &limits
                    .and_then(|limits| limits.providers.clone())
                    .unwrap_or(t3_contracts::managed_policy::ManagedProviderLimits {
                        allowed_drivers: None,
                        instances: None,
                    }),
                limits.and_then(|limits| limits.max_runtime_mode),
                native.platform,
            )
            .map_err(|_| {
                crate::Error::Service("Could not apply managed provider policy.".into())
            })?,
        );
        let (background, receiver) = watch::channel(BackgroundPolicy::default());
        let (_, mut lifecycle) = context.local.lifecycle.subscribe()?;
        let cancel = context.shutdown.clone();
        let sessions = Arc::new(Sessions(runtime.sessions.clone()));
        let acp_host = crate::adapters::acp_registry::AcpHost::new(runtime.clone());
        #[cfg(unix)]
        let owner_uid = Some(rustix::process::getuid().as_raw());
        #[cfg(not(unix))]
        let owner_uid = None;
        let domain = ProviderDomain::open(
            DomainOptions {
                context: native,
                paths: domain_paths,
                environment_id: context.local.environment.environment_id.0.clone(),
                owner_uid,
                architecture: if cfg!(target_arch = "aarch64") {
                    "arm64"
                } else {
                    "x64"
                }
                .into(),
                helper_command: std::env::current_exe()
                    .map_err(|_| {
                        crate::Error::Service("Could not locate the server executable.".into())
                    })?
                    .to_string_lossy()
                    .into_owned(),
                helper_args: vec![],
                manifest_cache: None,
                manifest: None,
            },
            DomainPorts {
                settings: context.local.settings.clone(),
                policy,
                http: effects.http.clone(),
                clock: effects.clock.clone(),
                sessions: sessions.clone(),
                stopper: sessions,
                acp_host: acp_host.clone(),
                background: receiver,
                codex: CodexInstallEffects {
                    transport: effects.codex.transport.clone(),
                    validator: effects.codex.validator.clone(),
                    local: effects.codex.local.clone(),
                },
                protocols: effects.protocols.clone(),
                executables: effects.executables.clone(),
                factories: BTreeMap::new(),
                installations: BTreeMap::new(),
                usage_sources: None,
                maintenance_probe: effects.maintenance_probe.clone(),
                maintenance_executor: effects.maintenance_executor.clone(),
                registry_fetch: effects.registry_fetch.clone(),
                registry_commands: effects.registry_commands.clone(),
                registry_downloads: effects.registry_downloads.clone(),
            },
        )
        .await
        .map_err(start_error)?;
        acp_host.bind(&domain);
        #[cfg(feature = "host")]
        let host = domains.get::<crate::adapters::host::HostAdapter>();
        #[cfg(feature = "host")]
        let activity_settings = context.local.settings.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {biased;_=cancel.cancelled()=>return,event=lifecycle.next()=>{
                    let Some(event)=event else{return};
                    if matches!(event,ServerLifecycleStreamEvent::Ready(_)) {break;}
                }}
            }
            background.send_modify(|policy| policy.server_ready = true);
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                tokio::select! {biased;_=cancel.cancelled()=>break,_=interval.tick()=>{
                    #[cfg(feature="host")]
                    if let Some(host)=&host {
                        let mut demanded=false;
                        if let Ok(settings)=activity_settings.snapshot().await {
                            let settings=t3_host::background::ActivitySettings::resolve(&settings);
                            if let Ok(snapshot)=host.background.snapshot(&settings).await {
                                for scope in snapshot.leases.iter().flat_map(|lease|&lease.scopes).filter(|scope|matches!(scope,t3_contracts::background::BackgroundScope::ServerConfig|t3_contracts::background::BackgroundScope::ProviderStatus{..})) {
                                    if host.background.should_run_scope_work(scope,&settings).await.unwrap_or(false) {demanded=true;break;}
                                }
                            }
                        }
                        background.send_if_modified(|policy|{let changed=policy.demanded!=demanded;policy.demanded=demanded;changed});
                    }
                }}
            }
        });
        Ok(Arc::new(Self {
            domain,
            http: effects.http.clone(),
            settings: context.local.settings.clone(),
            clock: effects.clock.clone(),
            authority,
            readiness: Mutex::new(Some(task)),
        }))
    }
    pub async fn instance(
        &self,
        id: &ProviderInstanceId,
        operation: &str,
    ) -> Result<(), Box<ProviderSetupError>> {
        let Some(entry) = self.domain.instances.get(id).await else {
            return Err(setup_error(
                id,
                operation,
                "This provider instance is no longer available.",
            ));
        };
        if !self.domain.policy.is_driver_allowed(&entry.source.driver) {
            return Err(setup_error(id, operation, "This provider is disabled."));
        }
        Ok(())
    }
    pub async fn auth_status(
        &self,
        input: t3_contracts::provider_setup::ProviderSetupInput,
        owner: String,
    ) -> Result<t3_provider::auth::AuthSubscription, Box<ProviderSetupError>> {
        self.instance(&input.instance_id, "subscribe").await?;
        let id = input.instance_id.clone();
        self.domain
            .auth
            .subscribe(input, owner)
            .await
            .map_err(|error| auth_error(&id, "subscribe", error))
    }
    pub async fn install_status(
        &self,
        input: t3_contracts::provider_setup::ProviderSetupInput,
    ) -> Result<
        watch::Receiver<t3_contracts::provider_setup::ProviderInstallState>,
        Box<ProviderSetupError>,
    > {
        self.instance(&input.instance_id, "subscribe").await?;
        let id = input.instance_id.clone();
        self.domain
            .installations
            .subscribe(input)
            .await
            .map_err(|error| setup_error(&id, "subscribe", &error.to_string()))
    }
    pub async fn text_models(
        &self,
        input: t3_contracts::settings::ListTextGeneratorModelsInput,
    ) -> Result<
        t3_contracts::settings::ListTextGeneratorModelsResult,
        t3_contracts::settings::TextGeneratorError,
    > {
        let refused = || t3_contracts::settings::TextGeneratorError {
            _tag: t3_contracts::settings::TextGeneratorErrorTag::TextGeneratorError,
            detail: "The text generator endpoint is not configured on this server.".into(),
        };
        if let Some(config) = input.config.as_ref().and_then(Option::as_ref) {
            let settings = self.settings.snapshot().await.map_err(|_| refused())?;
            let supplied = url::Url::parse(&config.base_url.0).map_err(|_| refused())?;
            let saved_config = settings.text_generators.get(&input.id);
            let saved = saved_config.and_then(|config| url::Url::parse(&config.base_url.0).ok());
            let reuses_secret = matches!(
                &config.api_key,
                t3_contracts::settings_secret_value::SettingsSecretValue::Reference(_)
            ) || matches!(&config.api_key,t3_contracts::settings_secret_value::SettingsSecretValue::Literal(value) if value==t3_config::secrets::REDACTED);
            if reuses_secret
                && (saved.as_ref() != Some(&supplied)
                    || saved_config.is_none_or(|saved| saved.kind != config.kind))
            {
                return Err(refused());
            }
            let official = matches!(
                config.kind,
                t3_contracts::settings::TextGeneratorKind::Anthropic
            ) && supplied.as_str() == "https://api.anthropic.com/";
            let official = official
                || supplied.as_str() == "https://api.openai.com/v1/"
                || supplied.as_str() == "https://api.openai.com/v1";
            if !matches!(supplied.scheme(), "http" | "https")
                || !supplied.username().is_empty()
                || supplied.password().is_some()
                || supplied.fragment().is_some()
                || (!official && saved.as_ref() != Some(&supplied))
            {
                return Err(refused());
            }
        }
        self.domain
            .providers
            .list_text_generator_models(input)
            .await
            .map_err(|error| t3_contracts::settings::TextGeneratorError {
                _tag: t3_contracts::settings::TextGeneratorErrorTag::TextGeneratorError,
                detail: error.to_string(),
            })
    }

    pub async fn start_auth(
        &self,
        mut input: t3_contracts::provider_setup::ProviderAuthStartInput,
        owner: &str,
    ) -> Result<t3_contracts::provider_setup::ProviderAuthState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "start").await?;
        if let Some(url) = input.return_url.as_deref() {
            input.return_url = Some(
                t3_provider::callback::provider_auth_return_url(Some(url))
                    .ok_or_else(|| auth_error(&id, "start", ServiceError::Callback))?,
            );
        }
        self.domain
            .auth
            .start(input, owner, self.clock.now_ms())
            .await
            .map(|state| state.into_inner())
            .map_err(|error| auth_error(&id, "start", error))
    }
    pub async fn respond_auth(
        &self,
        input: t3_contracts::provider_setup::ProviderAuthRespondInput,
        owner: &str,
    ) -> Result<t3_contracts::provider_setup::ProviderAuthState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "respond").await?;
        self.domain
            .auth
            .respond(input, owner)
            .await
            .map(|state| state.into_inner())
            .map_err(|error| auth_error(&id, "respond", error))
    }
    pub async fn complete_auth(
        &self,
        input: t3_contracts::provider_setup::ProviderAuthCompleteInput,
        owner: &str,
    ) -> Result<t3_contracts::provider_setup::ProviderAuthState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "complete").await?;
        self.domain
            .auth
            .complete(input, owner)
            .await
            .map(|state| state.into_inner())
            .map_err(|error| auth_error(&id, "complete", error))
    }
    pub async fn cancel_auth(
        &self,
        input: t3_contracts::provider_setup::ProviderAuthCancelInput,
        owner: &str,
    ) -> Result<t3_contracts::provider_setup::ProviderAuthState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "cancel").await?;
        self.domain
            .auth
            .cancel(input, owner)
            .await
            .map(|state| state.into_inner())
            .map_err(|error| auth_error(&id, "cancel", error))
    }
    pub async fn remove_auth(
        &self,
        input: t3_contracts::provider_setup::ProviderAuthRemoveMethodInput,
    ) -> Result<t3_contracts::provider_setup::ProviderAuthState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "removeMethod").await?;
        self.domain
            .auth
            .remove_method(input)
            .await
            .map(|state| state.into_inner())
            .map_err(|error| auth_error(&id, "removeMethod", error))
    }
    pub async fn logout_auth(
        &self,
        input: t3_contracts::provider_setup::ProviderSetupInput,
    ) -> Result<t3_contracts::provider_setup::ProviderAuthState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "logout").await?;
        self.domain
            .auth
            .logout(input)
            .await
            .map(|state| state.into_inner())
            .map_err(|error| auth_error(&id, "logout", error))
    }
    pub async fn reconnect_profile(
        &self,
        input: t3_contracts::provider_setup::ChatGptReconnectProfileInput,
    ) -> Result<
        Option<t3_contracts::provider_setup::ChatGptReconnectProfile>,
        Box<ProviderSetupError>,
    > {
        let id = input.instance_id.clone();
        self.instance(&id, "export").await?;
        self.domain
            .auth
            .reconnect_profile(input)
            .await
            .map(|profile| profile.map(|value| value.into_inner()))
            .map_err(|error| {
                setup_error(
                    &id,
                    "export",
                    match error {
                        ServiceError::Unsupported => {
                            "This provider does not support ChatGPT profile transfer."
                        }
                        ServiceError::Unavailable => {
                            "This saved connection is no longer available."
                        }
                        _ => return auth_error(&id, "export", error),
                    },
                )
            })
    }
    pub async fn import_auth(
        &self,
        input: t3_contracts::provider_setup::ChatGptImportProfileInput,
    ) -> Result<t3_contracts::provider_setup::ProviderAuthState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "import").await?;
        if input
            .profile
            .registration
            .redirect_uri
            .as_deref()
            .is_some_and(|redirect| {
                url::Url::parse(redirect).map_or(true, |url| {
                    url.scheme() != "http"
                        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
                        || url.path() != "/auth/callback"
                        || !url.username().is_empty()
                        || url.password().is_some()
                        || url.query().is_some()
                        || url.fragment().is_some()
                })
            })
        {
            return Err(auth_error(&id, "import", ServiceError::Callback));
        }
        self.domain
            .auth
            .import_profile(input)
            .await
            .map(|state| state.into_inner())
            .map_err(|error| auth_error(&id, "import", error))
    }
    pub async fn start_install(
        &self,
        input: t3_contracts::provider_setup::ProviderSetupInput,
    ) -> Result<t3_contracts::provider_setup::ProviderInstallState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "install").await?;
        self.domain
            .installations
            .start(input)
            .await
            .map_err(|error| setup_error(&id, "install", &error.to_string()))
    }
    pub async fn cancel_install(
        &self,
        input: t3_contracts::provider_setup::ProviderInstallCancelInput,
    ) -> Result<t3_contracts::provider_setup::ProviderInstallState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "cancel-install").await?;
        self.domain
            .installations
            .cancel(input)
            .await
            .map_err(|error| setup_error(&id, "cancel-install", &error.to_string()))
    }
    pub async fn remove_install(
        &self,
        input: t3_contracts::provider_setup::ProviderSetupInput,
    ) -> Result<t3_contracts::provider_setup::ProviderInstallState, Box<ProviderSetupError>> {
        let id = input.instance_id.clone();
        self.instance(&id, "remove-install").await?;
        self.domain
            .installations
            .remove(input)
            .await
            .map_err(|error| setup_error(&id, "remove-install", &error.to_string()))
    }
    pub async fn update_provider(
        &self,
        input: ServerProviderUpdateInput,
    ) -> Result<ServerProviderUpdatedPayload, ServerProviderUpdateError> {
        let refused = |detail: &str| ServerProviderUpdateError {
            tag: ServerProviderUpdateErrorTag::ServerProviderUpdateError,
            provider: input.provider.clone(),
            reason: detail.to_string().into(),
            cause: None,
        };
        let id = input
            .instance_id
            .clone()
            .unwrap_or_else(|| input.provider.0.clone().into());
        self.instance(&id, "update")
            .await
            .map_err(|_| refused("This provider instance is no longer available."))?;
        if self
            .domain
            .instances
            .get(&id)
            .await
            .is_none_or(|entry| entry.source.driver != input.provider)
        {
            return Err(refused("This provider does not support this update."));
        }
        let provider = input.provider.clone();
        self.domain
            .providers
            .update_provider(input)
            .await
            .map_err(|error| ServerProviderUpdateError {
                tag: ServerProviderUpdateErrorTag::ServerProviderUpdateError,
                provider,
                reason: error.to_string().into(),
                cause: None,
            })
    }
    pub async fn workspace(&self, cwd: &str) -> ServiceResult<()> {
        let known = t3_workspace::paths::KnownWorkspaces::new(self.authority.roots().await)
            .map_err(|_| ServiceError::Unavailable)?;
        known
            .assert_contains(Path::new(cwd))
            .map(|_| ())
            .map_err(|_| ServiceError::Unavailable)
    }
}
fn start_error(_: ServiceError) -> crate::Error {
    crate::Error::Service("Could not start provider services.".into())
}
pub fn setup_error(
    id: &ProviderInstanceId,
    operation: &str,
    detail: &str,
) -> Box<ProviderSetupError> {
    Box::new(ProviderSetupError {
        _tag: ProviderSetupErrorTag::Error,
        instance_id: id.clone(),
        operation: operation.into(),
        detail: detail.into(),
        cause: None,
    })
}
impl DomainRuntime for ProvidersAdapter {
    fn name(&self) -> &'static str {
        "providers"
    }
    fn shutdown(&self) -> BoxFuture<'_, crate::Result<()>> {
        Box::pin(async move {
            if let Some(task) = self.readiness.lock().await.take() {
                task.abort();
                let _ = task.await;
            }
            self.domain.shutdown().await.map_err(start_error)
        })
    }
}
struct Sessions(
    t3_orchestration::ports::sessions_recovery::provider_session_manager::ProviderSessionManager,
);
#[async_trait::async_trait]
impl ProviderSessions for Sessions {
    async fn retire(&self, id: &ProviderInstanceId) -> ServiceResult<()> {
        self.0
            .close_instance(id)
            .await
            .map_err(|_| ServiceError::StopSessions)
    }
}
#[async_trait::async_trait]
impl SessionStopper for Sessions {
    async fn stop(
        &self,
        id: &ProviderInstanceId,
        binding: &CredentialBinding,
    ) -> ServiceResult<()> {
        if binding.owner != t3_contracts::provider_setup::ProviderCredentialOwner::T3
            || binding.key != t3_provider::secrets::credential_binding_key("codex-chatgpt", &id.0)
        {
            return Err(ServiceError::StopSessions);
        }
        self.retire(id).await
    }
}
pub async fn config_for(
    services: &crate::services::ServiceRegistry,
    secret_references: bool,
) -> Result<ServerConfig, t3_contracts::rpc::WsServerGetConfigError> {
    let mut config = services
        .local
        .get_config_for(secret_references)
        .await
        .map_err(|_| config_error(services))?;
    if let Some(adapter) = services.domain::<ProvidersAdapter>() {
        config.providers = adapter.domain.config_providers().await;
    }
    Ok(config)
}
pub fn config_stream_for(
    services: Arc<crate::services::ServiceRegistry>,
    secret_references: bool,
) -> futures::stream::BoxStream<
    'static,
    Result<ServerConfigStreamEvent, t3_contracts::rpc::WsServerGetConfigError>,
> {
    async_stream::try_stream! {
        use futures::future::Either;
        let adapter=services.domain::<ProvidersAdapter>();
        let mut changes=adapter.as_ref().map(|adapter|adapter.domain.providers.subscribe());
        let mut existing=crate::adapters::config::config_stream_for(services.clone(),secret_references);
        let Some(first)=existing.next().await else {return;};
        let mut first=first.map_err(|_|config_error(&services))?;
        if let ServerConfigStreamEvent::Snapshot(snapshot)=&mut first && let Some(adapter)=&adapter {snapshot.config.providers=adapter.domain.config_providers().await;}
        yield first;
        loop {
            let next=tokio::select! {
                event=existing.next()=>Either::Left(event),
                changed=async {match changes.as_mut(){Some(changes)=>changes.changed().await.is_ok(),None=>std::future::pending().await}}=>Either::Right(changed),
            };
            match next {
                Either::Left(Some(event))=>{yield event.map_err(|_|config_error(&services))?;},
                Either::Right(true)=>{
                    if let Some(adapter)=&adapter {
                        let event=serde_json::from_value(serde_json::json!({"version":1,"type":"providerStatuses","payload":{"providers":adapter.domain.config_providers().await}}))
                            .map_err(|_|config_error(&services))?;
                        yield event;
                    }
                },
                _=>break,
            }
        }
    }.boxed()
}

pub fn auth_error(
    id: &ProviderInstanceId,
    operation: &str,
    error: ServiceError,
) -> Box<ProviderSetupError> {
    setup_error(
        id,
        operation,
        if error == ServiceError::Unsupported {
            "This provider does not support sign-in in T3 Code."
        } else {
            return setup_error(id, operation, &error.to_string());
        },
    )
}

pub async fn managed_token_helper(
    arguments: &[std::ffi::OsString],
) -> Option<std::process::ExitCode> {
    if arguments
        .first()
        .is_none_or(|argument| argument != "__managed-chatgpt-token")
    {
        return None;
    }
    let result = async {
        let [_, endpoint, instance, environment, profile] = arguments else {
            return Err(());
        };
        let token = t3_provider::token_broker::request_endpoint_token(
            endpoint.to_str().ok_or(())?,
            &t3_provider::token_broker::TokenRequest {
                version: 1,
                instance_id: instance.to_str().ok_or(())?.into(),
                environment_id: environment.to_str().ok_or(())?.into(),
                profile_key: profile.to_str().ok_or(())?.into(),
            },
        )
        .await
        .map_err(|_| ())?;
        std::io::Write::write_all(&mut std::io::stdout().lock(), token.expose().as_bytes())
            .map_err(|_| ())
    }
    .await;
    Some(if result.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    })
}

fn config_error(
    services: &crate::services::ServiceRegistry,
) -> t3_contracts::rpc::WsServerGetConfigError {
    use t3_contracts::settings::{
        ServerSettingsError, ServerSettingsErrorTag, ServerSettingsOperation,
    };
    t3_contracts::rpc::WsServerGetConfigError::ServerSettingsError(Box::new(ServerSettingsError {
        _tag: ServerSettingsErrorTag::ServerSettingsError,
        settings_path: services
            .local
            .config
            .paths
            .settings()
            .to_string_lossy()
            .into_owned(),
        operation: ServerSettingsOperation::ReadFile,
        provider_instance_id: None,
        environment_variable: None,
        managed_setting: None,
        managed_by: None,
        provider_label: None,
        cause: None,
    }))
}
