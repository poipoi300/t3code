#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use common::*;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering},
    },
};
use t3_contracts::device::*;
use t3_device::{
    Error, Result,
    host::DeviceHost,
    io::{Command, CommandOutput, Http, HttpRequest, HttpResponse, ManagedChild, Spawner},
    local::{LocalDeviceHost, LocalOptions, Machine, Ports},
    ssh::{Dns, SshDeviceHost, SshOptions},
    tools::{Tool, Toolchain},
};
use tokio::sync::{Mutex, Notify};

struct ChildControl {
    exit: Notify,
    stops: AtomicUsize,
}
struct Child(Arc<ChildControl>);
#[async_trait]
impl ManagedChild for Child {
    async fn exited(&mut self) -> Result<()> {
        self.0.exit.notified().await;
        Ok(())
    }
    async fn stop(&mut self) -> Result<()> {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
struct FakeSpawner {
    calls: Mutex<Vec<Command>>,
    children: Mutex<Vec<Arc<ChildControl>>>,
    fail: AtomicBool,
    spawned: Notify,
}
impl FakeSpawner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(vec![]),
            children: Mutex::new(vec![]),
            fail: AtomicBool::new(false),
            spawned: Notify::new(),
        })
    }
}
#[async_trait]
impl Spawner for FakeSpawner {
    async fn spawn(&self, c: Command) -> Result<Box<dyn ManagedChild>> {
        self.calls.lock().await.push(c);
        self.spawned.notify_one();
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error::Unavailable("spawn failed".into()));
        }
        let control = Arc::new(ChildControl {
            exit: Notify::new(),
            stops: AtomicUsize::new(0),
        });
        self.children.lock().await.push(control.clone());
        Ok(Box::new(Child(control)))
    }
}
struct FakePorts(AtomicU16);
#[async_trait]
impl Ports for FakePorts {
    async fn reserve(&self) -> Result<u16> {
        Ok(self.0.fetch_add(1, Ordering::SeqCst))
    }
}
struct Mac;
#[async_trait]
impl Machine for Mac {
    fn platform(&self) -> &str {
        "darwin"
    }
    fn environment(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
    async fn exists(&self, p: &Path) -> bool {
        tokio::fs::try_exists(p).await.unwrap_or(false)
    }
    async fn real_path(&self, p: &Path) -> Option<PathBuf> {
        tokio::fs::canonicalize(p).await.ok()
    }
}
struct Health {
    unhealthy_port: AtomicU16,
}
#[async_trait]
impl Http for Health {
    async fn execute(&self, r: HttpRequest) -> Result<HttpResponse> {
        let status = if r.url.path() == "/health"
            && r.url.port() == Some(self.unhealthy_port.load(Ordering::SeqCst))
        {
            503
        } else {
            200
        };
        Ok(HttpResponse::bytes(
            status,
            if r.url.path() == "/rpc" {
                br#"{"result":{"ok":true}}"#.to_vec()
            } else {
                b"ok".to_vec()
            },
        ))
    }
}
async fn installed(base: &Path, tool: Tool) {
    let paths = tool.paths(base, tool.version());
    tokio::fs::create_dir_all(paths.entry.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(paths.entry, "").await.unwrap();
    tokio::fs::write(paths.sentinel, tool.version())
        .await
        .unwrap();
}
async fn local_fixture(
    startup_code: i32,
) -> (
    tempfile::TempDir,
    LocalDeviceHost,
    Arc<FakeCommands>,
    Arc<FakeSpawner>,
    Arc<Health>,
    Arc<AtomicUsize>,
) {
    let temp = tempfile::tempdir().unwrap();
    installed(temp.path(), Tool::Hub).await;
    installed(temp.path(), Tool::Agent).await;
    let state = temp.path().join("state");
    let daemon = state.join("device/agent-device/daemon.json");
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let commands = FakeCommands::new(move |c| {
        if c.args.get(1).is_some_and(|a| a == "devices") {
            let attempt = count.fetch_add(1, Ordering::SeqCst) + 1;
            if startup_code == 0 {
                std::fs::create_dir_all(daemon.parent().unwrap()).unwrap();
                std::fs::write(&daemon,json!({"httpPort":7000+attempt,"token":format!("token-{attempt}"),"version":Tool::Agent.version()}).to_string()).unwrap();
            }
            return Ok(CommandOutput {
                code: startup_code,
                stdout: if startup_code == 1 {
                    json!({"error":{"details":{"reason":"daemon_registration_unproven","inspection":{"reason":"non-directory"}}}}).to_string()
                } else {
                    "private credentials".into()
                },
                stderr: String::new(),
            });
        }
        Ok(CommandOutput::default())
    });
    let spawner = FakeSpawner::new();
    let http = Arc::new(Health {
        unhealthy_port: AtomicU16::new(0),
    });
    let tools = Arc::new(Toolchain::new(temp.path().into(), commands.clone()));
    let host = LocalDeviceHost::new(LocalOptions {
        state,
        cwd: temp.path().into(),
        node: "node".into(),
        tools,
        http: http.clone(),
        commands: commands.clone(),
        spawner: spawner.clone(),
        ports: Arc::new(FakePorts(AtomicU16::new(8100))),
        machine: Arc::new(Mac),
    });
    (temp, host, commands, spawner, http, attempts)
}
#[tokio::test]
async fn local_revalidates_cached_daemon_and_recovers_with_registered_endpoint() {
    let (_temp, host, _commands, spawner, http, attempts) = local_fixture(0).await;
    let first = host.ensure_agent_ready(Arc::new(|_, _| {})).await.unwrap();
    assert_eq!(
        first.agent.as_ref().unwrap().base_url,
        "http://127.0.0.1:7001"
    );
    http.unhealthy_port.store(7001, Ordering::SeqCst);
    let next = host.ensure_agent_ready(Arc::new(|_, _| {})).await.unwrap();
    assert_eq!(
        next.agent.as_ref().unwrap().base_url,
        "http://127.0.0.1:7002"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(spawner.calls.lock().await.len(), 1);
    assert!(host.current().await.unwrap().release.is_some());
    host.stop().await.unwrap();
    assert!(host.current().await.is_none());
    assert_eq!(
        spawner.children.lock().await[0]
            .stops
            .load(Ordering::SeqCst),
        1
    );
}
#[tokio::test]
async fn local_daemon_registration_failure_preserves_lock_and_exposes_safe_guidance() {
    let (temp, host, _commands, _spawner, _http, _attempts) = local_fixture(1).await;
    let lock = temp.path().join("state/device/agent-device/daemon.lock");
    tokio::fs::create_dir_all(lock.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&lock, "legacy-owner").await.unwrap();
    let error = host
        .ensure_agent_ready(Arc::new(|_, _| {}))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("daemon.lock is not a directory"));
    assert!(error.to_string().contains("original CLI"));
    assert!(!format!("{error:?}").contains("private credentials"));
    assert_eq!(
        tokio::fs::read_to_string(lock).await.unwrap(),
        "legacy-owner"
    );
    assert!(host.current().await.is_some());
    host.stop().await.unwrap();
}
#[tokio::test]
async fn local_unknown_startup_exit_is_immediate_and_redacts_output() {
    let (_temp, host, _commands, _spawner, _http, _attempts) = local_fixture(2).await;
    let error = host
        .ensure_agent_ready(Arc::new(|_, _| {}))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("exit code 2"));
    assert!(!format!("{error:?}").contains("private credentials"));
    host.stop().await.unwrap();
}
#[tokio::test]
async fn local_stop_uses_recorded_agent_version_if_installed() {
    let (temp, host, commands, _spawner, _http, _attempts) = local_fixture(0).await;
    let paths = Tool::Agent.paths(temp.path(), "0.21.30");
    tokio::fs::create_dir_all(paths.entry.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&paths.entry, "").await.unwrap();
    let daemon = temp.path().join("state/device/agent-device/daemon.json");
    tokio::fs::create_dir_all(daemon.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        daemon,
        json!({"httpPort":7001,"token":"old-token","version":"0.21.30"}).to_string(),
    )
    .await
    .unwrap();
    host.stop().await.unwrap();
    let calls = commands.calls.lock().unwrap();
    let stop = calls
        .iter()
        .find(|c| c.args.contains(&"daemon".into()))
        .unwrap();
    assert_eq!(Path::new(&stop.args[0]), paths.entry);
}
#[tokio::test]
async fn dead_hub_restart_failure_retains_agent_release_endpoint() {
    let (_temp, host, _commands, spawner, _http, _attempts) = local_fixture(0).await;
    host.ensure_agent_ready(Arc::new(|_, _| {})).await.unwrap();
    spawner.spawned.notified().await;
    spawner.fail.store(true, Ordering::SeqCst);
    spawner.children.lock().await[0].exit.notify_one();
    spawner.spawned.notified().await;
    assert!(host.ensure_ready(Arc::new(|_, _| {})).await.is_err());
    assert!(host.current().await.unwrap().release.is_some());
    host.stop().await.unwrap();
}

fn ssh_config() -> SshDeviceHostConfig {
    serde_json::from_value(json!({"kind":"ssh","id":"remote","label":"Remote","target":"test.example","identityFile":"C:/keys/key with spaces"})).unwrap()
}
fn ssh_commands() -> Arc<FakeCommands> {
    FakeCommands::new(|c| {
        if let Some(stdin) = &c.stdin {
            let source = String::from_utf8_lossy(stdin);
            if source.contains("const mode=\"probe\"") {
                return Ok(CommandOutput{stdout:json!({"nodePath":"/usr/bin/node","platforms":[{"platform":"ios","available":true},{"platform":"android","available":false}]}).to_string(),..Default::default()});
            }
            if source.contains("const mode=\"start\"")
                || source.contains("const mode=\"agent-start\"")
            {
                let agent = source.contains("const mode=\"agent-start\"");
                let mut value = json!({"nodePath":"/usr/bin/node","hubPort":9200,"helpers":{"serveSimAxSettings":null,"serveSimCli":null},"platforms":[{"platform":"ios","available":true}]});
                if agent {
                    value["daemonPort"] = json!(9201);
                    value["token"] = json!("remote-secret");
                    value["entryPath"] = json!("/agent.mjs");
                }
                return Ok(CommandOutput {
                    stdout: value.to_string(),
                    ..Default::default()
                });
            }
        }
        Ok(CommandOutput::default())
    })
}
#[tokio::test]
async fn ssh_forward_owns_stdin_disables_multiplexing_and_cleans_up() {
    let commands = ssh_commands();
    let spawner = FakeSpawner::new();
    let host = SshDeviceHost::new(SshOptions {
        config: ssh_config(),
        owner: t3_device::ssh::owner("env", "state", "remote"),
        commands: commands.clone(),
        http: Arc::new(Health {
            unhealthy_port: AtomicU16::new(0),
        }),
        spawner: spawner.clone(),
        ports: Arc::new(FakePorts(AtomicU16::new(8300))),
    })
    .unwrap();
    let ready = host.ensure_agent_ready(Arc::new(|_, _| {})).await.unwrap();
    assert!(ready.release.is_some());
    assert_eq!(ready.agent.unwrap().base_url, "http://127.0.0.1:8301");
    let calls = spawner.calls.lock().await;
    let args = &calls[0].args;
    for option in ["ControlMaster=no", "ControlPath=none", "ControlPersist=no"] {
        assert!(args.contains(&option.into()));
    }
    assert!(!args.contains(&"-N".into()));
    assert_eq!(
        &args[args.iter().position(|s| s == "test.example").unwrap() + 1..],
        ["sh", "-c", "'exec cat >/dev/null'"]
    );
    assert!(args.contains(&"127.0.0.1:8300:127.0.0.1:9200".into()));
    drop(calls);
    host.stop().await.unwrap();
    assert!(host.current().await.is_none());
    assert_eq!(
        spawner.children.lock().await[0]
            .stops
            .load(Ordering::SeqCst),
        1
    );
    assert!(commands.calls.lock().unwrap().iter().any(|c| {
        c.stdin
            .as_ref()
            .is_some_and(|b| String::from_utf8_lossy(b).contains("const mode=\"stop\""))
    }));
}
struct NoDns;
#[async_trait]
impl Dns for NoDns {
    async fn lookup(&self, _: &str) -> Result<Vec<IpAddr>> {
        Ok(vec![])
    }
}
#[tokio::test]
async fn ssh_self_detection_unmaps_ipv4_and_refuses_forwards_proxies_and_unresolved_names() {
    for (output, expected) in [
        ("hostname 127.0.1.1\nport 22\n", true),
        ("hostname ::1\nport 22\n", true),
        ("hostname ::ffff:127.0.0.1\nport 22\n", true),
        ("hostname 100.65.180.100\nport 22\n", true),
        ("hostname 127.0.0.1\nport 2222\n", false),
        ("hostname 127.0.0.1\nport 22\nproxyjump bastion\n", false),
        (
            "hostname 127.0.0.1\nport 22\nproxycommand nc remote 22\n",
            false,
        ),
        ("hostname example.invalid\nport 22\n", false),
    ] {
        let commands = FakeCommands::new(move |c| {
            assert!(c.args.contains(&"-G".into()));
            Ok(CommandOutput {
                stdout: output.into(),
                ..Default::default()
            })
        });
        let addresses = BTreeSet::from(["100.65.180.100".parse().unwrap()]);
        assert_eq!(
            t3_device::ssh::is_local(&ssh_config(), commands.as_ref(), &NoDns, &addresses).await,
            expected,
            "{output}"
        );
    }
}
#[tokio::test]
async fn ssh_probe_windows_failure_is_classified_without_private_stderr() {
    for (code, windows) in [(1, true), (9009, true), (255, false)] {
        let commands = FakeCommands::new(move |c| {
            if c.args.last().is_some_and(|a| a == "echo %OS% $env:OS") {
                return Ok(CommandOutput {
                    stdout: "Windows_NT".into(),
                    ..Default::default()
                });
            }
            Ok(CommandOutput {
                code,
                stderr: "private credentials".into(),
                ..Default::default()
            })
        });
        let options = SshOptions {
            config: ssh_config(),
            owner: "0123456789abcdef01234567".into(),
            commands: commands.clone(),
            http: Arc::new(Health {
                unhealthy_port: AtomicU16::new(0),
            }),
            spawner: FakeSpawner::new(),
            ports: Arc::new(FakePorts(AtomicU16::new(8400))),
        };
        let error = options.probe().await.unwrap_err();
        assert_eq!(error.to_string().contains("runs Windows"), windows);
        assert!(!format!("{error:?}").contains("private credentials"));
        if code == 255 {
            assert_eq!(commands.calls.lock().unwrap().len(), 1);
        }
    }
}
#[test]
fn ssh_bootstrap_and_targets_validate_before_spawning() {
    for target in ["-oProxyCommand=malicious", "invalid\nremote", ""] {
        let mut c = ssh_config();
        c.target = target.into();
        assert!(t3_device::ssh::base_args(&c).is_err());
    }
    assert!(t3_device::ssh::bootstrap_script("../path", "start").is_err());
    assert!(t3_device::ssh::bootstrap_script("0123456789abcdef01234567", "arbitrary").is_err());
    let text = "a b 'quoted' $(literal)\nnext";
    assert_eq!(
        t3_device::ssh::quote(text),
        "'a b '\"'\"'quoted'\"'\"' $(literal)\nnext'"
    );
}
