use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_tool_call_denied_write_fx_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[super::input::TOOL_CALL_DENIED_WRITE_PROMPT])?;
    assert_runtime_request_counts(&p, 1, Some(1))?;
    assert_all_runtime_requests_resolved(&p)?;
    assert_field_sequence(&p, "runtimeRequests", "decision", &["decline"])?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|item| item["type"] == "file_change")
                .map(|item| &item["status"])
                .collect::<Vec<_>>()
        ) == json!(["failed"]),
        "declined fx write must fail",
    )
}
