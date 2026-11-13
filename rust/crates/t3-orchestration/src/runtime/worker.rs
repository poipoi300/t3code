use crate::{
    api::{Clock, EffectExecutor, EffectWorker as WorkerApi},
    effects::{ClaimOptions, EffectExecutionOptions, EffectRequest, EffectRow, EffectStatus},
    error::{Error, Result},
    stores::{StoreDatabase, outbox},
};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::watch, task::JoinSet};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Clone)]
pub struct OutboxSignals(Arc<SignalState>);

struct SignalState {
    changed: watch::Sender<u64>,
    cancellations: Mutex<HashMap<String, Weak<CancellationToken>>>,
    batch_jobs: AtomicUsize,
    settled: Mutex<HashMap<String, Weak<watch::Sender<bool>>>>,
}

impl Default for OutboxSignals {
    fn default() -> Self {
        Self(Arc::new(SignalState {
            changed: watch::channel(0).0,
            cancellations: Mutex::new(HashMap::new()),
            batch_jobs: AtomicUsize::new(0),
            settled: Mutex::new(HashMap::new()),
        }))
    }
}

impl OutboxSignals {
    pub fn pending_batch_jobs(&self) -> usize {
        self.0.batch_jobs.load(Ordering::Acquire)
    }

    pub(crate) fn track_batch(&self) -> PendingBatch {
        self.0.batch_jobs.fetch_add(1, Ordering::AcqRel);
        PendingBatch(self.clone())
    }
    pub fn notify(&self) {
        self.0
            .changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changed.subscribe()
    }

    pub fn cancel(&self, ids: &[String]) {
        let mut cancellations = self
            .0
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for id in ids {
            if let Some(token) = cancellations.remove(id).and_then(|token| token.upgrade()) {
                token.cancel();
            }
        }
        drop(cancellations);
        self.signal_settled(ids);
        if !ids.is_empty() {
            self.notify();
        }
    }

    pub fn signal_settled(&self, ids: &[String]) {
        let mut settled = self
            .0
            .settled
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for id in ids {
            if let Some(signal) = settled.remove(id).and_then(|signal| signal.upgrade()) {
                signal.send_replace(true);
            }
        }
    }

    fn settlement(&self, id: &str) -> Arc<watch::Sender<bool>> {
        let mut settled = self
            .0
            .settled
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        settled.retain(|_, signal| signal.strong_count() > 0);
        if let Some(signal) = settled.get(id).and_then(Weak::upgrade) {
            return signal;
        }
        let signal = Arc::new(watch::channel(false).0);
        settled.insert(id.into(), Arc::downgrade(&signal));
        signal
    }

    fn cancellation(&self, id: &str, shutdown: &CancellationToken) -> Arc<CancellationToken> {
        let token = Arc::new(shutdown.child_token());
        let mut cancellations = self
            .0
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cancellations.retain(|_, value| value.strong_count() != 0);
        cancellations.insert(id.into(), Arc::downgrade(&token));
        token
    }
}

pub(crate) struct PendingBatch(OutboxSignals);

impl Drop for PendingBatch {
    fn drop(&mut self) {
        self.0.0.batch_jobs.fetch_sub(1, Ordering::AcqRel);
        self.0.notify();
    }
}

#[derive(Debug, Clone)]
pub struct WorkerOptions {
    pub worker_id: String,
    pub max_attempts: i64,
    pub concurrency: usize,
    pub liveness_poll_interval: Duration,
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            worker_id: format!("orchestration-v2:{}", std::process::id()),
            max_attempts: 5,
            concurrency: 4,
            liveness_poll_interval: Duration::from_secs(30),
        }
    }
}

#[derive(Clone)]
pub struct EffectWorker(Arc<WorkerState>);

struct WorkerState {
    stores: StoreDatabase,
    executor: Arc<dyn EffectExecutor>,
    clock: Arc<dyn Clock>,
    signals: OutboxSignals,
    options: WorkerOptions,
    shutdown: CancellationToken,
    executions: TaskTracker,
    running: AtomicUsize,
    admission: Mutex<bool>,
}

