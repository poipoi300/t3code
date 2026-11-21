//! GUEST-2: the owner's "Share with guest" dialog at desktop and phone widths: create, copy,
//! the replace notice, and a refusal. Bound to a scripted creator; the client service behind it is
//! covered by t3-client's `guest_owner` tests.

use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_contracts::base_schemas::ThreadId;
use t3_ui::screens::guest_share_dialog::{
    GuestShareCreator, ReplacesShare, guest_share_dialog_host, open_guest_share_dialog,
};

use crate::support::{DESKTOP, PHONE};

const URL: &str = "https://box.example/guest#share1.signature";

fn key() -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId("env".into()),
        thread: ThreadId("thread-a".into()),
    }
}

fn root(replacing: bool, refuse: bool) -> AnyElement {
    t3_ui::theme::install(&Default::default());
    provide_context(GuestShareCreator(Rc::new(move |_| {
        Box::pin(async move {
            if refuse {
                Err("Guest links are off. Turn them on in Settings > Labs.".to_owned())
            } else {
                Ok(URL.to_owned())
            }
        })
    })));
    provide_context(ReplacesShare(Rc::new(move |_| replacing)));
    view()
        .w_full()
        .h_full()
        .child(
            nsh_base::button()
                .accessibility_label(Some(SharedString::from("Open guest dialog")))
                .on_press(|| {
                    open_guest_share_dialog(key());
                })
                .child(text("Open guest dialog")),
        )
        .child(guest_share_dialog_host())
        .into_element()
}

#[component]
fn First() -> impl IntoElement {
    root(false, false)
}

#[component]
fn Replacing() -> impl IntoElement {
    root(true, false)
}

#[component]
fn Refusing() -> impl IntoElement {
    root(false, true)
}

fn start(window: (f32, f32), root: Root) -> Headless {
    let mut config = EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(root)
    };
    if window == DESKTOP {
        config.environment.os = nsh_core::platform::Os::Linux;
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    }
    Headless::start(config).expect("the engine starts")
}

fn create_and_copy(window: (f32, f32)) {
    let mut app = start(window, Root::of::<First>());
    app.wait_for("Open guest dialog").expect("the harness");
    app.press("role:button[Open guest dialog]").expect("open");
    app.wait_for("Share with guest").expect("the dialog");
    app.wait_for("The link works once and expires in 24 hours.")
        .expect("first link notice");
    app.press("role:button[Create link]").expect("create");
    app.wait_for("label:Guest link").expect("the link field");
    assert!(!app.screen().shows("Create link"));
    app.press("role:button[Copy]").expect("copy");
    app.wait_for("Copied").expect("copy confirmation");
    app.press("role:button[Done]").expect("done");
    app.wait_for_gone("Share with guest", std::time::Duration::from_secs(5))
        .expect("closed");
}

fn replace_notice(window: (f32, f32)) {
    let mut app = start(window, Root::of::<Replacing>());
    app.wait_for("Open guest dialog").expect("the harness");
    app.press("role:button[Open guest dialog]").expect("open");
    app.wait_for("A new link ends the current one.")
        .expect("replace notice");
}

fn refusal(window: (f32, f32)) {
    let mut app = start(window, Root::of::<Refusing>());
    app.wait_for("Open guest dialog").expect("the harness");
    app.press("role:button[Open guest dialog]").expect("open");
    app.press("role:button[Create link]").expect("create");
    app.wait_for("Guest links are off. Turn them on in Settings > Labs.")
        .expect("the refusal");
    assert!(app.screen().shows("Create link"), "the owner can try again");
    assert!(!app.screen().shows("Copy"));
}

#[test]
fn create_and_copy_on_desktop() {
    create_and_copy(DESKTOP);
}

#[test]
fn create_and_copy_on_phone() {
    create_and_copy(PHONE);
}

#[test]
fn a_new_link_warns_that_it_ends_the_current_one_at_both_widths() {
    replace_notice(DESKTOP);
    replace_notice(PHONE);
}

#[test]
fn a_refusal_is_shown_and_can_be_retried_at_both_widths() {
    refusal(DESKTOP);
    refusal(PHONE);
}
