#![allow(dead_code)]
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use t3_contracts::device::*;
use t3_device::{
    DeviceService, Error, Result,
    host::{
        self, DeviceHost, DeviceOperations, Listing, OpenContext, Phase, Ready, SessionRelease,
    },
    io::{Command, CommandOutput, Commands, Http, HttpRequest, HttpResponse},
    service::{DeviceSettings, MemorySettings},
};
use tokio::sync::{Mutex, Notify};

pub type Responder = Arc<dyn Fn(&Command) -> Result<CommandOutput> + Send + Sync>;
pub struct FakeCommands {
    pub calls: StdMutex<Vec<Command>>,
    pub respond: Responder,
}
impl FakeCommands {
    pub fn new(
        respond: impl Fn(&Command) -> Result<CommandOutput> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            calls: StdMutex::new(vec![]),
            respond: Arc::new(respond),
        })
    }
    pub fn encoder() -> Arc<Self> {
        Self::new(|_| {
            Ok(CommandOutput {
                stdout: " V....D libx264".into(),
                ..Default::default()
            })
        })
    }
}
#[async_trait]
impl Commands for FakeCommands {
    async fn run(&self, c: Command) -> Result<CommandOutput> {
        let result = (self.respond)(&c);
        self.calls.lock().unwrap().push(c);
        result
    }
}

pub struct FakeHttp {
    pub responses: Mutex<VecDeque<(u16, Vec<u8>)>>,
    pub requests: Mutex<Vec<HttpRequest>>,
}
impl FakeHttp {
    pub fn new(responses: impl IntoIterator<Item = (u16, Value)>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(
                responses
                    .into_iter()
                    .map(|(s, v)| (s, serde_json::to_vec(&v).unwrap()))
                    .collect(),
            ),
            requests: Mutex::new(vec![]),
        })
    }
}
#[async_trait]
impl Http for FakeHttp {
    async fn execute(&self, r: HttpRequest) -> Result<HttpResponse> {
        self.requests.lock().await.push(r);
        let (status, bytes) = self
            .responses
            .lock()
            .await
            .pop_front()
            .expect("unexpected HTTP request");
        Ok(HttpResponse::bytes(status, bytes))
    }
}

pub struct Gate {
    pub entered: Notify,
    pub release: Notify,
}
impl Gate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            release: Notify::new(),
        })
    }
    pub async fn wait(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}

pub struct FakeOps {
    pub listing: Mutex<Listing>,
    pub boots: AtomicUsize,
    pub attaches: AtomicUsize,
    pub shutdowns: AtomicUsize,
    pub fail_boot: AtomicBool,
    pub fail_shutdown: AtomicBool,
    pub boot_gate: StdMutex<Option<Arc<Gate>>>,
    pub list_gate: StdMutex<Option<Arc<Gate>>>,
    pub ends: bool,
    pub boot_id: String,
}
impl FakeOps {
    pub fn new(host: &str, platform: DevicePlatform, booted: bool) -> Arc<Self> {
        Arc::new(Self {
            listing: Mutex::new(Listing {
                devices: vec![device(host, "phone", platform, booted)],
                ..Default::default()
            }),
            boots: AtomicUsize::new(0),
            attaches: AtomicUsize::new(0),
            shutdowns: AtomicUsize::new(0),
            fail_boot: AtomicBool::new(false),
            fail_shutdown: AtomicBool::new(false),
            boot_gate: StdMutex::new(None),
            list_gate: StdMutex::new(None),
            ends: false,
            boot_id: "booted-phone".into(),
        })
    }
}
#[async_trait]
impl DeviceOperations for FakeOps {
    async fn list(&self) -> Result<Listing> {
        let before = self.listing.lock().await.clone();
        let gate = self.list_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.wait().await;
        }
        Ok(before)
    }
    async fn boot(&self, _: &DeviceSummary, _: &OpenContext) -> Result<String> {
        self.boots.fetch_add(1, Ordering::SeqCst);
        let gate = self.boot_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.wait().await;
        }
        if self.fail_boot.load(Ordering::SeqCst) {
            return Err(Error::Unavailable("boot failed".into()));
        }
        let mut listing = self.listing.lock().await;
        listing.devices[0].id = self.boot_id.as_str().into();
        listing.devices[0].booted = true;
        Ok(self.boot_id.clone())
    }
    async fn attach(&self, _: &DeviceSummary) -> Result<()> {
        self.attaches.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn shutdown(&self, id: &str, _: DevicePlatform) -> Result<()> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        if self.fail_shutdown.load(Ordering::SeqCst) {
            return Err(Error::Unavailable("shutdown failed".into()));
        }
        for device in &mut self.listing.lock().await.devices {
            if device.id.as_ref() == id {
                device.booted = false;
            }
        }
        Ok(())
    }
    async fn screenshot(&self, _: &DeviceSummary) -> Result<Vec<u8>> {
        Ok(vec![137, 80, 78, 71])
    }
    fn ends_devices(&self) -> bool {
        self.ends
    }
}
pub struct FakeRelease {
    pub calls: Mutex<Vec<String>>,
    pub fail: AtomicBool,
    pub gate: StdMutex<Option<Arc<Gate>>>,
}
impl FakeRelease {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(vec![]),
            fail: AtomicBool::new(false),
            gate: StdMutex::new(None),
        })
    }
}
#[async_trait]
impl SessionRelease for FakeRelease {
    async fn release(&self, name: &str, _: &str) -> Result<()> {
        self.calls.lock().await.push(name.into());
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.wait().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error::Unavailable("release failed".into()));
        }
        Ok(())
    }
}

