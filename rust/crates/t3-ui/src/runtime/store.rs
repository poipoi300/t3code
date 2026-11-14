//! `ClientStore` to NSH signals.
//!
//! One task drains the store's subscription (subscribed before anything is read) and bumps one
//! tick signal per changed scope. Selectors are memos that read one tick and compare the client's
//! structurally shared `Rc`s by pointer, so only what the client actually replaced notifies.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::ClientStore;
use t3_client::state::sharing::{SharedShellSnapshot, SharedThreadProjection};
use t3_client::state::shell::ShellState;
use t3_client::state::thread::ThreadState;
use t3_client::store::{ChangeSet, Scope};
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_client::view_logic::session::{
    TimelineEntriesInput, TimelineEntriesProjection, TimelineEntry,
    derive_timeline_entries_from_visible_turn_items_with_state,
};

use crate::runtime::tasks::watch;

/// An `Rc` compared by identity: a memo holding it notifies only when the client produced a new
/// value, never because an equal-looking value was rebuilt.
pub struct Shared<T>(pub Rc<T>);

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Shared({:p})", Rc::as_ptr(&self.0))
    }
}

/// Which ticks a change bumps, given the ticks that exist.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Bump {
    pub environments: bool,
    pub shells: BTreeSet<EnvironmentId>,
    pub threads: BTreeSet<ThreadKey>,
}

pub fn affected<'a>(
    change: &ChangeSet,
    shells: impl Iterator<Item = &'a EnvironmentId> + Clone,
    threads: impl Iterator<Item = &'a ThreadKey> + Clone,
) -> Bump {
    let mut bump = Bump::default();
    for scope in &change.scopes {
        match scope {
            Scope::All => {
                bump.environments = true;
                bump.shells.extend(shells.clone().cloned());
                bump.threads.extend(threads.clone().cloned());
            }
            Scope::Environment(environment) => {
                bump.environments = true;
                bump.shells.insert(environment.clone());
                bump.threads.extend(
                    threads
                        .clone()
                        .filter(|key| &key.environment == environment)
                        .cloned(),
                );
            }
            Scope::Shell(environment) => {
                bump.shells.insert(environment.clone());
            }
            Scope::Thread(key) => {
                bump.threads.insert(key.clone());
            }
        }
    }
    bump
}

/// The store as signals. `Copy`; provided by `App` through the runtime handle.
#[derive(Clone, Copy)]
pub struct StoreSignals {
    store: Stored<ClientStore>,
    owner: Owner,
    environments: Signal<u64>,
    shells: Stored<BTreeMap<EnvironmentId, Signal<u64>>>,
    threads: Stored<BTreeMap<ThreadKey, Signal<u64>>>,
}

impl StoreSignals {
    /// Subscribes, then starts the drain task under the current owner (`App`'s).
    pub fn attach(store: ClientStore) -> Self {
        let mut subscription = store.subscribe();
        let signals = Self {
            store: stored(store),
            owner: Owner::current(),
            environments: signal(0),
            shells: stored(BTreeMap::new()),
            threads: stored(BTreeMap::new()),
        };
        watch(async move {
            while let Some(change) = subscription.next().await {
                signals.apply(&change);
            }
        });
        signals
    }

    fn apply(&self, change: &ChangeSet) {
        let bump = self.shells.with(|shells| {
            self.threads
                .with(|threads| affected(change, shells.keys(), threads.keys()))
        });
        let mut ticks: Vec<Signal<u64>> = self.shells.with(|shells| {
            bump.shells
                .iter()
                .filter_map(|environment| shells.get(environment).copied())
                .collect()
        });
        self.threads.with(|threads| {
            ticks.extend(
                bump.threads
                    .iter()
                    .filter_map(|key| threads.get(key).copied()),
            );
        });
        if bump.environments {
            ticks.push(self.environments);
        }
        batch(|| {
            for tick in ticks {
                tick.update(|t| *t += 1);
            }
        });
    }

    pub fn store(&self) -> ClientStore {
        self.store.get()
    }

    fn shell_tick(&self, environment: &EnvironmentId) -> Signal<u64> {
        let owner = self.owner;
        self.shells.with(|shells| {
            *shells
                .entry(environment.clone())
                .or_insert_with(|| owner.run(|| signal(0)))
        })
    }

