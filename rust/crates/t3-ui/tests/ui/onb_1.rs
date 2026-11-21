//! Welcome through the app root, with the production client paired to a fake computer.

use std::{
    cell::Cell,
    rc::Rc,
    sync::{Mutex, mpsc},
};

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

#[derive(Clone, Copy, Default, PartialEq)]
enum Scenario {
    #[default]
    Fresh,
    AwaitCatalog,
    EmptyCatalog,
    Completed,
    FailedSettings,
    NoWelcome,
    MultipleProjects,
    ConflictingCompletion,
}

struct Probe {
    server: FakeEnvironment,
    client: ClientRuntime<t3_client::testing::environment::EnvironmentPlatform>,
    release_catalog: Cell<Option<futures::channel::oneshot::Sender<()>>>,
    stack_depth: Cell<usize>,
}

// The engine owns the !Send client. Only commands and their plain results cross threads.
type Command = Box<dyn FnOnce(&Probe) + Send>;
struct Controls(futures::channel::mpsc::UnboundedSender<Command>);

impl Controls {
    fn inspect<T: Send + 'static>(&self, run: impl FnOnce(&Probe) -> T + Send + 'static) -> T {
        let (reply, result) = mpsc::sync_channel(1);
        self.0
            .unbounded_send(Box::new(move |probe| {
                let _ = reply.send(run(probe));
            }))
            .expect("running fixture");
        result
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("fixture reply")
    }
    fn release(&self) {
        self.inspect(|p| {
            p.release_catalog
                .take()
                .expect("held catalog")
                .send(())
                .expect("release")
        });
    }
}

struct SetupInput {
    route: Route,
    scenario: Scenario,
    ready_agent: bool,
    commands: futures::channel::mpsc::UnboundedReceiver<Command>,
}
// Root constructs its props on the engine thread, immediately before mounting the component.
thread_local! { static INPUT: Cell<Option<SetupInput>> = const { Cell::new(None) }; }

fn settings_key() -> t3_client::platform::StorageKey {
    t3_client::platform::StorageKey::new("client-settings", None, "t3code:client-settings:v1")
}

