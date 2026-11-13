use crate::{
    McpError, Result,
    catalog::{DecodeDefinition, catalog, tool},
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{collections::HashMap, sync::OnceLock};

#[derive(Clone, Debug)]
pub struct DecodedInput(Value);
impl DecodedInput {
    pub(crate) fn from_normalized(value: Value) -> Self {
        Self(value)
    }
    pub fn value(&self) -> &Value {
        &self.0
    }
    pub fn into_value(self) -> Value {
        self.0
    }
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.0.clone())
            .map_err(|error| McpError::InvalidInput(error.to_string()))
    }
}

struct Validators {
    encoded: jsonschema::Validator,
    decoded: jsonschema::Validator,
}

fn validators() -> &'static HashMap<String, Validators> {
    static VALIDATORS: OnceLock<HashMap<String, Validators>> = OnceLock::new();
    VALIDATORS.get_or_init(|| {
        catalog()
            .definitions
            .iter()
            .map(|(name, definition)| {
                (
                    name.clone(),
                    Validators {
                        encoded: jsonschema::validator_for(&definition.input_schema)
                            .expect("generated decoder schema must compile"),
                        decoded: jsonschema::validator_for(&definition.decoded_input_schema)
                            .expect("generated decoded schema must compile"),
                    },
                )
            })
            .collect()
    })
}

pub fn decode_input(name: &str, mut value: Value) -> Result<DecodedInput> {
    let tool =
        tool(name).ok_or_else(|| McpError::InvalidInput(format!("Tool '{name}' not found")))?;
    if value.is_null() {
        value = serde_json::json!({});
    }
    let properties = tool.input_schema["properties"].as_object();
    let input = value
        .as_object()
        .ok_or_else(|| McpError::InvalidInput("Expected an object".into()))?;
    let unknown = input
        .keys()
        .filter(|key| properties.is_none_or(|p| !p.contains_key(*key)))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        let names = unknown
            .iter()
            .take(5)
            .map(|name| serde_json::to_string(&cut_utf16(name, 64)).expect("string encodes"))
            .collect::<Vec<_>>()
            .join(", ");
        let more = if unknown.len() > 5 {
            format!(" and {} more", unknown.len() - 5)
        } else {
            String::new()
        };
        let accepted = properties
            .map(|p| p.keys().cloned().collect::<Vec<_>>().join(", "))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "none".into());
        return Err(McpError::InvalidInput(format!(
            "Unknown argument{} {names}{more}. Accepted arguments: {accepted}.",
            if unknown.len() == 1 { "" } else { "s" }
        )));
    }
    let definition = &catalog().definitions[name];
    validators()[name]
        .encoded
        .validate(&value)
        .map_err(|error| McpError::InvalidInput(error.to_string()))?;
    if name == "preview_act" {
        let action = value["action"]
            .as_str()
            .ok_or_else(|| McpError::InvalidInput("Expected action".into()))?;
        let schema = catalog()
            .action_schemas
            .get(action)
            .ok_or_else(|| McpError::InvalidInput("Unknown preview action".into()))?;
        jsonschema::validator_for(schema)
            .map_err(|_| McpError::Internal)?
            .validate(&value)
            .map_err(|error| McpError::failure("invalid_request", error.to_string()))?;
    }
    transform(&mut value, definition, "$")?;
    normalize(
        &mut value,
        &definition.decoded_input_schema,
        &definition.null_paths,
        "$",
    );
    // These are decoding transforms, deliberately absent from the advertised schema.
    if name == "t3_thread_read" {
        for (key, max) in [
            ("limit", 100),
            ("runLimit", 50),
            ("maxCharsPerItem", 20_000),
        ] {
            if let Some(number) = value[key].as_u64() {
                value[key] = number.min(max).into();
            }
        }
    }
    validators()[name]
        .decoded
        .validate(&value)
        .map_err(|error| McpError::InvalidInput(error.to_string()))?;
    validate_string_lengths(&value, &definition.decoded_input_schema)?;
    if name == "t3_thread_update" && (value["action"] == "rename") != value.get("title").is_some() {
        return Err(McpError::InvalidInput(
            "rename requires title; regenerate_title does not accept one.".into(),
        ));
    }
    if name == "request_secret" {
        if value.get("value").and_then(Value::as_str) == Some("") {
            return Err(McpError::InvalidInput("Expected a non-empty string".into()));
        }
        if let Some(url) = value.get("url").and_then(Value::as_str)
            && !reqwest::Url::parse(url).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            })
        {
            return Err(McpError::InvalidInput("Expected an http(s) URL".into()));
        }
    }
    Ok(DecodedInput(value))
}

