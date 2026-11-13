use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::json;

pub fn assert_pi_provider_thread_resume_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let frames = replay_frames(transcript);
    require(
        frames
            .iter()
            .filter(|(inbound, frame)| !*inbound && frame["type"] == "process_start")
            .count()
            == 2,
        "idle release must retire the first Pi process",
    )?;
    let switches: Vec<_> = transcript
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ReplayEntry::ExpectOutbound(entry) if entry.frame["type"] == "switch_session" => {
                Some(json!([entry.label, entry.frame["sessionPath"]]))
            }
            _ => None,
        })
        .collect();
    require(
        json!(switches) == json!([["switch_session@p2", "/pi-sessions/session-1.jsonl"]]),
        "respawn must resume the first session file",
    )?;
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "completed"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(
        &p,
        &[
            PROVIDER_THREAD_RESUME_FIRST_PROMPT,
            PROVIDER_THREAD_RESUME_SECOND_PROMPT,
        ],
    )?;
    require(
        rows(&p, "providerThreads").count() == 1
            && p["providerThreads"][0]["nativeThreadRef"]["nativeId"]
                == "/pi-sessions/session-1.jsonl",
        "resume must keep one native provider thread",
    )?;
    let answers: Vec<_> = rows(&p, "turnItems")
        .filter(|item| item["type"] == "assistant_message")
        .filter_map(|item| item["text"].as_str())
        .collect();
    require(
        answers.len() == 2
            && answers[1].contains("provider thread resume fixture first turn complete")
            && answers[1].contains("provider thread resume fixture second turn complete"),
        "resumed session must remember its first answer",
    )
}
