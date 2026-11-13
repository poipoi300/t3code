use serde::{Deserialize, Serialize};

pub type BrowserProfileId = String;
pub type BrowserProfileName = String;
pub const BROWSER_PROFILE_NAME_MAX_LENGTH: usize = 48;
pub const BROWSER_PROFILE_MAX_COUNT: usize = 24;
pub const DEFAULT_BROWSER_PROFILE_ID: &str = "default";
pub const INCOGNITO_BROWSER_PROFILE_ID: &str = "incognito";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserProfileKind {
    #[serde(rename = "persistent")]
    Persistent,
    #[serde(rename = "incognito")]
    Incognito,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfile {
    pub id: BrowserProfileId,
    pub name: BrowserProfileName,
    pub kind: BrowserProfileKind,
}
