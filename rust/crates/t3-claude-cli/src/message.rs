use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, sync::Arc};

pub type Extensions = Map<String, Value>;

/// An immutable typed view retaining the exact JSON, including absent vs null keys.
#[derive(Clone, PartialEq)]
pub struct Decoded<T> {
    data: T,
    raw: Arc<Value>,
}

impl<T: DeserializeOwned> Decoded<T> {
    pub fn from_value(raw: Value) -> Result<Self, crate::Error> {
        Self::from_raw(Arc::new(raw))
    }
    fn from_raw(raw: Arc<Value>) -> Result<Self, crate::Error> {
        Ok(Self {
            data: T::deserialize(serde_stacker::Deserializer::new(raw.as_ref()))
                .map_err(crate::Error::from)?,
            raw,
        })
    }
}

impl<T> Decoded<T> {
    pub fn get(&self) -> &T {
        &self.data
    }
    pub fn raw(&self) -> &Value {
        &self.raw
    }
}

impl<T> Serialize for Decoded<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.as_ref().serialize(serializer)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Decoded<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_value(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Nested typed bodies share their enclosing message's raw snapshot instead of cloning it.
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Typed<T>(T);
impl<T> Typed<T> {
    pub fn get(&self) -> &T {
        &self.0
    }
}
impl<T> std::fmt::Debug for Typed<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Typed { <redacted> }")
    }
}
impl<T> std::fmt::Debug for Decoded<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decoded { <redacted> }")
    }
}

