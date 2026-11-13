use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_open_code_subagent_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_execution_node_kinds(&p, &["root_turn", "subagent", "assistant_message"])?;
    assert_turn_item_types(&p, &["user_message", "subagent", "assistant_message"])?;
    assert_run_provider_turn_cardinality(&p, 1, None)?;
    assert_no_extra_app_runs_for_provider_children(&p, 1)?;
    assert_user_messages_include(&p, &[OPENCODE_SUBAGENT_PROMPT])?;
    assert_assistant_text_includes(&p, "PARENT_OK")?;
    require(
        rows(&p, "subagents").count() == 1 && result.shell_snapshot.threads.len() == 2,
        "expected one native child",
    )?;
    let subagent = &p["subagents"][0];
    assert_includes_fields(
        subagent,
        &json!({"origin":"provider_native","createdBy":"agent","driver":"opencode","status":"completed","prompt":"Respond exactly CHILD_OK."}),
        "native subagent metadata",
    )?;
    require(
        subagent["result"]
            .as_str()
            .is_some_and(|text| text.contains("CHILD_OK")),
        "native child result",
    )?;
    for field in [
        "childThreadId",
        "providerThreadId",
        "nativeTaskRef",
        "completedAt",
    ] {
        require(!subagent[field].is_null(), format!("missing child {field}"))?;
    }
    let provider =
        rows(&p, "providerThreads").find(|thread| thread["id"] == subagent["providerThreadId"]);
    require(
        provider.is_some_and(|thread| {
            thread["appThreadId"] == subagent["childThreadId"]
                && thread["ownerNodeId"] == subagent["id"]
        }),
        "native child provider-thread ownership",
    )?;
    let child = result
        .projections
        .get(subagent["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(child.is_some(), "missing child projection")?;
    let child = child.expect("child asserted");
    require(
        child["thread"]["lineage"]["parentThreadId"] == p["thread"]["id"]
            && child["thread"]["lineage"]["relationshipToParent"] == "subagent"
            && child["thread"]["activeProviderThreadId"] == subagent["providerThreadId"]
            && rows(&child, "runs").count() == 0
            && rows(&child, "providerThreads").count() == 1
            && rows(&child, "providerTurns").count() == 1
            && child["providerTurns"][0]["status"] == "completed",
        "native child lineage/provider turn",
    )?;
    assert_execution_node_kinds(&child, &["root_turn", "assistant_message"])?;
    assert_turn_item_types(&child, &["user_message", "assistant_message"])?;
    assert_user_messages_include(&child, &["Respond exactly CHILD_OK."])?;
    assert_assistant_text_includes(&child, "CHILD_OK")
}
