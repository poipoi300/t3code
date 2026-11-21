//! Providers settings through the settings route, with a paired client and a wire fixture.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{
        Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

use futures::{
    FutureExt, StreamExt,
    channel::mpsc,
    future::{Either, LocalBoxFuture, select},
    stream::LocalBoxStream,
};
use nsh_core::{Checked, prelude::*};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::{Value, json};
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    platform::{
        HostAction, HostFuture, HostReply, HttpRequest, HttpResponse, Platform, PlatformError,
        PlatformEvent, SocketRequest, Storage, StorageArea, StorageKey, StorageTransaction,
        StorageWrite, WebSocket,
    },
    services::providers::{TestCallKind, TestFrame},
    testing::environment::{EnvironmentPlatform, FakeEnvironment},
    types::EnvironmentId,
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    screens::settings::shell,
    state::nav::{Navigator, Route, SettingsPage},
    theme,
};

type WireLog = BTreeMap<&'static str, Vec<serde_json::Value>>;

static WIRE_LOGS: Mutex<BTreeMap<u32, WireLog>> = Mutex::new(BTreeMap::new());

struct Probe(u32);

impl Probe {
    fn new() -> Self {
        static NEXT_ID: AtomicU32 = AtomicU32::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        WIRE_LOGS.lock().expect("wire logs").remove(&self.0);
    }
}

#[derive(Clone, PartialEq, Props)]
struct ProvidersFixture {
    two_environments: bool,
    probe: u32,
    scenario: String,
}

