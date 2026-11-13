use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code2_inbox_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        3,
        Some(2),
        Some(&["completed", "completed", "cancelled"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(
        &p,
        &[
            OPENCODE2_STEER_PROMPT,
            OPENCODE2_STEER_TEXT,
            OPENCODE2_QUEUED_PROMPT,
        ],
    )?;
    assert_user_message_input_intents(&p, &["turn_start", "steer", "queued_turn"])?;
    assert_assistant_text_includes(&p, "STEERED")?;
    assert_assistant_text_includes(&p, "QUEUED_B")?;
    require(
        !rows(&p, "turnItems").any(|item| {
            item["type"] == "user_message" && item["text"] == OPENCODE2_CANCELLED_PROMPT
        }),
        "cancelled queued prompt must not become a user item",
    )?;
    let mut runs: Vec<_> = rows(&p, "runs").collect();
    runs.sort_by_key(|run| run["ordinal"].as_u64());
    for run in &runs[..2] {
        let turns: Vec<_> = rows(&p, "providerTurns")
            .filter(|turn| turn["runAttemptId"] == run["activeAttemptId"])
            .collect();
        require(
            turns.len() == 1
                && turns[0]["status"] == "completed"
                && turns[0]["nativeTurnRef"]["nativeId"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("msg_t3_turn_")),
            "inbox turn identity/status/cardinality",
        )?;
    }
    let first = runs[0];
    require(
        rows(&p, "turnItems")
            .find(|item| {
                item["type"] == "assistant_message"
                    && item["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("STEERED"))
            })
            .is_some_and(|item| item["runId"] == first["id"]),
        "native steer must retain first run",
    )?;
    require(
        rows(&p, "turnItems")
            .find(|item| item["type"] == "command_execution")
            .is_some_and(|item| item["status"] == "completed" && item["runId"] == first["id"]),
        "shell must retain first run",
    )
}
