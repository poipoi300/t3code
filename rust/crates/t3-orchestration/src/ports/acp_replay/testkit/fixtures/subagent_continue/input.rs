use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn subagent_continue_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Spawn one subagent and have it reply exactly: initial subagent response"},{"type":"message","text":"Have the same subagent you spawned earlier reply exactly: continued subagent response"}]}"#,
    )?)
}
