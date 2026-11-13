use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_pi_thread_rollback_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        3,
        None,
        Some(&["completed", "rolled_back", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_run_ordinals(&p, &[1, 2, 3])?;
    let turn = [
        "user_message",
        "reasoning",
        "assistant_message",
        "checkpoint",
    ];
    assert_turn_item_type_sequence(&p, &turn.repeat(3))?;
    assert_visible_turn_item_type_sequence(&p, &turn.repeat(2))?;
    assert_user_messages_include(
        &p,
        &[
            THREAD_ROLLBACK_FIRST_PROMPT,
            THREAD_ROLLBACK_SECOND_PROMPT,
            THREAD_ROLLBACK_AFTER_PROMPT,
        ],
    )?;
    assert_visible_user_messages_include(
        &p,
        &[THREAD_ROLLBACK_FIRST_PROMPT, THREAD_ROLLBACK_AFTER_PROMPT],
    )?;
    assert_visible_user_messages_exclude(&p, &[THREAD_ROLLBACK_SECOND_PROMPT])?;
    let frames = replay_frames(transcript);
    let forks: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| !*inbound && frame["type"] == "fork")
        .collect();
    let discarded = rows(&p, "providerTurns").find(|turn| turn["ordinal"] == 2);
    require(
        forks.len() == 1
            && discarded.is_some_and(|turn| {
                turn["nativeTurnRef"]["strength"] == "strong"
                    && forks[0].1["entryId"] == turn["nativeTurnRef"]["nativeId"]
            }),
        "rollback must fork exactly at the discarded turn's strong native entry",
    )?;
    require(
        rows(&p, "providerThreads").count() == 1
            && p["providerThreads"][0]["nativeThreadRef"]["nativeId"]
                == "/pi-sessions/session-2.jsonl",
        "Pi rollback must retain one provider thread with the forked file",
    )?;
    require(
        json!(
            frames
                .iter()
                .filter(|(inbound, frame)| !*inbound && frame["type"] == "switch_session")
                .map(|(_, frame)| &frame["sessionPath"])
                .collect::<Vec<_>>()
        ) == json!(["/pi-sessions/session-2.jsonl"]),
        "rollback must switch to the forked session",
    )?;
    require(
        rows(&p, "turnItems")
            .rfind(|item| item["type"] == "assistant_message")
            .is_some_and(|item| {
                item["text"].as_str().is_some_and(|text| {
                    text.contains("rollback fixture first turn complete")
                        && !text.contains("second turn")
                })
            }),
        "rollback retained discarded answer",
    )
}
