use crate::{
    Result,
    environment::{Environment, Platform},
    instances::{BUILT_IN_DRIVERS, stored_enabled},
    invalid,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    managed_policy::{ManagedProviderInstance, ManagedProviderLimits},
    provider_instance::*,
    provider_policy::RuntimeMode,
    settings::ServerSettings,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvironmentRule {
    All,
    Locked,
    Fill,
}
#[derive(Clone)]
struct Rule {
    pin: Option<ManagedProviderInstance>,
    config: Vec<String>,
    fill: Vec<String>,
    environment: EnvironmentRule,
}

pub struct ManagedProviders {
    allowed: Option<BTreeSet<ProviderDriverKind>>,
    rules: BTreeMap<ProviderDriverKind, Rule>,
    platform: Platform,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WriteRefusal {
    #[error("managed provider field is locked: {0}")]
    Locked(String),
    #[error("provider instance is disallowed: {0:?}")]
    Disallowed(ProviderInstanceId),
}

fn fields(driver: &str) -> &'static [&'static str] {
    match driver {
        "codex" => &["binaryPath", "launchArgs", "homePath", "shadowHomePath"],
        "claudeAgent" => &["binaryPath", "homePath", "launchArgs"],
        "antigravity" => &["binaryPath"],
        "cursor" => &["binaryPath"],
        "pi" => &["binaryPath", "launchArgs"],
        "opencode" => &["binaryPath", "serverUrl"],
        "acpRegistry" => &["commandPath", "commandArgs"],
        _ => &["binaryPath"],
    }
}

fn key(platform: Platform, name: &str) -> String {
    if platform == Platform::Windows {
        name.to_uppercase()
    } else {
        name.into()
    }
}
fn record(instance: Option<&ProviderInstanceConfig>) -> Map<String, Value> {
    instance
        .and_then(|i| i.config.as_ref())
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}
fn value(instance: Option<&ProviderInstanceConfig>, field: &str) -> Option<Value> {
    record(instance)
        .get(field)
        .filter(|v| v.as_str() != Some("") && !v.as_array().is_some_and(Vec::is_empty))
        .cloned()
}
fn pinned(pin: Option<&ManagedProviderInstance>, field: &str) -> Option<Value> {
    match field {
        "binaryPath" => pin?
            .binary_path
            .as_ref()
            .map(|s| Value::String(s.0.clone())),
        "launchArgs" => pin?
            .launch_args
            .as_ref()
            .map(|s| Value::String(s.0.clone())),
        _ => None,
    }
}
fn enabled(instance: &ProviderInstanceConfig) -> bool {
    stored_enabled(instance)
}
fn same_variable(
    a: &ProviderInstanceEnvironmentVariable,
    b: &ProviderInstanceEnvironmentVariable,
) -> bool {
    a.value == b.value && a.secret == b.secret && a.sensitive == b.sensitive
}

impl ManagedProviders {
    pub fn new(
        limits: &ManagedProviderLimits,
        ceiling: Option<RuntimeMode>,
        platform: Platform,
    ) -> Result<Self> {
        for driver in limits
            .allowed_drivers
            .iter()
            .flatten()
            .chain(limits.instances.iter().flat_map(|p| p.keys()))
        {
            if !BUILT_IN_DRIVERS.contains(&driver.0.as_str()) {
                return Err(invalid(
                    "managed providers",
                    format!("{} is not a known provider", driver.0),
                ));
            }
        }
        for (driver, pin) in limits.instances.iter().flatten() {
            for field in ["binaryPath", "launchArgs"] {
                if pinned(Some(pin), field).is_some() && !fields(&driver.0).contains(&field) {
                    return Err(invalid(
                        "managed providers",
                        format!("{} has no {field} setting", driver.0),
                    ));
                }
            }
            let names: Vec<_> = pin
                .environment
                .iter()
                .flatten()
                .map(|v| key(platform, &v.name))
                .collect();
            if names.iter().collect::<BTreeSet<_>>().len() != names.len() {
                return Err(invalid(
                    "managed providers",
                    format!("instances.{}.environment repeats a variable", driver.0),
                ));
            }
        }
        let mut rules = BTreeMap::new();
        for driver in BUILT_IN_DRIVERS {
            let driver = ProviderDriverKind((*driver).into());
            let pin = limits
                .instances
                .as_ref()
                .and_then(|p| p.get(&driver))
                .cloned();
            let pinned_fields: Vec<String> = ["binaryPath", "launchArgs"]
                .into_iter()
                .filter(|f| pinned(pin.as_ref(), f).is_some())
                .map(str::to_owned)
                .collect();
            let bypasses = if pin.is_none() {
                vec![]
            } else {
                match driver.0.as_str() {
                    "codex" => vec!["setupMode".into()],
                    "opencode" => vec!["serverUrl".into()],
                    _ => vec![],
                }
            };
            let rule = if ceiling.is_some() {
                Some(Rule {
                    pin,
                    config: fields(&driver.0)
                        .iter()
                        .map(|f| (*f).into())
                        .chain(bypasses)
                        .collect(),
                    fill: vec![],
                    environment: EnvironmentRule::All,
                })
            } else if let Some(pin) = pin {
                let home = match driver.0.as_str() {
                    "codex" => Some((
                        "CODEX_HOME",
                        vec!["homePath".into(), "shadowHomePath".into()],
                    )),
                    "claudeAgent" => Some(("CLAUDE_CONFIG_DIR", vec!["homePath".into()])),
                    _ => None,
                };
                let home_fields = home
                    .filter(|(name, _)| {
                        pin.environment
                            .iter()
                            .flatten()
                            .any(|v| key(platform, &v.name) == key(platform, name))
                    })
                    .map(|(_, f)| f)
                    .unwrap_or_default();
                let locked = pin.locked;
                Some(Rule {
                    pin: Some(pin),
                    config: if locked {
                        pinned_fields
                            .clone()
                            .into_iter()
                            .chain(bypasses)
                            .chain(home_fields)
                            .collect()
                    } else {
                        vec![]
                    },
                    fill: if locked { vec![] } else { pinned_fields },
                    environment: if locked {
                        EnvironmentRule::Locked
                    } else {
                        EnvironmentRule::Fill
                    },
                })
            } else {
                None
            };
            if let Some(rule) = rule {
                rules.insert(driver, rule);
            }
        }
        Ok(Self {
            allowed: limits
                .allowed_drivers
                .as_ref()
                .map(|a| a.iter().cloned().collect()),
            rules,
            platform,
        })
    }

    pub fn is_allowlisted(&self) -> bool {
        self.allowed.is_some()
    }
    pub fn is_driver_allowed(&self, driver: &ProviderDriverKind) -> bool {
        self.allowed.as_ref().is_none_or(|a| a.contains(driver))
    }
    pub fn locked_instance_fields(&self) -> BTreeMap<ProviderDriverKind, Vec<String>> {
        self.rules
            .iter()
            .filter_map(|(driver, rule)| {
                let mut fields: Vec<_> =
                    rule.config.iter().map(|f| format!("config.{f}")).collect();
                match rule.environment {
                    EnvironmentRule::All => fields.push("environment".into()),
                    EnvironmentRule::Locked => fields.extend(
                        rule.pin
                            .iter()
                            .flat_map(|p| p.environment.iter().flatten())
                            .map(|v| format!("environment.{}", v.name)),
                    ),
                    _ => {}
                }
                (!fields.is_empty()).then(|| (driver.clone(), fields))
            })
            .collect()
    }

    fn overlay(&self, instance: &ProviderInstanceConfig, rule: &Rule) -> ProviderInstanceConfig {
        let mut next = instance.clone();
        let mut config = record(Some(instance));
        for field in &rule.config {
            if let Some(value) = pinned(rule.pin.as_ref(), field) {
                config.insert(field.clone(), value);
            } else {
                config.remove(field);
            }
        }
        for field in &rule.fill {
            if value(Some(instance), field).is_none()
                && let Some(value) = pinned(rule.pin.as_ref(), field)
            {
                config.insert(field.clone(), value);
            }
        }
        next.config = if instance.config.is_none() && config.is_empty() {
            None
        } else {
            Some(Value::Object(config))
        };
        let pins: Vec<_> = rule
            .pin
            .iter()
            .flat_map(|p| p.environment.iter().flatten())
            .map(|v| ProviderInstanceEnvironmentVariable {
                name: v.name.clone(),
                value: v.value.clone(),
                sensitive: false,
                value_redacted: None,
                secret: None,
            })
            .collect();
        let mut environment = if rule.environment == EnvironmentRule::All {
            pins.clone()
        } else {
            vec![]
        };
        if rule.environment != EnvironmentRule::All {
            let mut seen = BTreeSet::new();
            for variable in instance.environment.iter().flatten() {
                let name = key(self.platform, &variable.name);
                if rule.config.iter().any(|f| f == "launchArgs")
                    && name == key(self.platform, "T3CODE_CODEX_LAUNCH_ARGS")
                {
                    continue;
                }
                let pin = if rule.environment == EnvironmentRule::Locked {
                    pins.iter().find(|p| key(self.platform, &p.name) == name)
                } else {
                    None
                };
                if let Some(pin) = pin {
                    if !seen.contains(&name) {
                        environment.push(pin.clone());
                    }
                } else {
                    environment.push(variable.clone());
                }
                seen.insert(name);
            }
            for pin in pins {
                if !seen.contains(&key(self.platform, &pin.name)) {
                    environment.push(pin);
                }
            }
        }
        next.environment = (!environment.is_empty()).then_some(environment);
        next
    }

    pub fn apply_limits(&self, settings: &ServerSettings) -> ServerSettings {
        let mut next = settings.clone();
        for (driver, rule) in &self.rules {
            if rule.pin.is_some() && driver.0 != "acpRegistry" {
                next.provider_instances
                    .entry(ProviderInstanceId(driver.0.clone()))
                    .or_insert_with(|| ProviderInstanceConfig {
                        driver: driver.clone(),
                        display_name: None,
                        accent_color: None,
                        environment: None,
                        enabled: None,
                        config: None,
                    });
            }
        }
        for instance in next.provider_instances.values_mut() {
            if let Some(rule) = self.rules.get(&instance.driver) {
                *instance = self.overlay(instance, rule);
            }
        }
        if self.allowed.is_some() {
            next.text_generators.clear();
        }
        let selection_allowed =
            |selection: &Option<t3_contracts::model_selection::ModelSelection>| {
                selection.as_ref().is_none_or(|s| {
                    self.is_driver_allowed(
                        &settings
                            .provider_instances
                            .get(&s.instance_id)
                            .map(|i| i.driver.clone())
                            .unwrap_or_else(|| ProviderDriverKind(s.instance_id.0.clone())),
                    )
                })
            };
        if !selection_allowed(&settings.source_control_writer_model_selection) {
            next.source_control_writer_model_selection = None;
        }
        if !selection_allowed(&settings.review_guide_model_selection) {
            next.review_guide_model_selection = None;
        }
        next
    }

    fn vet_environment(
        &self,
        id: &ProviderInstanceId,
        stored: Option<&ProviderInstanceConfig>,
        next: &ProviderInstanceConfig,
        rule: &Rule,
    ) -> std::result::Result<ProviderInstanceEnvironment, WriteRefusal> {
        let saved = stored
            .and_then(|i| i.environment.clone())
            .unwrap_or_default();
        let pins: Vec<_> = rule
            .pin
            .iter()
            .flat_map(|p| p.environment.iter().flatten())
            .collect();
        let echoed = |v: &ProviderInstanceEnvironmentVariable| {
            !v.sensitive
                && v.secret.is_none()
                && pins.iter().any(|p| p.name == v.name && p.value == v.value)
        };
        let sent = next.environment.clone().unwrap_or_default();
        if rule.environment == EnvironmentRule::All {
            if sent
                .iter()
                .any(|v| !echoed(v) && !saved.iter().any(|s| same_variable(v, s)))
            {
                return Err(WriteRefusal::Locked(format!(
                    "providerInstances.{}.environment",
                    id.0
                )));
            }
            return Ok(saved);
        }
        let mut environment = sent;
        for pin in &pins {
            let matches = |v: &ProviderInstanceEnvironmentVariable| {
                key(self.platform, &v.name) == key(self.platform, &pin.name)
            };
            let sent = environment.iter().find(|v| matches(v));
            let saved_first = saved.iter().find(|v| matches(v));
            let unchanged = match (sent, saved_first) {
                (None, None) => true,
                (Some(a), Some(b)) => same_variable(a, b),
                _ => false,
            };
            if sent.is_some_and(echoed) || unchanged {
                let mut restored = false;
                environment = environment
                    .into_iter()
                    .flat_map(|v| {
                        if !matches(&v) {
                            return vec![v];
                        }
                        if restored {
                            return vec![];
                        }
                        restored = true;
                        saved.iter().filter(|s| matches(s)).cloned().collect()
                    })
                    .collect();
            } else if rule.environment == EnvironmentRule::Locked {
                return Err(WriteRefusal::Locked(format!(
                    "providerInstances.{}.environment.{}",
                    id.0, pin.name
                )));
            }
        }
        if rule.config.iter().any(|f| f == "launchArgs") {
            let is_override = |v: &ProviderInstanceEnvironmentVariable| {
                key(self.platform, &v.name) == key(self.platform, "T3CODE_CODEX_LAUNCH_ARGS")
            };
            environment.retain(|v| !is_override(v));
            environment.extend(saved.into_iter().filter(is_override));
        }
        Ok(environment)
    }

    pub fn vet_write(
        &self,
        current: &ServerSettings,
        next: &ServerSettings,
    ) -> std::result::Result<ServerSettings, WriteRefusal> {
        let mut result = next.clone();
        result.provider_instances.clear();
        for (id, instance) in &next.provider_instances {
            let stored = current.provider_instances.get(id);
            let was_enabled = stored.map_or(id.0 == instance.driver.0, |s| {
                s.driver == instance.driver && enabled(s)
            });
            if !self.is_driver_allowed(&instance.driver) && enabled(instance) && !was_enabled {
                return Err(WriteRefusal::Disallowed(id.clone()));
            }
            let Some(rule) = self.rules.get(&instance.driver) else {
                result
                    .provider_instances
                    .insert(id.clone(), instance.clone());
                continue;
            };
            let stored = stored.filter(|s| s.driver == instance.driver);
            let mut config = record(Some(instance));
            for field in rule.config.iter().chain(&rule.fill) {
                let sent = value(Some(instance), field);
                let saved = value(stored, field);
                let harmless = match (instance.driver.0.as_str(), field.as_str()) {
                    ("codex", "setupMode") => Some(Value::String("existing".into())),
                    ("opencode", "serverUrl") => Some(Value::String("".into())),
                    _ => None,
                };
                if sent == pinned(rule.pin.as_ref(), field)
                    || sent == saved
                    || harmless.as_ref().is_some_and(|h| sent.as_ref() == Some(h))
                {
                    if let Some(saved) = saved {
                        config.insert(field.clone(), saved);
                    } else {
                        config.remove(field);
                    }
                } else if rule.config.contains(field) {
                    return Err(WriteRefusal::Locked(format!(
                        "providerInstances.{}.config.{field}",
                        id.0
                    )));
                }
            }
            let environment = self.vet_environment(id, stored, instance, rule)?;
            let mut kept = instance.clone();
            kept.environment = (!environment.is_empty()).then_some(environment);
            kept.config = if config.is_empty() && stored.is_none_or(|s| s.config.is_none()) {
                None
            } else {
                Some(Value::Object(config))
            };
            if stored.is_none()
                && id.0 == instance.driver.0
                && kept.config.is_none()
                && kept.environment.is_none()
                && kept.enabled.is_none()
                && kept.display_name.is_none()
                && kept.accent_color.is_none()
            {
                continue;
            }
            result.provider_instances.insert(id.clone(), kept);
        }
        for (project, overrides) in &next.project_settings_overrides {
            for (id, override_) in overrides.provider_instances.iter().flatten() {
                let before = current
                    .project_settings_overrides
                    .get(project)
                    .and_then(|p| p.provider_instances.as_ref())
                    .and_then(|p| p.get(id))
                    .and_then(|p| p.enabled);
                let driver = next
                    .provider_instances
                    .get(id)
                    .map(|i| i.driver.clone())
                    .unwrap_or_else(|| ProviderDriverKind(id.0.clone()));
                if override_.enabled == Some(true)
                    && before != Some(true)
                    && !self.is_driver_allowed(&driver)
                {
                    return Err(WriteRefusal::Disallowed(id.clone()));
                }
            }
        }
        Ok(result)
    }
}

pub fn provider_host_environment(managed: bool, env: &Environment) -> Environment {
    if !managed {
        return env.clone();
    }
    env.iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("T3CODE_CODEX_LAUNCH_ARGS"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub fn kiro_autopilot(
    mode: RuntimeMode,
    approval_override: bool,
    sandbox_override: bool,
) -> &'static str {
    if mode == RuntimeMode::FullAccess && !approval_override && !sandbox_override {
        "on"
    } else {
        "off"
    }
}
pub fn antigravity_mode(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::FullAccess => "yolo",
        RuntimeMode::AutoAcceptEdits => "auto_edit",
        _ => "default",
    }
}
pub fn antigravity_cli_mode(mode: &str) -> &str {
    if matches!(mode, "yolo" | "auto_edit") {
        "default"
    } else {
        mode
    }
}
