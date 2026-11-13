use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const ACTIVE_RUN_SHELL_SNAPSHOT_KEY: &str = r#"active-run-over-cancelled-latest"#;

pub fn queued_cancelled_while_active_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: first fixture turn complete"},{"type":"queue_message","text":"Respond with exactly: second fixture turn complete","createdBy":"agent","creationSource":"server"},{"type":"cancel_queued_run","targetRunIndex":2},{"type":"await_run_status","targetRunIndex":1,"status":"running"},{"type":"capture_shell_snapshot","key":"active-run-over-cancelled-latest"},{"type":"release_replay_gate","label":"turn/completed"}]}"#,
    )?)
}
