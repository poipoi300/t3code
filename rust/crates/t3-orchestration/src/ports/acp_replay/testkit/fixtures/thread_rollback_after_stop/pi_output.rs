use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};

pub fn assert_pi_thread_rollback_after_stop_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 4, None, None)?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_run_ordinals(&p, &[1, 2, 3, 4])?;
    for (ordinal, status) in [
        (1, "completed"),
        (2, "rolled_back"),
        (3, "rolled_back"),
        (4, "completed"),
    ] {
        require(
            rows(&p, "runs")
                .find(|run| run["ordinal"] == ordinal)
                .is_some_and(|run| run["status"] == status),
            "rollback-after-stop run status",
        )?;
    }
    let stopped = rows(&p, "providerTurns").find(|turn| turn["ordinal"] == 2);
    require(
        stopped.is_some_and(|turn| {
            turn["status"] == "interrupted" && turn["nativeTurnRef"]["strength"] == "strong"
        }),
        "stopped turn must keep strong native identity",
    )?;
    let stop = transcript.entries.iter().position(
        |entry| matches!(entry,ReplayEntry::ExpectOutbound(entry) if entry.frame["type"]=="abort"),
    );
    let capture=transcript.entries.iter().enumerate().find(|(index,entry)|stop.is_none_or(|stop|*index>stop) && matches!(entry,ReplayEntry::ExpectOutbound(entry) if entry.frame["type"]=="get_entries")).map(|(index,_)|index);
    require(
        stop.zip(capture)
            .is_some_and(|(stop, capture)| capture > stop),
        "Stop must capture the session tree before terminating Pi",
    )?;
    let frames = replay_frames(transcript);
    let forks: Vec<_> = frames
        .iter()
        .filter(|(inbound, frame)| !*inbound && frame["type"] == "fork")
        .collect();
    require(
        forks.len() == 1
            && stopped
                .is_some_and(|turn| forks[0].1["entryId"] == turn["nativeTurnRef"]["nativeId"]),
        "rollback must fork at stopped turn's native entry",
    )?;
    assert_visible_user_messages_include(
        &p,
        &[THREAD_ROLLBACK_FIRST_PROMPT, THREAD_ROLLBACK_AFTER_PROMPT],
    )?;
    assert_visible_user_messages_exclude(
        &p,
        &[
            TURN_INTERRUPT_MID_TOOL_PROMPT,
            THREAD_ROLLBACK_SECOND_PROMPT,
        ],
    )?;
    require(
        rows(&p, "turnItems")
            .rfind(|item| item["type"] == "assistant_message")
            .is_some_and(|item| {
                item["text"].as_str().is_some_and(|text| {
                    text.contains("rollback fixture first turn complete")
                        && !text.contains("second turn")
                        && !text.contains("interrupt fixture")
                })
            }),
        "rollback retained stopped/discarded answer",
    )
}
