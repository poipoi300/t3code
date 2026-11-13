use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderOptionDescriptorType {
    #[serde(rename = "select")]
    Select,
    #[serde(rename = "boolean")]
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectProviderOptionType {
    #[serde(rename = "select")]
    Select,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BooleanProviderOptionType {
    #[serde(rename = "boolean")]
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderOptionRole {
    #[serde(rename = "reasoning")]
    Reasoning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOptionChoice {
    pub id: String,
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_default: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectProviderOptionDescriptor {
    pub id: String,
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<String>>,
    pub r#type: SelectProviderOptionType,
    pub options: Vec<ProviderOptionChoice>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub current_value: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub prompt_injected_values: Option<Option<Vec<String>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub role: Option<Option<ProviderOptionRole>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BooleanProviderOptionDescriptor {
    pub id: String,
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<String>>,
    pub r#type: BooleanProviderOptionType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub current_value: Option<Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProviderOptionDescriptor {
    Select(SelectProviderOptionDescriptor),
    Boolean(BooleanProviderOptionDescriptor),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProviderOptionSelectionValue {
    String(String),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOptionSelection {
    pub id: String,
    pub value: ProviderOptionSelectionValue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProviderOptionSelections(pub Vec<ProviderOptionSelection>);

impl<'de> Deserialize<'de> for ProviderOptionSelections {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Selections;
        impl<'de> serde::de::Visitor<'de> for Selections {
            type Value = ProviderOptionSelections;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an array of selections or a legacy options object")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut selections = Vec::new();
                while let Some(selection) = seq.next_element()? {
                    selections.push(selection);
                }
                Ok(ProviderOptionSelections(selections))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::<(String, serde_json::Value)>::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                entries.sort_by_key(|(key, _)| {
                    key.parse::<u32>()
                        .ok()
                        .filter(|index| *index != u32::MAX && index.to_string() == *key)
                        .map_or((1, 0), |index| (0, index))
                });
                let selections = entries
                    .into_iter()
                    .filter_map(|(key, value)| {
                        let id = crate::wire::trim_js(&key).to_owned();
                        if id.is_empty() {
                            return None;
                        }
                        let value = match value {
                            serde_json::Value::String(value) => {
                                let value = crate::wire::trim_js(&value).to_owned();
                                if value.is_empty() {
                                    return None;
                                }
                                ProviderOptionSelectionValue::String(value)
                            }
                            serde_json::Value::Bool(value) => {
                                ProviderOptionSelectionValue::Boolean(value)
                            }
                            _ => return None,
                        };
                        Some(ProviderOptionSelection { id, value })
                    })
                    .collect();
                Ok(ProviderOptionSelections(selections))
            }
        }
        deserializer.deserialize_any(Selections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_options_preserve_js_order_and_normalize_to_array() -> Result<(), serde_json::Error> {
        let selections: ProviderOptionSelections = serde_json::from_str(
            r#"{"z":" yes ","10":false,"2":true,"a":false," ":true,"blank":"  ","invalid":1}"#,
        )?;
        assert_eq!(
            serde_json::to_value(selections)?,
            json!([
                {"id": "2", "value": true}, {"id": "10", "value": false},
                {"id": "z", "value": "yes"}, {"id": "a", "value": false}
            ])
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub option_descriptors: Option<Option<Vec<ProviderOptionDescriptor>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomModelEntry {
    pub slug: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub name: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub capabilities: Option<Option<ModelCapabilities>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomModelSetting {
    String(String),
    Entry(CustomModelEntry),
}

pub const DEFAULT_MODEL: &str = "gpt-6-astra";
pub const PREFERRED_DEFAULT_CODEX_MODELS: &[&str] =
    &[DEFAULT_MODEL, "gpt-5.6-sol", "gpt-5.6-terra"];
pub const DEFAULT_TEXT_GENERATION_MODEL: &str = "gpt-6-luna";
pub const ANTIGRAVITY_DEFAULT_MODEL: &str = "antigravity-default";
pub const DEFAULT_TEXT_GENERATION_REASONING_EFFORT: &str = "low";
pub const MUSE_DEFAULT_MODEL: &str = "default";
pub const DEFAULT_MODEL_BY_PROVIDER: &[(&str, &str)] = &[
    ("codex", DEFAULT_MODEL),
    ("claudeAgent", "claude-opus-5-5"),
    ("cursor", "auto"),
    ("grok", "grok-build"),
    ("fx", "default"),
    ("kiro", "default"),
    ("muse", MUSE_DEFAULT_MODEL),
    ("acpRegistry", "default"),
    ("pi", "default"),
    ("opencode", "openai/gpt-5"),
    ("antigravity", ANTIGRAVITY_DEFAULT_MODEL),
];
pub const DEFAULT_TEXT_GENERATION_MODEL_BY_PROVIDER: &[(&str, &str)] = &[
    ("codex", DEFAULT_TEXT_GENERATION_MODEL),
    ("antigravity", ANTIGRAVITY_DEFAULT_MODEL),
    ("claudeAgent", "claude-haiku-5-5"),
    ("cursor", "composer-2"),
    ("opencode", "opencode/big-pickle"),
];
pub const MODEL_SLUG_ALIASES_BY_PROVIDER: &[(&str, &[(&str, &str)])] = &[
    (
        "codex",
        &[
            ("gpt-5-codex", "gpt-5.4"),
            ("5.4", "gpt-5.4"),
            ("5.3", "gpt-5.3-codex"),
            ("gpt-5.3", "gpt-5.3-codex"),
            ("5.3-spark", "gpt-5.3-codex-spark"),
            ("gpt-5.3-spark", "gpt-5.3-codex-spark"),
        ],
    ),
    ("claudeAgent", &[]),
    (
        "cursor",
        &[
            ("composer", "composer-2"),
            ("composer-1.5", "composer-1.5"),
            ("composer-1", "composer-1.5"),
            ("opus-4.6-thinking", "claude-opus-4-6"),
            ("opus-4.6", "claude-opus-4-6"),
            ("sonnet-4.6-thinking", "claude-sonnet-4-6"),
            ("sonnet-4.6", "claude-sonnet-4-6"),
            ("opus-4.5-thinking", "claude-opus-4-5"),
            ("opus-4.5", "claude-opus-4-5"),
        ],
    ),
    ("opencode", &[]),
];
pub const PROVIDER_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("antigravity", "Antigravity"),
    ("codex", "Codex"),
    ("claudeAgent", "Claude"),
    ("cursor", "Cursor"),
    ("grok", "Grok"),
    ("fx", "fx"),
    ("kiro", "Kiro"),
    ("muse", "Muse Code"),
    ("acpRegistry", "ACP Registry"),
    ("pi", "Pi"),
    ("opencode", "OpenCode"),
];
