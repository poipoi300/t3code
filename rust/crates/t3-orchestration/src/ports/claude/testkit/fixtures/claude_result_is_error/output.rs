#![cfg(feature = "claude-adapter")]

use crate::{
    error::Result,
    ports::acp_replay::testkit::{fixtures::shared::*, orchestrator_scenario::ScenarioResult},
    testing::replay::ReplayTranscript,
};
use serde_json::{Value, json};

pub fn assert_claude_result_is_error_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    assert_base_projection(result, transcript, 2, None, Some(&["failed", "completed"]))?;
    let p = projection_value_for(result, &transcript.scenario.0)?;
    assert_semantic_projection_integrity(&p)?;

    assert_user_messages_include(
        &p,
        &[
            CLAUDE_RESULT_IS_ERROR_PROMPT,
            CLAUDE_RESULT_IS_ERROR_FOLLOW_UP,
        ],
    )?;
    let failed = rows(&p, "runs")
        .find(|r| r["ordinal"] == 1)
        .unwrap_or(&Value::Null);
    require(failed["status"] == "failed", "first run must fail")?;
    let turn = rows(&p, "providerTurns")
        .find(|t| t["status"] != "completed" && t["status"] != "running")
        .unwrap_or(&Value::Null);
    require(
        turn["status"] == "failed" && turn["turnTokenUsage"].get("models").is_none(),
        "synthetic API notice must not count as an executed model",
    )?;
    let error = rows(&p, "turnItems")
        .find(|i| i["runId"] == failed["id"] && i["type"] == "error")
        .unwrap_or(&Value::Null);
    require(error["type"] == "error", "missing API failure item")?;
    assert_includes_fields(
        &error["failure"],
        &json!({"message":"Claude could not authenticate. For subscription login, run `claude auth login` on this environment's machine, then start a new thread. For API-key authentication, check this instance's configured credentials.","code":"api_error_401","class":"provider_error"}),
        "API failure mapping differs",
    )?;
    let replies = |run: &Value| {
        json!(
            rows(&p, "turnItems")
                .filter(|i| i["runId"] == run["id"] && i["type"] == "assistant_message")
                .map(|i| i["text"].clone())
                .collect::<Vec<_>>()
        )
    };
    require(
        replies(failed)
            == json!(["Failed to authenticate. API Error: 401 Invalid authentication credentials"]),
        "native authentication notice changed",
    )?;
    let recovered = rows(&p, "runs")
        .find(|r| r["ordinal"] == 2)
        .unwrap_or(&Value::Null);
    require(
        recovered["status"] == "completed"
            && replies(recovered) == json!(["claude result is_error fixture recovered"]),
        "follow-up did not recover",
    )?;
    Ok(())
}