#[component]
fn Providers(props: ProvidersFixture) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::Providers)));
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = if props.scenario == "managed" {
        FakeEnvironment::at_origin("provider-machine", "http://localhost:3499", spawner.clone())
    } else {
        FakeEnvironment::new("provider-machine", spawner.clone())
    };
    server.set_permissions(&[
        "orchestration:read",
        "orchestration:operate",
        "providers:manage",
        "settings:write",
        "terminal:read",
        "terminal:operate",
    ]);
    server.edit_config(|config| {
        config["environment"]["platform"]["os"] = json!("linux");
        config["providers"] = json!([{
            "instanceId": "codex",
            "driver": "codex",
            "displayName": "Fixture Codex",
            "enabled": true,
            "installed": true,
            "version": "1.0.0",
            "status": "ready",
            "auth": {"status": "authenticated"},
            "checkedAt": "2026-11-21T00:00:00.000Z",
            "setup": {"canAuthenticate": true, "canInstall": false},
            "models": [{"slug": "fixture-model", "name": "Fixture Model", "capabilities": null, "isCustom": false}],
            "slashCommands": [],
            "skills": []
        }, {
            "instanceId": "claudeAgent",
            "driver": "claudeAgent",
            "displayName": "Fixture Claude",
            "enabled": true,
            "installed": false,
            "version": null,
            "status": "warning",
            "auth": {"status": "unknown"},
            "checkedAt": "2026-11-21T00:00:00.000Z",
            "models": [],
            "slashCommands": [],
            "skills": []
        }]);
        config["settings"]["providerInstances"]["codex"] = json!({
            "driver": "codex", "enabled": true, "displayName": "Fixture Codex"
        });
        if props.scenario == "model-prefs" {
            config["providers"][0]["models"] = json!([
                {"slug":"fixture-model","name":"Fixture Model","capabilities":null,"isCustom":false},
                {"slug":"other-model","name":"Other Model","capabilities":null,"isCustom":false}
            ]);
        }
        if props.scenario == "policy" {
            config["settings"]["providerInstances"]["codex"]["enabled"] = json!(false);
            config["managedPolicy"] = json!({
                "organization":"Fixture policy","lockedSettings":[],
                "limits":{"providers":{"allowedDrivers":["claudeAgent"],"lockedInstanceFields":{"codex":["displayName"]}}}
            });
        }
        if props.scenario == "project-overrides" {
            config["environment"]["capabilities"]["projectProviderInstanceOverrides"] = json!(true);
            config["settings"]["projectSettingsOverrides"]["project"] =
                json!({"providerInstances":{"codex":{"enabled":false}}});
        }
        if props.scenario == "acp" {
            config["providers"][0]["instanceId"] = json!("registry-fixture");
            config["providers"][0]["driver"] = json!("acpRegistry");
            config["providers"][0]["displayName"] = json!("Registry Fixture");
            config["providers"][0]["nativeSessions"] = json!({"canList":true,"canLoad":true,"canResume":false,"canDelete":true});
            config["settings"]["providerInstances"] = json!({
                "registry-fixture":{"driver":"acpRegistry","enabled":true,"config":{"agentId":"registry-agent","source":"managed"}}
            });
        }
        if !props.scenario.is_empty() {
            config["providers"][0]["auth"]["status"] = json!("unknown");
        }
        if props.scenario == "managed" || props.scenario == "remote-managed" {
            config["settings"]["providerInstances"]["codex"]["config"] = json!({"setupMode":"managed"});
        }
        if props.scenario == "install" || props.scenario == "managed" {
            config["providers"][0]["setup"]["canInstall"] = json!(true);
            if props.scenario == "install" {
                config["settings"]["providerInstances"]["codex"]["enabled"] = json!(false);
            }
        }
    });
    let saved = serde_json::to_value(t3_contracts::settings::ServerSettings::default())
        .expect("settings reply");
    for _ in 0..4 {
        server.reply("server.updateSettings", Ok(saved.clone()));
    }
    server.register(
        "server.uninstallAcpRegistryManagedBinary",
        TestCallKind::Unary,
    );
    server.reply(
        "server.uninstallAcpRegistryManagedBinary",
        Ok(json!({"agentId":"registry-agent","removed":true})),
    );
    server.register("server.refreshProviders", TestCallKind::Unary);
    server.reply(
        "server.refreshProviders",
        Err(json!({"_tag":"ServerProviderUpdateError","detail":"Fixture refresh failed"})),
    );
    if props.scenario == "acp" {
        for (method, result) in [
            (
                "server.listAcpRegistrySessions",
                json!({
                    "sessions":[{
                        "sessionId":"native-session","cwd":"/workspace/project","additionalDirectories":[],
                        "title":"Native fixture","updatedAt":null,"importedThreadId":null
                    }],
                    "nextCursor":null,"canLoad":true,"canResume":false,"canDelete":true
                }),
            ),
            (
                "server.importAcpRegistrySession",
                json!({"threadId":"imported-thread","imported":true}),
            ),
            ("server.deleteAcpRegistrySession", json!({"deleted":true})),
        ] {
            server.register(method, TestCallKind::Unary);
            server.reply(method, Ok(result.clone()));
            server.reply(method, Ok(result));
        }
    }
    let second = props.two_environments.then(|| {
        FakeEnvironment::at_origin(
            "second-provider-machine",
            "https://second.test",
            spawner.clone(),
        )
    });
    let mut platform = server.platform();
    if let Some(second) = &second {
        second.edit_config(|config| config["environment"]["label"] = json!("Second machine"));
        platform = platform.with_environment(second);
    }
    let script_log = Rc::new(RefCell::new(WireLog::new()));
    let mut streams = BTreeMap::new();
    let initial = match props.scenario.as_str() {
        "terminal" => auth(
            "waiting",
            json!({"type":"terminal","id":"auth-interaction","output":"Private prompt"}),
        ),
        "draft-credentials" => auth(
            "waiting",
            json!({
                "type":"credentials","id":"auth-interaction",
                "fields":[{"name":"apiKey","label":"API key","secret":true}]
            }),
        ),
        "fallback" => {
            let mut waiting = auth("waiting", Value::Null);
            waiting["authorizationUrl"] = json!("https://signin.test/fallback");
            waiting
        }
        "signed-in" => auth("succeeded", Value::Null),
        "invalid-url" => auth(
            "waiting",
            json!({
                "type":"browser","id":"auth-interaction","url":"https://[invalid","requiresConsent":true
            }),
        ),
        _ => auth("idle", Value::Null),
    };
    streams.insert("provider.auth.subscribe", initial);
    if matches!(props.scenario.as_str(), "install" | "managed") {
        let mut runtime = install("idle");
        if props.scenario == "managed" {
            runtime["installedVersion"] = runtime["version"].clone();
        }
        streams.insert("provider.install.subscribe", runtime);
    }
    for (method, response) in [
        (
            "provider.auth.start",
            Ok(auth(
                "waiting",
                json!({"type":"browser","id":"auth-interaction","url":"https://signin.test/consent","requiresConsent":true}),
            )),
        ),
        ("provider.auth.cancel", Ok(auth("cancelled", Value::Null))),
        ("provider.auth.logout", Ok(auth("idle", Value::Null))),
        (
            "provider.auth.respond",
            if props.scenario == "consent-denied" {
                Err(
                    json!({"_tag":"ProviderSetupError","instanceId":"codex","operation":"respond","detail":"Consent refused"}),
                )
            } else {
                Ok(auth("waiting", Value::Null))
            },
        ),
        ("provider.install.start", Ok(install("downloading"))),
        ("provider.install.cancel", Ok(install("cancelled"))),
        ("provider.install.remove", Ok(install("succeeded"))),
    ] {
        server.register(method, TestCallKind::Unary);
        server.reply(method, response);
    }
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        ScriptPlatform {
            base: platform,
            probe: props.probe,
            streams: Rc::new(streams),
            log: script_log.clone(),
        },
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    provide_context(Rt::new(
        store.clone(),
        Rc::new(client.clone()),
        Choice::Client,
    ));
    provide_context(t3_ui::runtime::services::links::link_opener(&client));
    let rename_provider = props.scenario == "draft-credentials";
    on_key("ctrl+shift+g", move |_| {
        let id = EnvironmentId("provider-machine".into());
        let snapshot = store.snapshot();
        let environment = &snapshot.environments[&id];
        let mut config = environment
            .config
            .as_ref()
            .expect("server config")
            .as_ref()
            .clone();
        config.providers.0[0].checked_at =
            serde_json::from_value(json!("2026-11-21T01:00:00.000Z")).expect("fixture timestamp");
        if rename_provider {
            config
                .settings
                .provider_instances
                .get_mut(&t3_contracts::provider_instance::ProviderInstanceId(
                    "codex".into(),
                ))
                .expect("fixture Codex configuration")
                .display_name = Some(Some("Renamed Codex".into()));
        }
        assert!(store.set_config(&id, environment.session, config));
    });
    let pairing = client.pair(server.pairing());
    let connecting = client.clone();
    let legacy = props.scenario == "model-prefs";
    spawner(
        async move {
            if legacy {
                connecting.settings().update_client(serde_json::from_value(json!({
                    "favorites":[{"provider":"codex","model":"legacy-favorite"}],
                    "providerModelPreferences":{"codex":{"hiddenModels":["legacy-hidden"],"modelOrder":["other-model","fixture-model"]}}
                })).expect("legacy model preference patch")).await.expect("save fixture preferences");
            }
            pairing.await.expect("pair the fixture");
            if let Some(second) = second {
                connecting
                    .pair(second.pairing())
                    .await
                    .expect("pair the second fixture");
            }
        }
        .boxed_local(),
    );
    let probe = props.probe;
    on_key("ctrl+shift+u", move |_| {
        let mut log = script_log.borrow().clone();
        log.extend(
            [
                "server.updateSettings",
                "terminal.open",
                "terminal.write",
                "provider.auth.start",
                "provider.auth.respond",
                "provider.auth.cancel",
                "provider.auth.logout",
                "provider.install.start",
                "provider.install.cancel",
                "provider.install.remove",
                "server.listAcpRegistrySessions",
                "server.importAcpRegistrySession",
                "server.deleteAcpRegistrySession",
                "server.uninstallAcpRegistryManagedBinary",
                "server.refreshProviders",
            ]
            .into_iter()
            .map(|method| (method, server.calls(method))),
        );
        WIRE_LOGS.lock().expect("wire logs").insert(probe, log);
    });
    t3_ui::ui::toast::toast_provider(
        view()
            .flex_1()
            .child(untracked(|| {
                shell::screen_for(SettingsPage::Providers, &client)
            }))
            .child(t3_ui::ui::confirm::confirm_dialog_host()),
        t3_ui::ui::toast::Position::TopRight,
    )
}

