use super::super::shared::*;
use crate::{
    error::Result, ports::acp_replay::testkit::orchestrator_scenario::ScenarioResult,
    testing::replay::ReplayTranscript,
};

pub fn assert_grok_subagent_lineage_output(
    result: &ScenarioResult,
    transcript: &ReplayTranscript,
) -> Result<()> {
    let expected = [
        (
            "Explore server architecture",
            "019f0220-e192-7c41-9e9d-b406bc3459c8",
            "I'll audit `apps/server` by mapping its layout first",
            "# `apps/server` Audit Summary",
            "Server audit complete.",
        ),
        (
            "Explore web client architecture",
            "019f0220-e197-7833-a8e4-ad38f2bd5b4c",
            "Auditing `apps/web`: mapping package structure",
            "# `apps/web` Audit Summary",
            "Web audit complete.",
        ),
    ];
    assert_base_projection(result, transcript, 1, None, Some(&["completed"]))?;
    let parent = projection_value_for(result, &transcript.scenario.0)?;
    assert_turn_item_types(
        &parent,
        &["user_message", "subagent", "subagent", "assistant_message"],
    )?;
    assert_execution_node_kinds(
        &parent,
        &["root_turn", "subagent", "subagent", "assistant_message"],
    )?;
    assert_no_extra_app_runs_for_provider_children(&parent, 1)?;
    require(
        rows(&parent, "subagents").count() == 2 && result.shell_snapshot.threads.len() == 3,
        "expected two native child sessions",
    )?;
    require(
        !rows(&parent, "messages").any(|message| {
            message["text"]
                .as_str()
                .is_some_and(|text| text.contains("Audit Summary"))
        }),
        "child output leaked into parent",
    )?;
    for (title, session, first, second, final_result) in expected {
        let subagent = rows(&parent, "subagents").find(|subagent| subagent["title"] == title);
        require(subagent.is_some(), format!("missing child {title}"))?;
        let subagent = subagent.expect("fixture structure validated above");
        require(
            subagent["status"] == "completed"
                && subagent["origin"] == "provider_native"
                && subagent["driver"] == "grok"
                && subagent["result"] == final_result,
            "native child status/result",
        )?;
        require(
            !subagent["childThreadId"].is_null() && !subagent["providerThreadId"].is_null(),
            "native child lacks provider/app lineage",
        )?;
        let child = result
            .projections
            .get(subagent["childThreadId"].as_str().unwrap_or_default())
            .map(serde_json::to_value)
            .transpose()?;
        require(child.is_some(), "missing child projection")?;
        let child = child.expect("fixture structure validated above");
        require(
            child["thread"]["lineage"]["parentThreadId"] == parent["thread"]["id"]
                && child["thread"]["lineage"]["relationshipToParent"] == "subagent"
                && child["thread"]["lineage"]["rootThreadId"]
                    == parent["thread"]["lineage"]["rootThreadId"]
                && child["thread"]["activeProviderThreadId"] == subagent["providerThreadId"],
            "native child thread lineage",
        )?;
        require(
            rows(&child, "runs").count() == 0
                && rows(&child, "providerThreads").count() == 1
                && child["providerThreads"][0]["nativeThreadRef"]["nativeId"] == session,
            "native child session identity",
        )?;
        let fork = &child["providerThreads"][0]["forkedFrom"];
        require(
            !fork.is_null()
                && fork["providerThreadId"] != subagent["providerThreadId"]
                && rows(&parent, "providerTurns").any(|turn| {
                    turn["id"] == fork["providerTurnId"]
                        && turn["providerThreadId"] == fork["providerThreadId"]
                }),
            "native child must fork from parent provider turn",
        )?;
        assert_turn_item_types(&child, &["user_message", "assistant_message"])?;
        let assistant = rows(&child, "messages").find(|message| message["role"] == "assistant");
        require(assistant.is_some(), "missing child assistant response")?;
        let text = assistant.expect("fixture structure validated above")["text"]
            .as_str()
            .unwrap_or_default();
        require(
            text.find(first)
                .zip(text.find(second))
                .is_some_and(|(first, second)| first < second),
            "child chunks lost their native order",
        )?;
        for (_, other_session, _, other_second, _) in expected {
            if other_session != session {
                require(
                    !text.contains(other_second),
                    "interleaved chunks leaked across child sessions",
                )?;
            }
        }
    }
    Ok(())
}
