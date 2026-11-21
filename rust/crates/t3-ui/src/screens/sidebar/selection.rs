//! Port of apps/web/src/threadSelectionStore.ts useThreadSelectionStore.

use nsh_core::prelude::*;
pub use t3_client::view_logic::shell::sidebar::{
    ThreadSelection, keys_to_deselect_after_delete as get_thread_keys_to_deselect_after_delete,
};

/// One app-local selection, retained when the sidebar or its virtual cells unmount.
#[derive(Clone, Copy)]
pub struct ThreadSelectionStore {
    state: Signal<ThreadSelection>,
    revision: Signal<SelectionRevision>,
}

#[derive(Clone, Default)]
struct SelectionRevision(u64);

pub fn use_thread_selection_store() -> ThreadSelectionStore {
    ThreadSelectionStore {
        state: global::<ThreadSelection>(),
        revision: global::<SelectionRevision>(),
    }
}

impl ThreadSelectionStore {
    fn update(self, change: impl FnOnce(&mut ThreadSelection)) {
        let mut next = self.state.get_untracked();
        change(&mut next);
        if self.state.with_untracked(|current| current != &next) {
            self.revision
                .update(|revision| revision.0 = revision.0.wrapping_add(1));
            self.state.set(next);
        }
    }

    pub(super) fn revision(self) -> u64 {
        self.revision.with_untracked(|revision| revision.0)
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

    pub(super) fn retain_environment(
        self,
        environment: &t3_client::types::EnvironmentId,
        threads: &t3_client::state::sharing::SharedShellSnapshot,
    ) {
        let prefix = environment_prefix(environment);
        let present = threads
            .threads
            .iter()
            .filter(|row| row.archived_at.is_none() && row.deleted_at.is_none())
            .map(|row| row.id.0.as_str())
            .collect::<std::collections::HashSet<_>>();
        let retain = |key: &str| {
            key.strip_prefix(&prefix)
                .is_none_or(|id| present.contains(id))
        };
        self.update(|state| {
            state.selected.retain(|key| retain(key));
            if state.anchor.as_deref().is_some_and(|key| !retain(key)) {
                state.anchor = None;
            }
        });
    }

    pub fn set_anchor(self, key: &str) {
        self.update(|state| state.set_anchor(key.into()));
    }
}

use super::row::RowThreadActions;
use crate::{
    runtime::tasks::TaskOwner,
    state::nav::{Navigator, Route},
};
use t3_client::{types::ThreadKey, view_logic::shell::sidebar::Section};

fn environment_prefix(environment: &t3_client::types::EnvironmentId) -> String {
    // Length-prefix the environment so ids containing ':' cannot alias another environment.
    format!("{}:{}", environment.0.len(), environment.0)
}

pub(super) fn row_key(key: &ThreadKey) -> String {
    format!("{}{}", environment_prefix(&key.environment), key.thread.0)
}

#[derive(Clone, Copy)]
pub(super) struct Interactions {
    pub store: ThreadSelectionStore,
    pub visible_threads: Memo<Vec<ThreadKey>>,
    pub available_threads: Memo<Vec<(ThreadKey, Section)>>,
    pub tasks: TaskOwner,
}

pub(super) struct NavigationPlan {
    pub(super) requested: Route,
    candidates: Vec<ThreadKey>,
    pub(super) project: Option<t3_client::workspace::ProjectKey>,
}

impl Interactions {
    pub fn press(self, key: ThreadKey, nav: Navigator, event: &PressEvent) {
        let scoped = row_key(&key);
        let toggle = if apple_keys() {
            event.modifiers.cmd
        } else {
            event.modifiers.ctrl
        };
        if toggle {
            self.store.toggle_thread(&scoped);
        } else if event.modifiers.shift {
            let ordered = self
                .visible_threads
                .with_untracked(|rows| rows.iter().map(row_key).collect::<Vec<_>>());
            self.store.range_select_to(&scoped, &ordered);
        } else if event.clicks < 2 {
            self.store.clear_selection();
            self.store.set_anchor(&scoped);
            nav.open(Route::Thread(key));
        }
    }

