use crate::{
    environment::{Environment, Platform, merge_environment},
    secrets::SecretResolver,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};
use t3_contracts::{
    provider_instance::*, secret_reference::SecretReference, settings::ServerSettings,
};
use tokio::sync::{Mutex, RwLock, mpsc};

pub const BUILT_IN_DRIVERS: &[&str] = &[
    "codex",
    "claudeAgent",
    "cursor",
    "grok",
    "fx",
    "opencode",
    "antigravity",
    "kiro",
    "pi",
    "muse",
    "acpRegistry",
];

pub fn config_enabled(config: Option<&Value>) -> Option<bool> {
    config
        .and_then(|c| c.get("enabled"))
        .and_then(Value::as_bool)
}

pub fn stored_enabled(entry: &ProviderInstanceConfig) -> bool {
    if entry.enabled == Some(false) || config_enabled(entry.config.as_ref()) == Some(false) {
        return false;
    }
    entry
        .enabled
        .or_else(|| config_enabled(entry.config.as_ref()))
        .unwrap_or(!matches!(
            entry.driver.0.as_str(),
            "cursor" | "grok" | "fx" | "kiro" | "muse" | "pi" | "opencode" | "antigravity"
        ))
}

pub fn resolve_enabled(entry: &ProviderInstanceConfig, decoded: &Value) -> bool {
    if entry.enabled == Some(false) || config_enabled(entry.config.as_ref()) == Some(false) {
        return false;
    }
    entry
        .enabled
        .or_else(|| config_enabled(Some(decoded)))
        .unwrap_or(true)
}

pub fn derive_config_map(settings: &ServerSettings) -> ProviderInstanceConfigMap {
    let mut map = settings.provider_instances.clone();
    for kind in BUILT_IN_DRIVERS.iter().filter(|k| **k != "acpRegistry") {
        map.entry(ProviderInstanceId((*kind).into()))
            .or_insert_with(|| ProviderInstanceConfig {
                driver: ProviderDriverKind((*kind).into()),
                display_name: None,
                accent_color: None,
                environment: None,
                enabled: None,
                config: None,
            });
    }
    for overrides in settings.project_settings_overrides.values() {
        for (id, override_) in overrides.provider_instances.iter().flatten() {
            if override_.enabled != Some(true) {
                continue;
            }
            if let Some(entry) = map.get_mut(id)
                && !stored_enabled(entry)
            {
                entry.enabled = Some(true);
                if config_enabled(entry.config.as_ref()) == Some(false)
                    && let Some(Value::Object(config)) = &mut entry.config
                {
                    config.insert("enabled".into(), Value::Bool(true));
                }
            }
        }
    }
    map
}

pub type ConfigDecoder = fn(Option<&Value>) -> std::result::Result<Value, String>;

/// The owning driver supplies its settings codec; this does not define an adapter seam.
pub struct DriverDefinition {
    pub kind: ProviderDriverKind,
    pub decode_config: ConfigDecoder,
    pub secret_fields: Vec<String>,
}

#[derive(Clone)]
pub struct InstanceSpec {
    pub instance_id: ProviderInstanceId,
    pub driver: ProviderDriverKind,
    pub display_name: Option<String>,
    pub accent_color: Option<String>,
    pub enabled: bool,
    pub config: Value,
    pub variables: ResolvedProviderInstanceEnvironment,
}

impl InstanceSpec {
    pub fn environment(&self, base: &Environment, home: &Path, platform: Platform) -> Environment {
        merge_environment(&self.variables, base, home, platform)
    }
    pub fn default_continuation_key(&self) -> String {
        format!("{}:instance:{}", self.driver.0, self.instance_id.0)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum UnavailableReason {
    Blocked(String),
    UnknownDriver,
    InvalidConfig(String),
    ReadingSecrets,
    UnreadableSecrets(Vec<String>),
}
impl std::fmt::Debug for UnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Blocked(_) => "Blocked([REDACTED])",
            Self::UnknownDriver => "UnknownDriver",
            Self::InvalidConfig(_) => "InvalidConfig([REDACTED])",
            Self::ReadingSecrets => "ReadingSecrets",
            Self::UnreadableSecrets(_) => "UnreadableSecrets([REDACTED])",
        })
    }
}
impl UnavailableReason {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Blocked(_) => "This provider is not available under the managed policy.",
            Self::UnknownDriver => "This provider driver is not available.",
            Self::InvalidConfig(_) => "This provider instance has invalid settings.",
            Self::ReadingSecrets => "Reading provider secrets...",
            Self::UnreadableSecrets(_) => {
                "Could not read this provider instance's configured secrets."
            }
        }
    }
}

