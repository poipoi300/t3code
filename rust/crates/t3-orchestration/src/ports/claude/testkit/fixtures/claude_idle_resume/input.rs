#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_IDLE_RESUME_PROMPT_1: &str =
    r###"Respond with exactly: idle resume first turn complete"###;

pub const CLAUDE_IDLE_RESUME_PROMPT_2: &str =
    r###"Respond with exactly: idle resume second turn complete"###;

pub fn claude_idle_resume_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Respond with exactly: idle resume first turn complete"},{"type":"advance_clock","duration":"31 minutes","releasesProviderSession":true},{"type":"message","text":"Respond with exactly: idle resume second turn complete"}]}"###,
    )?)
}
