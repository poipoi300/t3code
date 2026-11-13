use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_open_code2_subagent_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let prompt = "List the files in the current directory and report their names.";
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_no_extra_app_runs_for_provider_children(&p, 1)?;
    assert_provider_native_subagent_root_turns(result)?;
    assert_user_messages_include(&p, &[OPENCODE2_SUBAGENT_PROMPT])?;
    assert_assistant_text_includes(&p, "hello.txt")?;
    require(
        rows(&p, "subagents").count() == 1,
        "expected one native subagent",
    )?;
    let subagent = &p["subagents"][0];
    assert_includes_fields(
        subagent,
        &json!({"origin":"provider_native","status":"completed","prompt":prompt,"title":"List files in current directory","runId":p["runs"][0]["id"]}),
        "native subagent metadata",
    )?;
    require(
        subagent["result"]
            .as_str()
            .is_some_and(|text| text.contains("hello.txt") && !text.contains("<subagent")),
        "native result must be the answer without its tag",
    )?;
    require(
        json!(
            rows(&p, "turnItems")
                .filter(|item| item["type"] == "subagent")
                .map(|item| &item["status"])
                .collect::<Vec<_>>()
        ) == json!(["completed"])
            && !rows(&p, "turnItems")
                .any(|item| matches!(item["type"].as_str(), Some("dynamic_tool" | "file_search"))),
        "native child tools must not leak into parent",
    )?;
    let child = result
        .projections
        .get(subagent["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(child.is_some(), "missing child projection")?;
    let child = child.expect("child asserted");
    require(
        child["thread"]["lineage"]["relationshipToParent"] == "subagent",
        "native child lineage",
    )?;
    assert_field_sequence(&child, "providerTurns", "status", &["completed"])?;
    assert_user_messages_include(&child, &[prompt])?;
    require(
        rows(&child, "turnItems")
            .filter(|item| matches!(item["type"].as_str(), Some("dynamic_tool" | "file_search")))
            .count()
            >= 2,
        "native child must contain read/glob calls",
    )?;
    assert_assistant_text_includes(&child, "hello.txt")?;
    require(
        p["providerTurns"][0]["turnTokenUsage"]["hasSubagents"] == true,
        "parent usage must record subagents",
    )
}
