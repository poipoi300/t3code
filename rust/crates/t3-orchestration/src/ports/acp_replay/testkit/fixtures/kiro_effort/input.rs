use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const KIRO_EFFORT_LEVEL: &str = r#"high"#;

pub const KIRO_EFFORT_MODEL: &str = r#"claude-opus-5.5"#;

pub const KIRO_EFFORT_PROMPT: &str = r#"Respond with exactly: kiro effort"#;

pub fn kiro_effort_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Respond with exactly: kiro effort"}]}"#,
    )?)
}
