use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_background_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use the subagent tool with background enabled to delegate to the general subagent with the prompt: 'Run the shell command `sleep 20` with the bash tool and then reply exactly CHILD_OK.' As soon as it is launched, reply exactly PARENT_OK and end your turn without waiting for it."},{"type":"await_run_status","targetRunIndex":2,"status":"completed"}]}"#,
    )?)
}
