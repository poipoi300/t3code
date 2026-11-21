//! Headless Appearance, Diffs and Worktrees tests through the real client.

use t3_ui::state::nav::SettingsPage;

use crate::set_shell::{Remote, start, start_connected};
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
