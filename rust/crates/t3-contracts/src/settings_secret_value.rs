use crate::secret_reference::SecretReference;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingsSecretValue {
    Literal(String),
    Reference(SecretReference),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum SecretSourceCheckResult {
    Resolved,
    Failed { detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingsSecretUnresolvedErrorTag {
    #[serde(rename = "SettingsSecretUnresolvedError")]
    SettingsSecretUnresolvedError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSecretUnresolvedError {
    #[serde(rename = "_tag")]
    pub _tag: SettingsSecretUnresolvedErrorTag,
    pub provider: String,
}

impl std::fmt::Debug for SettingsSecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Literal(_) => f.write_str("Literal([REDACTED])"),
            Self::Reference(reference) => f.debug_tuple("Reference").field(reference).finish(),
        }
    }
}
