//! SET-THREADS: headless tests of Settings: threads with no connected environment.

use t3_ui::state::nav::SettingsPage;

use crate::set_shell::{Remote, start, start_connected};

/// Tall enough for the whole page: `matches` sees only views inside the viewport.
const TALL: (f32, f32) = (1280.0, 4000.0);

#[test]
fn without_an_environment_server_rows_are_inert_and_auto_settle_is_hidden() {
    let mut app = start(TALL, SettingsPage::Threads);
    app.wait_for("Usage limits").expect("threads page");
    app.wait_for("Applying settings for")
        .expect("the scope sentence");
    app.wait_for("Connect an environment to choose its text generation model.")
        .expect("text generation needs a target");
    // `hasServerTargets`: no connected environment, no Auto-settle section.
    assert!(!app.screen().shows("Auto-settle"));
    // A server row's control is inert, one element named by the web's reason.
    assert!(
        app.screen()
            .matches(
                "id:auto-resume-limited-threads >> label:This connection lacks permission to change settings on the selected environment."
            )
            .expect("selector")
    );
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
    // The snooze menu's options: the default presets, one remove button each.
    assert!(app.screen().shows("Snooze options"));
    assert!(
        app.screen()
            .matches("role:button[Add option]")
            .expect("selector")
    );
    assert!(
        app.screen()
            .matches("role:button[Remove snooze option 1]")
            .expect("selector")
    );
    app.press("role:button[Advanced]").expect("open the fold");
    app.wait_for("Thread title instructions")
        .expect("title instructions row");
    assert!(app.screen().shows("Response streaming"));
}

#[test]
fn a_connected_environment_shows_where_values_come_from_and_its_administrator_locks() {
    let mut app = start_connected(
        TALL,
        SettingsPage::Threads,
        Remote {
            auto_resume: true,
            policy: true,
        },
    );
    // `autoResumeLimitedThreads` differs from the built-in default on the environment.
    app.wait_for("id:auto-resume-limited-threads >> label:/^Set on the environment/")
        .expect("the inheritance marker names the environment");
    // The policy locks `snoozeLimitedThreads`: the lock wins over the switch.
    app.wait_for("Managed by Acme.")
        .expect("the administrator's lock");
    assert!(
        app.screen()
            .matches("id:snooze-limited-threads >> label:Managed by Acme.")
            .expect("selector")
    );
    // A press on the locked switch reaches nothing; the unlocked row still writes.
    let _ = app.press("role:switch[Snooze until limits reset]");
    app.press("role:switch[Resume after usage limits]")
        .expect("turn auto-resume off");
    app.press("id:read-server").expect("read the server");
    app.wait_for("Updates 1")
        .expect("one settings write, none from the locked row");
    app.wait_for("text:/\"autoResumeLimitedThreads\":false/")
        .expect("the patch the environment received");
}

#[test]
fn without_a_policy_values_at_the_built_in_default_say_so() {
    let mut app = start_connected(TALL, SettingsPage::Threads, Remote::default());
    app.wait_for("id:auto-resume-limited-threads >> label:/^Built-in default/")
        .expect("the marker says the value is the default");
    assert!(
        app.screen()
            .matches("role:switch[Snooze until limits reset]")
            .expect("selector")
    );
    assert!(!app.screen().shows("Managed by Acme."));
}
