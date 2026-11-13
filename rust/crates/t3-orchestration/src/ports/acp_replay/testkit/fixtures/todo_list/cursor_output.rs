use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_todo_list_cursor_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_run_ordinals(&p, &[1])?;
    assert_execution_node_kinds(&p, &["root_turn", "todo_list", "tool_call"])?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "file_search",
            "todo_list",
            "assistant_message",
        ],
    )?;
    assert_user_messages_include(&p, &[TODO_LIST_PROMPT])?;
    assert_assistant_text_includes(&p, "todo list fixture complete")?;
    let conversational: Vec<_> = rows(&p, "turnItems")
        .filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("assistant_message" | "reasoning")
            )
        })
        .collect();
    require(
        json!(
            conversational
                .iter()
                .map(|item| &item["type"])
                .collect::<Vec<_>>()
        ) == json!([
            "reasoning",
            "assistant_message",
            "reasoning",
            "reasoning",
            "reasoning",
            "reasoning",
            "assistant_message",
            "reasoning",
            "assistant_message"
        ]),
        "Cursor conversational segment order",
    )?;
    let messages: Vec<_> = conversational
        .iter()
        .filter(|item| item["type"] == "assistant_message")
        .collect();
    require(
        messages.len() == 3
            && messages[0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("Checking for the `update_plan` tool"))
            && messages[1]["text"]
                .as_str()
                .is_some_and(|text| text.contains("`update_plan` isn't available"))
            && messages[2]["text"] == "todo list fixture complete",
        "unexpected Cursor progress/final response",
    )?;
    require(
        conversational
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .count()
            == 6,
        "reasoning segments must remain separate",
    )?;
    let todo = rows(&p, "plans").rfind(|plan| plan["kind"] == "todo_list");
    require(
        todo.is_some_and(|todo| {
            json!(values(todo, "steps", "status")) == json!(["completed", "completed", "completed"])
        }),
        "todo list did not complete its three steps",
    )
}
