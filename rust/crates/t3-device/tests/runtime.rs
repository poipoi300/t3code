#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use common::*;
use futures_util::StreamExt;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use t3_contracts::{device::*, settings::ServerSettings};
use t3_device::{
    DeviceRuntime, Error, Result, RuntimeInputs,
    host::{DeviceHost, HostFactory},
    service::{Accounts, DeviceSettings, MemorySettings, ToolInstaller, configuration_patch},
};
use tokio::sync::{Mutex, mpsc};

struct Factory {
    hosts: Mutex<Vec<Arc<FakeHost>>>,
}
#[async_trait]
impl HostFactory for Factory {
    async fn create(&self, config: &DeviceHostConfig) -> Result<Arc<dyn DeviceHost>> {
        let (id, kind) = match config {
            DeviceHostConfig::SshDeviceHostConfig(c) => (c.id.as_ref(), DeviceHostKind::Ssh),
            DeviceHostConfig::EasDeviceHostConfig(c) => (c.id.as_ref(), DeviceHostKind::Eas),
            DeviceHostConfig::SimlockDeviceHostConfig(c) => {
                (c.id.as_ref(), DeviceHostKind::Simlock)
            }
        };
        let mut host = FakeHost::new(
            id,
            FakeOps::new(id, DevicePlatform::Ios, true),
            FakeCommands::encoder(),
            None,
        );
        Arc::get_mut(&mut host).unwrap().kind = kind;
        self.hosts.lock().await.push(host.clone());
        Ok(host)
    }
    async fn test(&self, _: &SshDeviceHostConfig) -> Result<DeviceHostSummary> {
        Err(Error::Unavailable("fixture has no SSH transport".into()))
    }
}
struct Unused;
#[async_trait]
impl ToolInstaller for Unused {
    async fn install(&self, _: DeviceUpdateTool) -> Result<()> {
        panic!("startup installed a tool")
    }
}
#[async_trait]
impl Accounts for Unused {
    async fn eas_account(&self, _: DeviceEasAccountInput) -> Result<DeviceEasAccount> {
        panic!("startup called Expo")
    }
    async fn simlock_identity(
        &self,
        _: DeviceSimlockIdentityInput,
    ) -> Result<DeviceSimlockIdentity> {
        panic!("startup called Simlock")
    }
}
#[async_trait]
impl t3_device::proxy::SocketConnector for Unused {
    async fn connect(
        &self,
        _: String,
        _: std::collections::BTreeMap<String, t3_device::io::Secret>,
        _: Vec<t3_device::io::Secret>,
    ) -> Result<t3_device::proxy::SocketConnection> {
        panic!("startup connected a device")
    }
}
struct Deny(AtomicUsize);
#[async_trait]
impl t3_device::proxy::Authenticator for Deny {
    async fn authorize(
        &self,
        _: &http::HeaderMap,
        _: &[(String, String)],
        _: t3_device::proxy::Scope,
    ) -> std::result::Result<(), t3_device::proxy::AuthFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(t3_device::proxy::AuthFailure(
            http::StatusCode::UNAUTHORIZED,
        ))
    }
}
fn inputs(
    settings: Arc<MemorySettings>,
    changes: mpsc::UnboundedReceiver<ServerSettings>,
    factory: Arc<Factory>,
    local: Arc<FakeHost>,
    auth: Arc<Deny>,
) -> RuntimeInputs {
    RuntimeInputs {
        local_host: local,
        host_factory: factory,
        launchers: FakeLaunchers::new(),
        installer: Arc::new(Unused),
        accounts: Arc::new(Unused),
        settings,
        settings_changes: changes,
        http: FakeHttp::new([]),
        auth,
        sockets: Arc::new(Unused),
    }
}
fn ssh(label: &str) -> DeviceHostConfig {
    serde_json::from_value(json!({"kind":"ssh","id":"remote","label":label,"target":"remote.test"}))
        .unwrap()
}

