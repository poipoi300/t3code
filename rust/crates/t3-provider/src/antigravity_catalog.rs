use crate::catalog;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    model::ProviderOptionDescriptor,
    server::{ServerProviderModel, ServerProviderSlashCommand},
    wire::trim_js,
};
pub type ThinkingLevels = BTreeMap<String, Option<Value>>;
pub fn thinking(options: &[Value]) -> Option<Value> {
    options
        .iter()
        .find(|option| option["category"] == "thought_level")
        .filter(|option| option["type"] == "select")
        .cloned()
}
pub fn observe(options: &[Value], levels: &mut ThinkingLevels) {
    if let Some(model) = options
        .iter()
        .find(|option| option["id"] == "model" || option["category"] == "model")
        .filter(|option| option["type"] == "select")
        && let Some(current) = model["currentValue"]
            .as_str()
            .filter(|current| !current.is_empty())
    {
        levels.insert(current.into(), thinking(options));
    }
}
pub fn models(setup: &Value, levels: &ThinkingLevels) -> Vec<ServerProviderModel> {
    let options = setup["configOptions"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let model = options
        .iter()
        .find(|option| option["id"] == "model" || option["category"] == "model");
    let current = if model.is_some_and(|option| option["type"] == "select") {
        model.and_then(|model| model["currentValue"].as_str())
    } else {
        setup["models"]["currentModelId"].as_str()
    };
    let entries = if let Some(model) = model {
        if model["type"] == "select" {
            model["options"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|option| {
                    if option.get("value").is_some() {
                        vec![option.clone()]
                    } else {
                        option["options"].as_array().cloned().unwrap_or_default()
                    }
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        }
    } else {
        setup["models"]["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|model| json!({"value":model["modelId"],"name":model["name"]}))
            .collect()
    };
    let current_thinking = thinking(options);
    let (mut seen, mut names) = (BTreeSet::new(), BTreeSet::new());
    let mut models = vec![];
    for entry in entries {
        let Some(id) = entry["value"]
            .as_str()
            .filter(|id| !trim_js(id).is_empty() && seen.insert((*id).to_owned()))
        else {
            continue;
        };
        let name = entry["name"]
            .as_str()
            .filter(|name| !trim_js(name).is_empty())
            .unwrap_or(id);
        let duplicate = !names.insert(name.to_owned());
        let name = if duplicate {
            format!("{name} ({id})")
        } else {
            name.into()
        };
        let observed = if current == Some(id) {
            Some(current_thinking.as_ref())
        } else {
            levels.get(id).map(Option::as_ref)
        };
        let config = observed.unwrap_or(current_thinking.as_ref());
        let mut descriptors = config
            .map(|config| crate::acp_catalog::descriptors(std::slice::from_ref(config), None))
            .unwrap_or_default();
        if observed.is_none() {
            for descriptor in &mut descriptors {
                if let ProviderOptionDescriptor::Select(descriptor) = descriptor {
                    descriptor.current_value = None;
                }
            }
        }
        let mut model = catalog::model(id, &name, Some(catalog::capabilities(descriptors)));
        if current == Some(id) {
            model.is_default = Some(Some(true));
            model.aliases = Some(Some(vec![
                t3_contracts::model::ANTIGRAVITY_DEFAULT_MODEL.into(),
            ]));
        }
        models.push(model);
    }
    models
}
pub fn commands(commands: &[Value]) -> Vec<ServerProviderSlashCommand> {
    let mut seen = BTreeSet::new();
    commands
        .iter()
        .filter_map(|command| {
            let name = command["name"]
                .as_str()
                .filter(|name| !trim_js(name).is_empty() && seen.insert(*name))?;
            let mut value = json!({"name":name});
            if let Some(description) = command["description"]
                .as_str()
                .map(trim_js)
                .filter(|description| !description.is_empty())
            {
                value["description"] = json!(description);
            }
            if let Some(hint) = command["input"]["hint"]
                .as_str()
                .map(trim_js)
                .filter(|hint| !hint.is_empty())
            {
                value["input"] = json!({"hint":hint});
            }
            serde_json::from_value(value).ok()
        })
        .collect()
}
