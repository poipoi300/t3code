//! SET-THREADS: headless tests of Settings: threads with no connected environment.

use t3_ui::state::nav::SettingsPage;

use crate::set_shell::start;
use crate::support::DESKTOP;

#[test]
fn without_an_environment_server_rows_are_inert_and_auto_settle_is_hidden() {
    let mut app = start(DESKTOP, SettingsPage::Threads);
    app.wait_for("Usage limits").expect("threads page");
    app.wait_for("Applying settings for")
        .expect("the scope sentence");
    app.wait_for("Connect an environment to choose its text generation model.")
        .expect("text generation needs a target");
    // `hasServerTargets`: no connected environment, no Auto-settle section.
    assert!(!app.screen().shows("Auto-settle"));
    // A server row's control is inert with the web's reason, never a switch that does nothing.
    assert!(!app.screen().shows("role:switch[Resume after usage limits]"));
}
