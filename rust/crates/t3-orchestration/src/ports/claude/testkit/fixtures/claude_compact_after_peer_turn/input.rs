#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_COMPACT_FIRST_PROMPT: &str = r###"Reply with exactly: compact probe first turn"###;

pub fn claude_compact_after_peer_turn_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Reply with exactly: compact probe first turn"},{"type":"message","text":"/compact"},{"type":"await_run_status","targetRunIndex":3,"status":"completed"},{"type":"release_replay_gate","label":"command_lifecycle:compact-completed"}]}"###,
    )?)
}

pub fn claude_compact_after_peer_turn_no_echo_input()
-> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Reply with exactly: compact probe first turn"},{"type":"message","text":"/compact"}]}"###,
    )?)
}
