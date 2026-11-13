use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn subagent_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Spawn 2 subagents, one to read package.json and one to read tsconfig.json"}]}"#,
    )?)
}
