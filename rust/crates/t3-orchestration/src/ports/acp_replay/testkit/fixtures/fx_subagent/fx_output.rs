use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_fx_subagent_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_execution_node_kinds(&p, &["root_turn", "subagent"])?;
    assert_no_extra_app_runs_for_provider_children(&p, 1)?;
    assert_user_messages_include(&p, &[SUBAGENT_PROMPT])?;
    assert_turn_item_types(&p, &["user_message", "subagent", "assistant_message"])?;
    require(
        !rows(&p, "turnItems").any(|item| item["type"] == "dynamic_tool"),
        "fx subagent calls must not duplicate dynamic tool rows",
    )?;
    require(
        rows(&p, "subagents").count() == 2 && result.shell_snapshot.threads.len() == 3,
        "expected two fx native children",
    )?;
    for subagent in rows(&p, "subagents") {
        require(
            subagent["status"] == "completed"
                && subagent["origin"] == "provider_native"
                && subagent["driver"] == "fx"
                && !subagent["nativeTaskRef"].is_null()
                && subagent["prompt"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
            "fx subagent metadata",
        )?;
        require(
            subagent["result"]
                .as_str()
                .is_some_and(|text| !text.contains("\"ok\":")),
            "fx result must be its reply without JSON wrapper",
        )?;
        let child = result
            .projections
            .get(subagent["childThreadId"].as_str().unwrap_or_default())
            .map(serde_json::to_value)
            .transpose()?;
        require(child.is_some(), "missing fx child projection")?;
        let child = child.expect("child asserted");
        require(
            child["thread"]["lineage"]["parentThreadId"] == p["thread"]["id"]
                && child["thread"]["lineage"]["relationshipToParent"] == "subagent"
                && rows(&child, "runs").count() == 0,
            "native fx child lineage/app runs",
        )?;
        assert_turn_item_type_sequence(&child, &["user_message", "assistant_message"])?;
        assert_user_messages_include(&child, &[subagent["prompt"].as_str().unwrap_or_default()])?;
        require(
            rows(&child, "turnItems")
                .find(|item| item["type"] == "assistant_message")
                .is_some_and(|item| item["text"] == subagent["result"]),
            "fx child reply must equal its native result",
        )?;
    }
    Ok(())
}
