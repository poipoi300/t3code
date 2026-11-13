#![cfg(feature = "claude-adapter")]
use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn assert_claude_subagent_output(
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
    assert_assistant_text_includes(&p, "claude-read-only-fixture")?;
    assert_assistant_text_includes(&p, "ES2022")?;
    require(
        !rows(&p, "turnItems").any(|i| i["type"] == "dynamic_tool"),
        "subagent tool leaked into the parent thread",
    )?;
    let nodes = rows(&p, "nodes")
        .filter(|n| n["kind"] == "subagent")
        .collect::<Vec<_>>();
    require(
        nodes.len() == 2 && nodes.iter().all(|n| n["status"] == "completed"),
        "native subagent nodes did not complete",
    )?;
    require(
        rows(&p, "subagents").count() == 2 && result.shell_snapshot.threads.len() == 3,
        "expected two native child threads",
    )?;
    for (file, contents) in [
        ("package.json", "claude-read-only-fixture"),
        ("tsconfig.json", "ES2022"),
    ] {
        let prompt = format!(
            "Read the file `{file}` in the current working directory and return its full contents."
        );
        require(
            rows(&p, "subagents").any(|a| {
                a["prompt"] == prompt && a["result"].as_str().is_some_and(|r| r.contains(contents))
            }),
            format!("missing {file} subagent result"),
        )?;
    }
    for agent in rows(&p, "subagents") {
        let progress = if agent["prompt"]
            .as_str()
            .is_some_and(|s| s.contains("package.json"))
        {
            "Summarizing package.json"
        } else {
            "Reading tsconfig.json"
        };
        assert_includes_fields(
            agent,
            &json!({"origin":"provider_native","createdBy":"agent","driver":"claudeAgent","status":"completed","progress":progress,"providerThreadId":null}),
            "subagent identity/lifecycle differs",
        )?;
        require(
            !agent["childThreadId"].is_null()
                && !agent["nativeTaskRef"].is_null()
                && !agent["completedAt"].is_null(),
            "missing native child identity/completion",
        )?;
        let child = agent["childThreadId"]
            .as_str()
            .and_then(|id| result.projections.get(id))
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or(Value::Null);
        require(child.is_object(), "missing child projection")?;
        require(
            child["thread"]["lineage"]["parentThreadId"] == p["thread"]["id"]
                && child["thread"]["lineage"]["relationshipToParent"] == "subagent"
                && child["thread"]["activeProviderThreadId"].is_null(),
            "child lineage differs",
        )?;
        for table in ["runs", "providerThreads", "providerTurns"] {
            require(
                rows(&child, table).count() == 0,
                format!("native child created app {table}"),
            )?;
        }
        assert_execution_node_kinds(&child, &["root_turn", "tool_call"])?;
        assert_turn_item_types(
            &child,
            &["user_message", "dynamic_tool", "assistant_message"],
        )?;
        assert_user_messages_include(&child, &[agent["prompt"].as_str().unwrap_or_default()])?;
        let prefix = agent["result"]
            .as_str()
            .map(|s| s.chars().take(40).collect::<String>());
        require(
            prefix.as_ref().is_some_and(|prefix| {
                rows(&child, "turnItems").any(|i| {
                    i["type"] == "assistant_message"
                        && i["text"].as_str().is_some_and(|s| s.contains(prefix))
                })
            }),
            "child is missing its native response",
        )?;
        require(
            !rows(&child, "turnItems").any(|i| i["type"] == "reasoning" && i["text"] == progress),
            "progress must not repeat as thinking",
        )?;
        require(
            rows(&p, "turnItems").any(|i| {
                i["type"] == "subagent"
                    && i["subagentId"] == agent["id"]
                    && i["progress"] == progress
            }),
            "missing parent lifecycle/progress item",
        )?;
    }

    Ok(())
}
