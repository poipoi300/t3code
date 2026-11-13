use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const STOP_BACKGROUND_WORK_AFTER_FAILED_TURN_PROMPT: &str =
    r#"Start the dev server in the background with `bun run dev` and keep it running."#;

pub fn stop_background_work_after_failed_turn_input()
-> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Start the dev server in the background with `bun run dev` and keep it running."},{"type":"stop_background_work","targetRunIndex":1}]}"#,
    )?)
}

pub fn stop_background_work_after_release_input() -> crate::error::Result<OrchestratorFixtureInput>
{
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Start the dev server in the background with `bun run dev` and keep it running."},{"type":"advance_clock","duration":"31 minutes","releasesProviderSession":true},{"type":"stop_background_work","targetRunIndex":1}]}"#,
    )?)
}
