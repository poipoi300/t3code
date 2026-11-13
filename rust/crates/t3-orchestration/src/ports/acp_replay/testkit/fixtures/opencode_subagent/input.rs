use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code_subagent_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use the task tool exactly once. Delegate to the general subagent with this prompt: Respond exactly CHILD_OK. After the task completes, respond exactly PARENT_OK."}]}"#,
    )?)
}
