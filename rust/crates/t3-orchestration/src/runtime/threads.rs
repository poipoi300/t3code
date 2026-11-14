/// Millisecond comparisons never rewrite persisted date strings.
pub(crate) fn date_ms(value: &str) -> Option<i64> {
    let (date, time) = value.split_once('T')?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (time, offset) = if let Some(time) = time.strip_suffix('Z') {
        (time, 0)
    } else {
        let position = time.rfind(['+', '-'])?;
        let zone = &time[position + 1..];
        let (hours, minutes) = zone.split_once(':')?;
        let minutes = hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?;
        (
            &time[..position],
            if time.as_bytes()[position] == b'+' {
                minutes
            } else {
                -minutes
            },
        )
    };
    let mut clock = time.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let seconds = clock.next()?;
    if clock.next().is_some() || !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let second: i64 = seconds.parse().ok()?;
    if !(0..=59).contains(&second) || !fraction.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let millis = fraction
        .bytes()
        .take(3)
        .enumerate()
        .map(|(i, c)| i64::from(c - b'0') * [100, 10, 1][i])
        .sum::<i64>();
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let days = era * 146097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year
        - 719468;
    Some(((days * 24 + hour) * 60 + minute - offset) * 60000 + second * 1000 + millis)
}

use crate::error::{Error, Result};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};
use tokio::sync::{Mutex, Semaphore, watch};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) async fn join_thread_reads<'a>(
    reads: impl IntoIterator<Item = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>>,
) -> Result<()> {
    let mut reads: Vec<_> = reads.into_iter().map(Some).collect();
    std::future::poll_fn(|cx| {
        let mut all_ready = true;
        for read in &mut reads {
            if let Some(future) = read {
                match future.as_mut().poll(cx) {
                    std::task::Poll::Ready(Ok(())) => *read = None,
                    std::task::Poll::Ready(Err(error)) => {
                        return std::task::Poll::Ready(Err(error));
                    }
                    std::task::Poll::Pending => all_ready = false,
                }
            }
        }
        if all_ready {
            std::task::Poll::Ready(Ok(()))
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}

pub(crate) fn normalize_pull_request_key(
    registry: &dyn crate::projections::write::PullRequestIdentityRegistry,
    key: &t3_contracts::thread_pull_request::ThreadPullRequestKey,
    url: Option<&str>,
) -> Result<t3_contracts::thread_pull_request::ThreadPullRequestKey> {
    let repository = key.repository.0.trim().to_lowercase();
    let authority = url
        .and_then(|url| registry.parse_change_request_url(url))
        .filter(|parsed| parsed.key.repository.0 == repository && parsed.key.number == key.number)
        .and_then(|parsed| parsed.authority);
    let host = authority
        .as_deref()
        .unwrap_or(&key.host.0)
        .trim()
        .to_lowercase();
    let canonical = registry.canonical_repository_key(&format!("{host}/{repository}"));
    let (host, repository) = canonical.split_once('/').ok_or_else(|| Error::Decode {
        entity: "canonical pull request identity",
        detail: canonical.clone(),
    })?;
    Ok(serde_json::from_value(
        serde_json::json!({"host":host,"repository":repository,"number":key.number}),
    )?)
}

#[derive(Default)]
pub struct ThreadServiceScope {
    closed: std::sync::Mutex<bool>,
    cancellation: CancellationToken,
    operations: TaskTracker,
}

impl ThreadServiceScope {
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.child_token()
    }

    pub async fn run<T>(&self, operation: impl Future<Output = Result<T>>) -> Result<T> {
        let _operation = {
            let closed = self
                .closed
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if *closed {
                return Err(Error::Cancelled);
            }
            self.operations.token()
        };
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(Error::Cancelled),
            result = operation => result,
        }
    }

    pub fn close(&self) {
        let mut closed = self
            .closed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *closed = true;
        self.cancellation.cancel();
        self.operations.close();
    }

    pub async fn shutdown(&self) {
        self.close();
        self.operations.wait().await;
    }
}

type Work<J> =
    dyn Fn(J, CancellationToken) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync;
type Merge<J> = dyn Fn(&mut J, J) + Send + Sync;

struct WorkerState<K, J> {
    pending: BTreeMap<K, Option<J>>,
    closed: bool,
}

struct WorkerInner<K, J> {
    state: Mutex<WorkerState<K, J>>,
    slots: Semaphore,
    changed: watch::Sender<()>,
    tasks: TaskTracker,
    cancellation: CancellationToken,
    work: Box<Work<J>>,
    merge: Box<Merge<J>>,
}

/// Jobs coalesce until admission; changes during execution retain one follow-up.
pub struct ThreadProjectWorker<K, J>(Arc<WorkerInner<K, J>>);

impl<K, J> Clone for ThreadProjectWorker<K, J> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K: Ord + Clone + Send + 'static, J: Send + 'static> ThreadProjectWorker<K, J> {
    pub fn new<F, Fut>(
        concurrency: usize,
        merge: impl Fn(&mut J, J) + Send + Sync + 'static,
        work: F,
    ) -> Self
    where
        F: Fn(J, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self(Arc::new(WorkerInner {
            state: Mutex::new(WorkerState {
                pending: BTreeMap::new(),
                closed: false,
            }),
            slots: Semaphore::new(concurrency.max(1)),
            changed: watch::channel(()).0,
            tasks: TaskTracker::new(),
            cancellation: CancellationToken::new(),
            work: Box::new(move |job, cancellation| Box::pin(work(job, cancellation))),
            merge: Box::new(merge),
        }))
    }

    pub async fn enqueue(&self, key: K, job: J) -> Result<()> {
        let mut state = self.0.state.lock().await;
        if state.closed {
            return Err(Error::Cancelled);
        }
        if let Some(next) = state.pending.get_mut(&key) {
            match next {
                Some(next) => (self.0.merge)(next, job),
                None => *next = Some(job),
            }
            return Ok(());
        }
        state.pending.insert(key.clone(), Some(job));
        let inner = self.0.clone();
        self.0.tasks.spawn(async move {
            loop {
                let permit = tokio::select! {
                    _ = inner.cancellation.cancelled() => break,
                    permit = inner.slots.acquire() => match permit { Ok(permit) => permit, Err(_) => break },
                };
                let job = inner.state.lock().await.pending.get_mut(&key).and_then(Option::take);
                if let Some(job) = job {
                    let mut execution = tokio::spawn((inner.work)(job, inner.cancellation.child_token()));
                    tokio::select! {
                        _ = inner.cancellation.cancelled() => { execution.abort(); let _ = execution.await; },
                        result = &mut execution => { if let Err(error) = result { eprintln!("thread/project background job failed: {error}"); } },
                    }
                }
                drop(permit);
                let mut state = inner.state.lock().await;
                if inner.cancellation.is_cancelled() || state.pending.get(&key).is_none_or(Option::is_none) {
                    state.pending.remove(&key);
                    inner.changed.send_replace(());
                    return;
                }
            }
            inner.state.lock().await.pending.remove(&key);
            inner.changed.send_replace(());
        });
        Ok(())
    }

    pub async fn drain(&self) {
        let mut changed = self.0.changed.subscribe();
        loop {
            if self.0.state.lock().await.pending.is_empty() {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    pub async fn shutdown(&self) {
        self.0.state.lock().await.closed = true;
        self.0.cancellation.cancel();
        self.0.tasks.close();
        self.0.tasks.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{ThreadProjectWorker, date_ms};

    #[test]
    fn dates_compare_across_offsets_leap_days_and_the_epoch() {
        for (date, expected) in [
            ("1970-01-01T00:00:00.000Z", 0),
            ("1969-12-31T23:59:59.999Z", -1),
            ("1970-01-01T00:00:00+01:00", -3600000),
            ("1970-01-01T00:00:00-01:00", 3600000),
            ("2000-03-01T00:00:00.001Z", 951868800001),
            ("2026-10-02T12:00:00Z", 1790942400000),
        ] {
            assert_eq!(date_ms(date), Some(expected), "{date}");
        }
        assert_eq!(date_ms(""), None);
        assert_eq!(date_ms("not a date"), None);
    }

    #[tokio::test]
    async fn worker_coalesces_reruns_keeps_other_keys_live_and_drains_executing_work() {
        let (calls, mut received) = tokio::sync::mpsc::unbounded_channel();
        let worker = ThreadProjectWorker::new(
            2,
            |current: &mut u64, next| *current = next,
            move |job, _| {
                let calls = calls.clone();
                async move {
                    let (complete, completed) = tokio::sync::oneshot::channel();
                    calls.send((job, complete)).unwrap();
                    let _ = completed.await;
                }
            },
        );
        worker.enqueue("a", 1).await.unwrap();
        let (job, first) = received.recv().await.unwrap();
        assert_eq!(job, 1);
        worker.enqueue("a", 2).await.unwrap();
        worker.enqueue("a", 3).await.unwrap();
        worker.enqueue("b", 4).await.unwrap();
        let (job, second) = received.recv().await.unwrap();
        assert_eq!(job, 4);
        second.send(()).unwrap();
        let draining = tokio::spawn({
            let worker = worker.clone();
            async move { worker.drain().await }
        });
        assert!(!draining.is_finished());
        first.send(()).unwrap();
        let (job, rerun) = received.recv().await.unwrap();
        assert_eq!(job, 3);
        assert!(!draining.is_finished());
        rerun.send(()).unwrap();
        draining.await.unwrap();
        assert!(received.try_recv().is_err());
        worker.shutdown().await;
        assert!(worker.enqueue("c", 5).await.is_err());
    }
}
