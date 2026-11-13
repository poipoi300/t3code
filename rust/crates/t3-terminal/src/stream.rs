use crate::{
    TerminalAttachStreamEvent, TerminalError, TerminalEvent, TerminalMetadataStreamEvent, lock,
    manager::{Inner, Key},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Notify;

pub(crate) struct Queue<T> {
    state: Mutex<QueueState<T>>,
    ready: Notify,
    max_bytes: usize,
}
struct QueueState<T> {
    items: VecDeque<(T, usize)>,
    bytes: usize,
    overflow: bool,
    closed: bool,
}
impl<T> Queue<T> {
    pub fn new(max_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(QueueState {
                items: VecDeque::new(),
                bytes: 0,
                overflow: false,
                closed: false,
            }),
            ready: Notify::new(),
            max_bytes,
        })
    }
    pub fn push(&self, item: T, size: usize) {
        let mut s = lock(&self.state);
        if s.closed || s.overflow {
            return;
        }
        let first = s.items.front().map(|(_, n)| *n).unwrap_or(size);
        s.bytes = s.bytes.saturating_add(size);
        s.items.push_back((item, size));
        if s.bytes.saturating_sub(first) > self.max_bytes {
            s.items.clear();
            s.bytes = 0;
            s.overflow = true;
        }
        self.ready.notify_one();
    }
    pub fn reset(&self, item: T, size: usize) {
        let mut s = lock(&self.state);
        s.items.clear();
        s.bytes = 0;
        s.overflow = false;
        drop(s);
        self.push(item, size);
    }
    pub fn close(&self) {
        lock(&self.state).closed = true;
        self.ready.notify_one();
    }
    pub fn overflowed(&self) -> bool {
        lock(&self.state).overflow
    }
    pub fn pop(&self) -> Option<T> {
        let mut s = lock(&self.state);
        let (item, size) = s.items.pop_front()?;
        s.bytes -= size;
        Some(item)
    }
    async fn wait(&self) -> bool {
        let notified = self.ready.notified();
        {
            let s = lock(&self.state);
            if s.overflow || !s.items.is_empty() {
                return true;
            }
            if s.closed {
                return false;
            }
        }
        notified.await;
        true
    }
}

pub struct AttachStream {
    pub(crate) queue: Arc<Queue<TerminalAttachStreamEvent>>,
    pub(crate) manager: Weak<Inner>,
    pub(crate) key: Key,
    pub(crate) registration: u64,
    pub(crate) viewer: Option<String>,
    pub(crate) max_history: Option<usize>,
}
impl AttachStream {
    /// Overflow replaces queued output with an atomic current snapshot; it never respawns a shell.
    pub async fn recv(&mut self) -> Result<Option<TerminalAttachStreamEvent>, TerminalError> {
        loop {
            if self.queue.overflowed() {
                if let Some(inner) = self.manager.upgrade() {
                    let key = self.key.clone();
                    let queue = self.queue.clone();
                    let max = self.max_history;
                    tokio::task::spawn_blocking(move || inner.resync(&key, &queue, max))
                        .await
                        .map_err(|error| TerminalError::Worker(error.to_string()))?;
                } else {
                    return Ok(None);
                }
            }
            if let Some(event) = self.queue.pop() {
                return Ok(Some(event));
            }
            if !self.queue.wait().await {
                return Ok(None);
            }
        }
    }
}
impl Drop for AttachStream {
    fn drop(&mut self) {
        if let Some(inner) = self.manager.upgrade() {
            let registration = self.registration;
            let key = self.key.clone();
            let viewer = self.viewer.clone();
            let runtime = inner.runtime.clone();
            runtime.spawn_blocking(move || inner.detach(registration, &key, viewer.as_deref()));
        }
    }
}
pub struct EventStream {
    pub(crate) queue: Arc<Queue<TerminalEvent>>,
}
impl EventStream {
    pub async fn recv(&mut self) -> Option<TerminalEvent> {
        loop {
            if self.queue.overflowed() {
                return None;
            }
            if let Some(event) = self.queue.pop() {
                return Some(event);
            }
            if !self.queue.wait().await {
                return None;
            }
        }
    }
}
pub struct MetadataStream {
    pub(crate) queue: Arc<Queue<TerminalMetadataStreamEvent>>,
    pub(crate) manager: Weak<Inner>,
}
impl MetadataStream {
    pub async fn recv(&mut self) -> Option<TerminalMetadataStreamEvent> {
        loop {
            if self.queue.overflowed() {
                {
                    let inner = self.manager.upgrade()?;
                    let state = lock(&inner.state);
                    self.queue.reset(
                        TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(
                            crate::TerminalMetadataSnapshotEvent {
                                r#type: crate::TerminalMetadataSnapshotEventType::Snapshot,
                                terminals: state.summaries(),
                            },
                        ),
                        128,
                    );
                }
            }
            if let Some(e) = self.queue.pop() {
                return Some(e);
            }
            if !self.queue.wait().await {
                return None;
            }
        }
    }
}
pub(crate) fn attached_size(event: &TerminalAttachStreamEvent) -> usize {
    use TerminalAttachStreamEvent::*;
    128 + match event {
        TerminalAttachSnapshotEvent(e) => snapshot_size(&e.snapshot),
        TerminalRestartedEvent(e) => {
            snapshot_size(&e.snapshot) + e.thread_id.len() + e.terminal_id.len()
        }
        TerminalOutputEvent(e) => e.thread_id.len() + e.terminal_id.len() + e.data.len(),
        TerminalErrorEvent(e) => e.thread_id.len() + e.terminal_id.len() + e.message.len(),
        TerminalActivityEvent(e) => e.thread_id.len() + e.terminal_id.len() + e.label.len(),
        TerminalExitedEvent(e) => e.thread_id.len() + e.terminal_id.len(),
        TerminalClosedEvent(e) => e.thread_id.len() + e.terminal_id.len(),
        TerminalClearedEvent(e) => e.thread_id.len() + e.terminal_id.len(),
    }
}
fn snapshot_size(s: &crate::TerminalSessionSnapshot) -> usize {
    s.thread_id.len()
        + s.terminal_id.len()
        + s.cwd.len()
        + s.history.len()
        + s.worktree_path.as_ref().map_or(0, |p| p.0.len())
        + s.label.len()
}
pub(crate) fn bounded_event(
    mut event: TerminalAttachStreamEvent,
    max: Option<usize>,
) -> TerminalAttachStreamEvent {
    if let Some(max) = max {
        match &mut event {
            TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(e) => {
                e.snapshot.history = crate::retained_terminal_history(&e.snapshot.history, max)
            }
            TerminalAttachStreamEvent::TerminalRestartedEvent(e) => {
                e.snapshot.history = crate::retained_terminal_history(&e.snapshot.history, max)
            }
            _ => {}
        }
    }
    event
}
pub(crate) fn metadata_size(event: &TerminalMetadataStreamEvent) -> usize {
    fn size(s: &crate::TerminalSummary) -> usize {
        128 + s.thread_id.len()
            + s.terminal_id.len()
            + s.cwd.len()
            + s.worktree_path.as_ref().map_or(0, |p| p.0.len())
            + s.label.len()
            + s.updated_at.len()
    }
    128 + match event {
        TerminalMetadataStreamEvent::TerminalMetadataSnapshotEvent(e) => {
            e.terminals.iter().map(size).sum()
        }
        TerminalMetadataStreamEvent::TerminalMetadataUpsertEvent(e) => size(&e.terminal),
        TerminalMetadataStreamEvent::TerminalMetadataRemoveEvent(e) => {
            e.thread_id.len() + e.terminal_id.len()
        }
    }
}

