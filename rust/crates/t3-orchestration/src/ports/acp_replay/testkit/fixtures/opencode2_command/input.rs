use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_command_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"/hello WORLD"}]}"#,
    )?)
}
