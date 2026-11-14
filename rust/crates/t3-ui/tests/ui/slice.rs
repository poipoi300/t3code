//! The first vertical slice: pair, list threads from the store, open one, send, see the reply.

use crate::support::{DESKTOP, PHONE, pair, start, start_on_web, start_without_switch};

const REPLY_TAIL: &str = "The same code ran on desktop, web and Android.";

#[test]
fn on_a_phone_the_list_is_a_page_and_a_thread_is_pushed_over_it() {
    let mut app = start(PHONE);
    pair(&mut app);
    assert!(app.screen().shows("Rich composer: chips, IME and paste"));
    assert!(!app.screen().shows("Select a thread"));
    app.press("role:button[Build the shared Rust UI slice]")
        .expect("open thread");
    app.wait_for("The project has a single math module. I will add a clamp helper beside sum.")
        .expect("timeline shows the conversation");
    app.press("role:button[Back]").expect("back");
    app.wait_for("Rich composer: chips, IME and paste")
        .expect("back at the list");
}

#[test]
fn on_a_desktop_the_sidebar_stays_beside_the_thread_and_a_sent_message_streams_a_reply() {
    let mut app = start(DESKTOP);
    pair(&mut app);
    app.wait_for("Select a thread")
        .expect("landing beside the sidebar");
    app.press("role:button[Build the shared Rust UI slice]")
        .expect("open thread");
    app.wait_for("The project has a single math module. I will add a clamp helper beside sum.")
        .expect("timeline");
    assert!(
        app.screen().shows("Rich composer: chips, IME and paste"),
        "sidebar still shown"
    );
    app.type_text("id:composer", "Ship it everywhere")
        .expect("type");
    app.press("role:button[Send message]").expect("send");
    app.wait_for("Ship it everywhere")
        .expect("user message in the timeline");
    app.wait_for(REPLY_TAIL).expect("streamed reply completes");
}

#[test]
fn a_malformed_pairing_link_shows_the_clients_reason_and_stays_on_the_pairing_screen() {
    let mut app = start(DESKTOP);
    app.wait_for("Connect to T3 Code").expect("pairing screen");
    app.type_text("label:Pairing link", "https://example.test/pair")
        .expect("type");
    app.press("role:button[Connect]").expect("connect");
    app.wait_for("Pairing URL is missing its token.")
        .expect("client's message shown");
    assert!(app.screen().shows("Connect to T3 Code"));
}

#[test]
fn on_the_web_the_slice_starts_pairs_and_opens_a_thread() {
    let mut app = start_on_web(DESKTOP, "https://t3.example.test/?backend=demo");
    pair(&mut app);
    app.press("role:button[Build the shared Rust UI slice]")
        .expect("open thread");
    app.wait_for("The project has a single math module. I will add a clamp helper beside sum.")
        .expect("timeline");
}

#[test]
fn without_the_demo_switch_the_app_runs_the_real_client() {
    let mut app = start_without_switch(DESKTOP);
    app.wait_for("Connect to T3 Code").expect("pairing screen");
    assert!(
        !app.screen().shows("Fill in the demo server"),
        "the demo server is offered only by the demo backend"
    );
    app.type_text("label:Pairing link", "https://example.test/pair#token=abc")
        .expect("type");
    app.press("role:button[Connect]").expect("connect");
    app.wait_for("This host keeps no app data, so the client cannot save servers.")
        .expect("the real client's start-up failure, not the demo's pairing");
}
