//! Labs flags use the active environment and the local preference store.
use crate::{
    set_maintenance::{Gate, has_control, start_gate, start_page},
    support::{DESKTOP, PHONE},
};

#[test]
fn scoped_simlock_toggle_at_desktop_and_phone_widths() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_page(window, true, true, true);
        app.wait_for("role:switch[Simlock device hosts]")
            .expect("Labs section");
        assert!(!has_control(&app, "role:switch[Fast worktree checkout]"));
        app.press("role:switch[Simlock device hosts]")
            .expect("enable Simlock");
        app.settle();
        app.press_key("ctrl+shift+p").expect("read machine");
        app.wait_for(r#"{"simlockDeviceHostsEnabled":true}"#)
            .expect("changed the selected machine's settings");
    }
}

#[test]
fn sidebar_preferences_stay_on_the_device() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_page(window, true, true, true);
        let sidebar = "role:switch[Boring sidebar (group by project)]";
        let multiline = "role:switch[Two-line folder rows]";
        app.wait_for(sidebar).expect("sidebar preference");
        assert!(!has_control(&app, multiline));
        app.press(sidebar).expect("enable local sidebar");
        app.wait_for(multiline)
            .expect("multiline preference appears");
        app.press(multiline).expect("enable local multiline rows");
        app.settle();
        app.press_key("ctrl+shift+b")
            .expect("read client preferences");
        app.wait_for("Local sidebar=true multiline=true")
            .expect("both values saved locally");
        app.press_key("ctrl+shift+p")
            .expect("read all server writes");
        app.wait_for("No settings writes")
            .expect("both writes stayed local");
        app.press(sidebar).expect("disable sidebar");
        app.wait_for_gone(multiline, std::time::Duration::from_secs(5))
            .expect("multiline row hidden");
    }
}

#[test]
fn supported_fast_checkout_and_signed_out_guest_links() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_gate(window, true, true, true, Gate::RelayAndFastCheckout);
        app.wait_for("role:switch[Fast worktree checkout]")
            .expect("capable environment");
        app.wait_for("Share a thread with a guest who watches it live in a browser and sends messages you approve. Sign in to T3 Connect to use guest links.").expect("signed out guidance");
        assert!(has_control(&app, "role:button[Sharing & access]"));
        assert!(!has_control(&app, "role:switch[Guest links]"));
    }
}
