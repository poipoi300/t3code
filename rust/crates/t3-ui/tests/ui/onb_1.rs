//! Welcome through the app root, with the production client paired to a fake computer.

use std::rc::Rc;

use futures::{FutureExt, StreamExt};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::{
    ClientStore,
    composition::{ClientRuntime, Spawner},
    testing::environment::{FakeEnvironment, shell},
};
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::{
    runtime::{Rt, host::Choice},
    state::nav::Route,
};

#[derive(Clone, PartialEq, Props)]
struct SetupProps {
    home: bool,
    failed_settings: bool,
    ready_agent: bool,
}

#[component]
fn Setup(props: SetupProps) -> impl IntoElement {
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        })
    });
    let store = ClientStore::default();
    let server = FakeEnvironment::new("env", spawner.clone());
    if props.failed_settings {
        let read = server.defer_read(t3_client::platform::StorageKey::new(
            "client-settings",
            None,
            "t3code:client-settings:v1",
        ));
        read.send(Some(b"damaged record".to_vec()))
            .expect("first settings read");
    }
    server.edit_config(|config| {
        config["settings"] = serde_json::to_value(t3_contracts::settings::ServerSettings::default()).expect("settings");
        config["providers"] = if props.ready_agent {
            serde_json::json!([{"instanceId":"codex","driver":"codex","displayName":"Codex","enabled":true,"installed":true,"version":"1.0.0","status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-08-23T00:00:00.000Z","models":[],"slashCommands":[],"skills":[]}])
        } else {serde_json::json!([])};
        config
            .as_object_mut()
            .expect("object")
            .remove("managedPolicy");
    });
    let mut empty = shell(&[]);
    empty["projects"] = serde_json::json!([]);
    server.set_shell(empty);
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let setup = client.clone();
    spawner(async move {
        setup.restore().await.expect("restore");
        let id=setup.pair(server.pairing()).await.expect("pair");
        let mut changes=setup.store().subscribe_environment(&id);
        while changes.next().await.is_some(){
            let snapshot=setup.store().snapshot();
            let Some(env)=snapshot.environments.get(&id) else {continue;};
            if let Some(config)=&env.config && env.shell.status==t3_client::types::SyncStatus::Live {
                // The shared UI fake serves shell and config streams. Deliver its matching
                // lifecycle evidence at the store boundary. client_runtime tests the wire.
                let welcome=serde_json::from_value(serde_json::json!({"environment":config.environment,"cwd":config.cwd,"projectName":"Test","bootstrapStatus":"complete"})).expect("welcome");
                setup.store().set_welcome(&id,env.session,Some(welcome));
                break;
            }
        }
    }.boxed_local());
    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);
    // App supplies the viewport above app_tree after asynchronous boot.
    view()
        .w_full()
        .h_full()
        .child(t3_ui::app::app_tree(
            runtime,
            if props.home {
                Route::Home
            } else {
                Route::Welcome
            },
            move || {
                t3_ui::app::provide_client_services(&client);
                provide_context(t3_ui::screens::onboarding::welcome::Onboarding::new(
                    &client,
                ));
            },
        ))
        .into_element()
}

pub(super) fn open(route: Route) -> Headless {
    open_with(route, false, false)
}

pub(super) fn open_with(route: Route, failed_settings: bool, ready_agent: bool) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(1280.0), px(800.0)),
        ..EngineConfig::new(Root::new::<Setup>(move || SetupProps {
            home: route == Route::Home,
            failed_settings,
            ready_agent,
        }))
    };
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    config.environment.os = nsh_core::platform::Os::Linux;
    Headless::start(config).expect("start root")
}

#[test]
fn start_leaves_the_welcome_page() {
    let mut app = open(Route::Welcome);
    app.wait_for("Welcome to T3 Code").expect("welcome page");
    app.press("role:button[Start without an agent]")
        .expect("start");
    app.wait_for_gone("Welcome to T3 Code", std::time::Duration::from_secs(5))
        .expect("left setup");
}

#[test]
fn fresh_home_opens_setup_through_the_root_gate() {
    let mut app = open(Route::Home);
    app.wait_for("Welcome to T3 Code")
        .expect("fresh home redirects");
}

#[test]
fn retrying_a_failed_settings_read_releases_the_root_gate() {
    let mut app = open_with(Route::Home, true, false);
    app.wait_for("Could not read settings").expect("recovery");
    app.press("role:button[Retry]").expect("retry settings");
    app.wait_for("Welcome to T3 Code").expect("fresh workspace");
}
