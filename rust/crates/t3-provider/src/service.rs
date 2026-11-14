use crate::{
    auth::AuthService,
    maintenance::{InstallationGate, MaintenanceAction, MaintenanceCapabilities},
    sensitive::{Sensitive, ServiceError, ServiceResult},
    transport::ProviderClock,
};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use t3_contracts::{
    provider_instance::ProviderInstanceId, rpc::WsServerRefreshProvidersPayload, server::*,
};
use tokio::sync::{Mutex, RwLock, watch};

#[async_trait]
pub trait ProviderInstanceService: Send + Sync {
    fn instance_id(&self) -> &ProviderInstanceId;
    async fn snapshot(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<ServerProvider>;
    async fn reload_secrets(&self) -> ServiceResult<()> {
        Ok(())
    }
    async fn refresh_models(&self) -> ServiceResult<()> {
        Ok(())
    }
    async fn invalidate_models(&self, _cwd: Option<&str>, _fresh: bool) -> ServiceResult<()> {
        Ok(())
    }
    async fn invalidate_caches(&self) -> ServiceResult<()> {
        self.invalidate_models(None, true).await
    }
    async fn after_update(&self) -> ServiceResult<()> {
        Ok(())
    }
    fn catalogs_per_cwd(&self) -> bool {
        false
    }
    async fn maintenance(&self, _fresh: bool) -> ServiceResult<MaintenanceCapabilities> {
        Err(ServiceError::Unsupported)
    }
    async fn latest_version(&self, _fresh: bool) -> ServiceResult<Option<String>> {
        Ok(None)
    }
    async fn consume_reset_credit(
        &self,
        _credit: Option<&str>,
    ) -> ServiceResult<t3_contracts::provider_usage_limits::ProviderConsumeResetCreditOutcome> {
        Err(ServiceError::Unsupported)
    }
}
#[async_trait]
pub trait GlobalProviderSources: Send + Sync {
    async fn consume_reset_credit(
        &self,
        _input: t3_contracts::provider_usage_limits::UsageLimitSourceConsumeResetCreditInput,
    ) -> ServiceResult<t3_contracts::provider_usage_limits::ProviderConsumeResetCreditResult> {
        Err(ServiceError::Unsupported)
    }
    async fn refresh_usage(&self) -> ServiceResult<()>;
    async fn reload_secrets(&self) -> ServiceResult<()>;
    async fn with_usage_commands(
        &self,
        providers: ServerProviders,
    ) -> ServiceResult<ServerProviders>;
}
#[async_trait]
pub trait CatalogRefresh: Send + Sync {
    async fn refresh(&self) -> ServiceResult<()>;
    async fn force_refresh(&self) -> ServiceResult<()> {
        self.refresh().await
    }
}
#[async_trait]
pub trait MaintenanceExecutor: Send + Sync {
    async fn execute(&self, action: MaintenanceAction) -> ServiceResult<()>;
}
pub struct ProcessMaintenanceExecutor;
#[async_trait]
impl MaintenanceExecutor for ProcessMaintenanceExecutor {
    async fn execute(&self, action: MaintenanceAction) -> ServiceResult<()> {
        let mut options = crate::environment::process_options(
            action.executable,
            &action.args,
            &action.environment,
        );
        options.timeout = Some(Duration::from_secs(30 * 60));
        options.max_output_bytes = 10000;
        options.check_exit = false;
        let result = t3_process::run(options)
            .await
            .map_err(|_| ServiceError::Maintenance)?;
        if result.code == Some(0) {
            Ok(())
        } else {
            Err(ServiceError::Maintenance)
        }
    }
}

pub struct ProviderService {
    pub auth: Arc<AuthService>,
    instances: RwLock<BTreeMap<ProviderInstanceId, Arc<dyn ProviderInstanceService>>>,
    snapshots: RwLock<BTreeMap<ProviderInstanceId, ServerProvider>>,
    changes: watch::Sender<u64>,
    refresh_gate: Mutex<()>,
    update_gates: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    updates: Mutex<
        BTreeMap<
            ProviderInstanceId,
            watch::Receiver<Option<ServiceResult<ServerProviderUpdatedPayload>>>,
        >,
    >,
    execution_gate: Arc<InstallationGate>,
    executor: Arc<dyn MaintenanceExecutor>,
    catalog: Arc<dyn CatalogRefresh>,
    clock: Arc<dyn ProviderClock>,
    server_version: String,
    manifest: crate::catalog::ManifestSource,
    text_generators: RwLock<Option<Arc<crate::text_generators::TextGenerators>>>,
    sources: RwLock<Option<Arc<dyn GlobalProviderSources>>>,
}
impl ProviderService {
    pub fn new(
        auth: Arc<AuthService>,
        executor: Arc<dyn MaintenanceExecutor>,
        catalog: Arc<dyn CatalogRefresh>,
        clock: Arc<dyn ProviderClock>,
        manifest: impl Into<crate::catalog::ManifestSource>,
        server_version: String,
    ) -> Arc<Self> {
        let (changes, _) = watch::channel(0);
        Arc::new(Self {
            auth,
            instances: RwLock::new(BTreeMap::new()),
            snapshots: RwLock::new(BTreeMap::new()),
            changes,
            refresh_gate: Mutex::new(()),
            update_gates: Mutex::new(BTreeMap::new()),
            updates: Mutex::new(BTreeMap::new()),
            execution_gate: Arc::new(InstallationGate::default()),
            executor,
            catalog,
            clock,
            server_version,
            manifest: manifest.into(),
            text_generators: RwLock::new(None),
            sources: RwLock::new(None),
        })
    }
    pub async fn set_text_generators(&self, service: Arc<crate::text_generators::TextGenerators>) {
        *self.text_generators.write().await = Some(service);
    }
    pub async fn set_global_sources(&self, sources: Arc<dyn GlobalProviderSources>) {
        *self.sources.write().await = Some(sources);
    }
    pub async fn consume_reset_credit(
        &self,
        input: t3_contracts::provider_usage_limits::ProviderConsumeResetCreditInput,
    ) -> ServiceResult<t3_contracts::provider_usage_limits::ProviderConsumeResetCreditResult> {
        use t3_contracts::provider_usage_limits::{
            ProviderConsumeResetCreditInput as Input, ProviderConsumeResetCreditResult,
        };
        match input {
            Input::UsageLimitSourceConsumeResetCreditInput(input) => {
                self.sources
                    .read()
                    .await
                    .clone()
                    .ok_or(ServiceError::Unavailable)?
                    .consume_reset_credit(input)
                    .await
            }
            Input::ProviderInstanceConsumeResetCreditInput(input) => {
                let instance = self
                    .instances
                    .read()
                    .await
                    .get(&input.instance_id)
                    .cloned()
                    .ok_or(ServiceError::Unavailable)?;
                if !instance.snapshot(None, false).await?.enabled {
                    return Err(ServiceError::Disabled);
                }
                let outcome = instance
                    .consume_reset_credit(
                        input
                            .credit_id
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map(|s| s.0.as_str()),
                    )
                    .await?;
                let snapshot = instance.snapshot(None, false).await?;
                self.snapshots
                    .write()
                    .await
                    .insert(input.instance_id, snapshot);
                self.publish();
                Ok(ProviderConsumeResetCreditResult {
                    outcome,
                    warning: None,
                })
            }
        }
    }
    pub async fn list_text_generator_models(
        &self,
        input: t3_contracts::settings::ListTextGeneratorModelsInput,
    ) -> ServiceResult<t3_contracts::settings::ListTextGeneratorModelsResult> {
        let service = self
            .text_generators
            .read()
            .await
            .clone()
            .ok_or(ServiceError::Unavailable)?;
        service.list_models(input).await
    }
    pub async fn register(&self, instance: Arc<dyn ProviderInstanceService>) {
        self.instances
            .write()
            .await
            .insert(instance.instance_id().clone(), instance);
    }
    pub async fn unregister(&self, id: &ProviderInstanceId) {
        self.instances.write().await.remove(id);
        self.snapshots.write().await.remove(id);
        self.auth.unregister(id).await;
        self.publish();
    }
    fn publish(&self) {
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }
    pub fn installation_gate(&self) -> Arc<InstallationGate> {
        self.execution_gate.clone()
    }
    pub async fn get_providers(&self) -> ServerProviders {
        let mut providers: Vec<_> = self.snapshots.read().await.values().cloned().collect();
        const ORDER: &[&str] = &[
            "codex",
            "claudeAgent",
            "cursor",
            "grok",
            "opencode",
            "antigravity",
            "fx",
            "kiro",
        ];
        let rank = |driver: &str| {
            ORDER
                .iter()
                .position(|d| *d == driver)
                .unwrap_or(ORDER.len())
        };
        providers.sort_by(|a, b| {
            rank(&a.driver.0)
                .cmp(&rank(&b.driver.0))
                .then_with(|| a.driver.cmp(&b.driver))
                .then_with(|| a.display_name.cmp(&b.display_name))
                .then_with(|| a.instance_id.cmp(&b.instance_id))
        });
        ServerCompatibleArray(providers)
    }
    pub async fn refresh_providers(
        &self,
        input: WsServerRefreshProvidersPayload,
    ) -> ServiceResult<ServerProviderUpdatedPayload> {
        if input.model_catalog_only.flatten() == Some(true) {
            self.catalog.refresh().await?;
            return Ok(ServerProviderUpdatedPayload {
                providers: ServerCompatibleArray(vec![]),
            });
        }
        let _gate = self.refresh_gate.lock().await;
        let id = input.instance_id.as_ref().and_then(Option::as_ref);
        let cwd = id
            .and(input.cwd.as_ref().and_then(Option::as_ref))
            .map(|s| s.0.as_str());
        let sources = self.sources.read().await.clone();
        if id.is_none()
            && let Some(sources) = sources.as_ref()
        {
            if input.reload_secrets.flatten() == Some(true) {
                sources.reload_secrets().await?;
            }
            sources.refresh_usage().await?;
        }
        let instances: Vec<_> = self
            .instances
            .read()
            .await
            .iter()
            .filter(|(key, _)| id.is_none_or(|id| id == *key))
            .map(|(_, instance)| instance.clone())
            .collect();
        for instance in instances {
            if input.reload_secrets.flatten() == Some(true) {
                instance.reload_secrets().await?;
            }
            if input.refresh_models.flatten() == Some(true) {
                instance
                    .invalidate_models(cwd, input.fresh.flatten() == Some(true))
                    .await?;
            }
            let mut snapshot = instance
                .snapshot(cwd, input.fresh.flatten() == Some(true))
                .await?;
            if input.refresh_models.flatten() == Some(true)
                && snapshot.enabled
                && snapshot.installed
            {
                instance.refresh_models().await?;
                snapshot = instance.snapshot(None, false).await?;
            }
            self.snapshots
                .write()
                .await
                .insert(instance.instance_id().clone(), snapshot);
        }
        self.publish();
        let mut providers = self.get_providers().await;
        if input.omit_providers.flatten() == Some(true) && (id.is_none() || cwd.is_none()) {
            providers.0.clear();
        }
        if input.workspace_snapshots_on_demand.flatten() == Some(true) {
            if input.usage_limits_command.flatten() == Some(true)
                && let Some(sources) = sources
            {
                providers = sources.with_usage_commands(providers).await?;
            }
            if let (Some(id), Some(cwd)) = (id, cwd) {
                providers.0.retain(|p| p.instance_id == *id);
                let per_cwd = input.provider_catalogs_on_demand.flatten() == Some(true)
                    && self
                        .instances
                        .read()
                        .await
                        .get(id)
                        .is_some_and(|p| p.catalogs_per_cwd());
                for provider in &mut providers.0 {
                    let full_commands = provider.slash_commands.clone();
                    let full_skills = provider.skills.clone();
                    let digest = machine_catalog_digest(provider)?;
                    if per_cwd && input.machine_catalog.flatten() == Some(true) {
                        machine_catalog_names(provider)?;
                        provider.catalog_digest = Some(digest.into());
                    } else {
                        provider.slash_commands.clear();
                        provider.skills.clear();
                    }
                    provider.models.clear();
                    let snapshots = provider.workspace_snapshots.get_or_insert_with(Vec::new);
                    snapshots.retain(|s| s.cwd.0 == cwd);
                    if snapshots.is_empty() && per_cwd {
                        snapshots.push(ServerProviderWorkspaceSnapshot {
                            cwd: cwd.to_owned().into(),
                            checked_at: provider.checked_at.clone(),
                            slash_commands: full_commands,
                            slash_commands_pending: Some(Some(true)),
                            skills: full_skills,
                            option_descriptors: None,
                            models: None,
                            models_pending: None,
                        });
                    }
                }
            } else {
                for provider in &mut providers.0 {
                    provider.workspace_snapshots = None;
                }
            }
        }
        Ok(ServerProviderUpdatedPayload { providers })
    }
    fn iso(&self) -> ServiceResult<t3_contracts::wire::WireDateTime> {
        chrono::DateTime::from_timestamp_millis(self.clock.now_ms())
            .map(|d| {
                t3_contracts::wire::WireDateTime(
                    d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                )
            })
            .ok_or(ServiceError::Response)
    }
    async fn update_state(
        &self,
        id: &ProviderInstanceId,
        status: ServerProviderUpdateStatus,
        started: Option<t3_contracts::wire::WireDateTime>,
        message: &str,
    ) -> ServiceResult<ServerProviderUpdatedPayload> {
        let terminal = matches!(
            status,
            ServerProviderUpdateStatus::Succeeded
                | ServerProviderUpdateStatus::Failed
                | ServerProviderUpdateStatus::Unchanged
        );
        if let Some(snapshot) = self.snapshots.write().await.get_mut(id) {
            snapshot.update_state = Some(ServerProviderUpdateState {
                status,
                started_at: started,
                finished_at: if terminal { Some(self.iso()?) } else { None },
                message: Some(message.to_owned().into()),
                output: None,
            });
        }
        self.publish();
        Ok(ServerProviderUpdatedPayload {
            providers: self.get_providers().await,
        })
    }
    pub async fn update_provider(
        self: &Arc<Self>,
        input: ServerProviderUpdateInput,
    ) -> ServiceResult<ServerProviderUpdatedPayload> {
        let id = input
            .instance_id
            .clone()
            .unwrap_or_else(|| ProviderInstanceId(input.provider.0.clone()));
        let mut updates = self.updates.lock().await;
        if updates.get(&id).is_some_and(|r| r.borrow().is_none()) {
            return Err(ServiceError::Busy);
        }
        let instance = self
            .instances
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or(ServiceError::Unavailable)?;
        let capabilities = instance.maintenance(false).await?;
        if capabilities.driver != input.provider.0 || capabilities.update.is_none() {
            return Err(ServiceError::Unsupported);
        }
        let (tx, mut rx) = watch::channel(None);
        updates.insert(id.clone(), rx.clone());
        drop(updates);
        let service = self.clone();
        tokio::spawn(async move {
            let result = service.run_update(instance, id, input, capabilities).await;
            tx.send_replace(Some(result));
        });
        rx.wait_for(|v| v.is_some())
            .await
            .map_err(|_| ServiceError::Maintenance)?;

        rx.borrow().clone().ok_or(ServiceError::Maintenance)?
    }
    async fn refresh_after_update(
        &self,
        driver: &str,
        selected: &ProviderInstanceId,
    ) -> ServiceResult<Vec<ServerProvider>> {
        self.catalog.force_refresh().await?;
        let snapshots = self.snapshots.read().await.clone();
        let instances: Vec<_> = self
            .instances
            .read()
            .await
            .iter()
            .filter(|(id, _)| {
                *id == selected
                    || snapshots
                        .get(*id)
                        .is_some_and(|value| value.driver.0 == driver && value.enabled)
            })
            .map(|(_, instance)| instance.clone())
            .collect();
        if let Some(instance) = instances
            .iter()
            .find(|instance| instance.instance_id() == selected)
        {
            let _ = instance.after_update().await;
        }
        let mut verified = vec![];
        for instance in instances {
            let id = instance.instance_id();
            let refresh = async {
                instance.invalidate_caches().await?;
                instance.maintenance(true).await?;
                instance.latest_version(true).await?;
                instance.snapshot(None, true).await
            }
            .await;
            let current = self.instances.read().await;
            if current
                .get(id)
                .is_none_or(|current| !Arc::ptr_eq(current, &instance))
            {
                continue;
            }
            match refresh {
                Ok(value) => {
                    self.snapshots
                        .write()
                        .await
                        .insert(id.clone(), value.clone());
                    verified.push(value);
                }
                Err(_) => {
                    if let Some(value) = self.snapshots.write().await.get_mut(id) {
                        value.status = ServerProviderState::Error;
                        value.message = Some(Some(
                            "Provider refresh failed after update.".to_owned().into(),
                        ));
                    }
                }
            }
        }
        Ok(verified)
    }
    async fn run_update(
        &self,
        instance: Arc<dyn ProviderInstanceService>,
        id: ProviderInstanceId,
        input: ServerProviderUpdateInput,
        original: MaintenanceCapabilities,
    ) -> ServiceResult<ServerProviderUpdatedPayload> {
        let key = original
            .update
            .as_ref()
            .ok_or(ServiceError::Unsupported)?
            .lock_key
            .clone();
        let lock = self
            .update_gates
            .lock()
            .await
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        self.update_state(
            &id,
            ServerProviderUpdateStatus::Queued,
            None,
            "Waiting for another provider update to finish.",
        )
        .await?;
        let _command = lock.lock().await;
        let started = Some(self.iso()?);
        self.update_state(
            &id,
            ServerProviderUpdateStatus::Running,
            started.clone(),
            "Checking for the latest version",
        )
        .await?;
        let work = async {
            if self.instances.read().await.get(&id).is_none_or(|current|!Arc::ptr_eq(current,&instance)){return Err(ServiceError::Unavailable);}
            let fresh = instance.maintenance(true).await?;
            if fresh.update.as_ref().is_none_or(|a| a.lock_key != key) {
                return Err(ServiceError::Maintenance);
            }
            let target = input.target_version.as_ref().map(|s| s.0.as_str());
            let latest = instance.latest_version(true).await?;
            let latest = latest.or(self
                .snapshots
                .read()
                .await
                .get(&id)
                .and_then(|p| p.version_advisory.as_ref())
                .and_then(|a| a.latest_version.as_ref())
                .map(|v| v.0.clone()));
            let candidate = target.or(latest.as_deref());
            let advisory = crate::versions::resolve_compatibility(
                self.manifest.current().compatibility.as_deref().unwrap_or_default(),
                &input.provider.0,
                candidate,
                &self.server_version,
            );
            if target.is_some()
                && advisory.as_ref().is_none_or(|a| {
                    a.status != ServerProviderCompatibilityStatus::Supported
                        || a.recommended_version.as_ref().map(|s| s.0.as_str()) != target
                })
                || target.is_none()
                    && advisory.as_ref().is_some_and(|a| {
                        matches!(
                            a.status,
                            ServerProviderCompatibilityStatus::Broken
                                | ServerProviderCompatibilityStatus::Unsupported
                        )
                    })
            {
                return Err(ServiceError::Maintenance);
            }
            if let Some(candidate)=candidate {
                let checked=instance.snapshot(None,true).await?;
                let current=checked.installed_version.as_ref().and_then(Option::as_ref).or(checked.version.as_ref());
                let already_current=current.is_some_and(|version|if target.is_some(){version.0.trim_start_matches('v')==candidate}else{!crate::versions::compare_versions(&version.0,candidate).is_lt()});
                let restart = needs_opencode_restart(std::slice::from_ref(&checked));
                self.snapshots.write().await.insert(id.clone(),checked);
                if already_current{return Ok((ServerProviderUpdateStatus::Succeeded,if restart {OPENCODE_SERVER_RESTART_MESSAGE}else{"Already up to date."}));}
            }
            let action = if let Some(target) = target {
                crate::maintenance::targeted_update(&fresh, target)
            } else {
                crate::maintenance::latest_update(&fresh, candidate)
            }
            .ok_or(ServiceError::Unsupported)?;
            let mut sharing = vec![id.clone()];
            for (other, instance) in self.instances.read().await.iter() {
                if *other != id
                    && instance
                        .maintenance(false)
                        .await
                        .is_ok_and(|c| c.update.is_some_and(|a| a.lock_key == key))
                {
                    sharing.push(other.clone());
                }
            }
            self.update_state(&id, ServerProviderUpdateStatus::Running, started.clone(), "Waiting for running provider work to finish.").await?;
            self.execution_gate
                .update(&sharing, async {
                    self.update_state(&id, ServerProviderUpdateStatus::Running, started.clone(), "Running provider update.").await?;
                    self.executor.execute(action).await
                })
                .await?;
            self.update_state(&id, ServerProviderUpdateStatus::Running, started.clone(), "Verifying the installed version.").await?;
            let refreshed = self.refresh_after_update(&input.provider.0, &id).await?;
            let installed_version = |provider: &ServerProvider| provider.installed_version.as_ref().and_then(Option::as_ref).or(provider.version.as_ref()).map(|version| version.0.clone());
            let failed = refreshed.is_empty() || refreshed.iter().any(|provider| !provider.installed
                || provider.status == ServerProviderState::Error && (provider.auth.status != ServerProviderAuthStatus::Unauthenticated || provider.version.is_none())
                || target.is_some_and(|target| installed_version(provider).is_none_or(|version| version.trim_start_matches('v') != target)));
            let outdated = target.is_none() && refreshed.iter().any(|provider| latest.as_deref().is_some_and(|latest| installed_version(provider).is_some_and(|version| crate::versions::compare_versions(&version, latest).is_lt())));
            if failed {
                return Ok((ServerProviderUpdateStatus::Unchanged,"Update command completed, but T3 Code could not verify the provider version."));
            }
            if outdated{return Ok((ServerProviderUpdateStatus::Unchanged,"Update command completed, but T3 Code still detects an outdated provider version."));}
            Ok((ServerProviderUpdateStatus::Succeeded, if needs_opencode_restart(&refreshed) {OPENCODE_SERVER_RESTART_MESSAGE}else{"Provider updated."}))
        }
        .await;
        self.update_state(
            &id,
            work.as_ref()
                .map(|(status, _)| *status)
                .unwrap_or(ServerProviderUpdateStatus::Failed),
            started,
            work.map(|(_, message)| message)
                .unwrap_or("Provider update could not be verified. Refresh provider settings."),
        )
        .await
    }
    pub async fn await_updates(&self) {
        let updates: Vec<_> = self.updates.lock().await.values().cloned().collect();
        for mut update in updates {
            let _ = update.wait_for(|v| v.is_some()).await;
        }
    }
}

const OPENCODE_SERVER_RESTART_MESSAGE: &str =
    "OpenCode CLI updated. Restart the configured local OpenCode server to use the new version.";
fn needs_opencode_restart(providers: &[ServerProvider]) -> bool {
    providers.iter().any(|provider| {
        provider.driver.0 == "opencode"
            && provider
                .installed_version
                .as_ref()
                .and_then(Option::as_ref)
                .zip(provider.version.as_ref())
                .is_some_and(|(installed, running)| installed != running)
    })
}

fn machine_catalog_digest(provider: &ServerProvider) -> ServiceResult<String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(&provider.slash_commands, &provider.skills))
        .map_err(|_| ServiceError::Response)?;
    Ok(format!("{:x}", Sha256::digest(bytes))[..16].to_owned())
}
fn machine_catalog_names(provider: &mut ServerProvider) -> ServiceResult<()> {
    provider.slash_commands = provider
        .slash_commands
        .iter()
        .map(|c| {
            serde_json::from_value(serde_json::json!({"name":c.name}))
                .map_err(|_| ServiceError::Response)
        })
        .collect::<ServiceResult<_>>()?;
    for skill in &mut provider.skills {
        let mut value = serde_json::to_value(&*skill).map_err(|_| ServiceError::Response)?;
        let map = value.as_object_mut().ok_or(ServiceError::Response)?;
        map.remove("description");
        map.remove("shortDescription");
        *skill = serde_json::from_value(value).map_err(|_| ServiceError::Response)?;
    }
    Ok(())
}

impl std::fmt::Debug for ProviderService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderService([REDACTED])")
    }
}
pub type ProtectedProviders = Sensitive<ServerProviderUpdatedPayload>;