fn transform(value: &mut Value, definition: &DecodeDefinition, path: &str) -> Result<()> {
    if definition.json_paths.iter().any(|p| p == path)
        && let Some(text) = value.as_str()
    {
        *value = serde_json::from_str(text)
            .map_err(|error| McpError::InvalidInput(error.to_string()))?;
    }
    if definition.options_paths.iter().any(|p| p == path)
        && let Some(options) = value.as_object()
    {
        *value = Value::Array(
            options
                .iter()
                .map(|(id, value)| serde_json::json!({"id":id,"value":value}))
                .collect(),
        );
    }
    if definition.trim_paths.iter().any(|p| p == path)
        && let Some(text) = value.as_str()
    {
        let text = t3_contracts::wire::trim_js(text);
        if text.is_empty() && definition.non_blank_paths.iter().any(|p| p == path) {
            return Err(McpError::InvalidInput("Expected a non-blank string".into()));
        }
        *value = Value::String(text.into());
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                transform(value, definition, &format!("{path}.{key}"))?;
            }
        }
        Value::Array(array) => {
            for value in array {
                transform(value, definition, &format!("{path}[]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_string_lengths(value: &Value, schema: &Value) -> Result<()> {
    if let Some(branches) = schema["anyOf"].as_array() {
        if let Some(branch) = branches
            .iter()
            .find(|branch| jsonschema::is_valid(branch, value))
        {
            return validate_string_lengths(value, branch);
        }
    } else if let Some(text) = value.as_str() {
        let length = text.encode_utf16().count() as u64;
        if schema["maxLength"]
            .as_u64()
            .is_some_and(|maximum| length > maximum)
        {
            return Err(McpError::InvalidInput(
                "String exceeds maximum UTF-16 length".into(),
            ));
        }
    } else if let Some(object) = value.as_object() {
        for (key, value) in object {
            if let Some(schema) = schema["properties"].get(key) {
                validate_string_lengths(value, schema)?;
            }
        }
    } else if let Some(array) = value.as_array()
        && let Some(items) = schema.get("items")
    {
        for value in array {
            validate_string_lengths(value, items)?;
        }
    }
    Ok(())
}

fn normalize(value: &mut Value, schema: &Value, null_paths: &[String], path: &str) {
    if let Some(branches) = schema.get("anyOf").and_then(Value::as_array)
        && let Some(branch) = branches
            .iter()
            .find(|branch| jsonschema::is_valid(branch, value))
    {
        normalize(value, branch, null_paths, path);
        return;
    }
    if let (Some(object), Some(properties)) =
        (value.as_object_mut(), schema["properties"].as_object())
    {
        let required = schema["required"].as_array();
        object.retain(|name, value| {
            properties.contains_key(name)
                && !(value.is_null()
                    && !null_paths.contains(&format!("{path}.{name}"))
                    && required.is_none_or(|r| !r.iter().any(|key| key == name)))
        });
        for (key, value) in object {
            normalize(
                value,
                &properties[key],
                null_paths,
                &format!("{path}.{key}"),
            );
        }
    } else if let (Some(array), Some(items)) = (value.as_array_mut(), schema.get("items")) {
        for item in array {
            normalize(item, items, null_paths, &format!("{path}[]"));
        }
    }
}

pub(crate) fn cut_utf16(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.into();
    }
    let mut units = text.encode_utf16().take(max).collect::<Vec<_>>();
    if units
        .last()
        .is_some_and(|last| (0xd800..=0xdbff).contains(last))
    {
        units.pop();
    }
    String::from_utf16_lossy(&units) + "…"
}
