use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventThreadCreated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2AppThread,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventThreadPullRequestLinkUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: crate::thread_pull_request::ThreadPullRequestLink,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventRunCreated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2Run,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventRunBackgroundWorkCancelled {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2RunBackgroundWorkCancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventRunAttemptCreated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2RunAttempt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventNodeUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2ExecutionNode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventSubagentUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2Subagent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventProviderSessionAttached {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2ProviderSession,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventProviderSessionDetached {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2ProviderSessionDetached,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventProviderThreadUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2ProviderThread,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventProviderTurnUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2ProviderTurn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventRuntimeRequestUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2RuntimeRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventMessageUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2ConversationMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventTurnItemUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2TurnItem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventPlanUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2PlanArtifact,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventCheckpointScopeCreated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2CheckpointScope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventCheckpointCaptured {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2Checkpoint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventCheckpointRollbackRequested {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2CheckpointRollbackRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventContextHandoffUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2ContextHandoff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventContextTransferCreated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2ContextTransfer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationV2DomainEvent {
    #[serde(rename = "thread.created")]
    ThreadCreated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.archived")]
    ThreadArchived(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.unarchived")]
    ThreadUnarchived(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.deleted")]
    ThreadDeleted(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.settled")]
    ThreadSettled(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.unsettled")]
    ThreadUnsettled(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.snoozed")]
    ThreadSnoozed(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.unsnoozed")]
    ThreadUnsnoozed(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.pinned")]
    ThreadPinned(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.auto-settle-set")]
    ThreadAutoSettleSet(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.unpinned")]
    ThreadUnpinned(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.pin-reordered")]
    ThreadPinReordered(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.active-reordered")]
    ThreadActiveReordered(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.tags-updated")]
    ThreadTagsUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.visited")]
    ThreadVisited(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.marked-unread")]
    ThreadMarkedUnread(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.metadata-updated")]
    ThreadMetadataUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.pull-request-synced")]
    ThreadPullRequestSynced(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.runtime-mode-updated")]
    ThreadRuntimeModeUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.interaction-mode-updated")]
    ThreadInteractionModeUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.model-selection-updated")]
    ThreadModelSelectionUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.provider-switched")]
    ThreadProviderSwitched(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.guest-share-updated")]
    ThreadGuestShareUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.pull-request-link-updated")]
    ThreadPullRequestLinkUpdated(Box<DomainEventThreadPullRequestLinkUpdated>),
    #[serde(rename = "run.created")]
    RunCreated(Box<DomainEventRunCreated>),
    #[serde(rename = "run.updated")]
    RunUpdated(Box<DomainEventRunCreated>),
    #[serde(rename = "run.background-work-cancelled")]
    RunBackgroundWorkCancelled(Box<DomainEventRunBackgroundWorkCancelled>),
    #[serde(rename = "run-attempt.created")]
    RunAttemptCreated(Box<DomainEventRunAttemptCreated>),
    #[serde(rename = "run-attempt.updated")]
    RunAttemptUpdated(Box<DomainEventRunAttemptCreated>),
    #[serde(rename = "node.updated")]
    NodeUpdated(Box<DomainEventNodeUpdated>),
    #[serde(rename = "subagent.updated")]
    SubagentUpdated(Box<DomainEventSubagentUpdated>),
    #[serde(rename = "provider-session.attached")]
    ProviderSessionAttached(Box<DomainEventProviderSessionAttached>),
    #[serde(rename = "provider-session.updated")]
    ProviderSessionUpdated(Box<DomainEventProviderSessionAttached>),
    #[serde(rename = "provider-session.detached")]
    ProviderSessionDetached(Box<DomainEventProviderSessionDetached>),
    #[serde(rename = "provider-thread.updated")]
    ProviderThreadUpdated(Box<DomainEventProviderThreadUpdated>),
    #[serde(rename = "provider-turn.updated")]
    ProviderTurnUpdated(Box<DomainEventProviderTurnUpdated>),
    #[serde(rename = "runtime-request.updated")]
    RuntimeRequestUpdated(Box<DomainEventRuntimeRequestUpdated>),
    #[serde(rename = "message.updated")]
    MessageUpdated(Box<DomainEventMessageUpdated>),
    #[serde(rename = "turn-item.updated")]
    TurnItemUpdated(Box<DomainEventTurnItemUpdated>),
    #[serde(rename = "plan.updated")]
    PlanUpdated(Box<DomainEventPlanUpdated>),
    #[serde(rename = "checkpoint-scope.created")]
    CheckpointScopeCreated(Box<DomainEventCheckpointScopeCreated>),
    #[serde(rename = "checkpoint.captured")]
    CheckpointCaptured(Box<DomainEventCheckpointCaptured>),
    #[serde(rename = "checkpoint.rollback-requested")]
    CheckpointRollbackRequested(Box<DomainEventCheckpointRollbackRequested>),
    #[serde(rename = "context-handoff.updated")]
    ContextHandoffUpdated(Box<DomainEventContextHandoffUpdated>),
    #[serde(rename = "context-transfer.created")]
    ContextTransferCreated(Box<DomainEventContextTransferCreated>),
    #[serde(rename = "context-transfer.updated")]
    ContextTransferUpdated(Box<DomainEventContextTransferCreated>),
}
impl crate::wire::TaggedUnion for OrchestrationV2DomainEvent {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "thread.created",
        "thread.archived",
        "thread.unarchived",
        "thread.deleted",
        "thread.settled",
        "thread.unsettled",
        "thread.snoozed",
        "thread.unsnoozed",
        "thread.pinned",
        "thread.auto-settle-set",
        "thread.unpinned",
        "thread.pin-reordered",
        "thread.active-reordered",
        "thread.tags-updated",
        "thread.visited",
        "thread.marked-unread",
        "thread.metadata-updated",
        "thread.pull-request-synced",
        "thread.runtime-mode-updated",
        "thread.interaction-mode-updated",
        "thread.model-selection-updated",
        "thread.provider-switched",
        "thread.guest-share-updated",
        "thread.pull-request-link-updated",
        "run.created",
        "run.updated",
        "run.background-work-cancelled",
        "run-attempt.created",
        "run-attempt.updated",
        "node.updated",
        "subagent.updated",
        "provider-session.attached",
        "provider-session.updated",
        "provider-session.detached",
        "provider-thread.updated",
        "provider-turn.updated",
        "runtime-request.updated",
        "message.updated",
        "turn-item.updated",
        "plan.updated",
        "checkpoint-scope.created",
        "checkpoint.captured",
        "checkpoint.rollback-requested",
        "context-handoff.updated",
        "context-transfer.created",
        "context-transfer.updated",
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationV2StoredEvent {
    #[serde(rename = "sequence")]
    pub sequence: crate::base_schemas::NonNegativeInt,
    #[serde(rename = "commandId", with = "crate::wire::nullable")]
    pub command_id: Option<crate::base_schemas::CommandId>,
    #[serde(rename = "event")]
    pub event: OrchestrationV2DomainEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEventJsonTurnItemUpdated {
    #[serde(rename = "id")]
    pub id: crate::base_schemas::EventId,
    #[serde(rename = "threadId")]
    pub thread_id: crate::base_schemas::ThreadId,
    #[serde(
        rename = "runId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub run_id: Option<Option<crate::base_schemas::RunId>>,
    #[serde(
        rename = "nodeId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub node_id: Option<Option<crate::base_schemas::NodeId>>,
    #[serde(
        rename = "driver",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub driver: Option<Option<crate::provider_instance::ProviderDriverKind>>,
    #[serde(
        rename = "providerInstanceId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_instance_id: Option<Option<crate::provider_instance::ProviderInstanceId>>,
    #[serde(
        rename = "rawEventId",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub raw_event_id: Option<Option<crate::base_schemas::RawEventId>>,
    #[serde(rename = "occurredAt")]
    pub occurred_at: crate::wire::WireDateTime,
    #[serde(rename = "payload")]
    pub payload: OrchestrationV2TurnItemJson,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationV2DomainEventJson {
    #[serde(rename = "thread.created")]
    ThreadCreated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.archived")]
    ThreadArchived(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.unarchived")]
    ThreadUnarchived(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.deleted")]
    ThreadDeleted(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.settled")]
    ThreadSettled(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.unsettled")]
    ThreadUnsettled(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.snoozed")]
    ThreadSnoozed(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.unsnoozed")]
    ThreadUnsnoozed(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.pinned")]
    ThreadPinned(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.auto-settle-set")]
    ThreadAutoSettleSet(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.unpinned")]
    ThreadUnpinned(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.pin-reordered")]
    ThreadPinReordered(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.active-reordered")]
    ThreadActiveReordered(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.tags-updated")]
    ThreadTagsUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.visited")]
    ThreadVisited(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.marked-unread")]
    ThreadMarkedUnread(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.pull-request-synced")]
    ThreadPullRequestSynced(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.metadata-updated")]
    ThreadMetadataUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.runtime-mode-updated")]
    ThreadRuntimeModeUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.interaction-mode-updated")]
    ThreadInteractionModeUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.model-selection-updated")]
    ThreadModelSelectionUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.provider-switched")]
    ThreadProviderSwitched(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.guest-share-updated")]
    ThreadGuestShareUpdated(Box<DomainEventThreadCreated>),
    #[serde(rename = "thread.pull-request-link-updated")]
    ThreadPullRequestLinkUpdated(Box<DomainEventThreadPullRequestLinkUpdated>),
    #[serde(rename = "run.created")]
    RunCreated(Box<DomainEventRunCreated>),
    #[serde(rename = "run.updated")]
    RunUpdated(Box<DomainEventRunCreated>),
    #[serde(rename = "run.background-work-cancelled")]
    RunBackgroundWorkCancelled(Box<DomainEventRunBackgroundWorkCancelled>),
    #[serde(rename = "run-attempt.created")]
    RunAttemptCreated(Box<DomainEventRunAttemptCreated>),
    #[serde(rename = "run-attempt.updated")]
    RunAttemptUpdated(Box<DomainEventRunAttemptCreated>),
    #[serde(rename = "node.updated")]
    NodeUpdated(Box<DomainEventNodeUpdated>),
    #[serde(rename = "subagent.updated")]
    SubagentUpdated(Box<DomainEventSubagentUpdated>),
    #[serde(rename = "provider-session.attached")]
    ProviderSessionAttached(Box<DomainEventProviderSessionAttached>),
    #[serde(rename = "provider-session.updated")]
    ProviderSessionUpdated(Box<DomainEventProviderSessionAttached>),
    #[serde(rename = "provider-session.detached")]
    ProviderSessionDetached(Box<DomainEventProviderSessionDetached>),
    #[serde(rename = "provider-thread.updated")]
    ProviderThreadUpdated(Box<DomainEventProviderThreadUpdated>),
    #[serde(rename = "provider-turn.updated")]
    ProviderTurnUpdated(Box<DomainEventProviderTurnUpdated>),
    #[serde(rename = "runtime-request.updated")]
    RuntimeRequestUpdated(Box<DomainEventRuntimeRequestUpdated>),
    #[serde(rename = "message.updated")]
    MessageUpdated(Box<DomainEventMessageUpdated>),
    #[serde(rename = "turn-item.updated")]
    TurnItemUpdated(Box<DomainEventJsonTurnItemUpdated>),
    #[serde(rename = "plan.updated")]
    PlanUpdated(Box<DomainEventPlanUpdated>),
    #[serde(rename = "checkpoint-scope.created")]
    CheckpointScopeCreated(Box<DomainEventCheckpointScopeCreated>),
    #[serde(rename = "checkpoint.captured")]
    CheckpointCaptured(Box<DomainEventCheckpointCaptured>),
    #[serde(rename = "checkpoint.rollback-requested")]
    CheckpointRollbackRequested(Box<DomainEventCheckpointRollbackRequested>),
    #[serde(rename = "context-handoff.updated")]
    ContextHandoffUpdated(Box<DomainEventContextHandoffUpdated>),
    #[serde(rename = "context-transfer.created")]
    ContextTransferCreated(Box<DomainEventContextTransferCreated>),
    #[serde(rename = "context-transfer.updated")]
    ContextTransferUpdated(Box<DomainEventContextTransferCreated>),
}
impl crate::wire::TaggedUnion for OrchestrationV2DomainEventJson {
    const TAG: &'static str = "type";
    const KNOWN_TAGS: &'static [&'static str] = &[
        "thread.created",
        "thread.archived",
        "thread.unarchived",
        "thread.deleted",
        "thread.settled",
        "thread.unsettled",
        "thread.snoozed",
        "thread.unsnoozed",
        "thread.pinned",
        "thread.auto-settle-set",
        "thread.unpinned",
        "thread.pin-reordered",
        "thread.active-reordered",
        "thread.tags-updated",
        "thread.visited",
        "thread.marked-unread",
        "thread.pull-request-synced",
        "thread.metadata-updated",
        "thread.runtime-mode-updated",
        "thread.interaction-mode-updated",
        "thread.model-selection-updated",
        "thread.provider-switched",
        "thread.guest-share-updated",
        "thread.pull-request-link-updated",
        "run.created",
        "run.updated",
        "run.background-work-cancelled",
        "run-attempt.created",
        "run-attempt.updated",
        "node.updated",
        "subagent.updated",
        "provider-session.attached",
        "provider-session.updated",
        "provider-session.detached",
        "provider-thread.updated",
        "provider-turn.updated",
        "runtime-request.updated",
        "message.updated",
        "turn-item.updated",
        "plan.updated",
        "checkpoint-scope.created",
        "checkpoint.captured",
        "checkpoint.rollback-requested",
        "context-handoff.updated",
        "context-transfer.created",
        "context-transfer.updated",
    ];
}
