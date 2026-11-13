use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn tool_call_read_only_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Read /tmp/claude-replay-tool_call_read_only/package.json and /tmp/claude-replay-tool_call_read_only/tsconfig.json, then answer exactly: read only tool fixture complete"}]}"#,
    )?)
}
