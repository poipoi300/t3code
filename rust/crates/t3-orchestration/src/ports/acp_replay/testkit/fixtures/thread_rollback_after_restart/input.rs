use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn thread_rollback_after_restart_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: rollback fixture first turn complete"},{"type":"message","text":"Respond with exactly: rollback fixture second turn complete"},{"type":"advance_clock","duration":"31 minutes"},{"type":"rollback","checkpointScopeSuffix":"root","checkpointSuffix":"1"},{"type":"message","text":"Repeat the conversation verbatim."}]}"#,
    )?)
}
