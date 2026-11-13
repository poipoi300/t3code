use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const PI_COMPACTION_MARKER: &str = r#"compaction-marker-4Z7Q"#;

pub fn pi_compaction_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Remember the opaque marker compaction-marker-4Z7Q for later. Respond with exactly: compaction marker stored"},{"type":"message","text":"/compact keep the opaque marker compaction-marker-4Z7Q"},{"type":"message","text":"Respond with exactly: second compaction fixture turn"},{"type":"message","text":"Respond with exactly: third compaction fixture turn"},{"type":"message","text":"/compact"},{"type":"message","text":"Return the opaque marker I asked you to remember. Respond with only the marker."}]}"#,
    )?)
}
