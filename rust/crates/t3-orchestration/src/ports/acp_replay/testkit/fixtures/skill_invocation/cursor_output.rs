use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};

pub fn assert_skill_invocation_cursor_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let sent: Vec<_> = transcript
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ReplayEntry::ExpectOutbound(entry) if entry.frame["type"] == "run.start" => {
                entry.frame["message"].as_str()
            }
            _ => None,
        })
        .collect();
    require(
        sent.len() == 1 && sent[0].starts_with(&format!("{SKILL_INVOCATION_CURSOR_MESSAGE}\n\n")),
        "skill mention must reach Cursor as a native slash invocation",
    )?;
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(&p, &[SKILL_INVOCATION_PROMPT])?;
    assert_assistant_text_includes(&p, super::input::SKILL_INVOCATION_FINAL)
}
