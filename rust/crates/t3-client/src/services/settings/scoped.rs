//! Port of apps/web/src/components/settings/scopedSettings.ts and the override half of
//! packages/shared/src/projectSettings.ts: which value a settings row shows for the selected
//! scope, and which writes a change becomes.
//!
//! Patches are JSON objects keyed by the wire names (`ServerSettingsPatch & ClientSettingsPatch`),
//! as TypeScript's are, and are split by the keys `ServerSettings` and `ClientSettingsSchema`
//! serialise.
use super::scope::{ResolvedSettingsScope, ScopeEnvironment, ScopeKind};
use crate::{connection::Phase, types::EnvironmentId};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, rc::Rc, sync::OnceLock};
use t3_contracts::{
    base_schemas::ProjectId,
    settings::{ClientSettingsSchema, ServerSettings},
};

pub type SettingsPatch = Map<String, Value>;

/// `PROJECT_SCOPED_SERVER_SETTING_KEYS`.
pub const PROJECT_SCOPED_SERVER_SETTING_KEYS: [&str; 27] = [
    "worktreeCleanup",
    "defaultModelSelection",
    "defaultRuntimeMode",
    "defaultThreadEnvMode",
    "newWorktreesStartFromOrigin",
    "worktreeSubmodules",
    "defaultAutoPull",
    "defaultProjectScripts",
    "enableAgentBrowserAccess",
    "enableAgentDeviceAccess",
    "textGenerationModelSelection",
    "threadTitleInstructions",
    "sourceControlWriterModelSelection",
    "reviewGuideModelSelection",
    "reviewGuideInstructions",
    "sourceControlWritingStyle",
    "removeAgentCreditsOnMerge",
    "branchNamingMode",
    "branchNamePrefix",
    "branchNameInstructions",
    "pullRequestMergeMethod",
    "sidebarAutoSettleOnMerge",
    "sidebarAutoSettleAfterDays",
    "sidebarAutoSettleKeepPullRequests",
    "continueThreadsAfterServerUpdate",
    "responseStreamingMode",
    "speechProjectCustomWords",
];

/// `NULLABLE_PROJECT_SETTINGS_OVERRIDES`: null stores an override; otherwise null removes it.
const NULLABLE_PROJECT_SETTINGS_OVERRIDES: [&str; 5] = [
    "defaultModelSelection",
    "sourceControlWriterModelSelection",
    "reviewGuideModelSelection",
    "pullRequestMergeMethod",
    "sidebarAutoSettleAfterDays",
];

pub fn is_project_scoped_setting_key(key: &str) -> bool {
    PROJECT_SCOPED_SERVER_SETTING_KEYS.contains(&key)
}

fn keys_of(value: Value) -> Vec<String> {
    value
        .as_object()
        .map(|fields| fields.keys().cloned().collect())
        .unwrap_or_default()
}

/// `Struct.keys(ServerSettings.fields)`.
pub fn server_setting_keys() -> &'static [String] {
    static KEYS: OnceLock<Vec<String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        keys_of(serde_json::to_value(ServerSettings::default()).unwrap_or_default())
    })
}

/// `Object.keys(ClientSettingsSchema.fields)`.
pub fn client_setting_keys() -> &'static [String] {
    static KEYS: OnceLock<Vec<String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        keys_of(serde_json::to_value(ClientSettingsSchema::default()).unwrap_or_default())
    })
}

pub fn is_server_setting_key(key: &str) -> bool {
    server_setting_keys().iter().any(|k| k == key)
}

pub fn is_client_setting_key(key: &str) -> bool {
    client_setting_keys().iter().any(|k| k == key)
}

/// `ProjectSettingSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProjectSettingSource {
    Environment,
    Project,
    /// The repository's `t3.json`. Not produced yet: the project file tier is not read.
    ProjectFile,
}

/// `ScopedSettingSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopedSettingSource {
    Environment,
    Project,
    ProjectFile,
    Mixed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProjectSettings {
    pub settings: Rc<ServerSettings>,
    /// The effective settings as wire JSON, for key-wise comparisons.
    pub json: Rc<Value>,
    pub sources: BTreeMap<&'static str, ProjectSettingSource>,
}

fn provider_instance_enabled(
    settings: &Value,
    project: Option<&ProjectId>,
    instance: &str,
) -> bool {
    if let Some(project) = project
        && let Some(enabled) = settings["projectSettingsOverrides"][&project.0]["providerInstances"]
            [instance]["enabled"]
            .as_bool()
    {
        return enabled;
    }
    match settings["providerInstances"].get(instance) {
        Some(config) => {
            let config_enabled = config["config"]
                .as_object()
                .and_then(|config| config.get("enabled"))
                .and_then(Value::as_bool);
            if config["enabled"].as_bool() == Some(false) || config_enabled == Some(false) {
                return false;
            }
            config["enabled"]
                .as_bool()
                .or(config_enabled)
                .unwrap_or(!matches!(
                    config["driver"].as_str().unwrap_or_default(),
                    "cursor" | "grok" | "fx" | "kiro" | "muse" | "pi" | "opencode" | "antigravity"
                ))
        }
        None => matches!(instance, "codex" | "claudeAgent"),
    }
}

