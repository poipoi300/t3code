//! The new-thread subset of projectSettings.ts, chatThreadActions.ts and usageLimits.ts.
use crate::{
    ClientError,
    thread_views::composer::delivery_policy::ThreadSettings,
    types::ThreadKey,
    view_logic::shell::models,
    workspace::projects::{EnvironmentProject, normalize_project_path_for_comparison},
};
use serde_json::{Value, json};
use t3_contracts::{
    environment::ThreadEnvMode,
    model_selection::ModelSelection,
    provider_policy::ProviderInteractionMode,
    server::{ServerConfig, ServerProvider},
    settings::ServerSettings,
};

pub(super) fn effective_settings(
    config: &ServerConfig,
    project: &EnvironmentProject,
) -> Result<ServerSettings, ClientError> {
    let mut value = serde_json::to_value(&config.settings)?;
    let mut overrides = json!({});
    let legacy = serde_json::to_value(&project.project)?;
    if !config.settings.project_settings_folded {
        for field in ["defaultModelSelection", "defaultThreadEnvMode"] {
            if !legacy[field].is_null() {
                overrides[field] = legacy[field].clone();
            }
        }
    }
    if let Some(stored) = value["projectSettingsOverrides"]
        .get(&project.project.id.0)
        .and_then(Value::as_object)
    {
        overrides
            .as_object_mut()
            .expect("override map")
            .extend(stored.clone());
    }
    for field in [
        "defaultModelSelection",
        "defaultThreadEnvMode",
        "defaultRuntimeMode",
        "newWorktreesStartFromOrigin",
    ] {
        let Some(next) = overrides.get(field) else {
            continue;
        };
        if field == "defaultModelSelection" && !next.is_null() {
            let pick: ModelSelection = serde_json::from_value(next.clone())?;
            if !models::project_instance_enabled(
                &config.settings,
                Some(&project.project.id),
                &pick.instance_id,
            ) {
                continue;
            }
            let settings = serde_json::to_value(&config.settings)?;
            let hidden = settings["providerModelPreferences"][&pick.instance_id.0]["hiddenModels"]
                .as_array()
                .is_some_and(|v| v.iter().any(|v| v == &pick.model));
            let custom =
                settings["providerInstances"][&pick.instance_id.0]["config"]["customModels"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|r| r["slug"] == pick.model));
            if hidden && !custom {
                continue;
            }
        }
        value[field] = next.clone();
    }
    Ok(serde_json::from_value(value)?)
}

