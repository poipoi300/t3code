use crate::message::{Content, Extensions};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "bypassPermissions")]
    BypassPermissions,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "dontAsk")]
    DontAsk,
    #[serde(rename = "auto")]
    Auto,
}
impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::BypassPermissions => "bypassPermissions",
            Self::Plan => "plan",
            Self::DontAsk => "dontAsk",
            Self::Auto => "auto",
        }
    }
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Initialize {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<BTreeMap<String, Vec<HookMatcher>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_mcp_servers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_mcp_server_configs: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_suggestions: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_dialog_kinds: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookMatcher {
    pub hook_callback_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "subtype")]
pub enum OutboundRequest {
    #[serde(rename = "initialize")]
    Initialize(Box<Initialize>),
    #[serde(rename = "mcp_set_servers")]
    McpSetServers { servers: BTreeMap<String, Value> },
    #[serde(rename = "set_model")]
    SetModel { model: String },
    #[serde(rename = "apply_flag_settings")]
    ApplyFlagSettings { settings: FlagSettings },
    #[serde(rename = "set_permission_mode")]
    SetPermissionMode { mode: PermissionMode },
    #[serde(rename = "stop_task")]
    StopTask { task_id: String },
    #[serde(rename = "background_tasks")]
    BackgroundTasks { tool_use_id: String },
    #[serde(rename = "mcp_status")]
    McpStatus,
    #[serde(rename = "mcp_read_resource")]
    McpReadResource {
        #[serde(rename = "serverName")]
        server_name: String,
        uri: String,
    },
    #[serde(rename = "mcp_call")]
    McpCall {
        tool: String,
        arguments: Extensions,
        result_not_for_model: bool,
    },
    #[serde(rename = "list_permission_rules")]
    ListPermissionRules,
    #[serde(rename = "interrupt")]
    Interrupt,
    #[serde(rename = "get_usage")]
    GetUsage {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        skip_behaviors: bool,
    },
    #[serde(rename = "get_settings")]
    GetSettings,
    #[serde(rename = "mcp_message")]
    McpMessage { server_name: String, message: Value },
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlagSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_level: Option<String>,
    pub fast_mode: Option<bool>,
    pub ultracode: Option<bool>,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct InitializeResult {
    #[serde(default)]
    pub commands: Vec<Value>,
    #[serde(default)]
    pub agents: Vec<Value>,
    #[serde(default)]
    pub models: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_style: Option<String>,
    #[serde(default)]
    pub available_output_styles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks_applied: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_on_failure_supported: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_applied: Option<bool>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct McpSetServersResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub errors: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct McpStatus {
    #[serde(rename = "mcpServers")]
    pub servers: Vec<McpServerStatus>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub status: String,
    #[serde(rename = "serverInfo")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct McpResource {
    pub contents: Vec<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct PermissionRules {
    pub state: PermissionRuleState,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct PermissionRuleState {
    pub rules: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<Value>>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct UsageResult {
    pub rate_limits_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct SettingsResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<Value>>,
    #[serde(flatten)]
    pub extra: Extensions,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct BackgroundTasksResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backgrounded: Option<bool>,
    #[serde(flatten)]
    pub extra: Extensions,
}
impl BackgroundTasksResult {
    pub fn was_backgrounded(&self) -> bool {
        self.backgrounded.unwrap_or(true)
    }
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct InterruptResult {
    #[serde(default)]
    pub still_queued: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extensions,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolPermission {
    pub tool_name: String,
    pub input: Extensions,
    pub tool_use_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_suggestions: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_server: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_to_no: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress_always_allow_rule: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_ask_rule: Option<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct HookCallback {
    pub callback_id: String,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Elicitation {
    pub mcp_server_name: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elicitation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property_order: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct UserDialog {
    pub dialog_kind: String,
    pub payload: Extensions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct McpMessage {
    pub server_name: String,
    pub message: Value,
    #[serde(flatten)]
    pub extra: Extensions,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "lowercase")]
pub enum PermissionDecision {
    Allow {
        #[serde(rename = "updatedInput")]
        updated_input: Extensions,
        #[serde(
            rename = "updatedPermissions",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        updated_permissions: Option<Vec<PermissionUpdate>>,
        #[serde(
            rename = "decisionClassification",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        decision_classification: Option<String>,
    },
    Deny {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interrupt: Option<bool>,
        #[serde(
            rename = "decisionClassification",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        decision_classification: Option<String>,
    },
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PermissionUpdate {
    AddRules {
        rules: Vec<PermissionRule>,
        behavior: RuleBehavior,
        destination: String,
    },
    ReplaceRules {
        rules: Vec<PermissionRule>,
        behavior: RuleBehavior,
        destination: String,
    },
    RemoveRules {
        rules: Vec<PermissionRule>,
        behavior: RuleBehavior,
        destination: String,
    },
    SetMode {
        mode: PermissionMode,
        destination: String,
    },
    AddDirectories {
        directories: Vec<String>,
        destination: String,
    },
    RemoveDirectories {
        directories: Vec<String>,
        destination: String,
    },
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleBehavior {
    Allow,
    Deny,
    Ask,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookOutput {
    #[serde(rename = "continue", default, skip_serializing_if = "Option::is_none")]
    pub continue_: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ElicitationValue {
    String(String),
    Boolean(bool),
    Number(f64),
    Strings(Vec<String>),
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum ElicitationResponse {
    Accept {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<BTreeMap<String, ElicitationValue>>,
    },
    Decline,
    Cancel,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "lowercase")]
pub enum DialogResponse {
    Completed { result: ResumeChoice },
    Cancelled,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResumeChoice {
    Compact,
    Continue,
    Never,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UserMessage {
    #[serde(rename = "type")]
    kind: UserKind,
    pub message: UserBody,
    pub parent_tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum UserKind {
    #[serde(rename = "user")]
    User,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct UserBody {
    pub role: String,
    pub content: Content,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum Priority {
    #[serde(rename = "now")]
    Now,
}
impl UserMessage {
    pub fn new(content: Content) -> Self {
        Self {
            kind: UserKind::User,
            message: UserBody {
                role: "user".into(),
                content,
            },
            parent_tool_use_id: None,
            uuid: None,
            priority: None,
            extra: Extensions::new(),
        }
    }
}

pub fn image_block(
    media_type: &str,
    bytes: &[u8],
) -> Result<crate::message::ContentBlock, crate::Error> {
    if !matches!(
        media_type,
        "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    ) {
        return Err(crate::Error::Options("unsupported image MIME type".into()));
    }
    media_block("image", media_type, bytes)
}
pub fn pdf_block(bytes: &[u8]) -> Result<crate::message::ContentBlock, crate::Error> {
    media_block("document", "application/pdf", bytes)
}
fn media_block(
    kind: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<crate::message::ContentBlock, crate::Error> {
    Ok(serde_json::from_value(
        serde_json::json!({"type":kind,"source":{"type":"base64","media_type":mime,"data":STANDARD.encode(bytes)}}),
    )?)
}

crate::redacted_debug!(
    PermissionMode,
    Initialize,
    HookMatcher,
    OutboundRequest,
    FlagSettings,
    InitializeResult,
    McpSetServersResult,
    McpStatus,
    McpServerStatus,
    McpResource,
    PermissionRules,
    PermissionRuleState,
    UsageResult,
    SettingsResult,
    BackgroundTasksResult,
    InterruptResult,
    ToolPermission,
    HookCallback,
    Elicitation,
    UserDialog,
    McpMessage,
    PermissionDecision,
    PermissionUpdate,
    RuleBehavior,
    PermissionRule,
    HookOutput,
    ElicitationValue,
    ElicitationResponse,
    DialogResponse,
    ResumeChoice,
    UserMessage,
    UserBody,
    Priority
);
