use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use t3_contracts::{
    base_schemas::{EnvironmentId, ThreadId},
    provider_instance::ProviderInstanceId,
    provider_policy::{ProviderInteractionMode, RuntimeMode},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Preview,
    Orchestration,
    Worktree,
    Device,
    PullRequests,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCaller {
    pub thread_id: ThreadId,
    pub provider_session_id: String,
    pub provider_instance_id: ProviderInstanceId,
    #[serde(default)]
    pub subagent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientAccess {
    ReadOnly,
    ApprovalRequired,
    AutoAcceptEdits,
    Auto,
    FullAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Modes {
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
}

impl Modes {
    pub fn full() -> Self {
        Self {
            runtime_mode: RuntimeMode::FullAccess,
            interaction_mode: ProviderInteractionMode::Default,
        }
    }
    pub fn narrow(self, other: Self) -> Self {
        Self {
            runtime_mode: if runtime_rank(self.runtime_mode) < runtime_rank(other.runtime_mode) {
                self.runtime_mode
            } else {
                other.runtime_mode
            },
            interaction_mode: if self.interaction_mode == ProviderInteractionMode::Plan
                || other.interaction_mode == ProviderInteractionMode::Plan
            {
                ProviderInteractionMode::Plan
            } else {
                ProviderInteractionMode::Default
            },
        }
    }
}

pub fn runtime_rank(mode: RuntimeMode) -> u8 {
    match mode {
        RuntimeMode::ApprovalRequired => 0,
        RuntimeMode::AutoAcceptEdits => 1,
        RuntimeMode::Auto => 2,
        RuntimeMode::FullAccess => 3,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCaller {
    pub session_id: String,
    pub label: String,
    pub access: ClientAccess,
    #[serde(default)]
    pub linked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrowed_to: Option<Modes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_thread: Option<serde_json::Value>,
}

impl ClientCaller {
    pub fn ceiling(&self) -> Modes {
        let runtime_mode = match self.access {
            ClientAccess::ReadOnly | ClientAccess::ApprovalRequired => {
                RuntimeMode::ApprovalRequired
            }
            ClientAccess::AutoAcceptEdits => RuntimeMode::AutoAcceptEdits,
            ClientAccess::Auto => RuntimeMode::Auto,
            ClientAccess::FullAccess => RuntimeMode::FullAccess,
        };
        let approved = Modes {
            runtime_mode,
            interaction_mode: ProviderInteractionMode::Default,
        };
        self.narrowed_to
            .map_or(approved, |asked| approved.narrow(asked))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationScope {
    pub environment_id: EnvironmentId,
    pub request_namespace: String,
    pub issued_at: i64,
    pub capabilities: BTreeSet<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadCaller>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientCaller>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_environments: Option<bool>,
}

impl InvocationScope {
    pub fn require_thread(&self, operation: &str) -> crate::Result<&ThreadCaller> {
        self.thread.as_ref().ok_or_else(|| crate::McpError::failure("thread_credential_required", format!("{operation} acts as the calling T3 thread, so it needs an agent running inside T3 Code. This MCP client signed in from outside a thread.")))
    }
    pub fn require_capability(&self, capability: Capability) -> crate::Result<()> {
        if self.capabilities.contains(&capability) {
            return Ok(());
        }
        let tag = if capability == Capability::Preview {
            "PreviewAutomationUnavailableError"
        } else {
            "McpCapabilityUnavailableError"
        };
        let capability_name = serde_json::to_value(capability)
            .expect("capability serializes")
            .as_str()
            .expect("capability string")
            .to_owned();
        let mut fields =
            serde_json::json!({"capability":capability,"environmentId":self.environment_id});
        if let Some(thread) = &self.thread {
            fields["threadId"] = serde_json::json!(thread.thread_id);
            fields["providerSessionId"] = serde_json::json!(thread.provider_session_id);
            fields["providerInstanceId"] = serde_json::json!(thread.provider_instance_id);
        }
        let message = if capability == Capability::Preview {
            crate::catalog::catalog().capability_messages["preview"].clone()
        } else {
            crate::catalog::catalog().capability_messages["generic"]
                .replace("__CAPABILITY__", &capability_name)
        };
        Err(crate::McpError::Domain {
            tag: tag.into(),
            message,
            fields,
        })
    }
}