struct Completion(Arc<WorkerState>);

impl Drop for Completion {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::AcqRel);
        self.0.signals.notify();
    }
}

impl EffectWorker {
    pub fn new(
        stores: StoreDatabase,
        executor: Arc<dyn EffectExecutor>,
        clock: Arc<dyn Clock>,
        signals: OutboxSignals,
        mut options: WorkerOptions,
    ) -> Self {
        options.max_attempts = options.max_attempts.max(1);
        options.concurrency = options.concurrency.max(1);
        options.liveness_poll_interval =
            options.liveness_poll_interval.max(Duration::from_millis(1));
        Self(Arc::new(WorkerState {
            stores,
            executor,
            clock,
            signals,
            options,
            shutdown: CancellationToken::new(),
            executions: TaskTracker::new(),
            running: AtomicUsize::new(0),
            admission: Mutex::new(true),
        }))
    }

    pub fn signals(&self) -> OutboxSignals {
        self.0.signals.clone()
    }

    pub fn executing(&self) -> usize {
        self.0.running.load(Ordering::Acquire)
    }

    pub async fn next_claimable_at(&self, max_titles: Option<i64>) -> Result<Option<String>> {
        self.0
            .stores
            .read(move |db| outbox::next_claimable_at(db, max_titles))
            .await
    }

    async fn wake_delay(&self, max_titles: Option<i64>) -> Result<Duration> {
        let now = self.0.clock.now().0;
        let liveness = self.0.options.liveness_poll_interval;
        self.0
            .stores
            .read(move |db| {
                let next = outbox::next_claimable_at(db, max_titles)?;
                let Some(next) = next else {
                    return Ok(liveness);
                };
                let millis: i64 = db.query_row(
                    "SELECT CAST(ROUND((julianday(?1)-julianday(?2))*86400000) AS INTEGER)",
                    [&next, &now],
                    |row| row.get(0),
                )?;
                Ok(if millis > 0 {
                    Duration::from_millis(millis as u64).min(liveness)
                } else {
                    Duration::from_millis(25).min(liveness)
                })
            })
            .await
    }

    async fn daemon_delay(&self, max_titles: Option<i64>) -> Duration {
        self.wake_delay(max_titles).await.unwrap_or_else(|error| {
            eprintln!("effect worker deadline read failed: {error}");
            self.0.options.liveness_poll_interval
        })
    }

