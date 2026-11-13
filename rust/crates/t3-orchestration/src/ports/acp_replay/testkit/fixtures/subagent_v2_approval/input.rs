use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const SUBAGENT_APPROVAL_PENDING_SHELL_KEY: &str = r#"subagent-approval-pending"#;

pub fn subagent_v2_approval_policy() -> serde_json::Value {
    serde_json::json!({"approvalPolicy": "untrusted", "sandboxPolicy": {"type": "readOnly"}})
}

pub fn subagent_v2_approval_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Do not run any commands yourself. Spawn one subagent whose only task is to run this exact shell command: printf 'subagent approval fixture' > subagent-approval.txt and then reply with exactly: Written. Wait for it to finish, then reply with exactly what it said."},{"type":"approve_next_runtime_request","shellSnapshotKeyWhilePending":"subagent-approval-pending"}]}"#,
    )?)
}
