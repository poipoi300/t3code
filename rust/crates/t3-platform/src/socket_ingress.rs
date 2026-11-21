use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
};

use futures::{StreamExt, channel::mpsc};
use t3_client::platform::{PlatformError, SocketClose};
use t3_rpc::Frame;

pub(crate) struct Limits {
    pub(crate) text: usize,
    pub(crate) binary: usize,
    pub(crate) preview: bool,
}

impl Limits {
    pub(crate) fn for_url(url: &url::Url) -> Self {
        if url.path() == "/api/preview-stream/ws" {
            Self {
                text: 256 * 1024,
                binary: 12 * 1024 * 1024,
                preview: true,
            }
        } else {
            let rpc = t3_rpc::transport::MAX_INCOMING_FRAME_BYTES;
            Self {
                text: rpc,
                binary: rpc,
                preview: false,
            }
        }
    }

    pub(crate) fn check(&self, bytes: usize, binary: bool) -> Result<(), PlatformError> {
        if bytes > if binary { self.binary } else { self.text } {
            Err(PlatformError::TooLarge)
        } else {
            Ok(())
        }
    }
}

pub(crate) enum Incoming {
    Frame(Frame),
    Close(SocketClose),
}

pub(crate) type Item = Result<Incoming, PlatformError>;

const MAX_PREVIEW_CONTROL_BACKLOG_BYTES: usize = 64 * 1024 * 1024;

fn control_bytes(item: &Item) -> usize {
    match item {
        Ok(Incoming::Frame(Frame::Text(text))) => size_of::<Item>() + text.capacity(),
        _ => 0,
    }
}

struct Pending {
    messages: VecDeque<Item>,
    // The count positions the newest image after the controls that preceded it.
    image: Option<(usize, Vec<u8>)>,
    preview_control_bytes: Option<usize>,
}

impl Pending {
    fn push(&mut self, item: Item) -> Result<(), PlatformError> {
        if let Some(bytes) = &mut self.preview_control_bytes {
            if let Ok(Incoming::Frame(Frame::Binary(image))) = item {
                self.image = Some((self.messages.len(), image));
                return Ok(());
            }
            let total = *bytes + control_bytes(&item);
            if total > MAX_PREVIEW_CONTROL_BACKLOG_BYTES {
                return Err(PlatformError::SocketBacklogOverflow);
            }
            *bytes = total;
        }
        self.messages.push_back(item);
        Ok(())
    }

    fn pop(&mut self) -> Option<Item> {
        if let Some((before, _)) = &mut self.image {
            if *before == 0 {
                return self
                    .image
                    .take()
                    .map(|(_, bytes)| Ok(Incoming::Frame(Frame::Binary(bytes))));
            }
            *before -= 1;
        }
        let item = self.messages.pop_front()?;
        if let Some(bytes) = &mut self.preview_control_bytes {
            *bytes -= control_bytes(&item);
        }
        Some(item)
    }
}

pub(crate) struct BufferedSender {
    pending: Arc<Mutex<Pending>>,
    ready: mpsc::Sender<()>,
}

pub(crate) struct BufferedReceiver {
    pending: Arc<Mutex<Pending>>,
    ready: mpsc::Receiver<()>,
}

pub(crate) fn buffered(preview: bool) -> (BufferedSender, BufferedReceiver) {
    let pending = Arc::new(Mutex::new(Pending {
        messages: VecDeque::new(),
        image: None,
        preview_control_bytes: preview.then_some(0),
    }));
    let (ready, next) = mpsc::channel(0);
    (
        BufferedSender {
            pending: pending.clone(),
            ready,
        },
        BufferedReceiver {
            pending,
            ready: next,
        },
    )
}

impl BufferedSender {
    pub(crate) fn send(&mut self, item: Item) -> Result<(), PlatformError> {
        if self.ready.is_closed() {
            return Err(PlatformError::Closed);
        }
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(item)?;
        // One outstanding notification is enough. The receiver checks the queue before awaiting.
        match self.ready.try_send(()) {
            Ok(()) => Ok(()),
            Err(error) if error.is_full() => Ok(()),
            Err(_) => Err(PlatformError::Closed),
        }
    }
}

impl BufferedReceiver {
    pub(crate) async fn next(&mut self) -> Option<Item> {
        loop {
            if let Some(item) = self
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop()
            {
                return Some(item);
            }
            self.ready.next().await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_control_backlog_caps_aggregate_bytes() {
        use futures::FutureExt;
        let (mut sender, mut receiver) = buffered(true);
        let control = || Ok(Incoming::Frame(Frame::Text("x".repeat(256 * 1024))));
        for _ in 0..255 {
            assert_eq!(sender.send(control()), Ok(()));
        }
        assert_eq!(
            sender.send(control()),
            Err(PlatformError::SocketBacklogOverflow)
        );
        assert!(matches!(
            receiver.next().now_or_never(),
            Some(Some(Ok(Incoming::Frame(Frame::Text(_)))))
        ));
        assert_eq!(sender.send(control()), Ok(()));
    }

    #[test]
    fn preview_buffer_keeps_controls_and_newest_image_before_terminal() {
        use futures::FutureExt;
        let (mut sender, mut receiver) = buffered(true);
        sender
            .send(Ok(Incoming::Frame(Frame::Text("before".into()))))
            .unwrap();
        sender
            .send(Ok(Incoming::Frame(Frame::Binary(vec![1]))))
            .unwrap();
        sender
            .send(Ok(Incoming::Frame(Frame::Text("between".into()))))
            .unwrap();
        sender
            .send(Ok(Incoming::Frame(Frame::Binary(vec![2]))))
            .unwrap();
        sender
            .send(Ok(Incoming::Close(SocketClose::ABNORMAL)))
            .unwrap();
        drop(sender);
        for expected in ["before", "between"] {
            assert!(matches!(receiver.next().now_or_never(),
                Some(Some(Ok(Incoming::Frame(Frame::Text(text))))) if text == expected));
        }
        assert!(matches!(receiver.next().now_or_never(),
            Some(Some(Ok(Incoming::Frame(Frame::Binary(bytes))))) if bytes == [2]));
        assert!(matches!(receiver.next().now_or_never(),
            Some(Some(Ok(Incoming::Close(close)))) if close == SocketClose::ABNORMAL));
        assert!(matches!(receiver.next().now_or_never(), Some(None)));
    }
}
