use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_resume_after_restart_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Run the shell command `sleep 25 && echo RESUMED` with the bash tool, then reply with its output."},{"type":"message","text":"What did I last ask you to run? Answer in one short sentence."}]}"#,
    )?)
}
