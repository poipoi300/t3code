use crate::{
    api::{CommitPreparation, CommitRequest, CommitResult, EventCommitter},
    error::{Error, Result},
    provider::{ProviderEvent, ProviderEventKind, ProviderResult},
    runtime::{
        commit::{BatchJob, CommitBarrier, EventSink, WriteOptions},
        worker::{PendingBatch, task_error},
    },
};
use std::{
    collections::VecDeque,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

pub const MAX_EVENTS: usize = 64;
pub const MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushReason {
    Ready,
    Limit,
    Control,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchStats {
    pub jobs: usize,
    pub bytes: usize,
    pub reason: FlushReason,
    pub committed: bool,
}

pub struct BatchReply(oneshot::Receiver<Result<Option<CommitResult>>>);

impl BatchReply {
    pub async fn wait(self) -> Result<Option<CommitResult>> {
        self.0.await.map_err(|_| Error::Cancelled)?
    }
}

pub struct ConsumerOwner(CancellationToken);

impl Default for ConsumerOwner {
    fn default() -> Self {
        Self(CancellationToken::new())
    }
}

impl ConsumerOwner {
    pub fn cancel(&self) {
        self.0.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

impl Drop for ConsumerOwner {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Job {
    owner: CancellationToken,
    bytes: usize,
    snapshot: bool,
    input: Option<PreparedCommit>,
    reply: oneshot::Sender<Result<Option<CommitResult>>>,
    pending: PendingBatch,
}

pub struct PreparedCommit {
    pub request: CommitRequest,
    pub options: WriteOptions,
    pub prepare: Option<CommitPreparation>,
}

impl From<CommitRequest> for PreparedCommit {
    fn from(request: CommitRequest) -> Self {
        Self {
            request,
            options: WriteOptions::default(),
            prepare: None,
        }
    }
}

#[derive(Clone)]
pub struct ProviderCommitBatch(Arc<Coordinator>);

struct Coordinator {
    sink: EventSink,
    enabled: bool,
    sender: mpsc::Sender<Job>,
    shutdown: CancellationToken,
    worker: Mutex<Option<JoinHandle<()>>>,
    stats: broadcast::Sender<BatchStats>,
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

impl ProviderCommitBatch {
    pub fn new(sink: EventSink, enabled: bool) -> Self {
        let (sender, receiver) = mpsc::channel(MAX_EVENTS);
        let shutdown = CancellationToken::new();
        let stats = broadcast::channel(64).0;
        let worker = tokio::spawn(run(sink.clone(), receiver, shutdown.clone(), stats.clone()));
        let coordinator = Arc::new(Coordinator {
            sink,
            enabled,
            sender,
            shutdown,
            worker: Mutex::new(Some(worker)),
            stats,
        });
        let barrier: Arc<dyn CommitBarrier> = coordinator.clone();
        coordinator.sink.attach_barrier(Arc::downgrade(&barrier));
        Self(coordinator)
    }

    pub fn subscribe_stats(&self) -> broadcast::Receiver<BatchStats> {
        self.0.stats.subscribe()
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.0.shutdown.clone()
    }

    pub async fn enqueue_snapshot(
        &self,
        owner: &ConsumerOwner,
        bytes: usize,
        input: CommitRequest,
    ) -> Result<BatchReply> {
        self.enqueue_snapshot_with_options(owner, bytes, input, WriteOptions::default())
            .await
    }

    pub async fn enqueue_snapshot_with_options(
        &self,
        owner: &ConsumerOwner,
        bytes: usize,
        input: CommitRequest,
        options: WriteOptions,
    ) -> Result<BatchReply> {
        self.enqueue_prepared_snapshot(
            owner,
            bytes,
            PreparedCommit {
                request: input,
                options,
                prepare: None,
            },
        )
        .await
    }

    pub async fn enqueue_prepared_snapshot(
        &self,
        owner: &ConsumerOwner,
        bytes: usize,
        input: PreparedCommit,
    ) -> Result<BatchReply> {
        if !self.0.enabled {
            let _pending = self.0.sink.signals().track_batch();
            if self.0.shutdown.is_cancelled() || owner.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let (sender, receiver) = oneshot::channel();
            let result = match input.prepare {
                Some(prepare) => {
                    self.0
                        .sink
                        .commit_prepared(input.request, input.options, prepare)
                        .await
                }
                None => {
                    self.0
                        .sink
                        .commit_with_options(input.request, input.options)
                        .await
                }
            };
            let _ = sender.send(result.map(Some));
            return Ok(BatchReply(receiver));
        }
        self.enqueue(owner, true, bytes, Some(input)).await
    }

    async fn enqueue(
        &self,
        owner: &ConsumerOwner,
        snapshot: bool,
        bytes: usize,
        input: Option<PreparedCommit>,
    ) -> Result<BatchReply> {
        let (reply, receiver) = oneshot::channel();
        let job = Job {
            owner: owner.0.clone(),
            bytes,
            snapshot,
            input,
            reply,
            pending: self.0.sink.signals().track_batch(),
        };
        tokio::select! {
            biased;
            _ = self.0.shutdown.cancelled() => Err(Error::Cancelled),
            _ = owner.0.cancelled() => Err(Error::Cancelled),
            accepted = self.0.sender.send(job) => {
                accepted.map_err(|_| Error::Cancelled)?;
                Ok(BatchReply(receiver))
            },
        }
    }

    pub async fn barrier<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        if self.0.shutdown.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if self.0.enabled {
            let owner = ConsumerOwner::default();
            self.enqueue(&owner, false, 0, None).await?.wait().await?;
        }
        // Controls and their background-work checks run in the caller, outside the shared worker.
        work.await
    }

    pub async fn drain(&self) -> Result<()> {
        self.barrier(async { Ok(()) }).await?;
        self.0.sink.drain().await;
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.0.shutdown.cancel();
        let worker = self
            .0
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(worker) = worker {
            worker.await.map_err(task_error)?;
        }
        self.0.sink.drain().await;
        Ok(())
    }

    pub async fn consume<P, PF, A, AF, B, BF>(
        &self,
        source: mpsc::Receiver<ProviderResult<ProviderEvent>>,
        mut process: P,
        after: A,
        can_batch: B,
    ) -> Result<()>
    where
        P: FnMut(ProviderEvent) -> PF,
        PF: Future<Output = Result<Option<CommitRequest>>>,
        A: FnMut() -> AF,
        AF: Future<Output = Result<bool>>,
        B: FnMut() -> BF,
        BF: Future<Output = bool>,
    {
        self.consume_prepared(
            source,
            |event| {
                let work = process(event);
                async move { work.await.map(|request| request.map(PreparedCommit::from)) }
            },
            after,
            can_batch,
        )
        .await
    }

    pub async fn consume_prepared<P, PF, A, AF, B, BF>(
        &self,
        mut source: mpsc::Receiver<ProviderResult<ProviderEvent>>,
        mut process: P,
        mut after: A,
        mut can_batch: B,
    ) -> Result<()>
    where
        P: FnMut(ProviderEvent) -> PF,
        PF: Future<Output = Result<Option<PreparedCommit>>>,
        A: FnMut() -> AF,
        AF: Future<Output = Result<bool>>,
        B: FnMut() -> BF,
        BF: Future<Output = bool>,
    {
        let owner = ConsumerOwner::default();
        let mut pending: VecDeque<BatchReply> = VecDeque::new();
        loop {
            let signal = tokio::select! {
                biased;
                _ = self.0.shutdown.cancelled() => return Err(Error::Cancelled),
                signal = source.recv() => Some(signal),
                result = async {
                    match pending.front_mut() {
                        Some(reply) => (&mut reply.0).await.map_err(|_| Error::Cancelled)?,
                        None => std::future::pending::<Result<Option<CommitResult>>>().await,
                    }
                } => {
                    pending.pop_front();
                    if let Err(error) = result { owner.cancel(); return Err(error) }
                    None
                },
            };
            let Some(signal) = signal else { continue };
            let Some(signal) = signal else {
                while let Some(reply) = pending.pop_front() {
                    wait_reply(reply, &owner).await?;
                }
                return Ok(());
            };
            let event = match signal {
                Ok(event) => event,
                Err(error) => {
                    while let Some(reply) = pending.pop_front() {
                        wait_reply(reply, &owner).await?;
                    }
                    return Err(error.into());
                }
            };
            let snapshot = self.0.enabled && is_snapshot(&event) && can_batch().await;
            if snapshot {
                let bytes = snapshot_bytes(&event)?;
                match process(event).await {
                    Ok(Some(input)) => pending
                        .push_back(self.enqueue_prepared_snapshot(&owner, bytes, input).await?),
                    Ok(None) => {}
                    Err(error) => {
                        while let Some(reply) = pending.pop_front() {
                            wait_reply(reply, &owner).await?;
                        }
                        return Err(error);
                    }
                }
                if pending.len() >= MAX_EVENTS {
                    wait_reply(pending.pop_front().ok_or(Error::Cancelled)?, &owner).await?;
                }
                if owner.is_cancelled() {
                    while let Some(reply) = pending.pop_front() {
                        wait_reply(reply, &owner).await?;
                    }
                    return Err(Error::Cancelled);
                }
            } else {
                while let Some(reply) = pending.pop_front() {
                    wait_reply(reply, &owner).await?;
                }
                self.barrier(async { Ok(()) }).await?;
                if let Some(input) = process(event).await? {
                    match input.prepare {
                        Some(prepare) => {
                            self.0
                                .sink
                                .commit_prepared(input.request, input.options, prepare)
                                .await?
                        }
                        None => {
                            self.0
                                .sink
                                .commit_with_options(input.request, input.options)
                                .await?
                        }
                    };
                }
                if after().await? {
                    return Ok(());
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl CommitBarrier for Coordinator {
    async fn wait(&self) -> Result<()> {
        if self.shutdown.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !self.enabled {
            return Ok(());
        }
        let owner = ConsumerOwner::default();
        let (reply, receiver) = oneshot::channel();
        let job = Job {
            owner: owner.0.clone(),
            bytes: 0,
            snapshot: false,
            input: None,
            reply,
            pending: self.sink.signals().track_batch(),
        };
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => return Err(Error::Cancelled),
            result = self.sender.send(job) => result.map_err(|_| Error::Cancelled)?,
        }
        BatchReply(receiver).wait().await?;
        Ok(())
    }
}

async fn wait_reply(reply: BatchReply, owner: &ConsumerOwner) -> Result<()> {
    if let Err(error) = reply.wait().await {
        owner.cancel();
        return Err(error);
    }
    Ok(())
}

async fn run(
    sink: EventSink,
    mut receiver: mpsc::Receiver<Job>,
    shutdown: CancellationToken,
    stats: broadcast::Sender<BatchStats>,
) {
    loop {
        let first = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            job = receiver.recv() => match job { Some(job) => job, None => break },
        };
        let mut ready = Vec::with_capacity(MAX_EVENTS);
        ready.push(first);
        while ready.len() < MAX_EVENTS {
            match receiver.try_recv() {
                Ok(job) => ready.push(job),
                Err(_) => break,
            }
        }
        let mut group = Vec::new();
        let mut bytes = 0;
        for job in ready {
            if shutdown.is_cancelled() {
                let _ = job.reply.send(Err(Error::Cancelled));
                continue;
            }
            if !job.snapshot {
                flush(
                    &sink,
                    std::mem::take(&mut group),
                    FlushReason::Control,
                    &stats,
                    &shutdown,
                )
                .await;
                bytes = 0;
                let _ = job.reply.send(if job.owner.is_cancelled() {
                    Err(Error::Cancelled)
                } else {
                    Ok(None)
                });
                continue;
            }
            if !group.is_empty() && bytes + job.bytes > MAX_BYTES {
                flush(
                    &sink,
                    std::mem::take(&mut group),
                    FlushReason::Limit,
                    &stats,
                    &shutdown,
                )
                .await;
                bytes = 0;
            }
            bytes += job.bytes;
            group.push(job);
            if group.len() >= MAX_EVENTS || bytes >= MAX_BYTES {
                flush(
                    &sink,
                    std::mem::take(&mut group),
                    FlushReason::Limit,
                    &stats,
                    &shutdown,
                )
                .await;
                bytes = 0;
            }
        }
        flush(&sink, group, FlushReason::Ready, &stats, &shutdown).await;
    }
    receiver.close();
    while let Some(job) = receiver.recv().await {
        let _ = job.reply.send(Err(Error::Cancelled));
    }
}

async fn flush(
    sink: &EventSink,
    jobs: Vec<Job>,
    reason: FlushReason,
    stats: &broadcast::Sender<BatchStats>,
    shutdown: &CancellationToken,
) {
    let mut replies = Vec::new();
    let mut work = Vec::new();
    let mut bytes = 0;
    for job in jobs {
        if job.owner.is_cancelled() || shutdown.is_cancelled() {
            let _ = job.reply.send(Err(Error::Cancelled));
            continue;
        }
        if let Some(input) = job.input {
            bytes += job.bytes;
            replies.push((job.owner.clone(), job.reply, job.pending));
            work.push(BatchJob {
                owner: job.owner,
                scope: Some(shutdown.clone()),
                request: input.request,
                options: input.options,
                prepare: input.prepare,
            });
        } else {
            let _ = job.reply.send(Ok(None));
        }
    }
    if work.is_empty() {
        return;
    }
    let mut report = BatchStats {
        jobs: work.len(),
        bytes,
        reason,
        committed: false,
    };
    match sink.commit_batch(work).await {
        Ok(results) => {
            report.committed = true;
            for ((owner, reply, _pending), result) in replies.into_iter().zip(results) {
                if result.is_err() {
                    owner.cancel();
                }
                let _ = reply.send(result.map(Some));
            }
        }
        Err(error) => {
            let detail = error.to_string();
            let mut original = Some(error);
            for (owner, reply, _pending) in replies {
                owner.cancel();
                let _ = reply.send(Err(original.take().unwrap_or_else(|| Error::Service {
                    operation: "provider-batch.commit",
                    detail: detail.clone(),
                })));
            }
        }
    }
    let _ = stats.send(report);
}

pub fn is_snapshot(event: &ProviderEvent) -> bool {
    matches!(
        event.kind,
        ProviderEventKind::Node(_)
            | ProviderEventKind::Message(_)
            | ProviderEventKind::TurnItem { .. }
            | ProviderEventKind::Plan(_)
    )
}

pub fn snapshot_bytes(event: &ProviderEvent) -> Result<usize> {
    let mut value = serde_json::Map::new();
    value.insert("driver".into(), serde_json::to_value(&event.driver)?);
    let (kind, key, payload) = match &event.kind {
        ProviderEventKind::Node(node) => ("node.updated", "node", serde_json::to_value(node)?),
        ProviderEventKind::Message(message) => {
            ("message.updated", "message", serde_json::to_value(message)?)
        }
        ProviderEventKind::TurnItem {
            item,
            session_scoped,
        } => {
            if let Some(scoped) = session_scoped {
                value.insert("sessionScoped".into(), (*scoped).into());
            }
            ("turn_item.updated", "turnItem", serde_json::to_value(item)?)
        }
        ProviderEventKind::Plan(plan) => ("plan.updated", "plan", serde_json::to_value(plan)?),
        _ => return Ok(0),
    };
    value.insert("type".into(), kind.into());
    value.insert(key.into(), payload);
    Ok(serde_json::to_vec(&value)?.len())
}
