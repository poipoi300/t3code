use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const GROK_BACKGROUND_SUBAGENT_PROMPT: &str = r#"Spawn one background subagent (run it in the background, do not wait for it) whose task is: run the shell command 'sleep 20' and then reply exactly SUBAGENT_DONE. As soon as the subagent is launched, end your turn by replying exactly ROOT_DONE without waiting for it."#;

pub fn grok_background_subagent_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Spawn one background subagent (run it in the background, do not wait for it) whose task is: run the shell command 'sleep 20' and then reply exactly SUBAGENT_DONE. As soon as the subagent is launched, end your turn by replying exactly ROOT_DONE without waiting for it."},{"type":"finish_held_run","targetRunIndex":1,"status":"completed"},{"type":"release_replay_gate","label":"notification:session/update:agent_message_chunk:subagent-completed-00000000-0000-4000-8000-000000000002"},{"type":"finish_held_run","targetRunIndex":2,"status":"completed"}]}"#,
    )?)
}
