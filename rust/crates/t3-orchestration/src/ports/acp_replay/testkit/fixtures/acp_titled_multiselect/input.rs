use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const ACP_TITLED_MULTISELECT_PROMPT: &str = r#"Ask which areas this fixture should cover, allowing several. After receiving the answer, respond exactly: titled multi-select fixture complete"#;

pub fn acp_titled_multi_select_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"interactionMode":"plan","steps":[{"type":"message","text":"Ask which areas this fixture should cover, allowing several. After receiving the answer, respond exactly: titled multi-select fixture complete"},{"type":"answer_next_user_input_request","answers":{"fixture_areas":["ui","unknown","schemas"]}}]}"#,
    )?)
}
