use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_inbox_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Run the shell command `sleep 12 && echo A` with the bash tool, then reply DONE_A."},{"type":"queue_message","text":"Reply exactly QUEUED_B."},{"type":"queue_message","text":"Reply exactly QUEUED_C."},{"type":"cancel_queued_run","targetRunIndex":3},{"type":"steer","text":"Also mention the word STEERED in your final reply.","targetRunIndex":1}]}"#,
    )?)
}