fn start(width: f32, idiom: nsh_core::platform::Idiom) -> (Headless, Probe) {
    start_scenario(width, idiom, "")
}

fn start_scenario(
    width: f32,
    idiom: nsh_core::platform::Idiom,
    scenario: &str,
) -> (Headless, Probe) {
    let probe = Probe::new();
    let root_probe = probe.0;
    let scenario = scenario.to_owned();
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(width), px(1400.0)),
        ..EngineConfig::new(Root::new::<Providers>(move || ProvidersFixture {
            two_environments: scenario != "managed",
            probe: root_probe,
            scenario: scenario.clone(),
        }))
    };
    config.environment.idiom = idiom;
    (Headless::start(config).expect("providers harness"), probe)
}

fn desktop() -> (Headless, Probe) {
    start(1280.0, nsh_core::platform::Idiom::Desktop)
}

fn phone() -> (Headless, Probe) {
    start(390.0, nsh_core::platform::Idiom::Phone)
}

fn calls(app: &mut Headless, probe: &Probe, method: &str) -> Vec<serde_json::Value> {
    app.press_key("ctrl+shift+u")
        .expect("read fixture requests");
    let captured = WIRE_LOGS
        .lock()
        .expect("wire logs")
        .get(&probe.0)
        .and_then(|log| log.get(method))
        .cloned();
    captured.expect("captured fixture requests")
}

fn list_and_default((mut app, probe): (Headless, Probe)) {
    app.wait_for("role:button[Select Fixture Codex]")
        .expect("live provider list");
    app.wait_for("Authenticated").expect("provider status");
    app.press("role:combobox[Default model]")
        .expect("open default model");
    app.press("role:option[Fixture Model]")
        .expect("select advertised model");
    app.settle();
    let updates = calls(&mut app, &probe, "server.updateSettings");
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert_eq!(
        updates[0]["patch"]["defaultModelSelection"],
        json!({"instanceId":"codex","model":"fixture-model"})
    );
    app.press("role:switch[Enable Fixture Codex]")
        .expect("disable provider");
    app.settle();
    let updates = calls(&mut app, &probe, "server.updateSettings");
    assert_eq!(updates.len(), 2, "{updates:?}");
    assert_eq!(
        updates[1]["providerInstanceMutation"],
        json!({
            "operation":"upsert", "instanceId":"codex",
            "instance":{"driver":"codex", "displayName":"Fixture Codex", "enabled":false}
        })
    );
}

