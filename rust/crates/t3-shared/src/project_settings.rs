//! Port of `resolveProjectSettings` (`packages/shared/src/projectSettings.ts`, without the
//! `t3.json` overlay) and the enablement helpers it needs from `packages/contracts`
//! (`resolveProjectProviderInstanceEnabled`, `resolveTextGenerationSelectionEnabled`) and
//! `packages/shared` (`fallbackTextGenerationModelSelection`, `readCustomModelEntries`).
//! TypeScript spreads settings objects key by key; this works on their JSON form the same way.
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use t3_contracts::{
    base_schemas::ProjectId,
    environment::ThreadEnvMode,
    model_selection::ModelSelection,
    settings::{ServerSettings, ServerSettingsProjectSettingsOverridesValue},
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSettingSource {
    Environment,
    Project,
}

#[derive(Debug, Clone)]
pub struct ResolvedProjectSettings {
    pub settings: ServerSettings,
    /// One entry per [`PROJECT_SCOPED_SERVER_SETTING_KEYS`] key.
    pub sources: BTreeMap<&'static str, ProjectSettingSource>,
    pub overrides: ServerSettingsProjectSettingsOverridesValue,
}

/// A project aggregate's own fields; they apply only until the server folds them into overrides.
#[derive(Debug, Clone, Default)]
pub struct LegacyProjectSettingsFields {
    pub default_model_selection: Option<ModelSelection>,
    pub default_thread_env_mode: Option<ThreadEnvMode>,
}

pub fn has_project_settings_overrides(settings: &ServerSettings) -> crate::Result<bool> {
    for entry in settings.project_settings_overrides.values() {
        if serde_json::to_value(entry)?
            .as_object()
            .is_some_and(|fields| !fields.is_empty())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn resolve_project_settings(
    settings: &ServerSettings,
    project_id: Option<&ProjectId>,
    project: Option<&LegacyProjectSettingsFields>,
) -> crate::Result<ResolvedProjectSettings> {
    let environment_sources = || {
        PROJECT_SCOPED_SERVER_SETTING_KEYS
            .iter()
            .map(|key| (*key, ProjectSettingSource::Environment))
            .collect::<BTreeMap<_, _>>()
    };
    let stored = match project_id.and_then(|id| settings.project_settings_overrides.get(id)) {
        Some(entry) => object(serde_json::to_value(entry)?),
        None => Map::new(),
    };
    let overrides = match project {
        Some(project) if !settings.project_settings_folded => {
            let mut merged = Map::new();
            if let Some(selection) = &project.default_model_selection {
                merged.insert(
                    "defaultModelSelection".into(),
                    serde_json::to_value(selection)?,
                );
            }
            if let Some(mode) = &project.default_thread_env_mode {
                merged.insert("defaultThreadEnvMode".into(), serde_json::to_value(mode)?);
            }
            merged.extend(stored);
            merged
        }
        _ => stored,
    };
    if overrides.is_empty() {
        return Ok(ResolvedProjectSettings {
            settings: settings.clone(),
            sources: environment_sources(),
            overrides: ServerSettingsProjectSettingsOverridesValue::default(),
        });
    }
    let environment = serde_json::to_value(settings)?;
    let project_id = project_id.map(|id| id.0.as_str());
    let mut effective = object(environment.clone());
    let mut sources = environment_sources();
    for key in PROJECT_SCOPED_SERVER_SETTING_KEYS {
        let Some(value) = overrides.get(key) else {
            continue;
        };
        let is_model = key == "textGenerationModelSelection" || key == "defaultModelSelection";
        if is_model && !value.is_null() {
            let instance = value["instanceId"].as_str().unwrap_or_default();
            let enabled = if key == "textGenerationModelSelection" {
                text_generation_selection_enabled(&environment, project_id, instance)
            } else {
                project_provider_instance_enabled(&environment, project_id, instance)
            };
            if !enabled {
                continue;
            }
        }
        // Clients resolve a hidden model to the catalog's first visible one, so a hidden
        // project default would silently pick a model nobody chose. Custom models stay visible.
        if key == "defaultModelSelection" && !value.is_null() {
            let instance = value["instanceId"].as_str().unwrap_or_default();
            let model = &value["model"];
            let hidden = environment["providerModelPreferences"][instance]["hiddenModels"]
                .as_array()
                .is_some_and(|models| models.contains(model));
            let custom = custom_model_slugs(
                &environment["providerInstances"][instance]["config"]["customModels"],
            );
            if hidden
                && !model
                    .as_str()
                    .is_some_and(|m| custom.iter().any(|s| s == m))
            {
                continue;
            }
        }
        effective.insert(key.into(), value.clone());
        sources.insert(key, ProjectSettingSource::Project);
    }
    let text_instance = effective["textGenerationModelSelection"]["instanceId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if !text_generation_selection_enabled(&environment, project_id, &text_instance)
        && let Some(fallback) = fallback_text_generation_model_selection(
            &environment["providerInstances"],
            |instance, _, _| project_provider_instance_enabled(&environment, project_id, instance),
            &effective["defaultModelSelection"],
        )
    {
        effective.insert("textGenerationModelSelection".into(), fallback);
    }
    Ok(ResolvedProjectSettings {
        settings: serde_json::from_value(Value::Object(effective))?,
        sources,
        overrides: serde_json::from_value(Value::Object(overrides))?,
    })
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(fields) => fields,
        _ => Map::new(),
    }
}

/// Must match each driver's `enabled` decoding default.
const DEFAULT_DISABLED_PROVIDER_DRIVERS: [&str; 8] = [
    "cursor",
    "grok",
    "fx",
    "kiro",
    "muse",
    "pi",
    "opencode",
    "antigravity",
];
const DEFAULT_ENABLED_DEFAULT_INSTANCES: [&str; 2] = ["codex", "claudeAgent"];

/// `resolveProviderInstanceEnabled` over one `providerInstances` entry.
pub fn provider_instance_enabled(instance: &Value) -> bool {
    let config_enabled = instance["config"]
        .as_object()
        .and_then(|config| config.get("enabled"))
        .and_then(Value::as_bool);
    let enabled = instance["enabled"].as_bool();
    if enabled == Some(false) || config_enabled == Some(false) {
        return false;
    }
    enabled.or(config_enabled).unwrap_or_else(|| {
        !instance["driver"]
            .as_str()
            .is_some_and(|driver| DEFAULT_DISABLED_PROVIDER_DRIVERS.contains(&driver))
    })
}

/// `resolveProjectProviderInstanceEnabled`, over the settings' JSON form.
pub fn project_provider_instance_enabled(
    settings: &Value,
    project_id: Option<&str>,
    instance_id: &str,
) -> bool {
    if let Some(project_id) = project_id
        && let Some(enabled) = settings["projectSettingsOverrides"][project_id]["providerInstances"]
            [instance_id]["enabled"]
            .as_bool()
    {
        return enabled;
    }
    match settings["providerInstances"].get(instance_id) {
        Some(instance) => provider_instance_enabled(instance),
        None => DEFAULT_ENABLED_DEFAULT_INSTANCES.contains(&instance_id),
    }
}

/// `resolveTextGenerationSelectionEnabled`: a text generator of that id takes precedence.
pub fn text_generation_selection_enabled(
    settings: &Value,
    project_id: Option<&str>,
    instance_id: &str,
) -> bool {
    match settings["textGenerators"].get(instance_id) {
        Some(generator) => generator["enabled"].as_bool().unwrap_or(false),
        None => project_provider_instance_enabled(settings, project_id, instance_id),
    }
}

/// The slugs `readCustomModelEntries` keeps: trimmed, non-empty, first occurrence.
pub fn custom_model_slugs(value: &Value) -> Vec<String> {
    let mut slugs: Vec<String> = Vec::new();
    for raw in value.as_array().into_iter().flatten() {
        let slug = match raw {
            Value::String(slug) => Some(slug.as_str()),
            Value::Object(record) => record.get("slug").and_then(Value::as_str),
            _ => None,
        };
        if let Some(slug) = slug.map(str::trim).filter(|slug| !slug.is_empty())
            && !slugs.iter().any(|seen| seen == slug)
        {
            slugs.push(slug.to_owned());
        }
    }
    slugs
}

const TEXT_GENERATION_FALLBACK_DRIVERS: [&str; 8] = [
    "codex",
    "claudeAgent",
    "cursor",
    "grok",
    "muse",
    "pi",
    "opencode",
    "antigravity",
];

/// `fallbackTextGenerationModelSelection`: default instances first, then configured ones (in
/// id order: the Rust settings map is sorted where TypeScript keeps insertion order). The
/// thread default's custom model wins on its own instance.
pub fn fallback_text_generation_model_selection(
    provider_instances: &Value,
    is_enabled: impl Fn(&str, &str, &str) -> bool,
    default_model_selection: &Value,
) -> Option<Value> {
    let configured = provider_instances
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, instance)| {
            instance["driver"]
                .as_str()
                .filter(|driver| TEXT_GENERATION_FALLBACK_DRIVERS.contains(driver))
                .map(|driver| (id.as_str(), driver))
        });
    let candidates = TEXT_GENERATION_FALLBACK_DRIVERS
        .iter()
        .map(|driver| (*driver, *driver))
        .chain(configured);
    for (instance, driver) in candidates {
        let driver_default = serde_json::json!({
            "instanceId": instance,
            "model": default_text_generation_model(driver),
        });
        let custom_default = default_model_selection["instanceId"].as_str() == Some(instance)
            && default_model_selection["model"]
                .as_str()
                .is_some_and(|model| {
                    custom_model_slugs(&provider_instances[instance]["config"]["customModels"])
                        .iter()
                        .any(|slug| slug == model)
                });
        let selections = if custom_default {
            vec![default_model_selection.clone(), driver_default]
        } else {
            vec![driver_default]
        };
        for selection in selections {
            if is_enabled(
                instance,
                driver,
                selection["model"].as_str().unwrap_or_default(),
            ) {
                return Some(selection);
            }
        }
    }
    None
}

fn default_text_generation_model(driver: &str) -> &'static str {
    use t3_contracts::model::{
        DEFAULT_MODEL_BY_PROVIDER, DEFAULT_TEXT_GENERATION_MODEL,
        DEFAULT_TEXT_GENERATION_MODEL_BY_PROVIDER,
    };
    DEFAULT_TEXT_GENERATION_MODEL_BY_PROVIDER
        .iter()
        .chain(DEFAULT_MODEL_BY_PROVIDER)
        .find(|(candidate, _)| *candidate == driver)
        .map_or(DEFAULT_TEXT_GENERATION_MODEL, |(_, model)| *model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PROJECT: &str = "project-a";
    const OTHER: &str = "project-b";

    fn settings(value: Value) -> ServerSettings {
        serde_json::from_value(value).unwrap()
    }
    fn id(value: &str) -> ProjectId {
        ProjectId(value.into())
    }
    fn resolve(settings: &ServerSettings, project: &str) -> ResolvedProjectSettings {
        resolve_project_settings(settings, Some(&id(project)), None).unwrap()
    }

    #[test]
    fn applies_overrides_per_key_and_reports_their_source() {
        let settings = settings(json!({
            "defaultAutoPull": true,
            "sidebarAutoSettleAfterDays": 3,
            "projectSettingsOverrides": {
                PROJECT: {"defaultAutoPull": false, "sidebarAutoSettleAfterDays": null}
            }
        }));
        let resolved = resolve(&settings, PROJECT);
        assert!(!resolved.settings.default_auto_pull);
        assert!(resolved.settings.sidebar_auto_settle_after_days.is_none());
        assert_eq!(
            resolved.settings.default_thread_env_mode,
            settings.default_thread_env_mode
        );
        assert_eq!(
            resolved.sources["defaultAutoPull"],
            ProjectSettingSource::Project
        );
        assert_eq!(
            resolved.sources["sidebarAutoSettleAfterDays"],
            ProjectSettingSource::Project
        );
        assert_eq!(
            resolved.sources["defaultThreadEnvMode"],
            ProjectSettingSource::Environment
        );
        assert!(resolve(&settings, OTHER).settings.default_auto_pull);
        assert!(has_project_settings_overrides(&settings).unwrap());
        assert!(!has_project_settings_overrides(&ServerSettings::default()).unwrap());
        assert!(
            resolve_project_settings(&settings, None, None)
                .unwrap()
                .settings
                .default_auto_pull
        );
    }

    #[test]
    fn honours_the_aggregates_own_fields_only_until_the_server_has_folded_them() {
        let aggregate: ModelSelection =
            serde_json::from_value(json!({"instanceId": "codex", "model": "gpt-5.5"})).unwrap();
        let project = LegacyProjectSettingsFields {
            default_model_selection: Some(aggregate.clone()),
            default_thread_env_mode: Some(serde_json::from_value(json!("local")).unwrap()),
        };
        let unfolded = resolve_project_settings(
            &settings(json!({"projectSettingsFolded": false})),
            Some(&id(PROJECT)),
            Some(&project),
        )
        .unwrap();
        assert_eq!(unfolded.settings.default_model_selection, Some(aggregate));
        assert_eq!(
            serde_json::to_value(&unfolded.settings.default_thread_env_mode).unwrap(),
            json!("local")
        );
        assert_eq!(
            unfolded.sources["defaultModelSelection"],
            ProjectSettingSource::Project
        );
        let overridden = resolve_project_settings(
            &settings(json!({
                "projectSettingsFolded": false,
                "projectSettingsOverrides": {PROJECT: {"defaultThreadEnvMode": "worktree"}}
            })),
            Some(&id(PROJECT)),
            Some(&project),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&overridden.settings.default_thread_env_mode).unwrap(),
            json!("worktree")
        );
        let folded = resolve_project_settings(
            &settings(json!({"projectSettingsFolded": true})),
            Some(&id(PROJECT)),
            Some(&project),
        )
        .unwrap();
        assert!(folded.settings.default_model_selection.is_none());
        assert_eq!(
            folded.sources["defaultModelSelection"],
            ProjectSettingSource::Environment
        );
    }

    #[test]
    fn keeps_the_environment_default_model_when_the_overrides_provider_is_disabled() {
        let resolved = resolve(
            &settings(json!({
                "providerInstances": {"claudeAgent": {"driver": "claudeAgent", "enabled": false}},
                "projectSettingsOverrides": {
                    PROJECT: {"defaultModelSelection": {"instanceId": "claudeAgent", "model": "opus"}}
                }
            })),
            PROJECT,
        );
        assert!(resolved.settings.default_model_selection.is_none());
        assert_eq!(
            resolved.sources["defaultModelSelection"],
            ProjectSettingSource::Environment
        );
    }

    // regression: #17556 a hidden project default silently became the catalog's first model
    #[test]
    fn resolves_hidden_visible_and_custom_project_default_models() {
        for (hidden, custom, expected, source) in [
            (
                json!(["gpt-5.6-terra", "gpt-5.6-sol"]),
                json!([]),
                "gpt-6.1-sol",
                ProjectSettingSource::Environment,
            ),
            (
                json!(["gpt-5.6-terra"]),
                json!([]),
                "gpt-5.6-sol",
                ProjectSettingSource::Project,
            ),
            (
                json!(["gpt-5.6-sol"]),
                json!(["gpt-5.6-sol"]),
                "gpt-5.6-sol",
                ProjectSettingSource::Project,
            ),
        ] {
            let resolved = resolve(
                &settings(json!({
                    "defaultModelSelection": {"instanceId": "codex", "model": "gpt-6.1-sol"},
                    "providerInstances": {"codex": {"driver": "codex", "config": {"customModels": custom}}},
                    "providerModelPreferences": {"codex": {"hiddenModels": hidden, "modelOrder": []}},
                    "projectSettingsOverrides": {
                        PROJECT: {"defaultModelSelection": {"instanceId": "codex", "model": "gpt-5.6-sol"}}
                    }
                })),
                PROJECT,
            );
            assert_eq!(
                resolved.settings.default_model_selection.unwrap().model,
                expected
            );
            assert_eq!(resolved.sources["defaultModelSelection"], source);
            assert_eq!(
                resolved
                    .overrides
                    .default_model_selection
                    .flatten()
                    .unwrap()
                    .model,
                "gpt-5.6-sol"
            );
        }
    }

    #[test]
    fn moves_text_generation_off_an_instance_the_project_turned_off() {
        let settings = settings(json!({
            "textGenerationModelSelection": {"instanceId": "codex", "model": "gpt-5"},
            "projectSettingsOverrides": {
                PROJECT: {"providerInstances": {"codex": {"enabled": false}}}
            }
        }));
        assert_eq!(
            resolve(&settings, PROJECT)
                .settings
                .text_generation_model_selection
                .instance_id
                .0,
            "claudeAgent"
        );
        assert_eq!(
            resolve(&settings, OTHER)
                .settings
                .text_generation_model_selection,
            settings.text_generation_model_selection
        );
    }

    #[test]
    fn falls_back_to_a_configured_instance_when_every_built_in_one_is_off_for_the_project() {
        let settings = settings(json!({
            "textGenerationModelSelection": {"instanceId": "codex", "model": "gpt-5"},
            "providerInstances": {"codex_work": {"driver": "codex", "config": {}}},
            "projectSettingsOverrides": {
                PROJECT: {"providerInstances": {
                    "codex": {"enabled": false},
                    "claudeAgent": {"enabled": false}
                }}
            }
        }));
        assert_eq!(
            resolve(&settings, PROJECT)
                .settings
                .text_generation_model_selection
                .instance_id
                .0,
            "codex_work"
        );
    }

    // serverSettings.test.ts: "uses Big Pickle when only OpenCode is enabled"
    #[test]
    fn fallback_uses_the_driver_default_when_the_custom_default_is_unusable() {
        assert_eq!(
            fallback_text_generation_model_selection(
                &json!({}),
                |instance, _, _| instance == "opencode",
                &Value::Null
            ),
            Some(json!({"instanceId": "opencode", "model": "opencode/big-pickle"}))
        );
        let thread_default = json!({"instanceId": "opencode", "model": "opencode-go/glm-5.3"});
        let instances = json!({"opencode": {"driver": "opencode", "config": {"customModels": ["opencode-go/glm-5.3"]}}});
        assert_eq!(
            fallback_text_generation_model_selection(
                &instances,
                |instance, _, model| instance == "opencode" && !model.starts_with("opencode-go/"),
                &thread_default
            ),
            Some(json!({"instanceId": "opencode", "model": "opencode/big-pickle"}))
        );
        assert_eq!(
            fallback_text_generation_model_selection(
                &instances,
                |instance, _, _| instance == "opencode",
                &thread_default
            ),
            Some(thread_default)
        );
    }
}
