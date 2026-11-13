#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};
pub fn assert_claude_nested_background_subagent_wake_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    assert_no_extra_app_runs_for_provider_children(&p, 2)?;
    let child = rows(&p, "subagents")
        .find(|a| a["nativeTaskRef"]["nativeId"] == "a5d7d869cbb05c754")
        .unwrap_or(&Value::Null);
    let grandchild = rows(&p, "subagents")
        .find(|a| a["nativeTaskRef"]["nativeId"] == "a412f30077dfd84dd")
        .unwrap_or(&Value::Null);
    require(
        child["status"] == "completed" && grandchild["status"] == "completed",
        "missing/completing nested background subagent",
    )?;
    require(
        child["parentNodeId"] == p["runs"][0]["rootNodeId"]
            && grandchild["parentNodeId"] == child["id"],
        "nested owner hierarchy differs",
    )?;
    require(
        rows(&p, "messages")
            .find(|m| m["id"] == p["runs"][1]["userMessageId"])
            .is_some_and(|m| m["text"] == "CHILD_DONE"),
        "grandchild must not wake the root",
    )?;
    require(!child["childThreadId"].is_null(), "missing child thread")?;
    require(
        json!(background_notifications(&p))
            == json!([{"summary":format!(r#"Subagent "{}" finished"#,child["title"].as_str().unwrap_or_default()),"outcome":"completed","source":{"kind":"subagent","childThreadId":child["childThreadId"]}}]),
        "nested wake notification differs",
    )?;

    Ok(())
}
