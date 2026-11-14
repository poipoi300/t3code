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
