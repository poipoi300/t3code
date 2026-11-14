use super::{
    auth::AuthService,
    persistence::{Database, service_error},
};
use futures::channel::mpsc;
use serde_json::{Value, json};
use std::sync::Arc;
pub use t3_config::config::{ConfigInputs, ResolvedConfig, RuntimeMode, ServerFlags};
use t3_config::{
    keybindings::KeybindingsService,
    runtime::{RuntimeState, ServerOwner},
    settings::SettingsService,
};
use t3_contracts::{
    environment::ExecutionEnvironmentDescriptor,
    keybindings::{KeybindingRule, KeybindingsConfigError},
    rpc::{WsServerUpdateSettingsPayload, WsServerUpsertKeybindingError},
    server::{
        ServerConfig, ServerLifecycleStreamEvent, ServerRemoveKeybindingInput,
        ServerUpsertKeybindingInput, ServerUpsertKeybindingResult,
    },
    settings::{ServerSettings, ServerSettingsError, ServerSettingsOperation},
};

/// Both ways `ServerOwner::acquire` refuses a home that another server owns.
fn ownership_error(error: t3_config::Error) -> crate::Error {
    match error {
        t3_config::Error::Persistence(t3_persistence::Error::ServerOwnsDatabase { .. }) => {
            crate::Error::StateDirOwned(error.to_string())
        }
        t3_config::Error::Invalid { ref context, .. } if context == "server ownership" => {
            crate::Error::StateDirOwned(error.to_string())
        }
        error => service_error(error),
    }
}

