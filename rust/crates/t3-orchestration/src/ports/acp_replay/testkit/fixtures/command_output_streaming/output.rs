use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use t3_contracts::base_schemas::TurnItemId;

// Local shim: the shared ANSI screen fold exposes these operations with the same state model.
pub trait TerminalOutputAssertions {
    fn normalize_text(&self, source: &str) -> Result<String>;
    fn plain_text(&self, text: &str) -> Result<String>;
}
pub fn assert_command_output_streaming_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
    terminal: &dyn TerminalOutputAssertions,
) -> Result<()> {
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_visible_turn_items_mirror_local_turn_items(&p)?;
    assert_user_messages_include(&p, &[COMMAND_OUTPUT_STREAMING_PROMPT])?;
    let command = rows(&p, "turnItems").find(|item| item["type"] == "command_execution");
    require(command.is_some(), "missing command")?;
    let command = command.expect("command asserted");
    let output = command["output"].as_str().unwrap_or_default();
    require(
        command["status"] == "completed" && output.contains("progress 100%"),
        "command must finish at 100%",
    )?;
    let streamed = result.command_output.as_ref().and_then(|recorded| {
        recorded.get(&TurnItemId(
            command["id"].as_str().unwrap_or_default().into(),
        ))
    });
    require(
        streamed.is_some_and(|streamed| streamed.chunks > 0),
        "running command must stream its output",
    )?;
    let live_text = streamed
        .and_then(|streamed| streamed.output["text"].as_str())
        .unwrap_or_default();
    let live = terminal.plain_text(live_text)?;
    for line in [
        "stream line 2",
        "stream line 3",
        "stream line 4",
        "progress 100%",
    ] {
        require(
            live.contains(line),
            format!("missing live command line {line}"),
        )?;
    }
    require(
        live_text.contains("\u{001b}[0;32mstream line 2\u{001b}[0m")
            && !live.contains('\u{001b}')
            && !live.contains("progress 50%"),
        "live terminal folding must retain SGR and overwrite progress",
    )?;
    let final_text = terminal.plain_text(&terminal.normalize_text(output)?)?;
    require(
        final_text.trim_end().ends_with(live.trim_end()) && !final_text.contains("\"stdout\""),
        "live tail must agree with persisted normalized final output",
    )?;
    for event in &result.domain_events {
        let event = serde_json::to_value(event)?;
        if event["type"] == "turn-item.updated"
            && event["payload"]["id"] == command["id"]
            && event["payload"]["type"] == "command_execution"
            && event["payload"]["status"] != "completed"
        {
            require(
                event["payload"].get("output").is_none(),
                "running command items must not persist live output",
            )?;
        }
    }
    Ok(())
}
