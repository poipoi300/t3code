use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn muse_subagent_interrupt_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Spawn one subagent whose only task is to run the shell command `sleep 30` and then reply with exactly: Done. Wait for it to finish, then reply with exactly what it said."},{"type":"interrupt","targetRunIndex":1,"waitForTurnItemType":"subagent"}]}"#,
    )?)
}