/// `resolveTextGenerationSelectionEnabled`.
fn text_generation_enabled(settings: &Value, project: Option<&ProjectId>, instance: &str) -> bool {
    settings["textGenerators"]
        .get(instance)
        .and_then(|generator| generator["enabled"].as_bool())
        .unwrap_or_else(|| provider_instance_enabled(settings, project, instance))
}

/// `readCustomModelEntries(config.customModels)` slugs.
fn custom_model_slugs(config: &Value) -> Vec<&str> {
    config["customModels"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().or_else(|| entry["slug"].as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// `resolveProjectSettings(settings, projectId)`: project overrides over the environment's
/// values. The `t3.json` tier and the disabled-text-generator fallback are not ported (see the
/// settings hand-off); a disabled override is skipped as TypeScript skips it.
pub fn resolve_project_settings(
    settings: &Rc<ServerSettings>,
    project: Option<&ProjectId>,
) -> ResolvedProjectSettings {
    let mut json = serde_json::to_value(settings.as_ref()).unwrap_or_default();
    let sources: BTreeMap<&'static str, ProjectSettingSource> = PROJECT_SCOPED_SERVER_SETTING_KEYS
        .iter()
        .map(|key| (*key, ProjectSettingSource::Environment))
        .collect();
    let overrides = project
        .and_then(|project| json["projectSettingsOverrides"].get(&project.0))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if overrides.is_empty() {
        return ResolvedProjectSettings {
            settings: settings.clone(),
            json: Rc::new(json),
            sources,
        };
    }
    let mut sources = sources;
    let original = json.clone();
    for key in PROJECT_SCOPED_SERVER_SETTING_KEYS {
        let Some(value) = overrides.get(key) else {
            continue;
        };
        if matches!(
            key,
            "textGenerationModelSelection" | "defaultModelSelection"
        ) && let Some(instance) = value["instanceId"].as_str()
        {
            let enabled = if key == "textGenerationModelSelection" {
                text_generation_enabled(&original, project, instance)
            } else {
                provider_instance_enabled(&original, project, instance)
            };
            if !enabled {
                continue;
            }
        }
        if key == "defaultModelSelection"
            && let (Some(instance), Some(model)) =
                (value["instanceId"].as_str(), value["model"].as_str())
        {
            let hidden = original["providerModelPreferences"][instance]["hiddenModels"]
                .as_array()
                .is_some_and(|models| models.iter().any(|m| m.as_str() == Some(model)));
            let custom = custom_model_slugs(&original["providerInstances"][instance]["config"])
                .contains(&model);
            if hidden && !custom {
                continue;
            }
        }
        json[key] = value.clone();
        sources.insert(key, ProjectSettingSource::Project);
    }
    let effective = serde_json::from_value::<ServerSettings>(json.clone())
        .map(Rc::new)
        .unwrap_or_else(|_| settings.clone());
    ResolvedProjectSettings {
        settings: effective,
        json: Rc::new(json),
        sources,
    }
}

/// `clearProjectSettingsOverrides`: the override entry without `keys`, or null when empty.
pub fn clear_project_settings_overrides(
    settings: &Value,
    project: &ProjectId,
    keys: &[&str],
) -> Value {
    let Some(current) = settings["projectSettingsOverrides"]
        .get(&project.0)
        .and_then(Value::as_object)
    else {
        return Value::Null;
    };
    let mut next = current.clone();
    for key in keys {
        next.remove(*key);
    }
    if next.is_empty() {
        Value::Null
    } else {
        Value::Object(next)
    }
}

/// `resolveWorktreeCleanup` over wire JSON.
fn resolve_worktree_cleanup(settings: &Value, effective: &Value) -> Value {
    let storage = &settings["storageCleanup"];
    let policy = &effective["worktreeCleanup"];
    match policy["mode"].as_str() {
        Some("custom") => policy["rules"].clone(),
        Some("off") => json!({
            "worktreeKeepWhen": storage["worktreeKeepWhen"],
            "worktreeAfterDays": null,
            "worktreeSettledAfterDays": null,
            "worktreeOnMerge": false,
            "worktreeOnDelete": false,
            "worktreeUnchanged": false,
        }),
        _ => json!({
            "worktreeKeepWhen": storage["worktreeKeepWhen"],
            "worktreeAfterDays": storage["worktreeAfterDays"],
            "worktreeSettledAfterDays": storage["worktreeSettledAfterDays"],
            "worktreeOnMerge": storage["worktreeOnMerge"],
            "worktreeOnDelete": storage["worktreeOnDelete"],
            "worktreeUnchanged": storage["worktreeUnchanged"],
        }),
    }
}

pub fn is_connected_with_config(environment: &ScopeEnvironment) -> bool {
    environment.phase == Phase::Connected && environment.config.is_some()
}

/// `selectScopedSettingsEnvironments`: the representative supplies display values, never the
/// set of write targets.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedEnvironments {
    pub environments: Vec<ScopeEnvironment>,
    pub connected: Vec<ScopeEnvironment>,
    pub representative: Option<ScopeEnvironment>,
}

pub fn select_scoped_settings_environments(
    scope: &ResolvedSettingsScope,
    available: &[ScopeEnvironment],
    primary: Option<&EnvironmentId>,
) -> ScopedEnvironments {
    let environments: Vec<_> = available
        .iter()
        .filter(|environment| scope.environment_ids.contains(&environment.environment_id))
        .cloned()
        .collect();
    let connected: Vec<_> = environments
        .iter()
        .filter(|environment| is_connected_with_config(environment))
        .cloned()
        .collect();
    let representative = connected
        .iter()
        .find(|candidate| Some(&candidate.environment_id) == primary)
        .or_else(|| connected.first())
        .cloned();
    ScopedEnvironments {
        environments,
        connected,
        representative,
    }
}

/// `ScopedSettingsTarget`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedSettingsTarget {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub project_id: Option<ProjectId>,
    pub settings: Rc<ServerSettings>,
    pub json: Rc<Value>,
    pub sources: BTreeMap<&'static str, ProjectSettingSource>,
}

/// `resolveScopedSettingsTargets` (without a project file tier).
pub fn resolve_scoped_settings_targets(
    scope: &ResolvedSettingsScope,
    connected: &[ScopeEnvironment],
) -> Vec<ScopedSettingsTarget> {
    let settings_of = |environment: &ScopeEnvironment| {
        environment
            .config
            .as_ref()
            .map(|config| Rc::new(config.settings.clone()))
    };
    if scope.is_project_scope() {
        return scope
            .members
            .iter()
            .filter_map(|member| {
                let environment = connected
                    .iter()
                    .find(|e| e.environment_id == member.environment_id)?;
                let resolved =
                    resolve_project_settings(&settings_of(environment)?, Some(&member.id));
                Some(ScopedSettingsTarget {
                    environment_id: member.environment_id.clone(),
                    label: environment.label.clone(),
                    project_id: Some(member.id.clone()),
                    settings: resolved.settings,
                    json: resolved.json,
                    sources: resolved.sources,
                })
            })
            .collect();
    }
    connected
        .iter()
        .filter_map(|environment| {
            let resolved = resolve_project_settings(&settings_of(environment)?, None);
            Some(ScopedSettingsTarget {
                environment_id: environment.environment_id.clone(),
                label: environment.label.clone(),
                project_id: None,
                settings: resolved.settings,
                json: resolved.json,
                sources: resolved.sources,
            })
        })
        .collect()
}

/// `scopedSettingsAreMixed`.
pub fn scoped_settings_are_mixed(targets: &[ScopedSettingsTarget], keys: &[&str]) -> bool {
    let Some(first) = targets.first() else {
        return false;
    };
    targets.iter().any(|candidate| {
        keys.iter()
            .any(|key| first.json[*key] != candidate.json[*key])
    })
}

/// `scopedSettingsSource`.
pub fn scoped_settings_source(
    targets: &[ScopedSettingsTarget],
    keys: &[&str],
) -> ScopedSettingSource {
    let scoped: Vec<_> = keys
        .iter()
        .filter(|key| is_project_scoped_setting_key(key))
        .collect();
    if scoped.is_empty() || targets.is_empty() {
        return ScopedSettingSource::Environment;
    }
    let mut sources: Vec<ProjectSettingSource> = targets
        .iter()
        .flat_map(|target| {
            scoped.iter().map(|key| {
                target
                    .sources
                    .get(**key)
                    .copied()
                    .unwrap_or(ProjectSettingSource::Environment)
            })
        })
        .collect();
    sources.sort();
    sources.dedup();
    if sources.len() > 1 {
        ScopedSettingSource::Mixed
    } else if sources.contains(&ProjectSettingSource::Project) {
        ScopedSettingSource::Project
    } else if sources.contains(&ProjectSettingSource::ProjectFile) {
        ScopedSettingSource::ProjectFile
    } else {
        ScopedSettingSource::Environment
    }
}

/// An English client sentence with named arguments (`{label}`), translated by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsText {
    pub key: &'static str,
    pub args: Vec<(&'static str, String)>,
}

impl SettingsText {
    pub fn key(key: &'static str) -> Self {
        Self {
            key,
            args: Vec::new(),
        }
    }
    pub fn with(key: &'static str, name: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            args: vec![(name, value.into())],
        }
    }
    /// The English text, for logs and tests.
    pub fn english(&self) -> String {
        self.args
            .iter()
            .fold(self.key.to_owned(), |text, (name, value)| {
                text.replace(&format!("{{{name}}}"), value)
            })
    }
}

