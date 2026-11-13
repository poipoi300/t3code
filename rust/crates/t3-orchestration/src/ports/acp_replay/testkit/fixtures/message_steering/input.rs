use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn message_restart_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: steering fixture initial response"},{"type":"restart","text":"Actually, respond with exactly: steering fixture observed","targetRunIndex":1}]}"#,
    )?)
}

pub fn message_steering_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: steering fixture initial response"},{"type":"steer","text":"Actually, respond with exactly: steering fixture observed","targetRunIndex":1}]}"#,
    )?)
}