    fn thread_tick(&self, key: &ThreadKey) -> Signal<u64> {
        let owner = self.owner;
        self.threads.with(|threads| {
            *threads
                .entry(key.clone())
                .or_insert_with(|| owner.run(|| signal(0)))
        })
    }

    /// Subscribes the caller to the set of environments the store knows.
    pub fn track_environments(&self) {
        self.environments.get();
    }

    /// A value derived from an environment's shell state; notifies when `f`'s result changes.
    pub fn shell_state<T: PartialEq + 'static>(
        &self,
        environment: EnvironmentId,
        f: impl Fn(Option<&ShellState>) -> T + 'static,
    ) -> Memo<T> {
        let this = *self;
        computed(move || {
            this.shell_tick(&environment).get();
            let snapshot = this.store.with(|store| store.snapshot());
            f(t3_client::state::selectors::shell(&snapshot, &environment))
        })
    }

    /// The shell snapshot with rows shared across revisions.
    pub fn shell(&self, environment: EnvironmentId) -> Memo<Option<Shared<SharedShellSnapshot>>> {
        let this = *self;
        computed(move || {
            this.shell_tick(&environment).get();
            this.store
                .with(|store| store.shared_shell(&environment))
                .map(Shared)
        })
    }

    /// A value derived from a thread's state; notifies when `f`'s result changes.
    pub fn thread_state<T: PartialEq + 'static>(
        &self,
        key: ThreadKey,
        f: impl Fn(Option<&ThreadState>) -> T + 'static,
    ) -> Memo<T> {
        let this = *self;
        computed(move || {
            this.thread_tick(&key).get();
            let snapshot = this.store.with(|store| store.snapshot());
            f(t3_client::state::selectors::thread(&snapshot, &key))
        })
    }

    /// The thread projection with rows shared across revisions.
    pub fn thread(&self, key: ThreadKey) -> Memo<Option<Shared<SharedThreadProjection>>> {
        let this = *self;
        computed(move || {
            this.thread_tick(&key).get();
            this.store
                .with(|store| store.shared_thread(&key))
                .map(Shared)
        })
    }

    /// The thread's timeline entries, derived incrementally by the client (unchanged entries keep
    /// their `Rc`), with list keys that are reused while the entry ids are unchanged.
    pub fn timeline(&self, key: ThreadKey) -> Memo<Shared<Timeline>> {
        let thread = self.thread(key);
        let previous: Stored<Option<(TimelineEntriesProjection, Rc<Timeline>)>> = stored(None);
        computed(move || {
            let Some(projection) = thread.get() else {
                previous.with(|p| *p = None);
                return Shared(Rc::new(Timeline::default()));
            };
            previous.with(|previous| {
                let input = timeline_input(&projection);
                let derived = derive_timeline_entries_from_visible_turn_items_with_state(
                    input,
                    previous.as_ref().map(|(projection, _)| projection),
                );
                let reused = previous
                    .as_ref()
                    .filter(|(_, timeline)| Rc::ptr_eq(&timeline.entries, &derived.entries))
                    .map(|(_, timeline)| timeline.clone());
                let timeline = reused.unwrap_or_else(|| {
                    Rc::new(Timeline::next(
                        previous.as_ref().map(|(_, timeline)| timeline.as_ref()),
                        derived.entries.clone(),
                    ))
                });
                *previous = Some((derived, timeline.clone()));
                Shared(timeline)
            })
        })
    }
}

fn timeline_input(projection: &SharedThreadProjection) -> TimelineEntriesInput {
    TimelineEntriesInput {
        visible_turn_items: projection.visible_turn_items.to_vec(),
        attempts: projection.attempts.to_vec(),
        nodes: projection.nodes.to_vec(),
        plans: projection
            .plans
            .iter()
            .map(|plan| (**plan).clone())
            .collect(),
        ..TimelineEntriesInput::default()
    }
}

/// Entries plus the list keys the virtual list diffs on.
#[derive(Debug, Default)]
pub struct Timeline {
    pub entries: Rc<Vec<Rc<TimelineEntry>>>,
    pub keys: Rc<Vec<SharedString>>,
}

