use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_muse_workflow_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_field_sequence(&p, "runs", "status", &["completed", "completed"])?;
    let frames = replay_frames(transcript);
    require(
        !frames
            .iter()
            .any(|(inbound, frame)| !*inbound && frame["method"] == "turn/interrupt")
            && frames
                .iter()
                .filter(|(inbound, frame)| !*inbound && frame["method"] == "turn/start")
                .count()
                == 1,
        "Muse report continuation must not interrupt/restart native turn",
    )?;
    let workflow = rows(&p, "turnItems")
        .find(|item| item["type"] == "dynamic_tool" && item["toolName"] == "workflow");
    require(
        workflow.is_some_and(|item| {
            item["status"] == "completed" && item["runId"] == p["runs"][0]["id"]
        }),
        "workflow row status/ownership",
    )?;
    let workflow = workflow.expect("workflow asserted");
    let summary = frames
        .iter()
        .filter(|(inbound, _)| *inbound)
        .filter_map(|(_, frame)| frame["params"]["item"]["finalSummary"].as_str())
        .next_back();
    require(
        summary.is_some_and(|summary| workflow["output"] == summary),
        "workflow output must be final summary",
    )?;
    require(
        rows(&p, "messages").any(|message| {
            message["role"] == "assistant" && message["runId"] == p["runs"][1]["id"]
        }),
        "workflow report must land in continuation run",
    )?;
    require(
        rows(&p, "subagents").count() == 2
            && rows(&p, "subagents").all(|agent| {
                agent["origin"] == "provider_native"
                    && agent["status"] == "completed"
                    && agent["childThreadId"].is_null()
            }),
        "workflow agents must remain rows without child sessions",
    )?;
    let agents: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "subagent")
        .collect();
    require(
        agents.len() == 2
            && agents
                .iter()
                .all(|agent| agent["parentItemId"] == workflow["id"])
            && rows(&p["providerThreads"][0], "pendingBackgroundTasks")
                .next()
                .is_none(),
        "workflow child rows/settled pending work",
    )
}
