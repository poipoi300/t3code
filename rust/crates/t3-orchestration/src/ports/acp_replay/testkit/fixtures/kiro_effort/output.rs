use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn assert_kiro_effort_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    use super::input::*;
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[KIRO_EFFORT_PROMPT])?;
    let frames = replay_frames(transcript);
    let is_write = |frame: &Value, id: &str| {
        frame["kind"] == "request"
            && frame["method"] == "session/set_config_option"
            && frame["params"]["configId"] == id
    };
    let model = frames.iter().position(|(inbound, frame)| {
        !*inbound && is_write(frame, "model") && frame["params"]["value"] == KIRO_EFFORT_MODEL
    });
    let effort: Vec<_> = frames
        .iter()
        .enumerate()
        .filter(|(_, (inbound, frame))| !*inbound && is_write(frame, "effortLevel"))
        .collect();
    let prompt = frames
        .iter()
        .position(|(inbound, frame)| !*inbound && frame["method"] == "session/prompt");
    require(
        effort.len() == 1 && effort[0].1.1["params"]["value"] == KIRO_EFFORT_LEVEL,
        "expected one effort-level write",
    )?;
    let write = effort[0].0;
    require(
        model
            .zip(prompt)
            .is_some_and(|(model, prompt)| model < write && write < prompt),
        "effort write must follow model selection and precede prompt",
    )?;
    let model = model.expect("model write was asserted");
    let prompt = prompt.expect("prompt was asserted");
    fn effort_value(frame: &Value) -> Option<&Value> {
        let config = frame["result"]
            .get("configOptions")
            .filter(|value| !value.is_null())
            .unwrap_or(&frame["params"]["update"]["configOptions"]);
        config
            .as_array()?
            .iter()
            .find(|option| option["id"] == "effortLevel")
            .and_then(|option| option.get("currentValue"))
    }
    require(
        frames[model..write]
            .iter()
            .any(|(inbound, frame)| *inbound && effort_value(frame).is_some()),
        "effort option must be advertised for the selected model",
    )?;
    require(
        frames[write..prompt]
            .iter()
            .filter(|(inbound, _)| *inbound)
            .filter_map(|(_, frame)| effort_value(frame))
            .next_back()
            == Some(&json!(KIRO_EFFORT_LEVEL)),
        "Kiro must report selected effort before prompting",
    )?;
    require(
        p["runs"][0]["modelSelection"]["model"] == KIRO_EFFORT_MODEL,
        "run must retain the selected model",
    )
}
