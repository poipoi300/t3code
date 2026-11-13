use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn grok_subagent_lineage_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"audit this codebase"}]}"#,
    )?)
}