#[derive(Clone)]
pub enum InstanceStatus {
    Prepared(Arc<InstanceSpec>),
    Unavailable(UnavailableReason),
}

#[derive(Clone)]
pub struct RegistryEntry {
    pub source: ProviderInstanceConfig,
    pub status: InstanceStatus,
}

#[derive(Default)]
struct State {
    entries: BTreeMap<ProviderInstanceId, RegistryEntry>,
    subscribers: Vec<mpsc::UnboundedSender<()>>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileChanges {
    pub retired: Vec<ProviderInstanceId>,
    pub prepared: Vec<ProviderInstanceId>,
}

/// Reconciles configuration and credentials. The caller retires runtimes before activating replacements.
pub struct InstanceRegistry {
    drivers: BTreeMap<ProviderDriverKind, DriverDefinition>,
    gate: Mutex<()>,
    state: RwLock<State>,
}

impl InstanceRegistry {
    pub fn new(drivers: impl IntoIterator<Item = DriverDefinition>) -> Self {
        Self {
            drivers: drivers.into_iter().map(|d| (d.kind.clone(), d)).collect(),
            gate: Mutex::new(()),
            state: RwLock::new(State::default()),
        }
    }
    pub async fn subscribe(&self) -> mpsc::UnboundedReceiver<()> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state.write().await.subscribers.push(tx);
        rx
    }
    pub async fn list(&self) -> BTreeMap<ProviderInstanceId, RegistryEntry> {
        self.state.read().await.entries.clone()
    }
    pub async fn get(&self, id: &ProviderInstanceId) -> Option<RegistryEntry> {
        self.state.read().await.entries.get(id).cloned()
    }

