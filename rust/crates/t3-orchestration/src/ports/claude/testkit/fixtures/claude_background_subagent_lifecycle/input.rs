#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_LAUNCH_PROMPT: &str = r###"Live-test background subagents. You must call the Agent tool before replying. Do exactly this, with no extra steps.

1) In one message, make TWO Agent tool calls that launch general-purpose subagents, both with run_in_background set to true:
   - description "Agent A", model "haiku", prompt: "Reply with exactly: A_FIRST"
   - description "Agent B", prompt: "Call the Bash tool in the foreground (run_in_background false) with this exact command and wait for it: node -e \"setTimeout(() => console.log('B_DONE'), 90000)\" Then reply with exactly: B_FINAL"
2) After both calls return, reply with exactly LAUNCHED and stop. Do not wait for them or check on them.
3) When Agent A's completion is reported, reply with exactly A_REPORTED and stop."###;

pub const CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_STOP_PROMPT: &str =
    r###"Stop Agent B now with the TaskStop tool. Then reply with exactly: B_STOPPED"###;

pub const CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_RESUME_PROMPT: &str = r###"Use the SendMessage tool to send Agent A this message: Reply with exactly: A_SECOND
Then reply with exactly RESUMED and stop. When Agent A's new completion is reported, reply with exactly A_RESUME_REPORTED and stop."###;

pub const CLAUDE_BACKGROUND_SUBAGENT_LIFECYCLE_FINAL_PROMPT: &str =
    r###"Reply with exactly: ALL_DONE"###;

pub fn claude_background_subagent_lifecycle_input() -> crate::error::Result<OrchestratorFixtureInput>
{
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Live-test background subagents. You must call the Agent tool before replying. Do exactly this, with no extra steps.\n\n1) In one message, make TWO Agent tool calls that launch general-purpose subagents, both with run_in_background set to true:\n   - description \"Agent A\", model \"haiku\", prompt: \"Reply with exactly: A_FIRST\"\n   - description \"Agent B\", prompt: \"Call the Bash tool in the foreground (run_in_background false) with this exact command and wait for it: node -e \\\"setTimeout(() => console.log('B_DONE'), 90000)\\\" Then reply with exactly: B_FINAL\"\n2) After both calls return, reply with exactly LAUNCHED and stop. Do not wait for them or check on them.\n3) When Agent A's completion is reported, reply with exactly A_REPORTED and stop."},{"type":"await_run_status","targetRunIndex":2,"status":"completed"},{"type":"message","text":"Stop Agent B now with the TaskStop tool. Then reply with exactly: B_STOPPED"},{"type":"await_run_status","targetRunIndex":4,"status":"completed"},{"type":"message","text":"Use the SendMessage tool to send Agent A this message: Reply with exactly: A_SECOND\nThen reply with exactly RESUMED and stop. When Agent A's new completion is reported, reply with exactly A_RESUME_REPORTED and stop."},{"type":"await_run_status","targetRunIndex":6,"status":"completed"},{"type":"message","text":"Reply with exactly: ALL_DONE"}]}"###,
    )?)
}
