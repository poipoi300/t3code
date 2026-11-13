use serde::{Deserialize, Serialize};

crate::string_id!(SecretProviderId);
pub type SecretProviderSlug = String;
pub type SecretReferencePath = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretReference {
    pub provider: SecretProviderId,
    pub reference: SecretReferencePath,
}
