//! SET-SHELL: headless tests of the settings shell (navigation, scope, breadcrumb), mounted as the
//! router mounts it, over a real `ClientRuntime` on the fake platform. The harness is shared by the
//! other settings test modules.

use std::rc::Rc;
use std::time::SystemTime;

use futures::StreamExt;
use nsh_core::prelude::*;
use nsh_core::time::Clock;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::ClientStore;
use t3_client::composition::ClientRuntime;
use t3_client::testing::FakePlatform;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::screens::settings::shell;
use t3_ui::state::nav::{Navigator, Route, SettingsPage};
use t3_ui::theme;

use crate::support::{DESKTOP, PHONE};

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    page: usize,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let nav = Navigator::new(Route::Settings(SettingsPage::ALL[props.page]));
    provide_context(nav);
    let client = ClientRuntime::new(
        FakePlatform::default(),
        ClientStore::default(),
        OrchestrationV2CreationSource::Web,
        Rc::new(|future| {
            spawn(future);
        }),
    );
    provide_context(t3_ui::runtime::services::settings::settings(&client));
    // What the client's record holds, so tests assert on the store and not only the screen.
    let settings = client.settings();
    let record = signal(Vec::<String>::new());
    let read = settings.clone();
    let probe = move || {
        let s = read.client_settings();
        vec![
            format!("delete={}", s.confirm_thread_delete),
            format!("language={}", s.language),
            format!(
                "send={}",
                serde_json::to_value(&s.send_shortcut)
                    .unwrap_or_default()
                    .as_str()
                    .unwrap_or_default()
            ),
        ]
    };
    record.set(probe());
    let mut changes = settings.changes();
    spawn(async move {
        while changes.next().await.is_some() {
            let _ = record.try_set(probe());
        }
    });
    view()
        .flex_1()
        .child(text(move || {
            SharedString::from(record.with(|r| r.first().cloned().unwrap_or_default()))
        }))
        .child(text(move || {
            SharedString::from(record.with(|r| r.get(1).cloned().unwrap_or_default()))
        }))
        .child(text(move || {
            SharedString::from(record.with(|r| r.get(2).cloned().unwrap_or_default()))
        }))
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
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps { page: index }))
    };
    config.environment.idiom = if window == PHONE {
        nsh_core::platform::Idiom::Phone
    } else {
        nsh_core::platform::Idiom::Desktop
    };
    let mut app = Headless::start(config).expect("settings harness starts");
    app.wait_for("delete=true").expect("harness mounted");
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
    assert!(!app.screen().shows("role:button[Secrets]"));
    app.press("role:button[Chat]").expect("chat section");
    app.wait_for("Route chat").expect("navigated");
    app.wait_for("Follow-ups")
        .expect("chat page renders inside the shell");
    // The breadcrumb: "Settings / Chat".
    assert!(app.screen().shows("Settings"));
    assert!(app.screen().shows("/"));
    // Restore device defaults belongs to General only.
    assert!(!app.screen().shows("role:button[Restore device defaults]"));
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
