use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_interrupt_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Run the shell command `sleep 60 && echo LATE` with the bash tool, then reply DONE."},{"type":"interrupt","targetRunIndex":1,"waitForTurnItemType":"command_execution"}]}"#,
    )?)
}
