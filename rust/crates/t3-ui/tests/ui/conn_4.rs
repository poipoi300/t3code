//! CONN-4: guest entry through the production app, at desktop and phone widths.

use crate::support::{DESKTOP, PHONE, start_on_web};

fn invalid_link(window: (f32, f32)) {
    let mut app = start_on_web(window, "https://guest.example/guest");
    app.wait_for("This link has ended or is not valid")
        .expect("standalone guest notice");
    assert!(!app.screen().shows("Connect to T3 Code"));
    assert!(!app.screen().shows("Build the shared Rust UI slice"));
    app.browser()
        .type_address("#another_share.another_signature")
        .expect("paste a guest invite into the same tab");
    app.wait_for("Guest sharing is unavailable.")
        .expect("the guest page picks up the new share");
    app.browser().back().expect("previous clean guest address");
    app.wait_for("This link has ended or is not valid")
        .expect("the notice follows the browser address");
}

fn replace_invite(window: (f32, f32), path: &str) {
    let mut app = start_on_web(
        window,
        &format!("https://guest.example{path}?share=old#share_id.test_signature"),
    );
    app.wait_for("Guest sharing is unavailable.")
        .expect("guest route opens without the owner app");
    assert_eq!(
        app.browser().url(),
        Some("https://guest.example/guest?share=share_id")
    );
    assert!(
        app.browser().back().is_err(),
        "the invite did not create a history entry"
    );

    app.browser()
        .type_address("#another_share.another_signature")
        .expect("paste another guest invite into the same tab");
    assert_eq!(
        app.browser().url(),
        Some("https://guest.example/guest?share=another_share")
    );
}

#[test]
fn invalid_guest_link_on_desktop() {
    invalid_link(DESKTOP);
}

#[test]
fn invalid_guest_link_on_phone() {
    invalid_link(PHONE);
}

#[test]
fn replaces_guest_invite_on_desktop() {
    replace_invite(DESKTOP, "/Guest/");
}

#[test]
fn replaces_guest_invite_on_phone() {
    replace_invite(PHONE, "/guest");
}
