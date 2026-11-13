use crate::{
    ClientStore,
    types::{CommandId, ThreadKey},
};
use futures::StreamExt;
use t3_contracts::{orchestration_v2::*, thread_title::ThreadTitleRegenerationOutcome};
pub enum TitleOutcome {
    Pending,
    Gone,
    Settled(ThreadTitleRegenerationOutcome),
}
pub fn settled_outcome(
    snapshot: Option<&OrchestrationV2ShellSnapshot>,
    thread: &t3_contracts::base_schemas::ThreadId,
    request: &CommandId,
    sequence: u64,
) -> TitleOutcome {
    let Some(snapshot) = snapshot.filter(|s| s.snapshot_sequence >= sequence) else {
        return TitleOutcome::Pending;
    };
    let Some(thread) = snapshot
        .threads
        .iter()
        .chain(&snapshot.archived_threads)
        .find(|t| &t.id == thread)
    else {
        return TitleOutcome::Gone;
    };
    if let Some(last) = thread
        .last_title_regeneration
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|last| &last.request_id == request)
    {
        return TitleOutcome::Settled(ThreadTitleRegenerationOutcome {
            status: last.status,
            message: last.message.clone(),
        });
    }
    if thread
        .title_regeneration
        .as_ref()
        .and_then(Option::as_ref)
        .is_some_and(|r| &r.request_id == request)
    {
        TitleOutcome::Pending
    } else {
        TitleOutcome::Gone
    }
}
pub async fn wait_for_title_regeneration_outcome(
    store: &ClientStore,
    key: &ThreadKey,
    request: &CommandId,
    sequence: u64,
) -> Option<ThreadTitleRegenerationOutcome> {
    let mut subscription = store.subscribe();
    loop {
        let snapshot = store.snapshot();
        let environment = snapshot.environments.get(&key.environment)?;
        match settled_outcome(
            environment.shell.snapshot.as_deref(),
            &key.thread,
            request,
            sequence,
        ) {
            TitleOutcome::Settled(outcome) => return Some(outcome.clone()),
            TitleOutcome::Gone => return None,
            TitleOutcome::Pending => {}
        }
        if matches!(
            environment.shell.status,
            crate::types::SyncStatus::Cached
                | crate::types::SyncStatus::Empty
                | crate::types::SyncStatus::Deleted
        ) {
            return None;
        }
        subscription.next().await?;
    }
}