macro_rules! wire_union {
    ($name:ident, $tag:literal, {$($variant:ident($body:ty) => $wire:literal),* $(,)?}) => {
        #[derive(Clone, PartialEq)]
        pub enum $name { $($variant(Typed<$body>),)* Unknown(Value) }
        impl $name {
            pub fn from_value(raw: Value) -> Result<Self, crate::Error> {
                Ok(match raw.get($tag).and_then(Value::as_str) {
                    $(Some($wire) => match <$body>::deserialize(serde_stacker::Deserializer::new(&raw)) { Ok(v) => Self::$variant(Typed(v)), Err(_) => Self::Unknown(raw) },)*
                    _ => Self::Unknown(raw),
                })
            }
            pub fn has_unknown(&self) -> bool {
                match self { $(Self::$variant(v) => v.get().has_unknown(),)* Self::Unknown(_) => true }
            }
        }
        crate::redacted_debug!($name);
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                match self { $(Self::$variant(v) => v.serialize(s),)* Self::Unknown(v) => v.serialize(s) }
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::from_value(Value::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}
macro_rules! message_union {
    ($name:ident, $tag:literal, {$($variant:ident($body:ty) => $wire:literal),* $(,)?}) => {
        #[derive(Clone, PartialEq)]
        pub enum $name { $($variant(Decoded<$body>),)* Unknown(Arc<Value>) }
        impl $name {
            pub fn from_value(raw: Value) -> Result<Self, crate::Error> { Self::from_raw(Arc::new(raw)) }
            pub fn from_raw(raw: Arc<Value>) -> Result<Self, crate::Error> {
                Ok(match raw.get($tag).and_then(Value::as_str) {
                    $(Some($wire) => match Decoded::from_raw(raw.clone()) { Ok(v) => Self::$variant(v), Err(_) => Self::Unknown(raw) },)*
                    _ => Self::Unknown(raw),
                })
            }
            pub fn raw(&self) -> &Value { match self { $(Self::$variant(v) => v.raw(),)* Self::Unknown(v) => v } }
            pub fn has_unknown(&self) -> bool {
                match self { $(Self::$variant(v) => v.get().has_unknown(),)* Self::Unknown(_) => true }
            }
        }
        crate::redacted_debug!($name);
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> { self.raw().serialize(s) }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::from_value(Value::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

trait Compatibility {
    fn has_unknown(&self) -> bool;
}
macro_rules! compatible_struct {
    ($name:ident, $extra:ident $(, $nested:ident)*) => {
        impl Compatibility for $name {
            fn has_unknown(&self) -> bool {
                self.$extra.keys().any(|key| key != "type" && key != "subtype") $(|| self.$nested.has_unknown())*
            }
        }
    };
}
impl<T: Compatibility> Compatibility for Option<T> {
    fn has_unknown(&self) -> bool {
        self.as_ref().is_some_and(Compatibility::has_unknown)
    }
}
impl<T: Compatibility> Compatibility for Vec<T> {
    fn has_unknown(&self) -> bool {
        self.iter().any(Compatibility::has_unknown)
    }
}
impl<T: Compatibility> Compatibility for BTreeMap<String, T> {
    fn has_unknown(&self) -> bool {
        self.values().any(Compatibility::has_unknown)
    }
}
impl Compatibility for Content {
    fn has_unknown(&self) -> bool {
        match self {
            Self::Text(_) => false,
            Self::Blocks(v) => v.iter().any(ContentBlock::has_unknown),
        }
    }
}
wire_union!(ContentBlock, "type", {
    Text(TextBlock) => "text", Thinking(ThinkingBlock) => "thinking",
    RedactedThinking(RedactedThinkingBlock) => "redacted_thinking",
    ToolUse(ToolUseBlock) => "tool_use", ServerToolUse(ToolUseBlock) => "server_tool_use",
    McpToolUse(ToolUseBlock) => "mcp_tool_use", ToolResult(ToolResultBlock) => "tool_result",
    McpToolResult(ToolResultBlock) => "mcp_tool_result",
    BashResult(ToolResultBlock) => "bash_code_execution_tool_result",
    CodeResult(ToolResultBlock) => "code_execution_tool_result",
    AdvisorResult(ToolResultBlock) => "advisor_tool_result",
    EditorResult(ToolResultBlock) => "text_editor_code_execution_tool_result",
    ToolSearchResult(ToolResultBlock) => "tool_search_tool_result",
    WebFetchResult(ToolResultBlock) => "web_fetch_tool_result",
    WebSearchResult(ToolResultBlock) => "web_search_tool_result",
    Image(MediaBlock) => "image", Document(MediaBlock) => "document",
    Audio(OpenBlock) => "audio", Container(OpenBlock) => "container",
    Compaction(CompactionBlock) => "compaction"
});

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct TextBlock {
    pub text: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ThinkingBlock {
    pub thinking: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct RedactedThinkingBlock {
    pub data: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: Value,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolResultBlock {
    pub tool_use_id: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct MediaBlock {
    pub source: Value,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct CompactionBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct OpenBlock {
    #[serde(flatten)]
    pub fields: Extensions,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation: Option<CacheCreation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens_details: Option<OutputTokenDetails>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct CacheCreation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_5m_input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_1h_input_tokens: Option<f64>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct OutputTokenDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_tokens: Option<f64>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<f64>,
    #[serde(rename = "costUSD")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<f64>,
    #[serde(flatten)]
    pub extra: Extensions,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: Content,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Conversation {
    pub message: ConversationMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub parent_tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_error_params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_meta: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result_meta: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_uuids: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extensions,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ResultMessage {
    pub subtype: String,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_error_status: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(rename = "modelUsage")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_usage: Option<BTreeMap<String, ModelUsage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_api_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_command: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_uuids: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extensions,
}

wire_union!(StreamEvent, "type", {
    MessageStart(MessageStart) => "message_start", MessageDelta(MessageDelta) => "message_delta",
    MessageStop(OpenBlock) => "message_stop", ContentBlockStart(BlockStart) => "content_block_start",
    ContentBlockDelta(BlockDelta) => "content_block_delta", ContentBlockStop(BlockStop) => "content_block_stop",
    Ping(OpenBlock) => "ping", Error(OpenBlock) => "error"
});
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct MessageStart {
    pub message: ConversationMessage,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct MessageDelta {
    pub delta: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct BlockStart {
    pub index: u64,
    pub content_block: ContentBlock,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct BlockDelta {
    pub index: u64,
    pub delta: Delta,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct BlockStop {
    pub index: u64,
    #[serde(flatten)]
    pub extra: Extensions,
}
wire_union!(Delta, "type", {
    Text(TextBlock) => "text_delta", Thinking(ThinkingBlock) => "thinking_delta",
    InputJson(JsonDelta) => "input_json_delta", Signature(SignatureDelta) => "signature_delta",
    Citations(CitationDelta) => "citations_delta", Compaction(CompactionBlock) => "compaction_delta"
});
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct JsonDelta {
    pub partial_json: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct SignatureDelta {
    pub signature: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct CitationDelta {
    pub citation: Value,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct PartialMessage {
    pub event: StreamEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub parent_tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_uuids: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extensions,
}

wire_union!(SystemMessage, "subtype", {
    Init(InitMessage) => "init", Status(StatusMessage) => "status", CompactBoundary(CompactBoundary) => "compact_boundary",
    ApiRetry(ApiRetry) => "api_retry", TaskStarted(TaskMessage) => "task_started",
    TaskProgress(TaskMessage) => "task_progress", TaskUpdated(TaskMessage) => "task_updated",
    TaskNotification(TaskMessage) => "task_notification", BackgroundTasksChanged(BackgroundTasks) => "background_tasks_changed",
    ModelRefusalFallback(ModelRefusal) => "model_refusal_fallback", ModelRefusalNoFallback(ModelRefusal) => "model_refusal_no_fallback",
    Informational(Notice) => "informational", LocalCommandOutput(Notice) => "local_command_output",
    HookStarted(HookMessage) => "hook_started", HookProgress(HookMessage) => "hook_progress", HookResponse(HookMessage) => "hook_response",
    PluginInstall(PluginInstall) => "plugin_install", ThinkingTokens(ThinkingTokens) => "thinking_tokens",
    SessionStateChanged(SessionState) => "session_state_changed", WorkerShuttingDown(WorkerShutdown) => "worker_shutting_down",
    CommandsChanged(CommandsChanged) => "commands_changed", Notification(Notification) => "notification",
    FilesPersisted(FilesPersisted) => "files_persisted", MemoryRecall(MemoryRecall) => "memory_recall",
    ElicitationComplete(ElicitationComplete) => "elicitation_complete", PermissionDenied(PermissionDenied) => "permission_denied",
    MirrorError(MirrorError) => "mirror_error", ConversationReset(ConversationReset) => "conversation_reset",
    SessionWritesLost(SessionWritesLost) => "session_writes_lost", ControlRequestProgress(ControlProgress) => "control_request_progress",
    AutocompactState(OpenBlock) => "autocompact_state", UiPanes(OpenBlock) => "ui_panes"
});
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct InitMessage {
    pub session_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code_version: Option<String>,
    #[serde(rename = "permissionMode")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
    #[serde(default)]
    pub slash_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct StatusMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "permissionMode")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_error: Option<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct CompactBoundary {
    pub compact_metadata: CompactMetadata,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct CompactMetadata {
    pub trigger: String,
    pub pre_tokens: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserved_segment: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserved_messages: Option<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ApiRetry {
    pub attempt: u64,
    pub max_retries: u64,
    pub retry_delay_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_status: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct TaskMessage {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_depth: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_backgrounded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_progress: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct BackgroundTasks {
    pub tasks: Vec<TaskMessage>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Notice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prevent_continuation: Option<bool>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct HookMessage {
    pub hook_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct RateLimit {
    pub rate_limit_info: RateLimitInfo,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitInfo {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overage_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overage_resets_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overage_disabled_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_using_overage: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overage_in_use: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overage_enabled: Option<bool>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct CommandLifecycle {
    pub command_uuid: String,
    pub state: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct PromptSuggestion {
    pub suggestion: String,
    #[serde(flatten)]
    pub extra: Extensions,
}

message_union!(Message, "type", {
    System(SystemMessage) => "system", Assistant(Conversation) => "assistant", User(Conversation) => "user",
    Result(ResultMessage) => "result", StreamEvent(PartialMessage) => "stream_event", RateLimit(RateLimit) => "rate_limit_event",
    CommandLifecycle(CommandLifecycle) => "command_lifecycle", PromptSuggestion(PromptSuggestion) => "prompt_suggestion",
    ActiveGoal(OpenBlock) => "active_goal", AuthStatus(AuthStatus) => "auth_status",
    ToolProgress(ToolProgress) => "tool_progress", ToolUseSummary(ToolSummary) => "tool_use_summary",
    ConversationReset(ConversationReset) => "conversation_reset",
    KeepAlive(OpenBlock) => "keep_alive", TranscriptMirror(OpenBlock) => "transcript_mirror"
});

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ThinkingTokens {
    pub estimated_tokens: f64,
    pub estimated_tokens_delta: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_uuid: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct PluginInstall {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct SessionState {
    pub state: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct WorkerShutdown {
    pub reason: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct CommandsChanged {
    pub commands: Vec<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Notification {
    pub key: String,
    pub text: String,
    pub priority: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<f64>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct FilesPersisted {
    pub files: Vec<Value>,
    pub failed: Vec<Value>,
    pub processed_at: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct MemoryRecall {
    pub mode: String,
    pub memories: Vec<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ElicitationComplete {
    pub mcp_server_name: String,
    pub elicitation_id: String,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct PermissionDenied {
    pub tool_name: String,
    pub tool_use_id: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason_type: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct MirrorError {
    pub error: String,
    pub key: Value,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ConversationReset {
    pub new_conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_uuid: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct SessionWritesLost {
    pub error_code: String,
    pub lost: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_session_ids: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ControlProgress {
    pub request_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_status: Option<u32>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ModelRefusal {
    pub content: String,
    pub original_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_refusal_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_refusal_explanation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted_message_uuids: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub is_authenticating: bool,
    pub output: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolProgress {
    pub tool_use_id: String,
    pub tool_name: String,
    pub parent_tool_use_id: Option<String>,
    pub elapsed_time_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_retry: Option<Value>,
    #[serde(flatten)]
    pub extra: Extensions,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolSummary {
    pub summary: String,
    pub preceding_tool_use_ids: Vec<String>,
    #[serde(flatten)]
    pub extra: Extensions,
}

impl Message {
    pub fn cli_version(&self) -> Option<&str> {
        match self {
            Self::System(v) => match v.get() {
                SystemMessage::Init(init) => init.get().claude_code_version.as_deref(),
                _ => None,
            },
            _ => None,
        }
    }
}

crate::redacted_debug!(
    TextBlock,
    ThinkingBlock,
    RedactedThinkingBlock,
    ToolUseBlock,
    ToolResultBlock,
    MediaBlock,
    CompactionBlock,
    OpenBlock,
    Content,
    Usage,
    CacheCreation,
    OutputTokenDetails,
    ModelUsage,
    ConversationMessage,
    Conversation,
    ResultMessage,
    MessageStart,
    MessageDelta,
    BlockStart,
    BlockDelta,
    BlockStop,
    JsonDelta,
    SignatureDelta,
    CitationDelta,
    PartialMessage,
    InitMessage,
    StatusMessage,
    CompactBoundary,
    CompactMetadata,
    ApiRetry,
    TaskMessage,
    BackgroundTasks,
    Notice,
    HookMessage,
    RateLimit,
    RateLimitInfo,
    CommandLifecycle,
    PromptSuggestion,
    ThinkingTokens,
    PluginInstall,
    SessionState,
    WorkerShutdown,
    CommandsChanged,
    Notification,
    FilesPersisted,
    MemoryRecall,
    ElicitationComplete,
    PermissionDenied,
    MirrorError,
    ConversationReset,
    SessionWritesLost,
    ControlProgress,
    ModelRefusal,
    AuthStatus,
    ToolProgress,
    ToolSummary
);

compatible_struct!(TextBlock, extra);
compatible_struct!(ThinkingBlock, extra);
compatible_struct!(RedactedThinkingBlock, extra);
compatible_struct!(ToolUseBlock, extra);
compatible_struct!(ToolResultBlock, extra);
compatible_struct!(MediaBlock, extra);
compatible_struct!(CompactionBlock, extra);
compatible_struct!(OpenBlock, fields);
compatible_struct!(Usage, extra, cache_creation, output_tokens_details);
compatible_struct!(CacheCreation, extra);
compatible_struct!(OutputTokenDetails, extra);
compatible_struct!(ModelUsage, extra);
compatible_struct!(ConversationMessage, extra, content, usage);
compatible_struct!(Conversation, extra, message);
compatible_struct!(ResultMessage, extra, usage, model_usage);
compatible_struct!(MessageStart, extra, message);
compatible_struct!(MessageDelta, extra, usage);
compatible_struct!(BlockStart, extra, content_block);
compatible_struct!(BlockDelta, extra, delta);
compatible_struct!(BlockStop, extra);
compatible_struct!(JsonDelta, extra);
compatible_struct!(SignatureDelta, extra);
compatible_struct!(CitationDelta, extra);
compatible_struct!(PartialMessage, extra, event);

compatible_struct!(CompactBoundary, extra, compact_metadata);
compatible_struct!(CompactMetadata, extra);
compatible_struct!(ApiRetry, extra);
compatible_struct!(TaskMessage, extra);
compatible_struct!(BackgroundTasks, extra, tasks);
compatible_struct!(Notice, extra);
compatible_struct!(HookMessage, extra);
compatible_struct!(RateLimit, extra, rate_limit_info);
compatible_struct!(RateLimitInfo, extra);
compatible_struct!(CommandLifecycle, extra);
compatible_struct!(PromptSuggestion, extra);
compatible_struct!(ThinkingTokens, extra);
compatible_struct!(PluginInstall, extra);
compatible_struct!(SessionState, extra);
compatible_struct!(WorkerShutdown, extra);
compatible_struct!(CommandsChanged, extra);
compatible_struct!(Notification, extra);
compatible_struct!(FilesPersisted, extra);
compatible_struct!(MemoryRecall, extra);
compatible_struct!(ElicitationComplete, extra);
compatible_struct!(PermissionDenied, extra);
compatible_struct!(MirrorError, extra);
compatible_struct!(ConversationReset, extra);
compatible_struct!(SessionWritesLost, extra);
compatible_struct!(ControlProgress, extra);
compatible_struct!(ModelRefusal, extra);
compatible_struct!(AuthStatus, extra);
compatible_struct!(ToolProgress, extra);
compatible_struct!(ToolSummary, extra);

fn unknown_permission_mode(mode: Option<&str>) -> bool {
    mode.is_some_and(|mode| {
        crate::control::PermissionMode::deserialize(Value::String(mode.into())).is_err()
    })
}
impl Compatibility for InitMessage {
    fn has_unknown(&self) -> bool {
        self.extra
            .keys()
            .any(|key| key != "type" && key != "subtype")
            || unknown_permission_mode(self.permission_mode.as_deref())
    }
}
impl Compatibility for StatusMessage {
    fn has_unknown(&self) -> bool {
        self.extra
            .keys()
            .any(|key| key != "type" && key != "subtype")
            || unknown_permission_mode(self.permission_mode.as_deref())
    }
}
