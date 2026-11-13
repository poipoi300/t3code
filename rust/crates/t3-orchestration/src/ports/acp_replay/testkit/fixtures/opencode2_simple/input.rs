use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_simple_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Think carefully step by step about whether 391 is prime, showing your reasoning, then answer in one short sentence."}]}"#,
    )?)
}
