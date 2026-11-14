//! Port of apps/web/src/components/chat/TimelineWorkGroupRows.tsx WorkGroupSection and work headers.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use futures::future::{LocalBoxFuture, join_all};
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRow, MessagesTimelineRowContent as Content, MessagesTimelineRowsInput,
    live_work_entry_label,
};
use t3_client::view_logic::session::{WorkLogEntry, work_entry_display_indicates_tool_failure};
use t3_client::work_log::live_thought_line;
use t3_contracts::orchestration_v2::OrchestrationV2TurnItem;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::settings::TimestampFormat;

use super::work::{WorkEntryRow, WorkRowContext, WorkRowState, work_entry_icon_name};
use crate::i18n::{Text, Value, t, tr};
use crate::runtime::Shared;
use crate::theme::{cn, tw};
use crate::ui::collapsible::CollapsibleHandle;
use crate::ui::icon::{Icon, icon_with_class};

pub type TimelineRow = Memo<Option<Shared<MessagesTimelineRow>>>;
pub type DeferredWorkLoader = Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<(), String>>>;

/// The outer timeline preserves its scroll anchor before changing the projection's expansions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disclosure {
    pub anchor_key: String,
    pub content_key: Option<String>,
    pub entry_id: Option<String>,
    pub collapsed: bool,
}

#[derive(Clone, Copy)]
pub struct WorkGroupsState {
    pub expanded_run_ids: Signal<BTreeSet<String>>,
    pub expanded_attempt_ids: Signal<BTreeSet<String>>,
    pub expanded_work_group_ids: Signal<BTreeSet<String>>,
    pub entries: WorkRowState,
    loads: Stored<BTreeMap<String, Signal<WorkLoad>>>,
    owner: Owner,
}

impl Default for WorkGroupsState {
    fn default() -> Self {
        Self {
            expanded_run_ids: signal(BTreeSet::new()),
            expanded_attempt_ids: signal(BTreeSet::new()),
            expanded_work_group_ids: signal(BTreeSet::new()),
            entries: WorkRowState::default(),
            loads: stored(BTreeMap::new()),
            owner: Owner::current(),
        }
    }
}

impl WorkGroupsState {
    /// Call inside the timeline's display-projection memo, before deriving C3 rows.
    pub fn apply_to_input(self, input: &mut MessagesTimelineRowsInput) {
        input.expanded_run_ids = self.expanded_run_ids.get();
        input.expanded_attempt_ids = self.expanded_attempt_ids.get();
        input.expanded_work_group_ids = self.expanded_work_group_ids.get();
    }

