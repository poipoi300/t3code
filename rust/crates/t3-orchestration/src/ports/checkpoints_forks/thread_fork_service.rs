use super::checkpoint_restore_safety::expanded_id;
use crate::{
    api::*,
    error::{Error, Result},
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;

pub struct ForkPlanInput<'a> {
    pub source_thread: &'a OrchestrationV2AppThread,
    pub source_run: Option<&'a OrchestrationV2Run>,
    pub source_provider_thread: Option<&'a OrchestrationV2ProviderThread>,
    pub source_token_usage: Option<OrchestrationV2ProviderTurnTokenUsage>,
    pub canonical_source_point: OrchestrationV2ContextSourcePoint,
    pub transfer_id: ContextTransferId,
    pub target_thread_id: ThreadId,
    pub title: Option<String>,
    pub side_chat: bool,
    pub created_by: OrchestrationV2Actor,
    pub creation_source: OrchestrationV2CreationSource,
    pub created_at: IsoDateTime,
}

#[derive(Debug, Clone)]
pub struct ThreadForkPlan {
    pub target_thread: OrchestrationV2AppThread,
    pub transfer: OrchestrationV2ContextTransfer,
}

pub fn is_forkable_source_run_status(status: &OrchestrationV2RunStatus) -> bool {
    matches!(
        status,
        OrchestrationV2RunStatus::Completed
            | OrchestrationV2RunStatus::Waiting
            | OrchestrationV2RunStatus::Failed
            | OrchestrationV2RunStatus::Interrupted
            | OrchestrationV2RunStatus::Cancelled
    )
}

pub fn plan(input: ForkPlanInput<'_>) -> Result<ThreadForkPlan> {
    let source = input.source_thread;
    let side_chat = source.lineage.side_chat == Some(Some(true));
    let reject = |detail: String| Error::Service {
        operation: "ThreadForkService.plan",
        detail,
    };
    if input.side_chat
        && (side_chat
            || source.lineage.relationship_to_parent
                == Some(AppThreadLineageRelationshipToParent::Subagent)
            || source.delegated_from.is_some())
    {
        return Err(reject(
            if side_chat {
                "A side chat cannot start another side chat."
            } else {
                "A subagent cannot start a side chat."
            }
            .into(),
        ));
    }
    if let Some(run) = input.source_run
        && !is_forkable_source_run_status(&run.status)
    {
        return Err(reject(format!(
            "Fork source run {} is {}; in-progress and rolled-back runs cannot be forked.",
            expanded_id(&run.id)?.0,
            serde_json::to_value(&run.status)?
                .as_str()
                .unwrap_or("unknown")
        )));
    }
    let source_id = expanded_id(&source.id)?;
    let mut target = source.clone();
    target.id = IdOrIndex::Id(input.target_thread_id.clone());
    target.delegated_from = None;
    if let Some(Some(links)) = &mut target.pull_requests {
        for link in links {
            link.watch = None;
        }
    }
    target.created_by = input.created_by.clone();
    target.creation_source = input.creation_source;
    target.title = TrimmedNonEmptyString(
        input
            .title
            .unwrap_or_else(|| format!("{} fork", source.title.0)),
    );
    target.title_regeneration = Some(None);
    target.active_provider_thread_id = None;
    target.lineage = OrchestrationV2AppThreadLineage {
        parent_thread_id: Some(source_id.clone()),
        relationship_to_parent: Some(AppThreadLineageRelationshipToParent::Fork),
        root_thread_id: source.lineage.root_thread_id.clone(),
        side_chat: input.side_chat.then_some(Some(true)),
    };
    target.forked_from = input
        .source_run
        .map(|run| {
            expanded_id(&run.id).map(|run_id| {
                AppThreadForkedFrom::Run(Box::new(AppThreadForkedFromRun {
                    thread_id: source_id,
                    run_id,
                }))
            })
        })
        .transpose()?;
    target.created_at = input.created_at.clone();
    target.updated_at = input.created_at.clone();
    target.archived_at = None;
    target.settled_override = None;
    target.settled_at = None;
    target.snoozed_until = Some(None);
    target.snoozed_at = Some(None);
    target.last_snooze_wake_at = Some(None);
    target.pinned_at = Some(None);
    target.pin_order_key = Some(None);
    target.active_order_key = Some(None);
    target.auto_settle_disabled_at = Some(None);
    target.guest_share = Some(None);
    target.last_visited_at = None;
    target.opens = Some(None);
    target.deleted_at = None;
    let strong = input
        .source_provider_thread
        .and_then(|p| p.native_thread_ref.as_ref())
        .is_some_and(|r| r.strength == OrchestrationV2NativeRefStrength::Strong);
    let source_token_usage = if strong
        && input.source_run.is_some_and(|r| {
            matches!(
                r.status,
                OrchestrationV2RunStatus::Completed | OrchestrationV2RunStatus::Waiting
            )
        }) {
        input.source_token_usage.map(Some)
    } else {
        None
    };
    let transfer = OrchestrationV2ContextTransfer {
        id: IdOrIndex::Id(input.transfer_id),
        r#type: OrchestrationV2ContextTransferType::Fork,
        source_thread_id: source.id.clone(),
        target_thread_id: IdOrIndex::Id(input.target_thread_id),
        source_point: input.canonical_source_point,
        base_point: None,
        source_provider_instance_id: Some(input.source_run.map_or_else(
            || source.provider_instance_id.clone(),
            |r| r.provider_instance_id.clone(),
        )),
        source_token_usage,
        target_provider_instance_id: None,
        target_run_id: None,
        status: ContextTransferStatus::Pending,
        resolution: None,
        created_by: input.created_by,
        error: (!strong)
            .then(|| "Source provider thread does not expose a strong native thread ref.".into()),
        created_at: input.created_at.clone(),
        updated_at: input.created_at,
        consumed_at: None,
    };
    Ok(ThreadForkPlan {
        target_thread: target,
        transfer,
    })
}
