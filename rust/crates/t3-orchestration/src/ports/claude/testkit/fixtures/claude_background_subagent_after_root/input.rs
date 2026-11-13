#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_BACKGROUND_SUBAGENT_AFTER_ROOT_PROMPT: &str = r###"Live-test a background subagent that keeps working after your turn ends. Do exactly this, with no extra steps.

1) Use the Agent tool with run_in_background set to true to launch ONE general-purpose subagent with this prompt:
   "Do 2 steps. Before each step, write exactly one line: SUB_STEP_N (N = 1, then 2). Then run this exact command with the Bash tool: sleep 3 && echo SUB_DONE_N. After both steps, reply with exactly: SUB_FINAL_REPORT"
2) Immediately after launching it, reply with exactly ROOT_STARTED and stop. Do not wait for it or check on it.
3) When its completion is reported, reply with exactly ROOT_WAKE_DONE and stop."###;

pub fn claude_background_subagent_after_root_input()
-> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Live-test a background subagent that keeps working after your turn ends. Do exactly this, with no extra steps.\n\n1) Use the Agent tool with run_in_background set to true to launch ONE general-purpose subagent with this prompt:\n   \"Do 2 steps. Before each step, write exactly one line: SUB_STEP_N (N = 1, then 2). Then run this exact command with the Bash tool: sleep 3 && echo SUB_DONE_N. After both steps, reply with exactly: SUB_FINAL_REPORT\"\n2) Immediately after launching it, reply with exactly ROOT_STARTED and stop. Do not wait for it or check on it.\n3) When its completion is reported, reply with exactly ROOT_WAKE_DONE and stop."},{"type":"await_run_status","targetRunIndex":2,"status":"running","waitForTurnItemType":"assistant_message"},{"type":"release_replay_gate","label":"result:background-wake:1"},{"type":"await_run_status","targetRunIndex":2,"status":"completed"}]}"###,
    )?)
}
