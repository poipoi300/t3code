//! Labs flags use the active environment and the local preference store.
use crate::{
    set_maintenance::start_page,
    support::{DESKTOP, PHONE},
};

#[test]
fn scoped_simlock_toggle_at_desktop_and_phone_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_page(window, true, true, true);
        app.wait_for("role:switch[Simlock device hosts]")
            .expect("Labs section");
        assert!(!app.screen().shows("role:switch[Fast worktree checkout]"));
        app.press("role:switch[Simlock device hosts]")
            .expect("enable Simlock");
        app.settle();
        app.press_key("ctrl+shift+p").expect("read machine");
        app.wait_for(r#"{"simlockDeviceHostsEnabled":true}"#)
            .expect("changed the selected machine's settings");
    }
}
