use crate::{
    acp_coordinator::{AvailableCommands, LiveConfiguration},
    sensitive::Sensitive,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use t3_contracts::{
    acp_registry::{AcpRegistryProbeAuthMethod, AcpRegistryProbeModel},
    model::ProviderOptionDescriptor,
    wire::trim_js,
};
pub const EMPTY_CHOICE: &str = "_t3/empty";
pub const SESSION_MODE: &str = "_t3/session-mode";
fn text(value: &Value, maximum: usize) -> String {
    String::from_utf16_lossy(
        &trim_js(value.as_str().unwrap_or_default())
            .encode_utf16()
            .take(maximum)
            .collect::<Vec<_>>(),
    )
}
fn opaque(value: &Value, maximum: usize) -> Option<&str> {
    value.as_str().filter(|value| {
        !value.is_empty() && trim_js(value) == *value && value.encode_utf16().count() <= maximum
    })
}
fn choice(value: &Value) -> Option<&str> {
    if value.as_str() == Some("") {
        Some(EMPTY_CHOICE)
    } else {
        opaque(value, 256)
    }
}
fn uri(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            output.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}
pub fn web_url(value: &str) -> Option<String> {
    if value.encode_utf16().count() > 2048 {
        return None;
    }
    url::Url::parse(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(Into::into)
}
fn shell_token(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
pub fn auth_methods(
    methods: &[Value],
    spawn: Option<(&str, &[String])>,
) -> Sensitive<Vec<AcpRegistryProbeAuthMethod>> {
    let mut result = vec![];
    for method in methods {
        let Some(id) = opaque(&method["id"], 128) else {
            continue;
        };
        let kind = method["type"].as_str().unwrap_or("agent");
        if !matches!(kind, "agent" | "env_var" | "terminal") {
            continue;
        }
        let name = text(&method["name"], 160);
        let description = text(&method["description"], 1024);
        let mut normalized = json!({"id":id,"name":if name.is_empty(){id}else{&name},"description":if description.is_empty(){None}else{Some(description)},"type":kind});
        if kind == "env_var" {
            let names: Vec<_> = method["vars"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|variable| opaque(&variable["name"], 128))
                .take(16)
                .collect();
            if !names.is_empty() {
                normalized["envVarNames"] = json!(names);
            }
            if let Some(url) = method["link"].as_str().and_then(web_url) {
                normalized["link"] = json!(url);
            }
        }
        if kind == "terminal"
            && let Some((command, args)) = spawn
        {
            let mut parts = method["env"]
                .as_object()
                .into_iter()
                .flat_map(|env| env.iter())
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| format!("{name}={}", shell_token(value)))
                })
                .collect::<Vec<_>>();
            parts.push(shell_token(command));
            parts.extend(args.iter().map(|arg| shell_token(arg)));
            parts.extend(
                method["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(shell_token),
            );
            let command = parts.join(" ");
            if command.encode_utf16().count() <= 2048 {
                normalized["command"] = json!(command);
            }
        }
        if let Ok(method) = serde_json::from_value(normalized) {
            result.push(method);
        }
        if result.len() == 32 {
            break;
        }
    }
    Sensitive::new(result)
}
pub fn commands(commands: &[Value]) -> AvailableCommands {
    let mut result = AvailableCommands {
        slash_commands: vec![],
        skills: vec![],
    };
    let mut seen = BTreeSet::new();
    for command in commands {
        let Some(name) = opaque(&command["name"], 128) else {
            continue;
        };
        if !seen.insert(name.to_lowercase()) {
            continue;
        }
        let description = text(&command["description"], 1024);
        if let Some(skill) = name.strip_prefix('$') {
            if skill.is_empty() || trim_js(skill) != skill {
                continue;
            }
            let mut value = json!({"name":skill,"path":format!("acp://skill/{}",uri(skill)),"scope":"agent","enabled":true});
            if !description.is_empty() {
                value["description"] = json!(description);
            }
            if let Ok(skill) = serde_json::from_value(value) {
                result.skills.push(skill);
            }
        } else {
            let mut value = json!({"name":name});
            if !description.is_empty() {
                value["description"] = json!(description);
            }
            let hint = text(&command["input"]["hint"], 1024);
            if !hint.is_empty() {
                value["input"] = json!({"hint":hint});
            }
            if let Ok(command) = serde_json::from_value(value) {
                result.slash_commands.push(command);
            }
        }
    }
    result
}
fn flatten(options: &Value) -> Vec<Value> {
    options
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
        .collect()
}
fn choices(candidates: &[Value]) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    candidates
        .iter()
        .filter_map(|candidate| {
            let id = choice(&candidate["value"])?;
            if !seen.insert(id.to_owned()) {
                return None;
            }
            let label = text(&candidate["name"], 256);
            let mut value = json!({"id":id,"label":if label.is_empty(){id}else{&label}});
            let description = text(&candidate["description"], 1024);
            if !description.is_empty() {
                value["description"] = json!(description);
            }
            Some(value)
        })
        .take(64)
        .collect()
}
pub fn descriptors(options: &[Value], mode: Option<&Value>) -> Vec<ProviderOptionDescriptor> {
    let mut descriptors = vec![];
    let mut seen = BTreeSet::new();
    let mut mirrored = vec![];
    let mut has_mode = false;
    for option in options {
        if matches!(
            option["category"].as_str(),
            Some("model" | "collaboration_mode")
        ) {
            continue;
        }
        let Some(id) = opaque(&option["id"], 256) else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let label = text(&option["name"], 256);
        let mut value =
            json!({"id":id,"label":if label.is_empty(){id}else{&label},"type":option["type"]});
        let description = text(&option["description"], 1024);
        if !description.is_empty() {
            value["description"] = json!(description);
        }
        if option["type"] == "boolean" {
            value["currentValue"] = option["currentValue"].clone();
        } else {
            let choices = choices(&flatten(&option["options"]));
            if choices.is_empty() {
                seen.remove(id);
                continue;
            }
            if option["category"] == "thought_level" {
                mirrored.push(
                    choices
                        .iter()
                        .filter_map(|choice| choice["id"].as_str().map(str::to_owned))
                        .collect::<BTreeSet<_>>(),
                );
                value["role"] = json!("reasoning");
            }
            if let Some(current) = choice(&option["currentValue"])
                && choices.iter().any(|choice| choice["id"] == current)
            {
                value["currentValue"] = json!(current);
            }
            value["options"] = json!(choices);
        }
        if option["category"] == "mode" {
            has_mode = true;
        }
        if let Ok(descriptor) = serde_json::from_value(value) {
            descriptors.push(descriptor);
        }
        if descriptors.len() == 16 {
            return descriptors;
        }
    }
    if !has_mode
        && !seen.contains(SESSION_MODE)
        && let Some(mode) = mode
    {
        let modes=mode["availableModes"].as_array().into_iter().flatten().map(|mode|json!({"value":mode["id"],"name":mode["name"],"description":mode["description"]})).collect::<Vec<_>>();
        let choices = choices(&modes);
        let ids = choices
            .iter()
            .filter_map(|choice| choice["id"].as_str().map(str::to_owned))
            .collect::<BTreeSet<_>>();
        if choices.len() > 1 && !mirrored.contains(&ids) {
            let mut value = json!({"id":SESSION_MODE,"label":"Mode","description":"Session mode advertised by the ACP agent.","type":"select","options":choices});
            if let Some(current) = choice(&mode["currentModeId"])
                && ids.contains(current)
            {
                value["currentValue"] = json!(current);
            }
            if let Ok(descriptor) = serde_json::from_value(value) {
                descriptors.push(descriptor);
            }
        }
    }
    descriptors
}
pub fn live_configuration(setup: &Value) -> LiveConfiguration {
    let options = setup["configOptions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let model = options.iter().find(|option| {
        option["category"] == "model"
            && option["type"] == "select"
            && option["id"]
                .as_str()
                .is_some_and(|id| !trim_js(id).is_empty())
    });
    let current = model
        .map(|model| &model["currentValue"])
        .unwrap_or(&setup["models"]["currentModelId"]);
    let current = opaque(current, 128);
    let candidates = if let Some(model) = model {
        flatten(&model["options"]).into_iter().map(|model|json!({"id":model["value"],"name":model["name"],"description":model["description"]})).collect::<Vec<_>>()
    } else {
        setup["models"]["availableModels"].as_array().into_iter().flatten().map(|model|json!({"id":model["modelId"],"name":model["name"],"description":model["description"]})).collect()
    };
    let normalized = |candidate: &Value, id: &str| {
        let name = text(&candidate["name"], 160);
        let description = text(&candidate["description"], 1024);
        AcpRegistryProbeModel {
            id: id.into(),
            name: if name.is_empty() {
                id.into()
            } else {
                name.into()
            },
            description: (!description.is_empty()).then_some(description),
        }
    };
    let mut seen = BTreeSet::new();
    let mut models = vec![];
    for candidate in &candidates {
        if models.len() == 256 {
            break;
        }
        if let Some(id) = opaque(&candidate["id"], 128)
            && seen.insert(id.to_owned())
        {
            models.push(normalized(candidate, id));
        }
    }
    if models.len() == 256
        && let Some(current) = current.filter(|current| !seen.contains(*current))
        && let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate["id"] == current)
    {
        models[255] = normalized(candidate, current);
    }
    let current = current
        .filter(|current| models.iter().any(|model| model.id.0 == *current))
        .map(str::to_owned);
    let mut mode = setup.get("modes").filter(|mode| mode.is_object()).cloned();
    if mode.is_none()
        && let Some(config) = options
            .iter()
            .find(|option| option["category"] == "mode" && option["type"] == "select")
    {
        mode = Some(
            json!({"currentModeId":config["currentValue"],"availableModes":flatten(&config["options"]).iter().map(|option|json!({"id":option["value"],"name":option["name"],"description":option["description"]})).collect::<Vec<_>>()}),
        );
    }
    let mode = mode.and_then(|mode| {
        let current = trim_js(mode["currentModeId"].as_str()?);
        if current.is_empty() {
            return None;
        }
        let available = mode["availableModes"]
            .as_array()?
            .iter()
            .filter_map(|mode| {
                let id = trim_js(mode["id"].as_str()?);
                let name = trim_js(mode["name"].as_str()?);
                if id.is_empty() || name.is_empty() {
                    return None;
                }
                let mut value = json!({"id":id,"name":name});
                if let Some(description) = mode["description"]
                    .as_str()
                    .map(trim_js)
                    .filter(|description| !description.is_empty())
                {
                    value["description"] = json!(description);
                }
                Some(value)
            })
            .collect::<Vec<_>>();
        if available.is_empty() {
            None
        } else {
            Some(json!({"currentModeId":current,"availableModes":available}))
        }
    });
    let config_options = descriptors(&options, mode.as_ref());
    let model_ids = options
        .iter()
        .filter(|option| {
            matches!(
                option["category"].as_str(),
                Some("thought_level" | "model_config")
            )
        })
        .filter_map(|option| option["id"].as_str())
        .collect::<BTreeSet<_>>();
    let model_option_ids = Some(
        config_options
            .iter()
            .filter_map(|descriptor| {
                serde_json::to_value(descriptor)
                    .ok()?
                    .get("id")?
                    .as_str()
                    .filter(|id| model_ids.contains(*id))
                    .map(str::to_owned)
            })
            .collect(),
    );
    LiveConfiguration {
        models,
        current_model_id: current,
        config_options,
        model_option_ids,
    }
}
