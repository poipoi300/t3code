use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const KIRO_COMPACTION_MARKER: &str = r#"compaction-marker-7K2V"#;

pub fn kiro_compaction_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Remember the opaque marker compaction-marker-7K2V for later. Respond with exactly: compaction marker stored"},{"type":"message","text":"/compact"},{"type":"message","text":"Return the opaque marker I asked you to remember. Respond with only the marker."}]}"#,
    )?)
}
