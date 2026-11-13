use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code2_resume_after_restart_output(
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
    assert_user_messages_include(
        &p,
        &[OPENCODE2_RESTART_PROMPT, OPENCODE2_RESTART_RECALL_PROMPT],
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "command_execution" && item["runId"] == p["runs"][0]["id"])
            .is_some_and(|item| {
                item["status"] == "failed" && item["input"] == "sleep 25 && echo RESUMED"
            }),
        "restarted running shell failure",
    )?;
    assert_field_sequence(&p, "providerTurns", "status", &["interrupted", "completed"])?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "assistant_message" && item["runId"] == p["runs"][1]["id"])
            .is_some_and(|item| {
                item["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("sleep 25"))
            })
            && rows(&p, "providerThreads").count() == 1,
        "resumed session must remember the prior shell",
    )
}
