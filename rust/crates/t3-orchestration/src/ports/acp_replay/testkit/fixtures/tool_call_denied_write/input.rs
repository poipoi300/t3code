use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn denied_write_policy() -> serde_json::Value {
    serde_json::json!({"approvalPolicy": "on-request", "sandboxPolicy": {"type": "workspaceWrite", "writableRoots": [], "readOnlyAccess": {"type": "fullAccess"}, "networkAccess": false}})
}

pub const TOOL_CALL_DENIED_WRITE_PROMPT: &str = r#"Use the Write tool once to create .claude-denied-write.txt containing exactly: approval fixture. If permission is denied, do not retry or use another tool; reply exactly: write permission denied. Do not read any files, use the shell, network, or MCP."#;

pub const TOOL_CALL_DENIED_WRITE_TARGET: &str = r#".claude-denied-write.txt"#;

pub fn tool_call_denied_write_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use the Write tool once to create .claude-denied-write.txt containing exactly: approval fixture. If permission is denied, do not retry or use another tool; reply exactly: write permission denied. Do not read any files, use the shell, network, or MCP."},{"type":"approve_next_runtime_request","decision":"decline"}]}"#,
    )?)
}
