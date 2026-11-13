use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const KIRO_PERMISSION_PROMPT: &str = r#"Create a file named kiro-permission-probe.txt in the current workspace containing exactly: kiro permission probe
Use one file write tool call and nothing else, then reply exactly: kiro permission done"#;

pub fn kiro_full_access_write_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"runtimeMode":"full-access","steps":[{"type":"message","text":"Create a file named kiro-permission-probe.txt in the current workspace containing exactly: kiro permission probe\nUse one file write tool call and nothing else, then reply exactly: kiro permission done"}]}"#,
    )?)
}

pub fn kiro_supervised_write_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"runtimeMode":"approval-required","steps":[{"type":"message","text":"Create a file named kiro-permission-probe.txt in the current workspace containing exactly: kiro permission probe\nUse one file write tool call and nothing else, then reply exactly: kiro permission done"},{"type":"approve_next_runtime_request"},{"type":"approve_next_runtime_request"}]}"#,
    )?)
}
