//! CONN-1: headless tests of pairing, pairing links and the saved environments on the connect
//! screen, through the whole app (demo backend by its switch) or a root that mounts the screen.

use std::rc::Rc;
use std::sync::Mutex;


use futures::FutureExt;
use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::ClientStore;
use t3_client::composition::{ClientRuntime, Spawner};
use t3_client::testing::environment::FakeEnvironment;
use t3_client::types::EnvironmentId;
use t3_contracts::orchestration_v2::OrchestrationV2CreationSource;
use t3_ui::runtime::Rt;
use t3_ui::runtime::host::{Choice, Started};
use t3_ui::screens::connect;
use t3_ui::state::nav::{Navigator, Route};
use t3_ui::theme::{self, ThemeChoice};

use crate::support::{DESKTOP, PHONE, start, start_on_web};

fn start_with_link(window: (f32, f32), link: &str) -> Headless {
    let config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<t3_ui::App>().url_schemes(["t3code"]))
    };
    Headless::start_with_link(config, link).expect("the engine starts")
}

/// A root that mounts the connect screen, in a desktop window as `support::start(DESKTOP)` uses
/// (the engine's default is a phone, whose menus open differently).
fn start_root(root: Root) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(DESKTOP.0), px(DESKTOP.1)),
        ..EngineConfig::new(root)
    };
    config.environment.os = nsh_core::platform::Os::Linux;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    Headless::start(config).expect("the engine starts")
}

#[test]
fn a_pairing_link_pasted_into_the_host_field_fills_both_fields_and_pairs() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window);
        app.wait_for("Connect to T3 Code").expect("pairing screen");
        app.press("role:button[Use a pairing code]")
            .expect("code fields");
        app.type_text(
            "label:Host",
            "https://demo.t3.codes/pair#token=demo-pairing-token",
        )
        .expect("paste a link");
        assert_eq!(
            app.screen()
                .input_text("label:Pairing code")
                .expect("code field"),
            Some("demo-pairing-token"),
            "the link's token moved into the code field"
        );
        assert_eq!(
            app.screen().input_text("label:Host").expect("host field"),
            Some("https://demo.t3.codes"),
            "the host field keeps the origin only"
        );
        app.press("role:button[Connect]").expect("connect");
        app.wait_for("Build the shared Rust UI slice")
            .expect("threads listed");
    }
}

#[test]
fn a_missing_code_is_named_before_the_client_is_asked_and_a_spent_link_leaves_the_field() {
    let mut app = start(DESKTOP);
    app.wait_for("Connect to T3 Code").expect("pairing screen");
    app.press("role:button[Use a pairing code]")
        .expect("code fields");
    app.type_text("label:Host", "box.test").expect("host");
    app.press("role:button[Connect]").expect("connect");
    app.wait_for("Enter a pairing code.")
        .expect("the form's own message");

    app.press("role:button[Use a pairing code]")
        .expect("back to the link field");
    app.type_text("label:Pairing link", "https://example.test/pair#token=")
        .expect("type");
    app.press("role:button[Connect]").expect("connect");
    app.wait_for("Pairing URL is missing its token.")
        .expect("client's message");
    assert_eq!(
        app.screen()
            .input_text("label:Pairing link")
            .expect("link field"),
        Some(""),
        "a pairing link does not stay in the field after an attempt"
    );
}

#[test]
fn a_hosted_pairing_link_pairs_at_once_and_shows_the_host() {
    let mut app = start_on_web(
        DESKTOP,
        "https://t3.example.test/pair?backend=demo&host=box.test&label=Box#token=one-time",
    );
    app.wait_for("Backend paired")
        .expect("paired from the link");
    assert!(
        app.screen().shows("box.test"),
        "the backend's host is shown"
    );
    assert!(app.screen().shows("Box is saved in this browser."));
    assert!(
        !app.screen().shows("one-time"),
        "the token is never on screen"
    );
    app.press("role:button[Open app]").expect("open the app");
    app.wait_for_gone("text:Backend paired", std::time::Duration::from_secs(5))
        .expect("left the pairing page");
}

#[test]
fn a_pairing_app_link_spends_its_token_only_after_the_origin_is_confirmed() {
    const LINK: &str = "t3code://pair?backend=demo&pairingUrl=http%3A%2F%2Fbox.test%3A3773%2Fpair%23token%3Done-time";
    let mut app = start_with_link(DESKTOP, LINK);
    app.wait_for("Add the backend at http://box.test:3773?")
        .expect("the origin, scheme included, is confirmed first");
    app.press("role:button[Cancel]").expect("decline");
    app.wait_for("Connect to T3 Code").expect("the form");
    assert!(!app.screen().shows("Environments"), "nothing was saved");

    let mut app = start_with_link(DESKTOP, LINK);
    app.wait_for("Add the backend at http://box.test:3773?")
        .expect("confirm");
    app.press("role:button[Confirm]").expect("accept");
    app.wait_for("Backend added").expect("saved");
}

