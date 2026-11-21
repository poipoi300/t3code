//! SET-SHELL: headless tests of the settings shell (navigation, scope, breadcrumb), mounted as the
//! router mounts it, over the real client runtime the app boots. The harness is shared by the
//! other settings test modules.

use std::cell::RefCell;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::rc::Rc;
use std::time::SystemTime;

use futures::StreamExt;
use nsh_core::prelude::*;
use nsh_core::time::Clock;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::ClientStore;
use t3_client::composition::Spawner;
use t3_client::testing::environment::FakeEnvironment;
use t3_client::thread_views::composer::preferences::SettingsHydration;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_platform::{EphemeralSecretStore, NativeConfig, NativePlatform};
use t3_ui::runtime::Rt;
use t3_ui::runtime::host::{Choice, Started};
use t3_ui::screens::settings::shell;
use t3_ui::state::nav::{Navigator, Route, SettingsPage};
use t3_ui::theme;

use crate::support::{DESKTOP, PHONE};

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    page: usize,
}

fn fixture_spawner() -> Spawner {
    let owner = Owner::current();
    Rc::new(move |future| {
        owner.run(|| {
            spawn(future);
        });
    })
}

/// Mount the production settings route and client over an isolated SQLite store.
/// Device settings use SQLite; the OS credential store is covered by the platform.
#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let nav = Navigator::new(Route::Settings(SettingsPage::ALL[props.page]));
    provide_context(nav);
    let config = NativeConfig::new(
        data_dir()
            .expect("the settings fixture has a data directory")
            .join("client"),
        Box::new(EphemeralSecretStore::default()),
    );
    let io = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("settings fixture I/O runtime");
    let handle = io.handle().clone();
    cleanup(move || drop(io));
    let spawner = fixture_spawner();
    let booted = signal(false);
    let slot: Rc<RefCell<Option<Started>>> = Rc::default();
    let store = ClientStore::default();
    {
        let slot = slot.clone();
        let store = store.clone();
        spawn(async move {
            let mut opening = pin!(NativePlatform::open(config));
            let platform = poll_fn(|cx| {
                let _entered = handle.enter();
                opening.as_mut().poll(cx)
            })
            .await
            .expect("the settings fixture opened its SQLite store");
            let (client, paired) = t3_ui::runtime::client::start_on(
                platform,
                store,
                OrchestrationV2CreationSource::Web,
                spawner,
            )
            .await;
            *slot.borrow_mut() = Some(Started {
                backend: Rc::new(client),
                choice: Choice::Client,
                paired,
            });
            booted.set(true);
        });
    }
    view().flex_1().child(dynamic(move || {
        if !booted.get() {
            return None;
        }
        let started = slot.borrow_mut().take()?;
        Some(untracked(|| booted_root(store.clone(), started)))
    }))
}

fn booted_root(store: ClientStore, started: Started) -> AnyElement {
    let rt = Rt::boot(store, started);
    let nav = t3_ui::state::nav::navigator();
    let client = rt
        .backend()
        .client()
        .expect("the settings fixture has a client");
    theme::bind(&client, None);
    let settings = client.settings();
    let preferences = client.composer_preferences();
    let record = signal(Vec::<String>::new());
    let mut changes = preferences.changes();
    spawn(async move {
        while changes.next().await.is_some() {
            match preferences.hydration() {
                SettingsHydration::Pending | SettingsHydration::Retrying => continue,
                SettingsHydration::Failed => {
                    panic!("settings fixture read failed: {:?}", preferences.error())
                }
                SettingsHydration::Ready => {}
            }
            let s = settings.client_settings();
            record.set(vec![
                format!("delete={}", s.confirm_thread_delete),
                format!("language={}", s.language),
                format!(
                    "send={}",
                    serde_json::to_value(&s.send_shortcut)
                        .unwrap_or_default()
                        .as_str()
                        .unwrap_or_default()
                ),
            ]);
        }
    });
    let line = move |index: usize| {
        text(move || SharedString::from(record.with(|r| r.get(index).cloned().unwrap_or_default())))
    };
    view()
        .flex_1()
        .child(line(0))
        .child(line(1))
        .child(line(2))
        .child(text(move || {
            SharedString::from(match nav.top() {
                Route::Settings(page) => format!("Route {}", page.slug()),
                other => format!("Route {other:?}"),
            })
        }))
        .child(dynamic(move || match nav.top() {
            Route::Settings(page) => untracked(|| shell::screen(page)),
            _ => view().into_element(),
        }))
        .into_element()
}

/// A data directory no other test shares, so each harness starts with no saved settings.
fn fresh_data_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "t3-ui-settings-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

pub fn start(window: (f32, f32), page: SettingsPage) -> Headless {
    let index = SettingsPage::ALL
        .iter()
        .position(|p| *p == page)
        .unwrap_or(0);
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        clock: Clock::manual(SystemTime::UNIX_EPOCH),
        data_dir: Some(fresh_data_dir()),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps { page: index }))
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("settings harness starts");
    app.wait_for("delete=true")
        .expect("the real client booted and read its record");
    app
}

