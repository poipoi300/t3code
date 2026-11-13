use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn plan_questions_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"interactionMode":"plan","steps":[{"type":"message","text":"Use request_user_input to ask one multiple-choice clarifying question about whether this fixture should prefer strict schemas or UI flexibility. After receiving the answer, respond exactly: plan questions fixture complete"},{"type":"answer_next_user_input_request","answers":{"schema_vs_ui_flexibility":"Strict schemas (Recommended)","schema_preference":"Strict schemas (Recommended)","question-0-schema-vs-flexibility":"Strict schemas"}}]}"#,
    )?)
}
