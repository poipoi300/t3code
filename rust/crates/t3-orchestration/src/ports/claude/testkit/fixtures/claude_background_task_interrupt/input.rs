#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_BACKGROUND_TASK_INTERRUPT_PROMPT: &str = r###"Live-test interrupting a turn that owns background work. You must make two Bash tool calls, one after the other. Do exactly this, with no extra steps.

1) Call the Bash tool with run_in_background set to true and this exact command:
   sleep 30 && echo BG_INTERRUPT_DONE
2) After that call returns, call the Bash tool again in the foreground (run_in_background false) with this exact command and wait for it:
   node -e "setTimeout(() => console.log('FG_DONE'), 30000)"
3) After it finishes, reply with exactly FINISHED."###;

pub fn claude_background_task_interrupt_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Live-test interrupting a turn that owns background work. You must make two Bash tool calls, one after the other. Do exactly this, with no extra steps.\n\n1) Call the Bash tool with run_in_background set to true and this exact command:\n   sleep 30 && echo BG_INTERRUPT_DONE\n2) After that call returns, call the Bash tool again in the foreground (run_in_background false) with this exact command and wait for it:\n   node -e \"setTimeout(() => console.log('FG_DONE'), 30000)\"\n3) After it finishes, reply with exactly FINISHED."},{"type":"interrupt","targetRunIndex":1,"waitForTurnItemType":"command_execution"}]}"###,
    )?)
}
