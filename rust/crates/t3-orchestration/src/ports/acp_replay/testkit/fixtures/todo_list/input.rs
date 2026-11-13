use crate::ports::acp_replay::testkit::fixtures::shared::OrchestratorFixtureInput;

pub fn todo_list_input() -> crate::error::Result<OrchestratorFixtureInput> {
    Ok(serde_json::from_str(
        r#"{"steps":[{"type":"message","text":"Use the update_plan tool to track exactly three steps: inspect package.json, inspect tsconfig.json, report completion. Then read package.json and tsconfig.json, and answer exactly: todo list fixture complete"}]}"#,
    )?)
}
