use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn open_code2_compaction_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Remember the codeword PAPAYA. Reply OK."},{"type":"message","text":"/compact"},{"type":"message","text":"What was the codeword? One word."}]}"#,
    )?)
}
