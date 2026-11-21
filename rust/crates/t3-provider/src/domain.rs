use crate::{
    acp_coordinator::AcpRuntimeCoordinator,
    acp_service::{AcpRegistryService, AcpSessionHost},
    auth::{AuthController, AuthService, CredentialBinding, SessionStopper},
    catalog::{ManifestCache, ModelManifest},
    chatgpt::{ChatGptAuth, ConfigProfileStore, RandomEntropy},
    codex_installation::CodexInstallation,
    environment::codex_home_layout,
    handoff::HandoffService,
    host::{ProviderFactory, ProviderHost, ProviderRuntime, ProviderSessions},
    installations::{InstallationContext, ManagedInstallation, ProviderInstallations},
    instances::{InstanceRegistry, InstanceSpec, core_driver_definitions},
    managed_codex::{ManagedCodexProvider, ManagedCodexRuntime},
    managed_provider::{BackgroundPolicy, ManagedOptions},
    native_probe::{NativeProbe, ProcessProbes},
    native_provider::{ExecutableProbe, NativeProviderContext, ProcessExecutableProbe},
    registry::RegistryCatalog,
    registry_installation::{ProcessRegistryCommands, ReqwestRegistryDownloads},
    registry_service::{RegistryAgentCatalog, RegistryCatalogOptions},
    secrets::{OnePassword, SecretResolver},
    sensitive::{ServiceError, ServiceResult},
    service::{CatalogRefresh, GlobalProviderSources, ProcessMaintenanceExecutor, ProviderService},
    text_generators::TextGenerators,
    token_broker::{BrokerListener, ManagedTokenBroker},
    transport::{HttpTransport, ProviderClock},
};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Weak},
    time::Duration,
};
use t3_contracts::{
    provider_instance::{ProviderDriverKind, ProviderInstanceId},
    provider_setup::ProviderCredentialOwner,
    server::ServerProviders,
};
use t3_process::CancellationToken;
use tokio::sync::{Mutex, RwLock, watch};