pub struct FakeHost {
    pub id: String,
    pub ready: Ready,
    pub kind: DeviceHostKind,
    pub online: AtomicBool,
    pub fail: AtomicBool,
    pub starts: AtomicUsize,
    pub agents: AtomicUsize,
    pub stops: AtomicUsize,
    pub agent_stops: AtomicUsize,
    pub inspection_fail: AtomicBool,
    pub availability_calls: AtomicUsize,
    pub availability_changed: Notify,
    pub phase: StdMutex<Option<Phase>>,
}
impl FakeHost {
    pub fn new(
        id: &str,
        ops: Arc<dyn DeviceOperations>,
        commands: Arc<dyn Commands>,
        release: Option<Arc<dyn SessionRelease>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            ready: Ready {
                origin: "http://hub.test".into(),
                node_path: "node".into(),
                devices: ops,
                commands,
                ax_helper: Some("/hub/simax/serve-sim-ax-settings".into()),
                sim_cli: Some("/hub/serve-sim.js".into()),
                agent: None,
                routing: None,
                release,
            },
            kind: DeviceHostKind::Local,
            online: AtomicBool::new(true),
            fail: AtomicBool::new(false),
            starts: AtomicUsize::new(0),
            agents: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            agent_stops: AtomicUsize::new(0),
            inspection_fail: AtomicBool::new(false),
            availability_calls: AtomicUsize::new(0),
            availability_changed: Notify::new(),
            phase: StdMutex::new(None),
        })
    }
}
#[async_trait]
impl DeviceHost for FakeHost {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        &self.id
    }
    async fn summary(&self) -> DeviceHostSummary {
        let mut s = host::summary(
            &self.id,
            &self.id,
            self.kind.clone(),
            vec![
                host::available(DevicePlatform::Ios, None),
                host::available(DevicePlatform::Android, None),
            ],
        );
        s.hub_installed = true;
        s
    }
    async fn inspect(&self) -> Result<DeviceHostSummary> {
        if self.inspection_fail.load(Ordering::SeqCst) {
            Err(Error::Unavailable("offline".into()))
        } else {
            Ok(self.summary().await)
        }
    }
    async fn availability(&self, p: DevicePlatform) -> DevicePlatformAvailability {
        self.availability_calls.fetch_add(1, Ordering::SeqCst);
        self.availability_changed.notify_one();
        host::available(p, None)
    }
    async fn ensure_ready(&self, phase: Phase) -> Result<Ready> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        phase(DeviceHostStatus::Starting, None);
        *self.phase.lock().unwrap() = Some(phase);
        if self.fail.load(Ordering::SeqCst) {
            Err(Error::Unavailable("offline".into()))
        } else {
            Ok(self.ready.clone())
        }
    }
    async fn ensure_agent_ready(&self, phase: Phase) -> Result<Ready> {
        self.agents.fetch_add(1, Ordering::SeqCst);
        self.ensure_ready(phase).await
    }
    async fn current(&self) -> Option<Ready> {
        self.online
            .load(Ordering::SeqCst)
            .then(|| self.ready.clone())
    }
    async fn stop_agent(&self) -> Result<()> {
        self.agent_stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
pub fn device(host: &str, id: &str, platform: DevicePlatform, booted: bool) -> DeviceSummary {
    serde_json::from_value(json!({"hostId":host,"id":id,"platform":platform,"name":"Pixel_API_35","version":if platform==DevicePlatform::Ios{"iOS 26.3"}else{"Android 15"},"booted":booted,"physical":false})).unwrap()
}
pub fn settings(enabled: bool, agent: bool) -> Arc<MemorySettings> {
    Arc::new(MemorySettings(Mutex::new(DeviceSettings {
        enabled,
        agent_access_enabled: agent,
        ..Default::default()
    })))
}
pub async fn service(hosts: Vec<Arc<dyn DeviceHost>>, enabled: bool, agent: bool) -> DeviceService {
    DeviceService::new(hosts, settings(enabled, agent), None, None, None, None)
        .await
        .unwrap()
}
pub fn open(host: &str, id: &str, thread: &str, platform: DevicePlatform) -> DeviceOpenInput {
    serde_json::from_value(
        json!({"hostId":host,"deviceId":id,"threadId":thread,"platform":platform}),
    )
    .unwrap()
}
pub fn close(thread: &str, shutdown: bool) -> DeviceCloseInput {
    serde_json::from_value(json!({"threadId":thread,"shutdown":shutdown})).unwrap()
}

pub struct FakeLaunchers {
    pub bound: Mutex<Vec<(String, Vec<String>)>>,
    pub released: Mutex<Vec<String>>,
    pub bind_gate: StdMutex<Option<Arc<Gate>>>,
    pub fail_release: AtomicBool,
}
impl FakeLaunchers {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            bound: Mutex::new(vec![]),
            released: Mutex::new(vec![]),
            bind_gate: StdMutex::new(None),
            fail_release: AtomicBool::new(false),
        })
    }
}
#[async_trait]
impl t3_device::agent::Launchers for FakeLaunchers {
    async fn configure(&self, _: &str, _: Option<&str>, _: &host::AgentEndpoint) -> Result<String> {
        Ok("/host.json".into())
    }
    async fn bind(&self, session: &str, args: &[String]) -> Result<String> {
        let gate = self.bind_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.wait().await;
        }
        self.bound.lock().await.push((session.into(), args.into()));
        Ok(format!("/managed/{session}"))
    }
    async fn release(&self, launcher: &str) -> Result<()> {
        self.released.lock().await.push(launcher.into());
        if self.fail_release.load(Ordering::SeqCst) {
            return Err(Error::Unavailable("launcher release failed".into()));
        }
        Ok(())
    }
    async fn prune(&self, _: &BTreeSet<String>) -> Result<()> {
        Ok(())
    }
}
