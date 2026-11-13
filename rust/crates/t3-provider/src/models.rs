use crate::{
    Result,
    catalog::{boolean_option, capabilities, model, select_option},
    environment::Environment,
    invalid,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString as Text,
    model::*,
    server::{ServerProviderAuth, ServerProviderModel},
};

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
fn effort_label(id: &str) -> &str {
    match id {
        "none" => "None",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra High",
        "max" => "Max",
        "ultra" => "Ultra",
        _ => id,
    }
}

/// SDK model data is supplied by the adapter host; this does not implement the SDK protocol.
pub fn cursor_models(models: &[Value]) -> Result<Vec<ServerProviderModel>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct NativeModel {
        id: String,
        display_name: String,
        #[serde(default)]
        parameters: Vec<Parameter>,
        #[serde(default)]
        variants: Vec<Variant>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Parameter {
        id: String,
        display_name: Option<String>,
        values: Vec<ParameterValue>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ParameterValue {
        value: String,
        display_name: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Variant {
        is_default: Option<bool>,
        params: Vec<VariantParameter>,
    }
    #[derive(Deserialize)]
    struct VariantParameter {
        id: String,
        value: String,
    }
    let priority = |id: &str| match id {
        "effort" | "reasoning" => 0,
        "context" => 1,
        "fast" => 2,
        "thinking" => 3,
        _ => 4,
    };
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for value in models {
        let mut native: NativeModel = serde_json::from_value(value.clone())?;
        let slug = native.id.trim();
        let name = native.display_name.trim();
        if slug.is_empty() || name.is_empty() || !seen.insert(slug.to_owned()) {
            continue;
        }
        native.parameters.sort_by_key(|p| priority(&p.id));
        let mut seen_options = BTreeSet::new();
        let mut options = Vec::new();
        for parameter in native.parameters {
            let native_id = parameter.id.trim();
            let id = match native_id {
                "context" => "contextWindow",
                "fast" => "fastMode",
                _ => native_id,
            };
            if native_id.is_empty() || !seen_options.insert(id.to_owned()) {
                continue;
            }
            let values: Vec<_> = parameter
                .values
                .iter()
                .filter_map(|v| {
                    let value = v.value.trim();
                    (!value.is_empty()).then(|| {
                        (
                            value,
                            v.display_name
                                .as_deref()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .unwrap_or(value),
                        )
                    })
                })
                .collect();
            if values.is_empty() {
                continue;
            }
            let fallback_label = id
                .split([' ', '_', '-'])
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let mut c = s.chars();
                    c.next()
                        .map(|first| first.to_uppercase().to_string() + &c.as_str().to_lowercase())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ");
            let label = parameter
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&fallback_label);
            let default = native
                .variants
                .iter()
                .find(|v| v.is_default == Some(true))
                .and_then(|v| v.params.iter().find(|p| p.id == native_id))
                .map(|p| p.value.as_str());
            let normalized: BTreeSet<_> = values.iter().map(|(v, _)| v.to_lowercase()).collect();
            if values.len() == 2 && normalized.contains("true") && normalized.contains("false") {
                options.push(boolean_option(
                    id,
                    label,
                    match default {
                        Some("true") => Some(true),
                        Some("false") => Some(false),
                        _ => None,
                    },
                ));
            } else {
                options.push(select_option(
                    id,
                    label,
                    &values,
                    default.filter(|d| values.iter().any(|(v, _)| v == d)),
                ));
            }
        }
        output.push(model(slug, name, Some(capabilities(options))));
    }
    Ok(output)
}

pub fn muse_capabilities(model: Option<&Value>) -> ModelCapabilities {
    let variants = model
        .and_then(|m| m.get("variants"))
        .and_then(Value::as_array);
    let fallback = ["low", "medium", "high", "xhigh", "max"];
    let efforts: Vec<_> = match variants {
        Some(values) => values.iter().filter_map(Value::as_str).collect(),
        None => fallback.to_vec(),
    }
    .into_iter()
    .filter(|id| {
        [
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ]
        .contains(id)
    })
    .collect();
    let preferred = if variants.is_some() {
        model
            .and_then(|m| m.get("defaultReasoningEffort"))
            .and_then(Value::as_str)
    } else {
        Some("high")
    };
    let default = preferred
        .filter(|d| efforts.contains(d))
        .or_else(|| efforts.first().copied());
    let Some(default) = default else {
        return capabilities(vec![]);
    };
    let choices: Vec<_> = efforts.iter().map(|id| (*id, effort_label(id))).collect();
    let mut descriptor = select_option("reasoningEffort", "Reasoning", &choices, Some(default));
    if let ProviderOptionDescriptor::Select(d) = &mut descriptor {
        for option in &mut d.options {
            let description = model
                .and_then(|m| m.get("reasoningEffortVariants"))
                .and_then(Value::as_array)
                .and_then(|a| {
                    a.iter()
                        .find(|v| v.get("tier").and_then(Value::as_str) == Some(option.id.as_str()))
                })
                .and_then(|v| v.get("description"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            option.description = description.map(|s| Some(s.to_owned()));
        }
    }
    capabilities(vec![descriptor])
}

pub fn muse_models(catalog: &Value) -> Result<Vec<ServerProviderModel>> {
    if catalog.get("providerId").and_then(Value::as_str) != Some("meta") {
        return Err(invalid(
            "Muse catalog",
            "Muse returned a catalog for another provider.",
        ));
    }
    let entries = catalog
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Muse catalog", "expected a model array"))?;
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for entry in entries {
        let id = entry
            .get("modelId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("Muse catalog", "missing modelId"))?;
        let label = entry
            .get("displayLabel")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("Muse catalog", "missing displayLabel"))?;
        let provider = entry
            .get("providerId")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("Muse catalog", "missing providerId"))?;
        let default = entry
            .get("isDefault")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("Muse catalog", "missing isDefault"))?;
        if entry
            .get("variants")
            .filter(|v| !v.is_null())
            .is_some_and(|v| match v {
                Value::String(s) => s != "unknown",
                Value::Array(values) => !values.iter().all(Value::is_string),
                _ => true,
            })
            || entry
                .get("defaultReasoningEffort")
                .filter(|v| !v.is_null())
                .is_some_and(|v| !v.is_string())
            || entry
                .get("reasoningEffortVariants")
                .filter(|v| !v.is_null())
                .is_some_and(|v| {
                    v.as_array().is_none_or(|values| {
                        values.iter().any(|v| {
                            v.get("tier").is_none_or(|v| !v.is_string())
                                || v.get("description")
                                    .filter(|v| !v.is_null())
                                    .is_some_and(|v| !v.is_string())
                        })
                    })
                })
        {
            return Err(invalid("Muse catalog", "invalid reasoning variants"));
        }
        if provider != "meta" || !seen.insert(id) {
            continue;
        }
        let label = if label.trim().is_empty() {
            id
        } else {
            label.trim()
        };
        let name = if label == id && label.starts_with("muse-") {
            let title = label.replace('-', " ");
            let mut upper_next = true;
            title
                .chars()
                .map(|c| {
                    let next = if upper_next && c.is_ascii_lowercase() {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    };
                    upper_next = !c.is_ascii_alphanumeric() && c != '_';
                    next
                })
                .collect()
        } else {
            label.to_owned()
        };
        let mut model = model(id, &name, Some(muse_capabilities(Some(entry))));
        model.is_default = Some(Some(default));
        output.push(model);
    }
    Ok(output)
}

pub fn grok_capabilities(model: &Value) -> ModelCapabilities {
    let Some(meta) = model
        .get("_meta")
        .filter(|m| m.get("supportsReasoningEffort").and_then(Value::as_bool) != Some(false))
    else {
        return capabilities(vec![]);
    };
    let valid_token = |s: &str| {
        s.len() <= 32
            && s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    let mut seen = BTreeSet::new();
    let mut choices = Vec::new();
    for entry in meta
        .get("reasoningEfforts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let value = text(entry.get("value"))
            .filter(|s| valid_token(s))
            .or_else(|| text(entry.get("id")).filter(|s| valid_token(s)));
        let Some(value) = value.filter(|s| seen.insert((*s).to_owned())) else {
            continue;
        };
        choices.push((
            value.to_owned(),
            text(entry.get("label")).unwrap_or(value).to_owned(),
            text(entry.get("description")).map(str::to_owned),
            entry.get("default").and_then(Value::as_bool) == Some(true)
                || entry.get("isDefault").and_then(Value::as_bool) == Some(true),
        ));
    }
    let current = text(meta.get("reasoningEffort")).filter(|c| choices.iter().any(|v| v.0 == *c));
    let default = choices
        .iter()
        .find(|c| c.3 && Some(c.0.as_str()) == current)
        .or_else(|| choices.iter().find(|c| c.3))
        .map(|c| c.0.clone());
    let rank = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
    if choices.iter().all(|c| rank.contains(&c.0.as_str())) {
        choices.sort_by_key(|c| rank.iter().position(|r| *r == c.0));
    }
    if choices.is_empty() {
        return capabilities(vec![]);
    }
    let pairs: Vec<_> = choices
        .iter()
        .map(|c| (c.0.as_str(), c.1.as_str()))
        .collect();
    let mut descriptor = select_option("reasoningEffort", "Reasoning", &pairs, default.as_deref());
    if let ProviderOptionDescriptor::Select(d) = &mut descriptor {
        d.current_value = current.or(default.as_deref()).map(|s| Some(s.into()));
        for (option, choice) in d.options.iter_mut().zip(&choices) {
            option.description = choice.2.clone().map(Some);
        }
    }
    capabilities(vec![descriptor])
}

pub fn kiro_auth_from_whoami(
    code: Option<i32>,
    stdout: Option<&str>,
    env: &Environment,
) -> Result<ServerProviderAuth> {
    let value = match (code, stdout) {
        (Some(code), Some(output)) if code != 0 => {
            if output.contains("\"account\":null") {
                serde_json::json!({"status":"unauthenticated"})
            } else {
                serde_json::json!({"status":"unknown"})
            }
        }
        (Some(0), Some(output)) => {
            let account = serde_json::from_str::<Value>(
                output.trim_start().lines().next().unwrap_or("").trim(),
            )
            .ok();
            if account
                .as_ref()
                .and_then(|a| a.get("accountType"))
                .and_then(Value::as_str)
                == Some("ApiKey")
                || env
                    .get("KIRO_API_KEY")
                    .is_some_and(|s| !s.trim().is_empty())
            {
                serde_json::json!({"status":"authenticated","type":"api_key","label":"Kiro API key"})
            } else {
                let mut auth = serde_json::json!({"status":"authenticated","label":"Kiro account"});
                if let Some(email) = account
                    .as_ref()
                    .and_then(|a| a.get("email"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    auth["email"] = serde_json::json!(email);
                }
                auth
            }
        }
        _ => serde_json::json!({"status":"unknown"}),
    };
    Ok(serde_json::from_value(value)?)
}

pub fn kiro_capabilities(id: &str) -> ModelCapabilities {
    let base = &["low", "medium", "high", "xhigh", "max"][..];
    let old = &["low", "medium", "high", "max"][..];
    let gpt = &["none", "low", "medium", "high", "xhigh", "max"][..];
    let (levels, default) = match id {
        "claude-opus-5.5" => (base, "medium"),
        "claude-opus-4.7" => (base, "xhigh"),
        "claude-fable-5.1" | "claude-opus-5" | "claude-opus-4.8" | "claude-sonnet-5.5"
        | "claude-sonnet-5" => (base, "high"),
        "claude-opus-4.6" | "claude-sonnet-4.6" => (old, "high"),
        "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" => (gpt, "high"),
        "kimi-k3" => (&["low", "high", "max"][..], "max"),
        _ => return capabilities(vec![]),
    };
    capabilities(vec![select_option(
        "reasoningEffort",
        "Reasoning",
        &levels
            .iter()
            .map(|id| (*id, effort_label(id)))
            .collect::<Vec<_>>(),
        Some(default),
    )])
}

pub fn kiro_models_from_list(code: i32, stdout: &str) -> Result<Option<Vec<ServerProviderModel>>> {
    #[derive(Deserialize)]
    struct Listed {
        models: Vec<ListedModel>,
        default_model: Option<String>,
    }
    #[derive(Deserialize)]
    struct ListedModel {
        model_id: String,
        model_name: Option<String>,
    }
    if code != 0 {
        return Ok(None);
    }
    let Ok(list) = serde_json::from_str::<Listed>(stdout.trim()) else {
        return Ok(None);
    };
    if list.models.is_empty() {
        return Ok(None);
    }
    let default = list.default_model.as_deref().unwrap_or("auto");
    let listed_default = list.models.iter().position(|m| m.model_id == default);
    let name = listed_default
        .map(|i| {
            list.models[i]
                .model_name
                .as_deref()
                .unwrap_or(&list.models[i].model_id)
        })
        .unwrap_or("Kiro default");
    let mut default_model = model("default", name, Some(capabilities(vec![])));
    default_model.aliases = Some(Some(vec![Text(default.into())]));
    default_model.is_default = Some(Some(true));
    let mut output = Vec::new();
    if listed_default.is_none() {
        output.push(default_model.clone());
    }
    for (i, m) in list.models.iter().enumerate() {
        output.push(if Some(i) == listed_default {
            default_model.clone()
        } else {
            model(
                &m.model_id,
                m.model_name.as_deref().unwrap_or(&m.model_id),
                Some(kiro_capabilities(&m.model_id)),
            )
        });
    }
    Ok(Some(output))
}

pub fn kiro_agent_descriptor(template: &Value) -> Option<ProviderOptionDescriptor> {
    let option = template
        .get("configOptions")?
        .as_array()?
        .iter()
        .find(|o| {
            o.get("id").and_then(Value::as_str) == Some("mode")
                && o.get("type").and_then(Value::as_str) == Some("select")
        })?;
    let current = option.get("currentValue").and_then(Value::as_str);
    let choices: Vec<_> = option
        .get("options")?
        .as_array()?
        .iter()
        .filter_map(|v| {
            let id = text(v.get("value"))?;
            Some((
                id,
                text(v.get("name")).unwrap_or(id),
                text(v.get("description")),
            ))
        })
        .collect();
    if choices.len() < 2 {
        return None;
    }
    let pairs: Vec<_> = choices.iter().map(|(id, name, _)| (*id, *name)).collect();
    let mut descriptor = select_option(
        "mode",
        "Agent",
        &pairs,
        current.filter(|c| choices.iter().any(|(id, _, _)| id == c)),
    );
    if let ProviderOptionDescriptor::Select(d) = &mut descriptor {
        for (choice, (_, _, description)) in d.options.iter_mut().zip(choices) {
            choice.description = description.map(|s| Some(s.to_owned()));
        }
    }
    Some(descriptor)
}

pub fn default_models_by_driver() -> BTreeMap<&'static str, &'static str> {
    t3_contracts::model::DEFAULT_MODEL_BY_PROVIDER
        .iter()
        .copied()
        .collect()
}