/// The connect screen through its production constructor over a real client runtime paired to a
/// fake environment server ("Fake host", id `env`).
#[component]
fn PairedRoot() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
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
        spawner,
    );
    Rt::boot(
        store.clone(),
        Started {
            backend: Rc::new(client.clone()),
            choice: Choice::Client,
            paired: false,
        },
    );
    provide_context(Navigator::new(Route::Pair));
    t3_ui::ui::toast::toast_provider(
        view()
            .w_full()
            .h_full()
            .child(connect::screen_for(Some(client)))
            .child(t3_ui::ui::confirm::confirm_dialog_host()),
        t3_ui::ui::toast::Position::TopRight,
    )
}

#[test]
fn removing_a_saved_environment_asks_first_then_drops_it_from_the_list() {
    let mut app = start_root(Root::of::<PairedRoot>());
    app.wait_for("Connect to T3 Code").expect("pairing screen");
    app.type_text(
        "label:Pairing link",
        "https://server.test/pair#token=pairing-token",
    )
    .expect("type");
    app.press("role:button[Connect]").expect("connect");
    app.wait_for("Backend added").expect("paired");
    app.wait_for("Fake host")
        .expect("the saved environment is listed");

    app.press("role:button[More actions for Fake host]")
        .expect("open actions");
    app.wait_for("role:menuitem[Remove from this device…]")
        .expect("menu open");
    app.press("role:menuitem[Remove from this device…]")
        .expect("remove");
    app.wait_for("Remove Fake host from this device?")
        .expect("asked first");
    app.press("role:button[Cancel]").expect("decline");
    app.wait_for_gone("role:alertdialog", std::time::Duration::from_secs(5))
        .expect("closed");
    assert!(
        app.screen().shows("Fake host"),
        "declining keeps the environment"
    );

    app.press("role:button[More actions for Fake host]")
        .expect("open actions");
    app.wait_for("role:menuitem[Remove from this device…]")
        .expect("menu open");
    app.press("role:menuitem[Remove from this device…]")
        .expect("remove");
    app.wait_for("Remove Fake host from this device?")
        .expect("asked again");
    app.press("role:button[Confirm]").expect("confirm");
    // The client store keeps a ghost of the environment after removal (config and connection
    // state written back after it was cleared): reported to the client integrator.
    app.wait_for_gone("text:Fake host", std::time::Duration::from_secs(5))
        .expect("no longer listed");
}

/// Released by the test once the form shows that the attempt started.
static GATE: Mutex<Option<futures::channel::oneshot::Sender<()>>> = Mutex::new(None);

/// A backend whose pairing finishes only when the test releases it, so the Connect button has
/// already re-rendered to "Connecting…" when the answer arrives.
struct GatedBackend;

impl t3_ui::runtime::backend::Backend for GatedBackend {
    fn pair(
        &self,
        _input: t3_client::connection::PairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, t3_client::connection::ConnectionFailure>>
    {
        let (release, released) = futures::channel::oneshot::channel();
        *GATE.lock().expect("gate") = Some(release);
        async move {
            let _ = released.await;
            Ok(EnvironmentId("gated".into()))
        }
        .boxed_local()
    }

    fn environments(&self) -> Vec<t3_ui::runtime::backend::EnvironmentEntry> {
        Vec::new()
    }

    fn changes(&self) -> futures::stream::LocalBoxStream<'static, ()> {
        futures::StreamExt::boxed_local(futures::stream::pending::<()>())
    }

    fn open_thread(&self, _key: &t3_client::types::ThreadKey) {}

    fn close_thread(&self, _key: &t3_client::types::ThreadKey) {}

    fn send_message(
        &self,
        _key: &t3_client::types::ThreadKey,
        _text: String,
    ) -> LocalBoxFuture<'static, Result<(), t3_ui::runtime::backend::SendError>> {
        futures::future::ready(Ok(())).boxed_local()
    }
}

#[component]
fn GatedConnectRoot() -> impl IntoElement {
    theme::install(&ThemeChoice::default());
    Rt::boot(
        ClientStore::default(),
        Started {
            backend: Rc::new(GatedBackend),
            choice: Choice::Client,
            paired: false,
        },
    );
    provide_context(Navigator::new(Route::Pair));
    t3_ui::ui::toast::toast_provider(
        view().w_full().h_full().child(connect::screen()).child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Release pairing")))
                .on_press(|| {
                    if let Some(release) = GATE.lock().expect("gate").take() {
                        let _ = release.send(());
                    }
                })
                .child(text("Release pairing")),
        ),
        t3_ui::ui::toast::Position::TopRight,
    )
}

// Integration 3: rebuilding Connect as the attempt starts cleaned the owner that had spawned the
// pairing, so its answer never arrived and the form stayed on "Connecting…".
#[test]
fn a_pairing_that_answers_after_the_button_rebuilt_still_completes() {
    let mut app = start_root(Root::of::<GatedConnectRoot>());
    app.wait_for("Connect to T3 Code").expect("pairing screen");
    app.type_text("label:Pairing link", "https://box.test/pair#token=abc")
        .expect("type");
    app.press("role:button[Connect]").expect("connect");
    app.wait_for("Connecting…")
        .expect("the attempt started and Connect re-rendered");
    app.press("role:button[Release pairing]")
        .expect("answer the pairing");
    app.wait_for("Backend added")
        .expect("the pairing's answer reached the form");
    assert!(
        !app.screen()
            .shows("The environment is saved and will reconnect on app startup."),
        "a host whose saved servers do not outlive this run makes no reconnect promise"
    );
}
