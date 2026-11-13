use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn proposed_plan_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"interactionMode":"plan","steps":[{"type":"message","text":"Create a short implementation plan for adding deterministic replay fixtures. Do not ask questions. Present the final plan in a proposed plan block."}]}"#,
    )?)
}
