use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn tool_call_restricted_granular_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Create or overwrite .codex-probe-write-action.txt with exactly this text: codex app-server approval fixture. Use a local shell command or file edit only, then briefly report what happened. Do not read package metadata, use GitHub, use web, or use MCP."},{"type":"approve_next_runtime_request"}]}"#,
    )?)
}
