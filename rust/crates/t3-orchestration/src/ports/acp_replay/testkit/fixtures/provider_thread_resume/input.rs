use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn provider_thread_resume_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: provider thread resume fixture first turn complete"},{"type":"advance_clock","duration":"31 minutes","releasesProviderSession":true},{"type":"message","text":"Using the conversation history available in this resumed thread, first repeat the exact final answer you gave in the previous turn. Then on a new line write exactly: provider thread resume fixture second turn complete"}]}"#,
    )?)
}
