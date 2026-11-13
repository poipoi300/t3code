use super::super::shared::*;
use crate::{
    error::Result,
    ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::{ReplayEntry, ReplayTranscript},
};
use serde_json::{Value, json};
use t3_contracts::orchestration_v2::{
    OrchestrationV2PendingBackgroundTask, OrchestrationV2ProviderThread, OrchestrationV2Run,
    OrchestrationV2TurnItem,
};

// Local shim: shared pending-work derivation must be available outside the client crate.
pub trait PendingBackgroundWorkDeriver {
    fn derive(
        &self,
        latest_run: &OrchestrationV2Run,
        provider_threads: &[OrchestrationV2ProviderThread],
        turn_items: &[OrchestrationV2TurnItem],
    ) -> Result<Vec<OrchestrationV2PendingBackgroundTask>>;
}
pub fn assert_grok_monitor_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
    pending_work: &dyn PendingBackgroundWorkDeriver,
) -> Result<()> {
    super::super::grok_background_bash::output::assert_background_command_output(
        result,
        transcript,
        super::input::GROK_MONITOR_PROMPT,
        super::input::GROK_MONITOR_TICKS,
        "Watch three tick echoes",
        "monitor",
    )?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    let monitor = rows(&p, "turnItems").find(|item| item["type"] == "command_execution");
    require(monitor.is_some(), "missing monitor")?;
    let events: Vec<Value> = result
        .domain_events
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<_, _>>()?;
    let running = events
        .iter()
        .filter(|event| {
            event["type"] == "turn-item.updated"
                && event["payload"]["id"]
                    == monitor.expect("fixture structure validated above")["id"]
                && event["payload"]["type"] == "command_execution"
        })
        .map(|event| &event["payload"])
        .find(|item| item["status"] == "running");
    require(running.is_some(), "missing running monitor update")?;
    let item: OrchestrationV2TurnItem =
        serde_json::from_value(running.expect("fixture structure validated above").clone())?;
    let root: OrchestrationV2Run = serde_json::from_value(p["runs"][0].clone())?;
    let work = pending_work.derive(&root, &[], std::slice::from_ref(&item))?;
    require(
        json!(work)
            .as_array()
            .is_some_and(|tasks| tasks.len() == 1 && tasks[0]["kind"] == "monitor"),
        "persistent monitor must remain pending after its app run",
    )?;
    let cancelled =
        crate::ports::runs_ingestion::restart_background_note::cancelled_turn_item_work(
            &item, None,
        )?;
    require(
        cancelled
            .map(serde_json::to_value)
            .transpose()?
            .is_some_and(|work| work["kind"] == "monitor"),
        "restart note must name persistent work as a monitor",
    )?;
    let answer = transcript.entries.iter().find_map(|entry| match entry {
        ReplayEntry::ExpectOutbound(entry)
            if entry.frame["kind"] == "response"
                && entry.frame["method"] == "session/request_permission" =>
        {
            Some(&entry.frame["result"])
        }
        _ => None,
    });
    require(
        answer == Some(&json!({"outcome":{"outcome":"selected","optionId":"allow-once"}})),
        "monitor permission response",
    )
}
