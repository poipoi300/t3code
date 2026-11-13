#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};
pub fn assert_claude_nested_subagent_model_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    assert_no_extra_app_runs_for_provider_children(&p, 1)?;
    assert_assistant_text_includes(&p, "NESTED_LEAF_OK")?;
    require(
        result.shell_snapshot.threads.len() == 3,
        "nested native children need two child threads",
    )?;
    let root = &p["runs"][0]["rootNodeId"];
    require(!root.is_null(), "missing root node")?;
    let outer = rows(&p, "subagents")
        .find(|a| a["nativeTaskRef"]["nativeId"] == "a457ca677bc1ecd82")
        .unwrap_or(&Value::Null);
    let nested = rows(&p, "subagents")
        .find(|a| a["nativeTaskRef"]["nativeId"] == "aac1d7a2af5371c7d")
        .unwrap_or(&Value::Null);
    require(
        outer["status"] == "completed" && nested["status"] == "completed",
        "missing/completing nested agent",
    )?;
    require(
        outer["parentNodeId"] == *root
            && nested["parentNodeId"] == outer["id"]
            && outer["model"] == "claude-haiku-4-5"
            && nested["model"] == "claude-haiku-4-5",
        "nested agents must inherit the owner's executed model and parent",
    )?;
    let selection = json!({"instanceId":outer["providerInstanceId"],"model":"claude-haiku-4-5"});
    require(
        outer["modelSelection"] == selection && nested["modelSelection"] == selection,
        "nested model selections differ",
    )?;
    let node = rows(&p, "nodes")
        .find(|n| n["id"] == nested["id"])
        .unwrap_or(&Value::Null);
    require(
        node.is_object()
            && node["parentNodeId"] == outer["id"]
            && node["rootNodeId"] == *root
            && nested["result"]
                .as_str()
                .is_some_and(|s| s.contains("NESTED_LEAF_OK")),
        "nested node/result differs",
    )?;
    let child = nested["childThreadId"]
        .as_str()
        .and_then(|id| result.projections.get(id));
    require(child.is_some(), "missing nested child projection")?;
    require(
        child.is_some_and(|child| child.thread.model_selection.model == "claude-haiku-4-5"),
        "nested child model differs",
    )?;
    require(
        rows(&p, "providerTurns").count() == 1,
        "native children must not create provider turns",
    )?;
    assert_includes_fields(
        &p["providerTurns"][0]["turnTokenUsage"],
        &json!({"usageStatus":"complete","hasSubagents":true,"inputTokens":74167,"outputTokens":236,"reasoningTokens":30,"models":["claude-sonnet-4-6"],"otherUsage":{"inputTokens":87409,"outputTokens":335}}),
        "root/process usage counted native children incorrectly",
    )?;

    Ok(())
}
