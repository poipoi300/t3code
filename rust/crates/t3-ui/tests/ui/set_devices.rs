//! The Devices settings route reads the selected environment's device state and writes real RPCs.
use crate::support::{DESKTOP, PHONE};
use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use serde_json::json;
use std::rc::Rc;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::{CallKind, environment::FakeEnvironment},
};
use t3_ui::{
    runtime::{Rt, host::Choice},
    screens::settings::shell,
    state::nav::{Navigator, Route, SettingsPage},
    theme,
};

#[derive(Clone, PartialEq, Props)]
struct FixtureProps {
    writable: bool,
    one_pool: bool,
}
#[component]
fn Fixture(props: FixtureProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    provide_context(Navigator::new(Route::Settings(SettingsPage::Devices)));
    let owner = Owner::current();
    let spawn_task: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        })
    });
    let server = FakeEnvironment::new("devices", spawn_task.clone());
    let mut scopes = vec!["orchestration:read", "preview:operate"];
    if props.writable {
        scopes.push("settings:write");
    }
    server.set_permissions(&scopes);
    server.edit_config(|c|{c["settings"]["enableDeviceSupport"]=json!(true);c["settings"]["enableAgentDeviceAccess"]=json!(true);c["settings"]["simlockDeviceHostsEnabled"]=json!(true);
        c["settings"]["easDeviceHosts"]=json!([{"kind":"eas","id":"expo","label":"Expo team","accessToken":"********","maxSessionMinutes":30,"idleMinutes":10,"maxConcurrentSessions":2}]);
        c["settings"]["simlockDeviceHosts"]=json!([{"kind":"simlock","id":"pool","label":"Device lab","gatewayUrl":"https://pool.example.test","agentToken":"","maxLeases":2,"maxLeaseMinutes":60}]);
    });
    if props.one_pool {
        server.edit_config(|config| {
            config["settings"]["easDeviceHosts"] = json!([]);
            config["settings"]["simlockDeviceHosts"][0]["agentToken"] = json!("********");
        });
    }
    server.register("device.simlockIdentity", CallKind::Unary);
    let state = json!({"supportsEasHosts":true,"supportsSimlockHosts":true,"supportsToolInspection":true,
        "hostStatus":"ready","hostStatuses":{},"devices":[],"sessions":[],"hosts":[],"onboardingCompleted":true,"agentAccessEnabled":true,"hubBasePath":"/api/device-hub","revision":1});
    server.register("subscribeDeviceState", CallKind::Stream);
    server.stream_snapshot("subscribeDeviceState", vec![state.clone()]);
    server.register("device.configure", CallKind::Unary);
    server.reply("device.configure", Ok(state));
    let store = ClientStore::default();
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        t3_contracts::orchestration_v2::OrchestrationV2CreationSource::Web,
        spawn_task.clone(),
    );
    let pairing = client.pair(server.pairing());
    spawn_task(
        async move {
            pairing.await.expect("pair device settings");
        }
        .boxed_local(),
    );
    let observed = signal(String::new());
    let writes = server.clone();
    let check_server = writes.clone();
    let account_server = server.clone();
    on_key("ctrl+shift+e", move |_| {
        let saved = account_server.calls("server.updateSettings");
        if saved
            .last()
            .is_some_and(|p| p["patch"]["easDeviceHosts"][0]["accessToken"] == "********")
        {
            observed.set("Saved Expo credential preserved".into());
        }
    });
    on_key("ctrl+shift+i", move |_| {
        let checks = check_server.calls("device.simlockIdentity");
        let patches = check_server.calls("server.updateSettings");
        if checks.last() == Some(&json!({"gatewayUrl":"https://pool.example.test"})) {
            observed.set("Checked machine credential".into());
        } else if patches
            .last()
            .is_some_and(|p| p["patch"]["simlockDeviceHosts"][0]["agentToken"] == "********")
        {
            observed.set("Saved token preserved".into());
        } else {
            observed.set("Credential operation mismatch".into());
        }
    });
    on_key("ctrl+shift+p", move |_| {
        let calls = writes.calls("server.updateSettings");
        if let Some(pools) = calls
            .last()
            .and_then(|c| c["patch"]["simlockDeviceHosts"].as_array())
            && pools.len() == 2
            && pools[0]["id"] == "pool"
            && pools[1]["gatewayUrl"] == "https://second.example.test"
            && pools[1]["agentToken"] == "fixture-token"
        {
            observed.set("Added pool and preserved existing source".into());
        }
    });
    on_key("ctrl+shift+u", move |_| {
        let calls = server.calls("device.configure");
        observed.set(
            if calls == [json!({"enabled":false,"agentAccessEnabled":false})] {
                "Hub and agent access disabled".to_owned()
            } else {
                format!("{} configuration calls", calls.len())
            },
        );
    });
    provide_context(Rt::new(store, Rc::new(client.clone()), Choice::Client));
    view()
        .flex_1()
        .child(text(move || observed.get()))
        .child(untracked(|| {
            shell::screen_for(SettingsPage::Devices, &client)
        }))
}
fn start(window: (f32, f32), writable: bool) -> Headless {
    start_with(window, writable, false)
}
fn start_with(window: (f32, f32), writable: bool, one_pool: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Fixture>(move || FixtureProps {
            writable,
            one_pool,
        }))
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    Headless::start(config).expect("headless device route")
}
fn configure(window: (f32, f32)) {
    let mut app = start(window, true);
    app.wait_for("Expo team").expect("EAS source");
    app.wait_for("Device lab").expect("Simlock pool");
    app.press("role:switch[Device hub]").expect("disable hub");
    app.settle();
    app.press_key("ctrl+shift+u").expect("observe");
    app.wait_for("Hub and agent access disabled")
        .expect("one atomic device.configure");
}
#[test]
fn desktop_hub_and_sources() {
    configure(DESKTOP);
}
#[test]
fn phone_hub_and_sources() {
    configure(PHONE);
}
#[test]
fn read_only_device_settings() {
    for w in [DESKTOP, PHONE] {
        let mut app = start(w, false);
        app.wait_for("Expo team").expect("read state");
        assert!(
            !app.screen()
                .enabled("role:switch[Device hub]")
                .expect("hub switch")
        );
    }
}

