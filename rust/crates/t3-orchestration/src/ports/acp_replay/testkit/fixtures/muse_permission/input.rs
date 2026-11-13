use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const MUSE_PERMISSION_APPROVED_FILE: &str = r#"muse-approved.txt"#;

pub const MUSE_PERMISSION_DECLINED_FILE: &str = r#"muse-declined.txt"#;

pub fn muse_permission_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"runtimeMode":"approval-required","steps":[{"type":"message","text":"Run the shell command `touch muse-approved.txt` with your shell tool, then reply DONE."},{"type":"approve_next_runtime_request"},{"type":"message","text":"Run the shell command `touch muse-declined.txt` with your shell tool once. If it is rejected, do not retry; reply REJECTED."},{"type":"approve_next_runtime_request","decision":"decline"}]}"#,
    )?)
}
