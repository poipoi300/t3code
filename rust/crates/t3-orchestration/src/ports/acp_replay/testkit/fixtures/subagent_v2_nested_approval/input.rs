use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const NESTED_SUBAGENT_APPROVAL_PENDING_SHELL_KEY: &str = r#"nested-subagent-approval-pending"#;

pub fn subagent_v2_nested_approval_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Do not run any commands yourself. Spawn one subagent and tell it not to run any commands itself but to spawn its own subagent, whose only task is to run this exact shell command: printf 'nested approval fixture' > nested-approval.txt and then reply with exactly: Written. Each agent waits for its child and replies with exactly what the child said. Wait for your subagent, then reply with exactly what it said."},{"type":"approve_next_runtime_request","shellSnapshotKeyWhilePending":"nested-subagent-approval-pending"}]}"#,
    )?)
}