#[component]
fn Setup() -> impl IntoElement {
    let SetupInput {
        route,
        scenario,
        ready_agent,
        mut commands,
    } = INPUT.take().expect("root input");
    let owner = Owner::current();
    let spawner: Spawner = Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        })
    });
    let store = ClientStore::default();
    let server = FakeEnvironment::new("env", spawner.clone());
    if scenario == Scenario::FailedSettings || scenario == Scenario::Completed {
        let bytes = if scenario == Scenario::FailedSettings {
            b"damaged record".to_vec()
        } else {
            let settings = t3_contracts::settings::ClientSettingsSchema {
                onboarding_completed_at: Some("2026-01-01T00:00:00Z".into()),
                ..Default::default()
            };
            t3_client::persistence::Record::new(
                "client-settings",
                None,
                settings_key().record,
                1,
                settings,
            )
            .encode()
            .expect("settings")
        };
        server
            .defer_read(settings_key())
            .send(Some(bytes))
            .expect("first settings read");
    }
    server.edit_config(|config| {
        config["settings"] = serde_json::to_value(t3_contracts::settings::ServerSettings::default()).expect("settings");
        config["providers"] = if ready_agent {
            serde_json::json!([{"instanceId":"codex","driver":"codex","displayName":"Codex","enabled":true,"installed":true,"version":"1.0.0","status":"ready","auth":{"status":"authenticated"},"checkedAt":"2026-08-23T00:00:00.000Z","models":[],"slashCommands":[],"skills":[]}])
        } else {serde_json::json!([])};
        config
            .as_object_mut()
            .expect("object")
            .remove("managedPolicy");
    });
    let mut empty = shell(&[]);
    if matches!(
        scenario,
        Scenario::MultipleProjects | Scenario::ConflictingCompletion
    ) {
        let mut second = empty["projects"][0].clone();
        second["id"] = "another-project".into();
        empty["projects"]
            .as_array_mut()
            .expect("projects")
            .push(second);
    } else {
        empty["projects"] = serde_json::json!([]);
    }
    server.set_shell(empty);
    let client = ClientRuntime::new(
        server.platform(),
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    let probe = Rc::new(Probe {
        server: server.clone(),
        client: client.clone(),
        release_catalog: Cell::new(None),
        stack_depth: Cell::new(0),
    });
    let inspected = probe.clone();
    spawn(async move {
        while let Some(check) = commands.next().await {
            check(&inspected);
        }
    });
    let wait = if matches!(
        scenario,
        Scenario::AwaitCatalog | Scenario::EmptyCatalog | Scenario::Completed
    ) {
        let (release, wait) = futures::channel::oneshot::channel();
        probe.release_catalog.replace(Some(release));
        Some(wait)
    } else {
        None
    };
    let setup = client.clone();
    spawner(
        async move {
            if let Some(wait) = wait {
                let _ = wait.await;
            }
            setup.restore().await.expect("restore");
            if scenario == Scenario::EmptyCatalog {
                return;
            }
            let id = setup.pair(server.pairing()).await.expect("pair");
            if scenario == Scenario::ConflictingCompletion {
                // The record changes after the settings read, before the conditional write.
                let stale = t3_client::persistence::Record::new(
                    "client-settings",
                    None,
                    settings_key().record,
                    1,
                    t3_contracts::settings::ClientSettingsSchema::default(),
                );
                server
                    .defer_read(settings_key())
                    .send(Some(stale.encode().expect("settings")))
                    .expect("simulate competing settings write");
            }
            setup.supply_test_primary_route(&id);
            if scenario == Scenario::NoWelcome {
                return;
            }
            let mut changes = setup.store().subscribe_environment(&id);
            while changes.next().await.is_some() {
                let snapshot = setup.store().snapshot();
                let Some(env) = snapshot.environments.get(&id) else {
                    continue;
                };
                if let Some(config) = &env.config
                    && env.shell.status == t3_client::types::SyncStatus::Live
                {
                    // The shared UI fake serves shell and config streams. Deliver its matching
                    // lifecycle evidence at the store boundary. client_runtime tests the wire.
                    let welcome = serde_json::from_value(serde_json::json!({
                        "environment": config.environment, "cwd": config.cwd,
                        "projectName": "Test", "bootstrapStatus": "complete"
                    }))
                    .expect("welcome");
                    setup.store().set_welcome(&id, env.session, Some(welcome));
                    break;
                }
            }
        }
        .boxed_local(),
    );
    let runtime = Rt::new(store, Rc::new(client.clone()), Choice::Client);
    // App supplies the viewport above app_tree after asynchronous boot.
    view()
        .w_full()
        .h_full()
        .child(t3_ui::app::app_tree(runtime, route, move || {
            let nav = t3_ui::state::nav::navigator();
            effect(move || probe.stack_depth.set(nav.stack.with(Vec::len)));
            t3_ui::app::provide_client_services(&client);
            provide_context(t3_ui::screens::onboarding::welcome::Onboarding::new(
                &client,
            ));
        }))
        .into_element()
}

pub(super) fn open(route: Route) -> Headless {
    open_with(route, false, false)
}

pub(super) fn open_with(route: Route, failed_settings: bool, ready_agent: bool) -> Headless {
    let scenario = if failed_settings {
        Scenario::FailedSettings
    } else {
        Scenario::Fresh
    };
    launch(route, scenario, ready_agent).0
}

fn launch(route: Route, scenario: Scenario, ready_agent: bool) -> (Headless, Controls) {
    let (send, receive) = futures::channel::mpsc::unbounded();
    let input = Mutex::new(Some(SetupInput {
        route,
        scenario,
        ready_agent,
        commands: receive,
    }));
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(1280.0), px(800.0)),
        ..EngineConfig::new(Root::new::<Setup>(move || {
            INPUT.set(input.lock().expect("input").take());
        }))
    };
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    config.environment.os = nsh_core::platform::Os::Linux;
    (Headless::start(config).expect("start root"), Controls(send))
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

fn cover(app: &mut Headless) -> bool {
    app.screen().matches("id:first-run-gate").expect("cover")
}

#[test]
fn pending_catalog_covers_the_welcome_outlet_without_flashing_the_home_hero() {
    let (mut app, host) = launch(Route::Welcome, Scenario::AwaitCatalog, false);
    app.settle();
    assert!(cover(&mut app));
    assert!(!app.screen().shows("Select a thread"));
    host.release();
    app.wait_for("Welcome to T3 Code")
        .expect("restored primary");
    assert!(!cover(&mut app));
}