fn account((mut app, probe): (Headless, Probe)) {
    app.wait_for("role:button[Select Fixture Codex]")
        .expect("live provider list");
    app.wait_for("Setup").expect("provider setup section");
    app.wait_for("Sign out")
        .expect("authenticated provider account");
    app.press("role:button[Select Fixture Claude]")
        .expect("select the uninstalled provider");
    app.press("role:button[Install]")
        .expect("open the install terminal");
    app.wait_for("Review the command, then press Enter to run it.")
        .expect("command prefilled");
    let writes = calls(&mut app, &probe, "terminal.write");
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert_eq!(
        writes[0]["data"],
        "curl -fsSL https://claude.ai/install.sh | bash"
    );
    let opens = calls(&mut app, &probe, "terminal.open");
    assert_eq!(opens.len(), 1, "{opens:?}");
    assert_eq!(opens[0]["providerInstanceId"], "claudeAgent");
}

fn models_and_usage((mut app, probe): (Headless, Probe)) {
    app.wait_for("role:button[Select Fixture Codex]")
        .expect("live provider list");
    app.wait_for("Models").expect("provider models section");
    app.wait_for("Fixture Model").expect("advertised model");
    app.wait_for("Usage providers")
        .expect("environment usage sources");
    app.wait_for("Load balancing")
        .expect("two connected machines");
    app.press("role:button[Add Fixture Model to favorites]")
        .expect("favorite the model");
    app.settle();
    let updates = calls(&mut app, &probe, "server.updateSettings");
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert_eq!(
        updates[0]["patch"]["setModelFavorites"],
        json!([
            {"provider":"codex","model":"fixture-model","favorite":true}
        ])
    );
    app.press("role:button[Add hub]").expect("open hub setup");
    app.type_text("label:Hub URL", "https://fixture-hub.test:8318")
        .expect("enter hub URL");
    app.type_text("label:Management key", "fixture-key")
        .expect("enter a fixture key");
    app.press("role:dialog[Add a CLIProxyAPI hub] >> role:button[Add hub]")
        .expect("save the hub");
    app.settle();
    let updates = calls(&mut app, &probe, "server.updateSettings");
    assert_eq!(updates.len(), 2, "{updates:?}");
    assert_eq!(
        updates[1]["patch"]["usageLimitSources"]["cliproxy-fixture-hub.test-8318"],
        json!({
            "kind":"cliproxy", "url":"https://fixture-hub.test:8318", "managementKey":"fixture-key", "enabled":true
        })
    );
    let toggle = "role:switch[Automatically balance load]";
    let before = app
        .screen()
        .accessibility_state(toggle)
        .expect("balancing switch")
        .checked;
    app.press(toggle)
        .expect("change the device balancing preference");
    app.wait_until(
        "the saved balancing preference is reflected by the switch",
        nsh_testing::WAIT,
        |screen| {
            screen
                .accessibility_state(toggle)
                .is_ok_and(|state| state.checked.is_some() && state.checked != before)
        },
    )
    .expect("balancing preference persisted");
}

#[test]
fn desktop_provider_list_and_default() {
    list_and_default(desktop());
}
#[test]
fn phone_provider_list_and_default() {
    list_and_default(phone());
}
#[test]
fn desktop_provider_account() {
    account(desktop());
}
#[test]
fn phone_provider_account() {
    account(phone());
}
#[test]
fn desktop_provider_models_and_usage() {
    models_and_usage(desktop());
}
#[test]
fn phone_provider_models_and_usage() {
    models_and_usage(phone());
}

fn auth(phase: &str, interaction: Value) -> Value {
    json!({
        "instanceId":"codex", "phase":phase,
        "flowId": (phase == "waiting").then_some("auth-flow"),
        "authorizationUrl":null, "expiresAt":null, "message":null,
        "interaction":interaction
    })
}

fn install(phase: &str) -> Value {
    json!({
        "driver":"codex", "phase":phase, "operationId": (phase == "downloading").then_some("install-operation"),
        "downloadedBytes":1000000, "totalBytes":3000000,
        "version":"2.0.0", "installedVersion":"1.0.0", "canRemove":true, "message":null
    })
}

#[derive(Clone)]
struct ScriptPlatform {
    base: EnvironmentPlatform,
    probe: u32,
    streams: Rc<BTreeMap<&'static str, Value>>,
    log: Rc<RefCell<WireLog>>,
}

