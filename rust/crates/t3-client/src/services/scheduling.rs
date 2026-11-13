use super::resources::LocalLimiter;
use crate::{ClientError, types::EnvironmentId};
use futures::{
    FutureExt,
    channel::oneshot,
    future::{LocalBoxFuture, Shared},
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::{Rc, Weak},
};

pub type ScheduledResult = Result<Rc<Value>, Rc<ClientError>>;
type Job = LocalBoxFuture<'static, ScheduledResult>;
type SharedJob = Shared<Job>;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommandLane {
    pub environment: EnvironmentId,
    pub group: String,
    pub arguments: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Concurrency {
    Parallel,
    Serial,
    SingleFlight,
    Latest,
}
struct LatestBatch {
    job: Job,
    waiting: Vec<oneshot::Sender<ScheduledResult>>,
}
#[derive(Default)]
struct LatestLane {
    pending: Option<LatestBatch>,
    driver: Option<SharedJob>,
    readers: Rc<Cell<usize>>,
}
#[derive(Default)]
struct SchedulerState {
    serial: BTreeMap<CommandLane, LocalLimiter>,
    single: BTreeMap<CommandLane, (u64, SharedJob, Rc<Cell<usize>>)>,
    latest: BTreeMap<CommandLane, Rc<RefCell<LatestLane>>>,
    next: u64,
}
#[derive(Clone, Default)]
pub struct CommandScheduler(Rc<RefCell<SchedulerState>>);
struct CommandConsumer {
    owner: Weak<RefCell<SchedulerState>>,
    lane: CommandLane,
    readers: Rc<Cell<usize>>,
    single: Option<u64>,
}
impl Drop for CommandConsumer {
    fn drop(&mut self) {
        self.readers.set(self.readers.get() - 1);
        if self.readers.get() != 0 {
            return;
        }
        if let Some(owner) = self.owner.upgrade() {
            let mut state = owner.borrow_mut();
            if let Some(id) = self.single {
                if state
                    .single
                    .get(&self.lane)
                    .is_some_and(|(current, _, _)| *current == id)
                {
                    state.single.remove(&self.lane);
                }
            } else if state
                .latest
                .get(&self.lane)
                .is_some_and(|lane| Rc::ptr_eq(&lane.borrow().readers, &self.readers))
            {
                state.latest.remove(&self.lane);
            }
        }
    }
}
fn follow_job(job: SharedJob, consumer: CommandConsumer) -> Job {
    async move {
        let _consumer = consumer;
        job.await
    }
    .boxed_local()
}
impl CommandScheduler {
    pub fn is_pending(&self, lane: &CommandLane) -> bool {
        self.0.borrow().single.contains_key(lane)
    }
    pub fn schedule(
        &self,
        lane: CommandLane,
        mode: Concurrency,
        execute: impl std::future::Future<Output = Result<Value, ClientError>> + 'static,
    ) -> LocalBoxFuture<'static, ScheduledResult> {
        let job = execute
            .map(|result| result.map(Rc::new).map_err(Rc::new))
            .boxed_local();
        match mode {
            Concurrency::Parallel => job,
            Concurrency::Serial => {
                let permit = self
                    .0
                    .borrow_mut()
                    .serial
                    .entry(lane)
                    .or_insert_with(|| LocalLimiter::new(1))
                    .acquire();
                async move {
                    let _permit = permit.await;
                    job.await
                }
                .boxed_local()
            }
            Concurrency::SingleFlight => {
                let mut state = self.0.borrow_mut();
                if let Some((id, shared, readers)) = state.single.get(&lane) {
                    readers.set(readers.get() + 1);
                    return follow_job(
                        shared.clone(),
                        CommandConsumer {
                            owner: Rc::downgrade(&self.0),
                            lane,
                            readers: readers.clone(),
                            single: Some(*id),
                        },
                    );
                }
                state.next += 1;
                let id = state.next;
                let owner = Rc::downgrade(&self.0);
                let key = lane.clone();
                let shared = async move {
                    let result = job.await;
                    if let Some(owner) = owner.upgrade() {
                        let mut state = owner.borrow_mut();
                        if state
                            .single
                            .get(&key)
                            .is_some_and(|(current, _, _)| *current == id)
                        {
                            state.single.remove(&key);
                        }
                    }
                    result
                }
                .boxed_local()
                .shared();
                let readers = Rc::new(Cell::new(1));
                state
                    .single
                    .insert(lane.clone(), (id, shared.clone(), readers.clone()));
                follow_job(
                    shared,
                    CommandConsumer {
                        owner: Rc::downgrade(&self.0),
                        lane,
                        readers,
                        single: Some(id),
                    },
                )
            }
            Concurrency::Latest => {
                let (tx, rx) = oneshot::channel();
                let slot = self
                    .0
                    .borrow_mut()
                    .latest
                    .entry(lane.clone())
                    .or_default()
                    .clone();
                let readers = slot.borrow().readers.clone();
                readers.set(readers.get() + 1);
                let consumer = CommandConsumer {
                    owner: Rc::downgrade(&self.0),
                    lane: lane.clone(),
                    readers,
                    single: None,
                };
                let driver = {
                    let mut slot = slot.borrow_mut();
                    if let Some(pending) = &mut slot.pending {
                        pending.job = job;
                        pending.waiting.push(tx);
                    } else {
                        slot.pending = Some(LatestBatch {
                            job,
                            waiting: vec![tx],
                        });
                    }
                    slot.driver.clone()
                };
                let driver = driver.unwrap_or_else(|| {
                    let owner = Rc::downgrade(&self.0);
                    let weak_slot = Rc::downgrade(&slot);
                    let driver = async move {
                        loop {
                            let batch = weak_slot
                                .upgrade()
                                .and_then(|slot| slot.borrow_mut().pending.take());
                            let Some(batch) = batch else {
                                break;
                            };
                            let result = batch.job.await;
                            for waiting in batch.waiting {
                                let _result = waiting.send(result.clone());
                            }
                        }
                        if let Some(owner) = owner.upgrade() {
                            owner.borrow_mut().latest.remove(&lane);
                        }
                        Ok(Rc::new(Value::Null))
                    }
                    .boxed_local()
                    .shared();
                    slot.borrow_mut().driver = Some(driver.clone());
                    driver
                });
                async move {
                    let _consumer = consumer;
                    futures::pin_mut!(rx);
                    let mut driver = driver;
                    futures::future::poll_fn(|cx| {
                        use std::{future::Future, task::Poll};
                        if let Poll::Ready(result) = rx.as_mut().poll(cx) {
                            return Poll::Ready(
                                result.unwrap_or_else(|_| Err(Rc::new(ClientError::Cancelled))),
                            );
                        }
                        let _progress = std::pin::Pin::new(&mut driver).poll(cx);
                        rx.as_mut().poll(cx).map(|result| {
                            result.unwrap_or_else(|_| Err(Rc::new(ClientError::Cancelled)))
                        })
                    })
                    .await
                }
                .boxed_local()
            }
        }
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        let mut state = self.0.borrow_mut();
        state
            .serial
            .retain(|key, _| &key.environment != environment);
        state
            .single
            .retain(|key, _| &key.environment != environment);
        state
            .latest
            .retain(|key, _| &key.environment != environment);
    }
}