#[tokio::test]
async fn runtime_reconciles_current_settings_recovers_from_errors_and_joins_shutdown() {
    let settings = settings(true, true);
    settings.0.lock().await.hosts = vec![ssh("before")];
    let local = FakeHost::new(
        "local",
        FakeOps::new("local", DevicePlatform::Ios, true),
        FakeCommands::encoder(),
        None,
    );
    let factory = Arc::new(Factory {
        hosts: Mutex::new(vec![]),
    });
    let auth = Arc::new(Deny(AtomicUsize::new(0)));
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(ServerSettings::default()).unwrap();
    let runtime = DeviceRuntime::new(inputs(
        settings.clone(),
        rx,
        factory.clone(),
        local.clone(),
        auth,
    ))
    .await
    .unwrap();
    let mut status = runtime.settings_status();
    status.changed().await.unwrap();
    assert!(status.borrow_and_update().is_none());
    assert_eq!(runtime.service.state().await.hosts.len(), 2);
    assert_eq!(local.starts.load(Ordering::SeqCst), 0);
    settings.0.lock().await.hosts = vec![ssh("after")];
    tx.send(ServerSettings::default()).unwrap();
    status.changed().await.unwrap();
    assert!(status.borrow_and_update().is_none());
    assert_eq!(factory.hosts.lock().await.len(), 2);
    assert_eq!(
        factory.hosts.lock().await[0].stops.load(Ordering::SeqCst),
        1
    );
    let mut invalid = ssh("invalid");
    let DeviceHostConfig::SshDeviceHostConfig(config) = &mut invalid else {
        unreachable!()
    };
    config.id = "..".into();
    settings.0.lock().await.hosts = vec![invalid];
    tx.send(ServerSettings::default()).unwrap();
    status.changed().await.unwrap();
    assert!(status.borrow_and_update().is_some());
    assert_eq!(runtime.service.state().await.hosts.len(), 2);
    assert_eq!(
        factory.hosts.lock().await[1].stops.load(Ordering::SeqCst),
        0
    );
    settings.0.lock().await.hosts.clear();
    tx.send(ServerSettings::default()).unwrap();
    status.changed().await.unwrap();
    assert!(status.borrow_and_update().is_none());
    assert_eq!(runtime.service.state().await.hosts.len(), 1);
    assert_eq!(
        factory.hosts.lock().await[1].stops.load(Ordering::SeqCst),
        1
    );
    runtime.shutdown().await.unwrap();
    runtime.shutdown().await.unwrap();
    assert_eq!(local.stops.load(Ordering::SeqCst), 1);
    assert!(settings.0.lock().await.enabled);
    assert!(tx.send(ServerSettings::default()).is_err());
}

