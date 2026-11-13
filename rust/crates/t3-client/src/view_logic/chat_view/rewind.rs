use crate::{ClientError, types::ThreadKey};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    base_schemas::*, orchestration_v2::*, orchestration_v2_id_table::IdOrIndex,
    provider_instance::ProviderInstanceId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum IdKey<'a> {
    Id(&'a str),
    Index(u64),
}

pub(super) fn id_key<T: AsRef<str>>(id: &IdOrIndex<T>) -> IdKey<'_> {
    match id {
        IdOrIndex::Id(id) => IdKey::Id(id.as_ref()),
        IdOrIndex::Index(index) => IdKey::Index(*index),
    }
}

pub fn derive_available_revert_turn_counts(
    projection: Option<&OrchestrationV2ThreadProjection>,
) -> BTreeSet<u64> {
    let mut available = BTreeSet::new();
    let Some(p) = projection else {
        return available;
    };
    let Some(thread) = p
        .provider_threads
        .iter()
        .find(|t| Some(&t.id) == p.thread.active_provider_thread_id.as_ref())
    else {
        return available;
    };
    if thread.provider_session_id.is_none()
        || thread.provider_instance_id
            != IdOrIndex::Id(p.thread.model_selection.instance_id.clone())
    {
        return available;
    }
    available.insert(0);
    let attempts: BTreeMap<_, _> = p
        .attempts
        .iter()
        .map(|attempt| (id_key(&attempt.id), attempt))
        .collect();
    let turns: BTreeMap<_, _> = p
        .provider_turns
        .iter()
        .map(|turn| (id_key(&turn.id), turn))
        .collect();
    let mut latest = BTreeMap::<_, &OrchestrationV2ProviderTurn>::new();
    for turn in &p.provider_turns {
        if let Some(attempt) = &turn.run_attempt_id {
            // Equal ordinals retain the earliest input row, matching the source's strict >.
            let entry = latest.entry(id_key(attempt)).or_insert(turn);
            if turn.ordinal > entry.ordinal {
                *entry = turn;
            }
        }
    }
    for run in &p.runs {
        let Some(attempt) = &run.active_attempt_id else {
            continue;
        };
        let turn = latest.get(&id_key(attempt)).copied().or_else(|| {
            attempts
                .get(&id_key(attempt))?
                .provider_turn_id
                .as_ref()
                .and_then(|id| turns.get(&id_key(id)).copied())
        });
        if turn.is_some_and(|turn| turn.provider_thread_id == thread.id) {
            available.insert(run.ordinal);
        }
    }
    available
}

#[derive(Debug, Clone)]
pub struct RevertWitness {
    thread_id: IdOrIndex<ThreadId>,
    provider_thread_id: Option<IdOrIndex<ProviderThreadId>>,
    provider_instance_id: Option<IdOrIndex<ProviderInstanceId>>,
    initial_provider_threads: Vec<IdOrIndex<ProviderThreadId>>,
    witness_run_id: IdOrIndex<RunId>,
    turn_count: u64,
    request_id: CommandId,
}

impl RevertWitness {
    pub fn capture(
        projection: &OrchestrationV2ThreadProjection,
        turn_count: u64,
        request_id: CommandId,
    ) -> Result<Self, ClientError> {
        let mut witness: Option<&OrchestrationV2Run> = None;
        for run in &projection.runs {
            if run.ordinal > turn_count
                && matches!(
                    run.status,
                    OrchestrationV2RunStatus::Completed
                        | OrchestrationV2RunStatus::Interrupted
                        | OrchestrationV2RunStatus::Failed
                        | OrchestrationV2RunStatus::Cancelled
                )
                && witness.is_none_or(|old| run.ordinal > old.ordinal)
            {
                witness = Some(run);
            }
        }
        let Some(witness) = witness else {
            return Err(ClientError::InvalidPayload(
                "The thread has changed. Select a message to rewind again.".into(),
            ));
        };
        let provider_thread_id = projection.thread.active_provider_thread_id.clone();
        let initial = projection
            .provider_threads
            .iter()
            .find(|thread| Some(&thread.id) == provider_thread_id.as_ref());
        Ok(Self {
            thread_id: projection.thread.id.clone(),
            provider_thread_id,
            provider_instance_id: initial.map(|t| t.provider_instance_id.clone()),
            initial_provider_threads: projection
                .provider_threads
                .iter()
                .map(|t| t.id.clone())
                .collect(),
            witness_run_id: witness.id.clone(),
            turn_count,
            request_id,
        })
    }

    pub fn inspect(
        &self,
        projection: &OrchestrationV2ThreadProjection,
        accepted: bool,
    ) -> Result<bool, ClientError> {
        if let Some(failure) = projection
            .thread
            .rollback_failure
            .as_ref()
            .and_then(Option::as_ref)
            && failure.request_id == self.request_id
        {
            return Err(ClientError::InvalidPayload(failure.message.0.clone()));
        }
        Ok(accepted
            && projection.provider_threads.iter().any(|thread| {
                let same = Some(&thread.id) == self.provider_thread_id.as_ref();
                let fresh_root = self.turn_count == 0
                    && thread.owner_node_id.is_none()
                    && thread.app_thread_id.as_ref() == Some(&self.thread_id)
                    && Some(&thread.provider_instance_id) == self.provider_instance_id.as_ref()
                    && !self.initial_provider_threads.contains(&thread.id);
                (same || fresh_root)
                    && thread.last_run_ordinal
                        == if self.turn_count == 0 {
                            None
                        } else {
                            Some(self.turn_count)
                        }
            })
            && projection.runs.iter().any(|run| {
                run.id == self.witness_run_id && run.status == OrchestrationV2RunStatus::RolledBack
            }))
    }
}

pub async fn wait_for_reverted_message<P, F>(
    store: &crate::ClientStore,
    platform: &P,
    thread: &ThreadKey,
    turn_count: u64,
    request_id: CommandId,
    revert: F,
    timeout_ms: u64,
) -> Result<(), ClientError>
where
    P: crate::platform::Platform + ?Sized,
    F: std::future::Future<Output = Result<(), ClientError>>,
{
    use futures::{FutureExt, StreamExt};
    let projection = || {
        store
            .snapshot()
            .environments
            .get(&thread.environment)?
            .threads
            .get(&thread.thread)?
            .projection
            .clone()
    };
    let initial = projection().ok_or_else(|| {
        ClientError::InvalidPayload(
            "The thread has changed. Select a message to rewind again.".into(),
        )
    })?;
    let witness = RevertWitness::capture(&initial, turn_count, request_id)?;
    let mut subscription = store.subscribe();
    let mut accepted = false;
    let revert = revert.fuse();
    let timeout = platform.sleep(timeout_ms).fuse();
    futures::pin_mut!(revert, timeout);
    loop {
        if let Some(projection) = projection()
            && witness.inspect(&projection, accepted)?
        {
            return Ok(());
        }
        futures::select! {
            result = revert => { result?; accepted = true; },
            _ = timeout => return Err(ClientError::InvalidPayload("Timed out waiting for the thread to rewind.".into())),
            change = subscription.next().fuse() => { if change.is_none() { return Err(ClientError::Cancelled); } },
        }
    }
}
