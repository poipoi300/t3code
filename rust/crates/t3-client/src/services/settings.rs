//! Shared settings rules, plus the settings pages' scope, search and composed service.
pub mod scope;
pub mod scoped;
pub mod search;
mod search_items;
pub mod panels;
pub mod service;

use crate::ClientError;
use serde_json::{Value, json};
use t3_contracts::{
    provider_instance::ProviderInstanceId,
    settings::{ServerSettings, ServerSettingsPatch},
};

const SHARED_KEYS: &[&str] = &[
    "continueThreadsAfterServerUpdate",
    "sidebarAutoSettleAfterDays",
    "sidebarAutoSettleOnMerge",
    "autoResumeLimitedThreads",
    "snoozeLimitedThreads",
    "snoozePresets",
    "newWorktreesStartFromOrigin",
    "sourceControlWritingStyle",
    "textGenerationModelSelection",
];
pub fn split_shared_server_patch(
    patch: ServerSettingsPatch,
) -> Result<(ServerSettingsPatch, ServerSettingsPatch), ClientError> {
    let value = serde_json::to_value(patch)?;
    let mut shared = json!({});
    let mut local = json!({});
    if let Some(fields) = value.as_object() {
        for (key, value) in fields {
            if SHARED_KEYS.contains(&key.as_str()) {
                shared[key] = value.clone();
            } else {
                local[key] = value.clone();
            }
        }
    }
    Ok((
        serde_json::from_value(shared)?,
        serde_json::from_value(local)?,
    ))
}
fn selection_enabled(settings: &ServerSettings, instance: &ProviderInstanceId) -> bool {
    if let Some(generator) = settings.text_generators.get(instance) {
        return generator.enabled;
    }
    if let Some(config) = settings.provider_instances.get(instance) {
        let config_enabled = config
            .config
            .as_ref()
            .and_then(|config| config.get("enabled"))
            .and_then(Value::as_bool);
        if config.enabled == Some(false) || config_enabled == Some(false) {
            return false;
        }
        return config.enabled.or(config_enabled).unwrap_or(!matches!(
            config.driver.0.as_str(),
            "cursor" | "grok" | "fx" | "kiro" | "muse" | "pi" | "opencode" | "antigravity"
        ));
    }
    matches!(instance.0.as_str(), "codex" | "claudeAgent")
}
pub fn filter_shared_server_patch(
    patch: ServerSettingsPatch,
    settings: Option<&ServerSettings>,
    source: Option<&ServerSettings>,
    target_is_source: bool,
) -> Result<ServerSettingsPatch, ClientError> {
    let source = source.or(settings);
    let mut value = serde_json::to_value(patch)?;
    if !target_is_source && let Some(selection) = value.get("textGenerationModelSelection") {
        let id = selection
            .get("instanceId")
            .and_then(Value::as_str)
            .map(|id| ProviderInstanceId(id.into()))
            .or_else(|| {
                source.map(|source| source.text_generation_model_selection.instance_id.clone())
            });
        let keep = settings.is_some_and(|settings| {
            id.as_ref().is_some_and(|id| {
                let source_driver = source
                    .and_then(|source| source.provider_instances.get(id))
                    .map_or(&id.0, |config| &config.driver.0);
                let driver = settings
                    .provider_instances
                    .get(id)
                    .map_or(&id.0, |config| &config.driver.0);
                source_driver == driver && selection_enabled(settings, id)
            })
        });
        if !keep && let Some(value) = value.as_object_mut() {
            value.remove("textGenerationModelSelection");
        }
    }
    if settings.is_none()
        && let Some(value) = value.as_object_mut()
    {
        value.remove("continueThreadsAfterServerUpdate");
    }
    Ok(serde_json::from_value(value)?)
}
pub fn supports_shared_settings_sync(phase: &str, config_loaded: bool) -> bool {
    phase == "connected" && config_loaded
}
pub fn hidden_thread_tags(settings: &ServerSettings) -> Option<Vec<String>> {
    if !settings.hide_tagged_threads {
        return None;
    }
    let tags: Vec<_> = settings
        .hidden_thread_tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect();
    (!tags.is_empty()).then_some(tags)
}
pub fn usage_scan_settings_key(settings: &ServerSettings) -> Result<String, ClientError> {
    let prices: Vec<_> = settings
        .usage_price_overrides
        .iter()
        .map(|(model, price)| {
            json!([
                model,
                price.input_cost_per_million_tokens,
                price.output_cost_per_million_tokens,
                price.cache_read_cost_per_million_tokens,
                price.cache_write_cost_per_million_tokens
            ])
        })
        .collect();
    let aliases: Vec<_> = settings.usage_model_aliases.iter().collect();
    Ok(serde_json::to_string(&(
        serde_json::to_string(&prices)?,
        aliases,
    ))?)
}
