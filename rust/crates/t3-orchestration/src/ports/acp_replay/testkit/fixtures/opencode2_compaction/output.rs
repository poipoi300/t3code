use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code2_compaction_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 3, None, Some(&["completed"; 3]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(
        &p,
        &[
            OPENCODE2_COMPACTION_FIRST_PROMPT,
            "/compact",
            OPENCODE2_COMPACTION_RECALL_PROMPT,
        ],
    )?;
    let compactions: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "compaction")
        .collect();
    require(
        compactions.len() == 1
            && compactions[0]["runId"] == p["runs"][1]["id"]
            && compactions[0]["status"] == "completed"
            && compactions[0]["title"] == "Context compacted"
            && compactions[0]["summary"]
                .as_str()
                .is_some_and(|text| text.contains("PAPAYA") && text.contains("## Objective")),
        "native compaction must project summary on compact run",
    )?;
    require(
        !rows(&p, "turnItems")
            .any(|item| item["type"] == "assistant_message" && item["runId"] == p["runs"][1]["id"]),
        "compact must not emit an assistant response",
    )?;
    assert_assistant_text_includes(&p, "PAPAYA")?;
    assert_field_sequence(&p, "providerTurns", "status", &["completed"; 3])
}
