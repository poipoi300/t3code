use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn simple_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with the following text: fixture simple ok"}]}"#,
    )?)
}
