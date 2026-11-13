#![allow(dead_code)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};
use t3_cloud::{
    Clock, Error, Result,
    http::{Request, Response, Transport},
    store::Secrets,
};
use tokio::sync::Mutex;

pub const NOW: i64 = 1_800_000_000_000;
pub struct Time(pub AtomicI64);
impl Default for Time {
    fn default() -> Self {
        Self(AtomicI64::new(NOW))
    }
}
impl Clock for Time {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
impl Time {
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}
#[derive(Default)]
pub struct MemorySecrets(pub Mutex<BTreeMap<String, Vec<u8>>>);
#[async_trait]
impl Secrets for MemorySecrets {
    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().await.get(name).cloned())
    }
    async fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        self.0.lock().await.insert(name.to_owned(), value.to_vec());
        Ok(())
    }
    async fn create(&self, name: &str, value: &[u8]) -> Result<bool> {
        let mut s = self.0.lock().await;
        if s.contains_key(name) {
            return Ok(false);
        }
        s.insert(name.to_owned(), value.to_vec());
        Ok(true)
    }
    async fn remove(&self, name: &str) -> Result<()> {
        self.0.lock().await.remove(name);
        Ok(())
    }
}
pub type Step = Box<dyn FnOnce(Request) -> Result<Response> + Send>;
#[derive(Default)]
pub struct Relay(pub Mutex<VecDeque<Step>>);
impl Relay {
    pub async fn add(&self, step: impl FnOnce(Request) -> Result<Response> + Send + 'static) {
        self.0.lock().await.push_back(Box::new(step));
    }
    pub async fn done(&self) {
        assert!(
            self.0.lock().await.is_empty(),
            "unconsumed protocol exchange"
        );
    }
}
#[async_trait]
impl Transport for Relay {
    async fn send(&self, request: Request) -> Result<Response> {
        let step = self
            .0
            .lock()
            .await
            .pop_front()
            .expect("unexpected network request");
        step(request)
    }
}
pub fn response(status: u16, body: Value) -> Response {
    Response {
        status,
        headers: BTreeMap::new(),
        body: serde_json::to_vec(&body).expect("json"),
    }
}
pub fn descriptor(id: &str) -> t3_contracts::environment::ExecutionEnvironmentDescriptor {
    serde_json::from_value(json!({"environmentId":id,"label":id,"platform":{"os":"windows","arch":"x64"},"serverVersion":"1.0.0","capabilities":{"repositoryIdentity":false,"mcpModeLimitHeader":true,"linkFence":true}})).expect("descriptor")
}
pub fn runtime() -> t3_contracts::relay::RelayManagedEndpointRuntimeConfig {
    serde_json::from_value(json!({"providerKind":"cloudflare_tunnel","connectorToken":"marker-connector-secret","tunnelId":"tunnel-1","tunnelName":"t3-test"})).expect("runtime")
}
pub async fn cloud() -> (
    Arc<t3_cloud::cloud::CloudLink>,
    Arc<MemorySecrets>,
    Arc<Relay>,
    Arc<Time>,
    t3_cloud::crypto::EnvironmentKey,
) {
    let secrets = Arc::new(MemorySecrets::default());
    let relay = Arc::new(Relay::default());
    let time = Arc::new(Time::default());
    let mint = t3_cloud::crypto::EnvironmentKey::generate().expect("key");
    let cloud = Arc::new(
        t3_cloud::cloud::CloudLink::new(
            descriptor("env-one"),
            secrets.clone(),
            relay.clone(),
            time.clone(),
        )
        .await
        .expect("cloud"),
    );
    cloud.install(t3_cloud::http::Sensitive::new(serde_json::from_value(json!({"relayUrl":"https://relay.test","relayIssuer":"https://issuer.test","cloudUserId":"user-one","environmentCredential":"marker-environment-secret","cloudMintPublicKey":mint.public_key,"endpointRuntime":runtime()})).expect("config"))).await.expect("install");
    (cloud, secrets, relay, time, mint)
}
pub fn remote_error(rejection: t3_cloud::Rejection) -> Error {
    Error::Remote {
        rejection,
        retry_after_ms: None,
    }
}
