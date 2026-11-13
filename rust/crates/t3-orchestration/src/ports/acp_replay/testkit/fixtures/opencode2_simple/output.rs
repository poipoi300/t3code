use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_open_code2_simple_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_turn_item_type_sequence(
        &p,
        &[
            "user_message",
            "reasoning",
            "assistant_message",
            "checkpoint",
        ],
    )?;
    assert_user_messages_include(&p, &[OPENCODE2_SIMPLE_PROMPT])?;
    assert_assistant_text_includes(&p, "391 is not prime: it's the product 17 × 23.")?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "reasoning")
            .is_some_and(|item| {
                item["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("17×23 = 391"))
            }),
        "missing OpenCode reasoning",
    )?;
    let turn = &p["providerTurns"][0];
    require(turn["status"] == "completed", "OpenCode turn must complete")?;
    assert_includes_fields(
        &turn["turnTokenUsage"],
        &json!({"usageStatus":"complete","inputTokens":8701+489,"cachedInputTokens":489,"outputTokens":113+147,"reasoningTokens":147,"models":["opencode/space-bunny-free"]}),
        "complete step usage totals",
    )?;
    assert_includes_fields(
        &turn["tokenUsage"],
        &json!({"usedTokens":8701+489+113,"maxTokens":524288}),
        "latest context-window usage",
    )
}