impl Storage for ScriptPlatform {
    fn get(&self, area: StorageArea, key: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
        self.base.get(area, key)
    }
    fn write(&self, area: StorageArea, writes: Vec<StorageWrite>) -> HostFuture<'_, ()> {
        self.record_storage(&writes);
        self.base.write(area, writes)
    }
    fn list(&self, area: StorageArea, namespace: String) -> HostFuture<'_, Vec<StorageKey>> {
        self.base.list(area, namespace)
    }
    fn transaction(&self, transaction: StorageTransaction) -> HostFuture<'_, bool> {
        self.record_storage(&transaction.ordinary);
        self.base.transaction(transaction)
    }
}

impl ScriptPlatform {
    fn record_storage(&self, writes: &[StorageWrite]) {
        for value in writes.iter().filter_map(|write| write.value.as_deref()) {
            if let Ok(settings) = serde_json::from_slice::<Value>(value)
                && let Some(weights) = settings
                    .get("payload")
                    .and_then(|payload| payload.get("loadBalancingWeights"))
            {
                self.log
                    .borrow_mut()
                    .entry("loadBalancingWeights")
                    .or_default()
                    .push(weights.clone());
            }
        }
    }
}

impl Platform for ScriptPlatform {
    fn now_epoch_ms(&self) -> u64 {
        self.base.now_epoch_ms()
    }
    fn monotonic_ms(&self) -> u64 {
        self.base.monotonic_ms()
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>, PlatformError> {
        self.base.random_bytes(length)
    }
    fn sleep(&self, milliseconds: u64) -> LocalBoxFuture<'_, ()> {
        self.base.sleep(milliseconds)
    }
    fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
        self.base.yield_to_host()
    }
    fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
        self.base.events()
    }
    fn http(&self, request: HttpRequest) -> HostFuture<'_, HttpResponse> {
        self.base.http(request)
    }
    fn host(&self, action: HostAction) -> HostFuture<'_, HostReply> {
        if let HostAction::OpenUrl(url) = action {
            self.log
                .borrow_mut()
                .entry("OpenUrl")
                .or_default()
                .push(json!(url));
            WIRE_LOGS
                .lock()
                .expect("wire logs")
                .entry(self.probe)
                .or_default()
                .insert("OpenUrl", self.log.borrow()["OpenUrl"].clone());
            return async { Ok(HostReply::Unit) }.boxed_local();
        }
        self.base.host(action)
    }
    fn websocket(&self, request: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
        async move {
            let (send, events) = mpsc::unbounded();
            Ok(Box::new(ScriptSocket {
                base: self.base.websocket(request).await?,
                streams: self.streams.clone(),
                log: self.log.clone(),
                send,
                events,
                subscriptions: BTreeSet::new(),
            }) as Box<dyn WebSocket>)
        }
        .boxed_local()
    }
}

struct ScriptSocket {
    base: Box<dyn WebSocket>,
    streams: Rc<BTreeMap<&'static str, Value>>,
    log: Rc<RefCell<WireLog>>,
    send: mpsc::UnboundedSender<TestFrame>,
    events: mpsc::UnboundedReceiver<TestFrame>,
    subscriptions: BTreeSet<String>,
}

impl WebSocket for ScriptSocket {
    fn send(&mut self, frame: TestFrame) -> HostFuture<'_, ()> {
        let text = match &frame {
            TestFrame::Text(text) => Some(text.as_str()),
            TestFrame::Binary(bytes) => std::str::from_utf8(bytes).ok(),
        };
        if let Some(request) = text.and_then(|text| serde_json::from_str::<Value>(text).ok()) {
            match request["_tag"].as_str() {
                Some("Request") => {
                    if !request["id"].is_null()
                        && let Some(tag) = request["tag"].as_str()
                        && let Some((tag, state)) = self.streams.get_key_value(tag)
                    {
                        self.log
                            .borrow_mut()
                            .entry(tag)
                            .or_default()
                            .push(request["payload"].clone());
                        let id = &request["id"];
                        self.subscriptions.insert(id.to_string());
                        self.send
                            .unbounded_send(TestFrame::Text(
                                json!({
                                    "_tag":"Chunk", "requestId":id, "values":[state]
                                })
                                .to_string(),
                            ))
                            .expect("stream is open");
                        return async { Ok(()) }.boxed_local();
                    }
                }
                Some("Ack" | "Interrupt")
                    if self
                        .subscriptions
                        .contains(&request["requestId"].to_string()) =>
                {
                    return async { Ok(()) }.boxed_local();
                }
                _ => {}
            }
        }
        self.base.send(frame)
    }

    fn receive(&mut self) -> HostFuture<'_, Option<TestFrame>> {
        async move {
            match select(self.events.next(), self.base.receive()).await {
                Either::Left((frame, _)) => Ok(frame),
                Either::Right((frame, _)) => frame,
            }
        }
        .boxed_local()
    }

    fn close(&mut self) -> HostFuture<'_, ()> {
        self.base.close()
    }
}

