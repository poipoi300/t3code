use crate::base_schemas::TrimmedNonEmptyString;
use crate::browser_profile::BrowserProfileId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserImportSourceId {
    #[serde(rename = "chrome")]
    Chrome,
    #[serde(rename = "edge")]
    Edge,
    #[serde(rename = "brave")]
    Brave,
    #[serde(rename = "vivaldi")]
    Vivaldi,
    #[serde(rename = "opera")]
    Opera,
    #[serde(rename = "arc")]
    Arc,
    #[serde(rename = "helium")]
    Helium,
    #[serde(rename = "firefox")]
    Firefox,
    #[serde(rename = "zen")]
    Zen,
    #[serde(rename = "safari")]
    Safari,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserImportUnavailableReason {
    #[serde(rename = "notInstalled")]
    NotInstalled,
    #[serde(rename = "needsKeychainApproval")]
    NeedsKeychainApproval,
    #[serde(rename = "keychainItemMissing")]
    KeychainItemMissing,
    #[serde(rename = "needsFullDiskAccess")]
    NeedsFullDiskAccess,
    #[serde(rename = "browserRunning")]
    BrowserRunning,
    #[serde(rename = "unsupportedPlatform")]
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserImportFailureReason {
    #[serde(rename = "notInstalled")]
    NotInstalled,
    #[serde(rename = "needsKeychainApproval")]
    NeedsKeychainApproval,
    #[serde(rename = "keychainItemMissing")]
    KeychainItemMissing,
    #[serde(rename = "needsFullDiskAccess")]
    NeedsFullDiskAccess,
    #[serde(rename = "browserRunning")]
    BrowserRunning,
    #[serde(rename = "unsupportedPlatform")]
    UnsupportedPlatform,
    #[serde(rename = "keychainUnavailable")]
    KeychainUnavailable,
    #[serde(rename = "unknownSource")]
    UnknownSource,
    #[serde(rename = "unknownSourceProfile")]
    UnknownSourceProfile,
    #[serde(rename = "sessionUnavailable")]
    SessionUnavailable,
    #[serde(rename = "profileNotSaved")]
    ProfileNotSaved,
    #[serde(rename = "profileLimitReached")]
    ProfileLimitReached,
    #[serde(rename = "readFailed")]
    ReadFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserImportSourceProfile {
    pub directory: TrimmedNonEmptyString,
    pub name: TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cookie_count: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserImportSource {
    pub id: BrowserImportSourceId,
    pub name: TrimmedNonEmptyString,
    pub profiles: Vec<BrowserImportSourceProfile>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub unavailable: Option<Option<BrowserImportUnavailableReason>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserImportInput {
    pub source_id: BrowserImportSourceId,
    pub source_profile_directory: TrimmedNonEmptyString,
    pub target_profile_id: BrowserProfileId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreviewImportCookiesInputSchema {
    pub environment_id: TrimmedNonEmptyString,
    pub source_id: BrowserImportSourceId,
    pub source_profile_directory: TrimmedNonEmptyString,
    pub target_profile_id: BrowserProfileId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserImportResult {
    pub imported: i64,
    pub skipped: i64,
    pub skipped_domains: Vec<String>,
}
