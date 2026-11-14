//! SET-GENERAL: headless tests of Settings: general through the shell and the real client.

use t3_ui::state::nav::SettingsPage;

use crate::set_shell::start;
use crate::support::{DESKTOP, PHONE};

#[test]
fn a_switch_saves_the_device_setting_and_its_reset_restores_the_default() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, SettingsPage::General);
        app.wait_for("delete=true").expect("default record");
        assert!(
            !app.screen()
                .matches("role:button[Reset delete confirmation to default]")
                .expect("selector")
        );
        app.press("role:switch[Confirm thread deletion]")
            .expect("toggle");
        app.wait_for("delete=false")
            .expect("saved to the client record");
        app.wait_for("role:button[Reset delete confirmation to default]")
            .expect("reset appears once the value differs");
        app.press("role:button[Reset delete confirmation to default]")
            .expect("reset");
        app.wait_for("delete=true").expect("default restored");
    }
}

#[test]
fn the_page_shows_the_web_sections_and_restore_lists_what_changed() {
    let mut app = start(DESKTOP, SettingsPage::General);
    for section in ["Notifications", "Sidebar", "Confirmations", "Display"] {
        app.wait_for(section).expect(section);
    }
    for row in [
        "Thread notifications",
        "In-app notifications",
        "Project grouping",
        "Project order",
        "Working section",
        "Language",
        "Time format",
    ] {
        assert!(app.screen().shows(row), "{row}");
    }
    // Restore device defaults is disabled until something differs from the defaults.
    assert!(
        !app.screen()
            .enabled("role:button[Restore device defaults]")
            .expect("restore button")
    );
    app.press("role:switch[Confirm thread deletion]")
        .expect("toggle");
    app.wait_for("delete=false").expect("saved");
    app.wait_until("restore enabled", nsh_testing::WAIT, |screen| {
        screen
            .enabled("role:button[Restore device defaults]")
            .unwrap_or(false)
    })
    .expect("restore enabled");
}
