use crate::chat_attachment::{ChatFileAttachment, ChatImageAttachment};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderApprovalPolicy {
    #[serde(rename = "untrusted")]
    Untrusted,
    #[serde(rename = "on-failure")]
    OnFailure,
    #[serde(rename = "on-request")]
    OnRequest,
    #[serde(rename = "never")]
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderSandboxMode {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "workspace-write")]
    WorkspaceWrite,
    #[serde(rename = "danger-full-access")]
    DangerFullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeMode {
    #[serde(rename = "approval-required")]
    ApprovalRequired,
    #[serde(rename = "auto-accept-edits")]
    AutoAcceptEdits,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "full-access")]
    FullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderInteractionMode {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "plan")]
    Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderRequestKind {
    #[serde(rename = "command")]
    Command,
    #[serde(rename = "file-read")]
    FileRead,
    #[serde(rename = "file-change")]
    FileChange,
    #[serde(rename = "mcp-elicitation")]
    McpElicitation,
    #[serde(rename = "permission")]
    Permission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderApprovalDecision {
    #[serde(rename = "accept")]
    Accept,
    #[serde(rename = "acceptForSession")]
    AcceptForSession,
    #[serde(rename = "acceptAlways")]
    AcceptAlways,
    #[serde(rename = "decline")]
    Decline,
    #[serde(rename = "cancel")]
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderApprovalFullAccessOverride {
    #[serde(rename = "ask_rule")]
    AskRule,
    #[serde(rename = "provider_check")]
    ProviderCheck,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderApprovalOption {
    pub decision: ProviderApprovalDecision,
    pub label: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub warning: Option<Option<String>>,
}

pub type ProviderUserInputAnswers = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserInputAttachment {
    Image(Box<ChatImageAttachment>),
    File(ChatFileAttachment),
}

pub type UserInputAttachments = BTreeMap<String, Vec<UserInputAttachment>>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputAttachmentAnswerPayload {
    pub request_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub question_text_by_id: Option<Option<BTreeMap<String, String>>>,
    pub answers: ProviderUserInputAnswers,
    pub attachments_by_question_id: UserInputAttachments,
}

pub const DEFAULT_RUNTIME_MODE: RuntimeMode = RuntimeMode::FullAccess;
pub const DEFAULT_PROVIDER_INTERACTION_MODE: ProviderInteractionMode =
    ProviderInteractionMode::Default;
