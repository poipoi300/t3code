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

pub fn opencode_v1(
    mode: RuntimeMode,
    approval: Option<&Value>,
    sandbox: Option<&Value>,
) -> Vec<Value> {
    let kind = sandbox.and_then(|sandbox| sandbox["type"].as_str());
    let approval_text = approval
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let ask = approval_text.map_or(
        approval.is_some_and(Value::is_object) || mode != RuntimeMode::FullAccess,
        |approval| approval != "never",
    );
    let rule = |permission: &str, pattern: &str, action: &str| json!({"permission":permission,"pattern":pattern,"action":action});
    if !ask
        && (matches!(kind, Some("externalSandbox" | "dangerFullAccess"))
            || kind.is_none() && mode == RuntimeMode::FullAccess)
    {
        return vec![rule("*", "*", "allow")];
    }
    let restricted = [
        "bash",
        "edit",
        "webfetch",
        "websearch",
        "codesearch",
        "external_directory",
        "doom_loop",
    ];
    let mut rules = vec![rule("*", "*", "deny")];
    rules.extend(
        restricted
            .iter()
            .map(|permission| rule(permission, "*", "deny")),
    );
    if ask {
        rules.push(rule("*", "*", "ask"));
        rules.extend(
            restricted
                .iter()
                .map(|permission| rule(permission, "*", "ask")),
        );
    }
    rules.extend(
        [
            "question",
            "read",
            "glob",
            "grep",
            "lsp",
            "todowrite",
            "task",
            "skill",
        ]
        .iter()
        .map(|permission| rule(permission, "*", "allow")),
    );
    rules.extend([
        rule("read", "*.env", if ask { "ask" } else { "deny" }),
        rule("read", "*.env.*", if ask { "ask" } else { "deny" }),
        rule("read", "*.env.example", "allow"),
    ]);
    if mode == RuntimeMode::AutoAcceptEdits || !ask && kind == Some("workspaceWrite") {
        rules.push(rule("edit", "*", "allow"));
    }
    if !ask && sandbox.is_some_and(|sandbox| sandbox["networkAccess"] == true) {
        rules.extend(
            ["webfetch", "websearch", "codesearch"]
                .iter()
                .map(|permission| rule(permission, "*", "allow")),
        );
    }
    if !ask
        && kind == Some("readOnly")
        && sandbox.is_some_and(|sandbox| sandbox["access"]["type"] == "fullAccess")
    {
        rules.push(rule("external_directory", "*", "allow"));
    }
    if !ask
        && kind == Some("workspaceWrite")
        && let Some(roots) = sandbox.and_then(|sandbox| sandbox["writableRoots"].as_array())
    {
        rules.extend(
            roots
                .iter()
                .filter_map(Value::as_str)
                .filter(|root| !root.trim().is_empty())
                .map(|root| {
                    rule(
                        "external_directory",
                        &format!("{}/*", root.strip_suffix('/').unwrap_or(root)),
                        "allow",
                    )
                }),
        );
    }
    rules
}

pub fn opencode_v2(
    mode: RuntimeMode,
    interaction: ProviderInteractionMode,
    grants: &[Value],
    paths: &[Value],
) -> Vec<Value> {
    let rule = |action: &str, effect: &str| json!({"action":action,"effect":effect,"resource":"*"});
    let mut rules = if mode == RuntimeMode::FullAccess {
        vec![rule("*", "allow")]
    } else {
        vec![
            rule("shell", "ask"),
            rule(
                "edit",
                if mode == RuntimeMode::AutoAcceptEdits {
                    "allow"
                } else {
                    "ask"
                },
            ),
            rule("external_directory", "ask"),
        ]
    };
    rules.extend_from_slice(grants);
    if interaction == ProviderInteractionMode::Plan {
        rules.push(rule("edit", "deny"));
    }
    rules.extend_from_slice(paths);
    rules
}
