use crate::{
    catalog::{self, capabilities, model, select_option},
    sensitive::{ServiceError, ServiceResult},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    model::ModelCapabilities,
    server::{ServerProviderModel, ServerProviderSkill, ServerProviderSlashCommand},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeModel {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    pub id: String,
    pub name: String,
    pub variants: Vec<Variant>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub provider_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub family: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub released: Option<t3_contracts::wire::JsNumber>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::optional"
    )]
    pub deprecated: Option<bool>,
}
fn title(value: &str) -> String {
    value
        .split(['-', '_', '/'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn default_variant<'a>(provider: &str, variants: &'a [String]) -> Option<&'a str> {
    if variants.len() == 1 {
        return variants.first().map(String::as_str);
    }
    let preferred = if provider == "anthropic" || provider.starts_with("google") {
        &["high"][..]
    } else if matches!(provider, "openai" | "opencode" | "opencode-go") {
        &["medium", "high"][..]
    } else {
        &[]
    };
    preferred.iter().find_map(|preferred| {
        variants
            .iter()
            .find(|variant| variant == preferred)
            .map(String::as_str)
    })
}
pub fn fallback_capabilities() -> ModelCapabilities {
    capabilities(vec![
        select_option(
            "variant",
            "Reasoning",
            &[
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("xhigh", "Extra High"),
            ],
            Some("medium"),
        ),
        select_option(
            "agent",
            "Agent",
            &[("build", "Build"), ("plan", "Plan")],
            Some("build"),
        ),
    ])
}
fn model_capabilities(
    provider: &str,
    variants: &[String],
    agents: Option<&[Value]>,
) -> ModelCapabilities {
    let mut descriptors = vec![];
    let variants_labels: Vec<_> = variants
        .iter()
        .map(|variant| (variant.as_str(), title(variant)))
        .collect();
    let variants_options: Vec<_> = variants_labels
        .iter()
        .map(|(id, label)| (*id, label.as_str()))
        .collect();
    if !variants_options.is_empty() {
        descriptors.push(select_option(
            "variant",
            "Reasoning",
            &variants_options,
            default_variant(provider, variants),
        ));
    }
    if let Some(agents) = agents {
        let primary: Vec<_> = agents
            .iter()
            .filter(|agent| {
                agent["hidden"] != true && matches!(agent["mode"].as_str(), Some("primary" | "all"))
            })
            .filter_map(|agent| agent["name"].as_str())
            .collect();
        let default = primary
            .iter()
            .find(|name| **name == "build")
            .or_else(|| primary.first())
            .copied();
        let labels: Vec<_> = primary.iter().map(|name| (*name, title(name))).collect();
        let options: Vec<_> = labels
            .iter()
            .map(|(name, label)| (*name, label.as_str()))
            .collect();
        if !options.is_empty() {
            descriptors.push(select_option("agent", "Agent", &options, default));
        }
    }
    capabilities(descriptors)
}
fn decorate(
    entries: Vec<(ServerProviderModel, OpenCodeModel)>,
    custom: &[Value],
) -> Vec<ServerProviderModel> {
    let mut newest = BTreeMap::<(&str, &str), f64>::new();
    for (_, release) in &entries {
        if release.deprecated == Some(true) {
            continue;
        }
        if let Some(family) = release
            .family
            .as_deref()
            .filter(|family| !family.is_empty())
            && let Some(at) = release.released.as_ref().map(|at| at.value())
        {
            newest
                .entry((&release.provider_id, family))
                .and_modify(|current| *current = current.max(at))
                .or_insert(at);
        }
    }
    let mut models: Vec<_> = entries
        .iter()
        .map(|(model, release)| {
            let mut model = model.clone();
            let superseded = release
                .family
                .as_deref()
                .and_then(|family| newest.get(&(release.provider_id.as_str(), family)))
                .zip(release.released.as_ref())
                .is_some_and(|(latest, released)| released.value() < *latest);
            if release.deprecated == Some(true) || superseded {
                model.is_legacy = Some(Some(true));
            }
            model
        })
        .collect();
    models.sort_by(|left, right| left.name.cmp(&right.name));
    catalog::custom_models(&models, custom, Some(&fallback_capabilities()))
}
pub fn v2_models(catalog: &[OpenCodeModel], custom: &[Value]) -> Vec<ServerProviderModel> {
    decorate(
        catalog
            .iter()
            .map(|entry| {
                let variants: Vec<_> = entry
                    .variants
                    .iter()
                    .map(|variant| variant.id.clone())
                    .collect();
                let mut model = model(
                    &format!("{}/{}", entry.provider_id, entry.id),
                    &entry.name,
                    Some(model_capabilities(&entry.provider_id, &variants, None)),
                );
                let label = entry
                    .provider_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .unwrap_or(&entry.provider_id);
                if !label.eq_ignore_ascii_case("OpenCode") {
                    model.sub_provider = Some(Some(label.to_owned().into()));
                }
                (model, entry.clone())
            })
            .collect(),
        custom,
    )
}
pub fn v1_models(
    providers: &Value,
    agents: &[Value],
    custom: &[Value],
) -> ServiceResult<Option<Vec<ServerProviderModel>>> {
    let connected: BTreeSet<_> = providers["connected"]
        .as_array()
        .ok_or(ServiceError::Response)?
        .iter()
        .map(|id| id.as_str().ok_or(ServiceError::Response))
        .collect::<ServiceResult<_>>()?;
    let mut entries = vec![];
    for provider in providers["all"].as_array().ok_or(ServiceError::Response)? {
        let id = provider["id"].as_str().ok_or(ServiceError::Response)?;
        if !connected.contains(id) {
            continue;
        }
        for value in provider["models"]
            .as_object()
            .ok_or(ServiceError::Response)?
            .values()
        {
            let Some(name) = value["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let native_id = value["id"].as_str().ok_or(ServiceError::Response)?;
            let variants: Vec<_> = value["variants"]
                .as_object()
                .map(|variants| variants.keys().cloned().collect())
                .unwrap_or_default();
            let mut model = model(
                &format!("{id}/{native_id}"),
                name,
                Some(model_capabilities(id, &variants, Some(agents))),
            );
            if let Some(name) = provider["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                model.sub_provider = Some(Some(name.to_owned().into()));
            }
            let release = OpenCodeModel {
                provider_id: id.into(),
                id: native_id.into(),
                name: name.into(),
                variants: vec![],
                provider_name: None,
                family: value["family"].as_str().map(str::to_owned),
                released: value["release_date"]
                    .as_str()
                    .and_then(catalog::date_millis)
                    .and_then(|time| t3_contracts::wire::JsNumber::from_f64(time as f64)),
                deprecated: Some(value["status"] == "deprecated"),
            };
            entries.push((model, release));
        }
    }
    Ok((!entries.is_empty()).then(|| decorate(entries, custom)))
}
pub fn skills(values: &[Value], v2: bool) -> ServiceResult<Vec<ServerProviderSkill>> {
    let mut skills: Vec<ServerProviderSkill> = vec![];
    for skill in values {
        let name = skill[if v2 { "id" } else { "name" }]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let path = skill[if v2 { "path" } else { "location" }]
            .as_str()
            .map(str::trim)
            .filter(|path| !path.is_empty());
        let (Some(name), Some(path)) = (name, path) else {
            continue;
        };
        let mut value = json!({"name":name,"path":path,"enabled":true});
        if v2 && let Some(display) = skill["name"].as_str().filter(|display| *display != name) {
            value["displayName"] = json!(display);
        }
        if let Some(description) = skill["description"]
            .as_str()
            .map(str::trim)
            .filter(|description| !description.is_empty())
        {
            value["description"] = json!(description);
            value["shortDescription"] = json!(description);
        }
        skills.push(
            serde_json::from_value(value).map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?,
        );
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}
pub fn commands(values: &[Value], v2: bool) -> ServiceResult<Vec<ServerProviderSlashCommand>> {
    let mut commands = vec![serde_json::from_value(json!({"name":"compact","description":"Summarize the conversation and reduce context usage"})).map_err(crate::diagnostics::mapped(concat!(module_path!(), "::error"), ServiceError::Response))?];
    let mut seen = BTreeSet::from(["compact"]);
    for command in values {
        let Some(name) = command["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        if !v2 && command["source"] == "skill" || !seen.insert(name) {
            continue;
        }
        let mut value = json!({"name":name});
        if let Some(description) = command["description"]
            .as_str()
            .map(str::trim)
            .filter(|description| !description.is_empty())
        {
            value["description"] = json!(description);
        }
        if !v2 && let Some(hints) = command["hints"].as_array() {
            let hint = hints
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_owned();
            if !hint.is_empty() {
                value["input"] = json!({"hint":hint});
            }
        }
        commands.push(
            serde_json::from_value(value).map_err(crate::diagnostics::mapped(
                concat!(module_path!(), "::error"),
                ServiceError::Response,
            ))?,
        );
    }
    Ok(commands)
}
