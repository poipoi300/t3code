use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn subagent_v2_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Spawn one subagent whose only task is to reply with exactly: Hello. Wait for it to finish, then reply with exactly what it said."}]}"#,
    )?)
}

pub fn subagent_v2_nested_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Spawn one subagent and tell it to spawn its own subagent, which must in turn spawn one more subagent whose only task is to reply with exactly: Hello. Each agent waits for its child and replies with exactly what the child said. Wait for your subagent, then reply with exactly what it said."}]}"#,
    )?)
}
