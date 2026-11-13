use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayClientSource {
    #[serde(rename = "override")]
    Override,
    #[serde(rename = "managed")]
    Managed,
    #[serde(rename = "path")]
    Path,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all_fields = "camelCase")]
pub enum RelayClientStatus {
    #[serde(rename = "available")]
    Available {
        executable_path: String,
        source: RelayClientSource,
        version: String,
    },
    #[serde(rename = "missing")]
    Missing { version: String },
    #[serde(rename = "unsupported")]
    Unsupported {
        platform: String,
        arch: String,
        version: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayClientInstallProgressStage {
    #[serde(rename = "checking")]
    Checking,
    #[serde(rename = "waiting_for_lock")]
    WaitingForLock,
    #[serde(rename = "downloading")]
    Downloading,
    #[serde(rename = "verifying")]
    Verifying,
    #[serde(rename = "installing")]
    Installing,
    #[serde(rename = "validating")]
    Validating,
    #[serde(rename = "activating")]
    Activating,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum RelayClientInstallProgressEvent {
    #[serde(rename = "progress")]
    Progress {
        stage: RelayClientInstallProgressStage,
    },
    #[serde(rename = "complete")]
    Complete { status: RelayClientStatus },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayClientInstallFailureReason {
    #[serde(rename = "download_failed")]
    DownloadFailed,
    #[serde(rename = "invalid_checksum")]
    InvalidChecksum,
    #[serde(rename = "install_locked")]
    InstallLocked,
    #[serde(rename = "managed_by_policy")]
    ManagedByPolicy,
    #[serde(rename = "override_missing")]
    OverrideMissing,
    #[serde(rename = "unsupported_platform")]
    UnsupportedPlatform,
    #[serde(rename = "validation_failed")]
    ValidationFailed,
    #[serde(rename = "write_failed")]
    WriteFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum RelayClientInstallFailedError {
    #[serde(rename = "RelayClientInstallFailedError")]
    RelayClientInstallFailedError {
        reason: RelayClientInstallFailureReason,
        message: String,
    },
}
pub type RelayClientStatusSchema = RelayClientStatus;
pub type RelayClientInstallProgressStageSchema = RelayClientInstallProgressStage;
pub type RelayClientInstallProgressEventSchema = RelayClientInstallProgressEvent;
pub type RelayClientInstallFailureReasonSchema = RelayClientInstallFailureReason;