fn scenario(name: &str) -> (Headless, Probe) {
    start_scenario(1280.0, nsh_core::platform::Idiom::Desktop, name)
}

#[test]
fn auth_terminal_does_not_display_or_accept_secret_input() {
    let (mut app, probe) = scenario("terminal");
    app.wait_for("role:button[Cancel]")
        .expect("terminal authentication is running");
    assert!(!app.screen().shows("Private prompt"));
    assert!(!app.screen().matches("label:Terminal input").unwrap());
    app.press("role:button[Cancel]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "provider.auth.cancel"),
        [json!({"instanceId":"codex","flowId":"auth-flow"})]
    );
}

#[test]
fn provider_update_keeps_hub_dialog_and_auth_subscription() {
    let (mut app, probe) = scenario("");
    app.wait_for("role:button[Sign out]").unwrap();
    app.press("role:button[Add hub]").unwrap();
    app.type_text("label:Hub URL", "https://draft.test")
        .unwrap();
    app.press_key("ctrl+shift+g").unwrap();
    assert!(app.screen().shows("Add a CLIProxyAPI hub"));
    assert_eq!(calls(&mut app, &probe, "provider.auth.subscribe").len(), 1);
    app.type_text("label:Management key", "fixture-key")
        .unwrap();
    app.press("role:dialog[Add a CLIProxyAPI hub] >> role:button[Add hub]")
        .unwrap();
    assert_eq!(
        calls(&mut app, &probe, "server.updateSettings")[0]["patch"]["usageLimitSources"]["cliproxy-draft.test"]
            ["url"],
        "https://draft.test"
    );
}

#[test]
fn authorization_url_without_interaction_opens_browser() {
    let (mut app, probe) = scenario("fallback");
    app.wait_for("role:button[Open browser]").unwrap();
    app.press("role:button[Open browser]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "OpenUrl"),
        [json!("https://signin.test/fallback")]
    );
}

#[test]
fn successful_auth_with_unknown_provider_status_is_signed_in() {
    let (mut app, _probe) = scenario("signed-in");
    app.wait_for("role:button[Sign out]").unwrap();
    assert!(app.screen().shows("Signed in."));
}

#[test]
fn refused_browser_consent_does_not_open_the_url() {
    let (mut app, probe) = scenario("consent-denied");
    app.wait_until("sign-in loaded", nsh_testing::WAIT, |screen| {
        screen
            .accessibility_state("role:button[Sign in]")
            .is_ok_and(|state| !state.disabled)
    })
    .unwrap();
    app.press("role:button[Sign in]").unwrap();
    app.press("role:button[Open sign-in page]").unwrap();
    app.wait_for("The server rejected the request.: Consent refused")
        .unwrap();
    assert_eq!(
        calls(&mut app, &probe, "provider.auth.start"),
        [json!({"instanceId":"codex"})]
    );
    assert_eq!(
        calls(&mut app, &probe, "provider.auth.respond"),
        [json!({
            "instanceId":"codex","flowId":"auth-flow","interactionId":"auth-interaction",
            "response":{"type":"browser","action":"accept"}
        })]
    );
    app.press_key("ctrl+shift+u").unwrap();
    assert!(!WIRE_LOGS.lock().expect("wire logs")[&probe.0].contains_key("OpenUrl"));
}

#[test]
fn remote_managed_sign_in_explains_why_it_is_disabled() {
    let (mut app, _probe) = scenario("remote-managed");
    app.wait_for("ChatGPT sign-in is unavailable on this remote connection.")
        .unwrap();
}

#[test]
fn install_update_cancel_and_confirm_remove() {
    let (mut app, probe) = scenario("install");
    app.wait_for("role:button[Update runtime]").unwrap();
    app.press("role:button[Update runtime]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "provider.install.start"),
        [json!({"instanceId":"codex"})]
    );
    app.wait_for("Downloading 1.0 MB of 3.0 MB.").unwrap();
    app.press("role:button[Cancel installation]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "provider.install.cancel"),
        [json!({"instanceId":"codex","operationId":"install-operation"})]
    );
    app.press("role:button[Remove runtime]").unwrap();
    assert!(calls(&mut app, &probe, "provider.install.remove").is_empty());
    app.press("role:button[Confirm]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "provider.install.remove"),
        [json!({"instanceId":"codex"})]
    );
}