    pub async fn run_daemon(&self) -> Result<()> {
        let mut slots = JoinSet::new();
        for _ in 0..self.0.options.concurrency {
            let worker = self.clone();
            slots.spawn(async move { worker.run_slot().await });
        }
        let mut failure = None;
        while let Some(result) = slots.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                    self.0.shutdown.cancel();
                }
                Err(error) => {
                    failure.get_or_insert_with(|| task_error(error));
                    self.0.shutdown.cancel();
                }
            }
        }
        self.shutdown().await?;
        failure.map_or(Ok(()), Err)
    }

    async fn run_slot(&self) -> Result<()> {
        let max_titles =
            (self.0.options.concurrency > 1).then_some(self.0.options.concurrency as i64 - 1);
        loop {
            let mut changed = self.0.signals.subscribe();
            if self.0.shutdown.is_cancelled() {
                return Ok(());
            }
            // Only the slot's wait expires; run_once owns a separately tracked execution.
            let outcome = tokio::select! {
                _ = self.0.shutdown.cancelled() => return Ok(()),
                result = tokio::time::timeout(Duration::from_secs(30), self.run_once(ClaimOptions {
                    max_running_title_generations: max_titles, ..Default::default()
                })) => result,
            };
            let delay = match outcome {
                Err(_) | Ok(Ok(true)) => continue,
                Ok(Err(_)) => Duration::from_secs(1).min(self.0.options.liveness_poll_interval),
                Ok(Ok(false)) => self.daemon_delay(max_titles).await,
            };
            tokio::select! {
                _ = self.0.shutdown.cancelled() => return Ok(()),
                _ = changed.changed() => {},
                _ = tokio::time::sleep(delay) => {},
            }
        }
    }

    async fn run_claim(&self, mut options: ClaimOptions) -> Result<bool> {
        if self.0.shutdown.is_cancelled() {
            return Err(Error::Cancelled);
        }
        options.worker_id.clone_from(&self.0.options.worker_id);
        let now = self.0.clock.now().0;
        let claim = self
            .0
            .stores
            .transaction(move |tx| outbox::claim_next_reporting(tx, &options, &now))
            .await?;
        for skipped in claim.undecodable {
            eprintln!(
                "effect worker could not decode claimed effect {} for {:?}: {}",
                skipped.effect_id, skipped.thread_id, skipped.detail
            );
        }
        let row = claim.effect;
        let Some(row) = row else { return Ok(false) };
        let cancellation = self.0.signals.cancellation(&row.id, &self.0.shutdown);
        let id = row.id.clone();
        let current = match self.0.stores.read(move |db| outbox::get(db, &id)).await {
            Ok(current) => current,
            Err(error) => {
                self.recover_claim(&row, &error, false).await;
                return Err(error);
            }
        };
        if current
            .as_ref()
            .is_some_and(|row| row.status == EffectStatus::Cancelled)
            || cancellation.is_cancelled()
        {
            return Ok(true);
        }
        if !current.as_ref().is_some_and(|current| {
            current.status == EffectStatus::Running && current.lease_owner == row.lease_owner
        }) {
            return Err(self.lease_lost(&row));
        }
        let execution = self.execute(&row, cancellation.clone()).await;
        if cancellation.is_cancelled() || self.0.shutdown.is_cancelled() {
            return Ok(true);
        }
        let result = self.settle(&row, execution).await;
        self.0.signals.notify();
        result.map(|()| true)
    }

    async fn execute(&self, row: &EffectRow, cancellation: Arc<CancellationToken>) -> Result<()> {
        let executor = self.0.executor.clone();
        let effect = row.clone();
        let token = cancellation.child_token();
        let operation = token.clone();
        let options = EffectExecutionOptions {
            will_retry: row.attempt_count < self.0.options.max_attempts,
        };
        let mut execution =
            tokio::spawn(
                async move { executor.execute_with_options(effect, token, options).await },
            );
        let deadline = matches!(row.request, EffectRequest::StopDelegated { .. });
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                let _ = execution.await;
                Err(Error::Cancelled)
            },
            result = &mut execution => result.map_err(task_error)?,
            _ = tokio::time::sleep(Duration::from_secs(30)), if deadline => {
                operation.cancel();
                let _ = execution.await;
                // Timeout cancels the operation, but still follows retry/fail settlement.
                Err(Error::Service { operation: "delegated-tasks.stop", detail: "Timed out after 30s.".into() })
            },
        }
    }

    async fn settle(&self, row: &EffectRow, outcome: Result<()>) -> Result<()> {
        let terminal = outcome.as_ref().map_or_else(
            |error| {
                non_retryable_interrupt(&row.request, &error.to_string())
                    || row.attempt_count >= self.0.options.max_attempts
            },
            |()| true,
        );
        let now = self.0.clock.now().0;
        let id = row.id.clone();
        let owner = self.0.options.worker_id.clone();
        let updated = match outcome {
            Ok(()) => match self
                .0
                .stores
                .transaction(move |tx| outbox::succeed(tx, &id, &owner, &now))
                .await
            {
                Ok(updated) => updated,
                Err(error) => {
                    self.recover_claim(row, &error, true).await;
                    return Err(error);
                }
            },
            Err(error) => {
                let text = error.to_string();
                let non_retryable = non_retryable_interrupt(&row.request, &text);
                let will_retry = !non_retryable && row.attempt_count < self.0.options.max_attempts;
                if !non_retryable {
                    let hook = tokio::select! {
                        biased;
                        _ = self.0.shutdown.cancelled() => return Err(Error::Cancelled),
                        hook = self.0.executor.failed(row.clone(), &error, will_retry) => hook,
                    };
                    if let Err(hook_error) = hook {
                        eprintln!(
                            "effect worker failure handler failed for {}: {hook_error}",
                            row.id
                        );
                    }
                }
                let attempts = row.attempt_count;
                let result = self
                    .0
                    .stores
                    .transaction(move |tx| {
                        if non_retryable {
                            outbox::succeed(tx, &id, &owner, &now)
                        } else if !will_retry {
                            outbox::fail(tx, &id, &owner, &text, &now)
                        } else {
                            let available = after_millis(tx, &now, retry_delay_ms(attempts))?;
                            outbox::retry(tx, &id, &owner, &text, &now, &available)
                        }
                    })
                    .await;
                match result {
                    Ok(updated) => updated,
                    Err(error) => {
                        self.recover_claim(row, &error, !will_retry).await;
                        return Err(error);
                    }
                }
            }
        };
        if updated {
            if terminal {
                self.0.signals.signal_settled(std::slice::from_ref(&row.id));
            }
            return Ok(());
        }
        let id = row.id.clone();
        if self
            .0
            .stores
            .read(move |db| {
                Ok(outbox::get(db, &id)?.is_some_and(|row| row.status == EffectStatus::Cancelled))
            })
            .await?
        {
            return Ok(());
        }
        Err(self.lease_lost(row))
    }

    fn lease_lost(&self, row: &EffectRow) -> Error {
        Error::LeaseLost {
            effect_id: row.id.clone(),
            worker_id: self.0.options.worker_id.clone(),
        }
    }

    async fn recover_claim(&self, row: &EffectRow, error: &Error, executed: bool) {
        let id = row.id.clone();
        let owner = self.0.options.worker_id.clone();
        let now = self.0.clock.now().0;
        let process_bound = executed && row.request.process_bound();
        let text = if process_bound {
            format!(
                "Worker failed to settle a process-bound effect after execution started: {error}"
            )
        } else {
            format!("Worker failed before settling the claimed effect: {error}")
        };
        let recovered = self
            .0
            .stores
            .transaction(move |tx| {
                if process_bound {
                    outbox::fail(tx, &id, &owner, &text, &now)
                } else {
                    outbox::retry(tx, &id, &owner, &text, &now, &now)
                }
            })
            .await;
        if process_bound && matches!(recovered, Ok(true)) {
            self.0.signals.signal_settled(std::slice::from_ref(&row.id));
        }
        self.0.signals.notify();
    }

    pub async fn prune_settled(&self) -> Result<usize> {
        let now = self.0.clock.now().0;
        let mut count = 0;
        loop {
            let now = now.clone();
            let batch = self
                .0
                .stores
                .transaction(move |tx| {
                    let cutoff: String = tx.query_row(
                        "SELECT strftime('%Y-%m-%dT%H:%M:%fZ',?1,'-7 days')",
                        [&now],
                        |row| row.get(0),
                    )?;
                    outbox::prune_settled_batch(tx, &cutoff)
                })
                .await?;
            count += batch;
            if batch < 500 {
                return Ok(count);
            }
        }
    }
}

