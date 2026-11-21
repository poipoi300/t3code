//! Headless Appearance, Diffs and Worktrees tests through the real client.

use t3_ui::state::nav::SettingsPage;

use crate::set_shell::{Remote, ReviewFixture, start, start_connected, start_connected_with};
use crate::support::{DESKTOP, PHONE};

#[test]
fn appearance_saves_math_rendering_and_resets_it_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, SettingsPage::Appearance);
        app.wait_for("role:button[Use dark mode]")
            .expect("appearance page");
        let math = "role:switch[Render math]";
        for _ in 0..6 {
            if app.screen().matches(math).expect("math selector") {
                break;
            }
            app.scroll("id:settings-page-content", nsh_core::prelude::px(350.0))
                .expect("scroll to the interface controls");
        }
        app.wait_for(math).expect("math row");
        app.press("role:switch[Render math]").expect("toggle");
        let reset = "role:button[Reset math rendering to default]";
        app.wait_for(reset).expect("saved non-default setting");
        app.press(reset).expect("restore default");
        app.wait_for_gone(reset, std::time::Duration::from_secs(5))
            .expect("the client saved the default");
    }
}

#[test]
fn diffs_saves_whitespace_and_resets_it_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, SettingsPage::Diffs);
        let toggle = "role:switch[Hide whitespace changes by default]";
        app.wait_for(toggle).expect("diff whitespace row");
        app.press(toggle).expect("toggle");
        let reset = "role:button[Reset diff whitespace changes to default]";
        app.wait_for(reset).expect("saved non-default setting");
        app.press(reset).expect("restore default");
        app.wait_for_gone(reset, std::time::Duration::from_secs(5))
            .expect("the client saved the default");
    }
}

#[test]
fn worktrees_saves_start_from_remote_on_the_environment_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_connected(window, SettingsPage::Worktrees, Remote::default());
        let toggle = "role:switch[Start new worktrees from remote by default]";
        app.wait_for(toggle).expect("worktree creation row");
        app.press(toggle).expect("toggle");
        app.press_key("ctrl+shift+u")
            .expect("read fake server calls");
        app.wait_for("Updates 1").expect("one server update");
        app.wait_for(r#"text:/"newWorktreesStartFromOrigin":true/"#)
            .expect("patch written to the environment");
    }
}

#[test]
fn diffs_saves_the_split_layout_at_both_sizes() {
    for window in [DESKTOP, PHONE] {
        let mut app = start(window, SettingsPage::Diffs);
        app.press("role:combobox[Diff layout]")
            .expect("layout picker");
        app.wait_for("role:option[Split]")
            .expect("side-by-side option");
        app.press("role:option[Split]").expect("pick split");
        app.wait_for("role:button[Reset diff layout to default]")
            .expect("layout persisted");
    }
}

fn scroll_to(app: &mut nsh_testing::Headless, selector: &str) {
    for _ in 0..15 {
        if app.screen().matches(selector).expect("selector") {
            break;
        }
        app.scroll("id:settings-page-content", nsh_core::prelude::px(250.0))
            .expect("scroll settings");
    }
    app.wait_for(selector).expect("settings control");
}

fn cleanup_fixture(window: (f32, f32)) -> nsh_testing::Headless {
    start_connected_with(
        window,
        SettingsPage::Worktrees,
        Remote::default(),
        ReviewFixture::WorktreeCleanup,
    )
}

#[test]
fn review_appearance_ignores_a_device_preference_loaded_after_a_local_choice() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_connected_with(
            window,
            SettingsPage::Appearance,
            Remote::default(),
            ReviewFixture::SlowAppearance,
        );
        let dark = "role:button[Use dark mode]";
        app.press(dark)
            .expect("choose dark while the saved mode is loading");
        app.press_key("ctrl+shift+e")
            .expect("finish loading the old light mode");
        app.wait_until(
            "dark choice survives the delayed read",
            nsh_testing::WAIT,
            |screen| screen.selected(dark) == Ok(true),
        )
        .expect("dark mode");
    }
}

