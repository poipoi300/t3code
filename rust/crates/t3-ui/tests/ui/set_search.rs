//! SET-SEARCH: headless tests of the settings nav search, over the client's search table.

use t3_ui::state::nav::SettingsPage;

use crate::set_shell::start;
use crate::support::DESKTOP;

#[test]
fn a_search_lists_matching_rows_and_a_result_opens_its_page() {
    let mut app = start(DESKTOP, SettingsPage::Chat);
    app.wait_for("Follow-ups").expect("chat page");
    app.type_text("role:combobox[Search settings]", "delete conf")
        .expect("search typed");
    app.wait_for("role:option[Delete confirmation]")
        .expect("ranked result");
    // The nav sections give way to the results while searching.
    assert!(!app.screen().shows("Agents"));
    app.press("role:option[Delete confirmation]")
        .expect("open result");
    app.wait_for("Route general")
        .expect("navigated to the row's page");
    app.wait_for("Confirmations").expect("general page");
}

#[test]
fn a_search_without_matches_says_so_and_clearing_restores_the_nav() {
    let mut app = start(DESKTOP, SettingsPage::General);
    app.type_text("role:combobox[Search settings]", "zzz-no-match")
        .expect("search typed");
    app.wait_for("No settings found").expect("empty state");
    app.press("role:button[Clear settings search]")
        .expect("clear");
    app.wait_for("Agents").expect("sections back");
}

#[test]
fn a_result_whose_scope_is_not_selected_shows_the_owning_scopes_instead_of_the_page() {
    let mut app = start(DESKTOP, SettingsPage::General);
    app.type_text("role:combobox[Search settings]", "project overview")
        .expect("search typed");
    app.press("role:option[Project overview]")
        .expect("open result");
    app.wait_for("Route projects").expect("the row's page");
    app.wait_for("Project overview is not available for the selected target. Choose its owning scope to continue.")
        .expect("the scope notice replaces the page");
}

#[test]
fn a_jump_to_a_folded_row_opens_its_advanced_section_on_the_other_page() {
    let mut app = start(DESKTOP, SettingsPage::General);
    app.type_text("role:combobox[Search settings]", "skills slash")
        .expect("search typed");
    app.press("role:option[Show skills in slash menu]")
        .expect("open result");
    app.wait_for("Route chat").expect("the row's page");
    app.wait_for("role:switch[Show skills in slash menu]")
        .expect("the fold opened for the target");
}
