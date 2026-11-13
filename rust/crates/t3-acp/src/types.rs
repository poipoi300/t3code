use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnDescription {
    pub command: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    Numeric(serde_json::Number),
    String(String),
}

impl RequestId {
    pub fn identity(&self) -> String {
        // ACP agents reuse numeric and string IDs; keep their response identities distinct.
        match self {
            Self::Number(n) => format!("$t3:jsonrpc:number:{}", *n as f64),
            Self::Numeric(n) => format!(
                "$t3:jsonrpc:number:{}",
                n.as_f64()
                    .map(|n| if n == 0.0 { 0.0 } else { n })
                    .map_or_else(|| n.to_string(), |n| n.to_string())
            ),
            Self::String(s) if s.starts_with("$t3:jsonrpc:") => format!("$t3:jsonrpc:string:{s}"),
            Self::String(s) => s.clone(),
        }
    }
}

impl RequestId {
    fn number_bits(&self) -> Option<u64> {
        let number = match self {
            Self::Number(n) => Some(*n as f64),
            Self::Numeric(n) => n.as_f64(),
            Self::String(_) => None,
        };
        number.map(|n| if n == 0.0 { 0 } else { n.to_bits() })
    }
}

impl PartialEq for RequestId {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(a), Self::String(b)) => a == b,
            (Self::String(_), _) | (_, Self::String(_)) => false,
            _ => self.number_bits() == other.number_bits(),
        }
    }
}
impl Eq for RequestId {}
impl Hash for RequestId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::String(s) => {
                1_u8.hash(state);
                s.hash(state);
            }
            _ => {
                0_u8.hash(state);
                self.number_bits().hash(state);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("ACP error {code}: {message}")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(
        default,
        deserialize_with = "present_data",
        skip_serializing_if = "Option::is_none"
    )]
    pub data: Option<Value>,
}

fn present_data<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl RpcError {
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {method}"),
            data: None,
        }
    }

    pub fn invalid_params() -> Self {
        Self {
            code: -32602,
            message: "Invalid params".into(),
            data: None,
        }
    }

    pub fn internal() -> Self {
        Self {
            code: -32603,
            message: "Internal error".into(),
            data: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error("ACP transport {operation} failed: {detail}")]
    Io {
        operation: &'static str,
        detail: String,
    },
    #[error("Invalid ACP JSON-RPC frame")]
    InvalidFrame,
    #[error("ACP connection closed")]
    Closed,
    #[error("ACP input line exceeds {limit} bytes")]
    LineTooLong { limit: usize },
    #[error("ACP session {0} already has an active prompt")]
    PromptActive(String),
    #[error("ACP {0} timed out")]
    Timeout(&'static str),
    #[error("ACP response schema mismatch for {0}")]
    Schema(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    V1,
    V2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Negotiated {
    pub generation: Generation,
    /// Normalized v1-shaped capabilities, auth methods, and agent information.
    pub response: Value,
    /// The complete initialize response, including vendor fields.
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionUpdate {
    pub session_id: String,
    /// Normalized update; unsupported variants remain intact.
    pub update: Value,
    /// Original notification params, without decoding away vendor fields.
    pub raw: Value,
}

impl SessionUpdate {
    pub fn is_replay(&self) -> bool {
        self.raw.pointer("/_meta/isReplay") == Some(&Value::Bool(true))
    }

    pub(crate) fn counts_as_replay_activity(&self) -> bool {
        fn visible(content: &Value) -> bool {
            match content["type"].as_str() {
                Some("text") => content["text"].as_str().is_some_and(|s| !s.is_empty()),
                Some("resource") if content["resource"].get("text").is_some() => {
                    content["resource"]["text"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty())
                }
                Some(_) => true,
                None => false,
            }
        }
        if self.is_replay() {
            return true;
        }
        match self.kind() {
            "agent_message_chunk" | "agent_thought_chunk" => visible(&self.update["content"]),
            "agent_message" | "agent_thought" => self.update["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(visible)),
            "tool_call"
            | "tool_call_update"
            | "plan"
            | "plan_update"
            | "plan_removed"
            | "available_commands_update"
            | "current_mode_update"
            | "config_option_update"
            | "session_info_update"
            | "usage_update"
            | "compaction_update"
            | "compaction_summary_chunk" => true,
            _ => false,
        }
    }
    pub fn kind(&self) -> &str {
        self.update["sessionUpdate"].as_str().unwrap_or("")
    }

    pub fn is_known(&self) -> bool {
        matches!(
            self.kind(),
            "agent_message_chunk"
                | "agent_thought_chunk"
                | "user_message_chunk"
                | "agent_message"
                | "agent_thought"
                | "user_message"
                | "tool_call"
                | "tool_call_update"
                | "tool_call_content_chunk"
                | "plan"
                | "plan_update"
                | "plan_removed"
                | "available_commands_update"
                | "current_mode_update"
                | "config_option_update"
                | "session_info_update"
                | "usage_update"
                | "state_update"
                | "terminal_update"
                | "terminal_output_chunk"
                | "compaction_update"
                | "compaction_summary_chunk"
                | "notice"
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionState {
    pub config_options: Option<Value>,
    pub modes: Option<Value>,
    pub models: Option<Value>,
    pub(crate) revision: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// An unroutable frame was skipped; payloads are deliberately excluded.
    UnroutableFrame,
    /// A duplicate in-flight request was ignored, matching Effect RPC.
    DuplicateRequest {
        id: RequestId,
    },
    SessionUpdate(SessionUpdate),
    Notification {
        method: String,
        params: Value,
    },
    /// A malformed typed notification is skipped without terminating the reader.
    InvalidNotification {
        method: String,
    },
    /// Emitted only after the serialized writer flushes an agent request response.
    ResponseSent {
        id: RequestId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestContext {
    pub id: RequestId,
    pub method: String,
}

/// Raw payloads retain draft fields and vendor extensions; decode known fields with
/// the re-exported SDK schemas when an implementation needs them.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientRequest {
    Permission(Value),
    ReadTextFile(Value),
    WriteTextFile(Value),
    TerminalCreate(Value),
    TerminalOutput(Value),
    TerminalWaitForExit(Value),
    TerminalKill(Value),
    TerminalRelease(Value),
    Elicitation(Value),
    McpConnect(Value),
    McpMessage(Value),
    McpDisconnect(Value),
    Extension { method: String, params: Value },
}

/// The server supplies policy and effects here; no filesystem or process access is
/// performed by this crate. Core requests run concurrently; extension handlers run inline.
#[async_trait::async_trait]
pub trait ClientHandler: Send + Sync + 'static {
    async fn request(
        &self,
        request: ClientRequest,
        context: RequestContext,
    ) -> Result<Value, RpcError>;
}

pub struct RejectRequests;

#[async_trait::async_trait]
impl ClientHandler for RejectRequests {
    async fn request(&self, _: ClientRequest, context: RequestContext) -> Result<Value, RpcError> {
        Err(RpcError::method_not_found(&context.method))
    }
}
