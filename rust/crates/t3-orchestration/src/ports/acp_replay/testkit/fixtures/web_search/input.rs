use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn web_search_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Search the web for FIFA World Cup ticket pricing, then answer exactly: web search fixture complete"}]}"#,
    )?)
}
