#![cfg(feature = "codex-adapter")]

use crate::ports::codex::adapters::codex_adapter_v2_testkit::FixtureExpectation;

pub const EXPECTATION: FixtureExpectation = FixtureExpectation {
    terminal_statuses: &["completed", "completed", "completed"],
    assistant_texts: &["first turn complete"],
    item_types: &["assistant_message"],
    request_kinds: &[],
    subagent_results: None,
};

pub fn assert_thread_rollback_after_restart_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;

    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "thread_rollback_after_restart",
        result,
        transcript,
        &projection,
    )
}
