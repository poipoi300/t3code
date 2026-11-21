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

#[derive(Debug, thiserror::Error)]
pub enum RefreshProvidersError {
    #[error(transparent)]
    Service(#[from] ServiceError),
    #[error("{error}")]
    Models {
        instance_id: ProviderInstanceId,
        error: ServiceError,
    },
}
impl RefreshProvidersError {
    pub fn service_error(self) -> ServiceError {
        match self {
            Self::Service(error) | Self::Models { error, .. } => error,
        }
    }
}

#[async_trait]
pub trait ProviderInstanceService: Send + Sync {
    fn instance_id(&self) -> &ProviderInstanceId;
    fn owns_usage_state(&self) -> bool {
        false
    }
    async fn configure_update_checks(&self, _enabled: bool) -> ServiceResult<bool> {
        Ok(false)
    }
    async fn snapshot(&self, cwd: Option<&str>, fresh: bool) -> ServiceResult<ServerProvider>;
    fn subscribe_snapshots(&self) -> Option<watch::Receiver<Option<ServerProvider>>> {
        None
    }
    async fn hydrate_cached_snapshot(&self, _cached: ServerProvider) -> Option<ServerProvider> {
        None
    }
    fn has_enrichment(&self) -> bool {
        false
    }
    async fn enrich_snapshot(&self, snapshot: ServerProvider) -> ServiceResult<ServerProvider> {
        Ok(snapshot)
    }
    async fn reload_secrets(&self) -> ServiceResult<()> {
        Ok(())
    }
    async fn refresh_models(&self) -> ServiceResult<()> {
        Ok(())
    }
    async fn invalidate_usage_cache(&self) -> ServiceResult<()> {
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
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Maintenance,
            ))?;
        if result.code == Some(0) {
            Ok(())
        } else {
            Err(crate::diagnostics::failure(
                "provider.maintenance.exit",
                ServiceError::Maintenance,
                result,
            ))
        }
    }
}

pub struct ProviderService {
    self_ref: std::sync::Weak<Self>,
    watchers: Mutex<BTreeMap<ProviderInstanceId, SnapshotWatcher>>,
    pub auth: Arc<AuthService>,
    instances: RwLock<BTreeMap<ProviderInstanceId, Arc<dyn ProviderInstanceService>>>,
    snapshots: RwLock<BTreeMap<ProviderInstanceId, ServerProvider>>,
    // The driver scans the project for parked worktrees; clients address the row by worktree.
    parked: RwLock<BTreeMap<ProviderInstanceId, Vec<ServerProviderWorkspaceSnapshot>>>,
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
    status_cache: RwLock<Option<Arc<crate::status_cache::ProviderStatusCache>>>,
}
struct SnapshotWatcher(tokio::task::JoinHandle<()>);
impl Drop for SnapshotWatcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl ProviderService {
    pub async fn configure_update_checks(&self, enabled: bool) -> ServiceResult<()> {
        let instances = self
            .instances
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for instance in instances {
            instance.configure_update_checks(enabled).await?;
        }
        Ok(())
    }

