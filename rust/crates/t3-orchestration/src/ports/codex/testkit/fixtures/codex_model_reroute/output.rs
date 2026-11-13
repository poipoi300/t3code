#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed", "completed"],
    assistant_texts: &[],
    item_types: &["assistant_message"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_codex_model_reroute_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(
        result,
        transcript,
        2,
        None,
        Some(&["completed", "completed"]),
    )?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;

    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "codex_model_reroute",
        result,
        transcript,
        &projection,
    )
}
