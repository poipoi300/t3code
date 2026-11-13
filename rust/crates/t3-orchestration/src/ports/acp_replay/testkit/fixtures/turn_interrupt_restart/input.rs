use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn turn_interrupt_restart_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Run this exact local command: `node -e \"console.log('interrupt fixture tool started'); setTimeout(() => {}, 30000)\"`. Do not answer until it completes, then respond exactly: interrupt fixture should not finish naturally."},{"type":"interrupt","targetRunIndex":1,"waitForTurnItemType":"command_execution"},{"type":"message","text":"Respond with exactly: interrupt recovery fixture complete"}]}"#,
    )?)
}
