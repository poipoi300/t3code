use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const GROK_PROMPT_ERROR_FOLLOW_UP: &str =
    r#"Respond with exactly: grok prompt error fixture recovered"#;

pub const GROK_PROMPT_ERROR_PROMPT: &str = r#"Respond with exactly: grok prompt error fixture"#;

pub fn grok_prompt_error_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: grok prompt error fixture"},{"type":"message","text":"Respond with exactly: grok prompt error fixture recovered"}]}"#,
    )?)
}
