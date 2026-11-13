use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};

pub fn kiro_failed_after_cancel(transcript: &ReplayTranscript) -> Result<Vec<String>> {
    let cancel=transcript.entries.iter().position(|entry|matches!(entry,ReplayEntry::ExpectOutbound(entry) if entry.label.as_ref().and_then(Option::as_ref).is_some_and(|label|label.0=="notification:session/cancel")));
    require(cancel.is_some(), "Stop must reach Kiro as session/cancel")?;
    Ok(transcript.entries[cancel.expect("cancel asserted")..]
        .iter()
        .filter_map(|entry| match entry {
            ReplayEntry::EmitInbound(entry)
                if entry.frame["params"]["update"]["sessionUpdate"] == "tool_call_update"
                    && entry.frame["params"]["update"]["status"] == "failed" =>
            {
                Some(
                    entry.frame["params"]["update"]["toolCallId"]
                        .as_str()
                        .unwrap_or("undefined")
                        .into(),
                )
            }
            _ => None,
        })
        .collect())
}
pub fn assert_kiro_subagent_interrupt_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let failed = kiro_failed_after_cancel(transcript)?;
    require(
        failed.iter().any(|id| id.starts_with("invoke_subagent_"))
            && failed.iter().any(|id| !id.starts_with("invoke_subagent_")),
        "Kiro must fail its subagent and running command after Stop",
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_field_sequence(&p, "runs", "status", &["interrupted"])?;
    require(
        !rows(&p, "turnItems")
            .any(|item| matches!(item["status"].as_str(), Some("running" | "failed"))),
        "cancelled items must be interrupted",
    )?;
    require(
        rows(&p, "subagents").count() == 1 && p["subagents"][0]["status"] == "interrupted",
        "subagent must be interrupted",
    )?;
    let child = result
        .projections
        .get(
            p["subagents"][0]["childThreadId"]
                .as_str()
                .unwrap_or_default(),
        )
        .map(serde_json::to_value)
        .transpose()?;
    require(child.is_some(), "missing child projection")?;
    let child = child.expect("child asserted");
    let tools: Vec<_> = rows(&child, "turnItems")
        .filter(|item| item["type"] == "dynamic_tool")
        .collect();
    require(
        !tools.is_empty()
            && tools.iter().all(|tool| tool["status"] == "interrupted")
            && !rows(&child, "nodes")
                .any(|node| matches!(node["status"].as_str(), Some("running" | "failed"))),
        "child tools/nodes must settle as interrupted",
    )
}
