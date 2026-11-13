use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_muse_permission_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    require(
        rows(&p, "runs").count() == 2
            && p["runs"][0]["status"] == "completed"
            && matches!(
                p["runs"][1]["status"].as_str(),
                Some("completed" | "interrupted")
            ),
        "Muse approved/declined run statuses",
    )?;
    let statuses: Vec<_> = rows(&p, "runs")
        .filter_map(|run| run["status"].as_str())
        .collect();
    assert_base_projection(result, transcript, 2, None, Some(&statuses))?;
    let mut requests: Vec<_> = rows(&p, "runtimeRequests").enumerate().collect();
    requests.sort_by(|(left_index, left), (right_index, right)| {
        left["createdAt"]
            .as_str()
            .cmp(&right["createdAt"].as_str())
            .then(left_index.cmp(right_index))
    });
    require(
        json!(
            requests
                .iter()
                .map(|(_, request)| json!([
                    request["kind"],
                    request["status"],
                    request["decision"]
                ]))
                .collect::<Vec<_>>()
        ) == json!([
            ["command", "resolved", "accept"],
            ["command", "resolved", "decline"]
        ]),
        "Muse approval choices/order",
    )?;
    let approval = rows(&p, "turnItems").find(|item| item["type"] == "approval_request");
    let options = approval
        .map(|item| values(item, "options", "decision"))
        .unwrap_or_default();
    require(
        options.contains(&json!("accept"))
            && options.contains(&json!("decline"))
            && !options.contains(&json!("cancel")),
        "Muse decline is reject/abort, not cancellation",
    )?;
    let choices: Vec<_> = replay_frames(transcript)
        .iter()
        .filter(|(inbound, frame)| !*inbound && frame["method"] == "approval/decide")
        .map(|(_, frame)| frame["params"]["choiceId"].clone())
        .collect();
    require(
        json!(choices) == json!(["allow_once", "abort"]),
        "Muse native approval decisions",
    )?;
    let completed = |path: &str| {
        rows(&p, "turnItems").any(|item| {
            item["type"] == "command_execution"
                && item["status"] == "completed"
                && item["input"]
                    .as_str()
                    .is_some_and(|input| input.contains(path))
        })
    };
    require(
        completed(super::input::MUSE_PERMISSION_APPROVED_FILE)
            && !completed(super::input::MUSE_PERMISSION_DECLINED_FILE),
        "declined Muse command must not execute",
    )
}
