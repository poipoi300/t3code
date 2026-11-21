use t3_client::platform::PlatformError;

pub(crate) struct Limits {
    pub(crate) text: usize,
    pub(crate) binary: usize,
    pub(crate) queue: Option<usize>,
}

impl Limits {
    pub(crate) fn for_url(url: &url::Url) -> Self {
        // Preview receives small control events and JPEGs, paced by the viewer ACK.
        if url.path() == "/api/preview-stream/ws" {
            Self {
                text: 256 * 1024,
                binary: 12 * 1024 * 1024,
                queue: Some(8),
            }
        } else {
            let rpc = t3_rpc::transport::MAX_INCOMING_FRAME_BYTES;
            Self {
                text: rpc,
                binary: rpc,
                queue: None,
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

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod browser {
    use futures::channel::mpsc;
    use t3_client::platform::{PlatformError, SocketClose};
    use t3_rpc::Frame;

    pub(crate) struct Ingress {
        frames: Option<mpsc::UnboundedSender<Frame>>,
        queue: Option<usize>,
        terminal: Option<Result<Option<SocketClose>, PlatformError>>,
    }

    impl Ingress {
        pub(crate) fn new(queue: Option<usize>) -> (Self, mpsc::UnboundedReceiver<Frame>) {
            let (frames, incoming) = mpsc::unbounded();
            (
                Self {
                    frames: Some(frames),
                    queue,
                    terminal: None,
                },
                incoming,
            )
        }

        pub(crate) fn send(&self, frame: Frame) -> Result<(), PlatformError> {
            let sender = self.frames.as_ref().ok_or(PlatformError::Closed)?;
            if self.queue.is_some_and(|max| sender.len() >= max) {
                return Err(PlatformError::TooLarge);
            }
            sender
                .unbounded_send(frame)
                .map_err(|_| PlatformError::Closed)
        }

        pub(crate) fn finish(&mut self, terminal: Result<Option<SocketClose>, PlatformError>) {
            if self.frames.take().is_some() {
                self.terminal = Some(terminal);
            }
        }

        pub(crate) fn terminal(&mut self) -> Option<Result<Option<SocketClose>, PlatformError>> {
            self.terminal.take()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use futures::{FutureExt, StreamExt};

        #[test]
        fn terminal_closes_the_stream_and_preserves_only_the_first_outcome() {
            let peer = SocketClose {
                code: 4503,
                reason: "sandbox".into(),
            };
            for ending in [
                Ok(Some(peer)),
                Err(PlatformError::Network("socket error".into())),
                Ok(None),
            ] {
                let (mut ingress, mut stream) = Ingress::new(Some(4));
                ingress.send(Frame::Text("last frame".into())).unwrap();
                ingress.finish(ending.clone());
                ingress.finish(Err(PlatformError::TooLarge));
                assert_eq!(
                    stream.next().now_or_never(),
                    Some(Some(Frame::Text("last frame".into())))
                );
                assert_eq!(stream.next().now_or_never(), Some(None));
                assert_eq!(ingress.terminal(), Some(ending));
                assert_eq!(ingress.terminal(), None);
                assert_eq!(
                    ingress.send(Frame::Text("late".into())),
                    Err(PlatformError::Closed)
                );
            }
        }

        #[test]
        fn preview_ingress_caps_message_sizes_and_queue() {
            let url = url::Url::parse("wss://host/api/preview-stream/ws?wsTicket=unused").unwrap();
            let limits = super::super::Limits::for_url(&url);
            for (binary, max) in [(false, 256 * 1024), (true, 12 * 1024 * 1024)] {
                assert_eq!(limits.check(max, binary), Ok(()));
                assert_eq!(limits.check(max + 1, binary), Err(PlatformError::TooLarge));
            }
            let (ingress, _stream) = Ingress::new(limits.queue);
            for _ in 0..8 {
                ingress.send(Frame::Binary(vec![0])).unwrap();
            }
            assert_eq!(
                ingress.send(Frame::Binary(vec![0])),
                Err(PlatformError::TooLarge)
            );
        }
    }
}
