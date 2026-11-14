#![allow(
    dead_code,
    reason = "Native Claude fixture setup must bind the composed replay harness."
)]

use serde_json::json;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_orchestration::{
    error::{Error, Result},
    ports::acp_replay::testkit::orchestrator_scenario::{ScenarioResult, ScenarioServices},
};

pub async fn assert_provider_native_child_refuses_messages_and_metadata_continuation(
    services: &ScenarioServices,
    result: &ScenarioResult,
) -> Result<()> {
    let child = result
        .projections
        .values()
        .find(|projection| {
            serde_json::to_value(&projection.thread).is_ok_and(|thread| {
                thread["creationSource"] == "provider"
                    && thread["lineage"]["relationshipToParent"] == "subagent"
            })
        })
        .ok_or(Error::Unsupported("Claude recorded subagent projection"))?;
    let IdOrIndex::Id(thread_id) = &child.thread.id else {
        return Err(Error::Unsupported("expanded native child thread ID"));
    };
    let before = services
        .committer
        .latest_sequence(Some(thread_id.clone()))
        .await?;
    for value in [
        json!({"type":"message.dispatch","createdBy":"user","creationSource":"web","commandId":"command:subagent:message-native-child","threadId":thread_id,"messageId":"message:subagent:message-native-child","text":"Also check the tests.","attachments":[],"dispatchMode":{"type":"start_immediately"}}),
        json!({"type":"thread.metadata.update","commandId":"command:subagent:continue-native-child","threadId":thread_id,"title":"Renamed","continuation":{"messageId":"message:subagent:continue-native-child","text":"Also check the tests."}}),
    ] {
        let refused = services
            .dispatcher
            .dispatch(serde_json::from_value(value)?)
            .await
            .expect_err("provider-native child must reject delivery");
        // Rust's canonical rejection carries the source error's user-facing reason.
        match refused {
            Error::Rejected { reason, .. } => assert_eq!(
                reason,
                "This subagent is run by its provider and cannot take messages. Message the parent thread instead."
            ),
            other => panic!("expected native-child rejection, got {other:?}"),
        }
    }
    assert_eq!(
        services
            .committer
            .latest_sequence(Some(thread_id.clone()))
            .await?,
        before
    );
    let after = services
        .projections
        .projection(thread_id.clone())
        .await?
        .ok_or_else(|| Error::ThreadNotFound(thread_id.clone()))?;
    assert!(after.runs.is_empty());
    assert_eq!(after.messages, child.messages);
    assert_eq!(after.thread.title, child.thread.title);
    Ok(())
}
