//! SET-CHAT: headless tests of Settings: chat through the shell and the real client.

use t3_ui::state::nav::SettingsPage;

use crate::set_shell::start;
use crate::support::{DESKTOP, PHONE};

#[test]
fn the_advanced_fold_holds_the_slash_menu_row_and_switches_save_to_the_device() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, SettingsPage::Chat);
        app.wait_for("Composer").expect("chat page");
        assert!(!app.screen().shows("Show skills in slash menu"));
        app.press("role:button[Advanced]").expect("open the fold");
        app.wait_for("Show skills in slash menu")
            .expect("advanced row");
        app.press("role:switch[Show skills in slash menu]")
            .expect("toggle");
        app.wait_for("role:button[Reset skills in slash menu to default]")
            .expect("saved and now differs from the default");
    }
}
