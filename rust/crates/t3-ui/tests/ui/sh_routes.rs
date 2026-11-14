//! SH-ROUTES at the client URL-service seam; App wiring belongs to the integrator.

use std::{cell::RefCell, rc::Rc};

use futures::{
    FutureExt, StreamExt, channel::mpsc, future::LocalBoxFuture, stream::LocalBoxStream,
};
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::view_logic::shell::navigation::Direction;
use t3_ui::{
    i18n::Text,
    state::{
        nav::{Navigator, Route, SettingsPage},
        url::{self, Location, RoutePolicy, UrlClient, UrlRequest, UrlSnapshot, UrlState},
    },
    theme,
};

use crate::support::{DESKTOP, PHONE};

fn snapshot(path: &str, index: u64) -> UrlSnapshot {
    UrlSnapshot {
        path: path.into(),
        history_index: index,
        entry_key: None,
        can_go_back: index > 0,
        can_go_forward: false,
        policy: Some(RoutePolicy {
            authenticated_or_hosted: true,
            ..Default::default()
        }),
    }
}

struct TestUrls {
    initial: String,
    reject: bool,
    updates: mpsc::UnboundedSender<UrlSnapshot>,
    receiver: RefCell<Option<mpsc::UnboundedReceiver<UrlSnapshot>>>,
    requests: Signal<Vec<UrlRequest>>,
}

impl UrlClient for TestUrls {
    fn stream_error(&self) -> Option<Text> {
        self.reject
            .then(|| Text::content("URL history permission denied"))
    }
    fn watch(&self) -> LocalBoxStream<'static, UrlSnapshot> {
        futures::stream::once(futures::future::ready(snapshot(&self.initial, 0)))
            .chain(self.receiver.borrow_mut().take().expect("one URL lease"))
            .boxed_local()
    }

    fn navigate(&self, request: UrlRequest) -> LocalBoxFuture<'static, Result<(), Text>> {
        self.requests
            .update(|requests| requests.push(request.clone()));
        if self.reject {
            return futures::future::ready(Err(Text::content("Rejected by URL host")))
                .boxed_local();
        }
        // A host acknowledgement, never a direct change to the navigator under test.
        if let UrlRequest::Push(path) | UrlRequest::Replace(path) = request {
            self.updates
                .unbounded_send(snapshot(&path, 1))
                .expect("live host stream");
        }
        futures::future::ready(Ok(())).boxed_local()
    }
}

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {
    initial: String,
    reject: bool,
}

#[component]
fn Harness(props: HarnessProps) -> impl IntoElement {
    theme::install(&theme::ThemeChoice::default());
    let location = Location::parse(&props.initial);
    let nav = Navigator::new(location.route.clone());
    provide_context(nav);
    let requests = signal(Vec::new());
    let (updates, receiver) = mpsc::unbounded();
    let host = updates.clone();
    let close_host = updates.clone();
    let urls = UrlState::install(
        location,
        nav,
        Rc::new(TestUrls {
            initial: props.initial.clone(),
            reject: props.reject,
            updates,
            receiver: RefCell::new(Some(receiver)),
            requests,
        }),
    );
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some("Warm Settings link".into()))
                .on_press(move || {
                    host.unbounded_send(snapshot("/settings/appearance?machine=env#colors", 2))
                        .expect("live host");
                })
                .child(text("Warm Settings link")),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Close URL host".into()))
                .on_press(move || close_host.close_channel())
                .child(text("Close URL host")),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Open usage".into()))
                .on_press(move || nav.open(Route::Usage))
                .child(text("Open usage")),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Back".into()))
                .on_press(move || {
                    urls.go(Direction::Back);
                })
                .child(text("Back")),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Forward".into()))
                .on_press(move || {
                    urls.go(Direction::Forward);
                })
                .child(text("Forward")),
        )
        .child(
            nsh_base::button()
                .accessibility_label(Some("Native back".into()))
                .on_press(move || nav.back())
                .child(text("Native back")),
        )
        .child(text(move || {
            format!("Location {}", urls.location.with(Location::href))
        }))
        .child(text(move || format!("Selected {:?}", nav.top())))
        .child(text(move || format!("Requests {:?}", requests.get())))
        .child(text(move || {
            urls.error
                .get()
                .map(|error| error.resolve())
                .unwrap_or_default()
        }))
        .child(dynamic(move || match nav.top() {
            Route::Settings(_) | Route::NotFound(_) => Some(untracked(|| url::screen(&nav.top()))),
            _ => None,
        }))
}

