use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_open_code2_revert_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "rolled_back"]),
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_user_messages_include(&p, &[OPENCODE2_REVERT_FIRST_PROMPT])?;
    assert_visible_user_messages_exclude(&p, &[OPENCODE2_REVERT_SECOND_PROMPT])?;
    require(
        rows(&p, "providerTurns")
            .find(|turn| turn["ordinal"] == 1)
            .is_some_and(|turn| {
                p["providerThreads"][0]["nativeConversationHeadRef"]["nativeId"]
                    == turn["nativeTurnRef"]["nativeId"]
            }),
        "native head must move to the retained turn",
    )
}
