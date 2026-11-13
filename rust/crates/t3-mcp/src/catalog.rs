use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::OnceLock};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    pub annotations: Value,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeDefinition {
    pub toolkit: String,
    pub failure_mode: String,
    pub routed: bool,
    pub null_paths: Vec<String>,
    pub trim_paths: Vec<String>,
    pub non_blank_paths: Vec<String>,
    pub options_paths: Vec<String>,
    pub json_paths: Vec<String>,
    pub decoded_input_schema: Value,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub server_version: String,
    pub tools: Vec<ToolDefinition>,
    pub modes: HashMap<String, Value>,
    pub legacy_modes: HashMap<String, Value>,
    pub definitions: HashMap<String, DecodeDefinition>,
    pub action_schemas: HashMap<String, Value>,
    pub capability_messages: HashMap<String, String>,
    pub error_tags: HashMap<String, Vec<String>>,
}

pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../fixtures/tools.json"))
            .expect("generated MCP catalog must be valid")
    })
}

pub fn tool(name: &str) -> Option<&'static ToolDefinition> {
    catalog().tools.iter().find(|tool| tool.name == name)
}

pub fn advertised(name: &str) -> Option<ToolDefinition> {
    let mut tool = tool(name)?.clone();
    tool.title = tool
        .annotations
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(tool)
}
