use futures::channel::oneshot;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use wasm_bindgen::{JsCast, closure::Closure};

/// Browsers fire longer timeouts immediately, so long sleeps are chained.
const MAX_TIMEOUT_MS: u64 = i32::MAX as u64;

struct Timeout {
    window: web_sys::Window,
    handle: Option<i32>,
    receiver: oneshot::Receiver<()>,
    _callback: Closure<dyn FnMut()>,
}
impl Timeout {
    fn new(window: &web_sys::Window, milliseconds: u64) -> Self {
        let (sender, receiver) = oneshot::channel();
        let mut sender = Some(sender);
        let callback = Closure::<dyn FnMut()>::new(move || {
            if let Some(sender) = sender.take() {
                let _ = sender.send(());
            }
        });
        let handle = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                milliseconds.min(MAX_TIMEOUT_MS) as i32,
            )
            .ok();
        Self {
            window: window.clone(),
            handle,
            receiver,
            _callback: callback,
        }
    }
}
impl Future for Timeout {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.handle.is_none() {
            // setTimeout itself failed; never resolve rather than spin.
            return Poll::Pending;
        }
        match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Ready(_) => {
                self.handle = None;
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
impl Drop for Timeout {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.window.clear_timeout_with_handle(handle);
        }
    }
}

pub(crate) async fn sleep(window: web_sys::Window, milliseconds: u64) {
    let mut remaining = milliseconds;
    loop {
        let step = remaining.min(MAX_TIMEOUT_MS);
        Timeout::new(&window, step).await;
        remaining -= step;
        if remaining == 0 {
            return;
        }
    }
}
