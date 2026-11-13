use crate::base_schemas::{ProjectId, ThreadId, TrimmedNonEmptyString};
use serde::{Deserialize, Serialize};
pub const DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopAppActivationPlatform {
    #[serde(rename = "darwin")]
    Darwin,
    #[serde(rename = "linux")]
    Linux,
    #[serde(rename = "win32")]
    Win32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopAppActivationErrorCode {
    #[serde(rename = "invalid-request")]
    InvalidRequest,
    #[serde(rename = "renderer-unavailable")]
    RendererUnavailable,
    #[serde(rename = "environment-unavailable")]
    EnvironmentUnavailable,
    #[serde(rename = "platform-mismatch")]
    PlatformMismatch,
    #[serde(rename = "project-create-failed")]
    ProjectCreateFailed,
    #[serde(rename = "thread-open-failed")]
    ThreadOpenFailed,
    #[serde(rename = "request-timeout")]
    RequestTimeout,
    #[serde(rename = "internal-error")]
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct DesktopActivationVersion;
impl TryFrom<u8> for DesktopActivationVersion {
    type Error = &'static str;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("unsupported activation version")
        }
    }
}
impl From<DesktopActivationVersion> for u8 {
    fn from(_: DesktopActivationVersion) -> Self {
        1
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "bool", into = "bool")]
pub struct ActivationOk<const OK: bool>;
impl<const OK: bool> TryFrom<bool> for ActivationOk<OK> {
    type Error = &'static str;
    fn try_from(value: bool) -> Result<Self, Self::Error> {
        if value == OK {
            Ok(Self)
        } else {
            Err("incorrect activation response discriminator")
        }
    }
}
impl<const OK: bool> From<ActivationOk<OK>> for bool {
    fn from(_: ActivationOk<OK>) -> Self {
        OK
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopActivationRequestType {
    #[serde(rename = "open-workspace")]
    OpenWorkspace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppActivationRequest {
    pub version: DesktopActivationVersion,
    pub request_id: TrimmedNonEmptyString,
    #[serde(rename = "type")]
    pub request_type: DesktopActivationRequestType,
    pub workspace_root: TrimmedNonEmptyString,
    pub platform: DesktopAppActivationPlatform,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppActivationSuccess {
    pub version: DesktopActivationVersion,
    pub request_id: TrimmedNonEmptyString,
    pub ok: ActivationOk<true>,
    pub project_id: ProjectId,
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppActivationFailure {
    pub version: DesktopActivationVersion,
    pub request_id: TrimmedNonEmptyString,
    pub ok: ActivationOk<false>,
    pub code: DesktopAppActivationErrorCode,
    pub message: TrimmedNonEmptyString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopAppActivationResponse {
    Success(DesktopAppActivationSuccess),
    Failure(DesktopAppActivationFailure),
}
