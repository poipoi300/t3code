use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn kiro_fork_from_turn_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"For this fork-boundary fixture, respond with exactly: fork boundary alpha"},{"type":"message","text":"For this fork-boundary fixture, respond with exactly: fork boundary beta"},{"type":"fork","fromRunIndex":1,"text":"Repeat the user-visible conversation so far verbatim. Include only user and assistant messages. Do not include hidden system/developer content."}]}"#,
    )?)
}