    pub async fn reconcile(
        &self,
        settings: &ServerSettings,
        resolver: &SecretResolver,
        rebuild: &BTreeSet<ProviderInstanceId>,
        deferred: bool,
        blocked: &BTreeMap<ProviderDriverKind, String>,
    ) -> ReconcileChanges {
        let _gate = self.gate.lock().await;
        self.reconcile_locked(
            settings,
            resolver,
            rebuild,
            deferred,
            blocked,
            &BTreeMap::new(),
        )
        .await
    }
    async fn reconcile_locked(
        &self,
        settings: &ServerSettings,
        resolver: &SecretResolver,
        rebuild: &BTreeSet<ProviderInstanceId>,
        deferred: bool,
        blocked: &BTreeMap<ProviderDriverKind, String>,
        carried: &BTreeMap<ProviderInstanceId, InstanceStatus>,
    ) -> ReconcileChanges {
        let configs = derive_config_map(settings);
        let previous = self.state.read().await.entries.clone();
        let mut changes = ReconcileChanges::default();
        let mut next = BTreeMap::new();
        let mut plans = Vec::new();
        let mut references = Vec::new();
        for (id, entry) in &configs {
            if let Some(status) = carried.get(id)
                && !blocked.contains_key(&entry.driver)
            {
                next.insert(
                    id.clone(),
                    RegistryEntry {
                        source: entry.clone(),
                        status: status.clone(),
                    },
                );
                continue;
            }
            if !rebuild.contains(id)
                && let Some(old) = previous.get(id)
                && old.source == *entry
                && match (&old.status, blocked.get(&entry.driver)) {
                    (
                        InstanceStatus::Unavailable(UnavailableReason::Blocked(previous)),
                        Some(current),
                    ) => previous == current,
                    (InstanceStatus::Unavailable(UnavailableReason::Blocked(_)), None) => false,
                    (_, Some(_)) => false,
                    (_, None) => true,
                }
                && !matches!(
                    old.status,
                    InstanceStatus::Unavailable(UnavailableReason::ReadingSecrets)
                )
            {
                next.insert(id.clone(), old.clone());
                continue;
            }
            let mut failure = blocked
                .get(&entry.driver)
                .cloned()
                .map(UnavailableReason::Blocked);
            let driver = self.drivers.get(&entry.driver);
            let decoded = match driver {
                None => {
                    failure.get_or_insert(UnavailableReason::UnknownDriver);
                    None
                }
                Some(driver) if failure.is_none() => {
                    match (driver.decode_config)(entry.config.as_ref()) {
                        Ok(config) => Some(config),
                        Err(detail) => {
                            failure = Some(UnavailableReason::InvalidConfig(detail));
                            None
                        }
                    }
                }
                _ => None,
            };
            if let Some(failure) = failure {
                next.insert(
                    id.clone(),
                    RegistryEntry {
                        source: entry.clone(),
                        status: InstanceStatus::Unavailable(failure),
                    },
                );
                continue;
            }
            let Some(decoded) = decoded else {
                continue;
            };
            let enabled = resolve_enabled(entry, &decoded);
            let slots = if enabled {
                self.secret_slots(entry, &decoded)
            } else {
                vec![]
            };
            if deferred && !slots.is_empty() {
                next.insert(
                    id.clone(),
                    RegistryEntry {
                        source: entry.clone(),
                        status: InstanceStatus::Unavailable(UnavailableReason::ReadingSecrets),
                    },
                );
                continue;
            }
            references.extend(slots.iter().map(|(_, r)| r.clone()));
            plans.push((id.clone(), entry.clone(), decoded, enabled, slots));
        }
        let values = resolver
            .read(&settings.secret_providers, &references, false)
            .await;
        let mut values = values.into_iter();
        for (id, entry, config, enabled, slots) in plans {
            let status = resolved_spec(&id, &entry, config, enabled, &slots, &mut values);
            next.insert(
                id,
                RegistryEntry {
                    source: entry,
                    status,
                },
            );
        }
        for (id, old) in &previous {
            if let InstanceStatus::Prepared(old)=&old.status
                && !next.get(id).is_some_and(|next|matches!(&next.status,InstanceStatus::Prepared(next) if Arc::ptr_eq(old,next)))
            {
                changes.retired.push(id.clone());
            }
        }
        for (id, entry) in &next {
            if let InstanceStatus::Prepared(spec) = &entry.status {
                let kept = previous.get(id).is_some_and(|old| matches!(&old.status,InstanceStatus::Prepared(before) if Arc::ptr_eq(before,spec)));
                if !kept {
                    changes.prepared.push(id.clone());
                }
            }
        }
        let mut state = self.state.write().await;
        state.entries = next;
        state
            .subscribers
            .retain(|subscriber| subscriber.send(()).is_ok());
        changes
    }
}

