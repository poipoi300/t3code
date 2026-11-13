#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_NESTED_SUBAGENT_MODEL_PROMPT: &str = r###"Live-test nested subagents. Do exactly this, with no extra steps.

1) Call the Agent tool once with subagent_type "general-purpose", model "haiku", run_in_background false, and this prompt:
   "Call the Agent tool once with subagent_type \"general-purpose\", run_in_background false, no model parameter, and the prompt: Reply with exactly NESTED_LEAF_OK. Then reply with exactly what that agent said."
2) When it finishes, reply with exactly what it said."###;

pub fn claude_nested_subagent_model_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Live-test nested subagents. Do exactly this, with no extra steps.\n\n1) Call the Agent tool once with subagent_type \"general-purpose\", model \"haiku\", run_in_background false, and this prompt:\n   \"Call the Agent tool once with subagent_type \\\"general-purpose\\\", run_in_background false, no model parameter, and the prompt: Reply with exactly NESTED_LEAF_OK. Then reply with exactly what that agent said.\"\n2) When it finishes, reply with exactly what it said."}]}"###,
    )?)
}
