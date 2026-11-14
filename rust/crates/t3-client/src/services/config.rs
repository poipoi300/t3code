use crate::{
    ClientError,
    platform::{Platform, StorageArea, StorageKey, StorageWrite},
    types::{EnvironmentId, Generation},
};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
use t3_contracts::{provider_instance::ProviderInstanceId, server::*};

pub fn reuse_unchanged<T: PartialEq>(previous: Option<&Rc<T>>, next: T) -> Rc<T> {
    previous
        .filter(|previous| previous.as_ref() == &next)
        .cloned()
        .unwrap_or_else(|| Rc::new(next))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    Cache,
    Live,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfigProjection {
    pub config: Rc<ServerConfig>,
    pub latest_event: ServerConfigStreamEvent,
    pub source: ConfigSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SharedServerProvider {
    pub source: Rc<ServerProvider>,
    pub models: Rc<Vec<ServerProviderModel>>,
    pub slash_commands: Rc<Vec<ServerProviderSlashCommand>>,
    pub skills: Rc<Vec<ServerProviderSkill>>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SharedServerConfig {
    pub source: Rc<ServerConfig>,
    pub settings: Rc<t3_contracts::settings::ServerSettings>,
    pub favorites: Rc<Option<t3_contracts::settings::ModelFavorites>>,
    pub provider_instances: std::collections::BTreeMap<
        ProviderInstanceId,
        Rc<t3_contracts::provider_instance::ProviderInstanceConfig>,
    >,
    pub providers: Rc<Vec<SharedServerProvider>>,
}
impl SharedServerConfig {
    pub fn update(source: Rc<ServerConfig>, previous: Option<&Self>) -> Self {
        let providers = source
            .providers
            .0
            .iter()
            .map(|provider| {
                let previous = previous.and_then(|previous| {
                    previous
                        .providers
                        .iter()
                        .find(|old| old.source.instance_id == provider.instance_id)
                });
                SharedServerProvider {
                    source: reuse_unchanged(
                        previous.map(|previous| &previous.source),
                        provider.clone(),
                    ),
                    models: reuse_unchanged(
                        previous.map(|previous| &previous.models),
                        provider.models.clone(),
                    ),
                    slash_commands: reuse_unchanged(
                        previous.map(|previous| &previous.slash_commands),
                        provider.slash_commands.clone(),
                    ),
                    skills: reuse_unchanged(
                        previous.map(|previous| &previous.skills),
                        provider.skills.clone(),
                    ),
                }
            })
            .collect();
        Self {
            settings: reuse_unchanged(
                previous.map(|previous| &previous.settings),
                source.settings.clone(),
            ),
            favorites: reuse_unchanged(
                previous.map(|previous| &previous.favorites),
                source.settings.favorites.clone(),
            ),
            provider_instances: source
                .settings
                .provider_instances
                .iter()
                .map(|(id, value)| {
                    (
                        id.clone(),
                        reuse_unchanged(
                            previous.and_then(|previous| previous.provider_instances.get(id)),
                            value.clone(),
                        ),
                    )
                })
                .collect(),
            providers: reuse_unchanged(previous.map(|previous| &previous.providers), providers),
            source,
        }
    }
}
pub fn without_environment_themes(mut config: ServerConfig) -> ServerConfig {
    config.environment_themes = None;
    config.usage_limit_sources = None;
    config
}
pub fn cacheable_config(config: &ServerConfig) -> ServerConfig {
    let mut config = without_environment_themes(config.clone());
    config.last_event_loop_stall = None;
    config.server_time = None;
    config
}
fn held_catalogs(previous: &[ServerProvider], providers: &mut [ServerProvider]) {
    for provider in providers {
        if provider.catalog_digest.is_none() || provider.loaded_catalog_digest.is_some() {
            continue;
        }
        if let Some(held) = previous
            .iter()
            .find(|held| held.instance_id == provider.instance_id)
        {
            provider.slash_commands.clone_from(&held.slash_commands);
            provider.skills.clone_from(&held.skills);
            provider
                .loaded_catalog_digest
                .clone_from(&held.loaded_catalog_digest);
        }
    }
}
pub fn apply_provider_deltas(
    previous: &[ServerProvider],
    deltas: &[ServerProviderDelta],
) -> Result<Vec<ServerProvider>, ClientError> {
    deltas
        .iter()
        .filter_map(|delta| {
            let held = previous
                .iter()
                .find(|provider| provider.instance_id == delta.instance_id);
            let models = delta
                .models
                .as_ref()
                .or_else(|| held.map(|provider| &provider.models));
            let commands = delta
                .slash_commands
                .as_ref()
                .or_else(|| held.map(|provider| &provider.slash_commands));
            let skills = delta
                .skills
                .as_ref()
                .or_else(|| held.map(|provider| &provider.skills));
            let (Some(models), Some(commands), Some(skills)) = (models, commands, skills) else {
                return None;
            };
            Some((|| {
                let mut value = serde_json::to_value(delta)?;
                value["models"] = serde_json::to_value(models)?;
                value["slashCommands"] = serde_json::to_value(commands)?;
                value["skills"] = serde_json::to_value(skills)?;
                Ok(serde_json::from_value(value)?)
            })())
        })
        .collect()
}
pub fn apply_server_config_projection(
    current: Option<&ServerConfigProjection>,
    event: ServerConfigStreamEvent,
) -> Result<Option<ServerConfigProjection>, ClientError> {
    let mut config = match &event {
        ServerConfigStreamEvent::Snapshot(event) => {
            let mut config = event.config.clone();
            if let Some(current) = current {
                config
                    .environment_themes
                    .clone_from(&current.config.environment_themes);
                config
                    .usage_limit_sources
                    .clone_from(&current.config.usage_limit_sources);
                held_catalogs(&current.config.providers.0, &mut config.providers.0);
            }
            config
        }
        _ => {
            let Some(current) = current else {
                return Ok(None);
            };
            current.config.as_ref().clone()
        }
    };
    match &event {
        ServerConfigStreamEvent::KeybindingsUpdated(event) => {
            config.keybindings = event.payload.keybindings.clone();
            config.issues = event.payload.issues.clone();
        }
        ServerConfigStreamEvent::ProviderStatuses(event) => {
            config.providers = event.payload.providers.clone();
            if let Some(current) = current {
                held_catalogs(&current.config.providers.0, &mut config.providers.0);
            }
        }
        ServerConfigStreamEvent::ProviderStatusDeltas(event) => {
            let previous = config.providers.0;
            config.providers.0 = apply_provider_deltas(&previous, &event.payload.providers.0)?;
            held_catalogs(&previous, &mut config.providers.0);
        }
        ServerConfigStreamEvent::SettingsUpdated(event) => {
            config.settings = event.payload.settings.clone()
        }
        ServerConfigStreamEvent::EnvironmentThemesUpdated(event) => {
            config.environment_themes =
                (!event.payload.themes.is_empty()).then(|| Some(event.payload.themes.clone()))
        }
        ServerConfigStreamEvent::UsageLimitSourcesUpdated(event) => {
            config.usage_limit_sources =
                (!event.payload.sources.0.is_empty()).then(|| Some(event.payload.sources.clone()))
        }
        ServerConfigStreamEvent::Snapshot(_) => {}
    }
    Ok(Some(ServerConfigProjection {
        config: reuse_unchanged(current.map(|current| &current.config), config),
        latest_event: event,
        source: ConfigSource::Live,
    }))
}
pub fn resolve_server_config<'a>(
    projection: Option<&'a ServerConfigProjection>,
    initial: Option<&'a ServerConfig>,
) -> Option<&'a ServerConfig> {
    if let Some(projection) = projection
        && projection.source == ConfigSource::Live
        && initial.is_none_or(|initial| {
            initial.environment.server_version == projection.config.environment.server_version
        })
    {
        return Some(&projection.config);
    }
    initial.or_else(|| projection.map(|projection| projection.config.as_ref()))
}
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedMachineCatalog {
    pub instance_id: ProviderInstanceId,
    pub digest: String,
    pub slash_commands: Vec<ServerProviderSlashCommand>,
    pub skills: Vec<ServerProviderSkill>,
}
fn with_machine_catalogs(config: &mut ServerConfig, catalogs: &[FetchedMachineCatalog]) {
    for provider in &mut config.providers.0 {
        let Some(catalog) = catalogs.iter().find(|catalog| {
            catalog.instance_id == provider.instance_id
                && provider
                    .catalog_digest
                    .as_ref()
                    .is_some_and(|digest| digest.0.as_str() == catalog.digest)
                && provider.loaded_catalog_digest != provider.catalog_digest
        }) else {
            continue;
        };
        provider.slash_commands.clone_from(&catalog.slash_commands);
        provider.skills.clone_from(&catalog.skills);
        provider
            .loaded_catalog_digest
            .clone_from(&provider.catalog_digest);
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedWorkspaceSnapshot {
    pub instance_id: ProviderInstanceId,
    pub snapshot: ServerProviderWorkspaceSnapshot,
    basis: Option<Value>,
}
fn snapshot_basis(provider: Option<&ServerProvider>) -> Option<Value> {
    provider.map(|provider| {
        let mut basis = json!({ "enabled": provider.enabled, "models": provider.models });
        if let Some(digest) = &provider.catalog_digest {
            basis["catalogDigest"] = json!(digest);
        } else {
            basis["slashCommands"] = json!(provider.slash_commands);
            basis["skills"] = json!(provider.skills);
        }
        basis
    })
}
pub fn record_workspace_snapshot(
    current: &mut Vec<FetchedWorkspaceSnapshot>,
    instance: &ProviderInstanceId,
    cwd: &str,
    fresh: bool,
    providers: &[ServerProvider],
    config_provider: Option<&ServerProvider>,
) -> bool {
    let basis = snapshot_basis(config_provider);
    let snapshot = providers
        .iter()
        .find(|provider| &provider.instance_id == instance)
        .and_then(|provider| provider.workspace_snapshots.as_ref())
        .and_then(|snapshots| {
            snapshots
                .iter()
                .find(|snapshot| snapshot.cwd.0.as_str() == cwd)
        });
    if !fresh
        && snapshot.is_some_and(|snapshot| {
            current.iter().any(|stored| {
                &stored.instance_id == instance
                    && stored.snapshot == *snapshot
                    && stored.basis == basis
            })
        })
    {
        return false;
    }
    let before = current.len();
    current.retain(|entry| {
        entry.snapshot.cwd.0.as_str() != cwd || (!fresh && &entry.instance_id != instance)
    });
    let changed = before != current.len();
    if let Some(snapshot) = snapshot {
        current.push(FetchedWorkspaceSnapshot {
            instance_id: instance.clone(),
            snapshot: snapshot.clone(),
            basis,
        });
        if current.len() > 32 {
            current.drain(..current.len() - 32);
        }
        return true;
    }
    changed
}
pub fn with_workspace_snapshots(
    config: &ServerConfig,
    fetched: &[FetchedWorkspaceSnapshot],
) -> ServerConfig {
    let mut config = config.clone();
    if config
        .environment
        .capabilities
        .provider_workspace_snapshots_on_demand
        != Some(true)
    {
        return config;
    }
    for provider in &mut config.providers.0 {
        let own: Vec<_> = fetched
            .iter()
            .filter(|entry| {
                entry.instance_id == provider.instance_id
                    && entry.basis == snapshot_basis(Some(provider))
            })
            .map(|entry| entry.snapshot.clone())
            .collect();
        if !own.is_empty() {
            provider.workspace_snapshots = Some(own);
        }
    }
    config
}
#[derive(Debug, Clone, Default)]
pub struct ServerWelcomeState {
    pub session: Option<Generation>,
    welcome_session: Option<Generation>,
    welcome: Option<ServerLifecycleWelcomePayload>,
}
impl ServerWelcomeState {
    pub fn replace_session(&mut self, session: Option<Generation>) {
        self.session = session;
    }
    pub fn subscribe(&mut self, session: Generation) {
        if self.session == Some(session) {
            self.welcome_session = Some(session);
            self.welcome = None;
        }
    }
    pub fn apply(&mut self, session: Generation, event: ServerLifecycleStreamEvent) {
        if self.session == Some(session)
            && let ServerLifecycleStreamEvent::Welcome(event) = event
        {
            self.welcome_session = Some(session);
            self.welcome = Some(event.payload);
        }
    }
    pub fn value(&self) -> Option<&ServerLifecycleWelcomePayload> {
        (self.session == self.welcome_session)
            .then_some(self.welcome.as_ref())
            .flatten()
    }
}

#[derive(Default)]
struct ConfigInner {
    session: Generation,
    projection: Option<ServerConfigProjection>,
    catalogs: Vec<FetchedMachineCatalog>,
    workspace: Vec<FetchedWorkspaceSnapshot>,
    pending: Option<Rc<ServerConfig>>,
    persisted: Option<ServerConfig>,
    flushing: bool,
    closed: bool,
    pending_write: Option<futures::future::AbortHandle>,
    shared: Option<Rc<SharedServerConfig>>,
}
#[derive(Clone)]
pub struct ServerConfigService {
    pub environment: EnvironmentId,
    inner: Rc<RefCell<ConfigInner>>,
    changes: super::resources::Resource<()>,
}
impl ServerConfigService {
    pub fn new(environment: EnvironmentId) -> Self {
        Self {
            environment,
            inner: Rc::new(RefCell::new(ConfigInner::default())),
            changes: super::resources::Resource::new(Default::default(), Generation::default()),
        }
    }
    pub fn begin_session(&self, session: Generation) {
        self.inner.borrow_mut().session = session;
    }
    pub fn close(&self) {
        let active = {
            let mut inner = self.inner.borrow_mut();
            inner.closed = true;
            inner.pending = None;
            inner.pending_write.take()
        };
        if let Some(active) = active {
            active.abort();
        }
    }
    pub fn snapshot(&self) -> Option<ServerConfigProjection> {
        self.inner.borrow().projection.clone()
    }
    pub fn observe(&self, now: u64) -> super::resources::ResourceSubscription<()> {
        self.changes.observe(now)
    }
    fn notify(&self) {
        let token = self.changes.begin();
        self.changes.finish(token, (), 0, false);
    }
    pub fn shared(&self) -> Option<Rc<SharedServerConfig>> {
        let mut inner = self.inner.borrow_mut();
        let config = with_workspace_snapshots(&inner.projection.as_ref()?.config, &inner.workspace);
        let source = reuse_unchanged(
            inner.shared.as_ref().map(|previous| &previous.source),
            config,
        );
        if let Some(previous) = &inner.shared
            && Rc::ptr_eq(&source, &previous.source)
        {
            return Some(previous.clone());
        }
        let next = Rc::new(SharedServerConfig::update(source, inner.shared.as_deref()));
        inner.shared = Some(next.clone());
        Some(next)
    }
    pub fn config(&self, initial: Option<&ServerConfig>) -> Option<ServerConfig> {
        let inner = self.inner.borrow();
        resolve_server_config(inner.projection.as_ref(), initial)
            .map(|config| with_workspace_snapshots(config, &inner.workspace))
    }
    pub fn restore(&self, config: ServerConfig) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.closed
            || inner
                .projection
                .as_ref()
                .is_some_and(|projection| projection.source == ConfigSource::Live)
        {
            return false;
        }
        let config = without_environment_themes(config);
        let event = ServerConfigStreamEvent::Snapshot(Box::new(ServerConfigStreamSnapshotEvent {
            version: t3_contracts::resource_telemetry::WireLiteral,
            r#type: ServerConfigStreamSnapshotType::Snapshot,
            config: config.clone(),
        }));
        inner.projection = Some(ServerConfigProjection {
            config: Rc::new(config),
            latest_event: event,
            source: ConfigSource::Cache,
        });
        drop(inner);
        self.notify();
        true
    }
    pub fn apply(
        &self,
        session: Generation,
        event: ServerConfigStreamEvent,
    ) -> Result<bool, ClientError> {
        let mut inner = self.inner.borrow_mut();
        if inner.closed || inner.session != session {
            return Ok(false);
        }
        let Some(mut projection) =
            apply_server_config_projection(inner.projection.as_ref(), event)?
        else {
            return Ok(false);
        };
        let mut config = projection.config.as_ref().clone();
        with_machine_catalogs(&mut config, &inner.catalogs);
        projection.config = reuse_unchanged(Some(&projection.config), config);
        inner.pending = Some(projection.config.clone());
        inner.projection = Some(projection);
        drop(inner);
        self.notify();
        Ok(true)
    }
    pub fn record_catalog(&self, session: Generation, catalog: FetchedMachineCatalog) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.closed || inner.session != session {
            return false;
        }
        inner
            .catalogs
            .retain(|entry| entry.instance_id != catalog.instance_id);
        inner.catalogs.push(catalog);
        let Some(projection) = &inner.projection else {
            return true;
        };
        let mut config = projection.config.as_ref().clone();
        with_machine_catalogs(&mut config, &inner.catalogs);
        let config = reuse_unchanged(Some(&projection.config), config);
        if !Rc::ptr_eq(&config, &projection.config) {
            if let Some(projection) = &mut inner.projection {
                projection.config = config.clone();
            }
            inner.pending = Some(config);
        }
        drop(inner);
        self.notify();
        true
    }
    pub fn record_workspace(
        &self,
        session: Generation,
        instance: &ProviderInstanceId,
        cwd: &str,
        fresh: bool,
        providers: &[ServerProvider],
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.closed || inner.session != session {
            return false;
        }
        let provider = inner
            .projection
            .as_ref()
            .and_then(|projection| {
                projection
                    .config
                    .providers
                    .0
                    .iter()
                    .find(|provider| &provider.instance_id == instance)
            })
            .cloned();
        let changed = record_workspace_snapshot(
            &mut inner.workspace,
            instance,
            cwd,
            fresh,
            providers,
            provider.as_ref(),
        );
        drop(inner);
        if changed {
            self.notify();
        }
        changed
    }
    fn cache_key(&self) -> StorageKey {
        StorageKey {
            namespace: "server-config".into(),
            environment: Some(self.environment.clone()),
            record: "current".into(),
        }
    }
    pub async fn load(&self, platform: &(impl Platform + ?Sized)) -> Result<bool, ClientError> {
        let session = self.inner.borrow().session;
        let Some(bytes) = platform
            .get(StorageArea::Ordinary, self.cache_key())
            .await?
        else {
            return Ok(false);
        };
        let record: crate::persistence::Record<ServerConfig> = match serde_json::from_slice(&bytes)
        {
            Ok(record) => record,
            Err(_) => return Ok(false),
        };
        if self.inner.borrow().session != session
            || record.format_version != 1
            || record.kind != "server-config"
            || record.environment_id.as_ref() != Some(&self.environment)
            || record.record_id != "current"
            || record.payload_version != 1
        {
            return Ok(false);
        }
        Ok(self.restore(record.payload))
    }
    pub async fn flush(&self, platform: &(impl Platform + ?Sized)) -> Result<(), ClientError> {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.closed {
                return Err(ClientError::Cancelled);
            }
            if inner.flushing {
                return Ok(());
            }
            inner.flushing = true;
        }
        let _guard = FlushGuard(self.inner.clone());
        loop {
            let Some(pending) = self.inner.borrow().pending.clone() else {
                return Ok(());
            };
            let cacheable = cacheable_config(&pending);
            if self.inner.borrow().persisted.as_ref() != Some(&cacheable) {
                let record = crate::persistence::Record {
                    format_version: 1,
                    kind: "server-config".into(),
                    environment_id: Some(self.environment.clone()),
                    record_id: "current".into(),
                    payload_version: 1,
                    payload: cacheable.clone(),
                };
                let (abort, registration) = futures::future::AbortHandle::new_pair();
                self.inner.borrow_mut().pending_write = Some(abort);
                let write = platform.write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: self.cache_key(),
                        value: Some(serde_json::to_vec(&record)?),
                    }],
                );
                futures::future::Abortable::new(write, registration)
                    .await
                    .map_err(|_| ClientError::Cancelled)??;
                self.inner.borrow_mut().pending_write = None;
                self.inner.borrow_mut().persisted = Some(cacheable);
            }
            let mut inner = self.inner.borrow_mut();
            if inner
                .pending
                .as_ref()
                .is_some_and(|current| Rc::ptr_eq(current, &pending))
            {
                inner.pending = None;
            }
        }
    }
}
struct FlushGuard(Rc<RefCell<ConfigInner>>);
impl Drop for FlushGuard {
    fn drop(&mut self) {
        let mut inner = self.0.borrow_mut();
        inner.flushing = false;
        inner.pending_write = None;
    }
}

/// Kept for existing callers (t3-ui); new code uses [`crate::time::epoch_ms`].
pub use crate::time::epoch_ms as timestamp_ms;