#[async_trait]
impl WorkerApi for EffectWorker {
    async fn run_once(&self, options: ClaimOptions) -> Result<bool> {
        let handle = {
            let admission = self.0.admission.lock().unwrap_or_else(|e| e.into_inner());
            if !*admission {
                return Err(Error::Cancelled);
            }
            let worker = self.clone();
            self.0.running.fetch_add(1, Ordering::AcqRel);
            let completion = Completion(self.0.clone());
            self.0.executions.spawn(async move {
                let _completion = completion;
                worker.run_claim(options).await
            })
        };
        handle.await.map_err(task_error)?
    }

    async fn run_recovery_once(&self) -> Result<bool> {
        self.run_once(ClaimOptions {
            exclude_restart_continuations: true,
            ..Default::default()
        })
        .await
    }

    async fn drain(&self, max_effects: usize) -> Result<usize> {
        let mut completed = 0;
        loop {
            let mut changed = self.0.signals.subscribe();
            if self.0.shutdown.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if completed < max_effects && self.run_once(ClaimOptions::default()).await? {
                completed += 1;
                continue;
            }
            let unsettled = self.0.stores.read(|db| {
                let mut statement = db.prepare("SELECT effect_id FROM orchestration_v2_effect_outbox WHERE status='running'")?;
                for id in statement.query_map([], |row| row.get::<_, String>(0))? {
                    // An undecodable claim has no execution that could wake this drain.
                    outbox::get(db, &id?)?;
                }
                Ok(db.query_row("SELECT COUNT(*) FROM orchestration_v2_effect_outbox WHERE status IN ('pending','running')", [], |row| row.get::<_, i64>(0))?)
            }).await?;
            if self.executing() == 0
                && self.0.signals.pending_batch_jobs() == 0
                && (unsettled == 0 || completed >= max_effects)
            {
                return Ok(completed);
            }
            let delay = self.wake_delay(None).await?;
            tokio::select! {
                _ = self.0.shutdown.cancelled() => return Err(Error::Cancelled),
                _ = changed.changed() => {},
                _ = tokio::time::sleep(delay), if completed < max_effects => {},
            }
        }
    }

