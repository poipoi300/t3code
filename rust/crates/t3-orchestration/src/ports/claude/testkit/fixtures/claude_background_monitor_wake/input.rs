#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_BACKGROUND_MONITOR_WAKE_PROMPT: &str = r###"Live-test a background Monitor wake. You must call the Monitor tool before replying. Do exactly this, with no extra steps.

1) Call the Monitor tool with description "Background monitor test" and this exact command:
   sleep 8 && echo MONITOR_DONE
2) After that tool call returns, reply with exactly STARTED and stop. Do not poll, read its output, or wait for it.
3) When its completion is reported later, reply with exactly WAKE_DONE and stop."###;

pub fn claude_background_monitor_wake_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Live-test a background Monitor wake. You must call the Monitor tool before replying. Do exactly this, with no extra steps.\n\n1) Call the Monitor tool with description \"Background monitor test\" and this exact command:\n   sleep 8 && echo MONITOR_DONE\n2) After that tool call returns, reply with exactly STARTED and stop. Do not poll, read its output, or wait for it.\n3) When its completion is reported later, reply with exactly WAKE_DONE and stop."},{"type":"await_run_status","targetRunIndex":2,"status":"running","waitForTurnItemType":"assistant_message"},{"type":"release_replay_gate","label":"result:background-wake:1"},{"type":"await_run_status","targetRunIndex":2,"status":"completed"}]}"###,
    )?)
}