#[test]
fn custom_simlock_source_saves_to_selected_environment() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true);
        app.wait_for("Device lab").expect("selected server sources");
        app.press("role:button[Add source]").expect("source menu");
        app.press("role:menuitem[Simlock pool (alpha)]")
            .expect("new pool");
        app.wait_for("Add Simlock host").expect("editor");
        assert!(
            !app.screen()
                .enabled("role:button[Save host]")
                .expect("initial validation")
        );
        app.type_text("label:Gateway URL", "https://second.example.test")
            .expect("set gateway");
        app.type_text("label:Agent token", "fixture-token")
            .expect("set agent token");
        app.press("role:button[Save host]")
            .expect("commit new source");
        app.settle();
        app.press_key("ctrl+shift+p")
            .expect("inspect real settings command");
        app.wait_for("Added pool and preserved existing source")
            .expect("scoped source patch");
    }
}

#[test]
fn emptying_a_trial_token_preserves_saved_credential() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_with(window, true, true);
        app.wait_for("Device lab").expect("pool");
        app.press("role:button[Edit]").expect("edit pool");
        app.type_text("label:Agent token", "x")
            .expect("trial token");
        app.press_key("backspace").expect("erase trial");
        app.press("role:button[Save host]").expect("save host");
        app.settle();
        app.press_key("ctrl+shift+i").expect("read commands");
        app.wait_for("Saved token preserved")
            .expect("untouched credential");
    }
}
#[test]
fn clearing_a_pool_token_checks_only_the_machine_credential() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_with(window, true, true);
        app.wait_for("Device lab").expect("pool");
        app.press("role:button[Edit]").expect("edit pool");
        app.press("role:button[Clear agent token]")
            .expect("clear configured token");
        app.press("role:button[Check identity]")
            .expect("check cleared credential");
        app.settle();
        app.press_key("ctrl+shift+i").expect("read commands");
        app.wait_for("Checked machine credential")
            .expect("no saved host id or token");
    }
}

#[test]
fn uncommitted_reference_selection_preserves_saved_expo_token() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, true);
        app.wait_for("Expo team").expect("Expo host");
        app.press("role:button[Edit] >> nth=0")
            .expect("edit Expo host");
        app.wait_for("Edit EAS host").expect("Expo editor");
        app.press("role:button[Secret reference]")
            .expect("choose reference entry");
        app.press("role:button[Secret reference]")
            .expect("return to literal entry");
        app.press("role:button[Save host]")
            .expect("save without replacing token");
        app.settle();
        app.press_key("ctrl+shift+e").expect("read patch");
        app.wait_for("Saved Expo credential preserved")
            .expect("original token");
    }
}
