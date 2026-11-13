use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_kiro_subagent_output(
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
        "child tools must not leak into parent",
    )?;
    require(
        rows(&p, "subagents").count() == 2 && result.shell_snapshot.threads.len() == 3,
        "expected two native Kiro children",
    )?;
    let parent_text = rows(&p, "turnItems")
        .filter(|item| item["type"] == "assistant_message")
        .filter_map(|item| item["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for subagent in rows(&p, "subagents") {
        require(
            subagent["status"] == "completed"
                && subagent["origin"] == "provider_native"
                && subagent["driver"] == "kiro",
            "Kiro subagent identity/status",
        )?;
        for field in ["nativeTaskRef", "title", "result", "childThreadId"] {
            require(
                !subagent[field].is_null(),
                format!("missing subagent {field}"),
            )?;
        }
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
                && rows(&child, "runs").next().is_none(),
            "Kiro child lineage/app runs",
        )?;
        assert_turn_item_types(
            &child,
            &["user_message", "dynamic_tool", "assistant_message"],
        )?;
        assert_user_messages_include(&child, &[subagent["prompt"].as_str().unwrap_or_default()])?;
        for item in rows(&child, "turnItems").filter(|item| item["type"] == "assistant_message") {
            let prefix: String = item["text"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .chars()
                .take(40)
                .collect();
            require(
                !parent_text.contains(&prefix),
                "child response leaked into parent",
            )?;
        }
    }
    Ok(())
}