    async fn await_work(&self) -> Result<()> {
        let mut changed = self.0.signals.subscribe();
        if self.next_claimable_at(None).await?.is_some() {
            return Ok(());
        }
        tokio::select! {
            _ = self.0.shutdown.cancelled() => Err(Error::Cancelled),
            _ = changed.changed() => Ok(()),
        }
    }

    async fn await_settled(&self, effect_id: String) -> Result<()> {
        let signal = self.0.signals.settlement(&effect_id);
        let mut changed = signal.subscribe();
        let id = effect_id.clone();
        let unsettled = self
            .0
            .stores
            .read(move |db| {
                Ok(outbox::get(db, &id)?.is_some_and(|row| {
                    matches!(row.status, EffectStatus::Pending | EffectStatus::Running)
                }))
            })
            .await?;
        if !unsettled {
            self.0
                .signals
                .signal_settled(std::slice::from_ref(&effect_id));
            return Ok(());
        }
        loop {
            if *changed.borrow_and_update() {
                return Ok(());
            }
            tokio::select! {
                _ = self.0.shutdown.cancelled() => return Err(Error::Cancelled),
                _ = changed.changed() => {},
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        *self.0.admission.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.0.shutdown.cancel();
        self.0.executions.close();
        self.0.executions.wait().await;
        self.0.signals.notify();
        Ok(())
    }
}

pub fn non_retryable_interrupt(request: &EffectRequest, error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    matches!(request, EffectRequest::Interrupt { .. })
        && [
            "is not active",
            "hard teardown is already in progress",
            "treating as already interrupted",
            "treating as already stopped",
        ]
        .iter()
        .any(|text| error.contains(text))
}

pub fn retry_delay_ms(attempt_count: i64) -> i64 {
    100_i64
        .saturating_mul(1_i64 << attempt_count.saturating_sub(1).clamp(0, 9))
        .min(30_000)
}

fn after_millis(
    db: &t3_persistence::rusqlite::Connection,
    now: &str,
    delay: i64,
) -> Result<String> {
    Ok(db.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%fZ',?1,printf('%+.3f seconds',?2/1000.0))",
        (now, delay),
        |row| row.get(0),
    )?)
}

pub(crate) fn task_error(error: tokio::task::JoinError) -> Error {
    Error::Service {
        operation: "task",
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        effects::PendingEffect,
        testing::{ManualClock, TEST_TIME, TestDatabase},
    };
    use t3_contracts::base_schemas::{CommandId, RunId, ThreadId};

    struct Executor;

    #[async_trait]
    impl EffectExecutor for Executor {
        async fn execute(&self, _: EffectRow, _: CancellationToken) -> Result<()> {
            Ok(())
        }

        async fn failed(&self, _: EffectRow, _: &Error, _: bool) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn invalid_deadline_uses_liveness_delay_without_ending_the_daemon_slot() -> Result<()> {
        let fixture = TestDatabase::memory()?;
        let worker = EffectWorker::new(
            fixture.stores.clone(),
            Arc::new(Executor),
            Arc::new(ManualClock::new(TEST_TIME)),
            OutboxSignals::default(),
            WorkerOptions::default(),
        );
        fixture
            .stores
            .transaction(|tx| {
                outbox::enqueue(
                    tx,
                    &[PendingEffect {
                        id: "invalid-deadline".into(),
                        command_id: CommandId("command".into()),
                        thread_id: ThreadId("thread".into()),
                        request: EffectRequest::CleanupPreviews,
                        available_at: None,
                    }],
                    TEST_TIME,
                )?;
                tx.execute(
                    "UPDATE orchestration_v2_effect_outbox SET available_at='not-a-date'",
                    [],
                )?;
                Ok(())
            })
            .await?;
        assert!(worker.next_claimable_at(None).await.is_err());
        assert_eq!(
            worker.daemon_delay(None).await,
            worker.0.options.liveness_poll_interval
        );
        assert!(worker.drain(usize::MAX).await.is_err());
        worker.shutdown().await
    }

    #[tokio::test]
    async fn terminal_recovery_signals_registered_settlement_waiters() -> Result<()> {
        let fixture = TestDatabase::memory()?;
        let worker = EffectWorker::new(
            fixture.stores.clone(),
            Arc::new(Executor),
            Arc::new(ManualClock::new(TEST_TIME)),
            OutboxSignals::default(),
            WorkerOptions::default(),
        );
        let owner = worker.0.options.worker_id.clone();
        let row = fixture
            .stores
            .transaction(move |tx| {
                outbox::enqueue(
                    tx,
                    &[PendingEffect {
                        id: "recover".into(),
                        command_id: CommandId("command".into()),
                        thread_id: ThreadId("thread".into()),
                        request: EffectRequest::Start {
                            run_id: RunId("run".into()),
                        },
                        available_at: None,
                    }],
                    TEST_TIME,
                )?;
                Ok(outbox::claim_next(
                    tx,
                    &ClaimOptions {
                        worker_id: owner,
                        ..Default::default()
                    },
                    TEST_TIME,
                )?
                .expect("claim"))
            })
            .await?;
        let signal = worker.0.signals.settlement(&row.id);
        let waiting = signal.subscribe();
        worker
            .recover_claim(&row, &Error::Unsupported("settlement failed"), true)
            .await;
        assert_eq!(
            fixture
                .stores
                .read(|db| Ok(outbox::get(db, "recover")?.expect("row").status))
                .await?,
            EffectStatus::Failed
        );
        assert!(
            *waiting.borrow(),
            "durable terminal recovery must settle registered waiters"
        );
        fixture.stores.transaction(|tx| {
            tx.execute("UPDATE orchestration_v2_effect_outbox SET status='running', lease_owner='new-owner', completed_at=NULL, last_error=NULL WHERE effect_id='recover'", [])?;
            Ok(())
        }).await?;
        let signal = worker.0.signals.settlement(&row.id);
        let waiting = signal.subscribe();
        worker
            .recover_claim(&row, &Error::Unsupported("lost owner"), true)
            .await;
        assert!(
            !*waiting.borrow(),
            "losing ownership must not settle another worker's row"
        );
        assert_eq!(
            fixture
                .stores
                .read(|db| Ok(outbox::get(db, "recover")?.expect("row").lease_owner))
                .await?
                .as_deref(),
            Some("new-owner")
        );
        Ok(())
    }
}
