//! Keyboard commands. The keybinding rules (defaults, `when` clauses, last-match-wins, `Mod`)
//! are the client's (`t3_client::view_logic::shell::keybindings`); this module binds the
//! effective keystrokes with NSH's `on_key` and runs the matching command.
//!
//! SH-KEYS' client-driven table and its key context (terminal, editable focus, palette open).

pub mod bindings;

use nsh_core::prelude::*;

use crate::state::panels::Panels;

/// Installs the keybindings under the current owner. `primary` is the designated-primary
/// environment, independent of the route (`None` for an unscoped client).
pub fn install(
    panels: Panels,
    primary: Signal<Option<t3_client::types::EnvironmentId>>,
) -> bindings::Keybindings {
    bindings::install(panels, primary)
}
