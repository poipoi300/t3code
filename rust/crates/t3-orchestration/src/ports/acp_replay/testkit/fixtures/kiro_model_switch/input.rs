use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const KIRO_MODEL_SWITCH_FIRST_PROMPT: &str = r#"Respond with exactly: kiro model switch first"#;

pub const KIRO_MODEL_SWITCH_SECOND_PROMPT: &str =
    r#"Respond with exactly: kiro model switch second"#;

pub const KIRO_MODEL_SWITCH_TARGET: &str = r#"claude-sonnet-4.5"#;

pub fn kiro_model_switch_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: kiro model switch first"},{"type":"set_model","model":"claude-sonnet-4.5"},{"type":"message","text":"Respond with exactly: kiro model switch second"}]}"#,
    )?)
}
