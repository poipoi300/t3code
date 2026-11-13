#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_claude_mcp_tool_presentation_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;

    assert_user_messages_include(&p, &[super::input::CLAUDE_MCP_TOOL_PRESENTATION_PROMPT])?;
    let tools = rows(&p, "turnItems")
        .filter(|i| i["type"] == "dynamic_tool")
        .collect::<Vec<_>>();
    require(
        json!(tools.iter().map(|i| &i["toolName"]).collect::<Vec<_>>())
            == json!([
                "mcp__claude_ai_Firecrawl__firecrawl_scrape",
                "mcp__claude_ai_Firecrawl__firecrawl_map"
            ]),
        "MCP tool order differs",
    )?;
    let scrape = tools[0];
    let map = tools[1];
    require(
        scrape["status"] == "completed"
            && scrape["title"] == "Firecrawl scrape"
            && scrape["toolIcon"] == scrape["toolSource"]["icon"],
        "scrape presentation differs",
    )?;
    require(
        scrape["toolSource"]
            == json!({"key":"mcp:firecrawl","name":"Firecrawl","kind":"integration","icon":{"_tag":"themed-logo","logoUrl":"https://www.google.com/s2/favicons?domain=firecrawl.dev&sz=64"}}),
        "scrape integration source differs",
    )?;
    require(
        map["status"] == "completed"
            && map["title"] == "firecrawl map"
            && map["toolSource"]
                == json!({"key":"mcp:firecrawl","name":"Firecrawl","kind":"integration"}),
        "map presentation differs",
    )?;
    Ok(())
}