/// `ScopedServerWrite`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedServerWrite {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub patch: SettingsPatch,
    pub previous: Rc<Value>,
}

/// The plan `planScopedSettingsPatch` and its clear variants return.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedSettingsPlan {
    pub client_patch: SettingsPatch,
    pub server_writes: Vec<ScopedServerWrite>,
    pub unavailable_reason: Option<SettingsText>,
}

impl ScopedSettingsPlan {
    pub fn has_client_write(&self) -> bool {
        !self.client_patch.is_empty()
    }
}

fn environment_json(environment: &ScopeEnvironment) -> Option<Rc<Value>> {
    environment
        .config
        .as_ref()
        .map(|config| Rc::new(serde_json::to_value(&config.settings).unwrap_or_default()))
}

fn project_override_writes(
    scope: &ResolvedSettingsScope,
    environments: &[ScopeEnvironment],
    update: impl Fn(&Map<String, Value>, &Value, &ProjectId, &Rc<ServerSettings>) -> Value,
) -> Vec<ScopedServerWrite> {
    let mut writes: Vec<ScopedServerWrite> = Vec::new();
    for member in &scope.members {
        let Some(environment) = environments
            .iter()
            .find(|e| e.environment_id == member.environment_id)
            .filter(|e| e.phase == Phase::Connected)
        else {
            continue;
        };
        let (Some(config), Some(json)) = (&environment.config, environment_json(environment))
        else {
            continue;
        };
        let current = json["projectSettingsOverrides"]
            .get(&member.id.0)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let entry = update(
            &current,
            &json,
            &member.id,
            &Rc::new(config.settings.clone()),
        );
        match writes
            .iter_mut()
            .find(|write| write.environment_id == member.environment_id)
        {
            Some(write) => {
                if let Some(overrides) = write
                    .patch
                    .get_mut("projectSettingsOverrides")
                    .and_then(Value::as_object_mut)
                {
                    overrides.insert(member.id.0.clone(), entry);
                }
            }
            None => {
                let mut patch = Map::new();
                patch.insert(
                    "projectSettingsOverrides".into(),
                    json!({ member.id.0.clone(): entry }),
                );
                writes.push(ScopedServerWrite {
                    environment_id: member.environment_id.clone(),
                    label: environment.label.clone(),
                    patch,
                    previous: json,
                });
            }
        }
    }
    writes
}

