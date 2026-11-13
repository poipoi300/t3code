use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_todo_list_grok_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_execution_node_kinds(&p, &["root_turn", "todo_list", "tool_call"])?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "todo_list",
            "dynamic_tool",
            "assistant_message",
        ],
    )?;
    assert_user_messages_include(&p, &[TODO_LIST_PROMPT])?;
    assert_assistant_text_includes(&p, "todo list fixture complete")?;
    if transcript.provider.0 == "grok" {
        assert_no_acp_client_file_or_terminal_requests(transcript)?;
    }
    let events: Vec<_> = result
        .domain_events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<_, _>>()?;
    let plans: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "plan.updated")
        .collect();
    require(
        plans.len() == 2 && plans[0]["payload"]["id"] == plans[1]["payload"]["id"],
        "ACP updates must preserve one plan identity",
    )?;
    let todo = rows(&p, "plans").rfind(|plan| plan["kind"] == "todo_list");
    require(
        todo.is_some_and(|todo| {
            json!(values(todo, "steps", "status")) == json!(["completed", "completed", "completed"])
        }),
        "todo steps must finish",
    )
}
