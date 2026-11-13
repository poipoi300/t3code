use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};
use serde_json::json;

pub fn assert_grok_prompt_error_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 2, None, Some(&["failed", "completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;
    assert_user_messages_include(
        &p,
        &[
            super::input::GROK_PROMPT_ERROR_PROMPT,
            super::input::GROK_PROMPT_ERROR_FOLLOW_UP,
        ],
    )?;
    let failed = rows(&p, "runs").find(|run| run["ordinal"] == 1);
    let recovered = rows(&p, "runs").find(|run| run["ordinal"] == 2);
    let error = rows(&p, "turnItems").find(|item| {
        item["type"] == "error" && failed.is_some_and(|run| item["runId"] == run["id"])
    });
    require(error.is_some_and(|item|item["failure"]["message"]=="API error (status 400 Bad Request): invalid_request_error: grok prompt error fixture" && item["failure"]["class"]=="provider_error"),"failed Grok run must retain its provider error")?;
    let texts: Vec<_> = rows(&p, "turnItems")
        .filter(|item| {
            item["type"] == "assistant_message"
                && recovered.is_some_and(|run| item["runId"] == run["id"])
        })
        .map(|item| item["text"].clone())
        .collect();
    require(
        json!(texts) == json!(["grok prompt error fixture recovered"]),
        "Grok follow-up did not recover",
    )
}