impl Timeline {
    /// Reuses the previous key list while every entry keeps its id, so a streamed token (which
    /// replaces one entry's content) does not make the list re-diff its rows.
    pub fn next(previous: Option<&Timeline>, entries: Rc<Vec<Rc<TimelineEntry>>>) -> Self {
        let same_ids = previous.is_some_and(|previous| {
            previous.entries.len() == entries.len()
                && previous
                    .entries
                    .iter()
                    .zip(entries.iter())
                    .all(|(old, new)| Rc::ptr_eq(old, new) || old.id == new.id)
        });
        let keys = match previous {
            Some(previous) if same_ids => previous.keys.clone(),
            _ => Rc::new(
                entries
                    .iter()
                    .map(|e| SharedString::from(e.id.clone()))
                    .collect(),
            ),
        };
        Self { entries, keys }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::types::ThreadId;
    use t3_client::view_logic::session::{ChatMessage, TimelineContent};

    fn env(id: &str) -> EnvironmentId {
        EnvironmentId(id.into())
    }

    fn key(environment: &str, thread: &str) -> ThreadKey {
        ThreadKey {
            environment: env(environment),
            thread: ThreadId(thread.into()),
        }
    }

    fn change(scopes: &[Scope]) -> ChangeSet {
        ChangeSet {
            revision: 1,
            scopes: scopes.iter().cloned().collect(),
        }
    }

    #[test]
    fn a_thread_change_bumps_only_that_thread() {
        let shells = [env("a"), env("b")];
        let threads = [key("a", "1"), key("a", "2"), key("b", "1")];
        let bump = affected(
            &change(&[Scope::Thread(key("a", "2"))]),
            shells.iter(),
            threads.iter(),
        );
        assert_eq!(
            bump,
            Bump {
                environments: false,
                shells: BTreeSet::new(),
                threads: [key("a", "2")].into(),
            }
        );
    }

    #[test]
    fn an_environment_change_bumps_its_shell_and_threads_but_not_another_environments() {
        let shells = [env("a"), env("b")];
        let threads = [key("a", "1"), key("b", "1")];
        let bump = affected(
            &change(&[Scope::Environment(env("a"))]),
            shells.iter(),
            threads.iter(),
        );
        assert!(bump.environments);
        assert_eq!(bump.shells, [env("a")].into());
        assert_eq!(bump.threads, [key("a", "1")].into());
    }

    #[test]
    fn all_bumps_every_known_tick_and_coalesced_scopes_union() {
        let shells = [env("a")];
        let threads = [key("a", "1"), key("b", "9")];
        let all = affected(&change(&[Scope::All]), shells.iter(), threads.iter());
        assert!(all.environments);
        assert_eq!(all.threads.len(), 2);
        let union = affected(
            &change(&[Scope::Shell(env("b")), Scope::Thread(key("a", "1"))]),
            shells.iter(),
            threads.iter(),
        );
        assert!(!union.environments);
        assert_eq!(union.shells, [env("b")].into());
        assert_eq!(union.threads, [key("a", "1")].into());
    }

    fn entry(id: &str, text: &str) -> Rc<TimelineEntry> {
        let message: ChatMessage = serde_json::from_value(serde_json::json!({
            "id": id, "role": "assistant", "text": text, "runId": null, "streaming": false,
            "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z"
        }))
        .expect("chat message");
        Rc::new(TimelineEntry {
            id: id.into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            attempt: None,
            content: TimelineContent::Message {
                message: Rc::new(message),
                projected_item: None,
            },
        })
    }

    #[test]
    fn keys_survive_a_content_change_and_rebuild_when_ids_change() {
        let first = Timeline::next(None, Rc::new(vec![entry("a", "x"), entry("b", "y")]));
        let streamed = Timeline::next(
            Some(&first),
            Rc::new(vec![first.entries[0].clone(), entry("b", "y more")]),
        );
        assert!(Rc::ptr_eq(&first.keys, &streamed.keys));
        let appended = Timeline::next(
            Some(&streamed),
            Rc::new(vec![
                streamed.entries[0].clone(),
                streamed.entries[1].clone(),
                entry("c", "z"),
            ]),
        );
        assert!(!Rc::ptr_eq(&streamed.keys, &appended.keys));
        assert_eq!(
            appended.keys.as_slice(),
            ["a", "b", "c"].map(SharedString::from)
        );
        let replaced = Timeline::next(
            Some(&appended),
            Rc::new(vec![entry("a", "x"), entry("q", "y"), entry("c", "z")]),
        );
        assert_eq!(
            replaced.keys.as_slice(),
            ["a", "q", "c"].map(SharedString::from)
        );
    }
}
