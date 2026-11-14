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
    // New threads defaults, with no environment to read providers from.
    for row in [
        "New threads",
        "Model",
        "Fallback model",
        "Permissions",
        "Workspace",
    ] {
        assert!(app.screen().shows(row), "{row}");
    }
    app.press("role:button[Advanced]").expect("open the fold");
    app.wait_for("Thread title instructions")
        .expect("title instructions row");
    assert!(app.screen().shows("Response streaming"));
}