#[test]
fn model_actions_migrate_device_preferences() {
    let (mut app, probe) = scenario("model-prefs");
    app.wait_for("role:button[Move Other Model down]").unwrap();
    app.press("role:button[Move Other Model down]").unwrap();
    app.press("role:switch[Enable Fixture Model]").unwrap();
    app.press("role:button[Remove legacy-favorite from favorites]")
        .unwrap();
    let requests = calls(&mut app, &probe, "server.updateSettings");
    assert_eq!(requests.len(), 3);
    let migration = json!({
        "favorites":[{"provider":"codex","model":"legacy-favorite"}],
        "providerModelPreferences":{"codex":{"hiddenModels":["legacy-hidden"],"modelOrder":["other-model","fixture-model"]}}
    });
    for request in &requests {
        assert_eq!(request["patch"]["migrateModelPreferences"], migration);
    }
    assert_eq!(
        requests[0]["patch"]["setProviderModelOrder"],
        json!({"provider":"codex","modelOrder":["legacy-favorite","fixture-model","other-model"]})
    );
    assert_eq!(
        requests[1]["patch"]["setModelsHidden"],
        json!([{"provider":"codex","model":"fixture-model","hidden":true}])
    );
    assert_eq!(
        requests[2]["patch"]["setModelFavorites"],
        json!([{"provider":"codex","model":"legacy-favorite","favorite":false}])
    );
}

#[test]
fn deleting_registry_instance_cleans_up_managed_binary() {
    let (mut app, probe) = scenario("acp");
    app.wait_for("role:button[Select Registry Fixture]")
        .unwrap();
    app.press("role:button[Select Registry Fixture]").unwrap();
    app.press("role:button[Delete provider instance]").unwrap();
    app.press("role:button[Confirm]").unwrap();
    let settings = calls(&mut app, &probe, "server.updateSettings");
    assert_eq!(
        settings[0]["providerInstanceMutation"],
        json!({"operation":"remove","instanceId":"registry-fixture"})
    );
    assert_eq!(
        calls(&mut app, &probe, "server.uninstallAcpRegistryManagedBinary"),
        [json!({"agentId":"registry-agent"})]
    );
}

#[test]
fn refresh_rpc_failure_does_not_show_a_toast() {
    let (mut app, probe) = desktop();
    app.wait_for("role:button[Select Fixture Codex]").unwrap();
    app.press("role:button[Refresh provider status]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "server.refreshProviders"),
        [json!({"refreshModels":true,"reloadSecrets":true})]
    );
    assert!(!app.screen().shows("Could not refresh provider status"));
}

#[test]
fn sign_out_uses_the_selected_instance() {
    let (mut app, probe) = phone();
    app.wait_until("sign-out loaded", nsh_testing::WAIT, |screen| {
        screen
            .accessibility_state("role:button[Sign out]")
            .is_ok_and(|state| !state.disabled)
    })
    .unwrap();
    app.press("role:button[Sign out]").unwrap();
    assert!(calls(&mut app, &probe, "provider.auth.logout").is_empty());
    app.press("role:button[Confirm]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "provider.auth.logout"),
        [json!({"instanceId":"codex"})]
    );
}

#[test]
fn managed_chatgpt_sign_in_uses_the_server_callback() {
    let (mut app, probe) = scenario("managed");
    app.wait_for("Installed.").unwrap();
    app.press("role:button[Continue with ChatGPT]").unwrap();
    app.wait_until(
        "managed sign-in opened the browser",
        nsh_testing::WAIT,
        |_| {
            WIRE_LOGS
                .lock()
                .unwrap()
                .get(&probe.0)
                .is_some_and(|log| log.contains_key("OpenUrl"))
        },
    )
    .unwrap();
    assert_eq!(
        calls(&mut app, &probe, "provider.auth.start"),
        [json!({
            "instanceId":"codex","methodId":"chatgpt","callbackMode":"server"
        })],
        "install calls: {:?}; visible text: {:?}",
        calls(&mut app, &probe, "provider.install.subscribe"),
        app.screen().texts()
    );
    assert_eq!(
        calls(&mut app, &probe, "OpenUrl"),
        [json!("https://signin.test/consent")]
    );
}

#[test]
fn acp_controls_list_import_and_delete_native_sessions() {
    let (mut app, probe) = scenario("acp");
    app.wait_for("role:button[Select Registry Fixture]")
        .unwrap();
    app.press("role:button[Select Registry Fixture]").unwrap();
    app.press("role:button[List sessions]").unwrap();
    app.wait_for("Native fixture").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "server.listAcpRegistrySessions"),
        [json!({
            "instanceId":"registry-fixture","projectId":"project"
        })]
    );
    app.press_key("ctrl+shift+g").unwrap();
    app.press("role:button[Import]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "server.importAcpRegistrySession"),
        [json!({
            "instanceId":"registry-fixture","projectId":"project","sessionId":"native-session","title":"Native fixture"
        })]
    );
    app.press("role:button[List sessions]").unwrap();
    app.press("role:button[Delete]").unwrap();
    app.press("role:button[Confirm]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "server.deleteAcpRegistrySession"),
        [json!({
            "instanceId":"registry-fixture","projectId":"project","sessionId":"native-session"
        })]
    );
}

