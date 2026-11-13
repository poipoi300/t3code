use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const OPENCODE_RUNNING_CHILD_APPROVAL_PENDING_SHELL_KEY: &str =
    r#"opencode-running-child-approval-pending"#;

pub fn open_code_running_child_approval_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use the task tool exactly once. Delegate to the general subagent with this prompt: Respond exactly CHILD_OK. After the task completes, respond exactly PARENT_OK."},{"type":"approve_next_runtime_request","shellSnapshotKeyWhilePending":"opencode-running-child-approval-pending"}]}"#,
    )?)
}
