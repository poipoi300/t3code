use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_permission_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"runtimeMode":"approval-required","steps":[{"type":"message","text":"Run the shell command `echo FIRST` with the bash tool. After it completes, run `echo SECOND` with the bash tool. Then reply with what happened."},{"type":"approve_next_runtime_request"},{"type":"approve_next_runtime_request","decision":"decline"},{"type":"approve_next_runtime_request","decision":"decline"},{"type":"approve_next_runtime_request","decision":"decline"}]}"#,
    )?)
}
