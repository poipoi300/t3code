use super::items;
use std::{
    cell::OnceCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_contracts::{
    orchestration_project::OrchestrationProjectShell, orchestration_v2::*,
    orchestration_v2_id_table::IdOrIndex,
};

pub type SharedRows<T> = Rc<[Rc<T>]>;

fn id_key<T: AsRef<str>>(id: &IdOrIndex<T>) -> String {
    match id {
        IdOrIndex::Id(id) => format!("s:{}", id.as_ref()),
        IdOrIndex::Index(index) => format!("n:{index}"),
    }
}

pub fn reuse_rows<T: Clone + PartialEq, K: Ord>(
    previous: Option<&SharedRows<T>>,
    next: &[T],
    key: impl Fn(&T) -> K,
) -> SharedRows<T> {
    let prior: BTreeMap<_, _> = previous
        .into_iter()
        .flat_map(|rows| rows.iter())
        .map(|row| (key(row), row))
        .collect();
    let rows: Vec<_> = next
        .iter()
        .map(|row| {
            prior
                .get(&key(row))
                .filter(|old| old.as_ref() == row)
                .map_or_else(|| Rc::new(row.clone()), |old| (*old).clone())
        })
        .collect();
    if let Some(previous) = previous
        && previous.len() == rows.len()
        && previous
            .iter()
            .zip(&rows)
            .all(|(old, new)| Rc::ptr_eq(old, new))
    {
        return previous.clone();
    }
    rows.into()
}

/// The shell snapshot as rows shared across revisions.
///
/// The view links to the store's snapshot only weakly: holding a view never pins the store's
/// copy, so the next delta mutates it in place instead of cloning every row (see
/// `tests/store_streaming.rs`).
#[derive(Debug, Clone)]
pub struct SharedShellSnapshot {
    source: Weak<OrchestrationV2ShellSnapshot>,
    pub snapshot_sequence: u64,
    pub older_settled: Option<Rc<OrchestrationV2OlderSettledThreads>>,
    pub projects: SharedRows<OrchestrationProjectShell>,
    pub threads: SharedRows<OrchestrationV2ThreadShell>,
    pub archived_threads: SharedRows<OrchestrationV2ThreadShell>,
}

impl PartialEq for SharedShellSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot_sequence == other.snapshot_sequence
            && self.older_settled == other.older_settled
            && self.projects == other.projects
            && self.threads == other.threads
            && self.archived_threads == other.archived_threads
    }
}

/// Shell rows that row deltas touched since a view was built.
#[derive(Debug, Default)]
pub(crate) struct ShellRowChanges {
    threads: std::collections::BTreeSet<String>,
    projects: std::collections::BTreeSet<String>,
}

impl ShellRowChanges {
    /// Notes the rows `item` can change; false when it can change any of them.
    pub(crate) fn record(&mut self, item: &OrchestrationV2ShellStreamItem) -> bool {
        use OrchestrationV2ShellStreamItem::*;
        match item {
            ThreadUpdated(v) => self.threads.insert(v.thread.id.0.clone()),
            ThreadRemoved(v) => self.threads.insert(v.thread_id.0.clone()),
            ProjectUpdated(v) => self.projects.insert(v.project.id.0.clone()),
            ProjectRemoved(v) => self.projects.insert(v.project_id.0.clone()),
            Synchronized(_) => true,
            Snapshot(_) | SnapshotRequired(_) => return false,
        };
        true
    }
}

/// Reuses `previous` row for row when no row moved, comparing only the rows named as changed.
/// Anything else (a row added, removed or moved) falls back to [`reuse_rows`].
fn reuse_changed_rows<T: Clone + PartialEq>(
    previous: Option<&SharedRows<T>>,
    next: &[T],
    id: impl Fn(&T) -> &str,
    changed_ids: &std::collections::BTreeSet<String>,
) -> SharedRows<T> {
    let Some(previous) = previous.filter(|previous| {
        previous.len() == next.len()
            && previous
                .iter()
                .zip(next)
                .all(|(old, new)| id(old) == id(new))
    }) else {
        return reuse_rows(previous, next, |row| id(row).to_owned());
    };
    let mut changed = false;
    let rows: Vec<_> = previous
        .iter()
        .zip(next)
        .map(|(old, new)| {
            if changed_ids.contains(id(new)) && old.as_ref() != new {
                changed = true;
                Rc::new(new.clone())
            } else {
                old.clone()
            }
        })
        .collect();
    if changed {
        rows.into()
    } else {
        previous.clone()
    }
}

