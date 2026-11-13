use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_open_code2_question_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[OPENCODE2_QUESTION_PROMPT])?;
    assert_turn_item_type_sequence(
        &p,
        &[
            "user_message",
            "assistant_message",
            "user_input_request",
            "assistant_message",
            "checkpoint",
        ],
    )?;
    assert_assistant_text_includes(&p, "blue")?;
    require(
        rows(&p, "runtimeRequests").count() == 1
            && p["runtimeRequests"][0]["kind"] == "user_input"
            && p["runtimeRequests"][0]["status"] == "resolved"
            && p["runtimeRequests"][0]["answers"] == json!({"q0":"blue"}),
        "resolved user-input answer",
    )?;
    require(rows(&p,"turnItems").find(|item|item["type"]=="user_input_request").is_some_and(|item|item["status"]=="completed" && item["questions"]==json!([{"id":"q0","header":"Color preference","question":"Which color do you prefer?","options":[{"label":"Red","description":"Choose the color red","value":"Red"},{"label":"Blue","description":"Choose the color blue","value":"Blue"}],"multiSelect":false,"allowCustomAnswer":true}])),"question option/answer schema")
}
