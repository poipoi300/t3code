#![cfg(feature = "claude-adapter")]

use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const CLAUDE_MCP_TOOL_PRESENTATION_PROMPT: &str = r###"Do exactly this, one tool call at a time, with no other tool calls:
1) Call the Firecrawl firecrawl_scrape tool on https://example.com with formats ["markdown"].
2) Call the Firecrawl firecrawl_map tool on https://example.com with limit 1.
3) Reply with exactly: mcp presentation fixture complete"###;

pub fn claude_mcp_tool_presentation_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r###"{"steps":[{"type":"message","text":"Do exactly this, one tool call at a time, with no other tool calls:\n1) Call the Firecrawl firecrawl_scrape tool on https://example.com with formats [\"markdown\"].\n2) Call the Firecrawl firecrawl_map tool on https://example.com with limit 1.\n3) Reply with exactly: mcp presentation fixture complete"}]}"###,
    )?)
}
