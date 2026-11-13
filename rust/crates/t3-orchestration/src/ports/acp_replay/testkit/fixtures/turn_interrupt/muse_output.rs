use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_muse_turn_interrupt_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["interrupted"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[TURN_INTERRUPT_PROMPT])?;
    require(
        replay_frames(transcript)
            .iter()
            .any(|(inbound, frame)| !*inbound && frame["method"] == "turn/interrupt"),
        "Stop must reach Muse",
    )?;
    require(
        !rows(&p, "turnItems").any(|item| item["status"] == "running")
            && p["providerThreads"][0]["status"] == "idle"
            && p["providerTurns"][0]["status"] == "interrupted",
        "Muse turn must stop and return its session to idle",
    )
}
