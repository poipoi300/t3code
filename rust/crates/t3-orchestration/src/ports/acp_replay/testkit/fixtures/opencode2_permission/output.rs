use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_open_code2_permission_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[OPENCODE2_PERMISSION_PROMPT])?;
    let mut requests: Vec<_> = rows(&p, "runtimeRequests").enumerate().collect();
    requests.sort_by(|(li, l), (ri, r)| {
        l["createdAt"]
            .as_str()
            .cmp(&r["createdAt"].as_str())
            .then(li.cmp(ri))
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
            ["command", "resolved", "decline"],
            ["command", "resolved", "decline"],
            ["command", "resolved", "decline"]
        ]),
        "native permission request sequence",
    )?;
    require(
        json!(
            requests
                .iter()
                .map(|(_, request)| &request["nativeRequestRef"]["nativeId"])
                .collect::<Vec<_>>()
        ) == json!([
            "per_0eb7c4d7e001Pyt8o50Vi4KrOO",
            "per_0eb7c5376001gkpJzQEeWtaoRE",
            "per_0eb7c5b1f001zGfBgDhzxz8z10",
            "per_0eb7c6755001DdRDvkLLSYU4BD"
        ]),
        "native permission request identities",
    )?;
    let approvals: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "approval_request")
        .collect();
    require(
        json!(
            approvals
                .iter()
                .map(|item| json!([item["status"], item["prompt"]]))
                .collect::<Vec<_>>()
        ) == json!([
            ["completed", "echo FIRST"],
            ["cancelled", "echo SECOND"],
            ["cancelled", "echo SECOND"],
            ["cancelled", "echo THIRD_PROBE"]
        ]),
        "approval item prompt/status sequence",
    )?;
    require(
        rows(approvals[0], "options").any(|option| {
            *option == json!({"decision":"acceptForSession","label":"Allow echo * this session"})
        }),
        "session-specific native pattern approval option",
    )?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|item| item["type"] == "command_execution")
                .map(|item| json!([item["input"], item["status"], item["output"]]))
                .collect::<Vec<_>>()
        ) == json!([
            ["echo FIRST", "completed", "FIRST\n"],
            [
                "echo SECOND",
                "failed",
                "Unable to execute command: echo SECOND"
            ],
            [
                "echo SECOND",
                "failed",
                "Unable to execute command: echo SECOND"
            ],
            [
                "echo THIRD_PROBE",
                "failed",
                "Unable to execute command: echo THIRD_PROBE"
            ]
        ]),
        "permission-denied shell results",
    )
}
