use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_subagent_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use the subagent tool to delegate to the explore subagent with the prompt: 'List the files in the current directory and report their names.' Wait for it, then summarize its answer in one line."}]}"#,
    )?)
}
