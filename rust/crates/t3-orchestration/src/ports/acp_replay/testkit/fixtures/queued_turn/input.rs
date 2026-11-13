use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn queued_turn_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: first fixture turn complete"},{"type":"queue_message","text":"Respond with exactly: second fixture turn complete"}]}"#,
    )?)
}
