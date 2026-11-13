use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_tool_call_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use the read tool to read hello.txt, then run the shell command `echo TOOL_OK` with the bash tool, then reply DONE."}]}"#,
    )?)
}