fn exhausted(provider: &ServerProvider, model: &str, now: u64) -> bool {
    let Ok(provider) = serde_json::to_value(provider) else {
        return false;
    };
    let all = provider["usageLimits"]["windows"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let exhausted = all
        .iter()
        .filter(|w| {
            w["usedPercent"].as_f64().unwrap_or(0.0) >= 100.0
                && w.get("modelIds")
                    .and_then(Value::as_array)
                    .is_none_or(|ids| ids.iter().any(|id| id == model))
                && w.get("providerIds")
                    .and_then(Value::as_array)
                    .is_none_or(|ids| {
                        ids.iter()
                            .any(|id| id == model.split('/').next().unwrap_or_default())
                    })
                && w["resetsAt"]
                    .as_str()
                    .and_then(crate::time::epoch_ms)
                    .is_none_or(|reset| reset > i64::try_from(now).unwrap_or(i64::MAX))
        })
        .collect::<Vec<_>>();
    let has = |id: &str| exhausted.iter().any(|w| w["id"] == id);
    match provider["driver"].as_str().unwrap_or_default() {
        "cursor" => {
            if ["auto", "default", "grok", "composer"]
                .iter()
                .any(|p| model.starts_with(p))
            {
                has("autoPercentUsed") && has("apiPercentUsed")
            } else {
                has("apiPercentUsed")
            }
        }
        "codex" => exhausted.iter().any(|w| {
            w["id"] != "individual"
                || !all
                    .iter()
                    .any(|w| matches!(w["id"].as_str(), Some("primary" | "secondary")))
        }),
        "opencode" => exhausted.iter().any(|w| {
            !w["id"].as_str().unwrap_or_default().starts_with("go_")
                || model.starts_with("opencode-go/")
        }),
        "antigravity" => {
            let family = ["gemini", "claude", "gpt"]
                .into_iter()
                .find(|p| model.starts_with(&format!("{p}-")));
            exhausted.iter().any(|w| {
                let pool = ["gemini", "claude", "gpt", "3p"].into_iter().find(|p| {
                    w["id"]
                        .as_str()
                        .unwrap_or_default()
                        .starts_with(&format!("{p}-"))
                });
                pool.is_none()
                    || family.is_none()
                    || pool == family
                    || pool == Some("3p") && matches!(family, Some("claude" | "gpt"))
            })
        }
        _ => !exhausted.is_empty(),
    }
}
pub(super) fn settings(
    config: &ServerConfig,
    project: &EnvironmentProject,
    draft: &Value,
    session: &Value,
    now: u64,
) -> Result<ThreadSettings, ClientError> {
    let settings = effective_settings(config, project)?;
    let active = draft["activeProvider"].as_str();
    let explicit = active.and_then(|id| {
        serde_json::from_value::<ModelSelection>(draft["modelSelectionByProvider"][id].clone()).ok()
    });
    let default = settings.default_model_selection.clone().or_else(|| {
        models::project_default(&config.providers.0, &settings, Some(&project.project.id))
    });
    let mut selection = explicit
        .clone()
        .or(default)
        .ok_or_else(|| ClientError::InvalidPayload("Choose a model before sending.".into()))?;
    if explicit.is_none()
        && let Some(fallback) = settings.fallback_model_selection.as_ref()
    {
        let current = config
            .providers
            .0
            .iter()
            .find(|p| p.instance_id == selection.instance_id);
        let alternative = config
            .providers
            .0
            .iter()
            .find(|p| p.instance_id == fallback.instance_id);
        if current.is_some_and(|p| exhausted(p, &selection.model, now))
            && alternative.is_some_and(|p| {
                p.enabled
                    && p.auth.status
                        != t3_contracts::server::ServerProviderAuthStatus::Unauthenticated
                    && p.availability
                        != Some(Some(
                            t3_contracts::server::ServerProviderAvailability::Unavailable,
                        ))
                    && !exhausted(p, &fallback.model, now)
            })
            && models::project_instance_enabled(
                &settings,
                Some(&project.project.id),
                &fallback.instance_id,
            )
        {
            selection = fallback.clone();
        }
    }
    Ok(ThreadSettings {
        model_selection: selection,
        runtime_mode: serde_json::from_value(session["runtimeMode"].clone())
            .unwrap_or(settings.default_runtime_mode),
        interaction_mode: serde_json::from_value(draft["interactionMode"].clone())
            .or_else(|_| serde_json::from_value(session["interactionMode"].clone()))
            .unwrap_or(ProviderInteractionMode::Default),
    })
}

pub(super) fn env_mode(
    settings: &ServerSettings,
    project: &EnvironmentProject,
    file: Option<&Value>,
    new_root: Option<&str>,
    has_threads: bool,
) -> ThreadEnvMode {
    let file_mode = file
        .and_then(|file| file.get("defaultThreadEnvMode"))
        .and_then(|mode| serde_json::from_value(mode.clone()).ok());
    let mode = settings
        .default_thread_env_mode
        .clone()
        .or(file_mode.clone())
        .unwrap_or(ThreadEnvMode::Local);
    let overridden = settings
        .project_settings_overrides
        .get(&project.project.id)
        .is_some_and(|p| p.default_thread_env_mode.is_some());
    if mode == ThreadEnvMode::Local
        || has_threads
        || overridden
        || settings.default_thread_env_mode.is_none() && file_mode.is_some()
    {
        return mode;
    }
    let Some(root) = new_root else { return mode };
    let root = normalize_project_path_for_comparison(root);
    let path = normalize_project_path_for_comparison(&project.project.workspace_root.0);
    let separator = if root.contains('\\') { '\\' } else { '/' };
    let prefix = if root.ends_with(separator) {
        root
    } else {
        format!("{root}{separator}")
    };
    if path
        .strip_prefix(&prefix)
        .is_some_and(|tail| !tail.is_empty() && !tail.contains(separator))
    {
        ThreadEnvMode::Local
    } else {
        mode
    }
}

pub(super) fn session_key(session: &Value) -> Option<ThreadKey> {
    Some(ThreadKey {
        environment: crate::types::EnvironmentId(session["environmentId"].as_str()?.into()),
        thread: crate::types::ThreadId(session["threadId"].as_str()?.into()),
    })
}
