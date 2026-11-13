use crate::{Error, Event, transport::lock};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::{Notify, watch};

#[derive(Default)]
pub(crate) struct RawQueue {
    queue: Mutex<(VecDeque<(Event, usize)>, usize)>,
    ready: Notify,
}

impl RawQueue {
    pub fn push(&self, event: Event, line_length: usize) {
        let mut state = lock(&self.queue);
        state.0.push_back((event, line_length));
        state.1 += line_length;
        // Chatty agents must not retain unbounded diagnostic notifications; always keep the newest.
        while state.0.len() > 1 && (state.0.len() > 32 || state.1 > 16 * 1024 * 1024) {
            if let Some((_, size)) = state.0.pop_front() {
                state.1 -= size;
            }
        }
        self.ready.notify_one();
    }

    fn pop(&self) -> Option<Event> {
        let mut state = lock(&self.queue);
        let (event, size) = state.0.pop_front()?;
        state.1 -= size;
        Some(event)
    }
}

/// Best-effort diagnostic stream: newest 32 notifications within 16 Mi UTF-16
/// code units, including the newest even when it alone exceeds that budget.
/// Multiple readers share one queue; use `Events` for lossless adapter ingestion.
pub struct RawNotifications {
    pub(crate) queue: Arc<RawQueue>,
    pub(crate) terminal: watch::Receiver<Option<Error>>,
}

impl RawNotifications {
    pub async fn next(&mut self) -> Option<Event> {
        loop {
            let notified = self.queue.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(event) = self.queue.pop() {
                return Some(event);
            }
            if self.terminal.borrow().is_some() {
                return None;
            }
            tokio::select! {_ = notified=>{},_ = self.terminal.changed()=>{}}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(index: usize) -> Event {
        Event::Notification {
            method: "_n".into(),
            params: json!(index),
        }
    }
    #[test]
    fn keeps_newest_count_and_full_batch_line_budget_including_oversized_latest() {
        let queue = RawQueue::default();
        for i in 0..40 {
            queue.push(event(i), 1);
        }
        assert_eq!(queue.pop(), Some(event(8)));
        for i in 40..44 {
            queue.push(event(i), 6 * 1024 * 1024);
        }
        assert_eq!(queue.pop(), Some(event(42)));
        assert_eq!(queue.pop(), Some(event(43)));
        assert_eq!(queue.pop(), None);
        queue.push(event(44), 17 * 1024 * 1024);
        assert_eq!(queue.pop(), Some(event(44)));
    }
}
