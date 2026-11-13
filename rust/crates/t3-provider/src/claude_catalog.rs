use crate::{
    catalog::{self, Catalog, capabilities},
    versions::compare_versions,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    model::{ModelCapabilities, ProviderOptionDescriptor},
    server::{ServerProviderModel, ServerProviderUpdateRequiredModel},
};

const LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
pub fn resolve<'a>(catalog: &'a Catalog, value: &str) -> Option<&'a catalog::CatalogModel> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    catalog
        .models
        .iter()
        .find(|e| e.model.slug.0 == value)
        .or_else(|| {
            catalog.models.iter().find(|e| {
                e.model
                    .aliases
                    .iter()
                    .flatten()
                    .flatten()
                    .any(|a| a.0.eq_ignore_ascii_case(value))
            })
        })
}
pub fn resolve_slug(catalog: &Catalog, value: &str) -> String {
    resolve(catalog, value)
        .map(|e| e.model.slug.0.clone())
        .unwrap_or_else(|| value.into())
}
pub fn resolve_reported_slug(catalog: &Catalog, value: &str) -> String {
    let without_date = value
        .rsplit_once('-')
        .filter(|(_, date)| date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()))
        .map(|(slug, _)| slug)
        .unwrap_or(value);
    resolve(catalog, value)
        .or_else(|| resolve(catalog, without_date))
        .map(|e| e.model.slug.0.clone())
        .unwrap_or_else(|| value.into())
}
fn runtime(entry: &catalog::CatalogModel) -> &Value {
    entry
        .profile_adapter
        .as_ref()
        .and_then(|v| v.get("claudeCode"))
        .unwrap_or(&Value::Null)
}
fn selected(
    catalog: &Catalog,
    model: &str,
    options: &[t3_contracts::model::ProviderOptionSelection],
    option: &str,
) -> Option<String> {
    let caps = resolve(catalog, model)
        .and_then(|e| ultracode(e.model.capabilities.clone()))
        .unwrap_or_else(|| catalog::capabilities(vec![]));
    catalog::option_descriptors(&caps, options)
        .into_iter()
        .find_map(|d| match d {
            ProviderOptionDescriptor::Select(d) if d.id == option => d.current_value.flatten(),
            _ => None,
        })
}
pub fn resolve_effort(catalog: &Catalog, model: Option<&str>, raw: Option<&str>) -> Option<String> {
    let model = model.unwrap_or_default();
    if resolve(catalog, model).is_none() {
        return raw
            .map(str::trim)
            .filter(|s| LEVELS.contains(s))
            .map(str::to_owned);
    }
    let options = raw
        .filter(|s| !s.is_empty())
        .map(|s| {
            vec![t3_contracts::model::ProviderOptionSelection {
                id: "effort".into(),
                value: t3_contracts::model::ProviderOptionSelectionValue::String(s.into()),
            }]
        })
        .unwrap_or_default();
    selected(catalog, model, &options, "effort")
}
pub fn normalize_effort(catalog: &Catalog, model: &str, effort: Option<&str>) -> Option<String> {
    let effort = effort.filter(|s| !s.is_empty())?;
    let mapped = resolve(catalog, model)
        .map(runtime)
        .and_then(|v| v["effortMap"].get(effort));
    match mapped {
        Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        _ => Some(effort.into()),
    }
}
pub fn api_model_id(
    catalog: &Catalog,
    selection: &t3_contracts::model_selection::ModelSelection,
) -> String {
    let Some(entry) = resolve(catalog, &selection.model) else {
        return selection.model.clone();
    };
    if let Some(suffixes) = runtime(entry)["modelSuffixes"].as_object() {
        for (id, values) in suffixes {
            if let Some(value) = selected(
                catalog,
                &selection.model,
                selection
                    .options
                    .as_ref()
                    .map(|v| v.0.as_slice())
                    .unwrap_or_default(),
                id,
            )
            .and_then(|key| values[&key].as_str())
            .filter(|s| !s.is_empty())
            {
                return format!("{}{value}", entry.model.slug.0);
            }
        }
    }
    entry.model.slug.0.clone()
}
pub fn context_window_tokens(
    catalog: &Catalog,
    selection: Option<&t3_contracts::model_selection::ModelSelection>,
) -> Option<f64> {
    let selection = selection?;
    let entry = resolve(catalog, &selection.model)?;
    if let Some(fixed) = runtime(entry)["fixedContextWindowTokens"]
        .as_f64()
        .filter(|v| *v != 0.)
    {
        return Some(fixed);
    }
    let value = selected(
        catalog,
        &selection.model,
        selection
            .options
            .as_ref()
            .map(|v| v.0.as_slice())
            .unwrap_or_default(),
        "contextWindow",
    )?;
    runtime(entry)["contextWindowTokens"][&value].as_f64()
}
fn rank(level: &str) -> Option<usize> {
    LEVELS.iter().position(|s| *s == level)
}
pub fn ultracode(caps: Option<ModelCapabilities>) -> Option<ModelCapabilities> {
    let mut caps = caps?;
    let mut options = catalog::descriptors(&caps).to_vec();
    let Some(index)=options.iter().position(|d|matches!(d,ProviderOptionDescriptor::Select(d) if d.id=="effort" && d.options.iter().any(|o|o.id=="ultracode"))) else{return Some(caps);};
    let ProviderOptionDescriptor::Select(effort) = &mut options[index] else {
        return Some(caps);
    };
    let default = effort
        .current_value
        .as_ref()
        .and_then(Option::as_ref)
        .map(String::as_str)
        .or_else(|| {
            effort
                .options
                .iter()
                .find(|o| o.is_default == Some(Some(true)))
                .map(|o| o.id.as_str())
        })
        == Some("ultracode");
    let has_xhigh = effort.options.iter().any(|o| o.id == "xhigh");
    effort.options = effort
        .options
        .iter()
        .cloned()
        .filter_map(|mut o| {
            if o.id == "ultracode" {
                if has_xhigh {
                    return None;
                }
                o.id = "xhigh".into();
                o.label = "Extra High".into();
            }
            if default {
                o.is_default = (o.id == "xhigh").then_some(Some(true));
            }
            Some(o)
        })
        .collect();
    if effort.current_value.as_ref().and_then(Option::as_deref) == Some("ultracode") {
        effort.current_value = Some(Some("xhigh".into()));
    }
    options.insert(
        index + 1,
        catalog::boolean_option("ultracode", "Ultracode", default.then_some(true)),
    );
    caps.option_descriptors = Some(Some(options));
    Some(caps)
}
fn compatibility(entry: &catalog::CatalogModel) -> &Value {
    entry
        .adapter
        .as_ref()
        .and_then(|v| v.get("claudeCode"))
        .unwrap_or(&Value::Null)
}
pub fn models_for_version(catalog: &Catalog, version: Option<&str>) -> Vec<ServerProviderModel> {
    catalog
        .models
        .iter()
        .filter(|entry| {
            let bounds = compatibility(entry);
            let min = bounds["minVersion"].as_str();
            let max = bounds["maxVersionExclusive"].as_str();
            if min.is_none() && max.is_none() {
                return true;
            }
            version.is_some_and(|v| {
                min.is_none_or(|m| !compare_versions(v, m).is_lt())
                    && max.is_none_or(|m| compare_versions(v, m).is_lt())
            })
        })
        .map(|entry| {
            let mut model = entry.model.clone();
            model.capabilities = ultracode(model.capabilities);
            model
        })
        .collect()
}
pub fn update_required(
    catalog: &Catalog,
    version: Option<&str>,
) -> Vec<ServerProviderUpdateRequiredModel> {
    let Some(version) = version else {
        return vec![];
    };
    catalog
        .models
        .iter()
        .filter_map(|entry| {
            let min = compatibility(entry)["minVersion"].as_str()?;
            if entry.model.is_legacy == Some(Some(true)) || !compare_versions(version, min).is_lt()
            {
                return None;
            }
            Some(ServerProviderUpdateRequiredModel {
                slug: entry.model.slug.clone(),
                name: entry.model.name.clone(),
                badge: entry.model.badge,
                min_version: min.to_owned().into(),
            })
        })
        .collect()
}
fn normalized(value: &str) -> String {
    value
        .rsplit_once('[')
        .filter(|_| value.ends_with(']'))
        .map_or(value, |(base, _)| base)
        .trim()
        .to_lowercase()
}
pub fn reported(
    catalog: &Catalog,
    custom: &[Value],
    reported: &[Value],
) -> Vec<ServerProviderModel> {
    let custom_models = catalog::custom_models(&[], custom, None);
    let mut known: BTreeSet<_> = catalog
        .models
        .iter()
        .flat_map(|entry| {
            std::iter::once(&entry.model.slug).chain(entry.model.aliases.iter().flatten().flatten())
        })
        .map(|s| normalized(&s.0))
        .chain(custom_models.iter().map(|m| normalized(&m.slug.0)))
        .collect();
    let mut models = vec![];
    for info in reported {
        let slug = info["value"].as_str().unwrap_or_default().trim();
        if slug.is_empty() || slug == "default" {
            continue;
        }
        let ids: Vec<_> = [slug, info["resolvedModel"].as_str().unwrap_or_default()]
            .into_iter()
            .map(normalized)
            .filter(|s| !s.is_empty())
            .collect();
        if ids.iter().any(|id| known.contains(id)) {
            continue;
        }
        known.extend(ids);
        let efforts: Vec<_> = if info["supportsEffort"].as_bool() == Some(true) {
            info.get("supportedEffortLevels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect()
        } else {
            vec![]
        };
        let options = if efforts.is_empty() {
            vec![]
        } else {
            let choices: Vec<_> = std::iter::once(("default", "Default"))
                .chain(efforts.iter().map(|id| {
                    (
                        *id,
                        match *id {
                            "low" => "Low",
                            "medium" => "Medium",
                            "high" => "High",
                            "xhigh" => "Extra High",
                            "max" => "Max",
                            _ => id,
                        },
                    )
                }))
                .collect();
            let mut option =
                catalog::select_option("effort", "Reasoning", &choices, Some("default"));
            if let ProviderOptionDescriptor::Select(d) = &mut option {
                d.current_value = None;
            }
            vec![option]
        };
        let label = info["displayName"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(slug);
        models.push(catalog::model(slug, label, Some(capabilities(options))));
    }
    models
}
fn settings_key(catalog: &Catalog, key: &str) -> Option<String> {
    let lower = key.trim().to_lowercase();
    let bare = lower.strip_suffix("[1m]").unwrap_or(&lower);
    if ["opus", "sonnet", "haiku", "fable"].contains(&bare) {
        return catalog
            .models
            .iter()
            .find(|e| {
                !e.model.is_custom
                    && e.model
                        .aliases
                        .iter()
                        .flatten()
                        .flatten()
                        .any(|a| a.0 == bare)
            })
            .map(|e| e.model.slug.0.clone());
    }
    catalog
        .models
        .iter()
        .filter(|e| !e.model.is_custom && lower.contains(&e.model.slug.0))
        .max_by_key(|e| e.model.slug.0.len())
        .map(|e| e.model.slug.0.clone())
}
pub fn cap_effort(catalog: &Catalog, models: &mut [ServerProviderModel], settings: &Value) {
    let sources: Vec<_> = settings
        .get("sources")
        .and_then(Value::as_array)
        .map(|sources| sources.iter().filter_map(|s| s.get("settings")).collect())
        .unwrap_or_else(|| settings.get("effective").into_iter().collect());
    let sources: Vec<_> = sources
        .into_iter()
        .map(|source| {
            let mut per_model = BTreeMap::<String, usize>::new();
            for (key, value) in source
                .get("modelSettings")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if let (Some(key), Some(rank)) = (
                    settings_key(catalog, key),
                    value["maxEffortLevel"].as_str().and_then(rank),
                ) {
                    per_model
                        .entry(key)
                        .and_modify(|r| *r = (*r).min(rank))
                        .or_insert(rank);
                }
            }
            (source["maxEffortLevel"].as_str().and_then(rank), per_model)
        })
        .collect();
    for model in models {
        if model.is_custom {
            continue;
        }
        let cap = sources
            .iter()
            .filter_map(|(global, local)| local.get(&model.slug.0).copied().or(*global))
            .filter(|rank| *rank < 4)
            .min();
        let Some(cap) = cap else {
            continue;
        };
        let effort_map = catalog
            .models
            .iter()
            .find(|e| e.model.slug == model.slug)
            .and_then(|e| e.profile_adapter.as_ref())
            .and_then(|v| v.get("claudeCode"))
            .and_then(|v| v.get("effortMap"));
        let Some(options) = model
            .capabilities
            .as_mut()
            .and_then(|c| c.option_descriptors.as_mut())
            .and_then(Option::as_mut)
        else {
            continue;
        };
        for descriptor in options {
            if let ProviderOptionDescriptor::Select(d) = descriptor {
                if d.id != "effort" {
                    continue;
                }
                let limit = d
                    .options
                    .iter()
                    .filter_map(|o| Some((rank(&o.id)?, o.label.clone())))
                    .filter(|(r, _)| *r <= cap)
                    .max_by_key(|(r, _)| *r);
                if let Some((limit, label)) = limit {
                    for option in &mut d.options {
                        let shown = rank(&option.id);
                        let sent = effort_map
                            .and_then(|v| v.get(&option.id))
                            .and_then(Value::as_str)
                            .and_then(rank);
                        if shown.or(sent).is_some_and(|r| r > limit)
                            && sent.or(shown).is_some_and(|r| r > limit)
                        {
                            option.label = format!("{} (capped to {label})", option.label);
                        }
                    }
                }
            }
        }
    }
}