pub fn resolve(inputs: &ConfigInputs) -> crate::Result<ResolvedConfig> {
    t3_config::config::resolve_config(inputs).map_err(service_error)
}
pub struct LocalServices {
    pub config: ResolvedConfig,
    pub auth: AuthService,
    pub environment: ExecutionEnvironmentDescriptor,
    pub immutable_assets: std::collections::HashSet<String>,
    pub settings: Arc<SettingsService>,
    pub managed_policy: Option<t3_contracts::managed_policy::ManagedPolicyDocument>,
    pub keybindings: Arc<KeybindingsService>,
    pub lifecycle: LifecycleEvents,
    database: Database,
    watchers: Vec<Box<dyn std::any::Any + Send + Sync>>,
    owner: std::sync::Mutex<ServerOwner>,
}
impl LocalServices {
    pub async fn open(
        mut config: ResolvedConfig,
        port: u16,
        policy: Option<t3_contracts::managed_policy::ManagedPolicyDocument>,
    ) -> crate::Result<Self> {
        config.port = port;
        config.ensure_directories().map_err(service_error)?;
        let owner = ServerOwner::acquire(&config.paths)
            .await
            .map_err(ownership_error)?;
        let database = super::persistence::open(config.paths.database()).await?;
        let id = t3_config::environment::environment_id(&config.paths)
            .await
            .map_err(service_error)?;
        let auth =
            AuthService::new(&config, id.0.clone(), database.clone(), policy.clone()).await?;
        let environment = serde_json::from_value(json!({
            "environmentId": id, "label": config.cwd.file_name().and_then(|s| s.to_str()).unwrap_or("T3 Code"),
            "platform": {"os": if cfg!(windows) {"windows"} else if cfg!(target_os="macos") {"darwin"} else {"linux"},
                "arch": if cfg!(target_arch="aarch64") {"arm64"} else {"x64"}},
            "serverVersion": env!("CARGO_PKG_VERSION"), "orchestrationProtocolVersion":2,
            "capabilities": {"repositoryIdentity": false, "connectionProbe": true}
        })).map_err(service_error)?;
        let settings =
            SettingsService::with_database(config.paths.settings(), Some(database.clone()));
        let keybindings = KeybindingsService::new(config.paths.keybindings());
        keybindings.sync_defaults().await.map_err(service_error)?;
        let watchers: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![
            Box::new(settings.start().await.map_err(service_error)?),
            Box::new(keybindings.start().await.map_err(service_error)?),
        ];
        let immutable_assets = load_immutable_assets(config.static_dir.as_deref()).await;
        // `welcome`, `ready` and the runtime file are published by the composition root once
        // its recovery and listening barriers have passed (`publish_welcome` and friends).
        let lifecycle = LifecycleEvents::default();
        Ok(Self {
            config,
            auth,
            environment,
            immutable_assets,
            managed_policy: policy,
            settings,
            keybindings,
            lifecycle,
            database,
            watchers,
            owner: std::sync::Mutex::new(owner),
        })
    }
    pub async fn get_config(&self) -> crate::Result<ServerConfig> {
        self.get_config_for(false).await
    }
    /// `secret_references` is the socket's `settingsSecretReferences` client capability.
    pub async fn get_config_for(&self, secret_references: bool) -> crate::Result<ServerConfig> {
        let keys = self.keybindings.snapshot().await.map_err(service_error)?;
        let settings = redacted_settings(
            self.settings.snapshot().await.map_err(service_error)?,
            secret_references,
        )?;
        let observability =
            t3_telemetry::logging::server_observability(&self.config).map_err(service_error)?;
        serde_json::from_value(json!({
            "environment": self.environment, "auth": self.auth.descriptor(),
            "cwd": self.config.cwd.to_string_lossy(), "keybindingsConfigPath": self.config.paths.keybindings().to_string_lossy(),
            "keybindings": keys.keybindings, "issues": keys.issues, "settings": settings,
            "providers": [], "availableEditors": [],
            // Upstream 2d80cb6bc7: clients count live timers on the host clock.
            "serverTime": super::auth::now_iso()?,
            "observability":observability
        })).map_err(service_error)
    }
    /// `server.upsertKeybinding`.
    pub async fn upsert_keybinding(
        &self,
        input: ServerUpsertKeybindingInput,
    ) -> Result<ServerUpsertKeybindingResult, WsServerUpsertKeybindingError> {
        let rule = KeybindingRule {
            key: input.key,
            command: input.command,
            when: input.when,
        };
        let replace = input.replace.flatten().map(|target| KeybindingRule {
            key: target.key,
            command: target.command,
            when: target.when,
        });
        let state = self
            .keybindings
            .upsert(rule, replace)
            .await
            .map_err(|e| self.keybindings_error(&e))?;
        self.keybindings_result(state)
    }
    /// `server.removeKeybinding`.
    pub async fn remove_keybinding(
        &self,
        input: ServerRemoveKeybindingInput,
    ) -> Result<ServerUpsertKeybindingResult, WsServerUpsertKeybindingError> {
        let rule = KeybindingRule {
            key: input.key,
            command: input.command,
            when: input.when,
        };
        let state = self
            .keybindings
            .remove(&rule)
            .await
            .map_err(|e| self.keybindings_error(&e))?;
        self.keybindings_result(state)
    }
    fn keybindings_result(
        &self,
        state: t3_config::keybindings::KeybindingsState,
    ) -> Result<ServerUpsertKeybindingResult, WsServerUpsertKeybindingError> {
        let issues = serde_json::from_value(json!(state.issues))
            .map_err(|e| self.keybindings_error(&e.into()))?;
        Ok(ServerUpsertKeybindingResult {
            keybindings: state.keybindings,
            issues,
        })
    }
    fn keybindings_error(&self, error: &t3_config::Error) -> WsServerUpsertKeybindingError {
        WsServerUpsertKeybindingError::KeybindingsConfigError(Box::new(
            KeybindingsConfigError::KeybindingsConfigParseError {
                config_path: self
                    .config
                    .paths
                    .keybindings()
                    .to_string_lossy()
                    .into_owned(),
                detail: error_detail(error),
                cause: None,
            },
        ))
    }

