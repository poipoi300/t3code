use crate::{
    base_schemas::{NonNegativeInt, PositiveInt},
    wire::JsNumber,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeTurnState {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeItemStatus {
    #[serde(rename = "inProgress")]
    InProgress,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "declined")]
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolLifecycleItemType {
    #[serde(rename = "command_execution")]
    CommandExecution,
    #[serde(rename = "file_change")]
    FileChange,
    #[serde(rename = "mcp_tool_call")]
    McpToolCall,
    #[serde(rename = "dynamic_tool_call")]
    DynamicToolCall,
    #[serde(rename = "collab_agent_tool_call")]
    CollabAgentToolCall,
    #[serde(rename = "web_search")]
    WebSearch,
    #[serde(rename = "image_view")]
    ImageView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageCost {
    pub amount: JsNumber,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTokenUsageSnapshot {
    pub used_tokens: NonNegativeInt,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub total_processed_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub max_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub input_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cached_input_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub output_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reasoning_output_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub last_used_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub last_input_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub last_cached_input_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub last_output_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub last_reasoning_output_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub tool_uses: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub duration_ms: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub compacts_automatically: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_compact_threshold: Option<Option<PositiveInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cost: Option<Option<TokenUsageCost>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnTokenUsageScope {
    #[serde(rename = "main_agent")]
    MainAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompleteUsageStatus {
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncompleteUsageStatus {
    #[serde(rename = "partial")]
    Partial,
    #[serde(rename = "unavailable")]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherTokenUsage {
    pub input_tokens: NonNegativeInt,
    pub output_tokens: NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteTurnTokenUsage {
    pub usage_scope: TurnTokenUsageScope,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cached_input_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cache_creation_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reasoning_tokens: Option<Option<NonNegativeInt>>,
    pub has_subagents: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub models: Option<Option<Vec<String>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub local_command: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub other_usage: Option<Option<OtherTokenUsage>>,
    pub usage_status: CompleteUsageStatus,
    pub input_tokens: NonNegativeInt,
    pub output_tokens: NonNegativeInt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncompleteTurnTokenUsage {
    pub usage_scope: TurnTokenUsageScope,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cached_input_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub cache_creation_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub reasoning_tokens: Option<Option<NonNegativeInt>>,
    pub has_subagents: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub models: Option<Option<Vec<String>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub local_command: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub other_usage: Option<Option<OtherTokenUsage>>,
    pub usage_status: IncompleteUsageStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub input_tokens: Option<Option<NonNegativeInt>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub output_tokens: Option<Option<NonNegativeInt>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TurnTokenUsage {
    Complete(CompleteTurnTokenUsage),
    Incomplete(IncompleteTurnTokenUsage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolActivitySurface {
    #[serde(rename = "browser")]
    Browser,
    #[serde(rename = "computer")]
    Computer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ToolActivityNativeAppReference {
    #[serde(rename = "app-id")]
    AppId {
        #[serde(rename = "appId")]
        app_id: String,
    },
    #[serde(rename = "display-name")]
    DisplayName {
        #[serde(rename = "displayName")]
        display_name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebsiteIconTag {
    #[serde(rename = "website")]
    Website,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeAppIconTag {
    #[serde(rename = "native-app")]
    NativeApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemedLogoIconTag {
    #[serde(rename = "themed-logo")]
    ThemedLogo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteIcon {
    #[serde(rename = "_tag")]
    pub _tag: WebsiteIconTag,
    pub page_url: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub favicon_url: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub favicon_url_dark: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeAppIcon {
    #[serde(rename = "_tag")]
    pub _tag: NativeAppIconTag,
    pub app: ToolActivityNativeAppReference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemedLogoIcon {
    #[serde(rename = "_tag")]
    pub _tag: ThemedLogoIconTag,
    pub logo_url: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub logo_url_dark: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolActivityIcon {
    Website(WebsiteIcon),
    NativeApp(NativeAppIcon),
    ThemedLogo(ThemedLogoIcon),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolActivitySourceKind {
    #[serde(rename = "browser")]
    Browser,
    #[serde(rename = "computer")]
    Computer,
    #[serde(rename = "integration")]
    Integration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolActivitySource {
    pub key: String,
    pub name: String,
    pub kind: ToolActivitySourceKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub icon: Option<Option<ToolActivityIcon>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestionOption {
    pub label: String,
    pub description: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub value: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<UserInputQuestionOption>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub allow_custom_answer: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub initial_answer: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub multi_select: Option<Option<bool>>,
}
