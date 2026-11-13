use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const GROK_BACKGROUND_BASH_PROMPT: &str = r#"Start this shell command as a background task (run it in the background, do not wait for it, do not use the Monitor tool): 'for i in 1 2 3; do sleep 8; echo tock $i; done'. As soon as it has started, end your turn by replying exactly ROOT_DONE. When it finishes, reply exactly with its last line."#;

pub const GROK_BACKGROUND_BASH_TICKS: &[&str] = &[r#"tock 1"#, r#"tock 2"#, r#"tock 3"#];

pub fn grok_background_bash_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Start this shell command as a background task (run it in the background, do not wait for it, do not use the Monitor tool): 'for i in 1 2 3; do sleep 8; echo tock $i; done'. As soon as it has started, end your turn by replying exactly ROOT_DONE. When it finishes, reply exactly with its last line."},{"type":"finish_held_run","targetRunIndex":1,"status":"completed"},{"type":"release_replay_gate","label":"notification:session/update:agent_thought_chunk:task-completed-00000000-0000-4000-8000-000000000002"},{"type":"finish_held_run","targetRunIndex":2,"status":"completed"}]}"#,
    )?)
}
