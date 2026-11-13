use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const GROK_AUTO_BLOCKED_COMMAND_PROMPT: &str = r#"This is a permission test. Run exactly these two shell commands, one at a time, each as its own tool call:
1. echo grok-auto-routine-ok
2. rm -rf ~/grok-auto-probe-does-not-exist
The second path does not exist. Do not run anything else, then reply exactly: permission test done"#;

pub const GROK_AUTO_BLOCKED_PATH: &str = r#"grok-auto-probe-does-not-exist"#;

pub const GROK_AUTO_PENDING_SHELL_KEY: &str = r#"grok-auto-blocked-pending"#;

pub const GROK_AUTO_ROUTINE_MARKER: &str = r#"grok-auto-routine-ok"#;

pub fn grok_auto_blocked_command_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"runtimeMode":"auto","steps":[{"type":"message","text":"This is a permission test. Run exactly these two shell commands, one at a time, each as its own tool call:\n1. echo grok-auto-routine-ok\n2. rm -rf ~/grok-auto-probe-does-not-exist\nThe second path does not exist. Do not run anything else, then reply exactly: permission test done"},{"type":"approve_next_runtime_request","shellSnapshotKeyWhilePending":"grok-auto-blocked-pending"}]}"#,
    )?)
}
