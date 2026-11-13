//! Keyboard commands. The keybinding rules (defaults, `when` clauses, last-match-wins, `Mod`)
//! are the client's (`t3_client::view_logic::shell::keybindings`); this module binds the
//! effective keystrokes with NSH's `on_key` and runs the matching command.
//!
//! The slice binds the sidebar toggle directly; package SHELL-KEYS replaces `install` with the
//! client-driven table and the `KeyContext` (terminal, editable focus, palette open).

pub mod bindings;

use nsh_core::prelude::*;

use crate::state::panels::Panels;

/// Binds the shell's commands under the current owner.
pub fn install(panels: Panels) {
    on_key("mod+b", move |_| {
        panels.sidebar_open.update(|open| *open = !*open)
    });
}
