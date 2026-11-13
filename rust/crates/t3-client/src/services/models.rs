use crate::types::EnvironmentId;
use std::rc::Rc;
use t3_contracts::{
    base_schemas::{MessageId, RunId},
    orchestration_project::OrchestrationProjectShell,
    orchestration_v2::*,
    provider_instance::ProviderInstanceId,
    wire::WireDateTime,
};

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentProject {
    pub environment: EnvironmentId,
    pub source: Rc<OrchestrationProjectShell>,
    pub is_scratch: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentThread {
    pub environment: EnvironmentId,
    pub projection: Rc<OrchestrationV2ThreadProjection>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRunSummary {
    pub run_id: RunId,
    pub status: OrchestrationV2RunStatus,
    pub requested_at: Option<WireDateTime>,
    pub started_at: Option<WireDateTime>,
    pub completed_at: Option<WireDateTime>,
    pub assistant_message_id: Option<MessageId>,
    pub turn_ended: Option<bool>,
    pub provider_starting_label: Option<String>,
    pub source_plan_ref: Option<(
        t3_contracts::base_schemas::ThreadId,
        t3_contracts::base_schemas::PlanId,
    )>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRuntimeSummary {
    pub status: OrchestrationV2ShellThreadStatus,
    pub active_run_id: Option<RunId>,
    pub activity_started_at: Option<Option<WireDateTime>>,
    pub provider_instance_id: ProviderInstanceId,
    pub provider_name: Option<String>,
    pub last_error: Option<String>,
    pub last_error_class: Option<OrchestrationV2ProviderFailureClass>,
    pub last_error_code: Option<String>,
    pub usage_limit_reset_at: Option<t3_contracts::base_schemas::IsoDateTime>,
    pub updated_at: WireDateTime,
}
pub struct ThreadStructureRuntime<'a> {
    pub status: &'a OrchestrationV2ShellThreadStatus,
    pub active_run_id: &'a Option<RunId>,
    pub activity_started_at: &'a Option<Option<WireDateTime>>,
    pub provider_instance_id: &'a ProviderInstanceId,
    pub provider_name: &'a Option<String>,
    pub last_error: &'a Option<String>,
    pub last_error_class: &'a Option<OrchestrationV2ProviderFailureClass>,
    pub last_error_code: &'a Option<String>,
    pub usage_limit_reset_at: &'a Option<t3_contracts::base_schemas::IsoDateTime>,
}
impl ThreadRuntimeSummary {
    pub fn structure(&self) -> ThreadStructureRuntime<'_> {
        ThreadStructureRuntime {
            status: &self.status,
            active_run_id: &self.active_run_id,
            activity_started_at: &self.activity_started_at,
            provider_instance_id: &self.provider_instance_id,
            provider_name: &self.provider_name,
            last_error: &self.last_error,
            last_error_class: &self.last_error_class,
            last_error_code: &self.last_error_code,
            usage_limit_reset_at: &self.usage_limit_reset_at,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingSubagentRequest {
    Approval,
    UserInput,
}
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentThreadShell {
    pub environment: EnvironmentId,
    pub source: Rc<OrchestrationV2ThreadShell>,
    pub title: String,
    pub latest_run: Option<ThreadRunSummary>,
    pub runtime: Option<ThreadRuntimeSummary>,
    pub has_pending_approvals: bool,
    pub has_pending_user_input: bool,
    pub pending_user_input_at: Option<WireDateTime>,
    pub pending_subagent_request: Option<PendingSubagentRequest>,
    pub pull_requests: Vec<t3_contracts::thread_pull_request::ThreadPullRequestLink>,
}
pub fn thread_runtime_is_active(runtime: Option<&ThreadRuntimeSummary>) -> bool {
    runtime.is_some_and(|runtime| {
        matches!(
            runtime.status,
            OrchestrationV2ShellThreadStatus::Preparing
                | OrchestrationV2ShellThreadStatus::Queued
                | OrchestrationV2ShellThreadStatus::Starting
                | OrchestrationV2ShellThreadStatus::Running
                | OrchestrationV2ShellThreadStatus::Waiting
        )
    })
}
pub fn thread_runtime_can_archive(runtime: Option<&ThreadRuntimeSummary>) -> bool {
    runtime.is_none_or(|runtime| match runtime.status {
        OrchestrationV2ShellThreadStatus::Queued => runtime.active_run_id.is_none(),
        OrchestrationV2ShellThreadStatus::Preparing
        | OrchestrationV2ShellThreadStatus::Starting
        | OrchestrationV2ShellThreadStatus::Running => false,
        _ => true,
    })
}
pub fn scope_project(
    environment: EnvironmentId,
    source: Rc<OrchestrationProjectShell>,
) -> EnvironmentProject {
    EnvironmentProject {
        environment,
        source,
        is_scratch: false,
    }
}
pub fn present_thread_shell(
    environment: EnvironmentId,
    source: Rc<OrchestrationV2ThreadShell>,
    pending_subagent_request: Option<PendingSubagentRequest>,
) -> EnvironmentThreadShell {
    use OrchestrationV2ShellThreadStatus as Status;
    let terminal = matches!(
        source.status,
        Status::Idle
            | Status::Completed
            | Status::Interrupted
            | Status::Failed
            | Status::Cancelled
            | Status::RolledBack
    );
    let latest_run = source.latest_run_id.as_ref().map(|id| ThreadRunSummary {
        run_id: id.clone(),
        status: match source.status {
            Status::Idle | Status::Completed => OrchestrationV2RunStatus::Completed,
            Status::Preparing => OrchestrationV2RunStatus::Preparing,
            Status::Queued => OrchestrationV2RunStatus::Queued,
            Status::Starting => OrchestrationV2RunStatus::Starting,
            Status::Running => OrchestrationV2RunStatus::Running,
            Status::Waiting => OrchestrationV2RunStatus::Waiting,
            Status::Interrupted => OrchestrationV2RunStatus::Interrupted,
            Status::Failed => OrchestrationV2RunStatus::Failed,
            Status::Cancelled => OrchestrationV2RunStatus::Cancelled,
            Status::RolledBack => OrchestrationV2RunStatus::RolledBack,
        },
        requested_at: source.latest_run_requested_at.clone().flatten(),
        started_at: source.latest_run_started_at.clone().flatten(),
        completed_at: source
            .latest_run_completed_at
            .clone()
            .unwrap_or_else(|| terminal.then(|| source.updated_at.clone())),
        assistant_message_id: None,
        turn_ended: None,
        provider_starting_label: None,
        source_plan_ref: None,
    });
    let parked = source.status != Status::Failed
        && crate::thread_views::threads::pending_work::background_work_holds_completion(
            source
                .pending_background_tasks
                .as_ref()
                .and_then(Option::as_deref)
                .unwrap_or(&[]),
        );
    let runtime = (source.latest_run_id.is_some() || parked).then(|| ThreadRuntimeSummary {
        status: if parked {
            Status::Idle
        } else {
            match source.activity_run_status.as_ref().and_then(Option::as_ref) {
                Some(ThreadShellActivityRunStatus::Preparing) => Status::Preparing,
                Some(ThreadShellActivityRunStatus::Starting) => Status::Starting,
                Some(ThreadShellActivityRunStatus::Running) => Status::Running,
                Some(ThreadShellActivityRunStatus::Waiting) => Status::Waiting,
                None => source.status.clone(),
            }
        },
        active_run_id: source.active_run_id.clone(),
        activity_started_at: source.activity_run_started_at.clone(),
        provider_instance_id: source.provider_instance_id.clone(),
        provider_name: None,
        last_error: source.last_error.clone().flatten(),
        last_error_class: source.last_error_class.clone().flatten(),
        last_error_code: source.last_error_code.clone().flatten(),
        usage_limit_reset_at: source.usage_limit_reset_at.clone().flatten(),
        updated_at: source.updated_at.clone(),
    });
    let request = source.pending_runtime_request.as_ref();
    let has_pending_user_input = request.is_some_and(|request| matches!(&request.kind, RuntimeRequestKind::Member2(kind) if **kind == RuntimeRequestKindMember2::UserInput));
    EnvironmentThreadShell {
        environment,
        title: if source.lineage.relationship_to_parent == Some(AppThreadLineageRelationshipToParent::Subagent) { crate::thread_views::threads::subagent_display::format_subagent_display_title(&source.title) } else { source.title.clone() },
        latest_run, runtime, has_pending_user_input,
        has_pending_approvals: request.is_some_and(|request| !matches!(&request.kind, RuntimeRequestKind::Member2(kind) if matches!(**kind, RuntimeRequestKindMember2::UserInput | RuntimeRequestKindMember2::AuthRefresh))),
        pending_user_input_at: request.filter(|_| has_pending_user_input).map(|request| request.created_at.clone()),
        pending_subagent_request,
        pull_requests: source.pull_requests.clone().flatten().unwrap_or_default(),
        source,
    }
}
pub fn resolve_thread_provider_stack(thread: &EnvironmentThreadShell) -> Vec<ProviderInstanceId> {
    let current = thread
        .runtime
        .as_ref()
        .map_or(&thread.source.model_selection.instance_id, |runtime| {
            &runtime.provider_instance_id
        });
    let previous: Vec<_> = thread
        .source
        .provider_instance_history
        .as_ref()
        .and_then(Option::as_deref)
        .unwrap_or(&[])
        .iter()
        .filter(|id| *id != current)
        .collect();
    previous
        .iter()
        .skip(previous.len().saturating_sub(2))
        .map(|id| (*id).clone())
        .chain(std::iter::once(current.clone()))
        .collect()
}
pub fn resolve_thread_working_started_at<'a>(
    run: Option<&'a ThreadRunSummary>,
    runtime: Option<&'a ThreadRuntimeSummary>,
) -> Option<&'a str> {
    let valid = |value: &'a WireDateTime| crate::time::epoch_ms(&value.0).map(|_| value.0.as_str());
    if let Some(activity) = runtime.and_then(|runtime| runtime.activity_started_at.as_ref()) {
        return activity.as_ref().and_then(valid);
    }
    let run = run.filter(|run| {
        run.completed_at.is_none()
            && Some(&run.run_id) == runtime.and_then(|runtime| runtime.active_run_id.as_ref())
    })?;
    run.started_at
        .as_ref()
        .and_then(valid)
        .or_else(|| run.requested_at.as_ref().and_then(valid))
}
pub fn format_working_duration_label(elapsed_ms: f64) -> String {
    let seconds = if elapsed_ms.is_finite() {
        (elapsed_ms / 1000.0).floor().max(0.0) as u64
    } else {
        0
    };
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds / 60 % 60)
    }
}

