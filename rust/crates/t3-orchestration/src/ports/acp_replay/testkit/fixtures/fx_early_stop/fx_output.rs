use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_fx_early_stop_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["interrupted", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[TURN_INTERRUPT_PROMPT, SIMPLE_PROMPT])?;
    assert_field_sequence(&p, "attempts", "status", &["interrupted", "completed"])?;
    require(
        rows(&p, "providerThreads").count() == 1
            && p["providerThreads"][0]["status"] == "idle"
            && p["providerTurns"][0]["status"] == "interrupted"
            && p["providerTurns"][1]["status"] == "completed"
            && p["providerTurns"][0]["providerThreadId"]
                == p["providerTurns"][1]["providerThreadId"],
        "follow-up must reuse fx session after early Stop",
    )
}
