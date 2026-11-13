use crate::secret_reference::SecretReference;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

crate::string_id!(ProviderDriverKind, ProviderInstanceId);
pub type ProviderInstanceEnvironmentVariableName = String;

#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceEnvironmentVariable {
    pub name: ProviderInstanceEnvironmentVariableName,
    pub value: String,
    pub sensitive: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub value_redacted: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub secret: Option<SecretReference>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEnvironmentVariableFields {
    pub name: ProviderInstanceEnvironmentVariableName,
    #[serde(default, deserialize_with = "crate::wire::default_on_null")]
    pub value: String,
    #[serde(default, deserialize_with = "crate::wire::default_on_null")]
    pub sensitive: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub value_redacted: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub secret: Option<SecretReference>,
}

impl<'de> Deserialize<'de> for ProviderInstanceEnvironmentVariable {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = StoredEnvironmentVariableFields::deserialize(deserializer)?;
        Ok(Self {
            name: fields.name,
            value: if fields.secret.is_some() {
                String::new()
            } else {
                fields.value
            },
            sensitive: fields.sensitive || fields.secret.is_some(),
            value_redacted: fields.value_redacted,
            secret: fields.secret,
        })
    }
}

// This is the stored wire envelope; secret resolution is a server responsibility.
pub type ProviderInstanceEnvironment = Vec<ProviderInstanceEnvironmentVariable>;
pub type ResolvedProviderInstanceEnvironment = Vec<ResolvedProviderInstanceEnvironmentVariable>;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedProviderInstanceEnvironmentVariable {
    pub name: ProviderInstanceEnvironmentVariableName,
    pub value: String,
    pub sensitive: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub value_redacted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstanceConfig {
    pub driver: ProviderDriverKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub display_name: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub accent_color: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub environment: Option<ProviderInstanceEnvironment>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::optional"
    )]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "camelCase")]
pub enum ProviderInstanceMutation {
    Create {
        #[serde(rename = "instanceId")]
        instance_id: ProviderInstanceId,
        instance: ProviderInstanceConfig,
    },
    Upsert {
        #[serde(rename = "instanceId")]
        instance_id: ProviderInstanceId,
        instance: ProviderInstanceConfig,
    },
    Remove {
        #[serde(rename = "instanceId")]
        instance_id: ProviderInstanceId,
    },
}

pub type ProviderInstanceConfigMap = BTreeMap<ProviderInstanceId, ProviderInstanceConfig>;

pub fn default_instance_id_for_driver(driver: &ProviderDriverKind) -> ProviderInstanceId {
    ProviderInstanceId(driver.0.clone())
}

/// Values print only when neither `sensitive` nor backed by a secret reference.
struct EnvironmentValue<'a>(&'a str, bool);
impl std::fmt::Debug for EnvironmentValue<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.1 {
            f.write_str("[REDACTED]")
        } else {
            std::fmt::Debug::fmt(self.0, f)
        }
    }
}

impl std::fmt::Debug for ProviderInstanceEnvironmentVariable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderInstanceEnvironmentVariable")
            .field("name", &self.name)
            .field(
                "value",
                &EnvironmentValue(&self.value, self.sensitive || self.secret.is_some()),
            )
            .field("sensitive", &self.sensitive)
            .field("value_redacted", &self.value_redacted)
            .field("secret", &self.secret)
            .finish()
    }
}

impl std::fmt::Debug for ResolvedProviderInstanceEnvironmentVariable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedProviderInstanceEnvironmentVariable")
            .field("name", &self.name)
            .field("value", &EnvironmentValue(&self.value, self.sensitive))
            .field("sensitive", &self.sensitive)
            .field("value_redacted", &self.value_redacted)
            .finish()
    }
}
