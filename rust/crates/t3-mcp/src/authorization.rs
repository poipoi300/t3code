use crate::{AccessRule, McpError, Result, access::tool_rule, catalog::tool};
use serde_json::Value;
use t3_contracts::auth::AuthEnvironmentScope as Scope;

pub fn required_scopes(name: &str, input: &Value) -> Result<Vec<Scope>> {
    if tool(name).is_none() {
        return Err(McpError::InvalidInput(format!("Tool '{name}' not found")));
    }
    let mut scopes = vec![Scope::OrchestrationRead];
    if !matches!(
        tool_rule(name, input),
        AccessRule::Reads | AccessRule::ReadsAsCaller
    ) {
        scopes.push(Scope::OrchestrationOperate);
    }
    if name == "t3_environment_preferences_update"
        || matches!(name, "t3_project_create" | "t3_project_update")
            && input.get("scripts").is_some()
    {
        scopes.push(Scope::SettingsWrite);
    }
    if matches!(name, "write_pull_request_guides" | "t3_worktree_handoff") {
        scopes.push(Scope::SourceControlWrite);
    }
    Ok(scopes)
}

pub fn authorize_tool(granted: &[Scope], name: &str, input: &Value) -> Result<()> {
    for required in required_scopes(name, input)? {
        if !granted.contains(&required) {
            return Err(McpError::failure(
                "capability_denied",
                format!(
                    "The authenticated token is missing required scope: {}.",
                    serde_json::to_value(required)
                        .map_err(|_| McpError::Internal)?
                        .as_str()
                        .ok_or(McpError::Internal)?
                ),
            ));
        }
    }
    Ok(())
}
