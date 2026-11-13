use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn command_output_streaming_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Run this exact shell command in the foreground and wait for it to finish: `for i in 1 2 3 4; do printf '\\033[32mstream line %s\\033[0m\\n' $i; sleep 1; done; printf 'progress 50%%\\rprogress 100%%\\n'`. Then respond exactly: command output fixture complete"}]}"#,
    )?)
}
