use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const KIRO_MESSAGE_STEERING_PROMPT: &str = r#"Run the shell command `sleep 5` once, then respond with exactly: steering fixture initial response"#;

pub fn kiro_message_steering_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"runtimeMode":"full-access","steps":[{"type":"message","text":"Run the shell command `sleep 5` once, then respond with exactly: steering fixture initial response"},{"type":"steer","text":"Actually, respond with exactly: steering fixture observed","targetRunIndex":1}]}"#,
    )?)
}
