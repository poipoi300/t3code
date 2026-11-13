#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use common::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use t3_contracts::device::*;
use t3_device::{
    Error, Result,
    cloud::{
        LiteralSecrets,
        eas::EasHost,
        expo::{Active, Create, ExpoApi, ExpoError, ExpoProvider, ExpoResult, Session},
        simlock::{SimlockHost, SimlockOptions},
    },
    host::{AgentEndpoint, DeviceHost, DeviceOperations, OpenContext, Routing, SessionRelease},
    io::{CommandOutput, Http, HttpRequest, HttpResponse, Secret},
};
use tokio::sync::Mutex;

struct FakeExpo {
    creates: Mutex<Vec<Create>>,
    stops: Mutex<Vec<String>>,
    active: Mutex<Vec<Active>>,
    fail_create: AtomicUsize,
    fail_stop: AtomicBool,
    ended: AtomicBool,
    gate: Mutex<Option<Arc<Gate>>>,
    session_gate: Mutex<Option<Arc<Gate>>>,
}
impl FakeExpo {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            creates: Mutex::new(vec![]),
            stops: Mutex::new(vec![]),
            active: Mutex::new(vec![]),
            fail_create: AtomicUsize::new(0),
            fail_stop: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            gate: Mutex::new(None),
            session_gate: Mutex::new(None),
        })
    }
}
fn expo_error(rejected: bool) -> ExpoError {
    ExpoError {
        operation: "fake operation".into(),
        detail: "Not allowed".into(),
        rejected,
        status: Some(400),
    }
}
#[async_trait]
impl ExpoApi for FakeExpo {
    async fn viewer(&self) -> ExpoResult<(String, Vec<DeviceEasApp>)> {
        Ok(("test".into(), vec![]))
    }
    async fn availability(&self, _: &str) -> ExpoResult<(String, bool)> {
        Ok(("account".into(), true))
    }
    async fn create(&self, input: Create) -> ExpoResult<String> {
        let name = input.name.clone();
        self.creates.lock().await.push(input);
        let gate = self.gate.lock().await.take();
        if let Some(gate) = gate {
            gate.wait().await;
        }
        let failure = self.fail_create.load(Ordering::SeqCst);
        if failure != 0 {
            return Err(expo_error(failure == 1));
        }
        let id = format!("session-{}", self.creates.lock().await.len());
        self.active.lock().await.push(Active {
            id: id.clone(),
            name: Some(name),
            status: "IN_PROGRESS".into(),
        });
        Ok(id)
    }
    async fn session(&self, id: &str) -> ExpoResult<Session> {
        let gate = self.session_gate.lock().await.take();
        if let Some(gate) = gate {
            gate.wait().await;
        }
        Ok(Session {
            id: id.into(),
            status: if self.ended.load(Ordering::SeqCst) {
                "STOPPED"
            } else {
                "IN_PROGRESS"
            }
            .into(),
            agent: Some(AgentEndpoint {
                base_url: "https://agent.test".into(),
                token: Secret::new("agent-token"),
                entry_path: PathBuf::new(),
                agent_device_id: None,
                session: None,
                state_dir: None,
                install_directory: None,
            }),
            preview: Some((
                "https://preview.test/?token=preview-token".into(),
                Secret::new("preview-token"),
            )),
        })
    }
    async fn active(&self, _: &str) -> ExpoResult<Vec<Active>> {
        Ok(self.active.lock().await.clone())
    }
    async fn stop(&self, id: &str) -> ExpoResult<()> {
        self.stops.lock().await.push(id.into());
        if self.fail_stop.load(Ordering::SeqCst) {
            return Err(expo_error(false));
        }
        self.active.lock().await.retain(|s| s.id != id);
        Ok(())
    }
}
struct Provider {
    api: Arc<FakeExpo>,
    readable: AtomicBool,
}
#[async_trait]
impl ExpoProvider for Provider {
    async fn api(&self) -> Result<Arc<dyn ExpoApi>> {
        if !self.readable.load(Ordering::SeqCst) {
            return Err(Error::Unavailable("secret provider unavailable".into()));
        }
        Ok(self.api.clone())
    }
}
struct Preview {
    requests: Mutex<Vec<HttpRequest>>,
    device_override: Mutex<Option<String>>,
}
#[async_trait]
impl Http for Preview {
    async fn execute(&self, r: HttpRequest) -> Result<HttpResponse> {
        assert_eq!(
            r.headers["authorization"].expose(),
            if r.url.host_str() == Some("agent.test") {
                "Bearer agent-token"
            } else {
                "Bearer preview-token"
            }
        );
        let body = if r.url.path().ends_with("screenshot") {
            vec![137, 80, 78, 71]
        } else if r.url.path() == "/rpc" {
            serde_json::to_vec(&json!({"result":{"ok":true}})).unwrap()
        } else if r.url.path() == "/vendor/serve-emu/api" {
            serde_json::to_vec(&json!({"serial":"emulator-5554"})).unwrap()
        } else {
            assert_eq!(r.url.path(), "/api");
            let id = self
                .device_override
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| "UDID-1".into());
            serde_json::to_vec(&json!({"device":id})).unwrap()
        };
        self.requests.lock().await.push(r);
        Ok(HttpResponse::bytes(200, body))
    }
}
fn eas_config(limit: u64) -> EasDeviceHostConfig {
    serde_json::from_value(json!({"kind":"eas","id":"eas","label":"Cloud","appId":"app-1","accessToken":"token","maxConcurrentSessions":limit,"maxSessionMinutes":30,"idleMinutes":10})).unwrap()
}
async fn eas_fixture(
    limit: u64,
) -> (
    tempfile::TempDir,
    EasHost,
    Arc<FakeExpo>,
    Arc<Provider>,
    Arc<Preview>,
) {
    let temp = tempfile::tempdir().unwrap();
    let api = FakeExpo::new();
    let provider = Arc::new(Provider {
        api: api.clone(),
        readable: AtomicBool::new(true),
    });
    let http = Arc::new(Preview {
        requests: Mutex::new(vec![]),
        device_override: Mutex::new(None),
    });
    let host = EasHost::new(
        eas_config(limit),
        temp.path().into(),
        provider.clone(),
        http.clone(),
    )
    .unwrap();
    host.ensure_ready(Arc::new(|_, _| {})).await.unwrap();
    (temp, host, api, provider, http)
}

