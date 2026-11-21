//! Port of apps/web/src/threadSelectionStore.test.ts against the shared reactive store.

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;
use t3_ui::screens::sidebar::selection::{
    get_thread_keys_to_deselect_after_delete, use_thread_selection_store,
};

use crate::support::{DESKTOP, PHONE};

fn ordered() -> Vec<String> {
    ["a", "b", "c", "d", "e"].map(String::from).into()
}

#[component]
fn SelectionReader() -> impl IntoElement {
    let selection = use_thread_selection_store();
    text(move || {
        let state = selection.state();
        SharedString::from(format!(
            "selected={:?};anchor={:?}",
            state.selected, state.anchor
        ))
    })
}

#[component]
fn SelectionFixture() -> impl IntoElement {
    let selection = use_thread_selection_store();
    let show_reader = signal(true);
    let captured = stored(Vec::<String>::new());
    let publications = signal(0usize);
    effect(move || {
        selection.state();
        publications.update(|count| *count += 1);
    });
    let mut controls = view().w_full().h_full();
    for key in ordered() {
        let toggle = key.clone();
        let range = key.clone();
        controls = controls
            .child(
                nsh_base::button()
                    .accessibility_label(format!("Toggle {key}"))
                    .child(text(format!("Toggle {key}")))
                    .on_press(move || selection.toggle_thread(&toggle)),
            )
            .child(
                nsh_base::button()
                    .accessibility_label(format!("Range {key}"))
                    .child(text(format!("Range {key}")))
                    .on_press(move || selection.range_select_to(&range, &ordered())),
            );
    }
    controls
        .child(
            nsh_base::button()
                .accessibility_label("Anchor b")
                .child(text("Anchor b"))
                .on_press(move || selection.set_anchor("b")),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Missing anchor range")
                .child(text("Missing anchor range"))
                .on_press(move || {
                    selection.range_select_to("c", &["b".into(), "c".into(), "d".into()])
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Capture deletion")
                .child(text("Capture deletion"))
                .on_press(move || {
                    captured.replace(selection.selected_thread_keys());
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Delete a and missing c")
                .child(text("Delete a and missing c"))
                .on_press(move || {
                    let removed = get_thread_keys_to_deselect_after_delete(
                        &captured.get(),
                        &["a".into()].into(),
                        |key| key == "a" || key == "b",
                    );
                    selection.remove_from_selection(&removed);
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Delete only a")
                .child(text("Delete only a"))
                .on_press(move || {
                    let removed = get_thread_keys_to_deselect_after_delete(
                        &captured.get(),
                        &["a".into()].into(),
                        |_| true,
                    );
                    selection.remove_from_selection(&removed);
                }),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Clear")
                .child(text("Clear"))
                .on_press(move || selection.clear_selection()),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Remove none")
                .child(text("Remove none"))
                .on_press(move || selection.remove_from_selection(&["missing".into()])),
        )
        .child(
            nsh_base::button()
                .accessibility_label("Remount reader")
                .child(text("Remount reader"))
                .on_press(move || show_reader.update(|show| *show = !*show)),
        )
        .child(when(move || show_reader.get(), selection_reader))
        .child(text(move || {
            SharedString::from(format!("has selection={}", selection.has_selection()))
        }))
        .child(text(move || {
            SharedString::from(format!("publications={}", publications.get()))
        }))
}

fn fixture(window: (f32, f32)) -> Headless {
    let mut app = Headless::start(EngineConfig {
        ui_sets: t3_ui::registries(),
        viewport: size(px(window.0), px(window.1)),
        ..EngineConfig::new(Root::of::<SelectionFixture>())
    })
    .expect("selection fixture starts");
    app.wait_for("role:button[Toggle a]")
        .expect("selection controls mount");
    app
}

fn press(app: &mut Headless, actions: &[&str]) {
    for action in actions {
        app.press(&format!("role:button[{action}]"))
            .expect("store action");
    }
}

fn expect_state(app: &mut Headless, selected: &[&str], anchor: Option<&str>) {
    app.wait_for(&format!("selected={selected:?};anchor={anchor:?}"))
        .expect("client selection publishes");
}

#[test]
fn bulk_delete_removes_completed_and_missing_rows_but_retains_a_failed_thread() {
    let mut app = fixture(DESKTOP);
    press(
        &mut app,
        &[
            "Toggle a",
            "Toggle b",
            "Toggle c",
            "Capture deletion",
            "Delete a and missing c",
        ],
    );
    expect_state(&mut app, &["b"], None);
}

#[test]
fn bulk_delete_retains_unprocessed_hidden_live_rows_and_new_selections() {
    let mut app = fixture(DESKTOP);
    press(
        &mut app,
        &[
            "Toggle a",
            "Toggle b",
            "Toggle c",
            "Capture deletion",
            "Toggle d",
            "Delete only a",
        ],
    );
    expect_state(&mut app, &["b", "c", "d"], Some("d"));
}

#[test]
fn a_plain_click_anchor_enables_range_selection() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        press(&mut app, &["Anchor b", "Range d"]);
        expect_state(&mut app, &["b", "c", "d"], Some("b"));
    }
}

#[test]
fn successive_ranges_keep_the_original_anchor() {
    let mut app = fixture(DESKTOP);
    press(&mut app, &["Toggle b", "Range d", "Range e"]);
    expect_state(&mut app, &["b", "c", "d", "e"], Some("b"));
}

#[test]
fn an_anchor_outside_the_ordered_scope_falls_back_to_the_target() {
    let mut app = fixture(DESKTOP);
    press(&mut app, &["Toggle a", "Missing anchor range"]);
    expect_state(&mut app, &["a", "c"], Some("c"));
}

#[test]
fn ranges_preserve_previous_selections_outside_the_range() {
    let mut app = fixture(DESKTOP);
    press(&mut app, &["Toggle a", "Toggle b", "Range d"]);
    expect_state(&mut app, &["a", "b", "c", "d"], Some("b"));
}

#[test]
fn the_global_store_survives_reader_remount_and_clear_resets_the_anchor() {
    for window in [DESKTOP, PHONE] {
        let mut app = fixture(window);
        press(
            &mut app,
            &["Toggle a", "Remount reader", "Toggle b", "Remount reader"],
        );
        expect_state(&mut app, &["a", "b"], Some("b"));
        press(&mut app, &["Toggle b"]);
        expect_state(&mut app, &["a"], Some("b"));
        press(&mut app, &["Clear"]);
        expect_state(&mut app, &[], None);
        app.wait_for("has selection=false")
            .expect("reverse state publishes");
    }
}

#[test]
fn unchanged_clear_anchor_and_removal_do_not_publish() {
    let mut app = fixture(DESKTOP);
    app.wait_for("publications=1").expect("initial observation");
    press(&mut app, &["Clear", "Remove none"]);
    assert!(app.screen().shows("publications=1"));
    press(&mut app, &["Anchor b"]);
    app.wait_for("publications=2")
        .expect("new anchor publishes");
    press(&mut app, &["Anchor b", "Remove none"]);
    assert!(app.screen().shows("publications=2"));
}

#[test]
fn live_sidebar_selects_a_range_and_batches_settlement_desktop() {
    selects_a_range_and_batches_settlement(DESKTOP);
}

#[test]
fn live_sidebar_selects_a_range_and_batches_settlement_phone() {
    selects_a_range_and_batches_settlement(PHONE);
}

fn selects_a_range_and_batches_settlement(window: (f32, f32)) {
    use super::sb_actions_1::{open_row_menu, start_live_sidebar};

    let mut app = start_live_sidebar(window);
    select_two(&mut app);
    open_row_menu(&mut app, window, "Beta thread");
    app.press("role:menuitem[Settle (2)]")
        .expect("settle both selected rows");
    app.wait_for("Settled 2,")
        .expect("one notice contains both rows");
    app.press_key("f9").expect("read RPC trace");
    let forward = app.shows("RPC thread.settle:alpha thread.settle:beta");
    let reverse = app.shows("RPC thread.settle:beta thread.settle:alpha");
    assert!(
        forward || reverse,
        "both selected rows must dispatch a settle command"
    );
}

pub(super) fn select_two(app: &mut Headless) {
    app.hold_keys("ctrl", true)
        .expect("hold selection modifier");
    app.press("role:button[Alpha thread, Project]")
        .expect("toggle first row");
    app.hold_keys("ctrl", false)
        .expect("release selection modifier");
    app.hold_keys("shift", true).expect("hold range modifier");
    app.press("role:button[Beta thread, Project]")
        .expect("extend through second row");
    app.hold_keys("shift", false)
        .expect("release range modifier");
}

#[test]
fn live_sidebar_confirms_batch_delete_desktop() {
    confirms_batch_delete(DESKTOP);
}

#[test]
fn live_sidebar_confirms_batch_delete_phone() {
    confirms_batch_delete(PHONE);
}

fn confirms_batch_delete(window: (f32, f32)) {
    use super::sb_actions_1::{assert_commands, open_row_menu, start_live_sidebar};

    let mut app = start_live_sidebar(window);
    select_two(&mut app);
    open_row_menu(&mut app, window, "Beta thread");
    app.press("role:menuitem[Delete (2)]")
        .expect("delete selected rows");
    app.wait_for("Delete 2 threads?")
        .expect("one confirmation for both rows");
    app.press("role:button[Cancel]").expect("cancel deletion");
    assert_commands(&mut app, "");
    open_row_menu(&mut app, window, "Beta thread");
    app.press("role:menuitem[Delete (2)]")
        .expect("selection survives cancellation");
    app.press("role:button[Confirm]").expect("delete both rows");
    assert_commands(&mut app, "thread.delete:alpha thread.delete:beta");
}

#[test]
fn live_sidebar_includes_search_hidden_targets_and_prunes_remote_removals() {
    use super::sb_actions_1::{assert_commands, open_row_menu, start_live_sidebar};

    for window in [DESKTOP, PHONE] {
        let mut app = start_live_sidebar(window);
        select_two(&mut app);
        app.type_text("label:Search threads", "Alpha")
            .expect("hide the other selection");
        app.wait_for("Selection 2")
            .expect("search retains selection");
        open_row_menu(&mut app, window, "Alpha thread");
        app.press("role:menuitem[Delete (2)]")
            .expect("hidden selection counts towards the batch");
        app.press("role:button[Cancel]")
            .expect("keep both selected");
        app.press_key("f8").expect("receive a remote deletion");
        app.wait_for("Selection 1")
            .expect("remote deletion prunes the hidden selection");
        open_row_menu(&mut app, window, "Alpha thread");
        app.press("role:menuitem[Delete (1)]")
            .expect("one surviving target");
        app.press("role:button[Confirm]").expect("delete");
        assert_commands(&mut app, "thread.delete:alpha");
        app.wait_for("Selection 0")
            .expect("successful batch clears its selection");
    }
}

#[test]
fn live_sidebar_deletes_the_hidden_selection_too() {
    use super::sb_actions_1::{assert_commands, open_row_menu, start_live_sidebar};

    let mut app = start_live_sidebar(DESKTOP);
    select_two(&mut app);
    app.type_text("label:Search threads", "Alpha")
        .expect("hide Beta");
    open_row_menu(&mut app, DESKTOP, "Alpha thread");
    app.press("role:menuitem[Delete (2)]")
        .expect("select batch delete");
    app.press("role:button[Confirm]").expect("delete both");
    assert_commands(&mut app, "thread.delete:alpha thread.delete:beta");
    app.wait_for("Selection 0")
        .expect("hidden selection also clears");
}
