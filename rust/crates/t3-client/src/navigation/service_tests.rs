use super::*;
use crate::{platform::*, testing::FakePlatform};
use futures::{channel::mpsc, executor::block_on};
use std::{cell::RefCell, rc::Rc};

type UrlReceiver = mpsc::UnboundedReceiver<Result<AppUrlSnapshot, PlatformError>>;

#[derive(Clone, Default)]
struct Host {
    fake: FakePlatform,
    changes: Rc<RefCell<Option<UrlReceiver>>>,
    requests: Rc<RefCell<Vec<AppUrlRequest>>>,
    denied: Rc<RefCell<bool>>,
    hold: Rc<RefCell<Option<futures::channel::oneshot::Receiver<()>>>>,
}
impl Storage for Host {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.fake.get(area, key)
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.fake.write(area, writes)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.fake.list(area, namespace)
    }
}
impl Platform for Host {
    fn now_epoch_ms(&self) -> u64 {
        self.fake.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.fake.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        self.fake.random_bytes(length)
    }
    fn sleep(&self, millis: u64) -> LocalBoxFuture<'_, ()> {
        self.fake.sleep(millis)
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        self.fake.yield_to_host()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.fake.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        self.fake.http(request)
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        self.fake.websocket(request)
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        self.fake.host(action)
    }
    fn app_url_changes(&self) -> LocalBoxStream<'static, Result<AppUrlSnapshot, PlatformError>> {
        self.changes
            .borrow_mut()
            .take()
            .expect("one listener")
            .boxed_local()
    }
    fn navigate_app_url(&self, request: AppUrlRequest) -> HostFuture<'_, ()> {
        async move {
            if *self.denied.borrow() {
                return Err(PlatformError::Denied("navigation".into()));
            }
            self.requests.borrow_mut().push(request);
            let hold = self.hold.borrow_mut().take();
            if let Some(hold) = hold {
                hold.await.map_err(|_| PlatformError::Closed)?;
            }
            Ok(())
        }
        .boxed_local()
    }
}

#[test]
fn traversal_and_following_navigation_are_serialized_and_cancellation_releases_the_lane() {
    block_on(async {
        let host = Host::default();
        let (resume, held) = futures::channel::oneshot::channel();
        *host.hold.borrow_mut() = Some(held);
        let client = UrlClient::new(host.clone());
        let mut close = client.navigate(AppUrlRequest::CloseModal(
            super::super::CloseModal::GoThenPush {
                delta: -2,
                href: "/e/t".into(),
            },
        ));
        let mut next = client
            .clone()
            .navigate(AppUrlRequest::Push("/usage".into()));
        assert!(futures::poll!(close.as_mut()).is_pending());
        assert!(futures::poll!(next.as_mut()).is_pending());
        assert_eq!(host.requests.borrow().len(), 1);
        resume.send(()).unwrap();
        close.await.unwrap();
        next.await.unwrap();
        assert_eq!(host.requests.borrow().len(), 2);
        let (_resume, held) = futures::channel::oneshot::channel();
        *host.hold.borrow_mut() = Some(held);
        let mut cancelled = client.navigate(AppUrlRequest::Go(-1));
        assert!(futures::poll!(cancelled.as_mut()).is_pending());
        drop(cancelled);
        client.navigate(AppUrlRequest::Go(1)).await.unwrap();
    });
}
fn snapshot(path: &str, index: u64) -> AppUrlSnapshot {
    AppUrlSnapshot {
        path: path.into(),
        history_index: index,
        can_go_back: index > 0,
        can_go_forward: false,
        policy: Default::default(),
    }
}
#[test]
fn host_snapshots_are_authoritative_and_dropping_a_watch_releases_it() {
    block_on(async {
        let host = Host::default();
        let (tx, rx) = mpsc::unbounded();
        *host.changes.borrow_mut() = Some(rx);
        tx.unbounded_send(Ok(snapshot("/e/one", 0))).unwrap();
        let client = UrlClient::new(host.clone());
        let mut changes = client.watch();
        assert_eq!(changes.next().await.unwrap().unwrap().path, "/e/one");
        client
            .navigate(AppUrlRequest::Push("/e/two".into()))
            .await
            .unwrap();
        assert!(
            futures::poll!(changes.next()).is_pending(),
            "no command echo"
        );
        tx.unbounded_send(Ok(snapshot("/e/two", 1))).unwrap();
        assert_eq!(changes.next().await.unwrap().unwrap().history_index, 1);
        *host.denied.borrow_mut() = true;
        assert!(matches!(
            client.navigate(AppUrlRequest::Go(-1)).await,
            Err(ClientError::Platform(PlatformError::Denied(_)))
        ));
        assert_eq!(host.requests.borrow().len(), 1);
        drop(changes);
        assert!(tx.is_closed());
    });
}
#[test]
fn redirect_waits_for_host_resolution_and_stream_termination_is_observable() {
    block_on(async {
        let host = Host::default();
        let (tx, rx) = mpsc::unbounded();
        *host.changes.borrow_mut() = Some(rx);
        tx.unbounded_send(Ok(snapshot("/settings/beta?x=1#anchor", 1)))
            .unwrap();
        let client = UrlClient::new(host.clone());
        let mut changes = client.watch();
        assert!(futures::poll!(changes.next()).is_pending());
        assert_eq!(
            *host.requests.borrow(),
            [AppUrlRequest::Replace("/settings/labs?x=1#anchor".into())]
        );
        tx.unbounded_send(Ok(snapshot("/settings/labs?x=1#anchor", 1)))
            .unwrap();
        assert_eq!(
            changes.next().await.unwrap().unwrap().path,
            "/settings/labs?x=1#anchor"
        );
        tx.unbounded_send(Err(PlatformError::Closed)).unwrap();
        assert!(matches!(
            changes.next().await,
            Some(Err(ClientError::Platform(PlatformError::Closed)))
        ));
        drop(tx);
        assert!(changes.next().await.is_none());
        assert!(matches!(
            client
                .navigate(AppUrlRequest::Push("//evil/pair".into()))
                .await,
            Err(ClientError::InvalidPayload(_))
        ));
    });
}
#[test]
fn absent_host_capabilities_remain_typed_failures() {
    block_on(async {
        let client = UrlClient::new(FakePlatform::default());
        assert!(matches!(
            client.launch_address().await,
            Err(ClientError::Platform(PlatformError::Unsupported(_)))
        ));
        assert!(matches!(
            client.watch().next().await,
            Some(Err(ClientError::Platform(PlatformError::Unsupported(_))))
        ));
        assert!(matches!(
            client.navigate(AppUrlRequest::Go(-1)).await,
            Err(ClientError::Platform(PlatformError::Unsupported(_)))
        ));
    });
}
