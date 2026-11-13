use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn fx_early_stop_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Do not answer immediately. First run the local shell command `sleep 30`, then respond with exactly: interrupt fixture should not finish naturally."},{"type":"interrupt","targetRunIndex":1},{"type":"message","text":"Respond with the following text: fixture simple ok"}]}"#,
    )?)
}