    /// `server.updateSettings`. SSH device hosts resolving to this machine are not filtered out
    /// as the TypeScript server does; that check needs the device service.
    pub async fn update_settings(
        &self,
        payload: WsServerUpdateSettingsPayload,
        secret_references: bool,
    ) -> Result<ServerSettings, Box<ServerSettingsError>> {
        let updated = match &payload.provider_instance_mutation {
            None => self.settings.update(&payload.patch.0).await,
            Some(mutation) => {
                self.settings
                    .update_provider_instance(mutation, &payload.patch.0)
                    .await
            }
        }
        .map_err(|e| self.settings_error(&e))?;
        redacted_settings(updated, secret_references)
            .and_then(|value| serde_json::from_value(value).map_err(service_error))
            .map_err(|e| {
                self.settings_error(&t3_config::Error::Invalid {
                    context: "normalize".into(),
                    detail: e.to_string(),
                })
            })
    }
    fn settings_error(&self, error: &t3_config::Error) -> Box<ServerSettingsError> {
        let operation = match error {
            t3_config::Error::Invalid { context, .. } => {
                serde_json::from_value(json!(context)).unwrap_or(ServerSettingsOperation::WriteFile)
            }
            t3_config::Error::Io {
                operation: "read", ..
            } => ServerSettingsOperation::ReadFile,
            t3_config::Error::Json(_) => ServerSettingsOperation::Normalize,
            _ => ServerSettingsOperation::WriteFile,
        };
        Box::new(ServerSettingsError {
            _tag: t3_contracts::settings::ServerSettingsErrorTag::ServerSettingsError,
            settings_path: self.config.paths.settings().to_string_lossy().into_owned(),
            operation,
            provider_instance_id: None,
            environment_variable: None,
            managed_setting: None,
            managed_by: None,
            provider_label: None,
            cause: Some(Some(
                json!({"name": "Error", "message": error_detail(error)}),
            )),
        })
    }
    /// Publishes `welcome`. TypeScript does this after recovery and once HTTP is listening.
    pub fn publish_welcome(
        &self,
        bootstrap_project_id: Option<&str>,
        bootstrap_thread_id: Option<&str>,
    ) -> crate::Result<()> {
        let cwd = self.config.cwd.to_string_lossy().into_owned();
        let project_name = self
            .config
            .cwd
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project")
            .to_owned();
        let mut welcome =
            json!({"environment": self.environment, "cwd": cwd, "projectName": project_name});
        if let Some(id) = bootstrap_project_id {
            welcome["bootstrapProjectId"] = json!(id);
        }
        if let Some(id) = bootstrap_thread_id {
            welcome["bootstrapThreadId"] = json!(id);
        }
        self.lifecycle.publish("welcome", welcome)?;
        Ok(())
    }
    /// Publishes `ready`, after the command gate opens.
    pub fn publish_ready(&self) -> crate::Result<()> {
        self.lifecycle.publish(
            "ready",
            json!({"at": super::auth::now_iso()?, "environment": self.environment}),
        )?;
        Ok(())
    }
    /// Writes the runtime file under the owner's id, once activation is reached, so a client
    /// that finds it can issue commands. Equivalent to `ServerOwner::publish`, which cannot
    /// be awaited through the owner's mutex.
    pub async fn publish_runtime_state(&self) -> crate::Result<()> {
        let owner_id = self
            .owner
            .lock()
            .map_err(|_| crate::Error::Service("server owner poisoned".into()))?
            .owner_id
            .clone();
        let mut state = RuntimeState::for_config(
            &self.config,
            self.config.port,
            super::auth::now_iso()?,
            env!("CARGO_PKG_VERSION").into(),
            false,
            None,
        );
        state.owner_id = Some(owner_id);
        t3_config::runtime::persist_runtime_state(&self.config.paths.runtime_state(), &state)
            .await
            .map_err(service_error)
    }
    /// The environment's one database handle, for domain adapters started by the composition root.
    pub fn database(&self) -> &Database {
        &self.database
    }
    pub async fn ownership_lost(&self) {
        let mut health = match self.owner.lock() {
            Ok(owner) => owner.ownership_health(),
            Err(_) => return,
        };
        loop {
            if *health.borrow_and_update() == t3_config::runtime::OwnershipHealth::Lost {
                return;
            }
            if health.changed().await.is_err() {
                return;
            }
        }
    }
    pub async fn shutdown(self) -> crate::Result<()> {
        let Self {
            config: _,
            auth,
            environment: _,
            immutable_assets: _,
            settings,
            managed_policy: _,
            keybindings,
            lifecycle: _,
            database,
            watchers,
            owner,
        } = self;
        drop(watchers);
        drop(settings);
        drop(keybindings);
        drop(auth);
        database.close().await.map_err(service_error)?;
        drop(owner);
        Ok(())
    }
}

