use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_acp_titled_multi_select_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_runtime_request_kinds(&p, &["user_input"])?;
    assert_all_runtime_requests_resolved(&p)?;
    assert_turn_item_types(
        &p,
        &["user_message", "user_input_request", "assistant_message"],
    )?;
    assert_user_messages_include(&p, &[super::input::ACP_TITLED_MULTISELECT_PROMPT])?;
    assert_assistant_text_includes(&p, "titled multi-select fixture complete")?;
    let request = rows(&p, "turnItems").find(|item| item["type"] == "user_input_request");
    require(request.is_some_and(|request|request["questions"]==serde_json::json!([{"id":"fixture_areas","header":"Areas","question":"Which areas should this fixture cover?","options":[{"label":"Strict schemas","description":"Typed wire shapes","value":"schemas"},{"label":"UI flexibility","description":"UI flexibility","value":"ui"},{"label":"Docs","description":"Docs","value":"docs"}],"allowCustomAnswer":false,"multiSelect":true}])),"titled multiselect question/options")
}
