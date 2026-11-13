pub use crate::wire::{ForwardCompatible, TaggedUnion, UnknownUnionMember};
use serde::{Deserialize, Serialize};
pub type IsoDateTime = crate::wire::WireDateTime;
pub type NonNegativeInt = u64;
pub type PositiveInt = u64;
pub type PortSchema = u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RpcClientId(pub NonNegativeInt);

crate::string_id!(
    ThreadId,
    ProjectId,
    EnvironmentId,
    CommandId,
    EventId,
    MessageId,
    TurnId,
    RunId,
    RunAttemptId,
    NodeId,
    GuestShareId,
    GuestMessageId,
    AuthSessionId,
    ProviderItemId,
    ProviderSessionId,
    ProviderThreadId,
    ProviderTurnId,
    TurnItemId,
    RuntimeRequestId,
    ScheduledTaskId,
    SecretRef,
    ApprovalRequestId,
    CheckpointRef,
    CheckpointId,
    CheckpointScopeId,
    ContextHandoffId,
    ContextTransferId,
    RawEventId,
    PlanId
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DpopFailureReason {
    #[serde(rename = "time_window")]
    TimeWindow,
    #[serde(rename = "key_mismatch")]
    KeyMismatch,
    #[serde(rename = "request_mismatch")]
    RequestMismatch,
    #[serde(rename = "token_mismatch")]
    TokenMismatch,
    #[serde(rename = "replay")]
    Replay,
    #[serde(rename = "invalid_proof")]
    InvalidProof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientSurface {
    #[serde(rename = "web")]
    Web,
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "mobile")]
    Mobile,
    #[serde(rename = "cli")]
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientOs {
    #[serde(rename = "macOS")]
    MacOS,
    #[serde(rename = "Windows")]
    Windows,
    #[serde(rename = "Linux")]
    Linux,
    #[serde(rename = "iOS")]
    IOS,
    #[serde(rename = "Android")]
    Android,
    #[serde(rename = "ChromeOS")]
    ChromeOS,
    #[serde(rename = "other")]
    Other,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientDeviceType {
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "phone")]
    Phone,
    #[serde(rename = "tablet")]
    Tablet,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientWebDeployment {
    #[serde(rename = "hosted")]
    Hosted,
    #[serde(rename = "server")]
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientConnectionMethod {
    #[serde(rename = "direct")]
    Direct,
    #[serde(rename = "ssh")]
    Ssh,
    #[serde(rename = "relay")]
    Relay,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrimmedString(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrimmedNonEmptyString(pub String);

macro_rules! trimmed_string {
    ($name:ident, $nonempty:expr) => {
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let value = crate::wire::trim_js(&self.0);
                if $nonempty && value.is_empty() {
                    return Err(serde::ser::Error::custom("expected a non-blank string"));
                }
                serializer.serialize_str(value)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                let value = crate::wire::trim_js(&raw);
                if $nonempty && value.is_empty() {
                    return Err(serde::de::Error::custom("expected a non-blank string"));
                }
                Ok(Self(value.to_owned()))
            }
        }
        impl $name {
            /// Trims like the TypeScript schema and rejects what it rejects.
            pub fn checked(value: impl AsRef<str>) -> Result<Self, String> {
                let value = crate::wire::trim_js(value.as_ref());
                if $nonempty && value.is_empty() {
                    return Err("expected a non-blank string".to_owned());
                }
                Ok(Self(value.to_owned()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        // Unchecked, like the id constructors: decoding is where input is validated.
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
trimmed_string!(TrimmedString, false);
trimmed_string!(TrimmedNonEmptyString, true);
