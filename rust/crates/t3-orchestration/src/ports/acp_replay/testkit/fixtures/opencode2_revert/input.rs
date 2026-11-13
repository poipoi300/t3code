use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_revert_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Create a file named reverted.txt containing the word ALPHA using the write tool, then reply DONE."},{"type":"message","text":"Overwrite reverted.txt so it contains the word BETA using the write tool, then reply DONE."},{"type":"rollback","checkpointScopeSuffix":"root","checkpointSuffix":"1"},{"type":"await_run_status","targetRunIndex":2,"status":"rolled_back"}]}"#,
    )?)
}
