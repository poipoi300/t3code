#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn claude_local_bash_task_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Run a local Bash typecheck command, then answer exactly: claude local bash task fixture complete"}]}"###,
    )?)
}
