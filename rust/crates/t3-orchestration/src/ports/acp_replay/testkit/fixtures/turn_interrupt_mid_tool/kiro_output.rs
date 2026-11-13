use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};

pub fn assert_turn_interrupt_mid_tool_kiro_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let cancel=transcript.entries.iter().position(|entry|matches!(entry,ReplayEntry::ExpectOutbound(entry) if entry.label.as_ref().and_then(Option::as_ref).is_some_and(|label|label.0=="notification:session/cancel"))).unwrap_or_default();
    let running: Vec<_> = transcript.entries[..cancel]
        .iter()
        .filter_map(|entry| match entry {
            ReplayEntry::EmitInbound(entry)
                if entry.frame["params"]["update"]["status"] == "in_progress" =>
            {
                Some(
                    entry.frame["params"]["update"]["toolCallId"]
                        .as_str()
                        .unwrap_or("undefined"),
                )
            }
            _ => None,
        })
        .collect();
    require(
        super::super::kiro_subagent_interrupt::output::kiro_failed_after_cancel(transcript)?
            .iter()
            .any(|id| running.contains(&id.as_str())),
        "Kiro must fail a command which was running before Stop",
    )?;
    assert_base_projection(result, transcript, 1, None, Some(&["interrupted"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_turn_item_types(
        &p,
        &[
            "user_message",
            "command_execution",
            "run_interrupt_request",
            "run_interrupt_result",
        ],
    )?;
    assert_user_messages_include(&p, &[TURN_INTERRUPT_MID_TOOL_PROMPT])?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "command_execution")
            .is_some_and(|item| {
                item["input"]
                    .as_str()
                    .is_some_and(|input| input.contains("node -e"))
                    && item["status"] == "interrupted"
            }),
        "running command must become interrupted",
    )?;
    require(
        !rows(&p, "turnItems").any(|item| item["status"] == "failed")
            && p["providerTurns"][0]["status"] == "interrupted",
        "Stop must not become provider failure",
    )
}