pub struct DomainPaths {
    pub base: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub secrets: PathBuf,
    pub broker_parent: PathBuf,
}
pub struct DomainOptions {
    pub context: NativeProviderContext,
    pub paths: DomainPaths,
    pub environment_id: String,
    pub owner_uid: Option<u32>,
    pub architecture: String,
    pub helper_command: String,
    pub helper_args: Vec<String>,
    pub manifest_cache: Option<ManifestCache>,
    pub manifest: Option<Arc<ModelManifest>>,
}
pub struct CodexInstallEffects {
    pub transport: Arc<dyn crate::codex_installation::ReleaseTransport>,
    pub validator: Arc<dyn crate::codex_installation::CodexExecutableValidator>,
    pub local: Arc<dyn crate::codex_installation::LocalCodex>,
}
impl CodexInstallEffects {
    pub fn native(context: &NativeProviderContext, paths: &DomainPaths) -> ServiceResult<Self> {
        Ok(Self {
            transport: Arc::new(crate::codex_installation::ReqwestReleases::new()?),
            validator: Arc::new(crate::codex_installation::ProcessCodexValidator),
            local: Arc::new(crate::codex_installation::ProcessLocalCodex::new(
                context.environment.clone(),
                paths.base.join("tools/codex"),
            )),
        })
    }
}
pub struct DomainPorts {
    pub settings: Arc<t3_config::settings::SettingsService>,
    pub policy: Arc<crate::policy::ManagedProviders>,
    pub http: Arc<dyn HttpTransport>,
    pub clock: Arc<dyn ProviderClock>,
    pub sessions: Arc<dyn ProviderSessions>,
    pub stopper: Arc<dyn SessionStopper>,
    pub acp_host: Arc<dyn AcpSessionHost>,
    pub background: watch::Receiver<BackgroundPolicy>,
    pub codex: CodexInstallEffects,
    pub protocols: Arc<dyn NativeProbe>,
    pub executables: Arc<dyn ExecutableProbe>,
    pub factories: BTreeMap<ProviderDriverKind, Arc<dyn ProviderFactory>>,
    pub installations: BTreeMap<String, Arc<dyn ManagedInstallation>>,
    pub usage_sources: Option<Arc<dyn GlobalProviderSources>>,
    pub maintenance_probe: Option<Arc<dyn crate::maintenance_resolution::MaintenanceProbe>>,
    pub maintenance_executor: Option<Arc<dyn crate::service::MaintenanceExecutor>>,
    pub registry_fetch: Option<Arc<dyn crate::registry::RegistryFetch>>,
    pub registry_commands: Option<Arc<dyn crate::registry_installation::RegistryCommands>>,
    pub registry_downloads: Option<Arc<dyn crate::registry_installation::RegistryDownloads>>,
}
impl DomainPorts {
    pub fn native_probes(
        server_version: String,
    ) -> (Arc<dyn NativeProbe>, Arc<dyn ExecutableProbe>) {
        (
            Arc::new(ProcessProbes { server_version }),
            Arc::new(ProcessExecutableProbe),
        )
    }
}
struct RefreshCatalog {
    manifest: Arc<ModelManifest>,
    settings: Arc<t3_config::settings::SettingsService>,
    clock: Arc<dyn ProviderClock>,
}
#[async_trait]
impl CatalogRefresh for RefreshCatalog {
    async fn refresh(&self) -> ServiceResult<()> {
        let settings = self
            .settings
            .snapshot()
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Storage,
            ))?;
        self.manifest
            .refresh(
                self.clock.now_ms(),
                true,
                true,
                settings.enable_provider_update_checks,
            )
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        Ok(())
    }
}
struct PolicySettings {
    settings: Arc<t3_config::settings::SettingsService>,
    policy: Arc<crate::policy::ManagedProviders>,
}
#[async_trait]
impl crate::text_generators::SettingsSource for PolicySettings {
    async fn settings(&self) -> ServiceResult<t3_contracts::settings::ServerSettings> {
        self.settings
            .snapshot()
            .await
            .map(|settings| self.policy.apply_limits(&settings))
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Storage,
            ))
    }
}
pub struct ProviderDomain {
    pub providers: Arc<ProviderService>,
    pub policy: Arc<crate::policy::ManagedProviders>,
    settings: Arc<PolicySettings>,
    blocked: BTreeMap<ProviderDriverKind, String>,
    pub auth: Arc<AuthService>,
    pub installations: Arc<ProviderInstallations>,
    pub handoff: Arc<HandoffService>,
    pub acp: Arc<AcpRegistryService>,
    pub instances: Arc<InstanceRegistry>,
    pub secrets: Arc<SecretResolver>,
    pub text_generators: Arc<TextGenerators>,
    pub cursor_accounts: Arc<crate::cursor_account::CursorAccountReader>,
    pub host: Arc<ProviderHost>,
    pub manifest: Arc<ModelManifest>,
    pub registry_catalog: Arc<RegistryAgentCatalog>,
    pub coordinator: Arc<AcpRuntimeCoordinator>,
    /// Live ACP Registry instances, for the application's session host.
    pub acp_instances: Arc<crate::acp_registry_factory::AcpRegistryInstances>,
    pub managed_codex: Arc<ManagedCodexFactory>,
    broker_listener: Arc<BrokerListener>,
    cancel: CancellationToken,
    settings_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    settings_failures: watch::Receiver<Option<ServiceError>>,
}
impl std::fmt::Debug for ProviderDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderDomain([REDACTED])")
    }
}
impl ProviderDomain {
    pub async fn open(options: DomainOptions, ports: DomainPorts) -> ServiceResult<Arc<Self>> {
        let mut settings_changes = ports.settings.subscribe_changes();
        let settings = ports
            .settings
            .snapshot()
            .await
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Storage,
            ))?;
        let settings = ports.policy.apply_limits(&settings);
        let policy = ports.policy.clone();
        let shared_settings = ports.settings.clone();
        let policy_settings = Arc::new(PolicySettings {
            settings: shared_settings.clone(),
            policy: policy.clone(),
        });
        let blocked = core_driver_definitions()
            .into_iter()
            .filter(|driver| !policy.is_driver_allowed(&driver.kind))
            .map(|driver| {
                (
                    driver.kind,
                    "This provider is unavailable under managed policy.".to_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let client = reqwest::Client::builder()
            .build()
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?;
        let manifest = match options.manifest {
            Some(manifest) => manifest,
            None => Arc::new(
                ModelManifest::new(client.clone(), options.manifest_cache).map_err(
                    crate::diagnostics::mapped(
                        concat!(module_path!(), "::error"),
                        ServiceError::Response,
                    ),
                )?,
            ),
        };
        let platform = match options.context.platform {
            crate::environment::Platform::Windows => "win32",
            crate::environment::Platform::Macos => "darwin",
            crate::environment::Platform::Linux => "linux",
        };
        let codex_installation = CodexInstallation::new(
            crate::codex_installation::InstallationOptions {
                base_directory: options.paths.base.clone(),
                platform: options.context.platform,
                environment: options.context.environment.clone(),
                pin: crate::codex_installation::pinned_release(platform, &options.architecture),
                manifest: manifest.source(),
                server_version: options.context.server_version.clone(),
            },
            ports.codex.transport,
            ports.codex.validator,
            ports.codex.local,
            ports.clock.clone(),
        );
        let secrets = Arc::new(SecretResolver::new(OnePassword::new(
            options.context.environment.clone(),
        )));
        let auth = Arc::new(AuthService::default());
        let providers = ProviderService::new(
            auth.clone(),
            ports
                .maintenance_executor
                .unwrap_or_else(|| Arc::new(ProcessMaintenanceExecutor)),
            Arc::new(RefreshCatalog {
                manifest: manifest.clone(),
                settings: ports.settings.clone(),
                clock: ports.clock.clone(),
            }),
            ports.clock.clone(),
            manifest.clone(),
            options.context.server_version.clone(),
        );
        if let Some(sources) = ports.usage_sources {
            providers.set_global_sources(sources).await;
        }
        let text_generators = Arc::new(TextGenerators::new(
            policy_settings.clone(),
            secrets.clone(),
            ports.http.clone(),
            policy.is_allowlisted(),
        ));
        providers.set_text_generators(text_generators.clone()).await;
        let cursor_accounts = crate::cursor_account::CursorAccountReader::new(
            ports.http.clone(),
            ports.clock.clone(),
        );
        let instances = Arc::new(InstanceRegistry::new(core_driver_definitions()));
        let broker = ManagedTokenBroker::new(options.environment_id.clone());
        let broker_listener = Arc::new(
            broker
                .listen(&options.paths.broker_parent, options.owner_uid)
                .await?,
        );
        let verifier = Arc::new(crate::identity::JwksVerifier::new(
            ports.http.clone(),
            ports.clock.clone(),
        ));
        let handoff = Arc::new(HandoffService {
            http: ports.http.clone(),
            verifier: verifier.clone(),
            entropy: Arc::new(RandomEntropy),
            clock: ports.clock.clone(),
            callbacks: Arc::new(crate::callback::LoopbackCallbacks),
        });
        let managed_codex = Arc::new(ManagedCodexFactory {
            context: options.context.clone(),
            secret_directory: options.paths.secrets.clone(),
            environment_id: options.environment_id,
            helper_command: options.helper_command,
            helper_args: options.helper_args,
            installation: codex_installation.clone(),
            protocols: ports.protocols.clone(),
            executables: ports.executables.clone(),
            clock: ports.clock.clone(),
            manifest: manifest.source(),
            http: ports.http.clone(),
            stopper: ports.stopper,
            broker,
            listener: Arc::downgrade(&broker_listener),
            background: ports.background.clone(),
            runtimes: RwLock::new(BTreeMap::new()),
        });
        let native = Arc::new(DomainNativeFactory {
            context: options.context.clone(),
            protocols: ports.protocols,
            executables: ports.executables,
            clock: ports.clock.clone(),
            manifest: manifest.source(),
            http: ports.http.clone(),
            versions: Arc::new(crate::maintenance::LatestVersions::default()),
            maintenance_probe: ports.maintenance_probe,
            update_checks: settings.enable_provider_update_checks,
            background: ports.background.clone(),
            managed: managed_codex.clone(),
        });
        let registry_catalog = Arc::new(RegistryAgentCatalog::new(
            RegistryCatalogOptions {
                cache_dir: options.paths.cache.join("acp-registry"),
                tools_dir: options.paths.base.join("tools/acp-registry"),
                platform: options.context.platform,
                architecture: options.architecture,
                environment: options.context.environment.clone(),
            },
            Arc::new(match ports.registry_fetch {
                Some(fetch) => RegistryCatalog::with_fetcher(fetch, None),
                None => RegistryCatalog::new(client, None),
            }),
            ports.settings.clone(),
            ports.registry_commands.unwrap_or_else(|| {
                Arc::new(ProcessRegistryCommands {
                    cwd: options.context.cwd.clone(),
                })
            }),
            match ports.registry_downloads {
                Some(downloads) => downloads,
                None => Arc::new(ReqwestRegistryDownloads::new().map_err(
                    crate::diagnostics::mapped(
                        concat!(module_path!(), "::error"),
                        ServiceError::Response,
                    ),
                )?),
            },
            ports.clock.clone(),
        ));
        let coordinator = Arc::new(AcpRuntimeCoordinator::new(ports.clock.clone()));
        let acp_instances = Arc::new(crate::acp_registry_factory::AcpRegistryInstances::default());
        let mut factories = ports.factories;
        factories.entry("codex".into()).or_insert(native.clone());
        factories.entry("claudeAgent".into()).or_insert(native);
        factories.entry("acpRegistry".into()).or_insert(Arc::new(
            crate::acp_registry_factory::AcpRegistryFactory {
                context: options.context.clone(),
                cache: options.paths.cache.clone(),
                settings: ports.settings.clone(),
                catalog: registry_catalog.clone(),
                coordinator: coordinator.clone(),
                clock: ports.clock.clone(),
                sessions: ports.sessions.clone(),
                instances: acp_instances.clone(),
            },
        ));
        let cache = Arc::new(crate::status_cache::ProviderStatusCache::new(
            options.paths.cache.join("provider-status"),
        ));
        let host = Arc::new(
            ProviderHost::new(
                instances.clone(),
                providers.clone(),
                factories,
                Arc::new(DomainSessions {
                    sessions: ports.sessions,
                    broker: managed_codex.broker.clone(),
                    managed: Arc::downgrade(&managed_codex),
                }),
                secrets.clone(),
                ports.clock.clone(),
            )
            .with_status_cache(cache),
        );
        let installations = Arc::new(ProviderInstallations::new(
            instances.clone(),
            providers.clone(),
            policy_settings.clone(),
            InstallationContext {
                environment: options.context.environment.clone(),
                home: options.context.home.clone(),
                cwd: options.context.cwd.clone(),
                platform: options.context.platform,
            },
        ));
        installations.register("codex", codex_installation).await;
        for (driver, installation) in ports.installations {
            installations.register(&driver, installation).await;
        }
        let acp = Arc::new(AcpRegistryService::new(
            registry_catalog.clone(),
            ports.acp_host,
            coordinator.clone(),
            auth.clone(),
            options.context.cwd,
        ));
        host.reconcile(policy_settings.as_ref(), &BTreeSet::new(), true, &blocked)
            .await?;
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let watched_host = host.clone();
        let (failed, settings_failures) = watch::channel(None);
        let mut readiness = ports.background.clone();
        let watched_settings = policy_settings.clone();
        let watched_blocked = blocked.clone();
        let settings_task = tokio::spawn(async move {
            let mut reconcile_now = readiness.borrow().server_ready;
            let mut ready_closed = false;
            loop {
                if !reconcile_now {
                    tokio::select! {
                        biased;
                        _=stop.cancelled()=>break,
                        changed=settings_changes.recv()=>{ if changed.is_none() { break; } },
                        changed=readiness.changed(), if !ready_closed => { ready_closed = changed.is_err(); if ready_closed { continue; } },
                    }
                }
                reconcile_now = false;
                let defer_secrets = !readiness.borrow_and_update().server_ready;
                let result = async {
                    watched_host
                        .reconcile(
                            watched_settings.as_ref(),
                            &BTreeSet::new(),
                            defer_secrets,
                            &watched_blocked,
                        )
                        .await?;
                    Ok(())
                }
                .await;
                failed.send_replace(result.err());
            }
        });
        Ok(Arc::new(Self {
            providers,
            policy,
            settings: policy_settings,
            blocked,
            auth,
            installations,
            handoff,
            acp,
            instances,
            secrets,
            text_generators,
            cursor_accounts,
            host,
            manifest,
            registry_catalog,
            coordinator,
            acp_instances,
            managed_codex,
            broker_listener,
            cancel,
            settings_task: Mutex::new(Some(settings_task)),
            settings_failures,
        }))
    }
    pub async fn refresh_providers(
        &self,
        input: t3_contracts::rpc::WsServerRefreshProvidersPayload,
    ) -> ServiceResult<t3_contracts::server::ServerProviderUpdatedPayload> {
        self.refresh_providers_at(input, None)
            .await
            .map_err(crate::service::RefreshProvidersError::service_error)
    }
    pub async fn refresh_providers_at(
        &self,
        input: t3_contracts::rpc::WsServerRefreshProvidersPayload,
        scan_cwd: Option<&str>,
    ) -> Result<
        t3_contracts::server::ServerProviderUpdatedPayload,
        crate::service::RefreshProvidersError,
    > {
        let scoped = input
            .instance_id
            .as_ref()
            .and_then(Option::as_ref)
            .is_some()
            && input.cwd.as_ref().and_then(Option::as_ref).is_some();
        if input.model_catalog_only.flatten() != Some(true)
            && !scoped
            && input.reload_secrets.flatten() == Some(true)
        {
            self.host
                .reload_secrets(
                    self.settings.as_ref(),
                    input.instance_id.as_ref().and_then(Option::as_ref),
                    &self.blocked,
                )
                .await?;
        }
        self.providers.refresh_providers_at(input, scan_cwd).await
    }
    pub async fn apply_secret(
        &self,
        reference: &t3_contracts::secret_reference::SecretReference,
        value: &str,
    ) -> ServiceResult<crate::host::HostReconcile> {
        self.host
            .apply_secret(self.settings.as_ref(), reference, value, &self.blocked)
            .await
    }
    pub async fn config_providers(&self) -> ServerProviders {
        self.providers.get_providers().await
    }
    pub fn subscribe_settings_failures(&self) -> watch::Receiver<Option<ServiceError>> {
        self.settings_failures.clone()
    }
    pub async fn shutdown(&self) -> ServiceResult<()> {
        self.cancel.cancel();
        if let Some(task) = self.settings_task.lock().await.take() {
            let _ = task.await;
        }
        self.host.close().await?;
        self.auth.close().await;
        self.broker_listener.close().await;
        Ok(())
    }
}
impl Drop for ProviderDomain {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub struct ManagedCodexFactory {
    context: NativeProviderContext,
    secret_directory: PathBuf,
    environment_id: String,
    helper_command: String,
    helper_args: Vec<String>,
    installation: Arc<CodexInstallation>,
    protocols: Arc<dyn NativeProbe>,
    executables: Arc<dyn ExecutableProbe>,
    clock: Arc<dyn ProviderClock>,
    manifest: crate::catalog::ManifestSource,
    http: Arc<dyn HttpTransport>,
    stopper: Arc<dyn SessionStopper>,
    broker: Arc<ManagedTokenBroker>,
    listener: Weak<BrokerListener>,
    background: watch::Receiver<BackgroundPolicy>,
    runtimes: RwLock<BTreeMap<ProviderInstanceId, Weak<ManagedCodexRuntime>>>,
}
impl ManagedCodexFactory {
    pub async fn runtime(&self, id: &ProviderInstanceId) -> Option<Arc<ManagedCodexRuntime>> {
        self.runtimes.read().await.get(id).and_then(Weak::upgrade)
    }
}
#[async_trait]
impl ProviderFactory for ManagedCodexFactory {
    async fn create(&self, spec: Arc<InstanceSpec>) -> ServiceResult<ProviderRuntime> {
        if spec.driver.0 != "codex" {
            return Err(ServiceError::Unsupported);
        }
        let environment = spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        );
        let configured_home = spec.config["homePath"].as_str().unwrap_or_default();
        let home = if t3_contracts::wire::trim_js(configured_home).is_empty() {
            environment
                .get("CODEX_HOME")
                .map(String::as_str)
                .unwrap_or(configured_home)
        } else {
            configured_home
        };
        let layout = codex_home_layout(
            home,
            spec.config["shadowHomePath"].as_str().unwrap_or_default(),
            &self.context.home,
            &self.context.cwd,
        );
        let store = ConfigProfileStore::new(&self.secret_directory, &spec.instance_id.0);
        let auth = ChatGptAuth::new(
            Arc::new(store),
            self.http.clone(),
            Arc::new(crate::identity::JwksVerifier::new(
                self.http.clone(),
                self.clock.clone(),
            )),
            Arc::new(RandomEntropy),
            self.clock.clone(),
            Arc::new(crate::callback::LoopbackCallbacks),
            self.environment_id.clone(),
        );
        let controller = AuthController::new(
            spec.instance_id.clone(),
            CredentialBinding {
                key: crate::secrets::credential_binding_key("codex-chatgpt", &spec.instance_id.0),
                owner: ProviderCredentialOwner::T3,
            },
            auth.clone(),
            self.stopper.clone(),
            Duration::from_secs(300),
        )
        .await;
        self.broker
            .register(spec.instance_id.clone(), auth.clone())
            .await;
        let runtime = Arc::new(ManagedCodexRuntime {
            auth,
            controller: controller.clone(),
            installation: self.installation.clone(),
            layout,
            environment,
            environment_id: self.environment_id.clone(),
            instance: spec.instance_id.clone(),
            helper_command: self.helper_command.clone(),
            helper_args: self.helper_args.clone(),
            listener: self.listener.upgrade().ok_or(ServiceError::Unavailable)?,
        });
        self.runtimes
            .write()
            .await
            .insert(spec.instance_id.clone(), Arc::downgrade(&runtime));
        let source = ManagedCodexProvider {
            runtime,
            spec,
            context: self.context.clone(),
            protocols: self.protocols.clone(),
            executables: self.executables.clone(),
            clock: self.clock.clone(),
            manifest: self.manifest.clone(),
            http: self.http.clone(),
        };
        let initial = source.initial_snapshot()?;
        let source = source
            .into_managed(ManagedOptions::new(self.background.clone()))
            .await?;
        Ok(ProviderRuntime {
            source,
            initial,
            auth: Some(controller),
        })
    }
}

struct DomainNativeFactory {
    context: NativeProviderContext,
    protocols: Arc<dyn NativeProbe>,
    executables: Arc<dyn ExecutableProbe>,
    clock: Arc<dyn ProviderClock>,
    manifest: crate::catalog::ManifestSource,
    http: Arc<dyn HttpTransport>,
    versions: Arc<crate::maintenance::LatestVersions>,
    maintenance_probe: Option<Arc<dyn crate::maintenance_resolution::MaintenanceProbe>>,
    update_checks: bool,
    background: watch::Receiver<BackgroundPolicy>,
    managed: Arc<ManagedCodexFactory>,
}
#[async_trait]
impl ProviderFactory for DomainNativeFactory {
    async fn create(&self, spec: Arc<InstanceSpec>) -> ServiceResult<ProviderRuntime> {
        if spec.driver.0 == "codex" && spec.config["setupMode"] == "managed" {
            return self.managed.create(spec).await;
        }
        if !matches!(spec.driver.0.as_str(), "codex" | "claudeAgent") {
            return Err(ServiceError::Unsupported);
        }
        let environment = spec.environment(
            &self.context.environment,
            &self.context.home,
            self.context.platform,
        );
        let binary = spec.config["binaryPath"]
            .as_str()
            .filter(|binary| !binary.is_empty())
            .unwrap_or(if spec.driver.0 == "codex" {
                "codex"
            } else {
                "claude"
            });
        let maintenance = Arc::new(crate::maintenance_resolution::NativeMaintenance::new(
            crate::maintenance_resolution::MaintenanceContext {
                driver: spec.driver.0.clone(),
                binary: crate::environment::expand_home_path(binary, &self.context.home),
                environment,
                host_environment: self.context.environment.clone(),
                home: self.context.home.clone(),
                cwd: self.context.cwd.clone(),
                platform: self.context.platform,
                shared_codex_home: self.context.home.join(".codex"),
            },
            self.maintenance_probe.clone().unwrap_or_else(|| {
                Arc::new(crate::maintenance_resolution::ProcessMaintenanceProbe {
                    cwd: self.context.cwd.clone(),
                })
            }),
            self.http.clone(),
            self.clock.clone(),
            self.versions.clone(),
        ));
        let source = crate::native_provider::NativeProvider::new(
            spec,
            self.context.clone(),
            self.protocols.clone(),
            self.executables.clone(),
            self.clock.clone(),
            self.manifest.clone(),
        )
        .with_maintenance(maintenance, self.update_checks)
        .with_reset_client(Arc::new(crate::reset_credits::ClaudeResetClient {
            http: self.http.clone(),
            clock: self.clock.clone(),
            platform: self.context.platform,
        }));
        let initial = source.initial_snapshot()?;
        let source = source.into_managed(ManagedOptions::new(self.background.clone()))?;
        Ok(ProviderRuntime {
            source,
            initial,
            auth: None,
        })
    }
}
struct DomainSessions {
    sessions: Arc<dyn ProviderSessions>,
    broker: Arc<ManagedTokenBroker>,
    managed: Weak<ManagedCodexFactory>,
}
#[async_trait]
impl ProviderSessions for DomainSessions {
    async fn retire(&self, id: &ProviderInstanceId) -> ServiceResult<()> {
        self.sessions.retire(id).await?;
        self.broker.unregister(id).await;
        if let Some(managed) = self.managed.upgrade() {
            managed.runtimes.write().await.remove(id);
        }
        Ok(())
    }
}
