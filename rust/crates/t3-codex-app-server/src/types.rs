//! T3-used protocol DTOs. Provenance and the upstream revision live in Cargo.toml.
//!
//! Open strings and flattened fields intentionally retain release additions.
//! Config, MCP content/schema, permissions and raw Responses items are JSON APIs.
use crate::{Result, wire::Notification};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type Object = BTreeMap<String, Value>;

macro_rules! dto {
    ($name:ident { $($(#[$attr:meta])* $field:ident : $ty:ty),* $(,)? } { $($(#[$oattr:meta])* $opt:ident : $oty:ty),* $(,)? }) => {
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            $( $(#[$attr])* pub $field: $ty,)*
            $( $(#[$oattr])* #[serde(default, skip_serializing_if = "Option::is_none")] pub $opt: Option<$oty>,)*
            #[serde(flatten)] pub extra: Object,
        }
    };
}

dto!(Empty {  } {  });
dto!(ClientInfo {
    name: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    title: Field<String>,
    version: String,
} {
});
dto!(InitializeCapabilities {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    extensions: Field<Object>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    opt_out_notification_methods: Field<Vec<String>>,
} {
    experimental_api: bool,
    explicit_gateway_oauth: bool,
    mcp_server_openai_form_elicitation: bool,
    request_attestation: bool,
});
dto!(InitializeParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    capabilities: Field<InitializeCapabilities>,
    client_info: ClientInfo,
} {
});
dto!(InitializeResponse {
    codex_home: String,
    platform_family: String,
    platform_os: String,
    user_agent: String,
} {
});

impl InitializeParams {
    pub fn t3(version: impl Into<String>) -> Self {
        Self {
            client_info: ClientInfo {
                name: "T3 Code".into(),
                title: Field::Value("T3 Code".into()),
                version: version.into(),
                ..Default::default()
            },
            capabilities: Field::Value(InitializeCapabilities {
                experimental_api: Some(true),
                opt_out_notification_methods: Field::Value(vec!["turn/diff/updated".into()]),
                extensions: Field::Value(BTreeMap::from([(
                    "io.modelcontextprotocol/ui".into(),
                    serde_json::json!({"mimeTypes":["text/html;profile=mcp-app"]}),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    pub fn probe(version: impl Into<String>) -> Self {
        let mut params = Self::t3(version);
        params.capabilities = Field::Value(InitializeCapabilities {
            experimental_api: Some(true),
            ..Default::default()
        });
        params
    }
}

dto!(ThreadStartParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approval_policy: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approvals_reviewer: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    base_instructions: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    config: Field<Object>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cwd: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    developer_instructions: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    ephemeral: Field<bool>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model_provider: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    personality: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    sandbox: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    service_name: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    service_tier: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    session_start_source: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    thread_source: Field<String>,
} {
});
dto!(ThreadResumeParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approval_policy: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approvals_reviewer: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    base_instructions: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    config: Field<Object>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cwd: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    developer_instructions: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model_provider: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    personality: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    sandbox: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    service_tier: Field<String>,
    thread_id: String,
} {
    exclude_turns: bool,
});
dto!(ThreadForkParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approval_policy: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approvals_reviewer: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    base_instructions: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    config: Field<Object>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cwd: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    developer_instructions: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    last_turn_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model_provider: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    sandbox: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    service_tier: Field<String>,
    thread_id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    thread_source: Field<String>,
} {
    ephemeral: bool,
    exclude_turns: bool,
});
dto!(ThreadIdParams { #[serde(rename = "threadId")] thread_id: String } {  });
dto!(ThreadReadParams {
    #[serde(rename = "includeTurns")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_turns: Option<bool>,
    #[serde(rename = "threadId")]
    thread_id: String,
} {
});
dto!(ThreadRevertParams {
    #[serde(rename = "beforeTurnId")]
    before_turn_id: String,
    #[serde(rename = "threadId")]
    thread_id: String,
} {
});
dto!(ThreadTurnsListParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cursor: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    items_view: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    limit: Field<u32>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    sort_direction: Field<String>,
    thread_id: String,
} {
});
dto!(ThreadInjectItemsParams { thread_id: String, items: Vec<Value> } {  });
dto!(ThreadGoalSetParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    objective: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    status: Field<String>,
    thread_id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    token_budget: Field<i64>,
} {
});
dto!(Thread {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    agent_nickname: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    agent_role: Field<String>,
    cli_version: String,
    created_at: i64,
    cwd: String,
    ephemeral: bool,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    forked_from_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    git_info: Field<Value>,
    id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model: Field<String>,
    model_provider: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    name: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    originator: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    parent_thread_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    path: Field<String>,
    preview: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    project_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    reasoning_effort: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    recency_at: Field<i64>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    section: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    section_entered_at: Field<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    source: Value,
    status: ThreadStatus,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    thread_source: Field<String>,
    turns: Vec<Turn>,
    updated_at: i64,
} {
    history_mode: String,
});

dto!(ThreadResponse {
    thread: Thread,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    reasoning_effort: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    service_tier: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    collaboration_mode: Field<CollaborationMode>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    items_backwards_cursor: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    turns_backwards_cursor: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approval_policy: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    sandbox: Field<Value>,
} {
    approvals_reviewer: String,
    cwd: String,
    disabled_plugin_ids: Vec<String>,
    instruction_sources: Vec<String>,
    model: String,
    model_provider: String,
});
dto!(ThreadTurnsListResponse {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    backwards_cursor: Field<String>,
    data: Vec<Turn>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    next_cursor: Field<String>,
} {
});
dto!(ThreadGoal {
    created_at: i64,
    objective: String,
    status: String,
    thread_id: String,
    time_used_seconds: i64,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    token_budget: Field<i64>,
    tokens_used: i64,
    updated_at: i64,
} {
});
dto!(ThreadGoalResponse {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    goal: Field<ThreadGoal>,
} {
});
dto!(ThreadGoalClearResponse { cleared: bool } {  });
dto!(Turn {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    completed_at: Field<i64>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    duration_ms: Field<i64>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    error: Field<TurnError>,
    id: String,
    items: Vec<ThreadItem>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    started_at: Field<i64>,
    status: String,
} {
    items_view: String,
});
dto!(TurnError {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    additional_details: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    codex_error_info: Field<Value>,
    message: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    misalignment: Field<Value>,
} {
});
dto!(TurnResponse { turn: Turn } {  });

macro_rules! open_union {
    ($name:ident ($tag:literal) { $($variant:ident = $wire:literal { $($(#[$attr:meta])* $field:ident : $ty:ty),* $(,)? });* $(;)? }) => {
        #[derive(Clone, Debug, PartialEq)]
        pub enum $name {
            $($variant { $($field: $ty,)* extra: Object },)*
            Other { kind: String, fields: Object },
        }
        impl Default for $name {
            fn default() -> Self { Self::Other { kind: String::new(), fields: Object::new() } }
        }
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
                $(#[allow(non_snake_case)] mod $variant {
                    #[derive(serde::Serialize)]
                    pub enum Tag { #[serde(rename = $wire)] Value }
                })*
                #[derive(Serialize)]
                #[serde(untagged)]
                enum Known<'a> {
                    $($variant {
                        #[serde(rename = $tag)] _wire_type: $variant::Tag,
                        $($(#[$attr])* $field: &'a $ty,)*
                        #[serde(flatten)] extra: &'a Object,
                    },)*
                }
                match self {
                    $(Self::$variant { $($field,)* extra } => Known::$variant { _wire_type: $variant::Tag::Value, $($field,)* extra }.serialize(serializer),)*
                    Self::Other { kind, fields } => {
                        use serde::ser::SerializeMap;
                        let mut map = serializer.serialize_map(Some(fields.len() + 1))?;
                        map.serialize_entry($tag, kind)?;
                        for (key, value) in fields { map.serialize_entry(key, value)?; }
                        map.end()
                    }
                }
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                $(#[allow(non_snake_case)] mod $variant {
                    #[derive(serde::Deserialize)]
                    pub enum Tag { #[serde(rename = $wire)] Value }
                })*
                #[derive(Deserialize)]
                #[serde(untagged)]
                enum Known {
                    $($variant {
                        #[serde(rename = $tag)] _kind: $variant::Tag,
                        $($(#[$attr])* $field: $ty,)*
                        #[serde(flatten)] extra: Object,
                    },)*
                }
                #[derive(Deserialize)]
                #[serde(untagged)]
                // This scratch value is consumed immediately; boxing adds a per-item allocation.
                #[allow(clippy::large_enum_variant)]
                enum Wire { Known(Known), Other(Object) }
                Ok(match Wire::deserialize(deserializer)? {
                    Wire::Known(known) => match known {
                        $(Known::$variant { $($field,)* extra, .. } => Self::$variant { $($field,)* extra },)*
                    },
                    Wire::Other(mut fields) => {
                        let kind = match fields.remove($tag) {
                            Some(Value::String(kind)) => kind,
                            _ => return Err(serde::de::Error::custom("missing union type")),
                        };
                        Self::Other { kind, fields }
                    }
                })
            }
        }
    };
}
open_union! { UserInput ("type") {
    Text = "text" {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_elements: Option<Vec<Value>>,
    };
    Image = "image" {
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        detail: Field<String>,
        url: String,
    };
    ImageFile = "image" {
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        detail: Field<String>,
        #[serde(rename = "fileId")]
        file_id: String,
    };
    LocalImage = "localImage" {
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        detail: Field<String>,
        path: String,
    };
    Audio = "audio" { url: String };
    LocalAudio = "localAudio" { path: String };
    Skill = "skill" { name: String, path: String };
    Mention = "mention" { name: String, path: String };
} }

dto!(CollaborationMode { mode: String, settings: CollaborationSettings } {  });
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CollaborationSettings {
    pub model: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub reasoning_effort: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub developer_instructions: Field<String>,
    #[serde(flatten)]
    pub extra: Object,
}
dto!(AdditionalContextEntry { kind: String, value: String } {  });
dto!(TurnStartParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approval_policy: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approvals_reviewer: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    client_user_message_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cwd: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    disabled_plugin_ids: Field<Vec<String>>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    effort: Field<String>,
    input: Vec<UserInput>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    personality: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    sandbox_policy: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    service_tier: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    service_tier_for_turn: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    summary: Field<String>,
    thread_id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    tool_output: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    turn_trigger: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    output_schema: Field<Value>,
} {
    collaboration_mode: CollaborationMode,
    additional_context: BTreeMap<String, AdditionalContextEntry>,
    cyber_access_program: String,
});
dto!(TurnSteerParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    client_user_message_id: Field<String>,
    expected_turn_id: String,
    input: Vec<UserInput>,
    thread_id: String,
} {
});
dto!(TurnSteerResponse { #[serde(rename = "turnId")] turn_id: String } {  });
dto!(TurnInterruptParams {
    #[serde(rename = "threadId")]
    thread_id: String,
    #[serde(rename = "turnId")]
    turn_id: String,
} {
});
dto!(ConfigReadParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cwd: Field<String>,
    #[serde(rename = "includeLayers")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_layers: Option<bool>,
} {
});
dto!(ConfigReadResponse {
    config: Object,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    layers: Field<Vec<Value>>,
    origins: Object,
} {
});
dto!(FeedbackUploadParams {
    classification: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    extra_log_files: Field<Vec<String>>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    reason: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    tags: Field<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    thread_id: Field<String>,
} {
    include_logs: bool,
});
dto!(FeedbackUploadResponse {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    prompt_hash: Field<String>,
    thread_id: String,
} {
});
dto!(ModelListParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cursor: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    include_hidden: Field<bool>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    limit: Field<u32>,
} {
});
dto!(ReasoningEffortOption { description: String, reasoning_effort: String } {  });
dto!(Model {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    availability_nux: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    available_access_programs: Field<Value>,
    default_reasoning_effort: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    default_service_tier: Field<String>,
    description: String,
    display_name: String,
    hidden: bool,
    id: String,
    is_default: bool,
    model: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model_specialty: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    multi_agent_version: Field<String>,
    supported_reasoning_efforts: Vec<ReasoningEffortOption>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    upgrade: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    upgrade_info: Field<Value>,
} {
    additional_speed_tiers: Vec<String>,
    input_modalities: Vec<String>,
    service_tiers: Vec<Value>,
    supports_personality: bool,
});
dto!(ModelListResponse {
    data: Vec<Model>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    next_cursor: Field<String>,
} {
});
dto!(GetAccountParams {
    #[serde(rename = "refreshToken")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<bool>,
} {
});

dto!(GetAccountResponse {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    account: Field<Account>,
    requires_openai_auth: bool,
} {
});
dto!(RateLimitWindow {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    resets_at: Field<i64>,
    used_percent: f64,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    window_duration_mins: Field<i64>,
} {
});
/// Partial updates distinguish an absent field from an explicit reset to null.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Field<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}
impl<T> Field<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}
impl<T: Serialize> Serialize for Field<T> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Value(value) => value.serialize(serializer),
            Self::Null | Self::Missing => serializer.serialize_none(),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer)
            .map(|value| value.map(Self::Value).unwrap_or(Self::Null))
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSnapshot {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub limit_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub plan_type: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub rate_limit_reached_type: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub primary: Field<RateLimitWindow>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub secondary: Field<RateLimitWindow>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub individual_limit: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub credits: Field<Value>,
    #[serde(flatten)]
    pub extra: Object,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub limit_name: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub normal_model_slug: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    pub spend_control_reached: Field<bool>,
}
dto!(GetAccountRateLimitsResponse {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    account_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    ordinary_usage_allowed: Field<bool>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    rate_limit_reset_credits: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    rate_limits_by_limit_id: Field<BTreeMap<String, RateLimitSnapshot>>,
    rate_limits: RateLimitSnapshot,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    rate_limit_upsell: Field<Value>,
} {
});
dto!(ConsumeResetCreditParams {
    #[serde(rename = "creditId")]
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    credit_id: Field<String>,
    #[serde(rename = "idempotencyKey")]
    idempotency_key: String,
} {
});
dto!(ConsumeResetCreditResponse { outcome: String } {  });
dto!(SkillsListParams {  } { cwds: Vec<String>, force_reload: bool });
dto!(Skill {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    dependencies: Field<Value>,
    description: String,
    enabled: bool,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    interface: Field<Value>,
    name: String,
    path: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    plugin_id: Field<String>,
    scope: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    short_description: Field<String>,
} {
});
dto!(SkillsListEntry { cwd: String, errors: Vec<Value>, skills: Vec<Skill> } {  });
dto!(SkillsListResponse { data: Vec<SkillsListEntry> } {  });
dto!(ListMcpServerStatusParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cursor: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    detail: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    limit: Field<u32>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    server_name: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    thread_id: Field<String>,
} {
});
dto!(McpServerStatus {
    auth_status: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    http_origin: Field<String>,
    name: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    plugin_id: Field<String>,
    resource_templates: Vec<Value>,
    resources: Vec<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    runtime_status: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    server_info: Field<Value>,
    tools: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    tools_error: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    server_capabilities: Field<Value>,
} {
});
dto!(ListMcpServerStatusResponse {
    data: Vec<McpServerStatus>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    next_cursor: Field<String>,
} {
});
dto!(McpServerToolCallParams {
    server: String,
    thread_id: String,
    tool: String,
    #[serde(rename = "_meta")]
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    meta: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    arguments: Field<Value>,
} {
});
dto!(McpServerToolCallResponse {
    content: Vec<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    is_error: Field<bool>,
    #[serde(rename = "_meta")]
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    meta: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    structured_content: Field<Value>,
} {
});
dto!(McpResourceReadParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    connector_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    origin_call_id: Field<String>,
    server: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    target: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    thread_id: Field<String>,
    uri: String,
} {
});
dto!(McpResourceReadResponse {
    contents: Vec<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    origin_call_id: Field<String>,
} {
});
dto!(BackgroundTerminalsListParams { thread_id: String } { cursor: String });
dto!(BackgroundTerminal { process_id: String } {  });
dto!(BackgroundTerminalsListResponse { data: Vec<BackgroundTerminal>, next_cursor: Option<String> } {  });
dto!(BackgroundTerminalsTerminateParams { thread_id: String, process_id: String } {  });
dto!(BackgroundTerminalsTerminateResponse { terminated: bool } {  });
dto!(ThreadSampleParams { thread_id: String, expected_turn_id: String, prompt: String, effort: String } {  });
dto!(ThreadSampleResponse { text: Option<String> } {  });

/// Native item payloads stay intact; access the discriminant before decoding a variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadItem {
    pub id: String,
    #[serde(flatten)]
    pub body: ItemBody,
}

open_union! { ItemBody ("type") {
    UserMessage = "userMessage" {
        #[serde(rename = "clientId")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        client_id: Field<String>,
        content: Vec<UserInput>,
    };
    HookPrompt = "hookPrompt" { fragments: Vec<Value> };
    AgentMessage = "agentMessage" {
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        delivery: Field<String>,
        #[serde(rename = "memoryCitation")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        memory_citation: Field<Value>,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        phase: Field<String>,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        questions: Field<Vec<Value>>,
        text: String,
    };
    FunctionCallOutput = "functionCallOutput" {
        name: String,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        namespace: Field<String>,
        output: FunctionCallOutputBody,
    };
    Plan = "plan" { text: String };
    Reasoning = "reasoning" {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<Vec<String>>,
    };
    CommandExecution = "commandExecution" {
        #[serde(rename = "aggregatedOutput")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        aggregated_output: Field<String>,
        command: String,
        #[serde(rename = "commandActions")]
        command_actions: Vec<Value>,
        cwd: String,
        #[serde(rename = "durationMs")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        duration_ms: Field<i64>,
        #[serde(rename = "exitCode")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        exit_code: Field<i64>,
        #[serde(rename = "pluginId")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        plugin_id: Field<String>,
        #[serde(rename = "processId")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        process_id: Field<String>,
        #[serde(rename = "scriptPath")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        script_path: Field<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        status: String,
    };
    FileChange = "fileChange" { changes: Vec<crate::types::FileChange>, status: String };
    McpToolCall = "mcpToolCall" {
        #[serde(rename = "appContext")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        app_context: Field<Value>,
        arguments: Value,
        #[serde(rename = "durationMs")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        duration_ms: Field<i64>,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        error: Field<ToolError>,
        #[serde(rename = "mcpAppResourceUri")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        mcp_app_resource_uri: Field<String>,
        #[serde(rename = "mcpAppUi")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        mcp_app_ui: Field<Value>,
        #[serde(rename = "pluginId")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        plugin_id: Field<String>,
        #[serde(rename = "readOnlyHint")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        read_only_hint: Field<bool>,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        result: Field<Value>,
        server: String,
        status: String,
        tool: String,
    };
    DynamicToolCall = "dynamicToolCall" {
        arguments: Value,
        #[serde(rename = "contentItems")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        content_items: Field<Vec<Value>>,
        #[serde(rename = "durationMs")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        duration_ms: Field<i64>,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        namespace: Field<String>,
        status: String,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        success: Field<bool>,
        tool: String,
    };
    CollabAgentToolCall = "collabAgentToolCall" {
        #[serde(rename = "agentsStates")]
        agents_states: Object,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        model: Field<String>,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        prompt: Field<String>,
        #[serde(rename = "reasoningEffort")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        reasoning_effort: Field<String>,
        #[serde(rename = "receiverThreadIds")]
        receiver_thread_ids: Vec<String>,
        #[serde(rename = "senderThreadId")]
        sender_thread_id: String,
        status: String,
        tool: String,
    };
    SubAgentActivity = "subAgentActivity" {
        #[serde(rename = "agentPath")]
        agent_path: String,
        #[serde(rename = "agentThreadId")]
        agent_thread_id: String,
        kind: String,
    };
    WebSearch = "webSearch" {
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        action: Field<Value>,
        query: String,
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        results: Field<Vec<Value>>,
    };
    ImageView = "imageView" { path: String };
    Sleep = "sleep" { #[serde(rename = "durationMs")] duration_ms: i64 };
    ImageGeneration = "imageGeneration" {
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        failure: Field<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(rename = "revisedPrompt")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        revised_prompt: Field<String>,
        #[serde(rename = "savedPath")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        saved_path: Field<String>,
        status: String,
        #[serde(rename = "transparentBackground")]
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        transparent_background: Field<bool>,
        #[serde(rename = "fileId")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
    };
    EnteredReviewMode = "enteredReviewMode" { review: String };
    ExitedReviewMode = "exitedReviewMode" { review: String };
    ContextCompaction = "contextCompaction" {  };
} }
impl Default for ThreadItem {
    fn default() -> Self {
        Self {
            id: String::new(),
            body: ItemBody::Other {
                kind: String::new(),
                fields: Object::new(),
            },
        }
    }
}
dto!(ToolError { message: String } {  });
dto!(FileChange { diff: String, kind: FileChangeKind, path: String } {  });
open_union! { FileChangeKind ("type") {
    Add = "add" {  };
    Delete = "delete" {  };
    Update = "update" {
        #[serde(default, skip_serializing_if = "Field::is_missing")]
        move_path: Field<String>,
    };
} }

dto!(ItemDelta {
    delta: String,
    item_id: String,
    thread_id: String,
    turn_id: String,
} {
    summary_index: u32,
    content_index: u32,
});
dto!(ItemNotification {
    item: ThreadItem,
    thread_id: String,
    turn_id: String,
} {
    started_at_ms: i64,
    completed_at_ms: i64,
});
dto!(TurnNotification { thread_id: String, turn: Turn } {  });
dto!(ThreadNotification { #[serde(rename = "threadId")] thread_id: String } {  });
dto!(PlanStep { status: String, step: String } {  });
dto!(TurnPlanUpdated {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    explanation: Field<String>,
    plan: Vec<PlanStep>,
    thread_id: String,
    turn_id: String,
} {
});
dto!(TokenUsageBreakdown {
    cached_input_tokens: i64,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
} {
    cache_write_input_tokens: i64,
});
dto!(TokenUsage {
    last: TokenUsageBreakdown,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    model_context_window: Field<i64>,
    total: TokenUsageBreakdown,
} {
});
dto!(ThreadTokenUsageUpdated { thread_id: String, token_usage: TokenUsage, turn_id: String } {  });
dto!(ThreadSettings {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    active_permission_profile: Field<Value>,
    approval_policy: Value,
    approvals_reviewer: String,
    collaboration_mode: CollaborationMode,
    cwd: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    effort: Field<String>,
    model: String,
    model_provider: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    personality: Field<String>,
    sandbox_policy: Value,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    service_tier: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    summary: Field<String>,
} {
    disabled_plugin_ids: Vec<String>,
});
dto!(ThreadSettingsUpdated { thread_id: String, thread_settings: ThreadSettings } {  });
dto!(ModelRerouted {
    from_model: String,
    reason: String,
    thread_id: String,
    to_model: String,
    turn_id: String,
} {
});
dto!(McpStartupStatusUpdated {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    error: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    failure_reason: Field<String>,
    name: String,
    status: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    thread_id: Field<String>,
} {
});
dto!(HookRun {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    completed_at: Field<i64>,
    display_order: i64,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    duration_ms: Field<i64>,
    entries: Vec<Value>,
    event_name: String,
    execution_mode: String,
    handler_type: String,
    id: String,
    scope: String,
    source_path: String,
    started_at: i64,
    status: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    status_message: Field<String>,
} {
    source: String,
});
dto!(HookCompleted {
    run: HookRun,
    thread_id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    turn_id: Field<String>,
} {
});
dto!(ThreadGoalUpdated {
    goal: ThreadGoal,
    thread_id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    turn_id: Field<String>,
} {
});
dto!(ErrorNotification { error: TurnError, thread_id: String, turn_id: String, will_retry: bool } {  });
dto!(AccountRateLimitsUpdated { rate_limits: RateLimitSnapshot } {  });

#[derive(Clone, Debug, PartialEq)]
pub enum ServerNotification {
    AgentMessageDelta(ItemDelta),
    CommandOutputDelta(ItemDelta),
    ReasoningSummaryDelta(ItemDelta),
    ReasoningTextDelta(ItemDelta),
    PlanDelta(ItemDelta),
    ItemStarted(ItemNotification),
    ItemCompleted(ItemNotification),
    TurnStarted(TurnNotification),
    TurnCompleted(TurnNotification),
    TurnPlanUpdated(TurnPlanUpdated),
    ThreadClosed(ThreadNotification),
    GoalCleared(ThreadNotification),
    GoalUpdated(ThreadGoalUpdated),
    TokenUsageUpdated(ThreadTokenUsageUpdated),
    SettingsUpdated(ThreadSettingsUpdated),
    ModelRerouted(ModelRerouted),
    McpStartupStatusUpdated(McpStartupStatusUpdated),
    HookCompleted(HookCompleted),
    Error(ErrorNotification),
    RateLimitsUpdated(AccountRateLimitsUpdated),
    RequestResolved(RequestResolved),
    Unknown(Notification),
}

impl ServerNotification {
    pub fn decode(raw: &Notification) -> Result<Self> {
        macro_rules! decode {
            ($variant:ident) => {
                Self::$variant(raw.params.decode(&raw.method)?)
            };
        }
        Ok(match raw.method.as_str() {
            "item/agentMessage/delta" => decode!(AgentMessageDelta),
            "item/commandExecution/outputDelta" => decode!(CommandOutputDelta),
            "item/reasoning/summaryTextDelta" => decode!(ReasoningSummaryDelta),
            "item/reasoning/textDelta" => decode!(ReasoningTextDelta),
            "item/plan/delta" => decode!(PlanDelta),
            "item/started" => decode!(ItemStarted),
            "item/completed" => decode!(ItemCompleted),
            "turn/started" => decode!(TurnStarted),
            "turn/completed" => decode!(TurnCompleted),
            "turn/plan/updated" => decode!(TurnPlanUpdated),
            "thread/closed" => decode!(ThreadClosed),
            "thread/goal/cleared" => decode!(GoalCleared),
            "thread/goal/updated" => decode!(GoalUpdated),
            "thread/tokenUsage/updated" => decode!(TokenUsageUpdated),
            "thread/settings/updated" => decode!(SettingsUpdated),
            "model/rerouted" => decode!(ModelRerouted),
            "mcpServer/startupStatus/updated" => decode!(McpStartupStatusUpdated),
            "hook/completed" => decode!(HookCompleted),
            "error" => decode!(Error),
            "account/rateLimits/updated" => decode!(RateLimitsUpdated),
            "serverRequest/resolved" => decode!(RequestResolved),
            _ => Self::Unknown(raw.clone()),
        })
    }
}

dto!(CommandApprovalParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approval_id: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    command: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    command_actions: Field<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    cwd: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    environment_id: Field<String>,
    item_id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    network_approval_context: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    proposed_execpolicy_amendment: Field<Vec<String>>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    proposed_network_policy_amendments: Field<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    reason: Field<String>,
    started_at_ms: i64,
    thread_id: String,
    turn_id: String,
} {
    kind: String,
});
dto!(FileChangeApprovalParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    grant_root: Field<String>,
    item_id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    reason: Field<String>,
    started_at_ms: i64,
    thread_id: String,
    turn_id: String,
} {
});
dto!(PermissionsApprovalParams {
    cwd: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    environment_id: Field<String>,
    item_id: String,
    permissions: Value,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    reason: Field<String>,
    started_at_ms: i64,
    thread_id: String,
    turn_id: String,
} {
});
dto!(ApprovalResponse { decision: Value } {  });
dto!(PermissionsApprovalResponse {
    permissions: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(rename = "strictAutoReview")]
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    strict_auto_review: Field<bool>,
} {
});
dto!(UserInputOption { description: String, label: String } {  });
dto!(UserInputQuestion {
    header: String,
    id: String,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    options: Field<Vec<UserInputOption>>,
    question: String,
} {
    is_other: bool,
    is_secret: bool,
});
dto!(UserInputParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    auto_resolution_ms: Field<i64>,
    is_blocking: bool,
    item_id: String,
    questions: Vec<UserInputQuestion>,
    thread_id: String,
    turn_id: String,
} {
});
dto!(UserInputAnswer { answers: Vec<String> } {  });
dto!(UserInputResponse { answers: BTreeMap<String, UserInputAnswer> } {  });
open_union! { ElicitationParams ("mode") {
    Form = "form" {
        #[serde(rename = "serverName")]
        server_name: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId", default, skip_serializing_if = "Field::is_missing")]
        turn_id: Field<String>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Field::is_missing")]
        meta: Field<Value>,
        message: String,
        #[serde(rename = "requestedSchema")]
        requested_schema: Value,
    };
    OpenaiForm = "openai/form" {
        #[serde(rename = "serverName")]
        server_name: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId", default, skip_serializing_if = "Field::is_missing")]
        turn_id: Field<String>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Field::is_missing")]
        meta: Field<Value>,
        message: String,
        #[serde(rename = "requestedSchema")]
        requested_schema: Value,
    };
    LegacyOpenaiForm = "openaiForm" {
        #[serde(rename = "serverName")]
        server_name: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId", default, skip_serializing_if = "Field::is_missing")]
        turn_id: Field<String>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Field::is_missing")]
        meta: Field<Value>,
        message: String,
        #[serde(rename = "requestedSchema")]
        requested_schema: Value,
    };
    Url = "url" {
        #[serde(rename = "serverName")]
        server_name: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId", default, skip_serializing_if = "Field::is_missing")]
        turn_id: Field<String>,
        #[serde(rename = "_meta", default, skip_serializing_if = "Field::is_missing")]
        meta: Field<Value>,
        message: String,
        #[serde(rename = "elicitationId")]
        elicitation_id: String,
        url: String,
    };
} }

dto!(ElicitationResponse {
    action: String,
    #[serde(rename = "_meta")]
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    meta: Field<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    content: Field<Value>,
} {
});
dto!(LegacyExecApprovalParams {
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    approval_id: Field<String>,
    call_id: String,
    command: Vec<String>,
    conversation_id: String,
    cwd: String,
    parsed_cmd: Vec<Value>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    reason: Field<String>,
} {
});
dto!(LegacyPatchApprovalParams {
    call_id: String,
    conversation_id: String,
    file_changes: Object,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    grant_root: Field<String>,
    #[serde(default, skip_serializing_if = "Field::is_missing")]
    reason: Field<String>,
} {
});

#[derive(Clone, Debug, PartialEq)]
pub enum ServerRequestParams {
    CommandApproval(CommandApprovalParams),
    FileChangeApproval(FileChangeApprovalParams),
    PermissionsApproval(PermissionsApprovalParams),
    UserInput(UserInputParams),
    Elicitation(ElicitationParams),
    LegacyExecApproval(LegacyExecApprovalParams),
    LegacyPatchApproval(LegacyPatchApprovalParams),
    Unknown {
        method: String,
        params: crate::wire::Params,
    },
}

impl ServerRequestParams {
    pub fn decode(method: &str, params: crate::wire::Params) -> Result<Self> {
        Ok(match method {
            "item/commandExecution/requestApproval" => {
                Self::CommandApproval(params.decode(method)?)
            }
            "item/fileChange/requestApproval" => Self::FileChangeApproval(params.decode(method)?),
            "item/permissions/requestApproval" => Self::PermissionsApproval(params.decode(method)?),
            "item/tool/requestUserInput" => Self::UserInput(params.decode(method)?),
            "mcpServer/elicitation/request" => Self::Elicitation(params.decode(method)?),
            "execCommandApproval" => Self::LegacyExecApproval(params.decode(method)?),
            "applyPatchApproval" => Self::LegacyPatchApproval(params.decode(method)?),
            _ => Self::Unknown {
                method: method.into(),
                params,
            },
        })
    }
}

open_union! { Account ("type") {
    ApiKey = "apiKey" {  };
    Chatgpt = "chatgpt" {
        email: Option<String>,
        #[serde(rename = "planType")]
        plan_type: String,
    };
    AmazonBedrock = "amazonBedrock" {
        #[serde(rename = "usesCodexManagedCredentials")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uses_codex_managed_credentials: Option<bool>,
    };
} }
open_union! { ThreadStatus ("type") {
    NotLoaded = "notLoaded" {  };
    Idle = "idle" {  };
    SystemError = "systemError" {  };
    Active = "active" { #[serde(rename = "activeFlags")] active_flags: Vec<String> };
} }

dto!(FileChangeApprovalResponse { decision: Value } {  });
dto!(LegacyExecApprovalResponse { decision: Value } {  });
dto!(LegacyPatchApprovalResponse { decision: Value } {  });

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestResolved {
    pub request_id: crate::wire::RequestId,
    pub thread_id: String,
    #[serde(flatten)]
    pub extra: Object,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FunctionCallOutputBody {
    Text(String),
    Content(Vec<Value>),
    Other(Value),
}
