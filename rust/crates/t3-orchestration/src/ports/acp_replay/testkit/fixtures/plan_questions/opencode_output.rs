use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code_plan_questions_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_plan_questions_fixture_output(result, transcript)?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "user_input_request")
            .is_some_and(|item| {
                item["questions"][0]["id"] == "question-0-schema-vs-flexibility"
                    && item["status"] == "completed"
            }),
        "OpenCode question ID/status",
    )?;
    require(
        !rows(&p, "turnItems")
            .any(|item| item["type"] == "dynamic_tool" && item["toolName"] == "question"),
        "user input must not be duplicated as a dynamic tool",
    )
}