impl SharedShellSnapshot {
    /// Like [`Self::update`], trusting `changes` (when known) to name every row that differs.
    pub(crate) fn update_changed(
        source: Rc<OrchestrationV2ShellSnapshot>,
        previous: Option<&Self>,
        changes: Option<&ShellRowChanges>,
    ) -> Self {
        let Some(changes) = changes.filter(|_| previous.is_some()) else {
            return Self::update(source, previous);
        };
        let mut next = Self::update_scalars(&source, previous);
        next.projects = reuse_changed_rows(
            previous.map(|p| &p.projects),
            &source.projects,
            |p| p.id.0.as_str(),
            &changes.projects,
        );
        next.threads = reuse_changed_rows(
            previous.map(|p| &p.threads),
            &source.threads,
            |t| t.id.0.as_str(),
            &changes.threads,
        );
        next.archived_threads = reuse_changed_rows(
            previous.map(|p| &p.archived_threads),
            &source.archived_threads,
            |t| t.id.0.as_str(),
            &changes.threads,
        );
        next
    }

    fn update_scalars(source: &Rc<OrchestrationV2ShellSnapshot>, previous: Option<&Self>) -> Self {
        Self {
            snapshot_sequence: source.snapshot_sequence,
            older_settled: match (&source.older_settled, previous) {
                (None, _) => None,
                (Some(next), Some(previous)) if previous.older_settled.as_deref() == Some(next) => {
                    previous.older_settled.clone()
                }
                (Some(next), _) => Some(Rc::new(next.clone())),
            },
            projects: Rc::from([]),
            threads: Rc::from([]),
            archived_threads: Rc::from([]),
            source: Rc::downgrade(source),
        }
    }

    pub fn update(source: Rc<OrchestrationV2ShellSnapshot>, previous: Option<&Self>) -> Self {
        Self {
            projects: reuse_rows(previous.map(|p| &p.projects), &source.projects, |p| {
                p.id.0.clone()
            }),
            threads: reuse_rows(previous.map(|p| &p.threads), &source.threads, |t| {
                t.id.0.clone()
            }),
            archived_threads: reuse_rows(
                previous.map(|p| &p.archived_threads),
                &source.archived_threads,
                |t| t.id.0.clone(),
            ),
            ..Self::update_scalars(&source, previous)
        }
    }

    /// Whether this view was built from exactly this snapshot allocation.
    pub(crate) fn is_view_of(&self, source: &Rc<OrchestrationV2ShellSnapshot>) -> bool {
        std::ptr::eq(self.source.as_ptr(), Rc::as_ptr(source))
    }
}

macro_rules! shared_projection {
    ($($field:ident: $ty:ty => $key:expr),* $(,)?) => {
        /// A thread projection as rows shared across revisions.
        ///
        /// The view links to the store's projection only weakly: holding a view (as a UI memo
        /// does) never pins the store's copy, so a streamed delta mutates it in place instead of
        /// cloning every row (see `tests/store_streaming.rs`).
        #[derive(Debug, Clone)]
        pub struct SharedThreadProjection {
            source: Weak<OrchestrationV2ThreadProjection>,
            materialized: OnceCell<Rc<OrchestrationV2ThreadProjection>>,
            pub thread: Rc<OrchestrationV2AppThread>,
            pub updated_at: t3_contracts::wire::WireDateTime,
            $(pub $field: SharedRows<$ty>,)*
        }
        impl PartialEq for SharedThreadProjection {
            fn eq(&self, other: &Self) -> bool {
                self.thread == other.thread
                    && self.updated_at == other.updated_at
                    $(&& self.$field == other.$field)*
            }
        }
        impl SharedThreadProjection {
            pub fn update(source: Rc<OrchestrationV2ThreadProjection>, previous: Option<&Self>) -> Self {
                Self {
                    thread: previous.filter(|p| p.thread.as_ref() == &source.thread).map_or_else(|| Rc::new(source.thread.clone()), |p| p.thread.clone()),
                    updated_at: source.updated_at.clone(),
                    $($field: reuse_rows(previous.map(|p| &p.$field), &source.$field, $key),)*
                    source: Rc::downgrade(&source),
                    materialized: OnceCell::new(),
                }
            }
            /// The whole projection this view shows. While the thread is unchanged this is the
            /// store's own allocation; once a later change replaced it, the projection is rebuilt
            /// from the view's rows on first use (a copy of every row, so prefer the row fields).
            pub fn source(&self) -> Rc<OrchestrationV2ThreadProjection> {
                self.source.upgrade().unwrap_or_else(|| {
                    self.materialized
                        .get_or_init(|| {
                            Rc::new(OrchestrationV2ThreadProjection {
                                thread: self.thread.as_ref().clone(),
                                updated_at: self.updated_at.clone(),
                                $($field: self.$field.iter().map(|row| row.as_ref().clone()).collect(),)*
                            })
                        })
                        .clone()
                })
            }
        }
    };
}