#[derive(Clone)]
pub struct EnvironmentThreadStructure {
    pub environment: EnvironmentId,
    projection: Rc<crate::state::sharing::SharedThreadProjection>,
}
impl EnvironmentThreadStructure {
    pub fn new(
        environment: EnvironmentId,
        projection: Rc<crate::state::sharing::SharedThreadProjection>,
    ) -> Self {
        Self {
            environment,
            projection,
        }
    }
    pub fn projection(&self) -> ThreadStructure<'_> {
        ThreadStructure(&self.projection)
    }
}
pub struct ThreadStructure<'a>(&'a crate::state::sharing::SharedThreadProjection);
pub struct ThreadStructureThread<'a>(&'a OrchestrationV2AppThread);
pub struct AssistantMessageStructure<'a>(&'a TurnItemAssistantMessage);
pub struct ReasoningStructure<'a>(&'a TurnItemReasoning);
pub enum ThreadStructureTurnItem<'a> {
    Stable(&'a OrchestrationV2TurnItem),
    AssistantMessage(AssistantMessageStructure<'a>),
    Reasoning(ReasoningStructure<'a>),
}
impl<'a> From<&'a OrchestrationV2TurnItem> for ThreadStructureTurnItem<'a> {
    fn from(item: &'a OrchestrationV2TurnItem) -> Self {
        match item {
            OrchestrationV2TurnItem::AssistantMessage(item) => {
                Self::AssistantMessage(AssistantMessageStructure(item))
            }
            OrchestrationV2TurnItem::Reasoning(item) => Self::Reasoning(ReasoningStructure(item)),
            item => Self::Stable(item),
        }
    }
}
pub struct ThreadStructureVisibleTurnItem<'a> {
    pub position: &'a t3_contracts::base_schemas::NonNegativeInt,
    pub visibility: &'a ProjectedTurnItemVisibility,
    pub source_thread_id: &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<
        t3_contracts::base_schemas::ThreadId,
    >,
    pub source_item_id: &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<
        t3_contracts::base_schemas::TurnItemId,
    >,
    pub item: ThreadStructureTurnItem<'a>,
}
impl<'a> ThreadStructure<'a> {
    pub fn thread(&self) -> ThreadStructureThread<'a> {
        ThreadStructureThread(&self.0.thread)
    }
    pub fn turn_items(&self) -> impl Iterator<Item = ThreadStructureTurnItem<'a>> {
        self.0
            .turn_items
            .iter()
            .map(|item| ThreadStructureTurnItem::from(item.as_ref()))
    }
    pub fn visible_turn_items(&self) -> impl Iterator<Item = ThreadStructureVisibleTurnItem<'a>> {
        self.0
            .visible_turn_items
            .iter()
            .map(|row| ThreadStructureVisibleTurnItem {
                position: &row.position,
                visibility: &row.visibility,
                source_thread_id: &row.source_thread_id,
                source_item_id: &row.source_item_id,
                item: ThreadStructureTurnItem::from(&row.item),
            })
    }
    pub fn runs(&self) -> &'a crate::state::sharing::SharedRows<OrchestrationV2Run> {
        &self.0.runs
    }
    pub fn attempts(&self) -> &'a crate::state::sharing::SharedRows<OrchestrationV2RunAttempt> {
        &self.0.attempts
    }
    pub fn nodes(&self) -> &'a crate::state::sharing::SharedRows<OrchestrationV2ExecutionNode> {
        &self.0.nodes
    }
    pub fn subagents(&self) -> &'a crate::state::sharing::SharedRows<OrchestrationV2Subagent> {
        &self.0.subagents
    }
    pub fn provider_sessions(
        &self,
    ) -> &'a crate::state::sharing::SharedRows<OrchestrationV2ProviderSession> {
        &self.0.provider_sessions
    }
    pub fn provider_threads(
        &self,
    ) -> &'a crate::state::sharing::SharedRows<OrchestrationV2ProviderThread> {
        &self.0.provider_threads
    }
    pub fn provider_turns(
        &self,
    ) -> &'a crate::state::sharing::SharedRows<OrchestrationV2ProviderTurn> {
        &self.0.provider_turns
    }
    pub fn runtime_requests(
        &self,
    ) -> &'a crate::state::sharing::SharedRows<OrchestrationV2RuntimeRequest> {
        &self.0.runtime_requests
    }
    pub fn messages(
        &self,
    ) -> &'a crate::state::sharing::SharedRows<OrchestrationV2ConversationMessage> {
        &self.0.messages
    }
    pub fn plans(&self) -> &'a crate::state::sharing::SharedRows<OrchestrationV2PlanArtifact> {
        &self.0.plans
    }
    pub fn checkpoint_scopes(
        &self,
    ) -> &'a crate::state::sharing::SharedRows<OrchestrationV2CheckpointScope> {
        &self.0.checkpoint_scopes
    }
    pub fn checkpoints(&self) -> &'a crate::state::sharing::SharedRows<OrchestrationV2Checkpoint> {
        &self.0.checkpoints
    }
    pub fn context_handoffs(
        &self,
    ) -> &'a crate::state::sharing::SharedRows<OrchestrationV2ContextHandoff> {
        &self.0.context_handoffs
    }
    pub fn context_transfers(
        &self,
    ) -> &'a crate::state::sharing::SharedRows<OrchestrationV2ContextTransfer> {
        &self.0.context_transfers
    }
}
impl<'a> ThreadStructureThread<'a> {
    pub fn created_by(&self) -> &'a OrchestrationV2Actor {
        &self.0.created_by
    }
    pub fn creation_source(&self) -> &'a OrchestrationV2CreationSource {
        &self.0.creation_source
    }
    pub fn id(
        &self,
    ) -> &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::ThreadId>
    {
        &self.0.id
    }
    pub fn project_id(
        &self,
    ) -> &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::ProjectId>
    {
        &self.0.project_id
    }
    pub fn title(&self) -> &'a t3_contracts::base_schemas::TrimmedNonEmptyString {
        &self.0.title
    }
    pub fn provider_instance_id(
        &self,
    ) -> &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<
        t3_contracts::provider_instance::ProviderInstanceId,
    > {
        &self.0.provider_instance_id
    }
    pub fn model_selection(&self) -> &'a t3_contracts::model_selection::ModelSelection {
        &self.0.model_selection
    }
    pub fn runtime_mode(&self) -> &'a t3_contracts::provider_policy::RuntimeMode {
        &self.0.runtime_mode
    }
    pub fn interaction_mode(&self) -> &'a t3_contracts::provider_policy::ProviderInteractionMode {
        &self.0.interaction_mode
    }
    pub fn branch(&self) -> &'a Option<t3_contracts::base_schemas::TrimmedNonEmptyString> {
        &self.0.branch
    }
    pub fn worktree_path(&self) -> &'a Option<t3_contracts::base_schemas::TrimmedNonEmptyString> {
        &self.0.worktree_path
    }
    pub fn linked_pull_request(
        &self,
    ) -> &'a Option<Option<t3_contracts::thread_pull_request::ThreadLinkedPullRequest>> {
        &self.0.linked_pull_request
    }
    pub fn pull_requests(
        &self,
    ) -> &'a Option<Option<Vec<t3_contracts::thread_pull_request::ThreadPullRequestLink>>> {
        &self.0.pull_requests
    }
    pub fn branch_pull_request(
        &self,
    ) -> &'a Option<Option<t3_contracts::thread_pull_request::ThreadLinkedPullRequest>> {
        &self.0.branch_pull_request
    }
    pub fn active_provider_thread_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<
            t3_contracts::base_schemas::ProviderThreadId,
        >,
    > {
        &self.0.active_provider_thread_id
    }
    pub fn history_origin(&self) -> &'a Option<Option<OrchestrationV2ThreadHistoryOrigin>> {
        &self.0.history_origin
    }
    pub fn link_origin(&self) -> &'a Option<Option<OrchestrationV2LinkOrigin>> {
        &self.0.link_origin
    }
    pub fn delegated_from(&self) -> &'a Option<Option<OrchestrationV2DelegatedFrom>> {
        &self.0.delegated_from
    }
    pub fn lineage(&self) -> &'a OrchestrationV2AppThreadLineage {
        &self.0.lineage
    }
    pub fn forked_from(&self) -> &'a Option<AppThreadForkedFrom> {
        &self.0.forked_from
    }
    pub fn created_at(&self) -> &'a t3_contracts::wire::WireDateTime {
        &self.0.created_at
    }
    pub fn archived_at(&self) -> &'a Option<t3_contracts::wire::WireDateTime> {
        &self.0.archived_at
    }
    pub fn settled_override(&self) -> &'a Option<AppThreadSettledOverride> {
        &self.0.settled_override
    }
    pub fn settled_at(&self) -> &'a Option<t3_contracts::wire::WireDateTime> {
        &self.0.settled_at
    }
    pub fn unsettled_at(&self) -> &'a Option<Option<t3_contracts::wire::WireDateTime>> {
        &self.0.unsettled_at
    }
    pub fn snoozed_until(&self) -> &'a Option<Option<t3_contracts::wire::WireDateTime>> {
        &self.0.snoozed_until
    }
    pub fn snoozed_at(&self) -> &'a Option<Option<t3_contracts::wire::WireDateTime>> {
        &self.0.snoozed_at
    }
    pub fn last_snooze_wake_at(&self) -> &'a Option<Option<t3_contracts::wire::WireDateTime>> {
        &self.0.last_snooze_wake_at
    }
    pub fn limit_recovery(&self) -> &'a Option<Option<OrchestrationV2LimitRecovery>> {
        &self.0.limit_recovery
    }
    pub fn guest_share(&self) -> &'a Option<Option<t3_contracts::guest_share::ThreadGuestShare>> {
        &self.0.guest_share
    }
    pub fn pinned_at(&self) -> &'a Option<Option<t3_contracts::wire::WireDateTime>> {
        &self.0.pinned_at
    }
    pub fn auto_settle_disabled_at(&self) -> &'a Option<Option<t3_contracts::wire::WireDateTime>> {
        &self.0.auto_settle_disabled_at
    }
    pub fn pin_order_key(
        &self,
    ) -> &'a Option<Option<t3_contracts::base_schemas::TrimmedNonEmptyString>> {
        &self.0.pin_order_key
    }
    pub fn active_order_key(
        &self,
    ) -> &'a Option<Option<t3_contracts::base_schemas::TrimmedNonEmptyString>> {
        &self.0.active_order_key
    }
    pub fn tags(&self) -> &'a Option<Option<t3_contracts::thread_tags::StoredThreadTags>> {
        &self.0.tags
    }
    pub fn opens(&self) -> &'a Option<Option<OrchestrationV2ThreadOpens>> {
        &self.0.opens
    }
    pub fn title_regeneration(&self) -> &'a Option<Option<AppThreadTitleRegeneration>> {
        &self.0.title_regeneration
    }
    pub fn last_title_regeneration(
        &self,
    ) -> &'a Option<Option<OrchestrationV2LastTitleRegeneration>> {
        &self.0.last_title_regeneration
    }
    pub fn rollback_request_id(
        &self,
    ) -> &'a Option<
        Option<
            t3_contracts::orchestration_v2_id_table::IdOrIndex<
                t3_contracts::base_schemas::CommandId,
            >,
        >,
    > {
        &self.0.rollback_request_id
    }
    pub fn rollback_failure(&self) -> &'a Option<Option<AppThreadRollbackFailure>> {
        &self.0.rollback_failure
    }
    pub fn deleted_at(&self) -> &'a Option<t3_contracts::wire::WireDateTime> {
        &self.0.deleted_at
    }
}
impl<'a> AssistantMessageStructure<'a> {
    pub fn deferred_work(&self) -> &'a Option<OrchestrationV2DeferredWork> {
        &self.0.deferred_work
    }
    pub fn tool_non_execution_kind(&self) -> &'a Option<Option<String>> {
        &self.0.tool_non_execution_kind
    }
    pub fn tool_surface(
        &self,
    ) -> &'a Option<Option<t3_contracts::provider_runtime::ToolActivitySurface>> {
        &self.0.tool_surface
    }
    pub fn tool_icon(
        &self,
    ) -> &'a Option<Option<t3_contracts::provider_runtime::ToolActivityIcon>> {
        &self.0.tool_icon
    }
    pub fn tool_source(
        &self,
    ) -> &'a Option<Option<t3_contracts::provider_runtime::ToolActivitySource>> {
        &self.0.tool_source
    }
    pub fn id(
        &self,
    ) -> &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<
        t3_contracts::base_schemas::TurnItemId,
    > {
        &self.0.id
    }
    pub fn thread_id(
        &self,
    ) -> &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::ThreadId>
    {
        &self.0.thread_id
    }
    pub fn run_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::RunId>,
    > {
        &self.0.run_id
    }
    pub fn node_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::NodeId>,
    > {
        &self.0.node_id
    }
    pub fn provider_thread_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<
            t3_contracts::base_schemas::ProviderThreadId,
        >,
    > {
        &self.0.provider_thread_id
    }
    pub fn provider_turn_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<
            t3_contracts::base_schemas::ProviderTurnId,
        >,
    > {
        &self.0.provider_turn_id
    }
    pub fn native_item_ref(&self) -> &'a Option<OrchestrationV2ProviderRef> {
        &self.0.native_item_ref
    }
    pub fn parent_item_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::TurnItemId>,
    > {
        &self.0.parent_item_id
    }
    pub fn ordinal(&self) -> &'a t3_contracts::base_schemas::NonNegativeInt {
        &self.0.ordinal
    }
    pub fn status(&self) -> &'a OrchestrationV2TurnItemStatus {
        &self.0.status
    }
    pub fn title(&self) -> &'a Option<String> {
        &self.0.title
    }
    pub fn started_at(&self) -> &'a Option<t3_contracts::wire::WireDateTime> {
        &self.0.started_at
    }
    pub fn completed_at(&self) -> &'a Option<t3_contracts::wire::WireDateTime> {
        &self.0.completed_at
    }
    pub fn phase(&self) -> &'a Option<Option<AssistantMessagePhase>> {
        &self.0.phase
    }
    pub fn message_id(
        &self,
    ) -> &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::MessageId>
    {
        &self.0.message_id
    }
    pub fn text_omitted(&self) -> &'a Option<LiteralTrue> {
        &self.0.text_omitted
    }
    pub fn attachments(
        &self,
    ) -> &'a Option<Option<Vec<t3_contracts::chat_attachment::ChatAttachment>>> {
        &self.0.attachments
    }
    pub fn streaming(&self) -> &'a bool {
        &self.0.streaming
    }
}
impl<'a> ReasoningStructure<'a> {
    pub fn deferred_work(&self) -> &'a Option<OrchestrationV2DeferredWork> {
        &self.0.deferred_work
    }
    pub fn tool_non_execution_kind(&self) -> &'a Option<Option<String>> {
        &self.0.tool_non_execution_kind
    }
    pub fn tool_surface(
        &self,
    ) -> &'a Option<Option<t3_contracts::provider_runtime::ToolActivitySurface>> {
        &self.0.tool_surface
    }
    pub fn tool_icon(
        &self,
    ) -> &'a Option<Option<t3_contracts::provider_runtime::ToolActivityIcon>> {
        &self.0.tool_icon
    }
    pub fn tool_source(
        &self,
    ) -> &'a Option<Option<t3_contracts::provider_runtime::ToolActivitySource>> {
        &self.0.tool_source
    }
    pub fn id(
        &self,
    ) -> &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<
        t3_contracts::base_schemas::TurnItemId,
    > {
        &self.0.id
    }
    pub fn thread_id(
        &self,
    ) -> &'a t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::ThreadId>
    {
        &self.0.thread_id
    }
    pub fn run_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::RunId>,
    > {
        &self.0.run_id
    }
    pub fn node_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::NodeId>,
    > {
        &self.0.node_id
    }
    pub fn provider_thread_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<
            t3_contracts::base_schemas::ProviderThreadId,
        >,
    > {
        &self.0.provider_thread_id
    }
    pub fn provider_turn_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<
            t3_contracts::base_schemas::ProviderTurnId,
        >,
    > {
        &self.0.provider_turn_id
    }
    pub fn native_item_ref(&self) -> &'a Option<OrchestrationV2ProviderRef> {
        &self.0.native_item_ref
    }
    pub fn parent_item_id(
        &self,
    ) -> &'a Option<
        t3_contracts::orchestration_v2_id_table::IdOrIndex<t3_contracts::base_schemas::TurnItemId>,
    > {
        &self.0.parent_item_id
    }
    pub fn ordinal(&self) -> &'a t3_contracts::base_schemas::NonNegativeInt {
        &self.0.ordinal
    }
    pub fn status(&self) -> &'a OrchestrationV2TurnItemStatus {
        &self.0.status
    }
    pub fn title(&self) -> &'a Option<String> {
        &self.0.title
    }
    pub fn started_at(&self) -> &'a Option<t3_contracts::wire::WireDateTime> {
        &self.0.started_at
    }
    pub fn completed_at(&self) -> &'a Option<t3_contracts::wire::WireDateTime> {
        &self.0.completed_at
    }
    pub fn streaming(&self) -> &'a bool {
        &self.0.streaming
    }
}
