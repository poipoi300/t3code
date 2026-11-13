use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_proposed_plan_cursor_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_run_ordinals(&p, &[1])?;
    assert_execution_node_kinds(&p, &["root_turn", "plan", "subagent", "tool_call"])?;
    assert_turn_item_types(
        &p,
        &["user_message", "subagent", "file_search", "proposed_plan"],
    )?;
    assert_user_messages_include(&p, &[PROPOSED_PLAN_PROMPT])?;
    let plan = rows(&p, "plans").rfind(|plan| plan["kind"] == "proposed_plan");
    require(
        plan.is_some_and(|plan| {
            plan["markdown"]
                .as_str()
                .is_some_and(|text| text.contains("Deterministic Replay Fixtures"))
        }),
        "missing final proposed plan",
    )?;
    let item = rows(&p, "turnItems").rfind(|item| item["type"] == "proposed_plan");
    require(
        item.is_some_and(|item| item["streaming"] == false),
        "proposed plan must finish streaming",
    )?;
    require(rows(&p, "subagents").next().is_some(), "missing subagent")
}