/// `forDestination`: puts an idle compaction mode in the form each destination server reads.
fn for_destination(previous: &Value, mut patch: SettingsPatch) -> SettingsPatch {
    let Some(mode) = patch.remove("idleCompaction") else {
        return patch;
    };
    patch.remove("compactIdleThreads");
    let has_modes = previous.get("idleCompaction").is_some_and(|v| !v.is_null());
    if has_modes {
        patch.insert("idleCompaction".into(), mode);
    } else if previous["compactIdleThreads"].is_string() {
        patch.insert("compactIdleThreads".into(), mode);
    } else {
        patch.insert(
            "compactIdleThreads".into(),
            Value::Bool(mode.as_str() == Some("beforeCacheExpires")),
        );
    }
    patch
}

const MODEL_SELECTION_KEYS: [&str; 4] = [
    "defaultModelSelection",
    "sourceControlWriterModelSelection",
    "reviewGuideModelSelection",
    "textGenerationModelSelection",
];

/// `planScopedSettingsPatch`.
pub fn plan_scoped_settings_patch(
    scope: &ResolvedSettingsScope,
    environments: &[ScopeEnvironment],
    patch: &SettingsPatch,
    device_only: bool,
) -> ScopedSettingsPlan {
    let is_project_scope = scope.is_project_scope();
    let refuses_client_keys = is_project_scope && !device_only;
    let client_keys: Vec<&String> = patch
        .keys()
        .filter(|key| is_client_setting_key(key) && !is_server_setting_key(key))
        .collect();
    let server_patch: SettingsPatch = patch
        .iter()
        .filter(|(key, _)| is_server_setting_key(key))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let connected = select_scoped_settings_environments(scope, environments, None).connected;
    let unscopable: Vec<&String> = if is_project_scope {
        client_keys
            .iter()
            .copied()
            .filter(|_| refuses_client_keys)
            .chain(
                server_patch
                    .keys()
                    .filter(|key| !is_project_scoped_setting_key(key)),
            )
            .collect()
    } else {
        Vec::new()
    };
    let client_patch: SettingsPatch = if refuses_client_keys {
        Map::new()
    } else {
        patch
            .iter()
            .filter(|(key, _)| client_keys.contains(key))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    let server_writes = if server_patch.is_empty() {
        Vec::new()
    } else if is_project_scope {
        if !unscopable.is_empty() {
            Vec::new()
        } else {
            project_override_writes(scope, environments, |current, settings, project, typed| {
                // Override entries store whole objects; partial patches are completed from each
                // target's effective value.
                let effective = resolve_project_settings(typed, Some(project)).json;
                let mut next = current.clone();
                for (key, value) in &server_patch {
                    if key == "worktreeCleanup" && value["mode"] == "custom" {
                        let mut rules = resolve_worktree_cleanup(settings, &effective);
                        if let (Some(rules), Some(patch_rules)) =
                            (rules.as_object_mut(), value["rules"].as_object())
                        {
                            rules.extend(patch_rules.clone());
                        }
                        next.insert(key.clone(), json!({"mode": "custom", "rules": rules}));
                        continue;
                    }
                    if value.is_null()
                        && !NULLABLE_PROJECT_SETTINGS_OVERRIDES.contains(&key.as_str())
                    {
                        next.remove(key);
                        continue;
                    }
                    let base = &effective[key.as_str()];
                    if let (Some(value), Some(base)) = (value.as_object(), base.as_object()) {
                        // A new model must not inherit the previous model's options.
                        let mut merged = if MODEL_SELECTION_KEYS.contains(&key.as_str())
                            && (value.contains_key("instanceId") || value.contains_key("model"))
                        {
                            let mut seed = Map::new();
                            for field in ["instanceId", "model"] {
                                if let Some(v) = base.get(field) {
                                    seed.insert(field.into(), v.clone());
                                }
                            }
                            seed
                        } else {
                            base.clone()
                        };
                        merged.extend(value.clone());
                        next.insert(key.clone(), Value::Object(merged));
                        continue;
                    }
                    next.insert(key.clone(), value.clone());
                }
                Value::Object(next)
            })
        }
    } else if matches!(scope.kind, ScopeKind::All | ScopeKind::Environment { .. }) {
        connected
            .iter()
            .filter_map(|environment| {
                let previous = environment_json(environment)?;
                let mut target = server_patch.clone();
                let worktree_storage = server_patch
                    .get("storageCleanup")
                    .and_then(Value::as_object)
                    .map(|storage| {
                        storage
                            .iter()
                            .filter(|(key, _)| key.starts_with("worktree"))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<Map<_, _>>()
                    })
                    .filter(|storage| !storage.is_empty());
                if !previous["worktreeCleanup"].is_null()
                    && let Some(storage) = worktree_storage
                {
                    let mut rules = resolve_worktree_cleanup(&previous, &previous);
                    if let Some(rules) = rules.as_object_mut() {
                        rules.extend(storage);
                    }
                    target.insert(
                        "worktreeCleanup".into(),
                        json!({"mode": "custom", "rules": rules}),
                    );
                }
                Some(ScopedServerWrite {
                    environment_id: environment.environment_id.clone(),
                    label: environment.label.clone(),
                    patch: for_destination(&previous, target),
                    previous,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let has_write = !client_patch.is_empty() || !server_writes.is_empty();
    let unavailable_reason = if has_write || patch.is_empty() {
        None
    } else if let Some(message) = scope.unavailable_message() {
        Some(SettingsText::key(message))
    } else if !unscopable.is_empty() {
        Some(SettingsText::key(
            "This setting is environment-wide and cannot be overridden by a project.",
        ))
    } else if is_project_scope {
        Some(SettingsText::key(
            "Connect the selected checkouts, or update their environments, to save a project override.",
        ))
    } else if matches!(scope.kind, ScopeKind::Environment { .. }) {
        Some(SettingsText::with(
            "Connect {label} to save this setting.",
            "label",
            scope.label.english(),
        ))
    } else {
        Some(SettingsText::key(
            "Connect an environment to save this setting.",
        ))
    };
    ScopedSettingsPlan {
        client_patch,
        server_writes,
        unavailable_reason,
    }
}

/// `planScopedSettingsClear`.
pub fn plan_scoped_settings_clear(
    scope: &ResolvedSettingsScope,
    environments: &[ScopeEnvironment],
    keys: &[&str],
) -> ScopedSettingsPlan {
    let server_writes = if scope.is_project_scope() {
        project_override_writes(scope, environments, |_, settings, project, _| {
            clear_project_settings_overrides(settings, project, keys)
        })
    } else {
        Vec::new()
    };
    ScopedSettingsPlan {
        client_patch: Map::new(),
        unavailable_reason: server_writes.is_empty().then(|| {
            SettingsText::key(
                "Connect the selected checkouts, or update their environments, to reset this override.",
            )
        }),
        server_writes,
    }
}

/// `ProjectOverrideEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOverrideEntry {
    pub environment_id: EnvironmentId,
    pub project_id: ProjectId,
}

/// `listProjectOverrides`.
pub fn list_project_overrides(
    environments: &[ScopeEnvironment],
    keys: &[&str],
) -> Vec<ProjectOverrideEntry> {
    let scoped: Vec<_> = keys
        .iter()
        .filter(|key| is_project_scoped_setting_key(key))
        .collect();
    if scoped.is_empty() {
        return Vec::new();
    }
    environments
        .iter()
        .filter_map(|environment| Some((environment, environment_json(environment)?)))
        .flat_map(|(environment, json)| {
            json["projectSettingsOverrides"]
                .as_object()
                .map(|overrides| {
                    overrides
                        .iter()
                        .filter(|(_, entry)| scoped.iter().any(|key| entry.get(**key).is_some()))
                        .map(|(project, _)| ProjectOverrideEntry {
                            environment_id: environment.environment_id.clone(),
                            project_id: ProjectId(project.clone()),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect()
}

/// `planProjectOverridesClear`.
pub fn plan_project_overrides_clear(
    environments: &[ScopeEnvironment],
    entries: &[ProjectOverrideEntry],
    keys: &[&str],
) -> ScopedSettingsPlan {
    let mut writes: Vec<ScopedServerWrite> = Vec::new();
    for entry in entries {
        let Some(environment) = environments
            .iter()
            .find(|e| e.environment_id == entry.environment_id)
            .filter(|e| e.phase == Phase::Connected)
        else {
            continue;
        };
        let Some(json) = environment_json(environment) else {
            continue;
        };
        let cleared = clear_project_settings_overrides(&json, &entry.project_id, keys);
        match writes
            .iter_mut()
            .find(|write| write.environment_id == entry.environment_id)
        {
            Some(write) => {
                if let Some(overrides) = write
                    .patch
                    .get_mut("projectSettingsOverrides")
                    .and_then(Value::as_object_mut)
                {
                    overrides.insert(entry.project_id.0.clone(), cleared);
                }
            }
            None => {
                let mut patch = Map::new();
                patch.insert(
                    "projectSettingsOverrides".into(),
                    json!({ entry.project_id.0.clone(): cleared }),
                );
                writes.push(ScopedServerWrite {
                    environment_id: entry.environment_id.clone(),
                    label: environment.label.clone(),
                    patch,
                    previous: json,
                });
            }
        }
    }
    ScopedSettingsPlan {
        client_patch: Map::new(),
        unavailable_reason: writes
            .is_empty()
            .then(|| SettingsText::key("Connect the environments to reset these overrides.")),
        server_writes: writes,
    }
}

fn write_was_dropped(value: &Value, before: &Value, saved: &Value) -> bool {
    if value.is_null() {
        return !before.is_null() && saved == before;
    }
    if let Some(fields) = value.as_object()
        && !before.is_array()
        && !saved.is_array()
    {
        return fields.iter().any(|(key, next)| {
            write_was_dropped(
                next,
                before.get(key).unwrap_or(&Value::Null),
                saved.get(key).unwrap_or(&Value::Null),
            )
        });
    }
    let unchanged = match (value.as_str(), before.as_str()) {
        (Some(value), Some(before)) => value.trim() == before,
        _ => value == before,
    };
    !unchanged && saved == before
}

/// `droppedSettingKeys`: older servers silently drop unknown keys.
pub fn dropped_setting_keys(write: &ScopedServerWrite, saved: &Value) -> Vec<String> {
    write
        .patch
        .iter()
        .filter(|(key, value)| {
            write_was_dropped(
                value,
                write.previous.get(key.as_str()).unwrap_or(&Value::Null),
                saved.get(key.as_str()).unwrap_or(&Value::Null),
            )
        })
        .map(|(key, _)| key.clone())
        .collect()
}

/// `requiredScopesForServerSettingsPatch` (packages/contracts/src/settings.ts).
pub fn required_scopes_for_server_settings_patch(
    patch: &SettingsPatch,
) -> Vec<t3_contracts::auth::AuthEnvironmentScope> {
    use t3_contracts::auth::AuthEnvironmentScope as Scope;
    let (mut providers, mut settings, mut access) = (false, false, false);
    for key in patch.keys() {
        match key.as_str() {
            "requireConnectDeviceApproval" => access = true,
            "providers" | "providerInstances" | "usageLimitSources" | "textGenerators"
            | "secretProviders" => providers = true,
            _ => settings = true,
        }
    }
    // Removing a secret provider also clears the settings fields that read from it.
    if patch
        .get("secretProviders")
        .and_then(Value::as_object)
        .is_some_and(|entries| entries.values().any(Value::is_null))
    {
        settings = true;
    }
    let mut scopes = Vec::new();
    if settings || (!providers && !access) {
        scopes.push(Scope::SettingsWrite);
    }
    if providers {
        scopes.push(Scope::ProvidersManage);
    }
    if access {
        scopes.push(Scope::AccessWrite);
    }
    scopes
}

#[cfg(test)]
mod tests {
    use super::super::scope::{
        ScopeGroup, ScopeLabel, ScopeMember, SettingsScopeSearch, resolve_settings_scope,
    };
    use super::*;
    use t3_contracts::server::ServerConfig;

    fn config(environment: &str, settings: Value) -> Rc<ServerConfig> {
        let mut config: Value = serde_json::from_str(
            include_str!("../../../../../fixtures/samples/ServerConfig.jsonl")
                .lines()
                .next()
                .unwrap_or_default(),
        )
        .expect("config sample");
        config["environment"]["environmentId"] = json!(environment);
        let mut base = serde_json::to_value(ServerSettings::default()).unwrap_or_default();
        if let (Some(base), Some(extra)) = (base.as_object_mut(), settings.as_object()) {
            base.extend(extra.clone());
        }
        config["settings"] = base;
        Rc::new(serde_json::from_value(config).expect("server config"))
    }
    fn environment(id: &str, label: &str, settings: Value) -> ScopeEnvironment {
        ScopeEnvironment {
            environment_id: EnvironmentId(id.into()),
            label: label.into(),
            display_url: None,
            phase: Phase::Connected,
            config: Some(config(id, settings)),
        }
    }
    fn member(project: &str, environment: &str) -> ScopeMember {
        ScopeMember {
            id: ProjectId(project.into()),
            environment_id: EnvironmentId(environment.into()),
            workspace_root: format!("/r/{project}"),
            physical_project_key: format!("{environment}:/r/{project}"),
            environment_label: None,
        }
    }
    fn patch(value: Value) -> SettingsPatch {
        value.as_object().cloned().unwrap_or_default()
    }
    fn project_scope(environments: &[ScopeEnvironment]) -> ResolvedSettingsScope {
        let groups = [ScopeGroup {
            project_key: "app".into(),
            display_name: "App".into(),
            members: vec![member("p1", "a"), member("p2", "b")],
        }];
        resolve_settings_scope(
            &SettingsScopeSearch {
                project: Some("app".into()),
                ..Default::default()
            },
            &groups,
            environments,
        )
    }

    #[test]
    fn client_keys_write_for_this_device_unless_a_project_is_selected() {
        let environments = [
            environment("a", "A", json!({})),
            environment("b", "B", json!({})),
        ];
        let all = resolve_settings_scope(&SettingsScopeSearch::default(), &[], &environments);
        let plan = plan_scoped_settings_patch(
            &all,
            &environments,
            &patch(json!({"confirmThreadDelete": false})),
            false,
        );
        assert_eq!(
            plan.client_patch,
            patch(json!({"confirmThreadDelete": false}))
        );
        assert!(plan.server_writes.is_empty() && plan.unavailable_reason.is_none());
        let project = project_scope(&environments);
        let refused = plan_scoped_settings_patch(
            &project,
            &environments,
            &patch(json!({"confirmThreadDelete": false})),
            false,
        );
        assert!(refused.client_patch.is_empty());
        assert_eq!(
            refused.unavailable_reason.map(|r| r.key),
            Some("This setting is environment-wide and cannot be overridden by a project.")
        );
        let device = plan_scoped_settings_patch(
            &project,
            &environments,
            &patch(json!({"confirmThreadDelete": false})),
            true,
        );
        assert!(device.has_client_write());
    }

    #[test]
    fn server_keys_write_to_every_connected_environment_in_scope() {
        let mut environments = vec![
            environment("a", "A", json!({})),
            environment("b", "B", json!({})),
        ];
        environments[1].phase = Phase::Backoff;
        let all = resolve_settings_scope(&SettingsScopeSearch::default(), &[], &environments);
        let plan = plan_scoped_settings_patch(
            &all,
            &environments,
            &patch(json!({"responseStreamingMode": "paragraph"})),
            false,
        );
        assert_eq!(plan.server_writes.len(), 1);
        assert_eq!(plan.server_writes[0].environment_id.0, "a");
        // A field that is absent by default is still a server key (`Struct.keys`).
        let optional = plan_scoped_settings_patch(
            &all,
            &environments,
            &patch(json!({"defaultThreadEnvMode": "worktree"})),
            false,
        );
        assert_eq!(optional.server_writes.len(), 1);
        environments[0].phase = Phase::Offline;
        let none = plan_scoped_settings_patch(
            &all,
            &environments,
            &patch(json!({"responseStreamingMode": "paragraph"})),
            false,
        );
        assert_eq!(
            none.unavailable_reason.map(|r| r.key),
            Some("Connect an environment to save this setting.")
        );
        let one = resolve_settings_scope(
            &SettingsScopeSearch {
                machine: Some("b".into()),
                ..Default::default()
            },
            &[],
            &environments,
        );
        assert_eq!(one.label, ScopeLabel::Content("B".into()));
        let reason = plan_scoped_settings_patch(
            &one,
            &environments,
            &patch(json!({"responseStreamingMode": "paragraph"})),
            false,
        )
        .unavailable_reason
        .expect("disconnected");
        assert_eq!(reason.english(), "Connect B to save this setting.");
    }

    #[test]
    fn project_overrides_complete_partial_objects_and_inherit_with_null() {
        let environments = [
            environment(
                "a",
                "A",
                json!({"projectSettingsOverrides": {"p1": {"sidebarAutoSettleOnMerge": true, "responseStreamingMode": "paragraph"}}}),
            ),
            environment("b", "B", json!({})),
        ];
        let scope = project_scope(&environments);
        let plan = plan_scoped_settings_patch(
            &scope,
            &environments,
            &patch(json!({"responseStreamingMode": null, "sidebarAutoSettleAfterDays": null})),
            false,
        );
        assert_eq!(plan.server_writes.len(), 2);
        let a = &plan.server_writes[0].patch["projectSettingsOverrides"]["p1"];
        assert_eq!(
            a,
            &json!({"sidebarAutoSettleOnMerge": true, "sidebarAutoSettleAfterDays": null})
        );
        let selection = plan_scoped_settings_patch(
            &scope,
            &environments,
            &patch(json!({"textGenerationModelSelection": {"model": "gpt"}})),
            false,
        );
        let entry = &selection.server_writes[1].patch["projectSettingsOverrides"]["p2"]["textGenerationModelSelection"];
        assert_eq!(entry["model"], "gpt");
        assert!(entry.get("instanceId").is_some() && entry.get("options").is_none());
    }

    #[test]
    fn sources_and_mixed_values_follow_the_targets() {
        let environments = [
            environment(
                "a",
                "A",
                json!({"projectSettingsOverrides": {"p1": {"responseStreamingMode": "turn"}}}),
            ),
            environment("b", "B", json!({})),
        ];
        let scope = project_scope(&environments);
        let targets = resolve_scoped_settings_targets(&scope, &environments);
        assert_eq!(targets.len(), 2);
        assert!(scoped_settings_are_mixed(
            &targets,
            &["responseStreamingMode"]
        ));
        assert!(!scoped_settings_are_mixed(
            &targets,
            &["sidebarAutoSettleOnMerge"]
        ));
        assert_eq!(
            scoped_settings_source(&targets, &["responseStreamingMode"]),
            ScopedSettingSource::Mixed
        );
        assert_eq!(
            scoped_settings_source(&targets[..1], &["responseStreamingMode"]),
            ScopedSettingSource::Project
        );
        assert_eq!(
            scoped_settings_source(&targets, &["confirmThreadDelete"]),
            ScopedSettingSource::Environment
        );
        assert_eq!(
            list_project_overrides(&environments, &["responseStreamingMode"]),
            vec![ProjectOverrideEntry {
                environment_id: EnvironmentId("a".into()),
                project_id: ProjectId("p1".into())
            }]
        );
        let clear = plan_scoped_settings_clear(&scope, &environments, &["responseStreamingMode"]);
        assert_eq!(
            clear.server_writes[0].patch["projectSettingsOverrides"]["p1"],
            Value::Null
        );
    }

    #[test]
    fn a_disabled_override_model_is_not_applied() {
        let settings = Rc::new(
            serde_json::from_value::<ServerSettings>({
                let mut base = serde_json::to_value(ServerSettings::default()).unwrap_or_default();
                base["projectSettingsOverrides"] =
                    json!({"p": {"defaultModelSelection": {"instanceId": "cursor", "model": "m"}}});
                base
            })
            .expect("settings"),
        );
        let resolved = resolve_project_settings(&settings, Some(&ProjectId("p".into())));
        assert_eq!(
            resolved.sources["defaultModelSelection"],
            ProjectSettingSource::Environment
        );
    }

    #[test]
    fn idle_compaction_is_sent_in_the_form_each_server_reads() {
        assert_eq!(
            for_destination(
                &json!({"idleCompaction": "never"}),
                patch(json!({"idleCompaction": "beforeCacheExpires"}))
            ),
            patch(json!({"idleCompaction": "beforeCacheExpires"}))
        );
        assert_eq!(
            for_destination(
                &json!({"compactIdleThreads": "never"}),
                patch(json!({"idleCompaction": "never"}))
            ),
            patch(json!({"compactIdleThreads": "never"}))
        );
        assert_eq!(
            for_destination(
                &json!({"compactIdleThreads": true}),
                patch(json!({"idleCompaction": "beforeCacheExpires"}))
            ),
            patch(json!({"compactIdleThreads": true}))
        );
    }

    #[test]
    fn patch_scopes_follow_the_keys_they_change() {
        use t3_contracts::auth::AuthEnvironmentScope as Scope;
        assert_eq!(
            required_scopes_for_server_settings_patch(&patch(json!({}))),
            vec![Scope::SettingsWrite]
        );
        assert_eq!(
            required_scopes_for_server_settings_patch(&patch(json!({"providerInstances": {}}))),
            vec![Scope::ProvidersManage]
        );
        assert_eq!(
            required_scopes_for_server_settings_patch(&patch(
                json!({"requireConnectDeviceApproval": false, "wordWrap": true})
            )),
            vec![Scope::SettingsWrite, Scope::AccessWrite]
        );
        assert_eq!(
            required_scopes_for_server_settings_patch(&patch(
                json!({"secretProviders": {"x": null}})
            )),
            vec![Scope::SettingsWrite, Scope::ProvidersManage]
        );
    }

    #[test]
    fn a_save_an_older_server_ignored_is_reported_as_dropped() {
        let write = ScopedServerWrite {
            environment_id: EnvironmentId("a".into()),
            label: "A".into(),
            patch: patch(json!({"newKey": true, "kept": "x "})),
            previous: Rc::new(json!({"kept": "y"})),
        };
        assert_eq!(
            dropped_setting_keys(&write, &json!({"kept": "x"})),
            vec!["newKey".to_string()]
        );
        assert_eq!(
            dropped_setting_keys(&write, &json!({"kept": "y", "newKey": true})),
            vec!["kept".to_string()]
        );
    }
}
