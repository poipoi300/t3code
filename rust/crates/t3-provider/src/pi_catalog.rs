use crate::{
    catalog::{self, capabilities, select_option},
    sensitive::{ServiceError, ServiceResult},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use t3_contracts::{
    model::{ModelCapabilities, ProviderOptionDescriptor},
    server::{ServerProviderModel, ServerProviderSkill, ServerProviderSlashCommand},
    wire::trim_js,
};
const THINKING: &[(&str, &str)] = &[
    ("off", "Off"),
    ("minimal", "Minimal"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra High"),
    ("max", "Max"),
];
fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    string(value, key)
        .map(trim_js)
        .filter(|value| !value.is_empty())
}
pub fn model_capabilities(model: &Value, thinking: Option<&str>) -> ModelCapabilities {
    let mut descriptors = vec![];
    if model["reasoning"] == true {
        let map = model
            .get("thinkingLevelMap")
            .filter(|value| value.is_object());
        let levels = THINKING
            .iter()
            .copied()
            .filter(|(level, _)| match map.and_then(|map| map.get(*level)) {
                Some(Value::Null) => false,
                Some(_) => true,
                None => !matches!(*level, "xhigh" | "max"),
            })
            .collect::<Vec<_>>();
        if !levels.is_empty() {
            let selected = thinking
                .and_then(|thinking| THINKING.iter().position(|(level, _)| *level == thinking))
                .and_then(|requested| {
                    levels
                        .iter()
                        .find(|(level, _)| Some(*level) == thinking)
                        .or_else(|| {
                            THINKING[requested + 1..].iter().find(|(level, _)| {
                                levels.iter().any(|(available, _)| available == level)
                            })
                        })
                        .or_else(|| {
                            THINKING[..requested].iter().rev().find(|(level, _)| {
                                levels.iter().any(|(available, _)| available == level)
                            })
                        })
                        .map(|(level, _)| *level)
                });
            let mut descriptor = select_option("thinking", "Thinking", &levels, selected);
            if let ProviderOptionDescriptor::Select(descriptor) = &mut descriptor {
                descriptor.current_value = None;
            }
            descriptors.push(descriptor);
        }
    }
    let mut seen = BTreeSet::from(["default"]);
    let mut choices = vec![];
    for tier in model["serviceTiers"].as_array().into_iter().flatten() {
        let Some(id) = string(tier, "id").filter(|id| !trim_js(id).is_empty() && seen.insert(id))
        else {
            continue;
        };
        let mut choice = json!({"id":id,"label":string(tier,"name").filter(|name|!trim_js(name).is_empty()).unwrap_or(id)});
        if let Some(description) =
            string(tier, "description").filter(|value| !trim_js(value).is_empty())
        {
            choice["description"] = json!(description);
        }
        choices.push(choice);
    }
    if !choices.is_empty() {
        choices.insert(
            0,
            json!({"id":"default","label":"Standard","isDefault":true}),
        );
        if let Ok(descriptor) = serde_json::from_value(
            json!({"id":"serviceTier","label":"Service Tier","type":"select","options":choices,"currentValue":"default"}),
        ) {
            descriptors.push(descriptor);
        }
    }
    capabilities(descriptors)
}
pub fn models(data: &Value, state: &Value) -> ServiceResult<Vec<ServerProviderModel>> {
    let entries = data["models"].as_array().ok_or(ServiceError::Response)?;
    let mut seen = BTreeSet::new();
    let mut models = vec![];
    for entry in entries {
        let provider = string(entry, "provider")
            .filter(|value| !trim_js(value).is_empty())
            .ok_or(ServiceError::Response)?;
        let id = string(entry, "id")
            .filter(|value| !trim_js(value).is_empty())
            .ok_or(ServiceError::Response)?;
        let slug = format!("{provider}/{id}");
        if !seen.insert(slug.clone()) {
            continue;
        }
        let thinking = (string(&state["model"], "provider") == Some(provider)
            && string(&state["model"], "id") == Some(id))
        .then(|| string(state, "thinkingLevel"))
        .flatten();
        let mut model = catalog::model(
            &slug,
            string(entry, "name").unwrap_or(&slug),
            Some(model_capabilities(entry, thinking)),
        );
        model.sub_provider = Some(Some(provider.into()));
        models.push(model);
    }
    Ok(models)
}
pub fn resolve_model_slug(slug: &str, reported: &Value) -> Option<(String, String)> {
    for model in reported.as_array().into_iter().flatten() {
        if let (Some(provider), Some(id)) = (string(model, "provider"), string(model, "id"))
            && format!("{provider}/{id}") == slug
        {
            return Some((provider.into(), id.into()));
        }
    }
    slug.split_once('/')
        .filter(|(provider, id)| !provider.is_empty() && !id.is_empty())
        .map(|(provider, id)| (provider.into(), id.into()))
}
pub fn compact_command(input: &str) -> Option<Option<String>> {
    let rest = trim_js(input).strip_prefix("/compact")?;
    if !rest.is_empty()
        && rest.chars().next().is_some_and(|character| {
            !t3_contracts::wire::trim_js(&character.to_string()).is_empty()
        })
    {
        return None;
    }
    Some((!trim_js(rest).is_empty()).then(|| trim_js(rest).into()))
}
pub fn builtin_commands(
    mut commands: Vec<ServerProviderSlashCommand>,
) -> Vec<ServerProviderSlashCommand> {
    commands.retain(|command| command.name.0 != "compact");
    commands.insert(0,serde_json::from_value(json!({"name":"compact","description":"Summarize the conversation and reduce context usage","input":{"hint":"Optional instructions"}})).expect("fixed Pi command"));
    commands
}
pub fn commands(data: &Value) -> (Vec<ServerProviderSlashCommand>, Vec<ServerProviderSkill>) {
    let (mut commands, mut skills) = (vec![], vec![]);
    for command in data["commands"].as_array().into_iter().flatten() {
        let Some(name) = text(command, "name") else {
            continue;
        };
        if text(command, "source") == Some("skill") {
            let name = name.strip_prefix("skill:").unwrap_or(name);
            if name.is_empty() {
                continue;
            }
            let source = &command["sourceInfo"];
            let interface = &command["interface"];
            let path = text(source, "path")
                .or_else(|| text(command, "path"))
                .map(str::to_owned)
                .unwrap_or_else(|| format!("pi:skill:{name}"));
            let mut skill = json!({"name":name,"path":path,"enabled":true});
            if let Some(scope) = text(source, "scope").or_else(|| text(command, "location")) {
                skill["scope"] = json!(match scope.to_lowercase().as_str() {
                    "global" | "personal" => "user",
                    "workspace" | "local" => "project",
                    _ => scope,
                });
            }
            for key in ["description", "displayName", "shortDescription"] {
                let value = if key == "description" {
                    text(command, key)
                } else {
                    text(command, key)
                        .or_else(|| text(source, key))
                        .or_else(|| text(interface, key))
                };
                if let Some(value) = value {
                    skill[key] = json!(value);
                }
            }
            if let Ok(skill) = serde_json::from_value(skill) {
                skills.push(skill);
            }
        } else {
            let mut value = json!({"name":name});
            if let Some(description) = text(command, "description") {
                value["description"] = json!(description);
            }
            if let Ok(command) = serde_json::from_value(value) {
                commands.push(command);
            }
        }
    }
    (commands, skills)
}
