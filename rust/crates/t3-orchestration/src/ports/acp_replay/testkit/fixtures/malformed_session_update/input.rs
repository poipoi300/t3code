use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const MALFORMED_SESSION_UPDATE_PROMPT: &str =
    r#"Respond with the following text: malformed update fixture ok"#;

pub fn malformed_session_update_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with the following text: malformed update fixture ok"}]}"#,
    )?)
}
