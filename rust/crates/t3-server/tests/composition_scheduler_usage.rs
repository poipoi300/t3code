//! Scheduled tasks and usage rates over a real socket (`server-wiring-scheduler-usage`).
//!
//! A scheduled task that fires is a queued `message.dispatch` through the orchestration
//! runtime's intake, the same path a client message takes: the Replay (dev) provider runs its
//! turn, the managed runtime-mode ceiling refuses it on a thread above the ceiling, and the
//! thread's own provider runs it rather than the task's stored selection. Due tasks are driven
//! by the scheduler's injected clock, never by sleeping. The price table is fetched over the
//! provider domain's HTTP transport, here a fake at the network edge.
#![cfg(all(
    feature = "local-services",
    feature = "orchestration",
    feature = "scheduler",
    feature = "usage",
    feature = "providers",
    feature = "project"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use t3_provider::{
    codex_installation::*,
    discovery::{Driver, VersionProbe},
    native_probe::*,
    native_provider::ExecutableProbe,
    sensitive::*,
    transport::*,
};
use t3_rpc::{CallEvent, CallKind, Client, Frame, RequestId};
use t3_server::{
    adapters::config::{ConfigInputs, ServerFlags},
    runtime::{RunningServer, StartOptions},
};
use tokio::sync::watch;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const T0: &str = "2026-11-14T10:00:00.000Z";
const LAUNCH_UNAVAILABLE: &str =
    "Unbound scheduled tasks require a launch environment, which is not assembled.";
const SCHEDULER_COMMANDS: [&str; 3] = [
    "scheduledTasks.upsert",
    "scheduledTasks.setEnabled",
    "scheduledTasks.runNow",
];

// ---- the scheduler's injected clock ---------------------------------------------------------

/// Time moves only when the test advances it; a sleeper wakes once its deadline has passed.
struct ManualClock(watch::Sender<DateTime<Utc>>);
impl ManualClock {
    fn at(iso: &str) -> Arc<Self> {
        Arc::new(Self(watch::channel(iso.parse().unwrap()).0))
    }
    fn advance(&self, by: Duration) {
        self.0
            .send_modify(|now| *now += TimeDelta::from_std(by).unwrap());
    }
}
#[async_trait]
impl t3_server::adapters::scheduler::Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.borrow()
    }
    async fn sleep(&self, duration: Duration) {
        let deadline = self.now() + TimeDelta::from_std(duration).unwrap();
        let mut now = self.0.subscribe();
        let _ = now.wait_for(|now| *now >= deadline).await;
    }
}