/// Transport adapter ACK accounting from OutputProtocol.ts. An adapter calls `sent` per batch
/// and `ack` per client ACK; synthetic ACKs may continue only while `is_full` is false.
#[derive(Default, Debug)]
pub struct OutputWindow {
    sizes: VecDeque<usize>,
}
impl OutputWindow {
    pub fn is_full(&self) -> bool {
        self.sizes.len() >= 8 || self.sizes.iter().sum::<usize>() >= 64 * 1024
    }
    pub fn sent(&mut self, events: &[TerminalAttachStreamEvent]) {
        self.sizes
            .push_back(128 + events.iter().map(attached_size).sum::<usize>());
    }
    pub fn ack(&mut self) -> bool {
        let full = self.is_full();
        self.sizes.pop_front();
        full && !self.is_full()
    }
}

pub(crate) fn event_size(event: &TerminalEvent) -> usize {
    use TerminalEvent::*;
    128 + match event {
        TerminalStartedEvent(e) => {
            snapshot_size(&e.snapshot) + e.thread_id.len() + e.terminal_id.len()
        }
        TerminalRestartedEvent(e) => {
            snapshot_size(&e.snapshot) + e.thread_id.len() + e.terminal_id.len()
        }
        TerminalOutputEvent(e) => e.thread_id.len() + e.terminal_id.len() + e.data.len(),
        TerminalErrorEvent(e) => e.thread_id.len() + e.terminal_id.len() + e.message.len(),
        TerminalActivityEvent(e) => e.thread_id.len() + e.terminal_id.len() + e.label.len(),
        TerminalExitedEvent(e) => e.thread_id.len() + e.terminal_id.len(),
        TerminalClosedEvent(e) => e.thread_id.len() + e.terminal_id.len(),
        TerminalClearedEvent(e) => e.thread_id.len() + e.terminal_id.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn output(data: &str) -> TerminalAttachStreamEvent {
        TerminalAttachStreamEvent::TerminalOutputEvent(crate::TerminalOutputEvent {
            r#type: crate::TerminalOutputEventType::Output,
            thread_id: "t".into(),
            terminal_id: "term-1".into(),
            sequence: Some(Some(1)),
            data: data.into(),
        })
    }

    #[test]
    fn queue_charges_utf8_and_envelopes_and_allows_one_oversized_event() {
        let queue = Queue::new(32);
        queue.push("snapshot", 1000);
        assert!(!queue.overflowed());
        queue.push("output", 20);
        assert!(!queue.overflowed());
        queue.push("output", 20);
        assert!(queue.overflowed());
        assert_eq!(queue.pop(), None);
        queue.reset("resync", 1000);
        assert_eq!(queue.pop(), Some("resync"));
        assert!(!queue.overflowed());
        let event = output("🙂");
        assert_eq!(attached_size(&event), 128 + "tterm-1".len() + 4);
        let queue = Queue::new(128);
        for _ in 0..3 {
            queue.push(output(""), attached_size(&output("")));
        }
        assert!(queue.overflowed());
    }

    #[test]
    fn ack_window_caps_count_and_utf8_bytes_and_only_wakes_when_unblocked() {
        let mut window = OutputWindow::default();
        for _ in 0..8 {
            window.sent(&[output("")]);
        }
        assert!(window.is_full());
        assert!(window.ack());
        assert!(!window.ack());
        let mut window = OutputWindow::default();
        window.sent(&[output(&"🙂".repeat(16384))]);
        assert!(window.is_full());
        assert!(window.ack());
        let mut window = OutputWindow::default();
        for _ in 0..10 {
            window.sent(&[output("")]);
        }
        assert!(!window.ack());
        assert!(!window.ack());
        assert!(window.ack());
    }
}