shared_projection! {
    runs: OrchestrationV2Run => |v| id_key(&v.id),
    attempts: OrchestrationV2RunAttempt => |v| id_key(&v.id),
    nodes: OrchestrationV2ExecutionNode => |v| id_key(&v.id),
    subagents: OrchestrationV2Subagent => |v| id_key(&v.id),
    provider_sessions: OrchestrationV2ProviderSession => |v| id_key(&v.id),
    provider_threads: OrchestrationV2ProviderThread => |v| id_key(&v.id),
    provider_turns: OrchestrationV2ProviderTurn => |v| id_key(&v.id),
    runtime_requests: OrchestrationV2RuntimeRequest => |v| id_key(&v.id),
    messages: OrchestrationV2ConversationMessage => |v| id_key(&v.id),
    plans: OrchestrationV2PlanArtifact => |v| match v {
        OrchestrationV2PlanArtifact::ProposedPlan(v) => id_key(&v.id),
        OrchestrationV2PlanArtifact::TodoList(v) => id_key(&v.id),
    },
    turn_items: OrchestrationV2TurnItem => |v| id_key(items::id(v)),
    checkpoint_scopes: OrchestrationV2CheckpointScope => |v| id_key(&v.id),
    checkpoints: OrchestrationV2Checkpoint => |v| id_key(&v.id),
    context_handoffs: OrchestrationV2ContextHandoff => |v| id_key(&v.id),
    context_transfers: OrchestrationV2ContextTransfer => |v| id_key(&v.id),
    visible_turn_items: OrchestrationV2ProjectedTurnItem => |v| (id_key(&v.source_thread_id), id_key(&v.source_item_id)),
}

#[derive(Clone, Copy)]
pub(crate) struct StreamReplacement {
    item: usize,
    visible: usize,
}

impl SharedThreadProjection {
    pub(crate) fn replacement(
        &self,
        event: &OrchestrationV2ThreadStreamItem,
    ) -> Option<StreamReplacement> {
        let OrchestrationV2ThreadStreamItem::Event(event) = event else {
            return None;
        };
        let OrchestrationV2DomainEvent::TurnItemUpdated(event) = &event.event else {
            return None;
        };
        let source = self.source.upgrade()?;
        if matches!(
            event.payload,
            OrchestrationV2TurnItem::RunInterruptRequest(_)
        ) {
            return None;
        }
        let item = source
            .turn_items
            .iter()
            .rposition(|item| items::id(item) == items::id(&event.payload))?;
        if matches!(
            source.turn_items[item],
            OrchestrationV2TurnItem::RunInterruptRequest(_)
        ) {
            return None;
        }
        let visible = source
            .visible_turn_items
            .iter()
            .rposition(|row| row.source_item_id == *items::id(&event.payload))?;
        let row = &source.visible_turn_items[visible];
        (row.visibility == ProjectedTurnItemVisibility::Local
            && row.source_thread_id == *items::thread_id(&event.payload)
            && items::ordinal(&row.item) == items::ordinal(&event.payload)
            && items::visible(&source, &event.payload))
        .then_some(StreamReplacement { item, visible })
    }

    /// Whether this view was built from exactly this projection allocation.
    pub(crate) fn is_view_of(&self, source: &Rc<OrchestrationV2ThreadProjection>) -> bool {
        std::ptr::eq(self.source.as_ptr(), Rc::as_ptr(source))
    }

    pub(crate) fn replace(
        &self,
        source: Rc<OrchestrationV2ThreadProjection>,
        hint: StreamReplacement,
    ) -> Self {
        let mut next = self.clone();
        let mut items = self.turn_items.to_vec();
        items[hint.item] = Rc::new(source.turn_items[hint.item].clone());
        let mut visible = self.visible_turn_items.to_vec();
        visible[hint.visible] = Rc::new(source.visible_turn_items[hint.visible].clone());
        next.turn_items = items.into();
        next.visible_turn_items = visible.into();
        next.updated_at = source.updated_at.clone();
        next.source = Rc::downgrade(&source);
        next.materialized = OnceCell::new();
        next
    }
}
