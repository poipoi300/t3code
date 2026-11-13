use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code_child_approval_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[OPENCODE_SUBAGENT_PROMPT])?;
    assert_assistant_text_includes(&p, "PARENT_OK")?;
    let approvals: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "approval_request")
        .collect();
    require(
        approvals.len() == 1
            && approvals[0]["status"] == "completed"
            && approvals[0]["runId"] == p["runs"][0]["id"],
        "child approval item must belong to parent run",
    )?;
    require(
        rows(&p, "runtimeRequests").count() == 1
            && p["runtimeRequests"][0]["kind"] == "command"
            && p["runtimeRequests"][0]["status"] == "resolved",
        "resolved child request",
    )?;
    require(
        rows(&p, "subagents").count() == 1
            && p["subagents"][0]["status"] == "completed"
            && p["subagents"][0]["result"]
                .as_str()
                .is_some_and(|text| text.contains("CHILD_OK")),
        "child must complete after approval",
    )
}
