use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_muse_subagent_v2_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[SUBAGENT_V2_PROMPT])?;
    assert_field_sequence(&p, "runs", "status", &["completed"])?;
    require(
        rows(&p, "subagents").count() == 1 && result.shell_snapshot.threads.len() == 2,
        "expected one native Muse child",
    )?;
    let subagent = &p["subagents"][0];
    require(
        subagent["origin"] == "provider_native"
            && subagent["driver"] == "muse"
            && subagent["status"] == "completed"
            && subagent["prompt"]
                .as_str()
                .is_some_and(|prompt| !prompt.is_empty())
            && matches!(subagent["result"].as_str(), Some("Hello" | "Hello."))
            && !subagent["completedAt"].is_null()
            && !subagent["childThreadId"].is_null(),
        "Muse child identity/prompt/result",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "subagent" && item["subagentId"] == subagent["id"])
            .is_some_and(|item| item["childThreadId"] == subagent["childThreadId"]),
        "subagent row must name child thread",
    )?;
    let child = result
        .projections
        .get(subagent["childThreadId"].as_str().unwrap_or_default())
        .map(serde_json::to_value)
        .transpose()?;
    require(child.is_some(), "missing child projection")?;
    let child = child.expect("child asserted");
    require(
        child["thread"]["lineage"]["parentThreadId"] == p["thread"]["id"]
            && child["thread"]["lineage"]["relationshipToParent"] == "subagent"
            && rows(&child, "runs").next().is_none(),
        "Muse native child lineage/app runs",
    )?;
    require(
        json!(
            rows(&child, "messages")
                .map(|message| json!([message["role"], message["text"]]))
                .collect::<Vec<_>>()
        ) == json!([
            ["user", subagent["prompt"]],
            ["assistant", subagent["result"]]
        ]),
        "Muse child must contain prompt and result only",
    )?;
    require(
        rows(&p["providerThreads"][0], "pendingBackgroundTasks")
            .next()
            .is_none(),
        "finished native child must clear pending work",
    )
}