fn start(window: (f32, f32), initial: &'static str, reject: bool) -> Headless {
    let initial = initial.to_owned();
    Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::new::<Harness>(move || HarnessProps {
            initial: initial.clone(),
            reject,
        }))
    })
    .expect("URL-service fixture")
}

#[test]
fn unbuilt_settings_use_the_web_not_found_action_and_replace_home_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "/settings/general", false);
        app.wait_for("Page not found").expect("registered fallback");
        app.press("role:button[Go home]")
            .expect("web fallback action");
        app.wait_for("Selected Home")
            .expect("host resolution selects home");
        assert!(app.screen().shows("Requests [Replace(\"/\")]"));
        assert!(app.screen().shows("Location /"));
        assert!(!app.screen().shows("Page not found"));
    }
}

#[test]
fn a_warm_settings_link_selects_its_page_and_keeps_its_search_and_hash() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, "/env/thread", false);
        app.wait_for("Location /env/thread")
            .expect("initial host snapshot");
        app.press("role:button[Warm Settings link]")
            .expect("incoming host event");
        app.wait_for(&format!(
            "Selected {:?}",
            Route::Settings(SettingsPage::Appearance)
        ))
        .expect("Settings selected");
        assert!(
            app.screen()
                .shows("Location /settings/appearance?machine=env#colors")
        );
        assert!(
            app.screen().shows("Requests []"),
            "a host event must not echo a new push"
        );
    }
}

#[test]
fn failed_navigation_retains_the_resolved_page_and_exposes_the_client_error() {
    let mut app = start(DESKTOP, "/env/thread", true);
    app.wait_for("Location /env/thread").expect("initial page");
    app.press("role:button[Open usage]")
        .expect("local navigation");
    app.wait_for("Rejected by URL host")
        .expect("failed request settles");
    assert!(app.screen().shows("Location /env/thread"));
    let expected = Route::Thread(t3_client::types::ThreadKey {
        environment: t3_client::types::EnvironmentId("env".into()),
        thread: t3_client::types::ThreadId("thread".into()),
    });
    assert!(
        app.screen().shows(&format!("Selected {expected:?}")),
        "{:?}",
        app.screen().texts()
    );
    assert!(app.screen().shows("Requests [Push(\"/usage\")]"));
}

#[test]
fn unavailable_history_directions_do_not_dispatch_and_native_back_returns_to_threads() {
    let mut app = start(PHONE, "/usage", false);
    app.wait_for("Selected Usage").expect("usage selected");
    app.press("role:button[Back]").expect("back unavailable");
    app.press("role:button[Forward]")
        .expect("forward unavailable");
    assert!(app.screen().shows("Requests []"));
    app.press("role:button[Native back]")
        .expect("system stack back");
    app.wait_for("Selected Home")
        .expect("returns to Threads instead of leaving");
    assert!(app.screen().shows("Requests [Push(\"/\")]"));
}

#[test]
fn a_closed_url_service_disables_navigation_instead_of_claiming_success() {
    for (reject, reason) in [
        (false, "resource closed"),
        (true, "URL history permission denied"),
    ] {
        let mut app = start(DESKTOP, "/settings/general", reject);
        app.wait_for("Page not found").expect("fallback");
        app.press("role:button[Close URL host]")
            .expect("capability closes");
        app.wait_for(reason).expect("client closure reason");
        let _ = app.press("role:button[Go home]");
        app.press("role:button[Open usage]")
            .expect("local route request");
        assert!(app.screen().shows("Location /settings/general"));
        assert!(app.screen().shows("Selected Settings(General)"));
        assert!(app.screen().shows("Requests []"));
    }
}
