use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn grok_background_bash_fast_wake_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Start this shell command as a background task (run it in the background, do not wait for it, do not use the Monitor tool): 'for i in 1 2 3; do sleep 8; echo tock $i; done'. As soon as it has started, end your turn by replying exactly ROOT_DONE. When it finishes, reply exactly with its last line."},{"type":"finish_held_run","targetRunIndex":1,"status":"completed"},{"type":"finish_held_run","targetRunIndex":2,"status":"completed"}]}"#,
    )?)
}
