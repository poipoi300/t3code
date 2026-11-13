#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};

pub fn assert_claude_thread_rollback_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        3,
        None,
        Some(&["completed", "rolled_back", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    assert_run_ordinals(&p, &[1, 2, 3])?;
    assert_turn_item_type_sequence(
        &p,
        &[
            "user_message",
            "assistant_message",
            "checkpoint",
            "user_message",
            "assistant_message",
            "checkpoint",
            "user_message",
            "reasoning",
            "assistant_message",
            "checkpoint",
        ],
    )?;
    assert_visible_turn_item_type_sequence(
        &p,
        &[
            "user_message",
            "assistant_message",
            "checkpoint",
            "user_message",
            "reasoning",
            "assistant_message",
            "checkpoint",
        ],
    )?;
    assert_user_messages_include(
        &p,
        &[
            THREAD_ROLLBACK_FIRST_PROMPT,
            THREAD_ROLLBACK_SECOND_PROMPT,
            THREAD_ROLLBACK_AFTER_PROMPT,
        ],
    )?;
    assert_visible_user_messages_include(
        &p,
        &[THREAD_ROLLBACK_FIRST_PROMPT, THREAD_ROLLBACK_AFTER_PROMPT],
    )?;
    assert_visible_user_messages_exclude(&p, &[THREAD_ROLLBACK_SECOND_PROMPT])?;
    require(
        rows(&p, "checkpoints").count() >= 2
            && rows(&p, "runs").any(|r| r["status"] == "rolled_back"),
        "rollback is absent from checkpoint/run state",
    )?;
    require(
        p["providerThreads"][0]["nativeConversationHeadRef"].is_null(),
        "rollback head must be reset",
    )?;
    Ok(())
}
