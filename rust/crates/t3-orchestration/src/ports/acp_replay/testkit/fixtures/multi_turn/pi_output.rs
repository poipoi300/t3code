use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_pi_multi_turn_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_multi_turn_fixture_output(result, transcript)?;
    super::super::simple::pi_output::assert_pi_settled_token_usage(result, transcript)
}
