use crate::{
    api::{EventCommitter, ProjectionReader},
    error::{Error, Result},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use t3_contracts::{base_schemas::*, orchestration_v2::*};
use tokio::{
    sync::{Notify, broadcast},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

pub use t3_shared::terminal_output;
pub use terminal_output::NativeTerminalOutputFold;
use terminal_output::{TerminalOutput, terminal_output_resume_text};

pub const TERMINAL_OUTPUT_TAIL_CHARS: usize = 64 * 1024;
const MAX_APPEND_CHARS: usize = 16 * 1024;
const MAX_FINAL_SOURCE_CHARS: usize = TERMINAL_OUTPUT_TAIL_CHARS * 4;

pub use terminal_output::TerminalText;

/// Opaque state is shared with the replay recorder; NativeTerminalOutputFold is the canonical fold.
pub trait TerminalOutputFold: Send + Sync {
    fn empty(&self) -> Value;
    fn append(&self, state: &Value, chunk: &str) -> Result<Value>;
    fn normalize(&self, text: &str) -> Result<TerminalText>;
    fn resume(&self, state: &Value) -> Result<TerminalText>;
    fn settled(&self, state: &Value) -> Result<TerminalText>;
}
type Key = (ThreadId, TurnItemId);
/// One live fold per command. Its own lock serialises chunks and frames for that command, so a
/// costly chunk never holds the hub's lock, which covers only the maps.
type Output = Arc<Mutex<TerminalOutput>>;
struct Tail {
    output: Output,
    written: u64,
}
#[derive(Default)]
struct Pending {
    append: String,
    replace: bool,
    overflowed: bool,
    snapshot_output: Option<String>,
    settled: Option<TurnItemCommandExecution>,
    lost: bool,
}
struct Subscriber {
    pending: Mutex<Pending>,
    wake: Notify,
}
#[derive(Default)]
struct HubState {
    tails: BTreeMap<Key, Tail>,
    finished: BTreeMap<Key, Tail>,
    subscribers: BTreeMap<Key, Vec<Weak<Subscriber>>>,
    serial: u64,
}
struct HubInner {
    state: Mutex<HubState>,
    /// Snapshots rendered for frames: appending never renders the retained output.
    snapshots: AtomicU64,
    terminal: Arc<dyn TerminalOutputFold>,
    projections: Arc<dyn ProjectionReader>,
    events: Arc<dyn EventCommitter>,
}
#[derive(Clone)]
pub struct CommandOutputHub(Arc<HubInner>);
impl CommandOutputHub {
    pub fn new(
        projections: Arc<dyn ProjectionReader>,
        events: Arc<dyn EventCommitter>,
        terminal: Arc<dyn TerminalOutputFold>,
    ) -> Self {
        Self(Arc::new(HubInner {
            state: Mutex::new(HubState::default()),
            snapshots: AtomicU64::new(0),
            projections,
            events,
            terminal,
        }))
    }
    pub async fn append(
        &self,
        target: OrchestrationV2SubscribeCommandOutputInput,
        chunk: String,
    ) -> Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        let key = (target.thread_id, target.item_id);
        let output = {
            let mut hub = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            hub.serial += 1;
            let written = hub.serial;
            let output = match hub.tails.get_mut(&key) {
                Some(tail) => {
                    tail.written = written;
                    tail.output.clone()
                }
                None => {
                    let output: Output =
                        Arc::new(Mutex::new(TerminalOutput::new(TERMINAL_OUTPUT_TAIL_CHARS)));
                    hub.tails.insert(
                        key.clone(),
                        Tail {
                            output: output.clone(),
                            written,
                        },
                    );
                    output
                }
            };
            if hub.tails.len() > 64
                && let Some(oldest) = hub
                    .tails
                    .iter()
                    .min_by_key(|(_, tail)| tail.written)
                    .map(|(key, _)| key.clone())
            {
                hub.tails.remove(&oldest);
                for subscriber in subscribers(&mut hub, &oldest) {
                    let mut pending = subscriber.pending.lock().unwrap_or_else(|e| e.into_inner());
                    pending.replace = true;
                    pending.append.clear();
                }
            }
            output
        };
        // Subscribers are read under the fold's lock, so a frame either includes this chunk or
        // receives it as an append.
        let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
        output.push(&chunk);
        let subscribers = {
            let mut hub = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
            subscribers(&mut hub, &key)
        };
        for subscriber in subscribers {
            let mut pending = subscriber.pending.lock().unwrap_or_else(|e| e.into_inner());
            if !pending.replace {
                if pending.append.encode_utf16().count() + chunk.encode_utf16().count()
                    > MAX_APPEND_CHARS
                {
                    pending.replace = true;
                    pending.overflowed = true;
                    pending.append.clear();
                } else {
                    pending.append.push_str(&chunk);
                }
            }
            subscriber.wake.notify_one();
        }
        drop(output);
        Ok(())
    }
    fn render(&self, output: &TerminalOutput) -> terminal_output::TerminalOutputState {
        self.0.snapshots.fetch_add(1, Ordering::Relaxed);
        output.snapshot()
    }
    pub async fn finish(&self, target: OrchestrationV2SubscribeCommandOutputInput) -> Result<()> {
        let key = (target.thread_id, target.item_id);
        let mut hub = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut tail) = hub.tails.remove(&key) {
            hub.serial += 1;
            tail.written = hub.serial;
            hub.finished.insert(key, tail);
            if hub.finished.len() > 16
                && let Some(oldest) = hub
                    .finished
                    .iter()
                    .min_by_key(|(_, tail)| tail.written)
                    .map(|(key, _)| key.clone())
            {
                hub.finished.remove(&oldest);
            }
        }
        Ok(())
    }
    pub async fn subscribe(
        &self,
        target: OrchestrationV2SubscribeCommandOutputInput,
    ) -> Result<CommandOutputSubscription> {
        let receiver = self.0.events.subscribe_shared()?;
        let subscriber = Arc::new(Subscriber {
            pending: Mutex::new(Pending {
                replace: true,
                ..Default::default()
            }),
            wake: Notify::new(),
        });
        let key = (target.thread_id.clone(), target.item_id.clone());
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subscribers
            .entry(key.clone())
            .or_default()
            .push(Arc::downgrade(&subscriber));
        let cancellation = CancellationToken::new();
        let mut stream = CommandOutputSubscription {
            hub: self.clone(),
            key,
            subscriber: subscriber.clone(),
            cancellation: cancellation.clone(),
            task: Some(tokio::spawn(observe_events(
                receiver,
                target.clone(),
                subscriber,
                cancellation,
            ))),
            last_frame: None,
            throttle: Duration::from_millis(100),
            ended: false,
        };
        let Some(item) = self
            .0
            .projections
            .turn_item(target.thread_id, target.item_id)
            .await?
        else {
            return Err(output_error("No such command."));
        };
        let OrchestrationV2TurnItem::CommandExecution(item) = item else {
            return Err(output_error("No such command."));
        };
        // Live observations win over a read that completed after their event was received.
        let mut pending = stream
            .subscriber
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if pending.settled.is_none() && pending.snapshot_output.is_none() {
            observe(&mut pending, *item);
        }
        drop(pending);
        stream.subscriber.wake.notify_one();
        if stream
            .subscriber
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lost
        {
            stream.ended = true;
            return Err(output_error("Lost the thread's event stream."));
        }
        Ok(stream)
    }
}
fn subscribers(hub: &mut HubState, key: &Key) -> Vec<Arc<Subscriber>> {
    let Some(entries) = hub.subscribers.get_mut(key) else {
        return vec![];
    };
    entries.retain(|entry| entry.strong_count() > 0);
    entries.iter().filter_map(Weak::upgrade).collect()
}
fn output_error(detail: &str) -> Error {
    Error::Service {
        operation: "CommandOutputHub",
        detail: detail.into(),
    }
}
fn observe(pending: &mut Pending, item: TurnItemCommandExecution) -> bool {
    let active = matches!(
        item.status,
        OrchestrationV2TurnItemStatus::Pending
            | OrchestrationV2TurnItemStatus::Running
            | OrchestrationV2TurnItemStatus::Waiting
    );
    if !active {
        pending.settled = Some(item);
    } else if let Some(output) = item.output {
        if pending.snapshot_output == output {
            return false;
        }
        pending.snapshot_output = output;
    } else {
        return false;
    }
    pending.replace = true;
    true
}
async fn observe_events(
    mut receiver: broadcast::Receiver<Arc<t3_contracts::application_event::ApplicationStoredEvent>>,
    target: OrchestrationV2SubscribeCommandOutputInput,
    subscriber: Arc<Subscriber>,
    cancel: CancellationToken,
) {
    use t3_contracts::application_event::ApplicationStoredEvent;
    loop {
        tokio::select! {
            biased;
            _=cancel.cancelled()=>break,
            received=receiver.recv()=>match received {
                Ok(event)=>if let ApplicationStoredEvent::Orchestration(event)=event.as_ref()
                    && let OrchestrationV2DomainEvent::TurnItemUpdated(event)=&event.event
                    && event.thread_id==target.thread_id
                    && let OrchestrationV2TurnItem::CommandExecution(item)=&event.payload
                    && item.id==t3_contracts::orchestration_v2_id_table::IdOrIndex::Id(target.item_id.clone()) {
                        let mut pending=subscriber.pending.lock().unwrap_or_else(|e|e.into_inner());
                        if pending.settled.is_none() && observe(&mut pending, *item.clone()) {subscriber.wake.notify_one();}
                    },
                Err(_)=>{subscriber.pending.lock().unwrap_or_else(|e|e.into_inner()).lost=true;subscriber.wake.notify_one();break;},
            }
        }
    }
}
pub struct CommandOutputSubscription {
    hub: CommandOutputHub,
    key: Key,
    subscriber: Arc<Subscriber>,
    cancellation: CancellationToken,
    task: Option<JoinHandle<()>>,
    last_frame: Option<Instant>,
    throttle: Duration,
    ended: bool,
}
impl CommandOutputSubscription {
    pub async fn pull(&mut self) -> Result<Option<OrchestrationV2CommandOutputFrame>> {
        if self.ended {
            return Ok(None);
        }
        loop {
            tokio::select! {biased;_=self.cancellation.cancelled()=>return Err(Error::Cancelled),_=self.subscriber.wake.notified()=>{}}
            if let Some(last) = self.last_frame {
                tokio::select! {biased;_=self.cancellation.cancelled()=>return Err(Error::Cancelled),_=tokio::time::sleep_until(last+self.throttle)=>{}}
            }
            self.last_frame = Some(Instant::now());
            self.throttle = Duration::from_millis(100);
            let output = {
                let hub = self.hub.0.state.lock().unwrap_or_else(|e| e.into_inner());
                hub.tails
                    .get(&self.key)
                    .or_else(|| hub.finished.get(&self.key))
                    .map(|tail| tail.output.clone())
            };
            let output = output
                .as_ref()
                .map(|output| output.lock().unwrap_or_else(|e| e.into_inner()));
            let mut pending = self
                .subscriber
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if pending.lost {
                self.ended = true;
                self.cancellation.cancel();
                return Err(output_error("Lost the thread's event stream."));
            }
            let frame = if let Some(item) = &pending.settled {
                let text = if let (None, Some(output)) = (&item.output, &output) {
                    let state = self.hub.render(output);
                    TerminalText {
                        text: state.text,
                        truncated: state.truncated,
                    }
                } else {
                    persisted_output(
                        self.hub.0.terminal.as_ref(),
                        item.output
                            .as_ref()
                            .and_then(Option::as_deref)
                            .map(legacy_claude_output)
                            .as_deref(),
                    )?
                };
                self.ended = true;
                self.cancellation.cancel();
                frame(CommandOutputFrameKind::Replace, text, false)
            } else if pending.replace || output.is_none() {
                if pending.overflowed {
                    self.throttle = Duration::from_millis(500);
                }
                pending.replace = false;
                pending.overflowed = false;
                pending.append.clear();
                let text = if let Some(output) = &output {
                    let state = self.hub.render(output);
                    TerminalText {
                        text: terminal_output_resume_text(&state),
                        truncated: state.truncated,
                    }
                } else {
                    persisted_output(
                        self.hub.0.terminal.as_ref(),
                        pending.snapshot_output.as_deref(),
                    )?
                };
                frame(CommandOutputFrameKind::Replace, text, true)
            } else {
                let truncated = output.as_ref().is_some_and(|output| output.truncated());
                frame(
                    CommandOutputFrameKind::Append,
                    TerminalText {
                        text: std::mem::take(&mut pending.append),
                        truncated,
                    },
                    true,
                )
            };
            if frame.kind == CommandOutputFrameKind::Append && frame.text.is_empty() {
                continue;
            }
            return Ok(Some(frame));
        }
    }
    pub async fn close(mut self) -> Result<()> {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.await
                .map_err(|error| output_error(&error.to_string()))?;
        }
        Ok(())
    }
}
impl Drop for CommandOutputSubscription {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
        let mut hub = self.hub.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entries) = hub.subscribers.get_mut(&self.key) {
            entries.retain(|entry| {
                !entry.ptr_eq(&Arc::downgrade(&self.subscriber)) && entry.strong_count() > 0
            });
            if entries.is_empty() {
                hub.subscribers.remove(&self.key);
            }
        }
    }
}
fn frame(
    kind: CommandOutputFrameKind,
    text: TerminalText,
    running: bool,
) -> OrchestrationV2CommandOutputFrame {
    OrchestrationV2CommandOutputFrame {
        kind,
        text: text.text,
        truncated: text.truncated,
        running,
    }
}
fn legacy_claude_output(output: &str) -> String {
    if !output.trim_start().starts_with("{\"stdout\"") {
        return output.into();
    }
    let Ok(value) = serde_json::from_str::<Value>(output) else {
        return output.into();
    };
    let (Some(stdout), Some(stderr), Some(_)) = (
        value["stdout"].as_str(),
        value["stderr"].as_str(),
        value["interrupted"].as_bool(),
    ) else {
        return output.into();
    };
    let stdout = if t3_contracts::wire::trim_js(stdout).is_empty() {
        ""
    } else {
        stdout
    };
    let stderr = if t3_contracts::wire::trim_js(stderr).is_empty() {
        ""
    } else {
        stderr
    };
    format!(
        "{stdout}{}{stderr}",
        if !stdout.is_empty() && !stderr.is_empty() && !stdout.ends_with('\n') {
            "\n"
        } else {
            ""
        }
    )
}
fn persisted_output(
    terminal: &dyn TerminalOutputFold,
    output: Option<&str>,
) -> Result<TerminalText> {
    let Some(output) = output else {
        return Ok(TerminalText {
            text: String::new(),
            truncated: false,
        });
    };
    let mut source = output;
    let units = output.encode_utf16().count();
    if units > MAX_FINAL_SOURCE_CHARS {
        let skip = units - MAX_FINAL_SOURCE_CHARS;
        let mut seen = 0;
        let cut = output
            .char_indices()
            .find_map(|(i, c)| {
                let at = seen;
                seen += c.len_utf16();
                (at >= skip).then_some(i)
            })
            .unwrap_or(output.len());
        source = &output[cut..];
        if let Some(newline) = source.find('\n')
            && newline + 1 < source.len()
        {
            source = &source[newline + 1..];
        }
    }
    let mut text = terminal.normalize(source)?;
    text.truncated |= source.len() != output.len();
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_bash_requires_the_complete_shape_and_preserves_newline_rules() {
        assert_eq!(
            legacy_claude_output(r#"{"stdout":"out","stderr":"err","interrupted":false}"#),
            "out\nerr"
        );
        assert_eq!(
            legacy_claude_output(r#"{"stdout":"out\n","stderr":"err","interrupted":true}"#),
            "out\nerr"
        );
        assert_eq!(
            legacy_claude_output(r#"{"stdout":"  ","stderr":"err","interrupted":false}"#),
            "err"
        );
        let invalid = r#"{"stdout":"out","stderr":"err"}"#;
        assert_eq!(legacy_claude_output(invalid), invalid);
    }
    fn hub() -> (CommandOutputHub, crate::testing::TestDatabase) {
        let database = crate::testing::TestDatabase::temporary().expect("test database");
        let hub = CommandOutputHub::new(
            Arc::new(crate::testing::FakeProjectionReader::default()),
            Arc::new(crate::testing::FakeEventCommitter::new(
                database.stores.clone(),
            )),
            Arc::new(NativeTerminalOutputFold),
        );
        (hub, database)
    }
    fn target(thread: &str) -> OrchestrationV2SubscribeCommandOutputInput {
        OrchestrationV2SubscribeCommandOutputInput {
            thread_id: thread.to_owned().into(),
            item_id: "item".to_owned().into(),
        }
    }

    /// Review S-L1: a chunk folds into the live output; nothing renders or serialises the
    /// retained output until a frame asks for it, however many chunks arrive.
    #[tokio::test]
    async fn appending_renders_nothing_until_a_frame_needs_it() {
        let (hub, _database) = hub();
        let line = format!("{}\n", "x".repeat(1_000));
        for _ in 0..10_000 {
            hub.append(target("thread"), line.clone())
                .await
                .expect("append");
        }
        assert_eq!(hub.0.snapshots.load(Ordering::Relaxed), 0);
        let key = (ThreadId("thread".into()), TurnItemId("item".into()));
        let output = hub.0.state.lock().expect("hub").tails[&key].output.clone();
        let state = hub.render(&output.lock().expect("output"));
        assert_eq!(hub.0.snapshots.load(Ordering::Relaxed), 1);
        assert!(state.truncated);
        assert!(state.text.encode_utf16().count() <= TERMINAL_OUTPUT_TAIL_CHARS);
    }

    /// A command whose fold is busy holds only its own output: another thread's chunk still
    /// lands, because the hub's lock is never held across a fold.
    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "the held lock stands for a fold in progress on another command"
    )]
    async fn one_commands_fold_does_not_block_another_threads_output() {
        let (hub, _database) = hub();
        hub.append(target("busy"), "start\n".into())
            .await
            .expect("append");
        let key = (ThreadId("busy".into()), TurnItemId("item".into()));
        let busy = hub.0.state.lock().expect("hub").tails[&key].output.clone();
        let _folding = busy.lock().expect("busy output");
        hub.append(target("other"), "other\n".into())
            .await
            .expect("another thread's chunk lands while the first is folding");
        assert_eq!(hub.0.state.lock().expect("hub").tails.len(), 2);
    }

    #[tokio::test]
    async fn lost_event_receiver_wakes_output_waiters_with_a_failure_state() {
        let (sender, receiver) = broadcast::channel(1);
        drop(sender);
        let subscriber = Arc::new(Subscriber {
            pending: Mutex::new(Pending::default()),
            wake: Notify::new(),
        });
        let target = OrchestrationV2SubscribeCommandOutputInput {
            thread_id: "thread".to_owned().into(),
            item_id: "item".to_owned().into(),
        };
        observe_events(
            receiver,
            target,
            subscriber.clone(),
            CancellationToken::new(),
        )
        .await;
        subscriber.wake.notified().await;
        assert!(subscriber.pending.lock().unwrap().lost);
    }
}
