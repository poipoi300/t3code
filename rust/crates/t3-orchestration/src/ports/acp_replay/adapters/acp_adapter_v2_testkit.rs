pub use crate::testing::replay::{
    ReplayCursor, ReplayEntry, ReplayError, ReplayGate, ReplayStatus, ReplayTranscript,
};

pub fn decode_acp_replay_transcript(
    transcript: ReplayTranscript,
    expected_provider: &str,
    retarget_provider: bool,
) -> Result<ReplayTranscript, ReplayError> {
    crate::testing::replay::decode_acp(transcript, expected_provider, retarget_provider)
}

pub fn make_turn_input(
    thread: t3_contracts::orchestration_v2::OrchestrationV2ProviderThread,
    selection: t3_contracts::model_selection::ModelSelection,
    policy: crate::provider::RuntimePolicy,
    now: t3_contracts::base_schemas::IsoDateTime,
    ordinal: u64,
    message: &str,
) -> crate::provider::ProviderResult<crate::provider::TurnInput> {
    use t3_contracts::{
        base_schemas::*, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    };
    let thread_id = match &thread.app_thread_id {
        Some(IdOrIndex::Id(thread)) => thread.clone(),
        _ => {
            return Err(crate::provider::ProviderError::Protocol {
                reason: None,
                detail: "test turn needs an expanded app thread ID".into(),
                payload: None,
            });
        }
    };
    let suffix = format!("{}:{ordinal}", thread_id.0);
    let app_thread = serde_json::from_value(serde_json::json!({"createdBy":"user","creationSource":"web","id":thread_id,"projectId":format!("project:{}",thread_id.0),"title":"ACP adapter test","providerInstanceId":selection.instance_id,"modelSelection":selection,"runtimeMode":"approval-required","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":thread.id,"lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":thread_id},"forkedFrom":null,"createdAt":now,"updatedAt":now,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null})).map_err(|error| crate::provider::ProviderError::Protocol { reason: None, detail: error.to_string(), payload: None })?;
    Ok(crate::provider::TurnInput {
        app_thread,
        thread_id,
        run_id: RunId(format!("run:{suffix}")),
        run_ordinal: ordinal,
        native_thread_has_turns: None,
        provider_turn_ordinal: ordinal,
        restart_continuation_of_run_id: None,
        attempt_id: RunAttemptId(format!("attempt:{suffix}")),
        root_node_id: NodeId(format!("node:{suffix}")),
        provider_thread: thread,
        message: crate::provider::TurnMessage {
            message_id: MessageId(format!("message:{suffix}")),
            text: message.into(),
            attachments: Vec::new(),
            created_by: OrchestrationV2Actor::User,
            creation_source: Some(OrchestrationV2CreationSource::Web),
            scheduled_task_id: None,
            sender_thread_id: None,
        },
        model_selection: selection,
        runtime_policy: policy,
        app_context: Vec::new(),
    })
}
