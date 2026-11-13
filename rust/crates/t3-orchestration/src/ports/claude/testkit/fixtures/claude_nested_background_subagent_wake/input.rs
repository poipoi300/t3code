#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_NESTED_BACKGROUND_SUBAGENT_WAKE_PROMPT: &str = r###"Live-test a background subagent that starts its own background subagent. Do exactly this, with no extra steps.

1) Use the Agent tool with subagent_type "general-purpose" and run_in_background set to true to launch ONE subagent with this prompt:
   "Do exactly this. a) Use the Agent tool with subagent_type \"general-purpose\" and run_in_background set to true to launch ONE subagent with the prompt: Reply with exactly GRANDCHILD_DONE. b) Run this exact command with the Bash tool: sleep 20 && echo CHILD_WAITED. c) Reply with exactly CHILD_DONE."
2) Immediately after launching it, reply with exactly ROOT_STARTED and stop. Do not wait for it or check on it.
3) When its completion is reported, reply with exactly ROOT_WAKE_DONE and stop."###;

pub fn claude_nested_background_subagent_wake_input()
-> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Live-test a background subagent that starts its own background subagent. Do exactly this, with no extra steps.\n\n1) Use the Agent tool with subagent_type \"general-purpose\" and run_in_background set to true to launch ONE subagent with this prompt:\n   \"Do exactly this. a) Use the Agent tool with subagent_type \\\"general-purpose\\\" and run_in_background set to true to launch ONE subagent with the prompt: Reply with exactly GRANDCHILD_DONE. b) Run this exact command with the Bash tool: sleep 20 && echo CHILD_WAITED. c) Reply with exactly CHILD_DONE.\"\n2) Immediately after launching it, reply with exactly ROOT_STARTED and stop. Do not wait for it or check on it.\n3) When its completion is reported, reply with exactly ROOT_WAKE_DONE and stop."},{"type":"await_run_status","targetRunIndex":2,"status":"running","waitForTurnItemType":"assistant_message"},{"type":"release_replay_gate","label":"result:background-wake:1"},{"type":"await_run_status","targetRunIndex":2,"status":"completed"}]}"###,
    )?)
}
