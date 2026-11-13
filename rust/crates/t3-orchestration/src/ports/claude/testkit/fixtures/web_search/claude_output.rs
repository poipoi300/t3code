#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_claude_web_search_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;

    assert_execution_node_kinds(&p, &["root_turn", "tool_call", "assistant_message"])?;
    assert_conversation_message_roles(&p, &["user", "assistant"])?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "dynamic_tool",
            "web_search",
            "assistant_message",
        ],
    )?;
    assert_runtime_request_counts(&p, 0, None)?;
    assert_user_messages_include(&p, &[WEB_SEARCH_PROMPT])?;
    assert_assistant_text_includes(&p, "web search fixture complete")?;
    require(
        rows(&p, "turnItems")
            .filter(|i| i["type"] == "dynamic_tool" && i["toolName"] == "ToolSearch")
            .count()
            == 1,
        "missing ToolSearch",
    )?;
    let searches = rows(&p, "turnItems")
        .filter(|i| i["type"] == "web_search")
        .collect::<Vec<_>>();
    require(searches.len() == 1, "expected one web search")?;
    let search = searches[0];
    require(
        search["status"] == "completed"
            && search["patterns"]
                .as_array()
                .is_some_and(|p| p.contains(&json!("FIFA World Cup 2026 ticket pricing"))),
        "search query/status differs",
    )?;
    require(
        search["results"].as_array().is_some_and(|r| {
            !r.is_empty()
                && r.iter().any(|r| {
                    r["url"]
                        .as_str()
                        .is_some_and(|url| url.contains("fifa.com"))
                })
        }),
        "missing structured search URLs",
    )?;
    Ok(())
}
