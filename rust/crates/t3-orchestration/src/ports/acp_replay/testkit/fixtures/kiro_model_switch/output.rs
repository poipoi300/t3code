use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_kiro_model_switch_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    use super::input::*;
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(
        &p,
        &[
            KIRO_MODEL_SWITCH_FIRST_PROMPT,
            KIRO_MODEL_SWITCH_SECOND_PROMPT,
        ],
    )?;
    let frames: Vec<_> = replay_frames(transcript)
        .into_iter()
        .filter(|(inbound, frame)| !*inbound && frame["kind"] == "request")
        .map(|(_, frame)| frame)
        .collect();
    require(
        frames
            .iter()
            .filter(|frame| frame["method"] == "session/new")
            .count()
            == 1,
        "model switch must reuse the live Kiro session",
    )?;
    let last = frames
        .iter()
        .filter(|frame| {
            frame["method"] == "session/set_config_option" && frame["params"]["configId"] == "model"
        })
        .map(|frame| &frame["params"]["value"])
        .next_back();
    require(
        last == Some(&json!(KIRO_MODEL_SWITCH_TARGET))
            && rows(&p, "runs")
                .next_back()
                .is_some_and(|run| run["modelSelection"]["model"] == KIRO_MODEL_SWITCH_TARGET),
        "model selection was not written/projected for the second turn",
    )
}
