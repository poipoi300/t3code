use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_cursor_subagent_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_execution_node_kinds(&p, &["root_turn", "subagent"])?;
    assert_turn_item_types(&p, &["user_message", "subagent", "assistant_message"])?;
    assert_run_provider_turn_cardinality(&p, 1, None)?;
    assert_no_extra_app_runs_for_provider_children(&p, 1)?;
    assert_user_messages_include(&p, &[SUBAGENT_PROMPT])?;
    assert_assistant_text_includes(&p, "cursor-read-only-fixture")?;
    assert_assistant_text_includes(&p, "ES2022")?;
    let lifecycle: Vec<_> = rows(&p, "turnItems")
        .filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("reasoning" | "assistant_message" | "subagent")
            )
        })
        .collect();
    require(
        json!(
            lifecycle
                .iter()
                .map(|item| &item["type"])
                .collect::<Vec<_>>()
        ) == json!([
            "reasoning",
            "assistant_message",
            "subagent",
            "subagent",
            "reasoning",
            "assistant_message"
        ]),
        "Cursor lifecycle order",
    )?;
    let messages: Vec<_> = lifecycle
        .iter()
        .filter(|item| item["type"] == "assistant_message")
        .collect();
    require(
        messages.len() == 2
            && messages[0]["text"]
                == "Spawning two subagents in parallel to read `package.json` and `tsconfig.json`.\n"
            && messages[1]["text"]
                .as_str()
                .is_some_and(|text| text.contains("Both subagents finished.")),
        "Cursor progress/final messages",
    )?;
    require(
        rows(&p, "subagents").count() == 2 && result.shell_snapshot.threads.len() == 3,
        "expected two native children",
    )?;
    assert_field_sequence(&p, "subagents", "status", &["completed", "completed"])?;
    for subagent in rows(&p, "subagents") {
        for (field, expected) in [
            ("origin", "provider_native"),
            ("createdBy", "agent"),
            ("driver", "cursor"),
        ] {
            require(
                subagent[field] == expected,
                format!("unexpected subagent {field}"),
            )?;
        }
        require(
            subagent["providerThreadId"].is_null(),
            "provider-native child must not own a provider thread",
        )?;
        for field in ["childThreadId", "nativeTaskRef", "completedAt", "result"] {
            require(
                !subagent[field].is_null(),
                format!("missing subagent {field}"),
            )?;
        }
        let child_id = subagent["childThreadId"].as_str().unwrap_or_default();
        let child =
            result
                .projections
                .get(child_id)
                .ok_or_else(|| crate::error::Error::Service {
                    operation: "replay fixture assertion",
                    detail: format!("missing native child {child_id}"),
                })?;
        let child = serde_json::to_value(child)?;
        require(
            child["thread"]["lineage"]["parentThreadId"] == p["thread"]["id"]
                && child["thread"]["lineage"]["relationshipToParent"] == "subagent"
                && child["thread"]["activeProviderThreadId"].is_null(),
            "native child lineage/provider thread",
        )?;
        for table in ["runs", "providerThreads", "providerTurns"] {
            require(
                rows(&child, table).next().is_none(),
                format!("native child has app-owned {table}"),
            )?;
        }
        assert_execution_node_kinds(&child, &["root_turn", "tool_call"])?;
        assert_turn_item_types(
            &child,
            &["user_message", "dynamic_tool", "assistant_message"],
        )?;
        assert_user_messages_include(&child, &[subagent["prompt"].as_str().unwrap_or_default()])?;
        let prefix: String = subagent["result"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(40)
            .collect();
        assert_assistant_text_includes(&child, &prefix)?;
    }
    Ok(())
}