fn resolved_spec(
    id: &ProviderInstanceId,
    entry: &ProviderInstanceConfig,
    mut config: Value,
    enabled: bool,
    slots: &[(String, SecretReference)],
    values: &mut impl Iterator<Item = Option<String>>,
) -> InstanceStatus {
    let mut resolved = BTreeMap::new();
    let mut unreadable = vec![];
    for (slot, _) in slots {
        if let Some(value) = values
            .next()
            .flatten()
            .filter(|value| !value.trim().is_empty())
        {
            resolved.insert(slot.clone(), value);
        } else {
            unreadable.push(slot.clone());
        }
    }
    if !unreadable.is_empty() {
        return InstanceStatus::Unavailable(UnavailableReason::UnreadableSecrets(unreadable));
    }
    let variables = entry
        .environment
        .iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, variable)| {
            let value = if variable.secret.is_some() {
                resolved.get(&format!("environment:{index}"))?.clone()
            } else {
                variable.value.clone()
            };
            Some(ResolvedProviderInstanceEnvironmentVariable {
                name: variable.name.clone(),
                value,
                sensitive: variable.sensitive || variable.secret.is_some(),
                value_redacted: variable.value_redacted,
            })
        })
        .collect();
    for (slot, value) in resolved {
        if !slot.starts_with("environment:") {
            config[slot] = Value::String(value);
        }
    }
    InstanceStatus::Prepared(Arc::new(InstanceSpec {
        instance_id: id.clone(),
        driver: entry.driver.clone(),
        display_name: entry.display_name.clone().flatten(),
        accent_color: entry.accent_color.clone().flatten(),
        enabled,
        config,
        variables,
    }))
}
#[derive(Clone, Copy)]
enum Reload<'a> {
    Instance(Option<&'a ProviderInstanceId>),
    Checked(&'a SecretReference, &'a str),
}
impl InstanceRegistry {
    fn secret_slots(
        &self,
        entry: &ProviderInstanceConfig,
        config: &Value,
    ) -> Vec<(String, SecretReference)> {
        let mut slots = entry
            .environment
            .iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, variable)| {
                variable
                    .secret
                    .as_ref()
                    .map(|reference| (format!("environment:{index}"), reference.clone()))
            })
            .collect::<Vec<_>>();
        for field in self
            .drivers
            .get(&entry.driver)
            .into_iter()
            .flat_map(|driver| &driver.secret_fields)
        {
            if let Some(value) = config.get(field)
                && let Ok(reference) = serde_json::from_value(value.clone())
            {
                slots.push((field.clone(), reference));
            }
        }
        slots
    }
    pub async fn reload_secrets(
        &self,
        settings: &ServerSettings,
        resolver: &SecretResolver,
        instance: Option<&ProviderInstanceId>,
        blocked: &BTreeMap<ProviderDriverKind, String>,
    ) -> ReconcileChanges {
        self.reload_where(settings, resolver, Reload::Instance(instance), blocked)
            .await
    }
    pub async fn apply_secret(
        &self,
        settings: &ServerSettings,
        resolver: &SecretResolver,
        reference: &SecretReference,
        value: &str,
        blocked: &BTreeMap<ProviderDriverKind, String>,
    ) -> ReconcileChanges {
        self.reload_where(
            settings,
            resolver,
            Reload::Checked(reference, value),
            blocked,
        )
        .await
    }
    async fn reload_where(
        &self,
        settings: &ServerSettings,
        resolver: &SecretResolver,
        selection: Reload<'_>,
        blocked: &BTreeMap<ProviderDriverKind, String>,
    ) -> ReconcileChanges {
        let _gate = self.gate.lock().await;
        let previous = self.state.read().await.entries.clone();
        let mut targets = vec![];
        for (id, entry) in derive_config_map(settings) {
            if let Reload::Instance(Some(selected)) = selection
                && selected != &id
            {
                continue;
            }
            let Some(driver) = self.drivers.get(&entry.driver) else {
                continue;
            };
            let Ok(config) = (driver.decode_config)(entry.config.as_ref()) else {
                continue;
            };
            if !resolve_enabled(&entry, &config) {
                continue;
            }
            let slots = self.secret_slots(&entry, &config);
            if slots.is_empty() {
                continue;
            }
            if let Reload::Checked(reference, _) = selection
                && !slots.iter().any(|(_, candidate)| candidate == reference)
            {
                continue;
            }
            targets.push((id, entry, config, slots));
        }
        if targets.is_empty() {
            return ReconcileChanges::default();
        }
        if matches!(selection, Reload::Instance(_)) {
            let references = targets
                .iter()
                .flat_map(|(_, _, _, slots)| slots.iter().map(|(_, reference)| reference.clone()))
                .collect::<Vec<_>>();
            resolver.invalidate(Some(&references)).await;
            resolver
                .read(&settings.secret_providers, &references, false)
                .await;
        }
        let mut carried = BTreeMap::new();
        let mut rebuild = BTreeSet::new();
        for (id, entry, config, slots) in targets {
            let live = previous
                .get(&id)
                .filter(|old| old.source == entry)
                .and_then(|old| match &old.status {
                    InstanceStatus::Prepared(spec) => Some(spec),
                    _ => None,
                });
            let values = if let (Reload::Checked(checked, value), Some(live)) = (selection, live) {
                slots
                    .iter()
                    .map(|(slot, reference)| {
                        if reference == checked {
                            Some(value.to_owned())
                        } else if let Some(index) = slot
                            .strip_prefix("environment:")
                            .and_then(|index| index.parse::<usize>().ok())
                        {
                            live.variables
                                .get(index)
                                .map(|variable| variable.value.clone())
                        } else {
                            live.config[slot].as_str().map(str::to_owned)
                        }
                    })
                    .collect::<Vec<_>>()
            } else {
                let references=slots.iter().filter(|(_,reference)|!matches!(selection,Reload::Checked(checked,_)if reference==checked)).map(|(_,reference)|reference.clone()).collect::<Vec<_>>();
                let mut read = resolver
                    .read(&settings.secret_providers, &references, false)
                    .await
                    .into_iter();
                slots
                    .iter()
                    .map(|(_, reference)| {
                        if let Reload::Checked(checked, value) = selection
                            && reference == checked
                        {
                            Some(value.to_owned())
                        } else {
                            read.next().flatten()
                        }
                    })
                    .collect()
            };
            let candidate =
                resolved_spec(&id, &entry, config, true, &slots, &mut values.into_iter());
            if let Some(live) = live {
                match &candidate {
                    InstanceStatus::Unavailable(_) => continue,
                    InstanceStatus::Prepared(next)
                        if next.config == live.config && next.variables == live.variables =>
                    {
                        continue;
                    }
                    _ => {}
                }
            }
            rebuild.insert(id.clone());
            carried.insert(id, candidate);
        }
        self.reconcile_locked(settings, resolver, &rebuild, false, blocked, &carried)
            .await
    }
}

