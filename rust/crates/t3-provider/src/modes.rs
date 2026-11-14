use serde_json::{Value, json};
use t3_contracts::provider_policy::{ProviderInteractionMode, RuntimeMode};

pub fn codex(mode: RuntimeMode, approval: Option<&Value>, sandbox: Option<&Value>) -> Value {
    let (policy, reviewer, sandbox_kind) = match mode {
        RuntimeMode::ApprovalRequired => ("untrusted", "user", "readOnly"),
        RuntimeMode::AutoAcceptEdits => ("on-request", "user", "workspaceWrite"),
        RuntimeMode::Auto => ("on-request", "auto_review", "workspaceWrite"),
        RuntimeMode::FullAccess => ("never", "user", "dangerFullAccess"),
    };
    json!({"approvalPolicy":approval.cloned().unwrap_or_else(||json!(policy)),"approvalsReviewer":reviewer,"sandboxPolicy":sandbox.cloned().unwrap_or_else(||json!({"type":sandbox_kind}))})
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeMode {
    pub permission_mode: &'static str,
    pub install_permission_callback: bool,
    pub allow_dangerously_skip_permissions: bool,
    pub read_only: bool,
}
pub fn claude(
    mode: RuntimeMode,
    interaction: ProviderInteractionMode,
    approval: Option<&str>,
    sandbox: Option<&str>,
    bypass_disabled: bool,
    auto_disabled: bool,
) -> ClaudeMode {
    let requested = if interaction == ProviderInteractionMode::Plan {
        "plan"
    } else if approval == Some("never") {
        match sandbox {
            Some("readOnly") => "dontAsk",
            Some("dangerFullAccess" | "externalSandbox") => "bypassPermissions",
            _ => match mode {
                RuntimeMode::ApprovalRequired => "dontAsk",
                RuntimeMode::AutoAcceptEdits => "acceptEdits",
                _ => "bypassPermissions",
            },
        }
    } else if approval.is_some() {
        "default"
    } else {
        match sandbox {
            Some("readOnly") => "dontAsk",
            Some("dangerFullAccess") if mode != RuntimeMode::ApprovalRequired => {
                "bypassPermissions"
            }
            _ => match mode {
                RuntimeMode::ApprovalRequired => "default",
                RuntimeMode::AutoAcceptEdits => "acceptEdits",
                RuntimeMode::Auto => "auto",
                RuntimeMode::FullAccess => "bypassPermissions",
            },
        }
    };
    let downgraded = requested == "bypassPermissions" && bypass_disabled;
    let permission_mode = if downgraded {
        if auto_disabled { "default" } else { "auto" }
    } else {
        requested
    };
    ClaudeMode {
        permission_mode,
        install_permission_callback: permission_mode == "plan"
            || downgraded
            || approval.map_or(
                matches!(
                    mode,
                    RuntimeMode::ApprovalRequired | RuntimeMode::AutoAcceptEdits
                ),
                |s| s != "never",
            ),
        allow_dangerously_skip_permissions: permission_mode == "bypassPermissions",
        read_only: sandbox == Some("readOnly"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorMode {
    pub auto_review: bool,
    pub sandbox_enabled: bool,
    pub mode: &'static str,
}
pub fn cursor(
    mode: RuntimeMode,
    interaction: ProviderInteractionMode,
    approval: Option<&str>,
    sandbox: Option<&str>,
    platform: crate::environment::Platform,
) -> CursorMode {
    let auto_review = approval.map_or(
        matches!(mode, RuntimeMode::ApprovalRequired | RuntimeMode::Auto),
        |approval| approval != "never",
    );
    CursorMode {
        auto_review,
        sandbox_enabled: sandbox.map_or(
            mode != RuntimeMode::FullAccess
                && !(platform == crate::environment::Platform::Windows && auto_review),
            |sandbox| sandbox != "dangerFullAccess",
        ),
        mode: if interaction == ProviderInteractionMode::Plan {
            "plan"
        } else {
            "agent"
        },
    }
}
