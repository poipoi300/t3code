#![allow(clippy::unwrap_used)]
use futures::{FutureExt, channel::oneshot, executor::block_on, stream::LocalBoxStream};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
};
use t3_client::{
    ClientError,
    platform::*,
    services::config::*,
    testing::FakePlatform,
    types::{EnvironmentId, Generation},
};
use t3_contracts::server::*;
#[derive(Default)]
struct GatePlatform {
    base: FakePlatform,
    gates: RefCell<VecDeque<oneshot::Receiver<Result<(), PlatformError>>>>,
    writes: Cell<usize>,
}
impl GatePlatform {
    fn gate(&self) -> oneshot::Sender<Result<(), PlatformError>> {
        let (tx, rx) = oneshot::channel();
        self.gates.borrow_mut().push_back(rx);
        tx
    }
}
impl Storage for GatePlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.base.get(area, key)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.base.list(area, namespace)
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        let gate = self.gates.borrow_mut().pop_front();
        async move {
            self.writes.set(self.writes.get() + 1);
            if let Some(gate) = gate {
                gate.await.map_err(|_| PlatformError::Cancelled)??;
            }
            self.base.write(area, writes).await
        }
        .boxed_local()
    }
}
impl Platform for GatePlatform {
    fn now_epoch_ms(&self) -> u64 {
        self.base.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.base.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        self.base.random_bytes(length)
    }
    fn sleep(&self, milliseconds: u64) -> futures::future::LocalBoxFuture<'_, ()> {
        self.base.sleep(milliseconds)
    }
    fn yield_to_host(&self) -> futures::future::LocalBoxFuture<'_, ()> {
        self.base.yield_to_host()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.base.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        self.base.http(request)
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        self.base.websocket(request)
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        self.base.host(action)
    }
}
fn config() -> ServerConfig {
    serde_json::from_str(
        include_str!("../../../fixtures/samples/ServerConfig.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap()
}
fn service() -> ServerConfigService {
    let service = ServerConfigService::new(EnvironmentId("environment".into()));
    service.begin_session(Generation(1));
    service
}
fn apply(service: &ServerConfigService, config: ServerConfig) {
    service
        .apply(
            Generation(1),
            serde_json::from_value(
                serde_json::json!({"version":1,"type":"snapshot","config":config}),
            )
            .unwrap(),
        )
        .unwrap();
}
#[test]
fn config_flush_serializes_writes_and_keeps_the_latest_event_while_storage_is_pending() {
    let platform = GatePlatform::default();
    let service = service();
    let release = platform.gate();
    apply(&service, config());
    let mut write = Box::pin(service.flush(&platform));
    assert!(write.as_mut().now_or_never().is_none());
    let mut latest = config();
    latest.settings.hide_tagged_threads = !latest.settings.hide_tagged_threads;
    apply(&service, latest.clone());
    block_on(service.flush(&platform)).unwrap();
    assert_eq!(platform.writes.get(), 1);
    release.send(Ok(())).unwrap();
    block_on(write).unwrap();
    assert_eq!(platform.writes.get(), 2);
    let bytes = block_on(platform.get(
        StorageArea::Ordinary,
        StorageKey::new(
            "server-config",
            Some(EnvironmentId("environment".into())),
            "current",
        ),
    ))
    .unwrap()
    .unwrap();
    let record: t3_client::persistence::Record<ServerConfig> =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(record.payload, cacheable_config(&latest));
    apply(&service, latest);
    block_on(service.flush(&platform)).unwrap();
    assert_eq!(platform.writes.get(), 2);
}
#[test]
fn failed_config_saves_are_retried_on_a_transient_event_and_closed_configs_cancel_storage() {
    let platform = GatePlatform::default();
    let service = service();
    let release = platform.gate();
    apply(&service, config());
    let mut write = Box::pin(service.flush(&platform));
    assert!(write.as_mut().now_or_never().is_none());
    release
        .send(Err(PlatformError::Storage("failed".into())))
        .unwrap();
    assert!(block_on(write).is_err());
    service.apply(Generation(1),serde_json::from_value(serde_json::json!({"version":1,"type":"environmentThemesUpdated","payload":{"themes":[]}})).unwrap()).unwrap();
    block_on(service.flush(&platform)).unwrap();
    assert_eq!(platform.writes.get(), 2);
    let release = platform.gate();
    let mut changed = config();
    changed.settings.hide_tagged_threads = !changed.settings.hide_tagged_threads;
    apply(&service, changed);
    let mut write = Box::pin(service.flush(&platform));
    assert!(write.as_mut().now_or_never().is_none());
    service.close();
    assert!(matches!(block_on(write), Err(ClientError::Cancelled)));
    assert!(release.is_canceled());
}
#[test]
fn query_answers_do_not_wait_for_a_save_and_disposal_cancels_that_save() {
    use futures::{StreamExt, channel::mpsc};
    use std::rc::Rc;
    use t3_client::{
        rpc::RpcMethod,
        services::{resources::*, sessions::*},
        testing::{RpcScript, ScriptedRpc},
    };
    struct Number;
    impl RpcMethod for Number {
        type Input = u64;
        type Output = u64;
        const NAME: &'static str = "number";
    }
    let platform = GatePlatform::default();
    let release = platform.gate();
    let resource = Resource::new(ResourcePolicy::default(), Generation(1));
    let rpc = Rc::new(ScriptedRpc::default());
    rpc.scripts.borrow_mut().push_back(RpcScript {
        method: "number".into(),
        input: serde_json::json!(0),
        replies: vec![Ok(serde_json::json!(7))],
    });
    let environment = EnvironmentId("environment".into());
    let (tx, rx) = mpsc::unbounded();
    tx.unbounded_send(QueryConnection {
        phase: QueryConnectionPhase::Connected,
        failure: None,
        session: Some(ServiceSession {
            identity: t3_client::environment::SessionIdentity {
                environment: environment.clone(),
                generation: Generation(1),
                account: None,
                dpop_thumbprint: None,
            },
            endpoint: rpc,
        }),
    })
    .unwrap();
    let answers = Cell::new(0);
    let mut driver = Box::pin(drive_query::<Number, _, _>(
        &resource,
        &platform,
        environment,
        rx.boxed_local(),
        0,
        Some("number"),
        |_| answers.set(answers.get() + 1),
    ));
    assert!(driver.as_mut().now_or_never().is_none());
    assert_eq!(resource.snapshot().value.as_deref(), Some(&7));
    assert!(!resource.snapshot().loading);
    assert_eq!(answers.get(), 1);
    assert_eq!(platform.writes.get(), 1);
    drop(driver);
    assert!(release.is_canceled());
    assert_eq!(platform.base.storage_write_count(), 0);
}
