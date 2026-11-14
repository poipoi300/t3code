//! Port of apps/web/src/threadSelectionStore.ts useThreadSelectionStore.

use nsh_core::prelude::*;
pub use t3_client::view_logic::shell::sidebar::{
    ThreadSelection, keys_to_deselect_after_delete as get_thread_keys_to_deselect_after_delete,
};

/// One app-local selection, retained when the sidebar or its virtual cells unmount.
#[derive(Clone, Copy)]
pub struct ThreadSelectionStore {
    state: Signal<ThreadSelection>,
}

pub fn use_thread_selection_store() -> ThreadSelectionStore {
    ThreadSelectionStore {
        state: global::<ThreadSelection>(),
    }
}

impl ThreadSelectionStore {
    fn update(self, change: impl FnOnce(&mut ThreadSelection)) {
        let mut next = self.state.get_untracked();
        change(&mut next);
        if self.state.with_untracked(|current| current != &next) {
            self.state.set(next);
        }
    }

    pub fn state(self) -> ThreadSelection {
        self.state.get()
    }

    pub fn selected_thread_keys(self) -> Vec<String> {
        self.state.with(|state| state.selected.clone())
    }

    pub fn anchor_thread_key(self) -> Option<String> {
        self.state.with(|state| state.anchor.clone())
    }

    pub fn is_selected(self, key: &str) -> bool {
        self.state
            .with(|state| state.selected.iter().any(|selected| selected == key))
    }

    pub fn has_selection(self) -> bool {
        self.state.with(|state| !state.selected.is_empty())
    }

    pub fn toggle_thread(self, key: &str) {
        self.update(|state| state.toggle(key));
    }

    /// `ordered` is the client's ordered scope, never an order reconstructed from mounted cells.
    pub fn range_select_to(self, key: &str, ordered: &[String]) {
        self.update(|state| state.range_select_to(key, ordered));
    }

    pub fn clear_selection(self) {
        self.update(ThreadSelection::clear);
    }

    pub fn remove_from_selection(self, keys: &[String]) {
        self.update(|state| state.remove(keys));
    }

    pub fn set_anchor(self, key: &str) {
        self.update(|state| state.set_anchor(key.into()));
    }
}