#[test]
fn review_thread_details_menu_disables_reset_at_default_and_restores_changed_items() {
    for window in [DESKTOP, PHONE] {
        let mut app = start_connected(window, SettingsPage::Appearance, Remote::default());
        let button = "role:button[/^Thread details:/]";
        scroll_to(&mut app, button);
        app.press(button).expect("open thread details");
        let reset = "role:menuitem[Reset to default]";
        app.wait_for(reset).expect("reset in the menu");
        assert!(
            !app.screen()
                .enabled(reset)
                .expect("reset disabled at default")
        );
        app.press("role:menuitemcheckbox[Environment and worktree]")
            .expect("hide item");
        assert!(
            app.screen()
                .enabled(reset)
                .expect("reset enabled after change")
        );
        app.press(reset).expect("restore default");
        app.wait_for("role:button[Thread details: 10 of 12 on]")
            .expect("default items");
    }
}

#[test]
fn review_worktree_retention_hides_before_the_server_replies_and_restores_on_failure() {
    use nsh_core::Checked;
    for window in [DESKTOP, PHONE] {
        let mut app = cleanup_fixture(window);
        let toggle = "role:switch[Delete inactive worktrees]";
        scroll_to(&mut app, toggle);
        assert_eq!(
            app.screen()
                .accessibility_state(toggle)
                .expect("retention")
                .checked,
            Some(Checked::True)
        );
        app.press(toggle)
            .expect("switch off while the update is in flight");
        assert_eq!(
            app.screen()
                .accessibility_state(toggle)
                .expect("retention")
                .checked,
            Some(Checked::False)
        );
        app.press_key("ctrl+shift+u").expect("read pending RPC");
        app.wait_for(r#"text:/"worktreeAfterDays":null/"#)
            .expect("sent null");
        app.press_key("ctrl+shift+e")
            .expect("server rejects update");
        app.wait_until(
            "server rejection restores the value",
            nsh_testing::WAIT,
            |screen| {
                screen
                    .accessibility_state(toggle)
                    .is_ok_and(|state| state.checked == Some(Checked::True))
            },
        )
        .expect("retention restored on failure");
    }
}

#[test]
fn review_worktree_keep_when_has_no_selected_option_when_mixed() {
    for window in [DESKTOP, PHONE] {
        let mut app = cleanup_fixture(window);
        let toggle = "role:switch[Delete inactive worktrees]";
        scroll_to(&mut app, toggle);
        let advanced = "id:storage-worktrees >> text:Advanced";
        app.scroll("id:settings-page-content", nsh_core::prelude::px(240.0))
            .expect("move to cleanup advanced");
        app.press(advanced).expect("open cleanup advanced");
        let keep = "role:combobox[Keep worktrees with local changes]";
        scroll_to(&mut app, keep);
        app.press(keep).expect("mixed value can be changed");
        for option in [
            "Any local files",
            "Uncommitted changes",
            "Edited tracked files",
        ] {
            assert_eq!(
                app.screen().selected(&format!("role:option[{option}]")),
                Ok(false)
            );
        }
    }
}

#[test]
fn review_settled_worktree_retention_warns_when_a_selected_machine_is_offline() {
    for window in [DESKTOP, PHONE] {
        let mut app = cleanup_fixture(window);
        scroll_to(&mut app, "role:switch[Delete inactive worktrees]");
        app.scroll("id:settings-page-content", nsh_core::prelude::px(240.0))
            .expect("move to cleanup advanced");
        app.press("id:storage-worktrees >> text:Advanced")
            .expect("open cleanup advanced");
        scroll_to(&mut app, "role:switch[Delete settled worktrees]");
        app.press_key("ctrl+shift+o")
            .expect("take a machine offline");
        app.wait_for("Offline machines will not get this setting")
            .expect("settled rule warns about the offline machine");
    }
}