#[test]
fn project_provider_toggle_and_reset_write_overrides() {
    let (mut app, probe) = scenario("project-overrides");
    app.wait_for("role:button[Select Fixture Codex]").unwrap();
    app.press("role:button[Project scope: All projects]")
        .unwrap();
    app.press("role:menuitemradio[Project]").unwrap();
    app.press("role:switch[Enable Fixture Codex]").unwrap();
    app.press("role:button[Reset to the device setting]")
        .unwrap();
    let requests = calls(&mut app, &probe, "server.updateSettings");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["patch"]["projectSettingsOverrides"],
        json!({
            "project":{"providerInstances":{"codex":{"enabled":true}}}
        })
    );
    assert_eq!(
        requests[1]["patch"]["projectSettingsOverrides"],
        json!({"project":null})
    );
}

#[test]
fn reset_add_and_custom_model_send_the_instance_mutations() {
    let (mut app, probe) = desktop();
    app.wait_for("role:button[Reset provider settings]")
        .unwrap();
    app.press("role:button[Reset provider settings]").unwrap();
    app.press("role:button[Confirm]").unwrap();
    app.press("role:button[Add custom model]").unwrap();
    app.type_text("label:Model ID", " custom.fixture ").unwrap();
    app.press_key("ctrl+shift+g").unwrap();
    app.press("role:button[Add model]").unwrap();
    app.press("role:button[Add provider]").unwrap();
    app.type_text("label:Instance ID", " fixture-extra ")
        .unwrap();
    app.press("role:button[Add provider] >> nth=1").unwrap();
    let requests = calls(&mut app, &probe, "server.updateSettings");
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[0]["providerInstanceMutation"],
        json!({"operation":"remove","instanceId":"codex"})
    );
    assert_eq!(
        requests[1]["providerInstanceMutation"]["instance"]["config"]["customModels"],
        json!(["custom.fixture"])
    );
    assert_eq!(
        requests[2]["providerInstanceMutation"]["operation"],
        "create"
    );
    assert_eq!(
        requests[2]["providerInstanceMutation"]["instanceId"],
        "fixture-extra"
    );
}

#[test]
fn load_weight_chooses_the_device_setting() {
    let (mut app, probe) = desktop();
    app.wait_for("Load balancing").unwrap();
    let toggle = "role:switch[Automatically balance load]";
    if app.screen().accessibility_state(toggle).unwrap().checked != Some(Checked::True) {
        app.press(toggle).unwrap();
    }
    app.press("role:combobox[Fake host load preference] >> nth=1")
        .unwrap();
    app.press("role:option[Manual only]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "loadBalancingWeights").last(),
        Some(&json!({"second-provider-machine":0}))
    );
    assert!(calls(&mut app, &probe, "server.updateSettings").is_empty());
}

#[test]
fn invalid_browser_url_cannot_send_consent() {
    let (mut app, probe) = scenario("invalid-url");
    app.wait_for("role:button[Open sign-in page]").unwrap();
    assert!(
        app.screen()
            .accessibility_state("role:button[Open sign-in page]")
            .unwrap()
            .disabled
    );
    assert!(app.press("role:button[Open sign-in page]").is_err());
    assert!(calls(&mut app, &probe, "provider.auth.respond").is_empty());
    assert!(!WIRE_LOGS.lock().expect("wire logs")[&probe.0].contains_key("OpenUrl"));
}

#[test]
fn blocked_provider_controls_explain_the_managed_policy() {
    let (mut app, _probe) = scenario("policy");
    app.wait_for("Disabled by policy").unwrap();
    app.wait_for("The display name is locked by policy.")
        .unwrap();
    assert!(
        app.screen()
            .accessibility_state("role:switch[Enable Fixture Codex]")
            .unwrap()
            .disabled
    );
    assert!(
        app.screen()
            .accessibility_state("label:Display name")
            .unwrap()
            .disabled
    );
}

#[test]
fn renamed_provider_preserves_the_credentials_draft() {
    let (mut app, probe) = scenario("draft-credentials");
    app.wait_for("label:API key").unwrap();
    app.type_text("label:API key", "fixture-secret").unwrap();
    app.press_key("ctrl+shift+g").unwrap();
    app.wait_for("role:button[Select Renamed Codex]").unwrap();
    app.press("role:button[Connect]").unwrap();
    assert_eq!(
        calls(&mut app, &probe, "provider.auth.respond"),
        [json!({
            "instanceId":"codex","flowId":"auth-flow","interactionId":"auth-interaction",
            "response":{"type":"credentials","values":{"apiKey":"fixture-secret"}}
        })]
    );
}
