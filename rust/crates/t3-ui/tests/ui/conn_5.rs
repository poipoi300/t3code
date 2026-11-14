//! CONN-5: a project link opened in the app (`/projects/<key>`) replaces itself, through the
//! production app tree over a client paired to a fake environment.

use std::rc::Rc;

use futures::FutureExt;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::ClientStore;
use t3_client::composition::{ClientRuntime, Spawner};
use t3_client::testing::environment::FakeEnvironment;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::runtime::Rt;
use t3_ui::runtime::host::Choice;
use t3_ui::state::nav::Route;

use crate::support::DESKTOP;

/// The app tree, mounted once its client has paired (as `App` mounts it after restore found a
/// saved environment), opened at a project link, with a probe of the app's address.
#[component]
fn LinkedApp() -> impl IntoElement {
    let store = ClientStore::default();
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    });
    let server = FakeEnvironment::new("env", spawner.clone());
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let paired = signal(false);
    let pairing = client.pair(server.pairing());
    spawner(
        async move {
            if pairing.await.is_ok() {
                paired.set(true);
            }
        }
        .boxed_local(),
    );
    let address = signal(String::new());
    view()
        .w_full()
        .h_full()
        .child(text(move || format!("address {}", address.get())))
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Read address")))
                .on_press(move || {
                    spawn(async move {
                        let _ = address.try_set(app_url().await.unwrap_or_default());
                    });
                })
                .child(text("Read address")),
        )
        .child(dynamic(move || {
            paired.get().then(|| {
                let store = store.clone();
                let client = client.clone();
                untracked(move || {
                    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);
                    t3_ui::app::app_tree(
                        runtime,
                        Route::ProjectLink("env:repo".into()),
                        move || t3_ui::app::provide_client_services(&client),
                    )
                })
            })
        }))
}

#[test]
fn a_project_link_replaces_itself_with_the_project_settings_when_an_environment_is_saved() {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(Root::of::<LinkedApp>())
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    let mut app = Headless::start(config).expect("the engine starts");
    app.wait_for("Settings: projects")
        .expect("the project settings replaced the link");
    app.press("role:button[Read address]").expect("probe");
    app.wait_for("address /settings/projects?project=env%3Arepo")
        .expect("the address carries the project for the settings page");
}
