//! Sharing & access through a paired runtime with no access-management grant.
use crate::set_shell::{Remote, start_connected};
use crate::support::{DESKTOP, PHONE};
use t3_ui::state::nav::SettingsPage;

fn narrow_session(window: (f32, f32)) {
    let mut app = start_connected(window, SettingsPage::Access, Remote::default());
    app.press("role:button[Permissions]")
        .expect("session permissions");
    app.wait_for("These permissions apply to this client’s current connection.")
        .expect("selected environment session");
    app.wait_for("Change environment settings")
        .expect("granted scope");
    assert!(!app.screen().shows("Authorized clients"));
    assert!(!app.screen().shows("Revoke"));
}

#[test]
fn session_permissions_desktop() {
    narrow_session(DESKTOP);
}

#[test]
fn session_permissions_phone() {
    narrow_session(PHONE);
}
