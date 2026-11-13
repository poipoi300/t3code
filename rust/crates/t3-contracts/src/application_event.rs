#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationClientOrigin {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub surface: Option<Option<crate::base_schemas::ClientSurface>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub app_version: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationEventMetadata {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub deferred_turn: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_turn_id: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub provider_item_id: Option<Option<crate::base_schemas::ProviderItemId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub adapter_key: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub request_id: Option<Option<crate::base_schemas::ApprovalRequestId>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub ingested_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub origin: Option<Option<OrchestrationClientOrigin>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationProjectCreatedPayload {
    pub project_id: crate::base_schemas::ProjectId,
    pub title: crate::base_schemas::TrimmedNonEmptyString,
    pub workspace_root: crate::base_schemas::TrimmedNonEmptyString,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub repository_identity: Option<Option<crate::environment::RepositoryIdentity>>,
    #[serde(with = "crate::wire::nullable")]
    pub default_model_selection: Option<crate::model_selection::ModelSelection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_thread_env_mode: Option<Option<crate::environment::ThreadEnvMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub favicon_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub project_icon: Option<Option<crate::project::StoredProjectIcon>>,
    pub scripts: Vec<crate::project::ProjectScript>,
    pub created_at: crate::base_schemas::IsoDateTime,
    pub updated_at: crate::base_schemas::IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationProjectMetaUpdatedPayload {
    pub project_id: crate::base_schemas::ProjectId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub title: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub workspace_root: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub repository_identity: Option<Option<crate::environment::RepositoryIdentity>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_model_selection: Option<Option<crate::model_selection::ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub default_thread_env_mode: Option<Option<crate::environment::ThreadEnvMode>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub favicon_path: Option<Option<crate::base_schemas::TrimmedNonEmptyString>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub project_icon: Option<Option<crate::project::StoredProjectIcon>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub scripts: Option<Option<Vec<crate::project::ProjectScript>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub auto_pull: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub pinned_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::nullable_optional"
    )]
    pub archived_at: Option<Option<crate::base_schemas::IsoDateTime>>,
    pub updated_at: crate::base_schemas::IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationProjectDeletedPayload {
    pub project_id: crate::base_schemas::ProjectId,
    pub deleted_at: crate::base_schemas::IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProjectAggregateKind {
    #[serde(rename = "project")]
    Project,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ApplicationProjectCreatedEventType {
    #[serde(rename = "project.created")]
    Created,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationProjectCreatedEvent {
    pub sequence: u64,
    pub event_id: crate::base_schemas::EventId,
    pub aggregate_kind: ProjectAggregateKind,
    pub aggregate_id: crate::base_schemas::ProjectId,
    pub occurred_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub command_id: Option<crate::base_schemas::CommandId>,
    #[serde(with = "crate::wire::nullable")]
    pub causation_event_id: Option<crate::base_schemas::EventId>,
    #[serde(with = "crate::wire::nullable")]
    pub correlation_id: Option<crate::base_schemas::CommandId>,
    pub metadata: ApplicationEventMetadata,
    pub r#type: ApplicationProjectCreatedEventType,
    pub payload: ApplicationProjectCreatedPayload,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ApplicationProjectMetaUpdatedEventType {
    #[serde(rename = "project.meta-updated")]
    MetaUpdated,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationProjectMetaUpdatedEvent {
    pub sequence: u64,
    pub event_id: crate::base_schemas::EventId,
    pub aggregate_kind: ProjectAggregateKind,
    pub aggregate_id: crate::base_schemas::ProjectId,
    pub occurred_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub command_id: Option<crate::base_schemas::CommandId>,
    #[serde(with = "crate::wire::nullable")]
    pub causation_event_id: Option<crate::base_schemas::EventId>,
    #[serde(with = "crate::wire::nullable")]
    pub correlation_id: Option<crate::base_schemas::CommandId>,
    pub metadata: ApplicationEventMetadata,
    pub r#type: ApplicationProjectMetaUpdatedEventType,
    pub payload: ApplicationProjectMetaUpdatedPayload,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ApplicationProjectDeletedEventType {
    #[serde(rename = "project.deleted")]
    Deleted,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationProjectDeletedEvent {
    pub sequence: u64,
    pub event_id: crate::base_schemas::EventId,
    pub aggregate_kind: ProjectAggregateKind,
    pub aggregate_id: crate::base_schemas::ProjectId,
    pub occurred_at: crate::base_schemas::IsoDateTime,
    #[serde(with = "crate::wire::nullable")]
    pub command_id: Option<crate::base_schemas::CommandId>,
    #[serde(with = "crate::wire::nullable")]
    pub causation_event_id: Option<crate::base_schemas::EventId>,
    #[serde(with = "crate::wire::nullable")]
    pub correlation_id: Option<crate::base_schemas::CommandId>,
    pub metadata: ApplicationEventMetadata,
    pub r#type: ApplicationProjectDeletedEventType,
    pub payload: ApplicationProjectDeletedPayload,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ApplicationProjectEvent {
    Created(Box<ApplicationProjectCreatedEvent>),
    MetaUpdated(Box<ApplicationProjectMetaUpdatedEvent>),
    Deleted(Box<ApplicationProjectDeletedEvent>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ApplicationStoredEvent {
    Project(Box<ApplicationProjectEvent>),
    Orchestration(Box<crate::orchestration_v2::OrchestrationV2StoredEvent>),
}
