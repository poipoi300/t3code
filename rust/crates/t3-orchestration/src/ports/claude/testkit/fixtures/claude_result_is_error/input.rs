#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn claude_result_is_error_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Say hello before the credentials expire."},{"type":"message","text":"Try again now that auth is back."}]}"###,
    )?)
}