#[test]
fn the_nav_lists_the_web_groups_and_opens_a_section_with_its_breadcrumb() {
    let mut app = start(DESKTOP, SettingsPage::General);
    app.wait_for("Notifications").expect("general page");
    for label in [
        "General",
        "Appearance",
        "Providers",
        "Projects",
        "Environments",
        "Archive",
        "Agents",
        "Code",
        "Connections",
    ] {
        assert!(app.screen().shows(label), "{label}");
    }
    // The secrets page is behind its feature flag, as on the web.
    assert!(
        !app.screen()
            .matches("role:button[Secrets]")
            .expect("selector")
    );
    app.press("role:button[Chat]").expect("chat section");
    app.wait_for("Route chat").expect("navigated");
    app.wait_for("Follow-ups")
        .expect("chat page renders inside the shell");
    // The breadcrumb: "Settings / Chat".
    assert!(app.screen().shows("Settings"));
    assert!(app.screen().shows("/"));
    // Restore device defaults belongs to General only.
    assert!(
        !app.screen()
            .matches("role:button[Restore device defaults]")
            .expect("selector")
    );
}

#[test]
fn the_phone_nav_is_an_icon_rail_whose_search_expands_over_the_page() {
    let mut app = start(PHONE, SettingsPage::General);
    app.wait_for("role:button[General]")
        .expect("rail row keeps its accessible name");
    // Group headings are hidden on the rail.
    assert!(!app.screen().shows("Agents"));
    app.press("role:button[Search settings]")
        .expect("rail search");
    app.wait_for("role:combobox[Search settings]")
        .expect("field shown");
}

#[test]
fn close_returns_to_the_page_that_opened_settings() {
    let mut app = start(DESKTOP, SettingsPage::Chat);
    app.wait_for("Follow-ups").expect("chat page");
    app.press("role:button[Close]").expect("close");
    app.wait_for("Route Home").expect("back to the origin");
}

/// What the connected harness's environment answers.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Remote {
    /// `autoResumeLimitedThreads` on the environment (the built-in default is `false`).
    pub auto_resume: bool,
    /// A managed policy from "Acme" locking `snoozeLimitedThreads`.
    pub policy: bool,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum ReviewFixture {
    #[default]
    Normal,
    SlowAppearance,
    PublishedThemes,
    WorktreeCleanup,
}

#[derive(Clone, PartialEq, Props)]
struct ConnectedProps {
    page: usize,
    auto_resume: bool,
    policy: bool,
    review: usize,
}