    pub fn targets(self, key: &ThreadKey) -> (Vec<ThreadKey>, bool) {
        if !self.store.is_selected(&row_key(key)) {
            return (vec![key.clone()], false);
        }
        let selected = self.store.selected_thread_keys();
        let threads = self.available_threads.with(|rows| {
            selected
                .iter()
                .filter_map(|selected| {
                    rows.iter()
                        .find(|(key, _)| row_key(key) == *selected)
                        .map(|(key, _)| key.clone())
                })
                .collect()
        });
        (threads, true)
    }

    pub fn plan_navigation(
        self,
        nav: Navigator,
        id: &str,
        targets: &[ThreadKey],
    ) -> Option<NavigationPlan> {
        if !matches!(id, "archive" | "delete" | "settle" | "snooze") {
            return None;
        }
        let requested = nav.top();
        let current = requested.thread().filter(|key| targets.contains(key))?;
        let candidates = self.available_threads.with_untracked(|rows| {
            let next = rows
                .iter()
                .position(|(key, _)| key == current)
                .map_or(0, |i| i + 1);
            rows[next..]
                .iter()
                .chain(&rows[..next])
                .filter(|(key, section)| {
                    !matches!(section, Section::Settled | Section::Snoozed) && key != current
                })
                .map(|(key, _)| key.clone())
                .collect()
        });
        Some(NavigationPlan {
            requested,
            candidates,
            project: None,
        })
    }

    pub fn finish(
        self,
        nav: Navigator,
        id: &str,
        attempted: &[ThreadKey],
        completed: &[ThreadKey],
        client: &dyn RowThreadActions,
        navigation: Option<NavigationPlan>,
    ) {
        if matches!(id, "archive" | "delete") {
            let removed = completed.iter().map(row_key).collect();
            let attempted = attempted.iter().map(row_key).collect::<Vec<_>>();
            let present = self.available_threads.get_untracked();
            self.store
                .remove_from_selection(&get_thread_keys_to_deselect_after_delete(
                    &attempted,
                    &removed,
                    |key| {
                        present.iter().any(|(thread, _)| {
                            row_key(thread) == key && client.can_still_show(thread)
                        })
                    },
                ));
        }
        if id != "archive"
            && let Some(plan) = navigation
            && nav.top() == plan.requested
            && plan
                .requested
                .thread()
                .is_some_and(|key| completed.contains(key) && client.action_moves_thread(key, id))
        {
            let project = if id == "delete" { plan.project } else { None };
            let next = plan.candidates.into_iter().find(|key| {
                !completed.contains(key)
                    && client.can_still_show(key)
                    && (!matches!(id, "settle" | "snooze") || !client.action_moves_thread(key, id))
                    && project
                        .as_ref()
                        .is_none_or(|project| client.project(key).as_ref() == Some(project))
            });
            nav.open(next.map_or(Route::Home, Route::Thread));
        }
    }
}

pub(super) fn install_escape(store: ThreadSelectionStore) {
    use std::rc::Rc;

    use crate::keys::bindings::{Keybindings, Scope};
    use t3_client::view_logic::shell::keybindings::parse_shortcut;

    if let Some(keys) = use_context::<Keybindings>() {
        keys.register(Scope {
            raw_keys: parse_shortcut("escape").into_iter().collect(),
            on_keydown: Some(Rc::new(move |event| {
                if event.inside_confirmation || event.key.key != "Escape" || !store.has_selection()
                {
                    return false;
                }
                store.clear_selection();
                event.claim();
                true
            })),
            ..Default::default()
        });
    }
}

/// The last archive registered to return to the thread that was open when it started.
#[derive(Clone, Default)]
pub(super) struct ArchiveReturn(pub Option<ThreadKey>);
