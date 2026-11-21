//! SB-SNOOZE: headless tests of Snooze menu and presets. Start the app with `crate::support`.

use crate::support::{DESKTOP, PHONE};

#[test]
fn live_sidebar_uses_a_configured_snooze_preset_desktop() {
    uses_a_configured_snooze_preset(DESKTOP);
}

#[test]
fn live_sidebar_uses_a_configured_snooze_preset_phone() {
    uses_a_configured_snooze_preset(PHONE);
}

fn uses_a_configured_snooze_preset(window: (f32, f32)) {
    use super::sb_actions_1::{assert_commands, open_row_menu, start_live_sidebar};

    let mut app = start_live_sidebar(window);
    open_row_menu(&mut app, window, "Alpha thread");
    app.press("role:menuitem[Snooze]")
        .expect("open snooze submenu");
    app.press("role:menuitem[In 17 minutes]")
        .expect("choose snooze preset");
    assert_commands(&mut app, "thread.snooze:alpha");
    super::sb_actions_1::wait_for_undo_summary(&mut app, "Snoozed,");
}

#[test]
fn live_sidebar_custom_snooze_desktop() {
    custom_snooze(DESKTOP);
}

#[test]
fn live_sidebar_custom_snooze_phone() {
    custom_snooze(PHONE);
}

fn custom_snooze(window: (f32, f32)) {
    use super::sb_actions_1::{assert_commands, open_row_menu, start_live_sidebar};

    let mut app = start_live_sidebar(window);
    open_row_menu(&mut app, window, "Alpha thread");
    app.press("role:menuitem[Snooze]")
        .expect("open snooze submenu");
    app.press("role:menuitem[Custom…]")
        .expect("open custom snooze");
    app.wait_for("Custom snooze").expect("dialog is mounted");
    app.press("role:button[Duration]")
        .expect("choose a duration");
    app.clear("label:Snooze for").expect("replace duration");
    app.type_text("label:Snooze for", "0")
        .expect("enter invalid duration");
    app.press("role:button[Snooze]").expect("try to snooze");
    app.wait_for("Enter a positive duration.")
        .expect("invalid duration is rejected");
    app.clear("label:Snooze for")
        .expect("replace invalid duration");
    app.type_text("label:Snooze for", "2.5")
        .expect("enter duration");
    app.press("role:button[Snooze]")
        .expect("snooze for the custom duration");
    assert_commands(&mut app, "thread.snooze:alpha");
}