#[test]
fn a_completed_settings_record_keeps_home_out_of_setup() {
    let (mut app, host) = launch(Route::Home, Scenario::Completed, false);
    app.settle();
    assert!(host.inspect(|p| {
        p.client
            .composer_preferences()
            .snapshot()
            .onboarding_completed_at
            .is_some()
    }));
    assert!(!cover(&mut app));
    host.release();
    app.wait_for("Select a thread").expect("home");
    app.settle();
    assert!(host.inspect(|p| p.client.settings().primary_environment().is_some()));
    assert!(!cover(&mut app));
    assert!(!app.screen().shows("Welcome to T3 Code"));
    assert_eq!(host.inspect(|p| p.stack_depth.get()), 1);
}

#[test]
fn the_home_to_welcome_redirect_replaces_the_back_entry() {
    let (mut app, host) = launch(Route::Home, Scenario::Fresh, false);
    app.wait_for("Welcome to T3 Code").expect("redirect");
    assert_eq!(
        host.inspect(|p| p.stack_depth.get()),
        1,
        "Back must not reopen the guarded home"
    );
}

#[test]
fn failed_settings_recovery_on_welcome_disables_retry_until_the_read_finishes() {
    let (mut app, host) = launch(Route::Welcome, Scenario::FailedSettings, false);
    app.wait_for("Could not read settings")
        .expect("recovery on welcome");
    let read = host.inspect(|p| p.server.defer_read(settings_key()));
    app.press("role:button[Retry]").expect("retry");
    app.settle();
    assert!(
        !app.screen()
            .enabled("role:button[Retry]")
            .expect("retry control")
    );
    read.send(None).expect("finish read");
    app.wait_for_gone("Could not read settings", std::time::Duration::from_secs(5))
        .expect("recovered");
    app.wait_for("Welcome to T3 Code").expect("welcome");
}

#[test]
fn stalled_pair_route_reload_reconnects_the_primary() {
    let (mut app, host) = launch(Route::Pair, Scenario::NoWelcome, false);
    app.settle();
    let before = host.inspect(|p| {
        p.server.platform().fake.advance(4000);
        p.client.environments()[0].state.generation
    });
    app.wait_for("Still connecting")
        .expect("stalled recovery on pair");
    app.press("role:button[Reload]").expect("reload");
    app.settle();
    assert_ne!(
        before,
        host.inspect(|p| p.client.environments()[0].state.generation)
    );
}

#[test]
fn multi_project_workspace_persists_completion_even_on_welcome() {
    use t3_client::platform::{Storage, StorageArea};
    let (mut app, host) = launch(Route::Welcome, Scenario::MultipleProjects, false);
    app.wait_for("Welcome to T3 Code")
        .expect("explicit welcome");
    app.settle();
    let (decision, record) = host.inspect(|p| {
        let platform = p.server.platform();
        let bytes = platform
            .get(StorageArea::Ordinary, settings_key())
            .now_or_never()
            .expect("fake read completes immediately")
            .expect("read");
        (p.client.first_run().snapshot().gate.decision, bytes)
    });
    assert_eq!(
        decision,
        t3_client::view_logic::onboarding::FirstRunDecision::App
    );
    let record: serde_json::Value =
        serde_json::from_slice(&record.expect("completion persisted")).expect("record");
    assert!(record["payload"]["onboardingCompletedAt"].is_string());
}

#[test]
fn a_conflicting_automatic_completion_retries_without_leaving_welcome() {
    let (mut app, host) = launch(Route::Welcome, Scenario::ConflictingCompletion, false);
    app.wait_for("Welcome to T3 Code").expect("welcome");
    app.settle();
    let first = host.inspect(|p| {
        let first = p
            .client
            .composer_preferences()
            .snapshot()
            .onboarding_completed_at
            .clone();
        p.server.platform().fake.advance(4000);
        first
    });
    assert!(first.is_none(), "first transaction conflicted");
    app.settle();
    assert!(
        host.inspect(|p| p
            .client
            .composer_preferences()
            .snapshot()
            .onboarding_completed_at
            .is_some()),
        "retry persisted completion"
    );
}

#[test]
fn restoring_an_empty_catalog_releases_the_cover_without_a_connection_event() {
    let (mut app, host) = launch(Route::Home, Scenario::EmptyCatalog, false);
    app.settle();
    assert!(cover(&mut app));
    host.release();
    app.settle();
    assert!(!cover(&mut app));
    assert!(app.screen().shows("Select a thread"));
}