    fn load(self, id: &str) -> Signal<WorkLoad> {
        self.loads.with(|loads| {
            *loads
                .entry(id.to_owned())
                .or_insert_with(|| self.owner.run(|| signal(WorkLoad::default())))
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WorkLoad {
    anchors: Vec<String>,
    generation: u64,
    loading: bool,
    failed: bool,
}

/// Mirrors TimelineRowCtx. Defaults mount directly; optional slots belong to other row packages.
#[derive(Clone)]
pub struct WorkGroupsContext {
    pub state: WorkGroupsState,
    pub work: WorkRowContext,
    pub workspace_root: Memo<Option<String>>,
    pub pending_approval_ids: Memo<BTreeSet<String>>,
    pub is_preparing_worktree: Memo<bool>,
    pub is_compacting: Memo<bool>,
    pub provider_starting_label: Memo<Option<String>>,
    pub live_answer_preview: Memo<Option<String>>,
    pub timestamp_format: Memo<TimestampFormat>,
    pub load_deferred_work: Option<DeferredWorkLoader>,
    pub on_disclosure: Rc<dyn Fn(Disclosure)>,
}

impl WorkGroupsContext {
    pub fn new(state: WorkGroupsState) -> Self {
        let mut work = WorkRowContext::new(state.entries);
        work.icon = Some(Rc::new(|_, name, class| activity_icon(name, class)));
        let timestamp_format = computed(|| TimestampFormat::Locale);
        work.timestamp_format = timestamp_format;
        Self {
            state,
            work,
            workspace_root: computed(|| None),
            pending_approval_ids: computed(BTreeSet::new),
            is_preparing_worktree: computed(|| false),
            is_compacting: computed(|| false),
            provider_starting_label: computed(|| None),
            live_answer_preview: computed(|| None),
            timestamp_format,
            load_deferred_work: None,
            on_disclosure: Rc::new(|_| {}),
        }
    }
}

/// Mount for Work, WorkLive, Thinking, WorkToggle, TurnFold and AttemptFold only.
/// Other C3 row kinds keep their positions and are rendered by their owning packages.
pub fn work_group_timeline_row(row: TimelineRow, context: WorkGroupsContext) -> impl IntoElement {
    let kind = computed(move || {
        row.with(|row| {
            row.as_ref().and_then(|row| match row.content {
                Content::Work { .. } => Some(0),
                Content::WorkLive { .. } => Some(1),
                Content::Thinking { .. } => Some(2),
                Content::WorkToggle { .. } => Some(3),
                Content::TurnFold { .. } => Some(4),
                Content::AttemptFold { .. } => Some(5),
                _ => None,
            })
        })
    });
    view()
        .app_class(move || {
            Some(tw(row.with(|row| {
                row.as_ref().map_or("", |row| match &row.content {
                    Content::Work {
                        is_expanded_tool_group: true,
                        ..
                    } if row.continues_work_log != Some(true) => "pb-1",
                    Content::WorkLive { expanded: true, .. }
                    | Content::Thinking {
                        expanded: Some(true),
                        ..
                    }
                    | Content::WorkToggle { .. } => "pb-0",
                    Content::Work { .. } | Content::WorkLive { .. } | Content::Thinking { .. }
                        if row.continues_work_log != Some(true) =>
                    {
                        "pb-2"
                    }
                    _ => "pb-0",
                })
            })))
        })
        .child(dynamic(move || {
            let kind = kind.get();
            untracked(|| match kind {
                Some(0) => Some(work_group_section(row, context.clone()).into_element()),
                Some(1) => Some(live_work_entry_timeline_row(row, context.clone()).into_element()),
                Some(2) => Some(thinking_timeline_row(row, context.clone()).into_element()),
                Some(3) => {
                    Some(work_group_toggle_timeline_row(row, context.clone()).into_element())
                }
                Some(4) => Some(turn_fold_timeline_row(row, context.clone()).into_element()),
                Some(5) => Some(attempt_fold_timeline_row(row, context.clone()).into_element()),
                _ => None,
            })
        }))
}

pub fn work_group_section(row: TimelineRow, mut context: WorkGroupsContext) -> impl IntoElement {
    let disclose = context.on_disclosure.clone();
    let entry_toggle = context.work.on_toggle_entry.clone();
    context.work.on_toggle_entry = Some(Rc::new(move |id, collapsed| {
        row.with(|row| {
            if let Some(row) = row {
                disclose(Disclosure {
                    anchor_key: row.id.clone(),
                    content_key: None,
                    entry_id: Some(id.into()),
                    collapsed,
                });
            }
        });
        if let Some(toggle) = &entry_toggle {
            toggle(id, collapsed);
        }
    }));
    let only_reasoning = computed(move || {
        row.with(|row| row.as_ref().is_some_and(|row| {
        matches!(&row.content, Content::Work { grouped_entries, is_expanded_tool_group: true, .. }
            if !grouped_entries.is_empty() && grouped_entries.iter().all(|entry| entry.item_type.as_deref() == Some("reasoning")))
    }))
    });
    dynamic(move || {
        let only_reasoning = only_reasoning.get();
        Some(untracked(|| {
            if !only_reasoning {
                return super::work::work_timeline_row(row, context.work.clone()).into_element();
            }
            each(
                move || {
                    row.with(|row| match row.as_ref().map(|row| &row.content) {
                        Some(Content::Work {
                            grouped_entries, ..
                        }) => grouped_entries.clone(),
                        _ => Vec::new(),
                    })
                },
                |entry| entry.id.clone(),
                move |entry, _| {
                    let entry = computed(move || {
                        let entry = entry.get();
                        WorkEntryRow::from_row(&MessagesTimelineRow {
                            id: entry.id.clone(),
                            created_at: Some(entry.created_at.clone()),
                            continues_work_log: None,
                            content: Content::Work {
                                grouped_entries: vec![entry],
                                is_expanded_tool_group: true,
                                display_label: None,
                            },
                        })
                    });
                    super::work::reasoning_trace_content(entry, context.state.entries)
                },
            )
            .into_element()
        }))
    })
}

fn pending(entry: &WorkLogEntry, ids: &BTreeSet<String>) -> bool {
    match entry.projected_item.as_ref().map(|item| &item.item) {
        Some(OrchestrationV2TurnItem::ApprovalRequest(item)) => match &item.request_id {
            IdOrIndex::Id(id) => ids.contains(&id.0),
            IdOrIndex::Index(_) => false,
        },
        _ => false,
    }
}

// The web caps previews at 8192 UTF-16 units; never scan the full streamed reasoning source.
fn preview_source(source: &str) -> &str {
    let mut units = 0;
    for (at, ch) in source.char_indices().take(8193) {
        units += ch.len_utf16();
        if units > 8192 {
            return &source[..at];
        }
    }
    source
}

pub fn thinking_timeline_row(row: TimelineRow, context: WorkGroupsContext) -> impl IntoElement {
    dynamic(move || {
        if context.is_preparing_worktree.get() || context.is_compacting.get() {
            return Some(view().app_class(tw("min-h-6")).into_element());
        }
        let current = row.get()?;
        let Content::Thinking {
            label,
            group_id,
            expanded,
        } = &current.content
        else {
            return None;
        };
        let label = context
            .provider_starting_label
            .get()
            .map(Text::client_key)
            .or_else(|| label.clone().map(Text::content))
            .or_else(|| context.live_answer_preview.get().map(Text::content))
            .unwrap_or_else(|| Text::from("Thinking"));
        Some(untracked(|| {
            let activity = live_activity_row(label.clone(), "brain", false);
            match group_id {
                Some(group) => group_button(
                    row,
                    &context,
                    group.clone(),
                    expanded == &Some(true),
                    label,
                    activity,
                    "group/live-work flex min-h-6 w-full max-w-full cursor-pointer items-center rounded-md text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70",
                ),
                None => activity,
            }
        }))
    })
}

pub fn live_work_entry_timeline_row(
    row: TimelineRow,
    context: WorkGroupsContext,
) -> impl IntoElement {
    dynamic(move || {
        let current = row.get()?;
        let Content::WorkLive {
            entry,
            group_id,
            expanded,
            active,
            thought,
            ..
        } = &current.content
        else {
            return None;
        };
        let pending = pending(entry, &context.pending_approval_ids.get());
        let workspace = context.workspace_root.get();
        if pending {
            return context.work.pending_approval.as_ref().and_then(|render| {
                WorkEntryRow::from_row(&current).map(|mut entry| {
                    entry.pending_approval = true;
                    render(&entry)
                })
            });
        }
        let failed = work_entry_display_indicates_tool_failure(entry);
        let thought_line = thought
            .as_ref()
            .filter(|_| !expanded)
            .map(|thought| {
                live_thought_line(preview_source(
                    thought.detail.as_deref().unwrap_or_default(),
                ))
            })
            .unwrap_or_default();
        let thought_is_status = !thought_line.is_empty()
            && thought
                .as_ref()
                .is_some_and(|thought| Rc::ptr_eq(thought, entry));
        let label = if thought_is_status {
            Text::from(if *active { "Thinking" } else { "Thought" })
        } else if entry.item_type.as_deref() == Some("reasoning") {
            Text::content(live_thought_line(preview_source(
                entry.detail.as_deref().unwrap_or_default(),
            )))
        } else {
            Text::content(live_work_entry_label(entry, workspace.as_deref(), *active))
        };
        let accessible = if failed {
            Text::content(tr(
                "{preview}, tool call failed",
                &[(
                    "preview",
                    Value::from(if thought_line.is_empty() {
                        label.resolve().to_string()
                    } else {
                        format!("{thought_line} {}", label.resolve())
                    }),
                )],
            ))
        } else {
            label.clone()
        };
        Some(untracked(|| {
            let activity = view()
                .child((!thought_line.is_empty()).then(|| {
                    work_log_row(
                        activity_icon("brain", "text-icon-muted"),
                        text(SharedString::from(thought_line.clone()))
                            .number_of_lines(4u32)
                            .into_element(),
                        None,
                        true,
                    )
                }))
                .child(live_activity_row(
                    label,
                    &work_entry_icon_name(entry),
                    failed,
                ))
                .into_element();
            group_button(
                row,
                &context,
                group_id.clone(),
                *expanded,
                accessible,
                activity,
                &cn([
                    Some(
                        "group/live-work flex w-full max-w-full cursor-pointer rounded-md text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70",
                    ),
                    Some(if thought_line.is_empty() {
                        "min-h-6 items-center"
                    } else {
                        "flex-col items-start"
                    }),
                ]),
            )
        }))
    })
}

fn group_button(
    row: TimelineRow,
    context: &WorkGroupsContext,
    group: String,
    expanded: bool,
    label: Text,
    body: AnyElement,
    class: &str,
) -> AnyElement {
    let state = context.state;
    let disclose = context.on_disclosure.clone();
    let handle = CollapsibleHandle::controlled(
        move || expanded,
        || false,
        move |next| {
            row.with(|row| {
                if let Some(row) = row {
                    disclose(Disclosure {
                        anchor_key: row.id.clone(),
                        content_key: next.then(|| format!("{group}:details")),
                        entry_id: None,
                        collapsed: !next,
                    });
                }
            });
            set_expanded(state.expanded_work_group_ids, &group, next);
        },
    );
    // Like CollapsibleTrigger's NSH host: ButtonProps cannot expose expanded. C3's panel is in sibling cells.
    view()
        .role(AccessibilityRole::Button)
        .label(move || label.resolve())
        .accessibility_state(move || AccessibilityState {
            expanded: Some(handle.is_open()),
            ..Default::default()
        })
        .test_id(&row.with(|row| row.as_ref().map(|row| row.id.clone()).unwrap_or_default()))
        .on_press(move || handle.toggle())
        .app_class(tw(class))
        .child(body)
        .into_element()
}

pub fn work_group_toggle_timeline_row(
    row: TimelineRow,
    context: WorkGroupsContext,
) -> impl IntoElement {
    view()
        .app_class(move || {
            Some(tw(row.with(|row| {
                row.as_ref().map_or("pb-0", |row| {
                    if matches!(row.content, Content::WorkToggle { expanded: true, .. })
                        || row.continues_work_log == Some(true)
                    {
                        "pb-0"
                    } else {
                        "pb-2"
                    }
                })
            })))
        })
        .child(work_group_header(row, context))
}

fn work_group_header(row: TimelineRow, context: WorkGroupsContext) -> impl IntoElement {
    dynamic(move || {
        let current = row.get()?;
        let Content::WorkToggle {
            group_id,
            summary,
            summary_kind,
            tool_surface,
            has_failure,
            expanded,
            ..
        } = &current.content
        else {
            return None;
        };
        let label = Text::content(summary.clone());
        let accessible = if *has_failure {
            Text::content(tr(
                "{preview}, tool call failed",
                &[("preview", Value::from(summary.clone()))],
            ))
        } else {
            label.clone()
        };
        Some(untracked(|| {
            group_button(
                row,
                &context,
                group_id.clone(),
                *expanded,
                accessible,
                work_log_line(
                    activity_icon(
                        tool_surface
                            .as_deref()
                            .unwrap_or_else(|| tool_group_summary_icon_name(summary_kind)),
                        "text-icon-muted",
                    ),
                    text(label.resolve())
                        .app_class(tw("block truncate"))
                        .into_element(),
                    Some(
                        view()
                            .app_class(tw("flex items-center gap-1.5"))
                            .child(icon_with_class(
                                if *expanded {
                                    Icon::ChevronDown
                                } else {
                                    Icon::ChevronRight
                                },
                                "size-3.5 shrink-0 text-muted-foreground",
                            ))
                            .child(
                                current
                                    .created_at
                                    .as_ref()
                                    .map(|at| timeline_row_timestamp(at, context.timestamp_format)),
                            )
                            .into_element(),
                    ),
                    false,
                )
                .into_element(),
                "group/timeline-row relative w-full min-w-0 rounded-md px-0.5 text-left transition-colors py-0.5 cursor-pointer focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70",
            )
        }))
    })
}

/// Port of MessagesTimeline.tsx TurnFoldTimelineRow.
pub fn turn_fold_timeline_row(row: TimelineRow, context: WorkGroupsContext) -> impl IntoElement {
    fold_timeline_row(row, context, false)
}

/// Port of MessagesTimeline.tsx AttemptFoldTimelineRow.
pub fn attempt_fold_timeline_row(row: TimelineRow, context: WorkGroupsContext) -> impl IntoElement {
    fold_timeline_row(row, context, true)
}

fn fold_timeline_row(
    row: TimelineRow,
    context: WorkGroupsContext,
    attempt: bool,
) -> impl IntoElement {
    let state = context.state;
    let loader = context.load_deferred_work.clone();
    // Restored expanded folds and newly arrived deferred anchors need the same service call.
    effect(move || {
        let current = row.get();
        if let Some(current) = current {
            let (expanded, anchors) = match &current.content {
                Content::TurnFold {
                    expanded,
                    deferred_work_item_ids,
                    ..
                } => (
                    *expanded,
                    deferred_work_item_ids.clone().unwrap_or_default(),
                ),
                Content::AttemptFold {
                    expanded,
                    deferred_work_item_ids,
                    ..
                } => (*expanded, deferred_work_item_ids.clone()),
                _ => return,
            };
            untracked(|| {
                let load = state.load(&current.id);
                if expanded && !anchors.is_empty() && load.get_untracked().anchors != anchors {
                    load_work(state.owner, load, anchors.clone(), loader.clone());
                }
                if anchors.is_empty() && !load.get_untracked().anchors.is_empty() {
                    load.set(WorkLoad {
                        generation: load.get_untracked().generation.wrapping_add(1),
                        ..Default::default()
                    });
                }
            });
        }
    });
    dynamic(move || {
        let current = row.get()?;
        let (key, label, expanded, anchors) = match &current.content {
            Content::TurnFold {
                run_id,
                label,
                expanded,
                deferred_work_item_ids,
            } => (
                run_id.clone(),
                label.clone(),
                *expanded,
                deferred_work_item_ids.clone().unwrap_or_default(),
            ),
            Content::AttemptFold {
                attempt_id,
                label,
                expanded,
                deferred_work_item_ids,
                ..
            } => (
                attempt_id.clone(),
                label.clone(),
                *expanded,
                deferred_work_item_ids.clone(),
            ),
            _ => return None,
        };
        let load = state.load(&current.id);
        let loading = load.get();
        let error = !anchors.is_empty() && loading.failed;
        let visible_label = if error {
            format!("{label} · {}", t("Couldn't load work. Click to retry."))
        } else {
            label
        };
        let row_id = current.id.clone();
        let loader = context.load_deferred_work.clone();
        let disclose = context.on_disclosure.clone();
        Some(untracked(|| {
            let handle = CollapsibleHandle::controlled(
                move || expanded,
                || false,
                move |next| {
                    if error && expanded {
                        load_work(state.owner, load, anchors.clone(), loader.clone());
                        return;
                    }
                    disclose(Disclosure {
                        anchor_key: row_id.clone(),
                        content_key: None,
                        entry_id: None,
                        collapsed: !next,
                    });
                    set_expanded(
                        if attempt {
                            state.expanded_attempt_ids
                        } else {
                            state.expanded_run_ids
                        },
                        &key,
                        next,
                    );
                },
            );
            // CollapsibleTrigger's host preserves expanded/busy; its panel lives in other virtual cells.
            let trigger = view().role(AccessibilityRole::Button)
                .label(SharedString::from(visible_label.clone()))
                .accessibility_state(move || AccessibilityState { expanded: Some(handle.is_open()), busy: loading.loading, ..Default::default() })
                .test_id(&current.id)
                .on_press(move || handle.toggle())
                .app_class(tw(if attempt {
                    "flex w-full cursor-pointer select-none items-center gap-2 rounded-md border border-border/60 bg-muted/20 px-2.5 py-2 text-left transition-colors hover:bg-muted/35 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70"
                } else {
                    "flex cursor-pointer select-none items-center gap-1 rounded-md px-1 text-sm leading-relaxed text-muted-foreground tabular-nums transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70"
                }));
            let chevron = || {
                icon_with_class(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    "size-3.5 shrink-0 text-muted-foreground",
                )
            };
            let trigger = if attempt {
                trigger
                    .child(chevron())
                    .child(
                        text(SharedString::from(visible_label))
                            .app_class(tw("text-xs font-medium text-foreground/80")),
                    )
                    .child(
                        text(t("Cut off by a steer"))
                            .app_class(tw("text-2xs text-muted-foreground")),
                    )
            } else {
                trigger
                    .child(text(SharedString::from(visible_label)))
                    .child(chevron())
            };
            view()
                .app_class(tw(if attempt {
                    ""
                } else {
                    "group/timeline-row border-b border-border/60 pb-2 pe-0.5 pt-1"
                }))
                .child(
                    view()
                        .app_class(tw("relative flex items-center gap-1"))
                        .child(trigger)
                        .child(
                            (!attempt)
                                .then(|| {
                                    current.created_at.as_ref().map(|at| {
                                        timeline_row_timestamp(at, context.timestamp_format)
                                    })
                                })
                                .flatten(),
                        ),
                )
                .child((expanded && loading.loading).then(work_history_skeleton))
        }))
    })
}

fn load_work(
    owner: Owner,
    load: Signal<WorkLoad>,
    anchors: Vec<String>,
    loader: Option<DeferredWorkLoader>,
) {
    let generation = load.get_untracked().generation.wrapping_add(1);
    load.set(WorkLoad {
        anchors: anchors.clone(),
        generation,
        loading: loader.is_some(),
        failed: loader.is_none(),
    });
    let Some(loader) = loader else {
        return;
    };
    // Recycling a header must not cancel its request and strand a thread-owned loading state.
    owner.run(|| {
        spawn(async move {
            let results = join_all(anchors.into_iter().map(|anchor| loader(anchor))).await;
            if load.get_untracked().generation == generation {
                load.update(|state| {
                    state.loading = false;
                    state.failed = results.iter().any(Result::is_err);
                });
            }
        })
    });
}

fn set_expanded(set: Signal<BTreeSet<String>>, key: &str, expanded: bool) {
    set.update(|set| {
        if expanded {
            set.insert(key.to_owned());
        } else {
            set.remove(key);
        }
    });
}

/// Port of MessagesTimeline.tsx WorkHistorySkeleton, with static placeholders (no repaint loop).
fn work_history_skeleton() -> impl IntoElement {
    let mut root = view()
        // The pinned NSH has no Status role or live-region builder; retain the labelled region.
        .role(AccessibilityRole::Region)
        .label(t("Loading work history"))
        .app_class(tw("space-y-2 px-1 pb-1 pt-3"));
    for class in ["h-2.5 w-2/5", "h-2.5 w-3/5", "h-2.5 w-1/3"] {
        root = root.child(
            view()
                .accessible(false)
                .app_class(tw("flex items-center gap-2"))
                .child(view().app_class(tw("size-3 shrink-0 rounded-md bg-muted")))
                .child(view().app_class(tw(&cn([Some("rounded-full bg-muted"), Some(class)])))),
        );
    }
    root
}

fn live_activity_row(label: Text, name: &str, failed: bool) -> AnyElement {
    // ActivityShimmerOverlay/live-tool-shine continuously repaint; the brief requires static activity.
    view().app_class(tw("relative min-h-6 w-fit max-w-full min-w-0 overflow-hidden rounded-md text-sm leading-relaxed"))
        .child(work_log_row(activity_icon(name, if failed { "text-tool-error-icon/40" } else { "text-icon-muted" }),
            text(label.resolve()).app_class(tw("block truncate")).into_element(), None, false))
        .into_element()
}

/// Port of WorkLog.tsx WorkLogRow/WorkLogLine; child selectors live on the selected children.
fn work_log_row(
    glyph: AnyElement,
    label: AnyElement,
    trailing: Option<AnyElement>,
    wrap: bool,
) -> impl IntoElement {
    view().app_class(tw("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 text-left transition-colors py-0.5"))
        .child(work_log_line(glyph, label, trailing, wrap))
}

fn work_log_line(
    glyph: AnyElement,
    label: AnyElement,
    trailing: Option<AnyElement>,
    wrap: bool,
) -> impl IntoElement {
    view()
        .app_class(tw(&cn([
            Some("flex min-h-6 min-w-0 gap-1.5 text-sm leading-relaxed select-none"),
            Some(if wrap { "items-start" } else { "items-center" }),
        ])))
        .child(
            view()
                .app_class(tw("flex min-w-0 flex-1 items-baseline gap-1.5"))
                // NSH has no cap-relative inline baseline; retain the exact 24px icon slot.
                .child(
                    view()
                        .app_class(tw("relative w-6 shrink-0 leading-0"))
                        .child(
                            view().app_class(tw("size-6 overflow-hidden")).child(
                                view()
                                    .app_class(tw("flex size-full items-center justify-center"))
                                    .child(glyph),
                            ),
                        ),
                )
                .child(
                    view()
                        .app_class(tw(&cn([
                            Some("min-w-0 flex-1 text-secondary-label"),
                            (!wrap).then_some("truncate"),
                        ])))
                        .child(label),
                ),
        )
        .child(trailing)
}

/// TimelineRowParts.tsx layout around WORK-1's shared TimestampPresenter host.
fn timeline_row_timestamp(created_at: &str, format: Memo<TimestampFormat>) -> impl IntoElement {
    view().app_class(tw("pointer-events-none absolute shrink-0 group-hover/timeline-row:pointer-events-auto group-hover/timeline-row:static group-focus-within/timeline-row:pointer-events-auto group-focus-within/timeline-row:static"))
        .child(super::work::timeline_row_timestamp(created_at.into(), format, false))
}

fn activity_icon(name: &str, class: &str) -> AnyElement {
    if matches!(name, "browser" | "computer") {
        // Port of TimelineToolActivityIcon.tsx BrowserAppIcon, absent from Lucide.
        return svg(ImageSrc::svg_markup(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M8.5 19H7.2C4.4 19 3 17.5 3 14.6V7.4C3 4.5 4.5 3 7.4 3h8.2C18.5 3 20 4.5 20 7.4v2.4"/><circle cx="7.4" cy="7.2" r="0.75" fill="currentColor" stroke="none"/><path d="M11.2 7.2h4.3"/><path d="m12.4 11.4 7.5 2.6-3.4 1.6-1.5 3.6z" fill="currentColor" stroke="none"/></svg>"#))
            .app_class(tw(&cn([Some("block size-4 shrink-0 opacity-70"), Some(class)]))).accessible(false).into_element();
    }
    let name = match name {
        "pull-request" => "git-pull-request",
        "device" => "smartphone",
        "code" => "code-xml",
        other => other,
    };
    view()
        .app_class(tw(&cn([
            Some("flex size-4 items-center justify-center"),
            Some(class),
        ])))
        .child(
            Icon::from_name(name)
                .map(|icon| icon_with_class(icon, "block size-4 shrink-0 stroke-2 opacity-70")),
        )
        .into_element()
}

/// Port of TimelineWorkEntryRow.logic.ts toolGroupSummaryIconName; summary policy stays in C3.
fn tool_group_summary_icon_name(kind: &str) -> &str {
    match kind {
        "pull-request" | "link-pr" | "unlink-pr" | "list-prs" | "watch-pr" | "unwatch-pr" => {
            "pull-request"
        }
        "read" => "eye",
        "edit" => "square-pen",
        "command" => "terminal",
        "thread-create" => "t3-code",
        "browser" => "browser",
        "device" => "device",
        "search" => "globe",
        "code-search" => "search",
        "other" => "wrench",
        "dynamic-tool" | "update" | "mixed" => "hammer",
        "reasoning" => "brain",
        "agent-tool" => "bot",
        "tone-tool" => "zap",
        _ => kind,
    }
}
