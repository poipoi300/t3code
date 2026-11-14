use crate::{
    ClientError,
    optimistic::{OptimisticState, ProjectionKey, ViewChange},
    persistence::Record,
    platform::{
        Platform, Storage, StorageArea, StorageCondition, StorageKey, StorageTransaction,
        StorageWrite,
    },
    rpc::{RpcCallError, RpcEndpoint},
    types::{CommandId, EnvironmentId},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::{Rc, Weak},
    task::{Poll, Waker},
};
use t3_contracts::{
    orchestration_v2::*,
    rpc::{RpcMethod, WsOrchestrationV2DispatchCommandRpc},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub command: CommandId,
    pub environment: EnvironmentId,
    pub committed_sequence: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandStatus {
    Pending,
    Accepted { sequence: u64 },
    AcceptedAwaitingEntity,
    Rejected(String),
    UnknownDelivery,
}
#[derive(Debug)]
pub enum CommandOutcome<T> {
    Accepted {
        value: T,
        receipt: Option<CommandReceipt>,
    },
    Rejected {
        reason: String,
    },
    UnknownAfterDisconnect,
    /// The session ended before the request left; the command is pending again.
    NotSent,
}
#[derive(Debug)]
pub struct DeliveryReport<T> {
    pub outcome: CommandOutcome<T>,
    pub persistence_error: Option<ClientError>,
}
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("command is already queued or executing")]
    Duplicate,
    #[error("command requires receipt reconciliation before another delivery")]
    Ambiguous,
    #[error("operation identity or method mismatch")]
    Identity,
}
#[derive(Debug)]
pub struct RestoreReport {
    pub restored: usize,
    pub errors: Vec<OperationError>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredOperation<I> {
    pub ordinal: u64,
    pub command: CommandId,
    pub projection: ProjectionKey,
    pub generation: u64,
    pub method: String,
    pub input: Value,
    pub change: ViewChange<I>,
    pub status: CommandStatus,
}
struct Operations<I> {
    optimistic: OptimisticState<ViewChange<I>>,
    records: BTreeMap<CommandId, StoredOperation<I>>,
    busy: BTreeSet<CommandId>,
    next_ordinal: u64,
    lanes: BTreeMap<(&'static str, String), Weak<RefCell<SerialLane>>>,
    /// Commands restored from storage. Projection lineages restart with each run, so their
    /// saved lineage proves nothing; they revalidate on their projection's next observation.
    from_earlier_run: BTreeSet<CommandId>,
}
/// What [`OperationQueue::observe_store`] last saw of the store.
#[derive(Debug, Default)]
pub struct StoreProgress {
    session: Option<crate::types::Generation>,
    lineages: BTreeMap<ProjectionKey, u64>,
}
/// One destination's durable commands. Shells keep one instance per environment.
#[derive(Clone)]
pub struct OperationQueue<I> {
    environment: EnvironmentId,
    inner: Rc<RefCell<Operations<I>>>,
}
const OUTBOX_NAMESPACE: &str = "operation-outbox";
impl<I: Clone + Serialize + DeserializeOwned> OperationQueue<I> {
    pub fn new(environment: EnvironmentId) -> Self {
        Self {
            environment,
            inner: Rc::new(RefCell::new(Operations {
                optimistic: OptimisticState::default(),
                records: BTreeMap::new(),
                busy: BTreeSet::new(),
                next_ordinal: 0,
                lanes: BTreeMap::new(),
                from_earlier_run: BTreeSet::new(),
            })),
        }
    }
    fn key(&self, command: &CommandId) -> StorageKey {
        StorageKey {
            namespace: OUTBOX_NAMESPACE.into(),
            environment: Some(self.environment.clone()),
            record: command.0.clone(),
        }
    }
    fn bytes(&self, operation: &StoredOperation<I>) -> Result<Vec<u8>, ClientError> {
        Ok(serde_json::to_vec(&Record {
            format_version: 1,
            kind: OUTBOX_NAMESPACE.into(),
            environment_id: Some(self.environment.clone()),
            record_id: operation.command.0.clone(),
            payload_version: 1,
            payload: operation,
        })?)
    }
    async fn persist(
        &self,
        storage: &impl Storage,
        operation: &StoredOperation<I>,
        expected: Option<Vec<u8>>,
    ) -> Result<(), ClientError> {
        let changed = storage
            .transaction(StorageTransaction {
                conditions: vec![StorageCondition {
                    area: StorageArea::Ordinary,
                    key: self.key(&operation.command),
                    expected,
                }],
                ordinary: vec![StorageWrite {
                    key: self.key(&operation.command),
                    value: Some(self.bytes(operation)?),
                }],
                secure: Vec::new(),
            })
            .await?;
        if !changed {
            return Err(ClientError::InvalidPayload(
                "operation changed in storage; reload before delivery".into(),
            ));
        }
        Ok(())
    }
    pub fn status(&self, command: &CommandId) -> Option<CommandStatus> {
        self.inner
            .borrow()
            .records
            .get(command)
            .map(|record| record.status.clone())
    }
    pub fn records(&self) -> Vec<StoredOperation<I>> {
        self.inner.borrow().records.values().cloned().collect()
    }
    pub fn view<V: Clone>(
        &self,
        base: &Rc<V>,
        projection: &ProjectionKey,
        apply: impl FnMut(&mut V, &ViewChange<I>, &CommandStatus),
    ) -> Rc<V> {
        self.inner.borrow().optimistic.view(base, projection, apply)
    }
    pub async fn enqueue<M: RpcMethod>(
        &self,
        storage: &impl Storage,
        input: &M::Payload,
        projection: ProjectionKey,
        generation: u64,
        change: ViewChange<I>,
    ) -> Result<CommandId, OperationError> {
        if M::STREAMS || projection.environment != self.environment {
            return Err(OperationError::Identity);
        }
        let input = serde_json::to_value(input).map_err(ClientError::from)?;
        let command = command_identity(&input)?;
        let ordinal = {
            let mut inner = self.inner.borrow_mut();
            if inner.records.contains_key(&command) || !inner.busy.insert(command.clone()) {
                return Err(OperationError::Duplicate);
            }
            let ordinal = inner.next_ordinal;
            inner.next_ordinal += 1;
            ordinal
        };
        let _guard = BusyGuard {
            inner: self.inner.clone(),
            command: command.clone(),
        };
        let record = StoredOperation {
            ordinal,
            command: command.clone(),
            projection: projection.clone(),
            generation,
            method: M::TAG.into(),
            input,
            change: change.clone(),
            status: CommandStatus::Pending,
        };
        self.persist(storage, &record, None).await?;
        let mut inner = self.inner.borrow_mut();
        inner
            .optimistic
            .insert(command.clone(), projection, generation, change);
        inner.records.insert(command.clone(), record);
        Ok(command)
    }
    pub async fn deliver<M: RpcMethod>(
        &self,
        storage: &impl Storage,
        endpoint: &impl RpcEndpoint,
        command: &CommandId,
        receipt_sequence: impl FnOnce(&M::Success) -> Option<u64>,
    ) -> Result<DeliveryReport<M::Success>, OperationError>
    where
        M::Success: 'static,
    {
        if endpoint
            .destination()
            .is_some_and(|environment| environment != self.environment)
        {
            return Err(OperationError::Identity);
        }
        let mut record = {
            let mut inner = self.inner.borrow_mut();
            let record = inner
                .records
                .get(command)
                .ok_or(OperationError::Identity)?
                .clone();
            if record.method != M::TAG || record.projection.environment != self.environment {
                return Err(OperationError::Identity);
            }
            if record.status != CommandStatus::Pending {
                return Err(OperationError::Ambiguous);
            }
            if !inner.busy.insert(command.clone()) {
                return Err(OperationError::Duplicate);
            }
            record
        };
        let _guard = BusyGuard {
            inner: self.inner.clone(),
            command: command.clone(),
        };
        let lane = {
            let mut inner = self.inner.borrow_mut();
            inner.lanes.retain(|_, lane| lane.strong_count() > 0);
            let key = if let Some(thread) = record
                .input
                .get("threadId")
                .or_else(|| record.input.get("sourceThreadId"))
                .and_then(Value::as_str)
            {
                ("thread", thread.to_owned())
            } else if let Some(project) = record.input.get("projectId").and_then(Value::as_str) {
                ("project", project.to_owned())
            } else {
                ("command", command.0.clone())
            };
            let lane = inner
                .lanes
                .get(&key)
                .and_then(Weak::upgrade)
                .unwrap_or_default();
            inner.lanes.insert(key, Rc::downgrade(&lane));
            lane
        };
        let permit = SerialPermit::reserve(lane);
        permit.ready().await;
        if self.status(command) != Some(CommandStatus::Pending) {
            return Err(OperationError::Ambiguous);
        }
        let input: M::Payload =
            serde_json::from_value(record.input.clone()).map_err(ClientError::from)?;
        if serde_json::to_value(&input).map_err(ClientError::from)? != record.input {
            return Err(OperationError::Identity);
        }
        // Persist uncertainty before any request can leave; cancellation/restart cannot replay it.
        let expected = self.bytes(&record)?;
        record.status = CommandStatus::UnknownDelivery;
        self.persist(storage, &record, Some(expected)).await?;
        let attempt_bytes = self.bytes(&record)?;
        self.inner
            .borrow_mut()
            .records
            .insert(command.clone(), record.clone());
        let response = endpoint.command::<M>(input).await;
        let outcome = match response {
            Ok(value) => match receipt_sequence(&value) {
                Some(sequence) => {
                    let receipt = CommandReceipt {
                        command: command.clone(),
                        environment: self.environment.clone(),
                        committed_sequence: sequence,
                    };
                    record.status = CommandStatus::Accepted { sequence };
                    if self.inner.borrow_mut().optimistic.accepted(&receipt) {
                        CommandOutcome::Accepted {
                            value,
                            receipt: Some(receipt),
                        }
                    } else {
                        record.status = CommandStatus::UnknownDelivery;
                        CommandOutcome::UnknownAfterDisconnect
                    }
                }
                None => {
                    if self
                        .inner
                        .borrow_mut()
                        .optimistic
                        .accepted_entity(command, &self.environment)
                    {
                        record.status = CommandStatus::AcceptedAwaitingEntity;
                        CommandOutcome::Accepted {
                            value,
                            receipt: None,
                        }
                    } else {
                        CommandOutcome::UnknownAfterDisconnect
                    }
                }
            },
            Err(RpcCallError::Rejected(errors)) => {
                let reason = crate::rpc::rejection(&errors).to_string();
                record.status = CommandStatus::Rejected(reason.clone());
                self.inner.borrow_mut().optimistic.reject(command);
                CommandOutcome::Rejected { reason }
            }
            // The session ended before the request left: nothing ran, so the command is pending
            // again, with its overlay, for the next session.
            Err(RpcCallError::NotSent(ClientError::Disconnected)) => {
                record.status = CommandStatus::Pending;
                CommandOutcome::NotSent
            }
            Err(RpcCallError::NotSent(_)) => {
                let reason = "RPC request was not sent".to_owned();
                record.status = CommandStatus::Rejected(reason.clone());
                self.inner.borrow_mut().optimistic.reject(command);
                CommandOutcome::Rejected { reason }
            }
            Err(_) => {
                self.inner.borrow_mut().optimistic.unknown(command);
                CommandOutcome::UnknownAfterDisconnect
            }
        };
        self.inner
            .borrow_mut()
            .records
            .insert(command.clone(), record.clone());
        let persistence_error = self
            .persist(storage, &record, Some(attempt_bytes))
            .await
            .err();
        Ok(DeliveryReport {
            outcome,
            persistence_error,
        })
    }
    pub async fn dispatch(
        &self,
        storage: &impl Storage,
        endpoint: &impl RpcEndpoint,
        command: &CommandId,
    ) -> Result<DeliveryReport<OrchestrationV2DispatchCommandResult>, OperationError> {
        self.deliver::<WsOrchestrationV2DispatchCommandRpc>(storage, endpoint, command, |reply| {
            Some(reply.sequence)
        })
        .await
    }
    pub fn observe(&self, projection: &ProjectionKey, generation: u64, sequence: u64) {
        self.inner
            .borrow_mut()
            .optimistic
            .observe(projection, generation, sequence);
    }
    pub fn reset(&self, projection: &ProjectionKey, generation: u64, sequence: u64) {
        let mut inner = self.inner.borrow_mut();
        // A pending record never left: delivery persists "unknown" before the request goes.
        let unsent: BTreeSet<_> = inner
            .records
            .values()
            .filter(|record| record.status == CommandStatus::Pending)
            .map(|record| record.command.clone())
            .collect();
        inner
            .optimistic
            .reset(projection, generation, sequence, &unsent);
        let invalidated: BTreeSet<_> = inner
            .records
            .keys()
            .filter(|id| inner.optimistic.needs_revalidation(id))
            .cloned()
            .collect();
        for record in inner.records.values_mut() {
            if invalidated.contains(&record.command)
                && !matches!(record.status, CommandStatus::Rejected(_))
            {
                record.status = CommandStatus::UnknownDelivery;
            }
        }
    }
    pub fn disconnect(&self) {
        let mut inner = self.inner.borrow_mut();
        let uncertain: Vec<_> = inner
            .records
            .values()
            .filter(|record| record.status == CommandStatus::UnknownDelivery)
            .map(|record| record.command.clone())
            .collect();
        for command in uncertain {
            inner.optimistic.unknown(&command);
        }
    }
    /// Feeds the store's progress to this queue's overlays: each projection it has commands
    /// for is observed at its lineage and sequence, so accepted overlays retire once the
    /// authoritative rows reach their receipt; a new lineage (an authoritative snapshot) resets
    /// it so pending commands revalidate; a new store session marks deliveries uncertain.
    pub fn observe_store(&self, store: &crate::ClientStore, progress: &mut StoreProgress) {
        let keys: BTreeMap<ProjectionKey, (BTreeSet<u64>, bool)> = {
            let inner = self.inner.borrow();
            inner
                .records
                .values()
                .fold(BTreeMap::new(), |mut keys, record| {
                    let entry: &mut (BTreeSet<u64>, bool) =
                        keys.entry(record.projection.clone()).or_default();
                    entry.0.insert(record.generation);
                    entry.1 |= inner.from_earlier_run.contains(&record.command);
                    keys
                })
        };
        let snapshot = store.snapshot();
        let Some(environment) = snapshot.environments.get(&self.environment) else {
            return;
        };
        if progress
            .session
            .replace(environment.session)
            .is_some_and(|session| session != environment.session)
        {
            self.disconnect();
        }
        for (key, (enqueued, earlier_run)) in keys {
            let state = match &key.thread {
                None => environment
                    .shell
                    .snapshot
                    .as_ref()
                    .map(|shell| (environment.shell.generation, shell.snapshot_sequence)),
                Some(thread) => environment
                    .threads
                    .get(thread)
                    .filter(|thread| thread.projection.is_some())
                    .map(|thread| (thread.generation, thread.sequence)),
            };
            let Some((lineage, sequence)) = state else {
                continue;
            };
            // First sight compares with the lineages the commands were enqueued under, so a
            // command restored from an earlier lineage is revalidated too.
            let moved = match progress.lineages.insert(key.clone(), lineage) {
                Some(previous) => previous != lineage,
                None => enqueued.iter().any(|generation| *generation != lineage),
            };
            if moved || earlier_run {
                self.reset(&key, lineage, sequence);
                let mut inner = self.inner.borrow_mut();
                let Operations {
                    records,
                    from_earlier_run,
                    ..
                } = &mut *inner;
                from_earlier_run.retain(|command| {
                    records
                        .get(command)
                        .is_none_or(|record| record.projection != key)
                });
            } else {
                self.observe(&key, lineage, sequence);
            }
        }
    }
    /// [`Self::observe_store`] on every store change until the store goes away. The shell keeps
    /// this future beside the queue and drops it with the queue.
    pub async fn follow_store(&self, store: crate::ClientStore) {
        use futures::StreamExt;
        let mut changes = store.subscribe_environment(&self.environment);
        let mut progress = StoreProgress::default();
        while changes.next().await.is_some() {
            self.observe_store(&store, &mut progress);
        }
    }
    pub async fn resolve_entity(
        &self,
        storage: &impl Storage,
        command: &CommandId,
        projection: &ProjectionKey,
        generation: u64,
    ) -> Result<(), OperationError> {
        if projection.environment != self.environment {
            return Err(OperationError::Identity);
        }
        let mut record = self
            .inner
            .borrow()
            .records
            .get(command)
            .cloned()
            .ok_or(OperationError::Identity)?;
        let expected = storage
            .get(StorageArea::Ordinary, self.key(command))
            .await
            .map_err(ClientError::from)?;
        {
            let mut inner = self.inner.borrow_mut();
            if inner.busy.contains(command) {
                return Err(OperationError::Duplicate);
            }
            let valid = if inner.optimistic.needs_revalidation(command) {
                inner
                    .optimistic
                    .revalidated_entity(command, projection, generation)
            } else {
                inner
                    .optimistic
                    .matching_entity(command, projection, generation)
            };
            if !valid {
                return Err(OperationError::Identity);
            }
        }
        record.status = CommandStatus::AcceptedAwaitingEntity;
        record.generation = generation;
        self.inner
            .borrow_mut()
            .records
            .insert(command.clone(), record.clone());
        self.persist(storage, &record, expected).await?;
        Ok(())
    }
    pub async fn resolve_receipt(
        &self,
        storage: &impl Storage,
        receipt: &CommandReceipt,
        generation: u64,
    ) -> Result<(), OperationError> {
        if receipt.environment != self.environment {
            return Err(OperationError::Identity);
        }
        let mut record = self
            .inner
            .borrow()
            .records
            .get(&receipt.command)
            .cloned()
            .ok_or(OperationError::Identity)?;
        let expected = storage
            .get(StorageArea::Ordinary, self.key(&receipt.command))
            .await
            .map_err(ClientError::from)?;
        {
            let mut inner = self.inner.borrow_mut();
            if inner.busy.contains(&receipt.command) {
                return Err(OperationError::Duplicate);
            }
            let valid = if inner.optimistic.needs_revalidation(&receipt.command) {
                inner.optimistic.revalidated(receipt, generation)
            } else {
                record.generation == generation && inner.optimistic.accepted(receipt)
            };
            if !valid {
                return Err(OperationError::Identity);
            }
            record.status = CommandStatus::Accepted {
                sequence: receipt.committed_sequence,
            };
            record.generation = generation;
            inner
                .records
                .insert(receipt.command.clone(), record.clone());
        }
        self.persist(storage, &record, expected).await?;
        Ok(())
    }
    /// Removes a never-sent or rejected command after the caller has recovered its draft.
    pub async fn remove_unsent(
        &self,
        storage: &impl Storage,
        command: &CommandId,
    ) -> Result<(), OperationError> {
        let record = {
            let inner = self.inner.borrow();
            let record = inner
                .records
                .get(command)
                .cloned()
                .ok_or(OperationError::Identity)?;
            if inner.busy.contains(command) {
                return Err(OperationError::Duplicate);
            }
            if !matches!(
                record.status,
                CommandStatus::Pending | CommandStatus::Rejected(_)
            ) {
                return Err(OperationError::Ambiguous);
            }
            record
        };
        let removed = storage
            .transaction(StorageTransaction {
                conditions: vec![StorageCondition {
                    area: StorageArea::Ordinary,
                    key: self.key(command),
                    expected: Some(self.bytes(&record)?),
                }],
                ordinary: vec![StorageWrite {
                    key: self.key(command),
                    value: None,
                }],
                secure: Vec::new(),
            })
            .await
            .map_err(ClientError::from)?;
        if !removed {
            return Err(OperationError::Ambiguous);
        }
        let mut inner = self.inner.borrow_mut();
        inner.records.remove(command);
        inner.optimistic.reject(command);
        Ok(())
    }
    /// Retries storage of known outcomes, without delivering any request again.
    pub async fn flush_outcomes(&self, storage: &impl Storage) -> Result<usize, OperationError> {
        let records: Vec<_> = self
            .records()
            .into_iter()
            .filter(|record| {
                matches!(
                    record.status,
                    CommandStatus::Accepted { .. }
                        | CommandStatus::AcceptedAwaitingEntity
                        | CommandStatus::Rejected(_)
                )
            })
            .collect();
        let mut count = 0;
        for record in records {
            let Some(expected) = storage
                .get(StorageArea::Ordinary, self.key(&record.command))
                .await
                .map_err(ClientError::from)?
            else {
                continue;
            };
            let bytes = self.bytes(&record)?;
            if expected == bytes {
                continue;
            }
            let stored: Record<StoredOperation<I>> =
                serde_json::from_slice(&expected).map_err(ClientError::from)?;
            if stored.payload.command != record.command
                || stored.payload.input != record.input
                || stored.payload.method != record.method
                || stored.environment_id.as_ref() != Some(&self.environment)
            {
                return Err(OperationError::Identity);
            }
            if !matches!(
                stored.payload.status,
                CommandStatus::Pending | CommandStatus::UnknownDelivery
            ) || self.status(&record.command).as_ref() != Some(&record.status)
            {
                return Err(OperationError::Ambiguous);
            }
            self.persist(storage, &record, Some(expected)).await?;
            count += 1;
        }
        Ok(count)
    }
    /// Remove accepted records only after their own projection has caught up.
    pub async fn prune_confirmed(&self, storage: &impl Storage) -> Result<usize, OperationError> {
        let commands: Vec<_> = {
            let inner = self.inner.borrow();
            inner
                .records
                .iter()
                .filter(|(id, record)| {
                    matches!(
                        record.status,
                        CommandStatus::Accepted { .. } | CommandStatus::AcceptedAwaitingEntity
                    ) && !inner.optimistic.pending.contains_key(*id)
                        && !inner.busy.contains(*id)
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        let conditions = {
            let inner = self.inner.borrow();
            commands
                .iter()
                .map(|id| {
                    Ok(StorageCondition {
                        area: StorageArea::Ordinary,
                        key: self.key(id),
                        expected: Some(self.bytes(&inner.records[id])?),
                    })
                })
                .collect::<Result<Vec<_>, ClientError>>()?
        };
        if !storage
            .transaction(StorageTransaction {
                conditions,
                ordinary: commands
                    .iter()
                    .map(|id| StorageWrite {
                        key: self.key(id),
                        value: None,
                    })
                    .collect(),
                secure: Vec::new(),
            })
            .await
            .map_err(ClientError::from)?
        {
            return Err(OperationError::Ambiguous);
        }
        let mut inner = self.inner.borrow_mut();
        for command in &commands {
            inner.records.remove(command);
        }
        Ok(commands.len())
    }
    pub async fn restore(&self, storage: &impl Storage) -> Result<usize, OperationError> {
        let report = self.restore_available(storage).await?;
        match report.errors.into_iter().next() {
            Some(error) => Err(error),
            None => Ok(report.restored),
        }
    }
    /// Retains unreadable records and restores the independently valid commands.
    pub async fn restore_available(
        &self,
        storage: &impl Storage,
    ) -> Result<RestoreReport, OperationError> {
        let keys = storage
            .list(StorageArea::Ordinary, OUTBOX_NAMESPACE.into())
            .await
            .map_err(ClientError::from)?;
        let mut restored = Vec::new();
        let mut errors = Vec::new();
        for key in keys
            .into_iter()
            .filter(|key| key.environment.as_ref() == Some(&self.environment))
        {
            let result: Result<_, OperationError> = async {
                let Some(bytes) = storage
                    .get(StorageArea::Ordinary, key.clone())
                    .await
                    .map_err(ClientError::from)?
                else {
                    return Ok(None);
                };
                let envelope: Record<StoredOperation<I>> =
                    serde_json::from_slice(&bytes).map_err(ClientError::from)?;
                let record = envelope.payload;
                if envelope.format_version != 1
                    || envelope.payload_version != 1
                    || envelope.kind != OUTBOX_NAMESPACE
                    || envelope.environment_id.as_ref() != Some(&self.environment)
                    || envelope.record_id != key.record
                    || record.command.0 != key.record
                    || record.projection.environment != self.environment
                    || command_identity(&record.input)? != record.command
                {
                    return Err(OperationError::Identity);
                }
                Ok(Some(record))
            }
            .await;
            match result {
                Ok(Some(record)) => restored.push(record),
                Ok(None) => {}
                Err(error) => errors.push(error),
            }
        }
        restored.sort_by(|a, b| (a.ordinal, &a.command).cmp(&(b.ordinal, &b.command)));
        let mut inner = self.inner.borrow_mut();
        let mut count = 0;
        for record in restored {
            inner.next_ordinal = inner.next_ordinal.max(record.ordinal.saturating_add(1));
            if inner.records.contains_key(&record.command) || inner.busy.contains(&record.command) {
                continue;
            }
            if !matches!(record.status, CommandStatus::Rejected(_)) {
                inner.optimistic.insert(
                    record.command.clone(),
                    record.projection.clone(),
                    record.generation,
                    record.change.clone(),
                );
                if let Some(pending) = inner.optimistic.pending.get_mut(&record.command) {
                    pending.status = record.status.clone();
                }
            }
            inner.from_earlier_run.insert(record.command.clone());
            inner.records.insert(record.command.clone(), record);
            count += 1;
        }
        Ok(RestoreReport {
            restored: count,
            errors,
        })
    }
}
struct BusyGuard<I> {
    inner: Rc<RefCell<Operations<I>>>,
    command: CommandId,
}
impl<I> Drop for BusyGuard<I> {
    fn drop(&mut self) {
        let mut inner = self.inner.borrow_mut();
        inner.busy.remove(&self.command);
        if inner
            .records
            .get(&self.command)
            .is_some_and(|record| record.status == CommandStatus::UnknownDelivery)
        {
            inner.optimistic.unknown(&self.command);
        }
    }
}
#[derive(Default)]
struct SerialLane {
    active: bool,
    waiting: VecDeque<Rc<RefCell<SerialWaiter>>>,
}
#[derive(Default)]
struct SerialWaiter {
    ready: bool,
    waker: Option<Waker>,
}
struct SerialPermit {
    lane: Rc<RefCell<SerialLane>>,
    waiter: Rc<RefCell<SerialWaiter>>,
}
impl SerialPermit {
    fn reserve(lane: Rc<RefCell<SerialLane>>) -> Self {
        let waiter = Rc::new(RefCell::new(SerialWaiter::default()));
        {
            let mut state = lane.borrow_mut();
            if state.active {
                state.waiting.push_back(waiter.clone());
            } else {
                state.active = true;
                waiter.borrow_mut().ready = true;
            }
        }
        Self { lane, waiter }
    }
    async fn ready(&self) {
        futures::future::poll_fn(|cx| {
            let mut waiter = self.waiter.borrow_mut();
            if waiter.ready {
                Poll::Ready(())
            } else {
                waiter.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}
impl Drop for SerialPermit {
    fn drop(&mut self) {
        let wake = {
            let mut lane = self.lane.borrow_mut();
            if self.waiter.borrow().ready {
                if let Some(next) = lane.waiting.pop_front() {
                    let mut next = next.borrow_mut();
                    next.ready = true;
                    next.waker.take()
                } else {
                    lane.active = false;
                    None
                }
            } else {
                lane.waiting
                    .retain(|waiter| !Rc::ptr_eq(waiter, &self.waiter));
                None
            }
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}
fn command_identity(input: &Value) -> Result<CommandId, OperationError> {
    let id = input
        .get("commandId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or(OperationError::Identity)?;
    Ok(CommandId(id.into()))
}

pub fn allocate_command_id(
    platform: &impl Platform,
    existing: Option<CommandId>,
) -> Result<CommandId, ClientError> {
    if let Some(id) = existing {
        return Ok(id);
    }
    let mut bytes = platform.random_bytes(16)?;
    if bytes.len() != 16 {
        return Err(ClientError::InvalidPayload("random UUID bytes".into()));
    }
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    Ok(CommandId(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerDispatchMode {
    Auto,
    Queue,
    Steer,
    Restart,
    Start,
}
pub fn resolve_composer_dispatch_mode(
    running: bool,
    alternate_modifier: bool,
    active_default: Option<ComposerDispatchMode>,
    can_steer: bool,
) -> ComposerDispatchMode {
    use ComposerDispatchMode::*;
    if !running {
        return Auto;
    }
    if !can_steer {
        return Queue;
    }
    let default = active_default.unwrap_or(Steer);
    if alternate_modifier {
        if default == Queue { Steer } else { Queue }
    } else {
        default
    }
}
pub fn compaction_blocks_steering(
    compact_command_active: bool,
    compaction_running: bool,
    steers_during_compaction: bool,
    message: Option<(&str, usize)>,
) -> bool {
    if compact_command_active {
        return true;
    }
    if !compaction_running {
        return false;
    }
    if !steers_during_compaction {
        return true;
    }
    message.is_some_and(|(text, attachments)| {
        attachments == 0
            && text
                .split_whitespace()
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case("/compact"))
    })
}
/// Delivery intent stays server-resolved; sending does not fetch a full thread projection.
pub fn message_command(
    mut input: CommandMessageDispatch,
    mode: ComposerDispatchMode,
) -> OrchestrationV2Command {
    use ComposerDispatchMode::*;
    if mode == Start {
        input.title_seed = None;
    } else {
        input.manual_continuation_of_run_id = None;
    }
    input.dispatch_mode = if mode == Queue {
        CommandMessageDispatchDispatchMode::QueueAfterActive(Box::new(
            ThreadForkSourcePointLatestStable {},
        ))
    } else {
        CommandMessageDispatchDispatchMode::StartImmediately(Box::new(
            ThreadForkSourcePointLatestStable {},
        ))
    };
    input.delivery_intent = match mode {
        Auto => Some(Some(CommandMessageDispatchDeliveryIntent::Auto)),
        Steer => Some(Some(CommandMessageDispatchDeliveryIntent::Steer)),
        Restart => Some(Some(CommandMessageDispatchDeliveryIntent::Restart)),
        Queue | Start => None,
    };
    OrchestrationV2Command::MessageDispatch(Box::new(input))
}

pub fn thread_title_text_from_prompt(text: &str) -> String {
    let expanded = title_citations(text);
    let text = expanded.as_str();
    let mut prose = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut images = BTreeSet::new();
    for reference in crate::view_logic::context_references::collect_references(text) {
        prose.push_str(&text[cursor..reference.start]);
        if reference.kind == "image" {
            images.insert(reference.context_id);
        } else {
            let source = &text[reference.start..reference.end];
            if let Some((_, label)) = source.split_once('[')
                && let Some((label, _)) = label.split_once("](t3-context:")
            {
                prose.push_str(label);
            }
        }
        cursor = reference.end;
    }
    prose.push_str(&text[cursor..]);
    let mut normalized = String::with_capacity(prose.len());
    cursor = 0;
    while let Some(relative) = prose[cursor..].find("model:") {
        let start = cursor + relative;
        normalized.push_str(&prose[cursor..start]);
        let prefix_ok = prose[..start]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || ",([\"'".contains(c));
        let rest = &prose[start + 6..];
        let token_end = rest
            .find(|c: char| c.is_whitespace() || ",\"'".contains(c))
            .unwrap_or(rest.len());
        let token = &rest[..token_end];
        if prefix_ok
            && let Some((instance, model)) = token.split_once('/')
            && instance
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            && instance
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            && !model.is_empty()
        {
            let mut end = model.len();
            while let Some(last) = model[..end].chars().next_back() {
                let unbalanced = match last {
                    ')' => model.matches(')').count() > model.matches('(').count(),
                    ']' => model.matches(']').count() > model.matches('[').count(),
                    _ => false,
                };
                if ".;:!?".contains(last) || unbalanced {
                    end -= last.len_utf8();
                } else {
                    break;
                }
            }
            if end > 0 {
                normalized.push_str(&model[..end]);
                normalized.push_str(&model[end..]);
            } else {
                normalized.push_str(&prose[start..start + 6 + token_end]);
            }
            cursor = start + 6 + token_end;
        } else {
            normalized.push_str("model:");
            cursor = start + 6;
        }
    }
    normalized.push_str(&prose[cursor..]);
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if !normalized.is_empty() {
        normalized
    } else {
        match images.len() {
            0 => String::new(),
            1 => "Attached image".into(),
            _ => "Attached images".into(),
        }
    }
}
fn title_citations(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let prefix = "[Assistant quote](t3-citation://v1/";
    while let Some(relative) = text[cursor..].find(prefix) {
        let start = cursor + relative;
        output.push_str(&text[cursor..start]);
        let href_start = start + "[Assistant quote](".len();
        let Some(relative_end) = text[href_start..].find(')') else {
            output.push_str(&text[start..]);
            return output;
        };
        let end = href_start + relative_end;
        let citation = title_citation(&text[href_start..end]);
        if let Some(citation) = citation {
            output.push_str(&citation.text);
            if let Some(comment) = citation.comment.flatten() {
                output.push_str("\nComment: ");
                output.push_str(&comment);
            }
        } else {
            output.push_str(&text[start..end + 1]);
        }
        cursor = end + 1;
    }
    output.push_str(&text[cursor..]);
    output
}
fn title_citation(href: &str) -> Option<t3_contracts::assistant_citations::AssistantCitation> {
    use t3_contracts::assistant_citations::*;
    let max = 9
        * (ASSISTANT_CITATION_MAX_TEXT_LENGTH
            + ASSISTANT_CITATION_MAX_COMMENT_LENGTH
            + 3 * ASSISTANT_CITATION_MAX_ID_LENGTH
            + 2 * ASSISTANT_CITATION_CONTEXT_LENGTH)
        + 1000;
    if href.len() > max || href.chars().any(char::is_whitespace) {
        return None;
    }
    let url = url::Url::parse(href).ok()?;
    if url.scheme() != "t3-citation"
        || url.host_str() != Some("v1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path().strip_prefix('/')?.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let decode = |value: &str| {
        url::form_urlencoded::parse(format!("v={}", value.replace('+', "%2B")).as_bytes())
            .next()
            .map(|(_, value)| value.into_owned())
    };
    let environment = decode(parts[0])?;
    let thread = decode(parts[1])?;
    let message = decode(parts[2])?;
    if [&environment, &thread, &message]
        .into_iter()
        .any(|id| id.is_empty() || id.encode_utf16().count() > ASSISTANT_CITATION_MAX_ID_LENGTH)
    {
        return None;
    }
    let mut pairs = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if pairs.insert(key.into_owned(), value.into_owned()).is_some() {
            return None;
        }
    }
    let expected = 5 + usize::from(pairs.contains_key("comment"));
    if pairs.len() != expected {
        return None;
    }
    let start = pairs.get("start")?;
    let end = pairs.get("end")?;
    if [start, end]
        .iter()
        .any(|n| n.is_empty() || n.len() > 16 || !n.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let start: u64 = start.parse().ok()?;
    let end: u64 = end.parse().ok()?;
    let text = pairs.get("text")?;
    let prefix = pairs.get("prefix")?;
    let suffix = pairs.get("suffix")?;
    if end <= start
        || end > 9_007_199_254_740_991
        || text.trim().is_empty()
        || text.encode_utf16().count() > ASSISTANT_CITATION_MAX_TEXT_LENGTH
        || prefix.encode_utf16().count() > ASSISTANT_CITATION_CONTEXT_LENGTH
        || suffix.encode_utf16().count() > ASSISTANT_CITATION_CONTEXT_LENGTH
        || pairs.get("comment").is_some_and(|comment| {
            comment.encode_utf16().count() > ASSISTANT_CITATION_MAX_COMMENT_LENGTH
        })
    {
        return None;
    }
    Some(AssistantCitation {
        version: 1,
        environment_id: EnvironmentId(environment),
        thread_id: crate::types::ThreadId(thread),
        message_id: t3_contracts::base_schemas::MessageId(message),
        text: text.clone(),
        comment: pairs.get("comment").cloned().map(Some),
        start,
        end,
        prefix: prefix.clone(),
        suffix: suffix.clone(),
    })
}
pub fn derive_thread_title_seed(
    text: &str,
    attachments: &[t3_contracts::chat_attachment::ChatAttachment],
    fallback_labels: &[&str],
) -> String {
    use t3_contracts::chat_attachment::ChatAttachment;
    let title = thread_title_text_from_prompt(text);
    if !title.is_empty() {
        return truncate_title(&title);
    }
    let images = attachments
        .iter()
        .filter(|a| matches!(a, ChatAttachment::Image(_)))
        .count();
    if images > 0 {
        return if images == 1 {
            "Attached image".into()
        } else {
            "Attached images".into()
        };
    }
    if let Some(attachment) = attachments.first() {
        let name = match attachment {
            ChatAttachment::Image(a) => &a.name,
            ChatAttachment::File(a) => &a.name,
            ChatAttachment::Unknown(a) => &a.name,
        };
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if !name.is_empty() {
            return truncate_title(&format!("File: {name}"));
        }
    }
    fallback_labels
        .iter()
        .map(|label| label.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|label| !label.is_empty())
        .map(|label| truncate_title(&label))
        .unwrap_or_else(|| "New thread".into())
}
fn truncate_title(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 50 {
        trimmed.into()
    } else {
        format!("{}...", trimmed.chars().take(50).collect::<String>())
    }
}

pub fn get_clone_directory_name(input: &str) -> String {
    let input = input.split(['?', '#']).next().unwrap_or_default().trim();
    let scheme = input.find("://");
    let scp = input
        .split_once(':')
        .is_some_and(|(host, _)| host.contains('@') && !host.contains(['/', '\\']));
    let has_host = scheme.is_some() || scp;
    let path = scheme.map_or(input, |index| &input[index + 3..]);
    let segments: Vec<_> = path
        .split(['/', '\\', ':'])
        .filter(|p| !p.trim().is_empty())
        .collect();
    let last = segments.last().copied().unwrap_or_default().trim();
    if has_host
        && (segments.len() < 2 || (segments.len() == 2 && last.bytes().all(|c| c.is_ascii_digit())))
    {
        return String::new();
    }
    last.strip_suffix(".git").unwrap_or(last).into()
}
pub fn get_clone_directory_path(remote: &str, ghq: bool, repository: Option<&str>) -> String {
    if ghq && let Some(path) = clone_repository_path(remote) {
        return path;
    }
    get_clone_directory_name(repository.unwrap_or(remote))
}
fn clone_repository_path(remote: &str) -> Option<String> {
    let remote = remote.trim();
    if crate::workspace::projects::is_windows_drive_path(remote) {
        return None;
    }
    let (host, path) = if let Some((scheme, rest)) = remote.split_once("://") {
        if !matches!(scheme, "http" | "https" | "ssh") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let url = url::Url::parse(&format!("{scheme}://{authority}/")).ok()?;
        (url.host_str()?.to_owned(), path)
    } else {
        let (host, path) = remote.split_once(':')?;
        if path.starts_with('/') {
            return None;
        }
        (host.rsplit('@').next()?.to_owned(), path)
    };
    let path = path.split(['?', '#']).next()?.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<_> = std::iter::once(host.as_str())
        .chain(path.split('/').filter(|s| !s.is_empty()))
        .collect();
    if segments.len() < 2 || segments.iter().any(|segment| unsafe_clone_segment(segment)) {
        return None;
    }
    Some(segments.join("/"))
}
fn unsafe_clone_segment(segment: &str) -> bool {
    if segment.is_empty()
        || matches!(segment, "." | "..")
        || segment.ends_with([' ', '.'])
        || segment
            .chars()
            .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
    {
        return true;
    }
    let name = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_lowercase();
    matches!(
        name.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$"
    ) || ["com", "lpt"].iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|suffix| {
            suffix.chars().count() == 1
                && suffix
                    .chars()
                    .all(|c| c.is_ascii_digit() || "¹²³".contains(c))
        })
    })
}
pub fn get_clone_destination_path(directory: &str, name: Option<&str>) -> String {
    let name = name.unwrap_or_default().trim();
    if name.is_empty() {
        return directory.into();
    }
    let windows = crate::workspace::projects::is_windows_drive_path(directory)
        || directory.starts_with("\\\\")
        || (!directory.starts_with('/') && directory.contains('\\'));
    let separator = if windows { '\\' } else { '/' };
    let name = if windows {
        name.replace('/', "\\")
    } else {
        name.into()
    };
    let base = directory.trim();
    if base.is_empty() || base.ends_with(separator) {
        format!("{base}{name}")
    } else {
        format!("{base}{separator}{name}")
    }
}
pub fn github_incomplete_owner_repository_message(
    source: &str,
    input: &str,
) -> Option<&'static str> {
    let input = input.trim();
    let owner = input.trim_end_matches('/');
    (source == "github"
        && owner.len() < input.len()
        && !owner.is_empty()
        && owner.len() <= 39
        && owner.as_bytes()[0].is_ascii_alphanumeric()
        && owner
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)))
    .then_some("Enter the full owner/repository name.")
}
pub fn normalize_pasted_clone_url(input: &str) -> String {
    let input = input.trim();
    if let Some((owner, repo)) = input.split_once('/')
        && !owner.is_empty()
        && owner.len() <= 39
        && owner.as_bytes()[0].is_ascii_alphanumeric()
        && owner
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        && !repo.is_empty()
        && repo
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return format!(
            "https://github.com/{owner}/{repo}{}",
            if repo.ends_with(".git") { "" } else { ".git" }
        );
    }
    if let Ok(mut url) = url::Url::parse(input)
        && matches!(url.scheme(), "http" | "https")
    {
        let path = url.path();
        let end = if url.host_str() == Some("github.com") || url.host_str() == Some("git.cafe") {
            let parts: Vec<_> = path.split('/').collect();
            (parts.len() >= 5 && matches!(parts[3], "tree" | "blob" | "pull" | "issues" | "commit"))
                .then(|| 1 + parts[1].len() + 1 + parts[2].len())
        } else if url.host_str() == Some("bitbucket.org") {
            path.find("/src/")
        } else {
            path.find("/-/")
        };
        if let Some(end) = end {
            let path = path[..end].to_owned();
            url.set_path(&path);
            url.set_query(None);
            url.set_fragment(None);
            return url.into();
        }
    }
    input.into()
}

pub fn metadata_commands(
    metadata: CommandThreadMetadataUpdate,
    selection: Option<t3_contracts::model_selection::ModelSelection>,
) -> Vec<OrchestrationV2Command> {
    let mut commands = Vec::new();
    let has_metadata = metadata.title.is_some()
        || metadata.branch.is_some()
        || metadata.worktree_path.is_some()
        || metadata.regenerate_title.is_some()
        || metadata.linked_pull_request.is_some()
        || metadata.limit_recovery.is_some();
    let id = metadata.command_id.clone();
    let thread = metadata.thread_id.clone();
    if has_metadata {
        commands.push(OrchestrationV2Command::ThreadMetadataUpdate(Box::new(
            metadata,
        )));
    }
    if let Some(model_selection) = selection {
        commands.push(OrchestrationV2Command::ThreadModelSelectionSet(Box::new(
            CommandThreadModelSelectionSet {
                command_id: if commands.is_empty() {
                    id
                } else {
                    CommandId(format!("{}:model-selection", id.0))
                },
                thread_id: thread,
                model_selection,
            },
        )));
    }
    commands
}

pub fn can_create_project_in_environment(phase: Option<crate::connection::Phase>) -> bool {
    phase == Some(crate::connection::Phase::Connected)
}
pub fn available_scratch_workspace_root(
    phase: Option<crate::connection::Phase>,
    config: Option<&t3_contracts::server::ServerConfig>,
) -> Option<&str> {
    if can_create_project_in_environment(phase) {
        config.and_then(|config| {
            config
                .scratch_workspace_root
                .as_ref()
                .map(|value| value.0.as_str())
        })
    } else {
        None
    }
}
fn absolute_path_parts(path: &str) -> Option<(String, char, Vec<&str>)> {
    let windows = crate::workspace::projects::is_windows_drive_path(path);
    if windows {
        Some((
            format!("{}\\", &path[..2]),
            '\\',
            path.get(3..)
                .unwrap_or_default()
                .split(['/', '\\'])
                .filter(|s| !s.is_empty())
                .collect(),
        ))
    } else if path.starts_with("\\\\") {
        let segments: Vec<_> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
        if segments.len() < 2 {
            None
        } else {
            Some((
                format!("\\\\{}\\{}\\", segments[0], segments[1]),
                '\\',
                segments[2..].to_vec(),
            ))
        }
    } else {
        path.strip_prefix('/').map(|rest| {
            (
                "/".into(),
                '/',
                rest.split('/').filter(|s| !s.is_empty()).collect(),
            )
        })
    }
}
pub use crate::workspace::project_ops::paths::infer_project_title_from_path;
pub fn get_clone_destination_for_picked_folder(
    picked_path: &str,
    clone_name: &str,
    case_sensitive: bool,
) -> String {
    let normalized = crate::workspace::projects::normalize_project_path_for_dispatch(picked_path);
    let leaf = infer_project_title_from_path(&normalized);
    let matches = if case_sensitive {
        leaf == clone_name
    } else {
        leaf.to_lowercase() == clone_name.to_lowercase()
    };
    if matches {
        let windows = crate::workspace::projects::is_windows_drive_path(&normalized)
            || normalized.starts_with("\\\\");
        let separator = if windows { '\\' } else { '/' };
        if normalized.ends_with(separator) {
            normalized
        } else {
            format!("{normalized}{separator}")
        }
    } else {
        get_clone_destination_path(&normalized, Some(clone_name))
    }
}
pub fn resolve_add_project_path(
    raw: &str,
    current_cwd: Option<&str>,
    platform: &str,
) -> Result<String, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Enter a project path.");
    }
    let windows = crate::workspace::projects::is_windows_drive_path(raw) || raw.starts_with("\\\\");
    if windows && !platform.to_lowercase().starts_with("win") {
        return Err("Windows-style paths are only supported on Windows environments.");
    }
    let relative = ["./", "../", ".\\", "..\\"]
        .iter()
        .any(|prefix| raw.starts_with(prefix))
        || matches!(raw, "." | "..");
    if relative {
        let cwd = current_cwd
            .filter(|cwd| !cwd.is_empty())
            .ok_or("Relative paths require an active project in this environment.")?;
        if let Some((root, separator, mut segments)) = absolute_path_parts(cwd) {
            for segment in raw.split(['/', '\\']) {
                match segment {
                    "" | "." => {}
                    ".." => {
                        segments.pop();
                    }
                    _ => segments.push(segment),
                }
            }
            return Ok(
                crate::workspace::projects::normalize_project_path_for_dispatch(&format!(
                    "{root}{}",
                    segments.join(&separator.to_string())
                )),
            );
        }
    }
    Ok(crate::workspace::projects::normalize_project_path_for_dispatch(raw))
}
pub fn find_existing_add_project<'a>(
    projects: &'a [crate::workspace::projects::EnvironmentProject],
    environment: &EnvironmentId,
    path: &str,
) -> Option<&'a crate::workspace::projects::EnvironmentProject> {
    let normalized = crate::workspace::projects::normalize_project_path_for_comparison(path);
    if normalized.is_empty() {
        return None;
    }
    projects.iter().find(|project| {
        &project.environment == environment
            && crate::workspace::projects::normalize_project_path_for_comparison(
                &project.project.workspace_root.0,
            ) == normalized
    })
}
pub fn build_project_create_command(
    command_id: CommandId,
    project_id: crate::types::ProjectId,
    workspace_root: String,
) -> t3_contracts::project::ProjectMutation {
    use t3_contracts::{base_schemas::TrimmedNonEmptyString, project::*};
    ProjectMutation::Create(ProjectMutationMember1 {
        r#type: ProjectMutationMember1Type::ProjectCreate,
        command_id,
        project_id,
        title: TrimmedNonEmptyString(infer_project_title_from_path(&workspace_root)),
        workspace_root: TrimmedNonEmptyString(workspace_root),
        create_workspace_root_if_missing: Some(Some(true)),
        default_model_selection: Some(None),
        scripts: None,
    })
}

#[derive(Debug, Clone)]
pub enum SendAttachment {
    Stored(t3_contracts::chat_attachment::ChatAttachment),
    Upload(Box<t3_contracts::chat_attachment::UploadChatAttachment>),
}
impl SendAttachment {
    fn id(&self) -> Option<&str> {
        match self {
            Self::Stored(attachment) => Some(attachment_id(attachment)),
            Self::Upload(attachment) => attachment.id.as_ref().and_then(|id| id.as_deref()),
        }
    }
}
pub(crate) fn attachment_id(attachment: &t3_contracts::chat_attachment::ChatAttachment) -> &str {
    use t3_contracts::chat_attachment::ChatAttachment::*;
    match attachment {
        Image(a) => &a.id,
        File(a) => &a.id,
        Unknown(a) => &a.id,
    }
}
pub async fn persist_attachments(
    endpoint: &impl RpcEndpoint,
    thread_id: crate::types::ThreadId,
    message_id: t3_contracts::base_schemas::MessageId,
    attachments: &[SendAttachment],
) -> Result<Vec<t3_contracts::chat_attachment::ChatAttachment>, ClientError> {
    let uploads: Vec<_> = attachments
        .iter()
        .filter_map(|attachment| match attachment {
            SendAttachment::Upload(value) => Some(value.as_ref().clone()),
            _ => None,
        })
        .collect();
    if uploads.is_empty() {
        return Ok(attachments
            .iter()
            .filter_map(|attachment| match attachment {
                SendAttachment::Stored(value) => Some(value.clone()),
                _ => None,
            })
            .collect());
    }
    let reply = endpoint
        .call::<t3_contracts::rpc::WsAssetsPersistChatAttachmentsRpc>(
            t3_contracts::chat_attachment::PersistChatAttachmentsInput {
                thread_id,
                message_id,
                attachments: uploads,
            },
        )
        .await?;
    if attachments
        .iter()
        .all(|attachment| matches!(attachment, SendAttachment::Upload(_)))
    {
        return Ok(reply.attachments);
    }
    let mut persisted = reply.attachments.into_iter();
    Ok(attachments
        .iter()
        .filter_map(|attachment| match attachment {
            SendAttachment::Stored(value) => Some(value.clone()),
            SendAttachment::Upload(_) => persisted.next(),
        })
        .collect())
}
pub fn remap_context_attachments(
    context: &mut t3_contracts::composer_context::OrchestrationMessageContext,
    before: &[SendAttachment],
    after: &[t3_contracts::chat_attachment::ChatAttachment],
) {
    use t3_contracts::composer_context::ComposerContextRecord;
    let ids: BTreeMap<_, _> = before
        .iter()
        .zip(after)
        .filter_map(|(old, new)| old.id().map(|id| (id, attachment_id(new))))
        .collect();
    for record in &mut context.records {
        let id = match record {
            ComposerContextRecord::Image(record) => &mut record.attachment_id,
            ComposerContextRecord::File(record) => &mut record.attachment_id,
            _ => continue,
        };
        if let Some(next) = ids.get(id.0.as_str()) {
            id.0 = (*next).into();
        }
    }
}
pub fn checkpoint_rollback_command(
    command_id: CommandId,
    projection: &OrchestrationV2ThreadProjection,
    checkpoint_id: Option<&t3_contracts::base_schemas::CheckpointId>,
    scope_id: Option<&t3_contracts::base_schemas::CheckpointScopeId>,
    turn_count: Option<u64>,
    restore_files: Option<bool>,
) -> Result<OrchestrationV2Command, ClientError> {
    use t3_contracts::orchestration_v2_id_table::IdOrIndex;
    let IdOrIndex::Id(thread_id) = &projection.thread.id else {
        return Err(ClientError::InvalidPayload(
            "unresolved thread identity".into(),
        ));
    };
    let (checkpoint_id, scope_id) = if let (Some(checkpoint), Some(scope)) =
        (checkpoint_id, scope_id)
    {
        (checkpoint.clone(), scope.clone())
    } else {
        let checkpoint = projection.checkpoints.iter().find(|checkpoint| matches!((&checkpoint.id, &checkpoint.scope_id, checkpoint_id, scope_id), (IdOrIndex::Id(id), IdOrIndex::Id(scope), Some(expected), Some(expected_scope)) if id == expected && scope == expected_scope))
            .or_else(|| projection.checkpoints.iter().rfind(|checkpoint| if turn_count == Some(0) { checkpoint.ordinal_within_scope == 0 && checkpoint.app_run_ordinal.is_none() } else { turn_count.is_some() && checkpoint.app_run_ordinal == turn_count }));
        let checkpoint = checkpoint
            .filter(|checkpoint| {
                checkpoint.status == CheckpointStatus::Ready
                    || (restore_files == Some(false)
                        && checkpoint.status == CheckpointStatus::Missing)
            })
            .ok_or_else(|| ClientError::InvalidPayload("checkpoint unavailable".into()))?;
        let (IdOrIndex::Id(id), IdOrIndex::Id(scope)) = (&checkpoint.id, &checkpoint.scope_id)
        else {
            return Err(ClientError::InvalidPayload(
                "unresolved checkpoint identity".into(),
            ));
        };
        (id.clone(), scope.clone())
    };
    Ok(OrchestrationV2Command::CheckpointRollback(Box::new(
        CommandCheckpointRollback {
            command_id,
            thread_id: thread_id.clone(),
            checkpoint_id,
            scope_id,
            restore_files: restore_files.map(Some),
        },
    )))
}
pub fn detach_provider_sessions(
    command: &CommandId,
    projection: &OrchestrationV2ThreadProjection,
) -> Result<Vec<OrchestrationV2Command>, ClientError> {
    use t3_contracts::orchestration_v2_id_table::IdOrIndex;
    let IdOrIndex::Id(thread_id) = &projection.thread.id else {
        return Err(ClientError::InvalidPayload(
            "unresolved thread identity".into(),
        ));
    };
    projection
        .provider_sessions
        .iter()
        .map(|session| {
            let IdOrIndex::Id(id) = &session.id else {
                return Err(ClientError::InvalidPayload(
                    "unresolved provider session identity".into(),
                ));
            };
            Ok(OrchestrationV2Command::ProviderSessionDetach(Box::new(
                CommandProviderSessionDetach {
                    command_id: CommandId(format!("{}:detach:{}", command.0, id.0)),
                    thread_id: thread_id.clone(),
                    provider_session_id: id.clone(),
                    reason: Some(Some("client-requested".into())),
                },
            )))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct InterruptInput {
    pub command_id: CommandId,
    pub thread_id: crate::types::ThreadId,
    pub run_id: Option<t3_contracts::base_schemas::RunId>,
    pub subagent_id: Option<t3_contracts::base_schemas::NodeId>,
    pub legacy_turn_id: Option<t3_contracts::base_schemas::RunId>,
}
#[derive(Debug, Clone)]
pub struct PlannedCommand {
    pub command: OrchestrationV2Command,
    pub best_effort: bool,
}
#[derive(Debug, Clone)]
pub enum InterruptPlan {
    Ready(Vec<PlannedCommand>),
    NeedProjection(crate::types::ThreadId),
}
/// The caller loads requested projections, then persists these commands through OperationQueue.
pub fn interrupt_plan(
    input: &InterruptInput,
    supports_thread_stop: bool,
    projections: &BTreeMap<crate::types::ThreadId, Rc<OrchestrationV2ThreadProjection>>,
) -> Result<InterruptPlan, ClientError> {
    use t3_contracts::{
        orchestration_v2_id_table::IdOrIndex, thread_pull_request::ThreadPullRequestLinkSource,
    };
    let mut commands = Vec::new();
    let mut thread_id = input.thread_id.clone();
    if let Some(subagent_id) = &input.subagent_id {
        let Some(projection) = projections.get(&thread_id) else {
            return Ok(InterruptPlan::NeedProjection(thread_id));
        };
        let subagent = projection
            .subagents
            .iter()
            .find(|subagent| subagent.id == IdOrIndex::Id(subagent_id.clone()));
        if let Some(subagent) = subagent
            && subagent.origin == SubagentOrigin::AppOwned
            && subagent.child_thread_id.is_some()
        {
            if let Some(IdOrIndex::Id(run_id)) = &subagent.run_id {
                commands.push(PlannedCommand {
                    best_effort: true,
                    command: OrchestrationV2Command::SubagentStop(Box::new(CommandSubagentStop {
                        command_id: CommandId(format!("{}:subagent-stop", input.command_id.0)),
                        thread_id: thread_id.clone(),
                        run_id: run_id.clone(),
                        subagent_id: subagent_id.clone(),
                    })),
                });
            }
            let Some(IdOrIndex::Id(child)) = &subagent.child_thread_id else {
                return Err(ClientError::InvalidPayload(
                    "unresolved child identity".into(),
                ));
            };
            thread_id = child.clone();
        } else {
            let Some(IdOrIndex::Id(run_id)) =
                subagent.and_then(|subagent| subagent.run_id.as_ref())
            else {
                return Ok(InterruptPlan::Ready(commands));
            };
            commands.push(PlannedCommand {
                best_effort: false,
                command: OrchestrationV2Command::SubagentStop(Box::new(CommandSubagentStop {
                    command_id: input.command_id.clone(),
                    thread_id,
                    run_id: run_id.clone(),
                    subagent_id: subagent_id.clone(),
                })),
            });
            return Ok(InterruptPlan::Ready(commands));
        }
    }
    let mut run_id = if thread_id == input.thread_id {
        input
            .run_id
            .clone()
            .or_else(|| input.legacy_turn_id.clone())
    } else {
        None
    };
    if run_id.is_none() && supports_thread_stop {
        commands.push(PlannedCommand {
            best_effort: false,
            command: OrchestrationV2Command::ThreadStop(Box::new(CommandThreadStop {
                command_id: input.command_id.clone(),
                thread_id,
                reason: None,
                stop_delegated_tasks: None,
            })),
        });
        return Ok(InterruptPlan::Ready(commands));
    }
    if run_id.is_none() {
        let Some(projection) = projections.get(&thread_id) else {
            return Ok(InterruptPlan::NeedProjection(thread_id));
        };
        let run = projection
            .runs
            .iter()
            .rfind(|run| {
                matches!(
                    run.status,
                    OrchestrationV2RunStatus::Preparing
                        | OrchestrationV2RunStatus::Starting
                        | OrchestrationV2RunStatus::Running
                        | OrchestrationV2RunStatus::Waiting
                )
            })
            .or_else(|| {
                projection
                    .runs
                    .last()
                    .filter(|_| has_pending_background_work(projection))
            });
        if let Some(run) = run {
            let IdOrIndex::Id(id) = &run.id else {
                return Err(ClientError::InvalidPayload(
                    "unresolved run identity".into(),
                ));
            };
            run_id = Some(id.clone());
        }
        if run_id.is_none() {
            for link in projection
                .thread
                .pull_requests
                .iter()
                .flatten()
                .flatten()
                .filter(|link| {
                    link.source != ThreadPullRequestLinkSource::StackDismissed
                        && link.watch.is_some()
                })
            {
                let command_id = if commands.is_empty() {
                    input.command_id.clone()
                } else {
                    CommandId(format!("{}:watch:{}", input.command_id.0, commands.len()))
                };
                commands.push(PlannedCommand {
                    best_effort: false,
                    command: OrchestrationV2Command::ThreadPullRequestWatch(Box::new(
                        CommandThreadPullRequestWatch {
                            command_id,
                            thread_id: thread_id.clone(),
                            host: link.host.clone(),
                            repository: link.repository.clone(),
                            number: link.number,
                            watching: false,
                            link: None,
                        },
                    )),
                });
            }
        }
    }
    if let Some(run_id) = run_id {
        commands.push(PlannedCommand {
            best_effort: false,
            command: OrchestrationV2Command::RunInterrupt(Box::new(CommandRunInterrupt {
                command_id: input.command_id.clone(),
                thread_id,
                run_id,
                reason: None,
                hold_queue: Some(Some(true)),
            })),
        });
    }
    Ok(InterruptPlan::Ready(commands))
}
fn has_pending_background_work(projection: &OrchestrationV2ThreadProjection) -> bool {
    let Some(latest) = projection.runs.last() else {
        return false;
    };
    if !matches!(
        latest.status,
        OrchestrationV2RunStatus::Cancelled
            | OrchestrationV2RunStatus::Completed
            | OrchestrationV2RunStatus::Failed
            | OrchestrationV2RunStatus::Interrupted
            | OrchestrationV2RunStatus::Waiting
    ) {
        return false;
    }
    let provider_work = projection
        .provider_threads
        .iter()
        .filter(|thread| {
            projection
                .thread
                .active_provider_thread_id
                .as_ref()
                .is_none_or(|active| active == &thread.id)
        })
        .any(|thread| {
            thread
                .pending_background_tasks
                .iter()
                .flatten()
                .flatten()
                .any(|task| match task {
                    OrchestrationV2PendingBackgroundTask::Subagent(task) => {
                        !task.task_id.0.is_empty()
                    }
                    OrchestrationV2PendingBackgroundTask::Command(task)
                    | OrchestrationV2PendingBackgroundTask::Monitor(task)
                    | OrchestrationV2PendingBackgroundTask::BackgroundTask(task) => {
                        !task.task_id.0.is_empty()
                    }
                })
        });
    provider_work || !crate::state::items::pending_background(projection).is_empty()
}

pub fn launch_workspace_strategy(
    branch: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
    worktree_path: Option<t3_contracts::base_schemas::TrimmedNonEmptyString>,
    prepare: Option<ThreadLaunchWorkspaceStrategyWorktree>,
) -> OrchestrationV2ThreadLaunchWorkspaceStrategy {
    if let Some(prepare) = prepare {
        return OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(Box::new(prepare));
    }
    if let Some(worktree_path) = worktree_path.filter(|path| !path.0.is_empty()) {
        return OrchestrationV2ThreadLaunchWorkspaceStrategy::ExistingWorktree(Box::new(
            ThreadLaunchWorkspaceStrategyExistingWorktree {
                worktree_path,
                branch: branch.map(Some),
            },
        ));
    }
    OrchestrationV2ThreadLaunchWorkspaceStrategy::Root(Box::new(
        ThreadLaunchWorkspaceStrategyRoot {
            branch: branch.map(Some),
        },
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSourceReadiness {
    pub ready: bool,
    pub hint: Option<String>,
}
pub fn add_project_remote_source_readiness(
    discovery: Option<&t3_contracts::source_control::SourceControlDiscoveryResult>,
    source: &str,
) -> RemoteSourceReadiness {
    use t3_contracts::source_control::*;
    if source == "url" {
        return RemoteSourceReadiness {
            ready: true,
            hint: None,
        };
    }
    let provider = discovery.and_then(|discovery| {
        discovery
            .source_control_providers
            .iter()
            .find(|provider| provider.kind.0 == source)
    });
    let Some(provider) = provider else {
        return RemoteSourceReadiness {
            ready: false,
            hint: Some(
                "Provider status unavailable. Open Source Control settings and rescan.".into(),
            ),
        };
    };
    if provider.status != SourceControlDiscoveryStatus::Available {
        return RemoteSourceReadiness {
            ready: false,
            hint: Some(provider.install_hint.0.clone()),
        };
    }
    if provider.auth.status == SourceControlProviderAuthStatus::Unauthenticated {
        let hint = match &provider.auth.detail {
            WireOption::Some { value } => value.0.clone(),
            WireOption::None => format!(
                "{} is not authenticated. Open Source Control settings for setup guidance.",
                provider.label.0
            ),
        };
        return RemoteSourceReadiness {
            ready: false,
            hint: Some(hint),
        };
    }
    RemoteSourceReadiness {
        ready: true,
        hint: None,
    }
}
pub fn is_source_control_provider_offered(
    offered: Option<&BTreeMap<String, bool>>,
    kind: &str,
    default_kind: &str,
) -> bool {
    offered
        .and_then(|offered| offered.get(kind))
        .copied()
        .unwrap_or(kind == default_kind)
}
/// Definitions and their order come from the source-control registry, not client copies.
pub fn sort_add_project_provider_sources<'a>(
    definitions: &'a [(&'a str, &'a str)],
    default_kind: &str,
    offered: Option<&BTreeMap<String, bool>>,
    readiness: impl Fn(&str) -> bool,
) -> Vec<&'a str> {
    let mut sources: Vec<_> = definitions
        .iter()
        .filter(|(kind, _)| is_source_control_provider_offered(offered, kind, default_kind))
        .map(|(kind, label)| (*kind, *label, readiness(kind)))
        .collect();
    sources.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(b.1)).then(a.0.cmp(b.0)));
    sources.into_iter().map(|(kind, _, _)| kind).collect()
}
pub fn get_default_clone_url(
    repository: &t3_contracts::source_control::SourceControlRepositoryInfo,
    prefer_https: bool,
) -> &str {
    if prefer_https {
        &repository.url.0
    } else {
        &repository.ssh_url.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        platform::{HostFuture, PlatformError},
        rpc::RpcMethod as ClientMethod,
        testing::FakePlatform,
    };
    use futures::{
        FutureExt, StreamExt, executor::block_on, future::LocalBoxFuture, stream::LocalBoxStream,
    };
    use serde_json::json;
    use std::cell::Cell;
    #[derive(Clone, Default)]
    struct AtomicStorage {
        host: FakePlatform,
        fail: Rc<Cell<bool>>,
    }
    impl Storage for AtomicStorage {
        fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
            self.host.get(area, key)
        }
        fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
            self.host.write(area, writes)
        }
        fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
            self.host.list(area, namespace)
        }
        fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
            async move {
                if self.fail.replace(false) {
                    return Err(PlatformError::Storage("injected failure".into()));
                }
                for condition in transaction.conditions {
                    if self.host.get(condition.area, condition.key).await? != condition.expected {
                        return Ok(false);
                    }
                }
                self.host
                    .write(StorageArea::Ordinary, transaction.ordinary)
                    .await?;
                self.host
                    .write(StorageArea::Secure, transaction.secure)
                    .await?;
                Ok(true)
            }
            .boxed_local()
        }
    }
    #[derive(Clone, Copy)]
    enum Reply {
        Accepted(u64),
        Rejected,
        Unknown,
        Pending,
    }
    struct Endpoint {
        reply: Reply,
        calls: Cell<usize>,
        on_call: Option<Box<dyn Fn()>>,
        command: &'static str,
    }
    impl Endpoint {
        fn new(reply: Reply) -> Self {
            Self::for_command(reply, "c")
        }
        fn for_command(reply: Reply, command: &'static str) -> Self {
            Self {
                reply,
                calls: Cell::new(0),
                on_call: None,
                command,
            }
        }
    }
    impl RpcEndpoint for Endpoint {
        fn call<M: ClientMethod>(
            &self,
            _input: M::Input,
        ) -> LocalBoxFuture<'_, Result<M::Output, ClientError>> {
            async { Err(ClientError::Unsupported("use command outcome")) }.boxed_local()
        }
        fn subscribe<M: ClientMethod>(
            &self,
            _input: M::Input,
        ) -> LocalBoxStream<'static, Result<M::Output, ClientError>> {
            futures::stream::iter(Vec::<Value>::new())
                .map(|_| Err(ClientError::Unsupported("test subscription")))
                .boxed_local()
        }
        fn command<M: ClientMethod>(
            &self,
            input: M::Input,
        ) -> LocalBoxFuture<'_, Result<M::Output, RpcCallError>>
        where
            M::Output: 'static,
        {
            self.calls.set(self.calls.get() + 1);
            if let Some(callback) = &self.on_call {
                callback();
            }
            assert_eq!(serde_json::to_value(input).unwrap()["commandId"], self.command);
            async move {
                match self.reply {
                    Reply::Accepted(sequence) => {
                        serde_json::from_value(json!({"sequence":sequence}))
                            .map_err(|_| RpcCallError::InvalidResponse)
                    }
                    Reply::Rejected => {
                        Err(RpcCallError::Rejected(vec![json!({"_tag":"OrchestrationV2DispatchCommandError","message":"Failed to dispatch orchestration V2 command","detail":"service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex."})]))
                    }
                    Reply::Unknown => Err(RpcCallError::Unknown),
                    Reply::Pending => futures::future::pending().await,
                }
            }
            .boxed_local()
        }
    }
    fn env() -> EnvironmentId {
        EnvironmentId("env".into())
    }
    fn key() -> ProjectionKey {
        ProjectionKey::shell(env())
    }
    fn command(id: &str) -> OrchestrationV2Command {
        serde_json::from_value(json!({"type":"thread.settle","commandId":id,"threadId":"thread"}))
            .unwrap()
    }
    async fn enqueue(queue: &OperationQueue<i32>, storage: &impl Storage) {
        queue
            .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
                storage,
                &command("c"),
                key(),
                0,
                ViewChange {
                    apply: 1,
                    revert: 0,
                },
            )
            .await
            .unwrap();
    }
    #[test]
    fn durability_precedes_delivery_and_acceptance_waits_for_its_projection() {
        block_on(async {
            let storage = AtomicStorage::default();
            let queue = OperationQueue::new(env());
            enqueue(&queue, &storage).await;
            let stored = storage
                .get(StorageArea::Ordinary, queue.key(&CommandId("c".into())))
                .await
                .unwrap()
                .unwrap();
            let envelope: Record<StoredOperation<i32>> = serde_json::from_slice(&stored).unwrap();
            assert_eq!(envelope.payload.status, CommandStatus::Pending);
            let endpoint = Endpoint::new(Reply::Accepted(7));
            let report = queue
                .dispatch(&storage, &endpoint, &CommandId("c".into()))
                .await
                .unwrap();
            assert!(matches!(
                report.outcome,
                CommandOutcome::Accepted {
                    receipt: Some(CommandReceipt {
                        committed_sequence: 7,
                        ..
                    }),
                    ..
                }
            ));
            assert!(report.persistence_error.is_none());
            let base = Rc::new(0);
            assert_eq!(*queue.view(&base, &key(), |v, i, _| *v = i.apply), 1);
            queue.observe(&key(), 0, 6);
            assert_eq!(queue.prune_confirmed(&storage).await.unwrap(), 0);
            queue.observe(&key(), 0, 7);
            assert!(Rc::ptr_eq(
                &queue.view(&base, &key(), |v, i, _| *v = i.apply),
                &base
            ));
            assert_eq!(queue.prune_confirmed(&storage).await.unwrap(), 1);
            assert!(
                storage
                    .list(StorageArea::Ordinary, OUTBOX_NAMESPACE.into())
                    .await
                    .unwrap()
                    .is_empty()
            );
        });
    }
    #[test]
    fn store_progress_retires_accepted_overlays_and_new_lineages_revalidate() {
        block_on(async {
            let store = crate::ClientStore::default();
            let session = store.begin_session(env());
            let shell = |sequence: u64| -> OrchestrationV2ShellStreamItem {
                serde_json::from_value(json!({"kind":"snapshot","snapshot":{"schemaVersion":1,"snapshotSequence":sequence,"threads":[],"archivedThreads":[],"projects":[]}})).unwrap()
            };
            store.apply_shell(&env(), session, vec![shell(5)]);
            let lineage = store.snapshot().environments[&env()].shell.generation;
            let storage = AtomicStorage::default();
            let queue = OperationQueue::new(env());
            queue
                .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
                    &storage,
                    &command("c"),
                    key(),
                    lineage,
                    ViewChange {
                        apply: 1,
                        revert: 0,
                    },
                )
                .await
                .unwrap();
            queue
                .dispatch(
                    &storage,
                    &Endpoint::new(Reply::Accepted(7)),
                    &CommandId("c".into()),
                )
                .await
                .unwrap();
            let base = Rc::new(0);
            let view =
                |queue: &OperationQueue<i32>| *queue.view(&base, &key(), |v, i, _| *v = i.apply);
            let mut progress = StoreProgress::default();
            queue.observe_store(&store, &mut progress);
            assert_eq!(view(&queue), 1, "the rows have not reached the receipt yet");
            store.apply_shell(
                &env(),
                session,
                vec![
                    serde_json::from_value(
                        json!({"kind":"project.removed","sequence":7,"projectId":"gone"}),
                    )
                    .unwrap(),
                ],
            );
            queue.observe_store(&store, &mut progress);
            assert_eq!(
                view(&queue),
                0,
                "the receipt's sequence retires the overlay"
            );

            queue
                .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
                    &storage,
                    &command("d"),
                    key(),
                    lineage,
                    ViewChange {
                        apply: 2,
                        revert: 0,
                    },
                )
                .await
                .unwrap();
            queue
                .dispatch(
                    &storage,
                    &Endpoint::for_command(Reply::Accepted(1000), "d"),
                    &CommandId("d".into()),
                )
                .await
                .unwrap();
            queue
                .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
                    &storage,
                    &command("u"),
                    key(),
                    lineage,
                    ViewChange {
                        apply: 4,
                        revert: 0,
                    },
                )
                .await
                .unwrap();
            queue.observe_store(&store, &mut progress);
            store.apply_shell(&env(), session, vec![shell(3)]);
            queue.observe_store(&store, &mut progress);
            assert_eq!(
                queue.status(&CommandId("d".into())),
                Some(CommandStatus::UnknownDelivery),
                "an authoritative snapshot revalidates commands it has not confirmed"
            );
            assert_eq!(
                queue.status(&CommandId("u".into())),
                Some(CommandStatus::Pending),
                "a command that never left has no outcome to revalidate; it is still deliverable"
            );
            assert_eq!(view(&queue), 4, "only the unsent command is still shown");

            // A command enqueued under a lineage the store has already left is revalidated on
            // the first observation.
            let queue = OperationQueue::new(env());
            queue
                .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
                    &storage,
                    &command("e"),
                    key(),
                    lineage,
                    ViewChange {
                        apply: 3,
                        revert: 0,
                    },
                )
                .await
                .unwrap();
            queue
                .dispatch(
                    &storage,
                    &Endpoint::for_command(Reply::Accepted(1000), "e"),
                    &CommandId("e".into()),
                )
                .await
                .unwrap();
            queue.observe_store(&store, &mut StoreProgress::default());
            assert_eq!(
                queue.status(&CommandId("e".into())),
                Some(CommandStatus::UnknownDelivery)
            );

            // A restart: lineage numbers restart too, so a command saved by an earlier run
            // revalidates on its first observation even when the saved number matches.
            let restarted = OperationQueue::<i32>::new(env());
            restarted.restore_available(&storage).await.unwrap();
            let current = store.snapshot().environments[&env()].shell.generation;
            let mut progress = StoreProgress::default();
            progress.lineages.insert(key(), current);
            restarted.observe_store(&store, &mut progress);
            assert_eq!(
                restarted.status(&CommandId("d".into())),
                Some(CommandStatus::UnknownDelivery)
            );
            assert_eq!(
                restarted.status(&CommandId("u".into())),
                Some(CommandStatus::Pending)
            );
            assert!(restarted.inner.borrow().from_earlier_run.is_empty());
        });
    }
    #[test]
    fn write_failure_never_sends_or_keeps_an_overlay() {
        block_on(async {
            let storage = AtomicStorage::default();
            let queue = OperationQueue::<i32>::new(env());
            storage.fail.set(true);
            assert!(
                queue
                    .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
                        &storage,
                        &command("c"),
                        key(),
                        0,
                        ViewChange {
                            apply: 1,
                            revert: 0
                        }
                    )
                    .await
                    .is_err()
            );
            assert!(queue.records().is_empty());
            enqueue(&queue, &storage).await;
            storage.fail.set(true);
            let endpoint = Endpoint::new(Reply::Accepted(1));
            assert!(
                queue
                    .dispatch(&storage, &endpoint, &CommandId("c".into()))
                    .await
                    .is_err()
            );
            assert_eq!(endpoint.calls.get(), 0);
            assert_eq!(
                queue.status(&CommandId("c".into())),
                Some(CommandStatus::Pending)
            );
            queue
                .remove_unsent(&storage, &CommandId("c".into()))
                .await
                .unwrap();
            assert!(queue.records().is_empty());
        });
    }
    #[test]
    fn rejected_unknown_and_accepted_outcomes_survive_restart_and_are_not_blindly_retried() {
        block_on(async {
            for reply in [Reply::Accepted(5), Reply::Rejected, Reply::Unknown] {
                let storage = AtomicStorage::default();
                let queue = OperationQueue::new(env());
                enqueue(&queue, &storage).await;
                let endpoint = Endpoint::new(reply);
                let report = queue
                    .dispatch(&storage, &endpoint, &CommandId("c".into()))
                    .await
                    .unwrap();
                let restored = OperationQueue::<i32>::new(env());
                assert_eq!(restored.restore(&storage).await.unwrap(), 1);
                assert_eq!(
                    restored.status(&CommandId("c".into())),
                    queue.status(&CommandId("c".into()))
                );
                assert!(matches!(
                    restored
                        .dispatch(&storage, &endpoint, &CommandId("c".into()))
                        .await,
                    Err(OperationError::Ambiguous)
                ));
                assert_eq!(endpoint.calls.get(), 1);
                if matches!(reply, Reply::Unknown | Reply::Accepted(_)) {
                    assert!(matches!(
                        restored
                            .remove_unsent(&storage, &CommandId("c".into()))
                            .await,
                        Err(OperationError::Ambiguous)
                    ));
                }
                let base = Rc::new(0);
                if matches!(reply, Reply::Rejected) {
                    let reason = "Failed to dispatch orchestration V2 command: service ProviderAdapterRegistry.get: No orchestration provider adapter is registered for codex.";
                    assert_eq!(
                        restored.status(&CommandId("c".into())),
                        Some(CommandStatus::Rejected(reason.into()))
                    );
                    assert!(
                        matches!(&report.outcome, CommandOutcome::Rejected { reason: r } if r == reason)
                    );
                }
                match report.outcome {
                    CommandOutcome::Rejected { .. } => assert!(Rc::ptr_eq(
                        &base,
                        &restored.view(&base, &key(), |v, i, _| *v = i.apply)
                    )),
                    CommandOutcome::Accepted { .. } | CommandOutcome::UnknownAfterDisconnect => {
                        assert_eq!(*restored.view(&base, &key(), |v, i, _| *v = i.apply), 1)
                    }
                    CommandOutcome::NotSent => panic!("every reply here left the client"),
                }
            }
        });
    }
    #[test]
    fn cancellation_after_request_start_is_durable_uncertainty() {
        block_on(async {
            let storage = AtomicStorage::default();
            let queue = OperationQueue::new(env());
            enqueue(&queue, &storage).await;
            let endpoint = Endpoint::new(Reply::Pending);
            let id = CommandId("c".into());
            let mut delivery = Box::pin(queue.dispatch(&storage, &endpoint, &id));
            assert!(futures::poll!(delivery.as_mut()).is_pending());
            assert_eq!(endpoint.calls.get(), 1);
            drop(delivery);
            assert_eq!(queue.status(&id), Some(CommandStatus::UnknownDelivery));
            let restored = OperationQueue::<i32>::new(env());
            restored.restore(&storage).await.unwrap();
            assert!(matches!(
                restored.dispatch(&storage, &endpoint, &id).await,
                Err(OperationError::Ambiguous)
            ));
            assert_eq!(endpoint.calls.get(), 1);
        });
    }
    #[test]
    fn receipt_reconciliation_can_resolve_unknown_delivery_without_a_resend() {
        block_on(async {
            let storage = AtomicStorage::default();
            let queue = OperationQueue::new(env());
            enqueue(&queue, &storage).await;
            let endpoint = Endpoint::new(Reply::Unknown);
            queue
                .dispatch(&storage, &endpoint, &CommandId("c".into()))
                .await
                .unwrap();
            queue.observe(&key(), 0, 9);
            queue
                .resolve_receipt(
                    &storage,
                    &CommandReceipt {
                        command: CommandId("c".into()),
                        environment: env(),
                        committed_sequence: 8,
                    },
                    0,
                )
                .await
                .unwrap();
            assert_eq!(queue.prune_confirmed(&storage).await.unwrap(), 1);
            assert_eq!(endpoint.calls.get(), 1);
        });
    }
    #[test]
    fn independent_drainers_cannot_send_the_same_durable_command_twice() {
        block_on(async {
            let storage = AtomicStorage::default();
            let first = OperationQueue::new(env());
            enqueue(&first, &storage).await;
            let second = OperationQueue::<i32>::new(env());
            second.restore(&storage).await.unwrap();
            let endpoint = Endpoint::new(Reply::Unknown);
            first
                .dispatch(&storage, &endpoint, &CommandId("c".into()))
                .await
                .unwrap();
            assert!(
                second
                    .dispatch(&storage, &endpoint, &CommandId("c".into()))
                    .await
                    .is_err()
            );
            assert_eq!(endpoint.calls.get(), 1);
        });
    }
    #[test]
    fn malformed_user_records_are_preserved_while_valid_commands_still_restore() {
        block_on(async {
            let storage = AtomicStorage::default();
            let queue = OperationQueue::new(env());
            enqueue(&queue, &storage).await;
            let bad_key = queue.key(&CommandId("bad".into()));
            storage
                .write(
                    StorageArea::Ordinary,
                    vec![StorageWrite {
                        key: bad_key.clone(),
                        value: Some(b"{invalid".to_vec()),
                    }],
                )
                .await
                .unwrap();
            let restored = OperationQueue::<i32>::new(env());
            let report = restored.restore_available(&storage).await.unwrap();
            assert_eq!(report.restored, 1);
            assert_eq!(report.errors.len(), 1);
            assert_eq!(restored.records().len(), 1);
            assert_eq!(
                storage.get(StorageArea::Ordinary, bad_key).await.unwrap(),
                Some(b"{invalid".to_vec())
            );
        });
    }
    #[test]
    fn restoration_preserves_enqueue_order_and_environment_isolation() {
        block_on(async {
            let storage = AtomicStorage::default();
            let queue = OperationQueue::<i32>::new(env());
            for (id, value) in [("z-first", 1), ("a-second", 2)] {
                queue
                    .enqueue::<WsOrchestrationV2DispatchCommandRpc>(
                        &storage,
                        &command(id),
                        key(),
                        0,
                        ViewChange {
                            apply: value,
                            revert: 0,
                        },
                    )
                    .await
                    .unwrap();
            }
            let restored = OperationQueue::<i32>::new(env());
            restored.restore(&storage).await.unwrap();
            let base = Rc::new(0);
            assert_eq!(*restored.view(&base, &key(), |v, i, _| *v = i.apply), 2);
            let other = OperationQueue::<i32>::new(EnvironmentId("other".into()));
            assert_eq!(other.restore(&storage).await.unwrap(), 0);
            assert!(Rc::ptr_eq(
                &base,
                &other.view(&base, &key(), |v, i, _| *v = i.apply)
            ));
        });
    }
    #[test]
    fn composer_dispatch_and_compaction_match_shared_policy() {
        use ComposerDispatchMode::*;
        for (running, alternate, default, steer, expected) in [
            (false, true, None, false, Auto),
            (true, false, None, true, Steer),
            (true, true, None, true, Queue),
            (true, true, Some(Queue), true, Steer),
            (true, false, Some(Restart), true, Restart),
            (true, true, Some(Restart), true, Queue),
            (true, true, Some(Queue), false, Queue),
        ] {
            assert_eq!(
                resolve_composer_dispatch_mode(running, alternate, default, steer),
                expected
            );
        }
        assert!(compaction_blocks_steering(true, false, true, None));
        assert!(!compaction_blocks_steering(false, false, false, None));
        assert!(compaction_blocks_steering(false, true, false, None));
        assert!(!compaction_blocks_steering(false, true, true, None));
        assert!(compaction_blocks_steering(
            false,
            true,
            true,
            Some((" /CoMpAcT note", 0))
        ));
        assert!(!compaction_blocks_steering(
            false,
            true,
            true,
            Some(("/compactly", 0))
        ));
        assert!(!compaction_blocks_steering(
            false,
            true,
            true,
            Some(("/compact", 1))
        ));
    }
    #[test]
    fn command_ids_preserve_callers_and_random_ids_are_uuid_v4() {
        let platform = FakePlatform::default();
        let existing = CommandId("stable-id".into());
        assert_eq!(
            allocate_command_id(&platform, Some(existing.clone())).unwrap(),
            existing
        );
        let random = allocate_command_id(&platform, None).unwrap();
        assert_eq!(random.0, "00010203-0405-4607-8809-0a0b0c0d0e0f");
    }
    #[test]
    fn send_keeps_server_delivery_intent_with_a_small_payload_and_no_projection_fetch() {
        let input: CommandMessageDispatch = serde_json::from_value(json!({"commandId":"c","threadId":"t","messageId":"m","text":"Hello","attachments":[],"createdBy":"user","creationSource":"web","dispatchMode":{"type":"start_immediately"}})).unwrap();
        for (mode, intent, dispatch) in [
            (
                ComposerDispatchMode::Auto,
                Some("auto"),
                "start_immediately",
            ),
            (
                ComposerDispatchMode::Steer,
                Some("steer"),
                "start_immediately",
            ),
            (
                ComposerDispatchMode::Restart,
                Some("restart"),
                "start_immediately",
            ),
            (ComposerDispatchMode::Queue, None, "queue_after_active"),
            (ComposerDispatchMode::Start, None, "start_immediately"),
        ] {
            let value = serde_json::to_value(message_command(input.clone(), mode)).unwrap();
            assert_eq!(value["commandId"], "c");
            assert_eq!(value["messageId"], "m");
            assert_eq!(value.get("deliveryIntent").and_then(Value::as_str), intent);
            assert_eq!(value["dispatchMode"]["type"], dispatch);
            assert!(serde_json::to_vec(&value).unwrap().len() < 1024);
        }
    }
    #[test]
    fn a_known_acceptance_survives_outcome_storage_failure_without_resending() {
        block_on(async {
            let storage = AtomicStorage::default();
            let queue = OperationQueue::new(env());
            enqueue(&queue, &storage).await;
            let failed_storage = storage.fail.clone();
            let endpoint = Endpoint {
                on_call: Some(Box::new(move || failed_storage.set(true))),
                ..Endpoint::new(Reply::Accepted(5))
            };
            let id = CommandId("c".into());
            let report = queue.dispatch(&storage, &endpoint, &id).await.unwrap();
            assert!(matches!(report.outcome, CommandOutcome::Accepted { .. }));
            assert!(report.persistence_error.is_some());
            let restarted = OperationQueue::<i32>::new(env());
            restarted.restore(&storage).await.unwrap();
            assert_eq!(restarted.status(&id), Some(CommandStatus::UnknownDelivery));
            assert_eq!(queue.flush_outcomes(&storage).await.unwrap(), 1);
            let restarted = OperationQueue::<i32>::new(env());
            restarted.restore(&storage).await.unwrap();
            assert_eq!(
                restarted.status(&id),
                Some(CommandStatus::Accepted { sequence: 5 })
            );
            assert_eq!(endpoint.calls.get(), 1);
        });
    }
    #[test]
    fn success_without_a_sequence_waits_for_matching_entity_evidence() {
        block_on(async {
            let storage = AtomicStorage::default();
            let queue = OperationQueue::new(env());
            enqueue(&queue, &storage).await;
            let endpoint = Endpoint::new(Reply::Accepted(5));
            let id = CommandId("c".into());
            let report = queue
                .deliver::<WsOrchestrationV2DispatchCommandRpc>(&storage, &endpoint, &id, |_| None)
                .await
                .unwrap();
            assert!(matches!(
                report.outcome,
                CommandOutcome::Accepted { receipt: None, .. }
            ));
            queue.observe(&key(), 0, 100);
            assert_eq!(queue.prune_confirmed(&storage).await.unwrap(), 0);
            queue
                .resolve_entity(&storage, &id, &key(), 0)
                .await
                .unwrap();
            assert_eq!(queue.prune_confirmed(&storage).await.unwrap(), 1);
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod helper_tests {
    use super::*;
    use futures::{FutureExt, executor::block_on};
    use serde_json::json;
    #[test]
    fn title_seeds_resolve_context_model_mentions_citations_and_unicode_truncation() {
        assert_eq!(
            thread_title_text_from_prompt("Delegate to model:codex/gpt-6.1-sol and report back"),
            "Delegate to gpt-6.1-sol and report back"
        );
        assert_eq!(
            thread_title_text_from_prompt(
                "model:opencode/anthropic/opus, then model:codex/gpt-5.4."
            ),
            "anthropic/opus, then gpt-5.4."
        );
        assert_eq!(
            thread_title_text_from_prompt("![image](t3-context://v1/image/id)"),
            "Attached image"
        );
        assert_eq!(
            thread_title_text_from_prompt(
                "![image](t3-context://v1/image/id) ![duplicate](t3-context://v1/image/id)"
            ),
            "Attached image"
        );
        assert_eq!(
            thread_title_text_from_prompt(
                "![image](t3-context://v1/image/a) ![image](t3-context://v1/image/b)"
            ),
            "Attached images"
        );
        assert_eq!(
            thread_title_text_from_prompt("See [a file](t3-context://v1/file/f)"),
            "See a file"
        );
        assert_eq!(
            thread_title_text_from_prompt(
                "[Assistant quote](t3-citation://v1/e/t/m?text=Fix+this&start=0&end=8&prefix=&suffix=&comment=Please)"
            ),
            "Fix this Comment: Please"
        );
        assert_eq!(
            derive_thread_title_seed("", &[], &[" ", "  Fallback  title "]),
            "Fallback title"
        );
        assert_eq!(derive_thread_title_seed("", &[], &[]), "New thread");
        assert_eq!(
            truncate_title(&"🦀".repeat(51)),
            format!("{}...", "🦀".repeat(50))
        );
    }
    #[test]
    fn clone_helpers_port_provider_pages_paths_numeric_names_and_ghq_safety() {
        for (input, expected) in [
            ("https://github.com/acme/repo.git?ref=main#readme", "repo"),
            ("git@github.com:repo.git", "repo"),
            ("ssh://git@github.com:22/acme/repo.git", "repo"),
            ("https://github.com/acme/123", "123"),
            ("https://github.com:123", ""),
            ("C:\\src\\repo.git", "repo"),
            ("/srv/repo.git", "repo"),
        ] {
            assert_eq!(get_clone_directory_name(input), expected, "{input}");
        }
        for (remote, expected) in [
            ("git@github.com:owner/repo.git", "github.com/owner/repo"),
            (
                "ssh://git@gitlab.com:2222/group/sub/repo.git",
                "gitlab.com/group/sub/repo",
            ),
            ("https://github.com/owner/../../repo", "repo"),
            ("/srv/git/repo.git", "repo"),
        ] {
            assert_eq!(get_clone_directory_path(remote, true, None), expected);
        }
        assert_eq!(
            get_clone_destination_path("C:\\Users\\me\\ghq", Some("github.com/owner/repo")),
            "C:\\Users\\me\\ghq\\github.com\\owner\\repo"
        );
        assert_eq!(
            normalize_pasted_clone_url("  pingdotgg/t3code  "),
            "https://github.com/pingdotgg/t3code.git"
        );
        for (input, expected) in [
            (
                "https://github.com/pingdotgg/t3code/tree/main",
                "https://github.com/pingdotgg/t3code",
            ),
            (
                "https://gitlab.com/group/repo/-/tree/main",
                "https://gitlab.com/group/repo",
            ),
            (
                "https://scm.test/group/repo/src/branch/worker.git",
                "https://scm.test/group/repo/src/branch/worker.git",
            ),
            (
                "https://alice:example@bitbucket.org/workspace/repo/src/master/",
                "https://alice:example@bitbucket.org/workspace/repo",
            ),
        ] {
            assert_eq!(normalize_pasted_clone_url(input), expected);
        }
        assert!(github_incomplete_owner_repository_message("github", "  owner// ").is_some());
        assert!(github_incomplete_owner_repository_message("gitlab", "owner/").is_none());
    }
    #[test]
    fn picked_folders_relative_paths_and_project_create_keep_destination_semantics() {
        for (path, sensitive, expected) in [
            ("/Users/me/code", true, "/Users/me/code/repo"),
            ("/Users/me/repo", true, "/Users/me/repo/"),
            ("C:\\Projects\\Repo", false, "C:\\Projects\\Repo\\"),
            ("/", true, "/repo"),
        ] {
            assert_eq!(
                get_clone_destination_for_picked_folder(path, "repo", sensitive),
                expected
            );
        }
        assert_eq!(
            resolve_add_project_path("../new", Some("/base/current"), "linux"),
            Ok("/base/new".into())
        );
        assert_eq!(
            resolve_add_project_path("..\\new", Some("C:\\base\\current"), "win32"),
            Ok("C:\\base\\new".into())
        );
        assert!(resolve_add_project_path("./relative", None, "linux").is_err());
        assert!(resolve_add_project_path("C:\\repo", None, "linux").is_err());
        let command = build_project_create_command(
            CommandId("c".into()),
            crate::types::ProjectId("p".into()),
            "/base/repo".into(),
        );
        let value = serde_json::to_value(command).unwrap();
        assert_eq!(value["title"], "repo");
        assert_eq!(value["createWorkspaceRootIfMissing"], true);
        assert_eq!(value["defaultModelSelection"], Value::Null);
    }
    #[test]
    fn metadata_uses_a_stable_derived_id_for_its_second_command() {
        let metadata: CommandThreadMetadataUpdate =
            serde_json::from_value(json!({"commandId":"caller-id","threadId":"t","title":"New"}))
                .unwrap();
        let model =
            serde_json::from_value(json!({"instanceId":"codex","model":"gpt-5.4"})).unwrap();
        let commands = metadata_commands(metadata, Some(model));
        assert_eq!(commands.len(), 2);
        assert_eq!(
            serde_json::to_value(&commands[0]).unwrap()["commandId"],
            "caller-id"
        );
        assert_eq!(
            serde_json::to_value(&commands[1]).unwrap()["commandId"],
            "caller-id:model-selection"
        );
        let empty = serde_json::from_value(json!({"commandId":"c","threadId":"t"})).unwrap();
        assert!(metadata_commands(empty, None).is_empty());
    }
    #[test]
    fn command_lanes_are_fifo_and_cancelling_a_waiter_does_not_stall_following_work() {
        block_on(async {
            let lane = Rc::new(RefCell::new(SerialLane::default()));
            let first = SerialPermit::reserve(lane.clone());
            let cancelled = SerialPermit::reserve(lane.clone());
            let second = SerialPermit::reserve(lane.clone());
            let third = SerialPermit::reserve(lane);
            assert!(first.ready().now_or_never().is_some());
            assert!(second.ready().now_or_never().is_none());
            drop(cancelled);
            drop(first);
            assert!(second.ready().now_or_never().is_some());
            assert!(third.ready().now_or_never().is_none());
            drop(second);
            assert!(third.ready().now_or_never().is_some());
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod command_plan_tests {
    use super::*;
    use crate::testing::{RpcScript, ScriptedRpc};
    use futures::executor::block_on;
    use serde_json::json;
    const NOW: &str = "2026-06-20T00:00:00.000Z";
    fn projection() -> OrchestrationV2ThreadProjection {
        serde_json::from_value(json!({"thread":{"id":"t","projectId":"p","title":"Thread","providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"activeProviderThreadId":null,"lineage":{"rootThreadId":"t","parentThreadId":null,"relationshipToParent":null},"forkedFrom":null,"createdBy":"user","creationSource":"web","createdAt":NOW,"updatedAt":NOW,"archivedAt":null,"settledOverride":null,"settledAt":null,"lastVisitedAt":null,"deletedAt":null},"runs":[],"attempts":[],"nodes":[],"subagents":[],"providerSessions":[],"providerThreads":[],"providerTurns":[],"runtimeRequests":[],"messages":[],"plans":[],"turnItems":[],"checkpointScopes":[],"checkpoints":[],"contextHandoffs":[],"contextTransfers":[],"visibleTurnItems":[],"updatedAt":NOW})).unwrap()
    }
    #[test]
    fn rollback_selects_the_persisted_thread_start_checkpoint_and_allows_missing_files_for_conversation_only()
     {
        let mut projection = projection();
        projection.checkpoints.push(serde_json::from_value(json!({"id":"checkpoint","threadId":"t","scopeId":"scope","runId":null,"nodeId":"node","parentCheckpointId":null,"ordinalWithinScope":0,"appRunOrdinal":null,"ref":"ref","status":"ready","files":[],"capturedAt":NOW})).unwrap());
        let id = CommandId("c".into());
        let command =
            checkpoint_rollback_command(id.clone(), &projection, None, None, Some(0), None)
                .unwrap();
        assert_eq!(
            serde_json::to_value(command).unwrap()["checkpointId"],
            "checkpoint"
        );
        projection.checkpoints[0].status = CheckpointStatus::Missing;
        assert!(
            checkpoint_rollback_command(id.clone(), &projection, None, None, Some(0), None)
                .is_err()
        );
        assert!(
            checkpoint_rollback_command(id, &projection, None, None, Some(0), Some(false)).is_ok()
        );
    }
    #[test]
    fn interrupt_uses_thread_stop_without_loading_history_and_preserves_explicit_run_ids() {
        let input = InterruptInput {
            command_id: CommandId("caller".into()),
            thread_id: crate::types::ThreadId("t".into()),
            run_id: None,
            subagent_id: None,
            legacy_turn_id: None,
        };
        let InterruptPlan::Ready(commands) =
            interrupt_plan(&input, true, &BTreeMap::new()).unwrap()
        else {
            panic!("must not load history");
        };
        assert_eq!(
            serde_json::to_value(&commands[0].command).unwrap()["type"],
            "thread.stop"
        );
        let input = InterruptInput {
            run_id: Some(t3_contracts::base_schemas::RunId("named-run".into())),
            ..input
        };
        let InterruptPlan::Ready(commands) =
            interrupt_plan(&input, true, &BTreeMap::new()).unwrap()
        else {
            panic!("must not load history");
        };
        let command = serde_json::to_value(&commands[0].command).unwrap();
        assert_eq!(command["type"], "run.interrupt");
        assert_eq!(command["commandId"], "caller");
        assert_eq!(command["runId"], "named-run");
        assert_eq!(command["holdQueue"], true);
    }
    #[test]
    fn legacy_stop_finds_pending_work_on_finished_runs_but_excludes_rolled_back_runs() {
        let mut projection = projection();
        projection.runs.push(serde_json::from_value(json!({"id":"run","threadId":"t","ordinal":1,"providerInstanceId":"codex","modelSelection":{"instanceId":"codex","model":"gpt-5.4"},"providerThreadId":null,"userMessageId":"message","rootNodeId":null,"activeAttemptId":null,"status":"completed","requestedAt":NOW,"startedAt":NOW,"completedAt":NOW,"checkpointId":null,"contextHandoffId":null})).unwrap());
        projection.turn_items.push(serde_json::from_value(json!({"id":"item","type":"command_execution","threadId":"t","runId":"run","nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":1,"status":"running","title":null,"input":"background command","output":"","exitCode":null,"startedAt":NOW,"completedAt":null,"updatedAt":NOW})).unwrap());
        let input = InterruptInput {
            command_id: CommandId("c".into()),
            thread_id: crate::types::ThreadId("t".into()),
            run_id: None,
            subagent_id: None,
            legacy_turn_id: None,
        };
        for status in [
            OrchestrationV2RunStatus::Waiting,
            OrchestrationV2RunStatus::Completed,
            OrchestrationV2RunStatus::RolledBack,
        ] {
            projection.runs[0].status = status.clone();
            let projections =
                BTreeMap::from([(input.thread_id.clone(), Rc::new(projection.clone()))]);
            let InterruptPlan::Ready(commands) =
                interrupt_plan(&input, false, &projections).unwrap()
            else {
                panic!("history supplied");
            };
            if status == OrchestrationV2RunStatus::RolledBack {
                assert!(commands.is_empty());
            } else {
                assert_eq!(
                    serde_json::to_value(&commands[0].command).unwrap()["runId"],
                    "run"
                );
            }
        }
    }
    #[test]
    fn stop_without_an_active_run_ends_visible_pull_request_watches() {
        let mut projection = projection();
        projection.thread.pull_requests = Some(Some(vec![serde_json::from_value(json!({"host":"github.com","repository":"org/repo","number":1,"url":"https://github.com/org/repo/pull/1","source":"manual","linkedAt":NOW,"snapshot":null,"stack":null,"watch":{"startedAt":NOW,"headSha":null,"failedChecks":[],"passed":false,"remarksThrough":NOW,"remarkIds":[],"conflicting":false,"wakes":0}})).unwrap()]));
        let input = InterruptInput {
            command_id: CommandId("c".into()),
            thread_id: crate::types::ThreadId("t".into()),
            run_id: None,
            subagent_id: None,
            legacy_turn_id: None,
        };
        let projections = BTreeMap::from([(input.thread_id.clone(), Rc::new(projection))]);
        let InterruptPlan::Ready(commands) = interrupt_plan(&input, false, &projections).unwrap()
        else {
            panic!("history supplied");
        };
        let value = serde_json::to_value(&commands[0].command).unwrap();
        assert_eq!(value["type"], "thread.pull-request.watch");
        assert_eq!(value["watching"], false);
        assert_eq!(value["commandId"], "c");
    }
    #[test]
    fn attachment_persistence_preserves_mixed_order_and_remaps_context_ids() {
        block_on(async {
            let stored = |id: &str| {
                serde_json::from_value::<t3_contracts::chat_attachment::ChatAttachment>(json!({"type":"image","id":id,"name":"image","mimeType":"image/png","sizeBytes":1})).unwrap()
            };
            let upload = serde_json::from_value::<t3_contracts::chat_attachment::UploadChatAttachment>(json!({"type":"image","id":"old","name":"image","mimeType":"image/png","sizeBytes":1,"dataUrl":"data:image/png;base64,AA=="})).unwrap();
            let attachments = vec![
                SendAttachment::Stored(stored("stored")),
                SendAttachment::Upload(Box::new(upload.clone())),
            ];
            let rpc = ScriptedRpc::default();
            rpc.scripts.borrow_mut().push_back(RpcScript {
                method: "assets.persistChatAttachments".into(),
                input: json!({"threadId":"t","messageId":"m","attachments":[upload]}),
                replies: vec![Ok(json!({"attachments":[stored("new")]}))],
            });
            let persisted = persist_attachments(
                &rpc,
                crate::types::ThreadId("t".into()),
                t3_contracts::base_schemas::MessageId("m".into()),
                &attachments,
            )
            .await
            .unwrap();
            assert_eq!(
                persisted.iter().map(attachment_id).collect::<Vec<_>>(),
                vec!["stored", "new"]
            );
            let mut context = serde_json::from_value(json!({"version":1,"records":[{"version":1,"kind":"image","contextId":"img","label":"Image","attachmentId":"old","name":"image","mimeType":"image/png","sizeBytes":1}]})).unwrap();
            remap_context_attachments(&mut context, &attachments, &persisted);
            assert_eq!(
                serde_json::to_value(context).unwrap()["records"][0]["attachmentId"],
                "new"
            );
            assert!(rpc.scripts.borrow().is_empty());
        });
    }
}