#[tokio::test]
async fn runtime_exposes_wire_state_and_the_same_proxy_admission_boundary() {
    use tower::ServiceExt;
    let settings = settings(false, false);
    settings.0.lock().await.hosts=vec![serde_json::from_value(json!({"kind":"eas","id":"cloud","label":"Cloud","appId":"12345678-1234-1234-1234-123456789abc","accessToken":""})).unwrap()];
    let local = FakeHost::new(
        "local",
        FakeOps::new("local", DevicePlatform::Ios, true),
        FakeCommands::encoder(),
        None,
    );
    let (_tx, rx) = mpsc::unbounded_channel();
    let auth = Arc::new(Deny(AtomicUsize::new(0)));
    let runtime = DeviceRuntime::new(inputs(
        settings,
        rx,
        Arc::new(Factory {
            hosts: Mutex::new(vec![]),
        }),
        local,
        auth.clone(),
    ))
    .await
    .unwrap();
    let states = runtime.state_stream().await;
    tokio::pin!(states);
    let state = states.next().await.unwrap();
    assert_eq!(state.hosts.len(), 1);
    assert_eq!(state.eas_hosts.unwrap().unwrap()[0].id.as_ref(), "cloud");
    for (path, status, count) in [
        (
            "/api/device-hub/vendor/serve-sim/helper/%252e%252e/config",
            404,
            0,
        ),
        ("/api/device-hub/api/devices", 401, 1),
    ] {
        let response = runtime
            .router()
            .oneshot(
                http::Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(auth.0.load(Ordering::SeqCst), count);
    }
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn canonical_settings_keep_saved_pool_credentials_when_pool_hosts_are_disabled() {
    let mut canonical = ServerSettings {
        enable_device_support: true,
        enable_agent_device_access: true,
        device_onboarding_completed: true,
        ..Default::default()
    };
    canonical.simlock_device_hosts=vec![serde_json::from_value(json!({"kind":"simlock","id":"pool","label":"Pool","gatewayUrl":"https://pool.test","agentToken":"saved"})).unwrap()];
    let mapped = DeviceSettings::from_server(&canonical);
    assert!(mapped.hosts.is_empty());
    assert!(mapped.enabled && mapped.agent_access_enabled && mapped.onboarding_completed);
    let temp = tempfile::tempdir().unwrap();
    let accounts = t3_device::cloud::AccountService {
        settings: Arc::new(MemorySettings(Mutex::new(mapped))),
        secrets: Arc::new(t3_device::cloud::LiteralSecrets),
        http: FakeHttp::new([(200, json!({"role":"agent","name":"QA"}))]),
        environment: Default::default(),
        expo_home: temp.path().into(),
    };
    let identity = accounts
        .simlock_identity(
            serde_json::from_value(json!({"hostId":"pool","gatewayUrl":"https://pool.test"}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(identity.token_source, DeviceSimlockTokenSource::Settings);
    canonical.simlock_device_hosts_enabled = true;
    canonical.device_hosts = vec![
        serde_json::from_value(
            json!({"kind":"ssh","id":"pool","label":"SSH","target":"remote.test"}),
        )
        .unwrap(),
    ];
    canonical.eas_device_hosts = vec![serde_json::from_value(json!({"kind":"eas","id":"pool","label":"EAS","appId":"12345678-1234-1234-1234-123456789abc","accessToken":"expo-saved"})).unwrap()];
    let mapped = DeviceSettings::from_server(&canonical);
    assert_eq!(mapped.hosts.len(), 1);
    assert!(matches!(
        &mapped.hosts[0],
        DeviceHostConfig::SshDeviceHostConfig(config) if config.label.as_ref() == "SSH"
    ));
    assert_eq!(mapped.account_hosts.as_ref().unwrap().len(), 3);
    let http = FakeHttp::new([(401, json!({}))]);
    let accounts = t3_device::cloud::AccountService {
        settings: Arc::new(MemorySettings(Mutex::new(mapped))),
        secrets: Arc::new(t3_device::cloud::LiteralSecrets),
        http: http.clone(),
        environment: [("EXPO_TOKEN".into(), "env-fallback".into())].into(),
        expo_home: temp.path().into(),
    };
    assert!(
        accounts
            .eas_account(serde_json::from_value(json!({"hostId":"pool"})).unwrap())
            .await
            .is_err()
    );
    assert_eq!(
        http.requests.lock().await[0].headers["authorization"].expose(),
        "Bearer expo-saved"
    );
    let patch = configuration_patch(
        &serde_json::from_value(json!({"enabled":false,"agentAccessEnabled":true})).unwrap(),
    );
    assert_eq!(
        serde_json::to_value(patch).unwrap(),
        json!({"enableDeviceSupport":false,"enableAgentDeviceAccess":true})
    );
}
#[tokio::test]
async fn runtime_preflights_pool_accounts_only_with_device_support_enabled() {
    for enabled in [true, false] {
        let settings = settings(enabled, false);
        settings.0.lock().await.hosts=vec![serde_json::from_value(json!({"kind":"simlock","id":"pool","label":"Pool","gatewayUrl":"https://pool.test","agentToken":"saved"})).unwrap()];
        let local = FakeHost::new(
            "local",
            FakeOps::new("local", DevicePlatform::Ios, true),
            FakeCommands::encoder(),
            None,
        );
        let factory = Arc::new(Factory {
            hosts: Mutex::new(vec![]),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let runtime = DeviceRuntime::new(inputs(
            settings,
            rx,
            factory.clone(),
            local,
            Arc::new(Deny(AtomicUsize::new(0))),
        ))
        .await
        .unwrap();
        let pool = factory.hosts.lock().await[0].clone();
        if enabled {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                pool.availability_changed.notified(),
            )
            .await
            .expect("enabled pool must be checked at startup");
        }
        let mut status = runtime.settings_status();
        tx.send(ServerSettings::default()).unwrap();
        status.changed().await.unwrap();
        assert_eq!(
            pool.availability_calls.load(Ordering::SeqCst),
            usize::from(enabled)
        );
        runtime.shutdown().await.unwrap();
    }
}
