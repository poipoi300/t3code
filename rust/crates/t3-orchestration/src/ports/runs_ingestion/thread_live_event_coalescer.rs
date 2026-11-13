use crate::{
    api::OrchestrationV2ThreadStreamItem,
    error::{Error, Result},
    ports::transport::live_stream_budget::{
        LiveBuffer, LiveStreamLimits, LiveStreamUsage, SharedLiveItem,
    },
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
fn update_key(input: &OrchestrationV2ThreadStreamItem) -> Result<Option<(String, String)>> {
    let OrchestrationV2ThreadStreamItem::Event(input) = input else {
        return Ok(None);
    };
    let event = t3_contracts::stored::to_value(&input.event)?;
    let kind = event["type"].as_str().unwrap_or_default();
    let payload = &event["payload"];
    if kind == "message.updated" {
        if payload["role"] != "assistant" || payload["streaming"] != true {
            return Ok(None);
        }
    } else if kind == "turn-item.updated" && payload["status"] == "running" {
        match payload["type"].as_str() {
            Some("assistant_message" | "reasoning") if payload["streaming"] == true => {}
            Some(
                "command_execution" | "file_change" | "file_search" | "web_search" | "dynamic_tool",
            ) => {}
            _ => return Ok(None),
        }
    } else {
        return Ok(None);
    }
    Ok(Some((
        kind.into(),
        format!(
            "{}\0{}\0{kind}\0{}\0{}",
            event["threadId"].as_str().unwrap_or_default(),
            event["runId"].as_str().unwrap_or_default(),
            payload["type"].as_str().unwrap_or_default(),
            payload["id"].as_str().unwrap_or_default()
        ),
    )))
}
pub fn coalesce_live_updated_events(
    events: Vec<OrchestrationV2ThreadStreamItem>,
) -> Result<Vec<OrchestrationV2ThreadStreamItem>> {
    let mut latest = HashMap::new();
    let mut superseded = HashSet::new();
    let mut families = HashMap::new();
    for (index, event) in events.iter().enumerate() {
        let Some((family, key)) = update_key(event)? else {
            latest.clear();
            families.clear();
            continue;
        };
        if families
            .get(&family)
            .is_some_and(|previous| previous != &key)
        {
            latest.clear();
        }
        families.insert(family, key.clone());
        if let Some(previous) = latest.insert(key, index) {
            superseded.insert(previous);
        }
    }
    Ok(events
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !superseded.contains(index))
        .map(|(_, event)| event)
        .collect())
}
#[derive(Default)]
struct Pending {
    serial: u64,
    latest: HashMap<String, String>,
    families: HashMap<String, String>,
    keys: HashSet<String>,
    window: Option<CancellationToken>,
    ended: bool,
}
pub struct ThreadLiveEventCoalescer {
    buffer: LiveBuffer<OrchestrationV2ThreadStreamItem>,
    pending: Mutex<Pending>,
    tasks: TaskTracker,
    cancellation: CancellationToken,
    closed: AtomicBool,
    upstream_failed: AtomicBool,
    failure: Mutex<Option<Error>>,
}
impl ThreadLiveEventCoalescer {
    pub fn new(limits: LiveStreamLimits) -> Arc<Self> {
        Arc::new(Self {
            buffer: LiveBuffer::new(limits),
            pending: Mutex::new(Pending::default()),
            tasks: TaskTracker::new(),
            cancellation: CancellationToken::new(),
            closed: AtomicBool::new(false),
            upstream_failed: AtomicBool::new(false),
            failure: Mutex::new(None),
        })
    }
    fn flush_locked(&self, pending: &mut Pending) {
        if let Some(window) = pending.window.take() {
            window.cancel();
        }
        pending.latest.clear();
        pending.families.clear();
        pending.keys.clear();
        self.buffer.flush_pending();
    }
    pub fn offer(self: &Arc<Self>, input: OrchestrationV2ThreadStreamItem) -> Result<()> {
        self.offer_all(vec![input])
    }
    pub fn offer_all(self: &Arc<Self>, inputs: Vec<OrchestrationV2ThreadStreamItem>) -> Result<()> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending.ended || self.closed.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        for input in inputs {
            let starts_window = pending.keys.is_empty();
            pending.serial += 1;
            let serial = pending.serial;
            let key = update_key(&input)?;
            if let Some((family, key)) = key {
                if pending
                    .families
                    .get(&family)
                    .is_some_and(|previous| previous != &key)
                {
                    pending.latest.clear();
                }
                pending.families.insert(family, key.clone());
                let buffer_key = pending
                    .latest
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| format!("pending:{serial}"));
                self.buffer
                    .push_deferred(SharedLiveItem::new(input, serial)?, buffer_key.clone())?;
                pending.keys.insert(buffer_key.clone());
                pending.latest.insert(key, buffer_key);
                if starts_window {
                    let token = self.cancellation.child_token();
                    pending.window = Some(token.clone());
                    let coalescer = Arc::downgrade(self);
                    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
                    self.tasks.spawn(async move{tokio::select!{biased;_=token.cancelled()=>{},_=tokio::time::sleep_until(deadline)=>{if let Some(coalescer)=coalescer.upgrade(){let mut pending=coalescer.pending.lock().unwrap_or_else(|error| error.into_inner());if !token.is_cancelled(){coalescer.flush_locked(&mut pending);}}}}});
                }
                if pending.keys.len() >= 512 {
                    self.flush_locked(&mut pending);
                }
            } else {
                self.buffer.push_deferred(
                    SharedLiveItem::new(input, serial)?,
                    format!("fifo:{serial}"),
                )?;
                self.flush_locked(&mut pending);
            }
        }
        Ok(())
    }
    pub fn end(&self) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending.ended {
            return;
        }
        pending.ended = true;
        self.flush_locked(&mut pending);
        self.buffer.close();
    }
    pub fn fail(&self, error: Error) {
        {
            let mut failure = self
                .failure
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if self.closed.load(Ordering::Acquire) || self.upstream_failed.load(Ordering::Acquire) {
                return;
            }
            *failure = Some(error);
            self.upstream_failed.store(true, Ordering::Release);
        }
        self.cancellation.cancel();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pending.ended = true;
        if let Some(window) = pending.window.take() {
            window.cancel();
        }
        pending.latest.clear();
        pending.families.clear();
        pending.keys.clear();
        self.buffer.fail();
        self.buffer.acknowledge();
    }
    pub fn consume_source(
        self: &Arc<Self>,
        mut source: tokio::sync::mpsc::Receiver<Result<Vec<OrchestrationV2ThreadStreamItem>>>,
    ) {
        let coalescer = Arc::downgrade(self);
        let cancellation = self.cancellation.clone();
        self.tasks.spawn(async move {loop {tokio::select! {biased;
            _=cancellation.cancelled()=>break,
            batch=source.recv()=>{let Some(coalescer)=coalescer.upgrade()else{break;};match batch {
                Some(Ok(batch))=>if let Err(error)=coalescer.offer_all(batch){coalescer.fail(error);break;},
                Some(Err(error))=>{coalescer.fail(error);break;},
                None=>{coalescer.end();break;},
            }}
        }}});
    }
    pub async fn pull(
        &self,
    ) -> Result<Option<Vec<Arc<SharedLiveItem<OrchestrationV2ThreadStreamItem>>>>> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        if self.upstream_failed.load(Ordering::Acquire) {
            return self
                .failure
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .map_or(Ok(None), Err);
        }
        let result = self.buffer.pull().await;
        if self.closed.load(Ordering::Acquire) {
            Ok(None)
        } else if self.upstream_failed.load(Ordering::Acquire) {
            self.failure
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .map_or(Ok(None), Err)
        } else {
            result
        }
    }
    pub fn acknowledge(&self) {
        self.buffer.acknowledge();
    }
    pub fn usage(&self) -> LiveStreamUsage {
        self.buffer.usage()
    }
    pub async fn failed(&self) -> bool {
        self.buffer.failed().await;
        !self.closed.load(Ordering::Acquire)
    }
    pub async fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.cancellation.cancel();
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending.ended = true;
            self.flush_locked(&mut pending);
            self.buffer.fail();
            self.buffer.acknowledge();
        }
        self.tasks.close();
        self.tasks.wait().await;
    }
}

impl Drop for ThreadLiveEventCoalescer {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.tasks.close();
    }
}
