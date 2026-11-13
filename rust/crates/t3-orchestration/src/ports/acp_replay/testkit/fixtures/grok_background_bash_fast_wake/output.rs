use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_grok_background_bash_fast_wake_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    super::super::grok_background_bash::output::assert_background_command_output(
        result,
        transcript,
        super::super::grok_background_bash::input::GROK_BACKGROUND_BASH_PROMPT,
        super::super::grok_background_bash::input::GROK_BACKGROUND_BASH_TICKS,
        "Run tock loop in background",
        "command",
    )
}
