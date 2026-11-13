use std::sync::Mutex;

pub(crate) struct Changes<T> {
    subscribers: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<T>>>,
}
impl<T: Clone> Changes<T> {
    pub(crate) fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }
    pub(crate) fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<T> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(sender);
        receiver
    }
    pub(crate) fn send(&self, value: T) {
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|s| s.send(value.clone()).is_ok());
    }
}