#[tokio::test]
async fn eas_refuses_option_like_remote_selector_and_stops_created_session() {
    let (_temp, host, api, _provider, http) = eas_fixture(1).await;
    *http.device_override.lock().await = Some("--config".into());
    let device = host.list().await.unwrap().devices[0].clone();
    assert!(host.boot(&device, &OpenContext::default()).await.is_err());
    assert_eq!(api.stops.lock().await.as_slice(), ["session-1"]);
    assert!(host.list().await.unwrap().devices.iter().all(|d| !d.booted));
}

#[tokio::test]
async fn eas_creates_bounded_sessions_routes_platforms_and_releases_claims() {
    let (_temp, host, api, _provider, http) = eas_fixture(2).await;
    let catalog = host.list().await.unwrap().devices;
    assert_eq!(
        catalog.iter().map(|d| d.id.as_ref()).collect::<Vec<_>>(),
        ["eas-ios", "eas-android"]
    );
    let ios = host
        .boot(&catalog[0], &OpenContext::default())
        .await
        .unwrap();
    let android = host
        .boot(&catalog[1], &OpenContext::default())
        .await
        .unwrap();
    assert_eq!(ios, "session-1");
    assert_eq!(android, "session-2");
    let creates = api.creates.lock().await;
    assert_eq!(creates[0].max_minutes, 30);
    assert_eq!(creates[0].idle_minutes, 10);
    assert_eq!(creates[1].platform, DevicePlatform::Android);
    assert_eq!(creates[0].app, "app-1");
    drop(creates);
    let route = host
        .upstream("/vendor/serve-sim/helper/session-1/stream.avcc", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(route.url, "https://preview.test/helper/UDID-1/stream.avcc");
    assert_eq!(
        route.headers["authorization"].expose(),
        "Bearer preview-token"
    );
    assert_eq!(route.protocols[0].expose(), "serve-sim.token.preview-token");
    let route = host
        .upstream(
            "/vendor/serve-emu/ws",
            &[("device".into(), android.clone())],
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        route.url,
        "https://preview.test/vendor/serve-emu/ws?device=emulator-5554"
    );
    assert!(
        host.upstream("/vendor/serve-sim/api", &[])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        host.upstream("/vendor/serve-sim/helper/%ZZ/config", &[])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        host.upstream("/vendor/serve-emu/ws", &[("device".into(), ios.clone())])
            .await
            .unwrap()
            .is_none()
    );
    let endpoint = host.agent_endpoint(&android).await.unwrap().unwrap();
    assert_eq!(endpoint.agent_device_id.as_deref(), Some("emulator-5554"));
    host.release("thread-session", &ios).await.unwrap();
    assert!(api.stops.lock().await.is_empty());
    assert_eq!(
        http.requests.lock().await.last().unwrap().url.path(),
        "/rpc"
    );
    host.stop().await.unwrap();
    assert_eq!(
        api.stops.lock().await.as_slice(),
        ["session-1", "session-2"]
    );
    assert_eq!(host.list().await.unwrap().devices.len(), 2);
}
#[tokio::test]
async fn eas_session_limit_is_reserved_before_concurrent_create() {
    let (_temp, host, api, _provider, _http) = eas_fixture(1).await;
    let device = host.list().await.unwrap().devices[0].clone();
    let gate = Gate::new();
    *api.gate.lock().await = Some(gate.clone());
    let copy = host.clone();
    let d = device.clone();
    let opening = tokio::spawn(async move { copy.boot(&d, &OpenContext::default()).await });
    gate.entered.notified().await;
    assert!(
        host.boot(&device, &OpenContext::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("limited to 1 running session")
    );
    assert_eq!(api.creates.lock().await.len(), 1);
    gate.release.notify_one();
    opening.await.unwrap().unwrap();
    host.stop().await.unwrap();
}
#[tokio::test]
async fn eas_writes_ledger_before_create_and_refuses_corrupt_records() {
    let (temp, host, api, _provider, _http) = eas_fixture(1).await;
    let path = temp.path().join("device/eas/eas.json");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, "not a ledger").await.unwrap();
    let device = host.list().await.unwrap().devices[0].clone();
    assert!(host.boot(&device, &OpenContext::default()).await.is_err());
    assert!(api.creates.lock().await.is_empty());
    assert_eq!(
        tokio::fs::read_to_string(path).await.unwrap(),
        "not a ledger"
    );
}
#[tokio::test]
async fn eas_unreadable_secret_blocks_new_sessions_but_retains_stop_credential() {
    let (_temp, host, api, provider, _http) = eas_fixture(2).await;
    let device = host.list().await.unwrap().devices[0].clone();
    host.boot(&device, &OpenContext::default()).await.unwrap();
    provider.readable.store(false, Ordering::SeqCst);
    assert!(host.boot(&device, &OpenContext::default()).await.is_err());
    assert_eq!(api.creates.lock().await.len(), 1);
    host.stop().await.unwrap();
    assert_eq!(api.stops.lock().await.as_slice(), ["session-1"]);
}
#[tokio::test]
async fn eas_failed_stop_stays_listed_counted_and_is_retried() {
    let (_temp, host, api, _provider, _http) = eas_fixture(1).await;
    let device = host.list().await.unwrap().devices[0].clone();
    let id = host.boot(&device, &OpenContext::default()).await.unwrap();
    api.fail_stop.store(true, Ordering::SeqCst);
    assert!(host.shutdown(&id, DevicePlatform::Ios).await.is_err());
    assert!(
        host.list()
            .await
            .unwrap()
            .devices
            .iter()
            .any(|d| d.id.as_ref() == id)
    );
    assert!(host.boot(&device, &OpenContext::default()).await.is_err());
    api.fail_stop.store(false, Ordering::SeqCst);
    assert_eq!(host.list().await.unwrap().devices.len(), 2);
    assert!(api.stops.lock().await.len() >= 2);
    host.boot(&device, &OpenContext::default()).await.unwrap();
    host.stop().await.unwrap();
}
#[tokio::test]
async fn eas_ended_session_frees_slot_even_when_stop_keeps_failing() {
    let (_temp, host, api, _provider, _http) = eas_fixture(1).await;
    let device = host.list().await.unwrap().devices[0].clone();
    let id = host.boot(&device, &OpenContext::default()).await.unwrap();
    api.fail_stop.store(true, Ordering::SeqCst);
    assert!(host.shutdown(&id, DevicePlatform::Ios).await.is_err());
    api.ended.store(true, Ordering::SeqCst);
    assert_eq!(host.list().await.unwrap().devices.len(), 2);
    api.ended.store(false, Ordering::SeqCst);
    host.boot(&device, &OpenContext::default()).await.unwrap();
    api.fail_stop.store(false, Ordering::SeqCst);
    host.stop().await.unwrap();
}
#[tokio::test]
async fn eas_refused_create_frees_slot_but_unknown_create_holds_it() {
    for failure in [1, 2] {
        let (temp, host, api, _provider, _http) = eas_fixture(1).await;
        api.fail_create.store(failure, Ordering::SeqCst);
        let device = host.list().await.unwrap().devices[0].clone();
        assert!(host.boot(&device, &OpenContext::default()).await.is_err());
        let entries: Value = serde_json::from_slice(
            &tokio::fs::read(temp.path().join("device/eas/eas.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            entries[0]["refused"],
            if failure == 1 {
                json!(true)
            } else {
                Value::Null
            }
        );
        api.fail_create.store(0, Ordering::SeqCst);
        let result = host.boot(&device, &OpenContext::default()).await;
        assert_eq!(result.is_ok(), failure == 1);
        host.stop().await.unwrap();
    }
}
#[tokio::test]
async fn eas_stop_during_start_cleans_the_created_session() {
    let (_temp, host, api, _provider, _http) = eas_fixture(1).await;
    let device = host.list().await.unwrap().devices[0].clone();
    let gate = Gate::new();
    *api.session_gate.lock().await = Some(gate.clone());
    let copy = host.clone();
    let opening = tokio::spawn(async move { copy.boot(&device, &OpenContext::default()).await });
    gate.entered.notified().await;
    host.stop().await.unwrap();
    assert!(opening.await.unwrap().is_err());
    assert_eq!(api.stops.lock().await.as_slice(), ["session-1"]);
    assert_eq!(host.list().await.unwrap().devices.len(), 2);
}

struct Pool {
    requests: Mutex<Vec<HttpRequest>>,
    catalog: Mutex<Value>,
    renew_404: AtomicBool,
    renewed: tokio::sync::Notify,
    closed: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl Http for Pool {
    async fn execute(&self, r: HttpRequest) -> Result<HttpResponse> {
        assert_eq!(r.headers["authorization"].expose(), "Bearer agent-token");
        let (status, body) = if r.url.path() == "/v1/catalog" {
            (200, self.catalog.lock().await.clone())
        } else if r.url.path().ends_with("/renew") {
            let body: Value = serde_json::from_slice(r.body.as_ref().unwrap()).unwrap();
            assert!(body.get("ttlMs").is_none());
            (
                if self.renew_404.load(Ordering::SeqCst) {
                    404
                } else {
                    200
                },
                json!({}),
            )
        } else if r.method == http::Method::DELETE {
            (202, json!({}))
        } else {
            panic!("unexpected pool route")
        };
        let renew = r.url.path().ends_with("/renew");
        self.requests.lock().await.push(r);
        if renew {
            self.renewed.notify_one();
        }
        Ok(HttpResponse::bytes(
            status,
            serde_json::to_vec(&body).unwrap(),
        ))
    }
}
fn pool_catalog() -> Value {
    json!({"platforms":[{"platform":"ios","modelRuntimes":{"iPhone 17 Pro":["27.0"],"Apple TV 4K":["27.0"],"iPhone 12":[]},"modelClasses":{"iPhone 17 Pro":"phone","Apple TV 4K":"tv","iPhone 12":"phone"}},{"platform":"android","modelRuntimes":{"Pixel 9":["36"],"Pixel 8":["36"]},"modelWorkers":{"Pixel 9":["worker-a"],"Pixel 8":[]}}]})
}
async fn simlock_fixture(
    limit: u64,
    failed: bool,
) -> (tempfile::TempDir, SimlockHost, Arc<FakeCommands>, Arc<Pool>) {
    simlock_fixture_with_minutes(limit, failed, 60).await
}
async fn simlock_fixture_with_minutes(
    limit: u64,
    failed: bool,
    minutes: u64,
) -> (tempfile::TempDir, SimlockHost, Arc<FakeCommands>, Arc<Pool>) {
    let temp = tempfile::tempdir().unwrap();
    let leases = Arc::new(AtomicUsize::new(0));
    let count = leases.clone();
    let closed = Arc::new(tokio::sync::Notify::new());
    let closing = closed.clone();
    let commands = FakeCommands::new(move |c| {
        let args = &c.args;
        let config = &args[args.iter().position(|a| a == "--config").unwrap() + 1];
        let session = &args[args.iter().position(|a| a == "--session").unwrap() + 1];
        let config: Value = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
        let dir = PathBuf::from(config["stateDir"].as_str().unwrap());
        assert_eq!(config["daemonAuthToken"], "agent-token");
        if args[1] == "open" {
            if failed {
                return Ok(CommandOutput{code:1,stdout:json!({"error":{"message":"Invalid daemon response","details":{"line":"404 Not Found agent-token"}}}).to_string(),stderr:String::new()});
            }
            let id = count.fetch_add(1, Ordering::SeqCst) + 1;
            std::fs::create_dir_all(dir.join("remote-connections")).unwrap();
            std::fs::write(
                dir.join("remote-connections")
                    .join(format!("{session}.json")),
                json!({"leaseId":format!("lease-{id}")}).to_string(),
            )
            .unwrap();
        }
        if args[1] == "screenshot" {
            std::fs::write(&args[2], [1, 2, 3]).unwrap();
        }
        if args[1] == "close" {
            closing.notify_one();
        }
        Ok(CommandOutput {
            stdout: "{}".into(),
            ..Default::default()
        })
    });
    let pool = Arc::new(Pool {
        requests: Mutex::new(vec![]),
        catalog: Mutex::new(pool_catalog()),
        renew_404: AtomicBool::new(false),
        renewed: tokio::sync::Notify::new(),
        closed,
    });
    let config:SimlockDeviceHostConfig=serde_json::from_value(json!({"kind":"simlock","id":"pool","label":"Pool","gatewayUrl":"https://simlock.test:4700/","agentToken":"agent-token","maxLeases":limit,"maxLeaseMinutes":minutes})).unwrap();
    let host = SimlockHost::new(SimlockOptions {
        config,
        state: temp.path().into(),
        node: "node".into(),
        agent_entry: "/tools/agent-device.mjs".into(),
        cli_provider: None,
        commands: commands.clone(),
        http: pool.clone(),
        secrets: Arc::new(LiteralSecrets),
        environment: BTreeMap::new(),
    })
    .unwrap();
    host.ensure_ready(Arc::new(|_, _| {})).await.unwrap();
    (temp, host, commands, pool)
}
#[tokio::test]
async fn simlock_catalog_filters_unavailable_workers_and_non_mobile_models() {
    let (_temp, host, _commands, pool) = simlock_fixture(2, false).await;
    let list = host.list().await.unwrap().devices;
    assert_eq!(
        list.iter().map(|d| d.id.as_ref()).collect::<Vec<_>>(),
        ["simlock:ios:iPhone 17 Pro", "simlock:android:Pixel 9"]
    );
    assert_eq!(list[0].version, "iOS 27.0");
    assert_eq!(list[1].version, "Android 36");
    host.list().await.unwrap();
    assert_eq!(pool.requests.lock().await.len(), 1);
    host.stop().await.unwrap();
}

struct InstalledCli { calls: AtomicUsize, fail: AtomicBool }
#[async_trait]
impl t3_device::host::AgentCliProvider for InstalledCli {
    async fn ensure(&self) -> Result<t3_device::host::AgentCli> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) { return Err(Error::Unavailable("install failed".into())); }
        Ok(t3_device::host::AgentCli { node: "bound-node".into(), entry: "/installed/agent.mjs".into() })
    }
}
#[tokio::test]
async fn simlock_installs_lazily_retries_failures_and_uses_the_installed_cli() {
    let (temp, previous, commands, pool) = simlock_fixture(1, false).await;
    previous.stop().await.unwrap();
    let provider = Arc::new(InstalledCli { calls: AtomicUsize::new(0), fail: AtomicBool::new(true) });
    let config = serde_json::from_value(json!({"kind":"simlock","id":"pool","label":"Pool","gatewayUrl":"https://simlock.test:4700","agentToken":"agent-token"})).unwrap();
    let host = SimlockHost::new(SimlockOptions { config, state: temp.path().into(), node: "wrong-node".into(), agent_entry: "/wrong-entry".into(), cli_provider: Some(provider.clone()), commands: commands.clone(), http: pool, secrets: Arc::new(LiteralSecrets), environment: BTreeMap::new() }).unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(host.ensure_ready(Arc::new(|_, _| {})).await.is_err());
    assert!(host.current().await.is_none());
    assert!(commands.calls.lock().unwrap().is_empty());
    provider.fail.store(false, Ordering::SeqCst);
    for _ in 0..2 { assert_eq!(host.ensure_ready(Arc::new(|_, _| {})).await.unwrap().node_path, "bound-node"); }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    let device = host.list().await.unwrap().devices[0].clone();
    let id = host.boot(&device, &OpenContext::default()).await.unwrap();
    assert_eq!(host.agent_endpoint(&id).await.unwrap().unwrap().entry_path, PathBuf::from("/installed/agent.mjs"));
    let call = commands.calls.lock().unwrap()[0].clone();
    assert_eq!(call.program, "bound-node");
    assert_eq!(call.args[0], "/installed/agent.mjs");
    host.stop().await.unwrap();
}

struct GatedOpen { commands: Arc<FakeCommands>, gate: Arc<Gate> }
#[async_trait]
impl t3_device::io::Commands for GatedOpen {
    async fn run(&self, command: Command) -> Result<CommandOutput> {
        if command.args.get(1).is_some_and(|arg| arg == "open") { self.gate.wait().await; }
        t3_device::io::Commands::run(self.commands.as_ref(), command).await
    }
}
#[tokio::test]
async fn simlock_stop_during_open_closes_the_partial_session_and_removes_its_state() {
    let (temp, previous, commands, pool) = simlock_fixture(1, false).await;
    previous.stop().await.unwrap();
    let gate = Gate::new();
    let config = serde_json::from_value(json!({"kind":"simlock","id":"pool","label":"Pool","gatewayUrl":"https://simlock.test:4700","agentToken":"agent-token"})).unwrap();
    let host = SimlockHost::new(SimlockOptions { config, state: temp.path().into(), node: "node".into(), agent_entry: "/entry".into(), cli_provider: None, commands: Arc::new(GatedOpen { commands: commands.clone(), gate: gate.clone() }), http: pool, secrets: Arc::new(LiteralSecrets), environment: BTreeMap::new() }).unwrap();
    host.ensure_ready(Arc::new(|_, _| {})).await.unwrap();
    let device = host.list().await.unwrap().devices[0].clone();
    let copy = host.clone();
    let opening = tokio::spawn(async move { copy.boot(&device, &OpenContext::default()).await });
    gate.entered.notified().await;
    host.stop().await.unwrap();
    assert!(opening.await.unwrap().is_err());
    let calls = commands.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args[1], "close");
    let mut entries = tokio::fs::read_dir(temp.path().join("device/simlock/pool")).await.unwrap();
    assert!(entries.next_entry().await.unwrap().is_none());
}
#[tokio::test]
async fn simlock_open_uses_shared_session_routes_only_lease_stream_and_returns_it() {
    let (temp, host, commands, _pool) = simlock_fixture(1, false).await;
    let device = host.list().await.unwrap().devices[1].clone();
    let workspace = temp.path().join("workspace");
    tokio::fs::create_dir(&workspace).await.unwrap();
    let id = host
        .boot(
            &device,
            &OpenContext {
                workspace_root: Some(workspace.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(id, "lease-1");
    let endpoint = host.agent_endpoint(&id).await.unwrap().unwrap();
    assert!(endpoint.agent_device_id.is_none());
    assert!(endpoint.session.is_some());
    assert_eq!(
        endpoint.install_directory,
        Some(tokio::fs::canonicalize(workspace).await.unwrap())
    );
    let stream = host
        .upstream("/vendor/serve-sim/helper/lease-1/stream.avcc", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stream.url,
        "https://simlock.test:4700/v1/leases/lease-1/stream"
    );
    let input = host
        .upstream(
            "/vendor/serve-sim/helper/ws",
            &[("device".into(), id.clone())],
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        input.url,
        "https://simlock.test:4700/v1/leases/lease-1/input"
    );
    assert!(
        host.upstream("/vendor/serve-sim/helper/lease-1/config", &[])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        host.upstream(
            "/vendor/serve-sim/helper/ws",
            &[("device".into(), "elsewhere".into())]
        )
        .await
        .unwrap()
        .is_none()
    );
    let mut running = device.clone();
    running.id = id.as_str().into();
    assert_eq!(host.screenshot(&running).await.unwrap(), [1, 2, 3]);
    assert!(host.boot(&device, &OpenContext::default()).await.is_err());
    host.shutdown(&id, DevicePlatform::Android).await.unwrap();
    host.stop().await.unwrap();
    let calls = commands.calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|c| c.args[1] == "open").count(), 1);
    assert_eq!(calls.iter().filter(|c| c.args[1] == "close").count(), 1);
    assert!(
        calls
            .iter()
            .all(|c| c.env["AGENT_DEVICE_DAEMON_AUTH_TOKEN"].is_none())
    );
}
#[tokio::test]
async fn simlock_failed_open_closes_session_and_scrubs_diagnostics() {
    let (_temp, host, commands, _pool) = simlock_fixture(1, true).await;
    let device = host.list().await.unwrap().devices[0].clone();
    let error = host
        .boot(&device, &OpenContext::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Invalid daemon response"));
    assert!(error.to_string().contains("404 Not Found"));
    assert!(!format!("{error:?}").contains("agent-token"));
    assert_eq!(
        commands
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.args[1] == "close")
            .count(),
        1
    );
    assert!(host.list().await.unwrap().devices.iter().all(|d| !d.booted));
    host.stop().await.unwrap();
}
#[tokio::test]
async fn simlock_empty_platform_catalog_is_not_cached() {
    let (_temp, host, _commands, pool) = simlock_fixture(2, false).await;
    *pool.catalog.lock().await = json!({"platforms":[{"platform":"ios","modelRuntimes":{}},{"platform":"android","modelRuntimes":{}}]});
    assert!(host.list().await.unwrap().devices.is_empty());
    *pool.catalog.lock().await = pool_catalog();
    assert_eq!(host.list().await.unwrap().devices.len(), 2);
    assert_eq!(pool.requests.lock().await.len(), 2);
    host.stop().await.unwrap();
}

#[tokio::test]
async fn simlock_renews_without_overriding_gateway_ttl_and_forgets_a_lost_lease() {
    let (_temp, host, commands, pool) = simlock_fixture(1, false).await;
    let device = host.list().await.unwrap().devices[0].clone();
    let id = host.boot(&device, &OpenContext::default()).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(120)).await;
    pool.renewed.notified().await;
    assert!(
        host.list()
            .await
            .unwrap()
            .devices
            .iter()
            .any(|d| d.id.as_ref() == id)
    );
    pool.renew_404.store(true, Ordering::SeqCst);
    tokio::time::advance(std::time::Duration::from_secs(120)).await;
    pool.renewed.notified().await;
    assert!(host.list().await.unwrap().devices.iter().all(|d| !d.booted));
    host.stop().await.unwrap();
    assert_eq!(
        commands
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.args[1] == "close")
            .count(),
        0
    );
}
#[tokio::test]
async fn simlock_lease_releases_at_its_maximum_age() {
    let (_temp, host, commands, pool) = simlock_fixture_with_minutes(1, false, 5).await;
    let device = host.list().await.unwrap().devices[0].clone();
    host.boot(&device, &OpenContext::default()).await.unwrap();
    tokio::time::pause();
    for _ in 0..2 {
        tokio::time::advance(std::time::Duration::from_secs(120)).await;
        pool.renewed.notified().await;
    }
    tokio::time::advance(std::time::Duration::from_secs(59)).await;
    assert_eq!(
        commands
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.args[1] == "close")
            .count(),
        0
    );
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    pool.closed.notified().await;
    assert!(host.list().await.unwrap().devices.iter().all(|d| !d.booted));
    host.stop().await.unwrap();
    assert_eq!(
        commands
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.args[1] == "close")
            .count(),
        1
    );
}
