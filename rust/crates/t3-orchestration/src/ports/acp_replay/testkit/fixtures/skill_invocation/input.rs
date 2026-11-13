use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub const SKILL_INVOCATION_FINAL: &str = r#"review skill fixture complete"#;

pub fn skill_invocation_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"workspaceFiles":{".cursor/skills/review/SKILL.md":"---\nname: review\ndescription: Review the named file for the skill invocation fixture.\n---\nDo not use any tools. Respond with exactly: review skill fixture complete\n"},"steps":[{"type":"message","text":"$review README.md"}]}"#,
    )?)
}
