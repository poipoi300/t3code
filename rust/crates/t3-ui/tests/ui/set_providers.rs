//! Providers settings through the settings route, with a paired client and a wire fixture.

use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{
        Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::environment::FakeEnvironment,
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
    let server = FakeEnvironment::new("provider-machine", spawner.clone());
    server.set_permissions(&[
        "orchestration:read",
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
    });
    let saved = serde_json::to_value(t3_contracts::settings::ServerSettings::default())
        .expect("settings reply");
    for _ in 0..4 {
        server.reply("server.updateSettings", Ok(saved.clone()));
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
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        platform,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    let pairing = client.pair(server.pairing());
    let connecting = client.clone();
    spawner(
        async move {
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
        let log = ["server.updateSettings", "terminal.open", "terminal.write"]
            .into_iter()
            .map(|method| (method, server.calls(method)))
            .collect();
        WIRE_LOGS.lock().expect("wire logs").insert(probe, log);
    });
    view()
        .flex_1()
        .child(t3_ui::ui::confirm::confirm_dialog_host())
        .child(untracked(|| {
            shell::screen_for(SettingsPage::Providers, &client)
        }))
}

fn start(width: f32, idiom: nsh_core::platform::Idiom) -> (Headless, Probe) {
    let probe = Probe::new();
    let root_probe = probe.0;
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(width), px(1400.0)),
        ..EngineConfig::new(Root::new::<Providers>(move || ProvidersFixture {
            two_environments: true,
            probe: root_probe,
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
