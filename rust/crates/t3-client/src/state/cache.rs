use crate::{ClientError, platform::Platform};
use futures::{future::poll_fn, lock::Mutex};
use std::{
    cell::RefCell,
    future::Future,
    rc::Rc,
    task::{Poll, Waker},
};

struct Pending<T> {
    version: u64,
    saved: u64,
    value: Option<T>,
    waker: Option<Waker>,
}

pub struct CachePersistence<T> {
    pending: Rc<RefCell<Pending<T>>>,
    lock: Rc<Mutex<()>>,
}

impl<T> Clone for CachePersistence<T> {
    fn clone(&self) -> Self {
        Self {
            pending: self.pending.clone(),
            lock: self.lock.clone(),
        }
    }
}

impl<T> Default for CachePersistence<T> {
    fn default() -> Self {
        Self {
            pending: Rc::new(RefCell::new(Pending {
                version: 0,
                saved: 0,
                value: None,
                waker: None,
            })),
            lock: Rc::new(Mutex::new(())),
        }
    }
}

impl<T: Clone> CachePersistence<T> {
    pub fn queue(&self, value: T) {
        let wake = {
            let mut pending = self.pending.borrow_mut();
            pending.version += 1;
            pending.value = Some(value);
            pending.waker.take()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    }

    pub fn is_pending(&self) -> bool {
        let pending = self.pending.borrow();
        pending.version != pending.saved
    }

    pub async fn flush<F, Fut>(&self, mut save: F) -> Result<(), ClientError>
    where
        F: FnMut(T) -> Fut,
        Fut: Future<Output = Result<(), ClientError>>,
    {
        self.persist(&mut save, true).await
    }

    async fn persist<F, Fut>(&self, save: &mut F, flush: bool) -> Result<(), ClientError>
    where
        F: FnMut(T) -> Fut,
        Fut: Future<Output = Result<(), ClientError>>,
    {
        let _guard = self.lock.lock().await;
        loop {
            let next = {
                let pending = self.pending.borrow();
                if pending.saved == pending.version {
                    return Ok(());
                }
                pending.value.clone().map(|value| (pending.version, value))
            };
            let Some((version, value)) = next else {
                return Ok(());
            };
            save(value).await?;
            self.pending.borrow_mut().saved = version;
            if !flush {
                return Ok(());
            }
        }
    }

    async fn changed(&self) {
        poll_fn(|cx| {
            let mut pending = self.pending.borrow_mut();
            if pending.version != pending.saved {
                Poll::Ready(())
            } else {
                pending.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }

    /// The caller owns this future and explicitly flushes it before dropping its scope.
    pub async fn run<P, F, Fut>(&self, platform: &P, mut save: F) -> Result<(), ClientError>
    where
        P: Platform,
        F: FnMut(T) -> Fut,
        Fut: Future<Output = Result<(), ClientError>>,
    {
        self.changed().await;
        platform.sleep(500).await;
        loop {
            let _ = self.persist(&mut save, false).await;
            platform.sleep(10_000).await;
            self.changed().await;
        }
    }
}
