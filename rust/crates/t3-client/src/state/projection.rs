use super::{history::HistoryMeta, items, shell::upsert};
use crate::ClientError;
use t3_contracts::{
    base_schemas::ThreadId, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
};
pub fn event_thread(event: &OrchestrationV2DomainEvent) -> &ThreadId {
    match event {
        OrchestrationV2DomainEvent::ThreadCreated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadArchived(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadUnarchived(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadDeleted(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadSettled(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadUnsettled(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadSnoozed(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadUnsnoozed(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadPinned(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadAutoSettleSet(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadUnpinned(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadPinReordered(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadActiveReordered(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadTagsUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadVisited(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadMarkedUnread(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadMetadataUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadPullRequestSynced(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadRuntimeModeUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadInteractionModeUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadModelSelectionUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadProviderSwitched(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadGuestShareUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ThreadPullRequestLinkUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::RunCreated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::RunUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::RunBackgroundWorkCancelled(e) => &e.thread_id,
        OrchestrationV2DomainEvent::RunAttemptCreated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::RunAttemptUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::NodeUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::SubagentUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ProviderSessionAttached(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ProviderSessionUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ProviderSessionDetached(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ProviderThreadUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ProviderTurnUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::RuntimeRequestUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::MessageUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::TurnItemUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::PlanUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::CheckpointScopeCreated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::CheckpointCaptured(e) => &e.thread_id,
        OrchestrationV2DomainEvent::CheckpointRollbackRequested(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ContextHandoffUpdated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ContextTransferCreated(e) => &e.thread_id,
        OrchestrationV2DomainEvent::ContextTransferUpdated(e) => &e.thread_id,
    }
}
pub fn apply_event(
    p: &mut OrchestrationV2ThreadProjection,
    event: OrchestrationV2DomainEvent,
    history: &HistoryMeta,
) -> Result<bool, ClientError> {
    if IdOrIndex::Id(event_thread(&event).clone()) != p.thread.id {
        return Ok(false);
    }
    use OrchestrationV2DomainEvent::*;
    match event {
        ThreadCreated(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadArchived(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadUnarchived(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadDeleted(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadSettled(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadUnsettled(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadSnoozed(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadUnsnoozed(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadPinned(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadAutoSettleSet(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadUnpinned(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadPinReordered(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadActiveReordered(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadTagsUpdated(e) => {
            p.thread = e.payload;
        }
        ThreadVisited(e) => {
            p.thread = e.payload;
        }
        ThreadMarkedUnread(e) => {
            p.thread = e.payload;
        }
        ThreadMetadataUpdated(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadPullRequestSynced(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadRuntimeModeUpdated(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadInteractionModeUpdated(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadModelSelectionUpdated(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadProviderSwitched(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadGuestShareUpdated(e) => {
            p.updated_at = e.occurred_at;
            p.thread = e.payload;
        }
        ThreadPullRequestLinkUpdated(e) => {
            super::pull_requests::apply_update(&mut p.thread, e.payload);
            p.updated_at = e.occurred_at;
        }
        RunCreated(e) => {
            let was_queued = p
                .runs
                .iter()
                .any(|r| r.id == e.payload.id && r.status == OrchestrationV2RunStatus::Queued);
            let restore = was_queued && e.payload.status != OrchestrationV2RunStatus::Queued;
            let run_id = e.payload.id.clone();
            upsert(&mut p.runs, e.payload, |r| &r.id);
            filter_visible(p);
            if restore {
                let queued = p
                    .turn_items
                    .iter()
                    .filter(|i| {
                        items::run_id(i).as_ref() == Some(&run_id)
                            && items::queued_input(i)
                            && items::visible(p, i)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for item in queued {
                    p.visible_turn_items
                        .retain(|r| r.source_item_id != *items::id(&item));
                    insert_visible(p, item);
                }
            }
            items::renumber(&mut p.visible_turn_items);
            p.updated_at = e.occurred_at;
        }
        RunUpdated(e) => {
            let was_queued = p
                .runs
                .iter()
                .any(|r| r.id == e.payload.id && r.status == OrchestrationV2RunStatus::Queued);
            let restore = was_queued && e.payload.status != OrchestrationV2RunStatus::Queued;
            let run_id = e.payload.id.clone();
            upsert(&mut p.runs, e.payload, |r| &r.id);
            filter_visible(p);
            if restore {
                let queued = p
                    .turn_items
                    .iter()
                    .filter(|i| {
                        items::run_id(i).as_ref() == Some(&run_id)
                            && items::queued_input(i)
                            && items::visible(p, i)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for item in queued {
                    p.visible_turn_items
                        .retain(|r| r.source_item_id != *items::id(&item));
                    insert_visible(p, item);
                }
            }
            items::renumber(&mut p.visible_turn_items);
            p.updated_at = e.occurred_at;
        }
        RunBackgroundWorkCancelled(e) => {
            if let Some(run) = p
                .runs
                .iter_mut()
                .find(|r| r.id == IdOrIndex::Id(e.payload.run_id.clone()))
            {
                run.restart_cancelled_background_work =
                    Some(Some(e.payload.restart_cancelled_background_work));
            }
            p.updated_at = e.occurred_at;
        }
        RunAttemptCreated(e) => {
            upsert(&mut p.attempts, e.payload, |r| &r.id);
            filter_visible(p);
            p.updated_at = e.occurred_at;
        }
        RunAttemptUpdated(e) => {
            upsert(&mut p.attempts, e.payload, |r| &r.id);
            filter_visible(p);
            p.updated_at = e.occurred_at;
        }
        NodeUpdated(e) => {
            upsert(&mut p.nodes, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        SubagentUpdated(e) => {
            let mut value = e.payload;
            let previous = p.subagents.iter().find(|a| a.id == value.id);
            items::restore_subagent(&mut value, previous);
            upsert(&mut p.subagents, value, |a| &a.id);
            p.updated_at = e.occurred_at;
        }
        ProviderSessionAttached(e) => {
            upsert(&mut p.provider_sessions, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        ProviderSessionUpdated(e) => {
            upsert(&mut p.provider_sessions, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        ProviderSessionDetached(e) => {
            p.provider_sessions
                .retain(|r| r.id != IdOrIndex::Id(e.payload.provider_session_id.clone()));
            p.updated_at = e.occurred_at;
        }
        ProviderThreadUpdated(e) => {
            upsert(&mut p.provider_threads, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        ProviderTurnUpdated(e) => {
            let mut value = e.payload;
            if value
                .token_usage
                .as_ref()
                .and_then(|v| v.as_ref())
                .is_none()
                && let Some(previous) = p.provider_turns.iter().find(|v| v.id == value.id)
            {
                value.token_usage = previous.token_usage.clone();
            }
            upsert(&mut p.provider_turns, value, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        RuntimeRequestUpdated(e) => {
            upsert(&mut p.runtime_requests, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        MessageUpdated(e) => {
            upsert(&mut p.messages, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        TurnItemUpdated(e) => {
            let mut item = e.payload;
            items::restore_body(&mut item, p);
            let existing = p
                .turn_items
                .iter()
                .rposition(|i| items::id(i) == items::id(&item));
            if let Some(index) = existing
                && items::deferred(&p.turn_items[index]).is_some()
            {
                items::set_deferred(&mut item, items::deferred(&p.turn_items[index]).clone());
            }
            if existing.is_none()
                && history.partial()
                && drop_partial(p, &item, history.latest_local_ordinal)
            {
                return Ok(false);
            }
            let was_interrupt = existing.is_some_and(|index| {
                matches!(
                    p.turn_items[index],
                    OrchestrationV2TurnItem::RunInterruptRequest(_)
                )
            });
            if let Some(index) = existing {
                p.turn_items[index] = item.clone();
            } else {
                p.turn_items.push(item.clone());
            }
            if matches!(item, OrchestrationV2TurnItem::RunInterruptRequest(_)) || was_interrupt {
                filter_visible(p);
            }
            if items::visible(p, &item) {
                upsert_visible(p, item);
            } else {
                p.visible_turn_items
                    .retain(|r| r.source_item_id != *items::id(&item));
                items::renumber(&mut p.visible_turn_items);
            }
            p.updated_at = e.occurred_at;
        }
        PlanUpdated(e) => {
            upsert(&mut p.plans, e.payload, |r| match r {
                OrchestrationV2PlanArtifact::ProposedPlan(v) => &v.id,
                OrchestrationV2PlanArtifact::TodoList(v) => &v.id,
            });
            p.updated_at = e.occurred_at;
        }
        CheckpointScopeCreated(e) => {
            upsert(&mut p.checkpoint_scopes, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        CheckpointCaptured(e) => {
            upsert(&mut p.checkpoints, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        CheckpointRollbackRequested(e) => {
            p.updated_at = e.occurred_at;
        }
        ContextHandoffUpdated(e) => {
            upsert(&mut p.context_handoffs, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        ContextTransferCreated(e) => {
            upsert(&mut p.context_transfers, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
        ContextTransferUpdated(e) => {
            upsert(&mut p.context_transfers, e.payload, |r| &r.id);
            p.updated_at = e.occurred_at;
        }
    }
    Ok(true)
}
pub(crate) fn ignores_partial_item(
    p: &OrchestrationV2ThreadProjection,
    event: &OrchestrationV2DomainEvent,
    history: &HistoryMeta,
) -> bool {
    if let OrchestrationV2DomainEvent::TurnItemUpdated(event) = event {
        history.partial()
            && !p
                .turn_items
                .iter()
                .any(|item| items::id(item) == items::id(&event.payload))
            && drop_partial(p, &event.payload, history.latest_local_ordinal)
    } else {
        false
    }
}
fn filter_visible(p: &mut OrchestrationV2ThreadProjection) {
    let visibility = items::Visibility::new(p);
    let mut rows = std::mem::take(&mut p.visible_turn_items);
    rows.retain(|r| {
        r.visibility != ProjectedTurnItemVisibility::Local || visibility.visible(&r.item)
    });
    items::renumber(&mut rows);
    p.visible_turn_items = rows;
}
fn item_order(a: &OrchestrationV2TurnItem, b: &OrchestrationV2TurnItem) -> std::cmp::Ordering {
    items::ordinal(a)
        .cmp(&items::ordinal(b))
        .then_with(|| match (items::id(a), items::id(b)) {
            (IdOrIndex::Id(a), IdOrIndex::Id(b)) => a.0.cmp(&b.0),
            (IdOrIndex::Index(a), IdOrIndex::Index(b)) => a.cmp(b),
            (IdOrIndex::Id(_), IdOrIndex::Index(_)) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        })
}
fn insert_visible(p: &mut OrchestrationV2ThreadProjection, item: OrchestrationV2TurnItem) {
    let index = p
        .visible_turn_items
        .iter()
        .position(|r| {
            r.visibility == ProjectedTurnItemVisibility::Local && item_order(&r.item, &item).is_gt()
        })
        .unwrap_or(p.visible_turn_items.len());
    let row = OrchestrationV2ProjectedTurnItem {
        position: 0,
        visibility: ProjectedTurnItemVisibility::Local,
        source_thread_id: items::thread_id(&item).clone(),
        source_item_id: items::id(&item).clone(),
        item,
    };
    p.visible_turn_items.insert(index, row);
}
fn upsert_visible(p: &mut OrchestrationV2ThreadProjection, item: OrchestrationV2TurnItem) {
    if let Some(index) = p
        .visible_turn_items
        .iter()
        .rposition(|r| r.source_item_id == *items::id(&item))
    {
        let row = &mut p.visible_turn_items[index];
        if row.visibility == ProjectedTurnItemVisibility::Local
            && row.source_thread_id == *items::thread_id(&item)
            && items::ordinal(&row.item) == items::ordinal(&item)
        {
            row.item = item;
            return;
        }
        p.visible_turn_items.remove(index);
    }
    insert_visible(p, item);
    items::renumber(&mut p.visible_turn_items);
}
fn drop_partial(
    p: &OrchestrationV2ThreadProjection,
    item: &OrchestrationV2TurnItem,
    watermark: Option<u64>,
) -> bool {
    if p.visible_turn_items
        .iter()
        .any(|r| r.source_item_id == *items::id(item))
    {
        return false;
    }
    if let Some(run) = items::run_id(item) {
        if p.runs.iter().any(|r| {
            &r.id == run
                && matches!(
                    r.status,
                    OrchestrationV2RunStatus::Preparing
                        | OrchestrationV2RunStatus::Queued
                        | OrchestrationV2RunStatus::Starting
                        | OrchestrationV2RunStatus::Running
                        | OrchestrationV2RunStatus::Waiting
                )
        }) {
            return false;
        }
        if p.visible_turn_items.iter().any(|r| {
            r.visibility == ProjectedTurnItemVisibility::Local
                && items::run_id(&r.item).as_ref() == Some(run)
                && items::ordinal(&r.item) <= items::ordinal(item)
        }) {
            return false;
        }
    }
    watermark.is_some_and(|n| items::ordinal(item) <= n)
        || p.visible_turn_items
            .iter()
            .filter(|r| r.visibility == ProjectedTurnItemVisibility::Local)
            .map(|r| items::ordinal(&r.item))
            .min()
            .is_some_and(|n| items::ordinal(item) < n)
}