pub fn core_driver_definitions() -> Vec<DriverDefinition> {
    fn decode<T: serde::de::DeserializeOwned + serde::Serialize>(
        raw: Option<&Value>,
    ) -> std::result::Result<Value, String> {
        serde_json::from_value::<T>(raw.cloned().unwrap_or_else(|| serde_json::json!({})))
            .and_then(serde_json::to_value)
            .map_err(|e| e.to_string())
    }
    let mut drivers = vec![
        DriverDefinition {
            kind: ProviderDriverKind("codex".into()),
            decode_config: decode::<t3_contracts::settings::CodexSettings>,
            secret_fields: vec![],
        },
        DriverDefinition {
            kind: ProviderDriverKind("claudeAgent".into()),
            decode_config: decode::<t3_contracts::settings::ClaudeSettings>,
            secret_fields: vec![],
        },
        DriverDefinition {
            kind: ProviderDriverKind("antigravity".into()),
            decode_config: decode::<t3_contracts::settings::AntigravitySettings>,
            secret_fields: vec!["apiKey".into()],
        },
    ];
    use crate::discovery::Driver;
    use crate::settings::decode_packaged;
    let packaged: &[(Driver, ConfigDecoder)] = &[
        (Driver::Cursor, |raw| {
            decode_packaged(Driver::Cursor, raw).map_err(|e| e.to_string())
        }),
        (Driver::Grok, |raw| {
            decode_packaged(Driver::Grok, raw).map_err(|e| e.to_string())
        }),
        (Driver::Fx, |raw| {
            decode_packaged(Driver::Fx, raw).map_err(|e| e.to_string())
        }),
        (Driver::OpenCode, |raw| {
            decode_packaged(Driver::OpenCode, raw).map_err(|e| e.to_string())
        }),
        (Driver::Kiro, |raw| {
            decode_packaged(Driver::Kiro, raw).map_err(|e| e.to_string())
        }),
        (Driver::Pi, |raw| {
            decode_packaged(Driver::Pi, raw).map_err(|e| e.to_string())
        }),
        (Driver::Muse, |raw| {
            decode_packaged(Driver::Muse, raw).map_err(|e| e.to_string())
        }),
        (Driver::AcpRegistry, |raw| {
            decode_packaged(Driver::AcpRegistry, raw).map_err(|e| e.to_string())
        }),
    ];
    drivers.extend(packaged.iter().map(|(driver, decode)| DriverDefinition {
        kind: ProviderDriverKind(driver.kind().into()),
        decode_config: *decode,
        secret_fields: if *driver == Driver::OpenCode {
            vec!["serverPassword".into()]
        } else {
            vec![]
        },
    }));
    drivers
}