pub fn config_stream(
    services: Arc<crate::services::ServiceRegistry>,
) -> futures::stream::BoxStream<'static, crate::Result<t3_contracts::server::ServerConfigStreamEvent>>
{
    config_stream_for(services, false)
}
/// `secret_references` is the socket's `settingsSecretReferences` client capability.
pub fn config_stream_for(
    services: Arc<crate::services::ServiceRegistry>,
    secret_references: bool,
) -> futures::stream::BoxStream<'static, crate::Result<t3_contracts::server::ServerConfigStreamEvent>>
{
    use futures::{StreamExt, future::Either};
    async_stream::try_stream! {
        let mut settings=services.local.settings.subscribe_changes();
        let mut keys=services.local.keybindings.subscribe_changes();
        let config=services.local.get_config_for(secret_references).await?;
        yield serde_json::from_value(json!({"version":1,"type":"snapshot","config":config})).map_err(service_error)?;
        loop {
            let change=tokio::select! {c=settings.recv()=>Either::Left(c),c=keys.recv()=>Either::Right(c)};
            let event=match change {
                Either::Left(Some(settings))=>{
                    let value=redacted_settings(settings,secret_references)?;
                    json!({"version":1,"type":"settingsUpdated","payload":{"settings":value}})
                },
                Either::Right(Some(keys))=>json!({"version":1,"type":"keybindingsUpdated","payload":{"keybindings":keys.keybindings,"issues":keys.issues}}),
                _=>break,
            };
            yield serde_json::from_value(event).map_err(service_error)?;
        }
    }.boxed()
}

fn redacted_settings(settings: ServerSettings, secret_references: bool) -> crate::Result<Value> {
    let mut value = serde_json::to_value(settings).map_err(service_error)?;
    t3_config::secrets::redact_for_client(&mut value, secret_references);
    Ok(value)
}

fn error_detail(error: &t3_config::Error) -> String {
    match error {
        t3_config::Error::Invalid { detail, .. } => detail.clone(),
        other => other.to_string(),
    }
}

#[derive(Default)]
struct LifecycleState {
    sequence: u64,
    /// The newest event of each type with its sequence, as the TypeScript snapshot keeps them.
    events: Vec<(u64, &'static str, ServerLifecycleStreamEvent)>,
    subscribers: Vec<mpsc::UnboundedSender<ServerLifecycleStreamEvent>>,
}

/// Port of `serverLifecycleEvents.ts`: numbered `welcome`/`ready`/`legacyThreadMigration`
/// events, a snapshot of the latest of each type, and a live feed.
#[derive(Default)]
pub struct LifecycleEvents {
    state: std::sync::Mutex<LifecycleState>,
}
impl LifecycleEvents {
    /// `kind` is `welcome`, `ready` or `legacyThreadMigration`; `payload` is that event's payload.
    pub fn publish(
        &self,
        kind: &'static str,
        payload: Value,
    ) -> crate::Result<ServerLifecycleStreamEvent> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::Error::Service("lifecycle events poisoned".into()))?;
        let sequence = state.sequence + 1;
        let event: ServerLifecycleStreamEvent = serde_json::from_value(
            json!({"version": 1, "sequence": sequence, "type": kind, "payload": payload}),
        )
        .map_err(service_error)?;
        state.sequence = sequence;
        state.events.retain(|(_, k, _)| *k != kind);
        state.events.push((sequence, kind, event.clone()));
        state
            .subscribers
            .retain(|subscriber| subscriber.unbounded_send(event.clone()).is_ok());
        Ok(event)
    }
    /// The snapshot in sequence order and a receiver of every later event, taken together so
    /// none is missed or repeated.
    pub fn subscribe(
        &self,
    ) -> crate::Result<(
        Vec<ServerLifecycleStreamEvent>,
        mpsc::UnboundedReceiver<ServerLifecycleStreamEvent>,
    )> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::Error::Service("lifecycle events poisoned".into()))?;
        let (sender, receiver) = mpsc::unbounded();
        state.subscribers.push(sender);
        let mut snapshot = state.events.clone();
        snapshot.sort_by_key(|(sequence, _, _)| *sequence);
        Ok((snapshot.into_iter().map(|(_, _, e)| e).collect(), receiver))
    }
}

pub fn lifecycle_stream(
    services: Arc<crate::services::ServiceRegistry>,
) -> futures::stream::BoxStream<'static, crate::Result<ServerLifecycleStreamEvent>> {
    use futures::StreamExt;
    async_stream::try_stream! {
        let (snapshot, mut live) = services.local.lifecycle.subscribe()?;
        for event in snapshot {
            yield event;
        }
        while let Some(event) = live.next().await {
            yield event;
        }
    }
    .boxed()
}

