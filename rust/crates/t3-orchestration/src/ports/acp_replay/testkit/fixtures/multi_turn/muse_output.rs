use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_muse_multi_turn_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_run_ordinals(&p, &[1, 2])?;
    assert_conversation_message_roles(&p, &["user", "assistant", "user", "assistant"])?;
    assert_user_messages_include(&p, &[MULTI_TURN_FIRST_PROMPT, MULTI_TURN_SECOND_PROMPT])?;
    assert_assistant_text_includes(&p, "first fixture turn complete")?;
    assert_assistant_text_includes(&p, "second fixture turn complete")?;
    require(
        rows(&p, "turnItems").all(|item| {
            matches!(
                item["type"].as_str(),
                Some("user_message" | "assistant_message" | "reasoning" | "checkpoint")
            )
        }) && rows(&p, "providerThreads").count() == 1,
        "unexpected Muse rows/provider-thread count",
    )
}