fn at(offset_seconds: i64) -> String {
    (T0.parse::<DateTime<Utc>>().unwrap() + TimeDelta::seconds(offset_seconds))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---- the provider domain's effects: no provider process, no network ------------------------

#[derive(Clone, Copy)]
enum Rates {
    /// The primary URL answers with a body over the 8 MiB bound; the fallback fails.
    Oversize,
    /// The primary URL answers with a one-model price table.
    Table,
}
struct Effects {
    rates: Rates,
    requests: Mutex<Vec<String>>,
}
impl Effects {
    fn new(rates: Rates) -> Arc<Self> {
        Arc::new(Self {
            rates,
            requests: Mutex::default(),
        })
    }
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
impl ProviderClock for Effects {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}
#[async_trait]
impl HttpTransport for Effects {
    async fn send(&self, request: HttpRequest) -> ServiceResult<HttpResponse> {
        self.requests.lock().unwrap().push(request.url);
        Err(ServiceError::Network)
    }
    async fn raw(&self, request: HttpRequest) -> ServiceResult<RawHttpResponse> {
        self.requests.lock().unwrap().push(request.url.clone());
        let body = match (self.rates, request.url.as_str()) {
            (_, url) if url != t3_usage::service::RATES_URL => {
                return Err(ServiceError::Network);
            }
            (Rates::Oversize, _) => vec![b' '; 8 * 1024 * 1024 + 1],
            (Rates::Table, _) => json!({"fixture-model":{"input_cost_per_token":0.000003,"output_cost_per_token":0.000015}})
                .to_string()
                .into_bytes(),
        };
        Ok(RawHttpResponse {
            status: 200,
            content_type: Some("application/json".into()),
            body: Sensitive::new(body),
        })
    }
}
#[async_trait]
impl NativeProbe for Effects {
    async fn codex(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        Err(ServiceError::Unavailable)
    }
    async fn claude(&self, _: ProbeLaunch) -> ServiceResult<Sensitive<Value>> {
        Err(ServiceError::Unavailable)
    }
}
#[async_trait]
impl ExecutableProbe for Effects {
    async fn version(
        &self,
        _: Driver,
        _: &str,
        _: &t3_provider::environment::Environment,
        _: &Path,
    ) -> ServiceResult<VersionProbe> {
        Err(ServiceError::Unavailable)
    }
}
#[async_trait]
impl ReleaseTransport for Effects {
    async fn latest(&self, _: &str) -> ServiceResult<Option<ReleaseAsset>> {
        Ok(None)
    }
    async fn download(
        &self,
        _: &ReleaseAsset,
        _: &Path,
        _: tokio::sync::watch::Sender<u64>,
    ) -> ServiceResult<()> {
        Err(ServiceError::Network)
    }
}
#[async_trait]
impl CodexExecutableValidator for Effects {
    async fn validate(
        &self,
        _: &Path,
        _: &str,
        _: &t3_provider::environment::Environment,
    ) -> ServiceResult<()> {
        Err(ServiceError::Response)
    }
}
#[async_trait]
impl t3_provider::registry::RegistryFetch for Effects {
    async fn bytes(&self, _: &str) -> ServiceResult<Vec<u8>> {
        Err(ServiceError::Network)
    }
}
#[async_trait]
impl t3_provider::registry_installation::RegistryCommands for Effects {
    fn resolve(&self, _: &str, _: &t3_provider::environment::Environment) -> Option<PathBuf> {
        None
    }
    async fn run(
        &self,
        _: &Path,
        _: &[String],
        _: &t3_provider::environment::Environment,
        _: Duration,
        _: usize,
    ) -> t3_provider::acp_service::AcpResult<Sensitive<String>> {
        panic!("no registry process runs in this test")
    }
}
#[async_trait]
impl t3_provider::registry_installation::RegistryDownloads for Effects {
    async fn download(&self, _: &str, _: &Path) -> t3_provider::acp_service::AcpResult<()> {
        panic!("no registry download runs in this test")
    }
}
#[async_trait]
impl t3_provider::maintenance_resolution::MaintenanceProbe for Effects {
    fn resolve(&self, _: &str, _: &t3_provider::environment::Environment) -> Option<String> {
        None
    }
    fn canonical(&self, path: &str) -> Option<String> {
        Some(path.into())
    }
    async fn run(
        &self,
        _: &str,
        _: &[String],
        _: &t3_provider::environment::Environment,
        _: &[i32],
    ) -> Option<String> {
        None
    }
}
#[async_trait]
impl t3_provider::service::MaintenanceExecutor for Effects {
    async fn execute(&self, _: t3_provider::maintenance::MaintenanceAction) -> ServiceResult<()> {
        Err(ServiceError::Unavailable)
    }
}
fn provider_effects(effects: Arc<Effects>) -> Arc<t3_server::adapters::providers::ProviderEffects> {
    Arc::new(t3_server::adapters::providers::ProviderEffects {
        http: effects.clone(),
        clock: effects.clone(),
        protocols: effects.clone(),
        executables: effects.clone(),
        codex: t3_provider::domain::CodexInstallEffects {
            transport: effects.clone(),
            validator: effects.clone(),
            local: Arc::new(NoLocalCodex),
        },
        maintenance_probe: Some(effects.clone()),
        maintenance_executor: Some(effects.clone()),
        registry_fetch: Some(effects.clone()),
        registry_commands: Some(effects.clone()),
        registry_downloads: Some(effects),
    })
}

// ---- server and sockets ---------------------------------------------------------------------

/// Dev mode (`--dev-url`) seeds Replay (dev). The managed policy path is always explicit, so no
/// policy of this machine is read.
async fn start(
    home: &Path,
    clock: Option<Arc<ManualClock>>,
    effects: Arc<Effects>,
    policy: &Path,
) -> RunningServer {
    std::fs::create_dir_all(home.join("userdata")).unwrap();
    let settings = home.join("userdata/settings.json");
    if !settings.exists() {
        std::fs::write(&settings, r#"{"enableProviderUpdateChecks":false}"#).unwrap();
    }
    let env: BTreeMap<String, String> = ["SystemRoot", "ComSpec", "WINDIR"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
        .chain([(
            "T3CODE_DEV_MANAGED_POLICY_PATH".to_owned(),
            policy.to_string_lossy().into_owned(),
        )])
        .collect();
    t3_server::runtime::start_with(
        ConfigInputs {
            flags: ServerFlags {
                base_dir: Some(home.to_string_lossy().into_owned()),
                host: Some("127.0.0.1".into()),
                port: Some(0),
                dev_url: Some("http://127.0.0.1:5733".into()),
                ..Default::default()
            },
            env,
            home: home.into(),
            process_cwd: home.into(),
            bootstrap: None,
            headless: true,
            force_auto_bootstrap: Some(false),
            service_managed: false,
            cli_log_level: None,
            automatic_port: None,
            persisted_observability: Default::default(),
            static_dir: None,
        },
        StartOptions {
            provider_effects: Some(provider_effects(effects)),
            scheduler_clock: clock
                .map(|clock| clock as Arc<dyn t3_server::adapters::scheduler::Clock>),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

async fn browser_session(origin: &str, credential: &Value) -> (String, Value) {
    let response = reqwest::Client::new()
        .post(format!("{origin}/api/auth/browser-session"))
        .json(&json!({ "credential": credential }))
        .send()
        .await
        .unwrap();
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    (cookie, response.json().await.unwrap())
}

/// One socket and the stream chunks it has received but no wait has consumed yet.
struct Session {
    socket: Socket,
    client: Client,
    chunks: Vec<(RequestId, Vec<Value>)>,
}
impl Session {
    async fn connect(server: &RunningServer, cookie: &str) -> Self {
        let ticket: Value = reqwest::Client::new()
            .post(format!(
                "http://{}/api/auth/websocket-ticket/rpc",
                server.address
            ))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let (socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{}/ws?orchestrationProtocol=2&wsTicket={}",
            server.address,
            ticket["ticket"].as_str().unwrap()
        ))
        .await
        .unwrap();
        Self {
            socket,
            client: Client::new(0, Default::default()),
            chunks: Vec::new(),
        }
    }
    /// The admin session and its permissions.
    async fn admin(server: &RunningServer) -> (Self, String, Vec<Value>) {
        let origin = format!("http://{}", server.address);
        let (cookie, body) = browser_session(&origin, &json!(server.pairing_credential)).await;
        let permissions = body["permissions"].as_array().unwrap().clone();
        (Self::connect(server, &cookie).await, cookie, permissions)
    }
    /// A session holding every admin scope except `withheld`.
    async fn without(
        server: &RunningServer,
        admin_cookie: &str,
        permissions: &[Value],
        withheld: &str,
    ) -> Self {
        let origin = format!("http://{}", server.address);
        let scopes: Vec<&Value> = permissions.iter().filter(|s| *s != withheld).collect();
        assert_eq!(scopes.len() + 1, permissions.len(), "{withheld} is held");
        let grant: Value = reqwest::Client::new()
            .post(format!("{origin}/api/auth/pairing-token"))
            .header("cookie", admin_cookie)
            .json(&json!({ "scopes": scopes }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let (cookie, _) = browser_session(&origin, &grant["credential"]).await;
        Self::connect(server, &cookie).await
    }
    async fn event(&mut self) -> CallEvent {
        loop {
            while let Some(message) = self.client.pop_outgoing() {
                let Frame::Text(text) = t3_rpc::codec::encode(&message).unwrap() else {
                    panic!("text frame expected");
                };
                self.socket.send(Message::Text(text.into())).await.unwrap();
            }
            if let Some(event) = self.client.pop_event() {
                return event;
            }
            let frame = match self.socket.next().await.unwrap().unwrap() {
                Message::Text(text) => Frame::Text(text.to_string()),
                Message::Binary(bytes) => Frame::Binary(bytes.to_vec()),
                Message::Ping(payload) => {
                    self.socket.send(Message::Pong(payload)).await.unwrap();
                    continue;
                }
                other => panic!("connection ended: {other:?}"),
            };
            for message in t3_rpc::codec::decode(&frame).unwrap() {
                self.client.receive(message, 0);
            }
        }
    }
    /// A unary call's exit, encoded; chunks of open streams are kept for their waits.
    async fn call(&mut self, tag: &str, payload: Value) -> Value {
        let id = self.client.start(tag, payload, CallKind::Unary).unwrap();
        loop {
            match self.event().await {
                CallEvent::Exit { id: done, exit } if done == id => {
                    return serde_json::to_value(exit).unwrap();
                }
                CallEvent::Chunk { id, values } => {
                    self.client.acknowledge(id.clone());
                    self.chunks.push((id, values));
                }
                CallEvent::Exit { exit, .. } => {
                    panic!("a stream ended: {}", serde_json::to_value(exit).unwrap())
                }
                _ => {}
            }
        }
    }
    fn stream(&mut self, tag: &str, payload: Value) -> RequestId {
        self.client.start(tag, payload, CallKind::Stream).unwrap()
    }
    /// The first chunk value of stream `id` that satisfies `done`, kept chunks first.
    async fn until(&mut self, id: &RequestId, done: impl Fn(&Value) -> bool) -> Value {
        let mut kept = std::mem::take(&mut self.chunks);
        if let Some(found) = kept
            .iter()
            .filter(|(chunk, _)| chunk == id)
            .flat_map(|(_, values)| values)
            .find(|value| done(value))
            .cloned()
        {
            kept.retain(|(chunk, _)| chunk != id);
            self.chunks = kept;
            return found;
        }
        kept.retain(|(chunk, _)| chunk != id);
        self.chunks = kept;
        loop {
            match self.event().await {
                CallEvent::Chunk { id: chunk, values } => {
                    self.client.acknowledge(chunk.clone());
                    if chunk == *id {
                        if let Some(found) = values.iter().find(|value| done(value)) {
                            return found.clone();
                        }
                    } else {
                        self.chunks.push((chunk, values));
                    }
                }
                CallEvent::Exit { exit, .. } => {
                    panic!("a stream ended: {}", serde_json::to_value(exit).unwrap())
                }
                _ => {}
            }
        }
    }
}

fn value(exit: &Value) -> &Value {
    assert_eq!(exit["_tag"], "Success", "{exit}");
    &exit["value"]
}
/// The typed error of a failed exit.
fn error(exit: &Value) -> &Value {
    assert_eq!(exit["_tag"], "Failure", "{exit}");
    &exit["cause"][0]["error"]
}
fn scheduled_task_error(exit: &Value) -> String {
    let error = error(exit);
    assert_eq!(error["_tag"], "ScheduledTaskError", "{exit}");
    error["message"].as_str().unwrap().to_owned()
}
fn refused_for(exit: &Value, scope: &str) {
    let error = error(exit);
    assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{exit}");
    assert_eq!(error["requiredPermission"], scope, "{exit}");
}

/// Any object in `value` that is a run for a message id starting with `message` and has `status`.
fn has_run(value: &Value, message: &str, status: &str) -> bool {
    match value {
        Value::Object(map) => {
            (map.get("userMessageId")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with(message))
                && map.get("status").and_then(Value::as_str) == Some(status))
                || map.values().any(|v| has_run(v, message, status))
        }
        Value::Array(items) => items.iter().any(|v| has_run(v, message, status)),
        _ => false,
    }
}
fn task<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from {list}"))
}

fn upsert(id: &str, project: &str, thread: Option<&str>, enabled: bool, schedule: Value) -> Value {
    let mut input = json!({
        "id": id, "title": id, "prompt": format!("Scheduled prompt for {id}"),
        "enabled": enabled, "schedule": schedule, "projectId": project,
        "workspaceStrategy": {"type":"root"},
        // Never used for a bound task: the thread's own provider runs it.
        "modelSelection": {"instanceId":"absent-instance","model":"absent-model"},
        "runtimeMode": "full-access", "interactionMode": "default",
    });
    if let Some(thread) = thread {
        input["threadId"] = json!(thread);
        input["threadMode"] = json!("continue");
    }
    input
}
const EVERY_MINUTE: fn() -> Value = || json!({"type":"interval","everyMs":60_000});

#[tokio::test]
async fn scheduled_tasks_run_through_the_runtime_intake_over_the_socket() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let no_policy = temp.path().join("no-policy.json");
    let clock = ManualClock::at(T0);
    let server = start(
        &home,
        Some(clock.clone()),
        Effects::new(Rates::Table),
        &no_policy,
    )
    .await;
    for method in SCHEDULER_COMMANDS {
        assert!(!server.unserved_methods.contains(&method), "{method}");
    }
    let (mut admin, admin_cookie, permissions) = Session::admin(&server).await;
    let scratch = admin.call("projects.ensureScratch", json!({})).await;
    let project = value(&scratch)["projectId"].as_str().unwrap().to_owned();
    const THREAD: &str = "thread:scheduled";
    let created = admin
        .call(
            "orchestration.dispatchCommand",
            json!({"type":"thread.create","commandId":"command:thread","threadId":THREAD,"projectId":project,"title":"Scheduled","modelSelection":{"instanceId":"devReplay","model":"multi-turn"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
        )
        .await;
    value(&created);

    // Typed failures with TypeScript's messages; nothing is saved.
    for (input, message) in [
        (
            upsert(
                "task:x",
                &project,
                Some("thread:absent"),
                true,
                EVERY_MINUTE(),
            ),
            "That thread no longer exists.",
        ),
        (
            upsert(
                "task:x",
                "project:absent",
                Some(THREAD),
                true,
                EVERY_MINUTE(),
            ),
            "That thread belongs to a different project.",
        ),
        (
            upsert("task:x", &project, None, true, EVERY_MINUTE()),
            LAUNCH_UNAVAILABLE,
        ),
    ] {
        let exit = admin.call("scheduledTasks.upsert", input).await;
        assert_eq!(scheduled_task_error(&exit), message);
    }

    // As in TypeScript, a disabled unbound draft saves, even for a project this server does not
    // know. It cannot be enabled, and running it records a failed run without launching.
    let draft = admin
        .call(
            "scheduledTasks.upsert",
            upsert("task:draft", "project:absent", None, false, EVERY_MINUTE()),
        )
        .await;
    let saved = &value(&draft)["task"];
    assert_eq!(saved["enabled"], false, "{draft}");
    assert!(saved.get("threadId").is_none_or(Value::is_null), "{draft}");
    let enable = admin
        .call(
            "scheduledTasks.setEnabled",
            json!({"id":"task:draft","enabled":true}),
        )
        .await;
    assert_eq!(scheduled_task_error(&enable), LAUNCH_UNAVAILABLE);
    let draft_run = admin
        .call("scheduledTasks.runNow", json!({"id":"task:draft"}))
        .await;
    let draft_task = &value(&draft_run)["task"];
    assert_eq!(draft_task["enabled"], false, "{draft_run}");
    assert_eq!(draft_task["lastRunStatus"], "failed", "{draft_run}");
    assert_eq!(
        draft_task["lastRunError"], LAUNCH_UNAVAILABLE,
        "{draft_run}"
    );
    assert!(
        draft_task.get("threadId").is_none_or(Value::is_null),
        "{draft_run}"
    );

    // Success: bound tasks queue in their thread.
    let interval = admin
        .call(
            "scheduledTasks.upsert",
            upsert(
                "task:interval",
                &project,
                Some(THREAD),
                true,
                EVERY_MINUTE(),
            ),
        )
        .await;
    assert_eq!(value(&interval)["task"]["nextRunAt"], at(60));
    for input in [
        upsert(
            "task:once",
            &project,
            Some(THREAD),
            true,
            json!({"type":"once","runAt":at(90)}),
        ),
        upsert("task:off", &project, Some(THREAD), false, EVERY_MINUTE()),
        upsert(
            "task:manual",
            &project,
            Some(THREAD),
            true,
            json!({"type":"interval","everyMs":86_400_000}),
        ),
    ] {
        value(&admin.call("scheduledTasks.upsert", input).await);
    }

    let thread = admin.stream("orchestration.subscribeThread", json!({"threadId": THREAD}));
    let tasks = admin.stream("scheduledTasks.subscribe", json!({}));
    admin.until(&tasks, |list| list["tasks"].is_array()).await;

    // runNow queues the task's message; Replay runs the turn on the thread's own provider.
    let ran = admin
        .call("scheduledTasks.runNow", json!({"id":"task:manual"}))
        .await;
    assert_eq!(value(&ran)["task"]["lastRunStatus"], "succeeded", "{ran}");
    assert_eq!(value(&ran)["task"]["runCount"], 1, "{ran}");
    admin
        .until(&thread, |chunk| {
            has_run(chunk, "scheduled-task-message:task:manual:", "completed")
        })
        .await;
    let missing = admin
        .call("scheduledTasks.runNow", json!({"id":"task:absent"}))
        .await;
    assert_eq!(scheduled_task_error(&missing), "Schedule task not found.");

    // setEnabled both ways; an unknown task is a typed failure.
    let on = admin
        .call(
            "scheduledTasks.setEnabled",
            json!({"id":"task:off","enabled":true}),
        )
        .await;
    assert_eq!(value(&on)["task"]["enabled"], true);
    let off = admin
        .call(
            "scheduledTasks.setEnabled",
            json!({"id":"task:off","enabled":false}),
        )
        .await;
    assert_eq!(value(&off)["task"]["nextRunAt"], Value::Null);
    let unknown = admin
        .call(
            "scheduledTasks.setEnabled",
            json!({"id":"task:absent","enabled":true}),
        )
        .await;
    assert_eq!(scheduled_task_error(&unknown), "Schedule task not found.");

    // The schedule fires when the scheduler's clock reaches the task's next run.
    clock.advance(Duration::from_secs(60));
    let fired = admin
        .until(&tasks, |list| {
            task(list, "task:interval")["runCount"] == 1
                && task(list, "task:interval")["lastRunStatus"] == "succeeded"
        })
        .await;
    assert_eq!(task(&fired, "task:interval")["nextRunAt"], at(120));
    admin
        .until(&thread, |chunk| {
            has_run(chunk, "scheduled-task-message:task:interval:", "completed")
        })
        .await;

    // A session without orchestration:operate cannot create, toggle or run tasks.
    let mut reader = Session::without(
        &server,
        &admin_cookie,
        &permissions,
        "orchestration:operate",
    )
    .await;
    for (tag, payload) in [
        (
            "scheduledTasks.upsert",
            upsert("task:refused", &project, Some(THREAD), true, EVERY_MINUTE()),
        ),
        (
            "scheduledTasks.setEnabled",
            json!({"id":"task:off","enabled":true}),
        ),
        ("scheduledTasks.runNow", json!({"id":"task:manual"})),
    ] {
        refused_for(&reader.call(tag, payload).await, "orchestration:operate");
    }
    let reader_tasks = reader.stream("scheduledTasks.subscribe", json!({}));
    let read = reader
        .until(&reader_tasks, |list| list["tasks"].is_array())
        .await;
    assert!(
        task(&read, "task:off")["enabled"] == false && task(&read, "task:manual")["runCount"] == 1,
        "{read}"
    );

    // Shutdown joins with `task:once` still pending.
    drop((admin, reader));
    server.shutdown().await.unwrap();

    // A stale due time on a disabled task (bound, or an unbound draft) must not make it fire.
    let layout = t3_config::home::HomeLayout {
        base_dir: home.clone(),
        state_dir: home.join("userdata"),
    };
    let stale = t3_persistence::rusqlite::Connection::open(layout.database())
        .unwrap()
        .execute(
            "UPDATE scheduled_tasks SET next_run_at = ?1
             WHERE task_id IN ('task:off', 'task:draft') AND enabled = 0",
            [T0],
        )
        .unwrap();
    assert_eq!(stale, 2);

    // Restart at T0+90 under a ceiling below the thread's full access. Only `task:once` is due;
    // the dispatch ceiling refuses its turn as it refuses a client's, with TypeScript's text.
    let policy = temp.path().join("policy.json");
    std::fs::write(
        &policy,
        r#"{"schemaVersion":1,"organization":"Acme","limits":{"maxRuntimeMode":"auto-accept-edits"}}"#,
    )
    .unwrap();
    let clock = ManualClock::at(&at(90));
    let server = start(&home, Some(clock), Effects::new(Rates::Table), &policy).await;
    let (mut admin, _, _) = Session::admin(&server).await;
    let tasks = admin.stream("scheduledTasks.subscribe", json!({}));
    let after = admin
        .until(&tasks, |list| {
            task(list, "task:once")["lastRunStatus"] == "failed"
        })
        .await;
    let once = task(&after, "task:once");
    assert!(
        once["lastRunError"]
            .as_str()
            .is_some_and(|error| error.contains("Acme limits threads to Auto-accept edits.")),
        "{after}"
    );
    assert_eq!(once["runCount"], 1, "{after}");
    // In the same pass, ordered by due time: the stale disabled task and the task that already
    // fired at T0+60 (next due T0+120) did not run.
    let interval = task(&after, "task:interval");
    assert_eq!(interval["runCount"], 1, "{after}");
    assert_eq!(interval["nextRunAt"], at(120), "{after}");
    let off = task(&after, "task:off");
    assert_eq!(off["runCount"], 0, "{after}");
    assert_eq!(off["lastRunStatus"], "never", "{after}");
    let draft = task(&after, "task:draft");
    assert_eq!(draft["runCount"], 1, "{after}");
    assert_eq!(draft["lastRunError"], LAUNCH_UNAVAILABLE, "{after}");
    drop(admin);
    server.shutdown().await.unwrap();
}

/// TypeScript `withVisibleWebhookUrls`: a webhook URL starts agents, so the task list streams it
/// only to sessions holding orchestration:operate; others still read the task.
#[tokio::test]
async fn scheduled_task_webhook_urls_reach_only_operate_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let server = start(
        &temp.path().join("home"),
        Some(ManualClock::at(T0)),
        Effects::new(Rates::Table),
        &temp.path().join("no-policy.json"),
    )
    .await;
    let (mut admin, cookie, permissions) = Session::admin(&server).await;
    let scratch = admin.call("projects.ensureScratch", json!({})).await;
    let project = value(&scratch)["projectId"].as_str().unwrap().to_owned();
    let created = admin
        .call(
            "orchestration.dispatchCommand",
            json!({"type":"thread.create","commandId":"command:thread","threadId":"thread:hook","projectId":project,"title":"Hook","modelSelection":{"instanceId":"devReplay","model":"multi-turn"},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null,"createdBy":"user","creationSource":"web"}),
        )
        .await;
    value(&created);
    let hook = admin
        .call(
            "scheduledTasks.upsert",
            upsert(
                "task:hook",
                &project,
                Some("thread:hook"),
                true,
                json!({"type":"webhook"}),
            ),
        )
        .await;
    let path = value(&hook)["task"]["webhook"]["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let tasks = admin.stream("scheduledTasks.subscribe", json!({}));
    let listed = admin.until(&tasks, |list| list["tasks"].is_array()).await;
    assert_eq!(task(&listed, "task:hook")["webhook"]["path"], path.as_str());

    let mut reader =
        Session::without(&server, &cookie, &permissions, "orchestration:operate").await;
    let reader_tasks = reader.stream("scheduledTasks.subscribe", json!({}));
    let read = reader
        .until(&reader_tasks, |list| list["tasks"].is_array())
        .await;
    let seen = task(&read, "task:hook");
    assert_eq!(seen["title"], "task:hook", "{read}");
    assert!(seen.get("webhook").is_none(), "{read}");
    assert!(!read.to_string().contains(&path), "{read}");
    // A later change reaches the reader without the URL too.
    value(
        &admin
            .call(
                "scheduledTasks.setEnabled",
                json!({"id":"task:hook","enabled":false}),
            )
            .await,
    );
    let changed = reader
        .until(&reader_tasks, |list| {
            task(list, "task:hook")["enabled"] == false
        })
        .await;
    assert!(!changed.to_string().contains(&path), "{changed}");
    drop((admin, reader));
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn usage_rates_refresh_over_the_shared_transport_within_its_bounds() {
    let temp = tempfile::tempdir().unwrap();
    let no_policy = temp.path().join("no-policy.json");

    // A body over the 8 MiB bound and a failing fallback: TypeScript's answer is still a
    // pricing result, with no rates.
    let effects = Effects::new(Rates::Oversize);
    let server = start(&temp.path().join("a"), None, effects.clone(), &no_policy).await;
    assert!(
        !server
            .unserved_methods
            .contains(&"server.refreshUsageRates")
    );
    let (mut admin, cookie, permissions) = Session::admin(&server).await;
    let unavailable = admin.call("server.refreshUsageRates", json!({})).await;
    assert_eq!(
        value(&unavailable)["status"],
        "unavailable",
        "{unavailable}"
    );
    assert_eq!(value(&unavailable)["knownModels"], 0, "{unavailable}");
    assert_eq!(
        effects.requests(),
        [
            t3_usage::service::RATES_URL,
            t3_usage::service::RATES_API_URL
        ]
    );
    // The scope the contract assigns the method.
    let scope =
        t3_auth::authorization::required_scope_for_rpc_method("server.refreshUsageRates").unwrap();
    let scope = serde_json::to_value(scope).unwrap();
    let mut refused =
        Session::without(&server, &cookie, &permissions, scope.as_str().unwrap()).await;
    refused_for(
        &refused.call("server.refreshUsageRates", json!({})).await,
        scope.as_str().unwrap(),
    );
    assert_eq!(
        effects.requests().len(),
        2,
        "a refused call fetches nothing"
    );
    drop((admin, refused));
    server.shutdown().await.unwrap();

    // The fixed price URL answers: fresh rates; a second refresh inside the floor fetches nothing.
    let effects = Effects::new(Rates::Table);
    let server = start(&temp.path().join("b"), None, effects.clone(), &no_policy).await;
    let (mut admin, _, _) = Session::admin(&server).await;
    let fresh = admin.call("server.refreshUsageRates", json!({})).await;
    assert_eq!(value(&fresh)["status"], "fresh", "{fresh}");
    assert_eq!(value(&fresh)["knownModels"], 1, "{fresh}");
    let again = admin.call("server.refreshUsageRates", json!({})).await;
    assert_eq!(value(&again)["status"], "fresh", "{again}");
    assert_eq!(effects.requests(), [t3_usage::service::RATES_URL]);
    // Still unserved: their global usage-source branch has no binding yet.
    for method in ["server.refreshProviders", "provider.consumeResetCredit"] {
        assert!(server.unserved_methods.contains(&method), "{method}");
    }
    drop(admin);
    server.shutdown().await.unwrap();
}
