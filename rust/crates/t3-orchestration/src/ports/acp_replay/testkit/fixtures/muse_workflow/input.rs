use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn muse_workflow_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use your workflow tool to launch a workflow with exactly TWO agents: one labeled 'ping' that replies with the word PING, and one labeled 'pong' that replies with the word PONG. Launch it and end your turn; do not wait for it."},{"type":"await_run_status","targetRunIndex":2,"status":"completed"}]}"#,
    )?)
}
