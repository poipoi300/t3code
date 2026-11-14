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

#[cfg(feature = "auth")]
pub async fn auth_at(temp: &tempfile::TempDir, now: i64) -> t3_auth::EnvironmentAuth {
    let path = temp.path().join("state.sqlite");
    let db = tokio::task::spawn_blocking(move || t3_persistence::Database::open(path))
        .await
        .unwrap()
        .unwrap();
    let secrets = t3_auth::ServerSecretStore::open(temp.path().join("secrets"))
        .await
        .unwrap();
    t3_auth::EnvironmentAuth::new(
        db,
        secrets,
        t3_auth::AuthConfig {
            port: 3773,
            host: Some("127.0.0.1".into()),
            environment_id: "env-one".into(),
            state_dir: temp.path().to_string_lossy().into_owned(),
            ..Default::default()
        },
        None,
        t3_auth::clock::TokioClock::at(now),
    )
    .await
    .unwrap()
}

#[cfg(feature = "auth")]
pub async fn issuer() -> (t3_cloud::cloud::AuthCredentialIssuer, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("fixture directory");
    let auth = auth_at(&temp, NOW).await;
    let settings = t3_config::settings::SettingsService::new(temp.path().join("settings.json"));
    (
        t3_cloud::cloud::AuthCredentialIssuer::new(auth, settings),
        temp,
    )
}