async fn load_immutable_assets(
    directory: Option<&std::path::Path>,
) -> std::collections::HashSet<String> {
    let Some(directory) = directory else {
        return Default::default();
    };
    let Ok(raw) = tokio::fs::read(directory.join(".vite/manifest.json")).await else {
        return Default::default();
    };
    let Ok(serde_json::Value::Object(manifest)) = serde_json::from_slice(&raw) else {
        return Default::default();
    };
    manifest
        .values()
        .flat_map(|entry| {
            entry
                .get("file")
                .into_iter()
                .chain(
                    entry
                        .get("css")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten(),
                )
                .chain(
                    entry
                        .get("assets")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten(),
                )
        })
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
pub fn discover_static_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(t3_config::config::resolve_static_dir))
        .or_else(|| {
            t3_config::config::resolve_static_dir(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../apps/server/src"),
            )
        })
}

/// The organisation's managed policy (`policy/ManagedPolicy.ts`): the Group Policy registry key
/// on Windows, the configuration profile on macOS, and the policy file; two of them refuse start
/// (`t3_config::policy_sources`). Source builds may name other sources through the
/// `T3CODE_DEV_MANAGED_POLICY_*` variables; release builds never do. A policy that cannot be read
/// or decoded refuses start.
pub fn managed_policy(
    env: &std::collections::BTreeMap<String, String>,
) -> crate::Result<Option<t3_contracts::managed_policy::ManagedPolicyDocument>> {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    #[cfg(feature = "providers")]
    let known_drivers = t3_provider::instances::BUILT_IN_DRIVERS;
    #[cfg(not(feature = "providers"))]
    let known_drivers: &[&str] = &[];
    let dev_build = cfg!(debug_assertions);
    t3_config::policy_sources::read_selected_managed_policy(platform, dev_build, env, known_drivers)
        .map(|policy| policy.document)
        .map_err(|error| match error {
            t3_config::Error::ManagedPolicy { .. } => crate::Error::ManagedPolicy(error.to_string()),
            error => crate::Error::ManagedPolicy(format!(
                "The managed policy at {} cannot be read: {error}",
                t3_config::policy::selected_policy_path(platform, dev_build, env).display()
            )),
        })
}

/// The parts of a managed policy this server does not enforce yet. Starting anyway would
/// silently ignore the organisation's policy, so start is refused while any is present.
/// Enforced: `clients` (bind address and auth), `connect`, `maxRuntimeMode` (thread creation,
/// mode changes and every turn), `providers` (provider registry), `updates` (no update method
/// is served).
pub fn unenforced_policy(
    document: &t3_contracts::managed_policy::ManagedPolicyDocument,
) -> Vec<&'static str> {
    let mut parts = Vec::new();
    if document
        .settings
        .as_ref()
        .is_some_and(|settings| !settings.is_empty())
    {
        parts.push("locked settings");
    }
    if let Some(limits) = &document.limits {
        if limits.webhook_tasks == Some(false) {
            parts.push("webhookTasks");
        }
        if limits.telemetry == Some(false) {
            parts.push("telemetry");
        }
        if limits.browser_cookie_import == Some(false) {
            parts.push("browserCookieImport");
        }
        if cfg!(not(feature = "providers")) && limits.providers.is_some() {
            parts.push("providers");
        }
    }
    parts
}

/// `checkServerReachability`, before binding: the address to listen on, or the organisation's
/// refusal. Without the host services the policy cannot be checked, so a policy refuses start.
pub fn reachable_host(
    policy: Option<&t3_contracts::managed_policy::ManagedPolicyDocument>,
    config: &ResolvedConfig,
) -> crate::Result<String> {
    let Some(document) = policy else {
        return Ok(config.host.clone().unwrap_or_else(|| "127.0.0.1".into()));
    };
    #[cfg(feature = "host")]
    {
        use t3_contracts::auth::ServerAuthPolicy;
        let remote = t3_auth::utils::loopback_bind_address(config.host.as_deref()).is_none();
        let auth = if remote {
            ServerAuthPolicy::RemoteReachable
        } else if config.mode == RuntimeMode::Desktop {
            ServerAuthPolicy::DesktopManagedLocal
        } else {
            ServerAuthPolicy::LoopbackBrowser
        };
        let policy = t3_host::policy::ManagedPolicy::from_document(Some(document.clone()))
            .map_err(|error| crate::Error::Service(error.to_string()))?;
        policy
            .check_server_reachability(auth, config.host.as_deref(), config.tailscale_serve_enabled)
            .map_err(|error| crate::Error::Service(error.to_string()))
    }
    #[cfg(not(feature = "host"))]
    Err(crate::Error::Service(format!(
        "{} manages this machine, and this server cannot enforce its policy.",
        document.organization.0
    )))
}
