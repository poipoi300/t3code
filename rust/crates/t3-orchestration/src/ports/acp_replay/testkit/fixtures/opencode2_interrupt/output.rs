use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code2_interrupt_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["interrupted"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "assistant_message",
            "command_execution",
            "run_interrupt_request",
            "run_interrupt_result",
        ],
    )?;
    assert_user_messages_include(&p, &[OPENCODE2_INTERRUPT_PROMPT])?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "command_execution")
            .is_some_and(|item| {
                item["input"] == "sleep 60 && echo LATE" && item["status"] == "interrupted"
            }),
        "native shell must interrupt",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "run_interrupt_result")
            .is_some_and(|item| item["status"] == "interrupted"),
        "Stop result status",
    )?;
    assert_field_sequence(&p, "providerTurns", "status", &["interrupted"])?;
    require(
        p["providerThreads"][0]["status"] == "idle",
        "provider thread must return idle",
    )
}