/// The production `settings::shell::screen_for` over a client runtime really paired with
/// `t3_client::testing::environment::FakeEnvironment`, which grants `settings:write`. "Read
/// server" shows the `server.updateSettings` payloads the environment received.
#[component]
fn Connected(props: ConnectedProps) -> impl IntoElement {
    use futures::FutureExt;
    use t3_client::composition::ClientRuntime;
    use t3_client::types::EnvironmentId;
    theme::install(&theme::ThemeChoice::default());
    let page = SettingsPage::ALL[props.page];
    provide_context(Navigator::new(Route::Settings(page)));
    let store = ClientStore::default();
    let owner = Owner::current();
    let spawner = fixture_spawner();
    let server = FakeEnvironment::new("env", spawner.clone());
    server.set_permissions(&[
        "orchestration:read",
        "orchestration:operate",
        "settings:write",
    ]);
    server.edit_config(|config| {
        config["settings"]["autoResumeLimitedThreads"] = serde_json::json!(props.auto_resume);
        config["settings"]["snoozeLimitedThreads"] = serde_json::json!(false);
        if props.review == ReviewFixture::PublishedThemes as usize {
            config["environmentThemes"] = serde_json::json!([{"id":"dusk-lab","name":"Lab dusk","appearance":"dark","canvas":"#222233","accent":"#dd66aa"}]);
        }
        if props.policy {
            config["managedPolicy"] =
                serde_json::json!({"organization": "Acme", "lockedSettings": ["snoozeLimitedThreads"]});
        }
    });
    let mut platform = server.platform();
    let mut extra = None;
    if props.review == ReviewFixture::WorktreeCleanup as usize {
        let other = FakeEnvironment::at_origin("other", "https://other.test", spawner.clone());
        for machine in [&server, &other] {
            machine.set_permissions(&[
                "orchestration:read",
                "orchestration:operate",
                "settings:write",
            ]);
            machine.edit_config(|config| {
                config["environment"]["capabilities"]["storageCleanup"] = serde_json::json!(true);
                config["environment"]["capabilities"]["settledWorktreeCleanup"] =
                    serde_json::json!(true);
                config["environment"]["capabilities"]["storageCleanupRun"] =
                    serde_json::json!(true);
                config["settings"]["worktreeCleanup"] = serde_json::Value::Null;
                config["settings"]["storageCleanup"]["worktreeAfterDays"] = serde_json::json!(8);
            });
        }
        other.edit_config(|config| {
            config["settings"]["storageCleanup"]["worktreeKeepWhen"] =
                serde_json::json!("tracked-changes");
        });
        platform = platform.with_environment(&other);
        extra = Some(other);
        let reply = Rc::new(RefCell::new(Some(
            server.defer_reply("server.updateSettings"),
        )));
        on_key("ctrl+shift+e", move |_| {
            if let Some(reply) = reply.borrow_mut().take() {
                let _ = reply.send(Err(
                    serde_json::json!({"message":"Fixture rejected the change"}),
                ));
            }
        });
    }
    if props.review == ReviewFixture::SlowAppearance as usize {
        use t3_client::platform::StorageKey;
        let saved = Rc::new(RefCell::new(Some(server.defer_read(StorageKey::new(
            "ui-preference",
            None,
            "t3code:theme-appearance-mode",
        )))));
        on_key("ctrl+shift+e", move |_| {
            if let Some(saved) = saved.borrow_mut().take() {
                let _ = saved.send(Some(b"light".to_vec()));
            }
        });
    }
    let client = ClientRuntime::new(
        platform,
        store.clone(),
        OrchestrationV2CreationSource::Web,
        spawner.clone(),
    );
    provide_context(Rt::new(
        store.clone(),
        Rc::new(client.clone()),
        t3_ui::runtime::host::Choice::Client,
    ));
    if page == SettingsPage::Appearance {
        let host = (props.review == ReviewFixture::PublishedThemes as usize)
            .then(|| EnvironmentId("env".into()));
        theme::bind(&client, host);
    }
    let pairing = client.pair(server.pairing());
    let additional_id = extra.as_ref().map(|_| EnvironmentId("other".into()));
    {
        let client = client.clone();
        spawner(
            async move {
                let _ = pairing.await;
                if let Some(extra) = extra {
                    let _ = client.pair(extra.pairing()).await;
                }
            }
            .boxed_local(),
        );
    }
    {
        let client = client.clone();
        on_key("ctrl+shift+o", move |_| {
            if let Some(client) = client.client(&EnvironmentId("other".into())) {
                client
                    .close()
                    .expect("disconnect the second fixture machine");
            }
        });
    }
    let ready = signal(false);
    {
        let client = client.clone();
        let mut changes = store.subscribe();
        spawn(async move {
            let environment = EnvironmentId("env".into());
            while changes.next().await.is_some() {
                let session = client
                    .client(&environment)
                    .and_then(|connected| connected.session())
                    .is_some();
                let config = client
                    .store()
                    .snapshot()
                    .environments
                    .get(&environment)
                    .is_some_and(|environment| environment.config.is_some());
                let other_ready = additional_id.as_ref().is_none_or(|id| {
                    client
                        .client(id)
                        .is_some_and(|client| client.session().is_some())
                });
                if session && config && other_ready {
                    ready.set(true);
                }
            }
        });
    }
    let theme_record = signal(String::new());
    if page == SettingsPage::Appearance {
        let settings = client.settings();
        on_key("ctrl+shift+y", move |_| {
            let settings = settings.clone();
            owner.run(move || {
                spawn(async move {
                    let halves = settings
                        .read_device_preference("t3code:theme-halves:v1")
                        .await;
                    let library = settings.read_device_preference("t3code:themes:v1").await;
                    let names = library
                        .as_ref()
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|theme| theme["label"].as_str())
                        .collect::<Vec<_>>();
                    theme_record.set(format!(
                        "Saved themes {} / {}",
                        halves.unwrap_or_default(),
                        names.join(", ")
                    ));
                });
            });
        });
    }
    let updates = signal(Vec::<serde_json::Value>::new());
    let observed = server.clone();
    // The phone dialog covers the outer fixture button.
    on_key("ctrl+shift+u", move |_| {
        updates.set(observed.calls("server.updateSettings"))
    });
    view()
        .flex_1()
        .child(text(move || {
            if ready.get() {
                "Session ready"
            } else {
                "No session"
            }
        }))
        .child(
            nsh_base::button()
                .test_id("read-server")
                .on_press(move || updates.set(server.calls("server.updateSettings")))
                .child(text("Read server")),
        )
        .child(text(move || SharedString::from(theme_record.get())))
        .child(text(move || format!("Updates {}", updates.with(Vec::len))))
        .child(text(move || {
            format!(
                "Last update {}",
                updates.with(|calls| calls
                    .last()
                    .map(serde_json::Value::to_string)
                    .unwrap_or_default())
            )
        }))
        .child(untracked(|| shell::screen_for(page, &client)))
}

pub fn start_connected(window: (f32, f32), page: SettingsPage, remote: Remote) -> Headless {
    start_connected_with(window, page, remote, ReviewFixture::Normal)
}

pub fn start_connected_with(
    window: (f32, f32),
    page: SettingsPage,
    remote: Remote,
    review: ReviewFixture,
) -> Headless {
    let index = SettingsPage::ALL
        .iter()
        .position(|p| *p == page)
        .unwrap_or(0);
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Connected>(move || ConnectedProps {
            page: index,
            auto_resume: remote.auto_resume,
            policy: remote.policy,
            review: review as usize,
        }))
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("connected settings harness starts");
    app.wait_for("Session ready")
        .expect("paired with the fake environment and read its config");
    app
}
