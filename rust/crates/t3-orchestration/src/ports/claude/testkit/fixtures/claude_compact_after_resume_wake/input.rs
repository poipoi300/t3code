#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn claude_compact_after_resume_wake_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Reply with exactly: compact probe first turn"},{"type":"advance_clock","duration":"31 minutes","releasesProviderSession":true},{"type":"message","text":"/compact"}]}"###,
    )?)
}