    pub fn new(
        auth: Arc<AuthService>,
        executor: Arc<dyn MaintenanceExecutor>,
        catalog: Arc<dyn CatalogRefresh>,
        clock: Arc<dyn ProviderClock>,
        manifest: impl Into<crate::catalog::ManifestSource>,
        server_version: String,
    ) -> Arc<Self> {
        let (changes, _) = watch::channel(0);
        Arc::new_cyclic(|weak| Self {
            self_ref: weak.clone(),
            watchers: Mutex::new(BTreeMap::new()),
            auth,
            instances: RwLock::new(BTreeMap::new()),
            snapshots: RwLock::new(BTreeMap::new()),
            parked: Default::default(),
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
            status_cache: RwLock::new(None),
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
        let id = instance.instance_id().clone();
        let subscription = instance.subscribe_snapshots();
        let expected = Arc::downgrade(&instance);
        let mut watchers = self.watchers.lock().await;
        watchers.remove(&id);
        {
            let mut instances = self.instances.write().await;
            instances.insert(id.clone(), instance);
            self.snapshots.write().await.remove(&id);
            self.parked.write().await.remove(&id);
        }
        if let Some(mut updates) = subscription {
            let service = self.self_ref.clone();
            let task_id = id.clone();
            let task = tokio::spawn(async move {
                loop {
                    let value = updates.borrow_and_update().clone();
                    if let Some(value) = value {
                        let (Some(service), Some(expected)) =
                            (service.upgrade(), expected.upgrade())
                        else {
                            break;
                        };
                        if value.instance_id != task_id
                            || !service.store_snapshot(&expected, value).await
                        {
                            break;
                        }
                        service.publish();
                    }
                    if updates.changed().await.is_err() {
                        break;
                    }
                }
            });
            watchers.insert(id, SnapshotWatcher(task));
        }
    }
    pub async fn register_with_snapshot(
        &self,
        instance: Arc<dyn ProviderInstanceService>,
        snapshot: ServerProvider,
    ) -> ServiceResult<()> {
        if snapshot.instance_id != *instance.instance_id() {
            return Err(ServiceError::Response);
        }
        self.register(instance.clone()).await;
        self.store_snapshot(&instance, snapshot).await;
        self.publish();
        Ok(())
    }
    async fn store_snapshot(
        &self,
        instance: &Arc<dyn ProviderInstanceService>,
        mut value: ServerProvider,
    ) -> bool {
        let id = instance.instance_id();
        let current = self.instances.read().await;
        if current
            .get(id)
            .is_none_or(|current| !Arc::ptr_eq(current, instance))
        {
            return false;
        }
        let mut snapshots = self.snapshots.write().await;
        if let Some(updates) = instance.subscribe_snapshots()
            && let Some(latest) = updates.borrow().as_ref()
            && latest.instance_id == *id
        {
            value = latest.clone();
        }
        if let Some(previous) = snapshots.get(id) {
            value.update_state = previous.update_state.clone();
        }
        if let Some(cache) = self.status_cache.read().await.as_ref() {
            let _ = cache.write(&value).await;
        }
        snapshots.insert(id.clone(), value);
        true
    }
    pub async fn set_status_cache(&self, cache: Arc<crate::status_cache::ProviderStatusCache>) {
        *self.status_cache.write().await = Some(cache);
    }
    pub async fn unregister(&self, id: &ProviderInstanceId) {
        self.watchers.lock().await.remove(id);
        {
            let mut instances = self.instances.write().await;
            instances.remove(id);
            self.snapshots.write().await.remove(id);
            self.parked.write().await.remove(id);
        }
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
        let snapshots = self.snapshots.read().await;
        let parked = self.parked.read().await;
        let mut providers: Vec<_> = snapshots.values().cloned().collect();
        for provider in &mut providers {
            if let Some(rows) = parked.get(&provider.instance_id) {
                let scoped = provider.workspace_snapshots.get_or_insert_with(Vec::new);
                scoped.retain(|s| !rows.iter().any(|r| r.cwd == s.cwd));
                scoped.extend_from_slice(rows);
            }
        }
        crate::status_cache::order(&mut providers);
        ServerCompatibleArray(providers)
    }
    pub async fn refresh_providers(
        &self,
        input: WsServerRefreshProvidersPayload,
    ) -> ServiceResult<ServerProviderUpdatedPayload> {
        self.refresh_providers_at(input, None)
            .await
            .map_err(RefreshProvidersError::service_error)
    }
    pub async fn refresh_providers_at(
        &self,
        input: WsServerRefreshProvidersPayload,
        scan_cwd: Option<&str>,
    ) -> Result<ServerProviderUpdatedPayload, RefreshProvidersError> {
        if input.model_catalog_only.flatten() == Some(true) {
            if let Err(error) = self.catalog.refresh().await {
                tracing::warn!(%error, "Could not refresh provider model catalog");
            }
            return Ok(ServerProviderUpdatedPayload {
                providers: ServerCompatibleArray(vec![]),
            });
        }
        let _gate = self.refresh_gate.lock().await;
        let id = input.instance_id.as_ref().and_then(Option::as_ref);
        let cwd = id
            .and(input.cwd.as_ref().and_then(Option::as_ref))
            .map(|s| s.0.as_str());
        if let (Some(id), Some(cwd), Some(true)) = (id, cwd, input.fresh.flatten()) {
            let mut snapshots = self.snapshots.write().await;
            let mut parked = self.parked.write().await;
            for provider in snapshots.values_mut().filter(|p| &p.instance_id != id) {
                if let Some(rows) = provider.workspace_snapshots.as_mut() {
                    rows.retain(|row| row.cwd.0 != cwd);
                }
            }
            parked.retain(|owner, rows| {
                if owner != id {
                    rows.retain(|row| row.cwd.0 != cwd);
                }
                !rows.is_empty()
            });
            self.publish();
        }
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
        let refresh_models = input.refresh_models.flatten() == Some(true);
        let fresh = input.fresh.flatten() == Some(true);
        for instance in &instances {
            if refresh_models {
                instance.invalidate_models(cwd, fresh).await?;
            }
            if cwd.is_none()
                && input.reload_secrets.flatten() == Some(true)
                && let Err(error) = instance.reload_secrets().await
            {
                tracing::warn!(%error, "Could not reload provider secrets; preserving cached provider");
                continue;
            }
            let snapshot = match instance.snapshot(scan_cwd.or(cwd), fresh).await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(%error, "Provider probe failed; preserving cached provider");
                    continue;
                }
            };
            if let Some(cwd) = cwd {
                let current = self.instances.read().await;
                if current
                    .get(instance.instance_id())
                    .is_some_and(|registered| Arc::ptr_eq(registered, instance))
                {
                    let mut parked = self.parked.write().await;
                    let id = instance.instance_id();
                    if let Some(rows) = parked.get_mut(id) {
                        rows.retain(|row| row.cwd.0 != cwd);
                        if rows.is_empty() {
                            parked.remove(id);
                        }
                    }
                    if let Some(scan) = scan_cwd.filter(|scan| *scan != cwd)
                        && let Some(mut row) = snapshot
                            .workspace_snapshots
                            .iter()
                            .flatten()
                            .find(|s| s.cwd.0 == scan)
                            .cloned()
                    {
                        row.cwd = cwd.to_owned().into();
                        let rows = parked.entry(id.clone()).or_default();
                        rows.push(row);
                        if rows.len() > 16 {
                            rows.remove(0);
                        }
                    }
                }
            }
            if self.store_snapshot(instance, snapshot).await {
                self.publish();
            }
        }
        if refresh_models {
            for instance in instances {
                let id = instance.instance_id();
                if !self
                    .instances
                    .read()
                    .await
                    .get(id)
                    .is_some_and(|current| Arc::ptr_eq(current, &instance))
                    || !self
                        .snapshots
                        .read()
                        .await
                        .get(id)
                        .is_some_and(|current| current.enabled && current.installed)
                {
                    continue;
                }
                instance
                    .refresh_models()
                    .await
                    .map_err(|error| RefreshProvidersError::Models {
                        instance_id: id.clone(),
                        error,
                    })?;
                match instance.snapshot(None, false).await {
                    Ok(snapshot) => {
                        if self.store_snapshot(&instance, snapshot).await {
                            self.publish();
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Provider probe after model refresh failed; preserving cached provider")
                    }
                }
            }
        }
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
            .map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Maintenance,
            ))?;

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
                    drop(current);
                    if self.store_snapshot(&instance, value.clone()).await {
                        verified.push(value);
                    }
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
    let bytes = serde_json::to_vec(&(&provider.slash_commands, &provider.skills)).map_err(
        crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response),
    )?;
    Ok(format!("{:x}", Sha256::digest(bytes))[..16].to_owned())
}
fn machine_catalog_names(provider: &mut ServerProvider) -> ServiceResult<()> {
    provider.slash_commands = provider
        .slash_commands
        .iter()
        .map(|c| {
            serde_json::from_value(serde_json::json!({"name":c.name})).map_err(
                crate::diagnostics::mapped(
                    concat!(module_path!(), "::error"),
                    ServiceError::Response,
                ),
            )
        })
        .collect::<ServiceResult<_>>()?;
    for skill in &mut provider.skills {
        let mut value = serde_json::to_value(&*skill).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
        let map = value.as_object_mut().ok_or(ServiceError::Response)?;
        map.remove("description");
        map.remove("shortDescription");
        *skill = serde_json::from_value(value).map_err(crate::diagnostics::mapped(
            concat!(module_path!(), "::error"),
            ServiceError::Response,
        ))?;
    }
    Ok(())
}

impl std::fmt::Debug for ProviderService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderService([REDACTED])")
    }
}
pub type ProtectedProviders = Sensitive<ServerProviderUpdatedPayload>;
