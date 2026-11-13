use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn muse_subagent_approval_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"runtimeMode":"approval-required","steps":[{"type":"message","text":"Do not run any commands yourself. Spawn one subagent whose only task is to run this exact shell command: printf 'subagent approval fixture' > subagent-approval.txt and then reply with exactly: Written. Wait for it to finish, then reply with exactly what it said."},{"type":"approve_next_runtime_request"}]}"#,
    )?)
}
