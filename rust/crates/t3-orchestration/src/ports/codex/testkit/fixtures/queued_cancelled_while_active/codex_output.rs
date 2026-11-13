#![cfg(feature = "codex-adapter")]

pub fn assert_queued_cancelled_while_active_output(
    result: &crate::ports::codex::adapters::codex_adapter_v2_testkit::ScenarioResult,
    transcript: &crate::testing::replay::ReplayTranscript,
) -> crate::error::Result<()> {
    use crate::ports::acp_replay::testkit::fixtures::shared;
    shared::assert_base_projection(
        result,
        transcript,
        2,
        Some(1),
        Some(&["completed", "cancelled"]),
    )?;
    let projection = shared::projection_value_for(result, &transcript.scenario.0)?;

    crate::ports::codex::adapters::codex_adapter_v2_testkit::scenario_details(
        "queued_cancelled_while_active",
        result,
        transcript,
        &projection,
    )
}
