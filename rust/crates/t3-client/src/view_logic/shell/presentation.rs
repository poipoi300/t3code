//! Composed sidebar selectors; UI adapters supply canonical shelf and request state.
use super::{Section, SidebarThread, group_threads_by_project, is_hidden_thread, status};
use crate::{
    thread_views::{
        shelves::{self, InboxObservation, ShelfInput, ShelfRow},
        threads::sort::{self, OrderedThread, SortOrder, ThreadSortInput},
    },
    types::ThreadKey,
    workspace::{ProjectKey, sidebar::SidebarProjectSnapshot},
};
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::orchestration_v2::AppThreadSettledOverride;

pub struct ThreadInput<'a> {
    pub thread: SidebarThread<'a>,
    pub shelf: ShelfInput,
    pub status: status::StatusInput<'a>,
}
impl<'a> ThreadInput<'a> {
    /// Adapter from WP6's canonical presentation; legacy visits apply only to old servers.
    pub fn from_environment_shell(
        view: &'a crate::services::models::EnvironmentThreadShell,
        local_visit: Option<&'a str>,
        parse: &impl Fn(&str) -> Option<i64>,
    ) -> Self {
        use crate::{
            services::models::PendingSubagentRequest,
            thread_views::shelves::{Run, RunState, RuntimeState},
        };
        use t3_contracts::{
            orchestration_v2::{
                OrchestrationV2RunStatus as R, OrchestrationV2ShellThreadStatus as S,
            },
            provider_policy::ProviderInteractionMode,
        };
        let s = &view.source;
        let runtime = view.runtime.as_ref().map(|r| match r.status {
            S::Preparing => RuntimeState::Preparing,
            S::Queued => RuntimeState::Queued,
            S::Starting => RuntimeState::Starting,
            S::Running => RuntimeState::Running,
            S::Waiting => RuntimeState::Waiting,
            S::Idle => RuntimeState::Idle,
            S::Failed => RuntimeState::Failed,
            _ => RuntimeState::Other,
        });
        let latest = view.latest_run.as_ref();
        let run_state = |r: &R| match r {
            R::Preparing => RunState::Preparing,
            R::Queued => RunState::Queued,
            R::Starting => RunState::Starting,
            R::Running => RunState::Running,
            R::Waiting => RunState::Waiting,
            R::Completed => RunState::Completed,
            R::Interrupted => RunState::Interrupted,
            R::Failed => RunState::Failed,
            R::Cancelled => RunState::Cancelled,
            R::RolledBack => RunState::RolledBack,
        };
        let parsed = |d: &Option<Option<t3_contracts::wire::WireDateTime>>| {
            d.as_ref()
                .and_then(Option::as_ref)
                .and_then(|d| parse(&d.0))
        };
        let guest_messages = s
            .pending_guest_message_count
            .as_ref()
            .and_then(Option::as_ref)
            .map_or(0, |n| *n as usize);
        let latest_run = latest.map(|r| Run {
            state: Some(run_state(&r.status)),
            requested_at: r.requested_at.as_ref().and_then(|d| parse(&d.0)),
            started_at: r.started_at.as_ref().and_then(|d| parse(&d.0)),
            completed_at: r.completed_at.as_ref().and_then(|d| parse(&d.0)),
            is_runtime_active_run: view.runtime.as_ref().and_then(|r| r.active_run_id.as_ref())
                == Some(&r.run_id),
        });
        let settled = latest_run.as_ref().is_some_and(|r| {
            !r.is_runtime_active_run
                && !matches!(
                    r.state,
                    Some(
                        RunState::Preparing
                            | RunState::Queued
                            | RunState::Starting
                            | RunState::Running
                            | RunState::Waiting
                    )
                )
        });
        let subagent_request = view.pending_subagent_request.map(|r| match r {
            PendingSubagentRequest::Approval => "approval",
            PendingSubagentRequest::UserInput => "user_input",
        });
        let plan_mode = s.interaction_mode == ProviderInteractionMode::Plan;
        Self {
            thread: SidebarThread {key:ThreadKey{environment:view.environment.clone(),thread:s.id.clone()},project:ProjectKey{environment:view.environment.clone(),project:s.project_id.clone()},shell:s},
            shelf: ShelfInput {runtime,runtime_updated_at:view.runtime.as_ref().and_then(|r|parse(&r.updated_at.0)),legacy_session_error:false,latest_run,latest_user_message_at:s.latest_user_message_at.as_ref().and_then(|d|parse(&d.0)),approvals:view.has_pending_approvals,questions:view.has_pending_user_input,pending_non_question_request:view.has_pending_approvals,question_at:view.pending_user_input_at.as_ref().and_then(|d|parse(&d.0)),subagent_request:subagent_request.is_some(),guest_messages,snoozed_at:parsed(&s.snoozed_at),snoozed_until:parsed(&s.snoozed_until),plan_mode,actionable_plan:s.has_actionable_proposed_plan},
            status: status::StatusInput {approvals:view.has_pending_approvals,user_input:view.has_pending_user_input,subagent_request,guest_messages,runtime,usage_limit:view.runtime.as_ref().is_some_and(|r|r.last_error_class==Some(t3_contracts::orchestration_v2::OrchestrationV2ProviderFailureClass::UsageLimit)),blocked_on_thread_wait:s.blocked_on_thread_wait==Some(Some(true)),latest_run_failed:latest.is_some_and(|r|r.status==R::Failed),latest_run_completed_at:latest.and_then(|r|r.completed_at.as_ref()).map(|d|d.0.as_str()),last_visited_at:status::resolve_last_visited_at(s.last_visited_at.as_ref().map(|d|d.as_ref().map(|d|d.0.as_str())),local_visit),background_holds_completion:crate::thread_views::threads::pending_work::background_work_holds_completion(s.pending_background_tasks.as_ref().and_then(Option::as_deref).unwrap_or(&[])),plan_mode,latest_run_settled:settled,actionable_plan:s.has_actionable_proposed_plan},
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActiveOrder {
    #[default]
    Manual,
    Created,
    LatestUserMessage,
    InboxReturn,
}
pub struct Options<'a> {
    pub now: i64,
    pub scope: Option<&'a BTreeSet<ProjectKey>>,
    pub delegated_under_parent: &'a BTreeSet<ThreadKey>,
    pub selected: &'a BTreeSet<ThreadKey>,
    pub route: Option<&'a ThreadKey>,
    pub working_shelf: bool,
    pub active_order: ActiveOrder,
    /// Optional observation from `InboxReturnTracker`, including its exit grace period.
    pub inbox: Option<&'a InboxObservation>,
}
pub struct Row<'a> {
    pub source: &'a ThreadInput<'a>,
    pub section: Section,
    pub status: status::Status,
    pub pill: Option<status::Pill>,
    pub unread: bool,
    pub woke: bool,
    pub active: bool,
    pub selected: bool,
    pub receded: bool,
}
pub struct Folder<'a> {
    pub group: &'a SidebarProjectSnapshot,
    /// Indices into `Sidebar.rows`, preserving their shelf/order sequence.
    pub rows: Vec<usize>,
    pub dot: Option<&'static str>,
}
pub struct Sidebar<'a> {
    /// Pinned, Active, Working, Snoozed, Settled, in that order.
    pub rows: Vec<Row<'a>>,
    pub folders: Vec<Folder<'a>>,
    pub next_snooze_wake: Option<i64>,
}
fn sort_input<'a>(t: &'a SidebarThread<'_>) -> ThreadSortInput<'a> {
    let s = t.shell;
    ThreadSortInput {
        created_at: &s.created_at.0,
        updated_at: &s.updated_at.0,
        latest_user_message_at: s.latest_user_message_at.as_ref().map(|d| d.0.as_str()),
        latest_user_authored_message_at: s
            .latest_user_authored_message_at
            .as_ref()
            .map(|d| d.as_ref().map(|d| d.0.as_str())),
        messages: &[],
    }
}
/// Uses existing grouping and thread policies, without reading the host or mutating the store.
pub fn present_sidebar<'a>(
    groups: &'a [SidebarProjectSnapshot],
    threads: &'a [ThreadInput<'a>],
    options: &Options<'_>,
    parse: &impl Fn(&str) -> Option<i64>,
    keep_empty: impl Fn(&SidebarProjectSnapshot) -> bool,
) -> Sidebar<'a> {
    let mut sections: [Vec<&ThreadInput<'_>>; 5] = Default::default();
    let mut next_wake = None;
    for input in threads {
        let t = &input.thread;
        let s = t.shell;
        if s.archived_at.is_some()
            || s.deleted_at.is_some()
            || is_hidden_thread(&s.lineage)
            || options.delegated_under_parent.contains(&t.key)
            || options
                .scope
                .is_some_and(|scope| !scope.contains(&t.project))
        {
            continue;
        }
        if let Some(wake) = input.shelf.snoozed_until.filter(|wake| *wake > options.now) {
            next_wake = Some(next_wake.map_or(wake, |previous: i64| previous.min(wake)));
        }
        let section = if shelves::effective_snoozed(&input.shelf, options.now) {
            3
        } else if s.settled_override == Some(AppThreadSettledOverride::Settled) {
            4
        } else if s.pinned_at.as_ref().is_some_and(Option::is_some) {
            0
        } else if options.working_shelf
            && options.inbox.map_or_else(
                || shelves::is_working(&input.shelf),
                |inbox| inbox.is_working(&t.key, &input.shelf),
            )
        {
            2
        } else {
            1
        };
        sections[section].push(input);
    }
    let parsed = |d: &Option<Option<t3_contracts::wire::WireDateTime>>| {
        d.as_ref()
            .and_then(Option::as_ref)
            .and_then(|d| parse(&d.0))
    };
    for (index, rows) in sections.iter_mut().enumerate() {
        if index == 0 || (index == 1 && options.active_order == ActiveOrder::Manual) {
            let mut ordered = rows
                .iter()
                .map(|r| OrderedThread {
                    key: &r.thread.key,
                    created_at: parse(&r.thread.shell.created_at.0),
                    unsettled_at: parsed(&r.thread.shell.unsettled_at),
                    order_key: (if index == 0 {
                        &r.thread.shell.pin_order_key
                    } else {
                        &r.thread.shell.active_order_key
                    })
                    .as_ref()
                    .and_then(Option::as_ref)
                    .map(|s| s.0.as_str()),
                })
                .collect::<Vec<_>>();
            if index == 0 {
                sort::sort_pinned_threads(&mut ordered);
            } else {
                sort::sort_active_threads(&mut ordered);
            }
            let rank: BTreeMap<_, _> = ordered
                .iter()
                .enumerate()
                .map(|(i, r)| (r.key, i))
                .collect();
            rows.sort_by_key(|r| rank[&r.thread.key]);
        } else if index == 2 || (index == 1 && options.active_order == ActiveOrder::InboxReturn) {
            let mut ordered = rows
                .iter()
                .map(|r| ShelfRow {
                    key: r.thread.key.clone(),
                    created_at: parse(&r.thread.shell.created_at.0),
                    unsettled_at: parsed(&r.thread.shell.unsettled_at),
                    run_requested_at: r.shelf.latest_run.as_ref().and_then(|r| r.requested_at),
                    run_completed_at: r.shelf.latest_run.as_ref().and_then(|r| r.completed_at),
                    user_authored_at: r
                        .thread
                        .shell
                        .latest_user_authored_message_at
                        .as_ref()
                        .map(|d| d.as_ref().and_then(|d| parse(&d.0))),
                })
                .collect::<Vec<_>>();
            if index == 2 {
                shelves::sort_working(&mut ordered);
            } else {
                let returns = rows
                    .iter()
                    .filter_map(|r| {
                        options
                            .inbox
                            .and_then(|inbox| inbox.returned_at(&r.thread.key))
                            .map(|time| (r.thread.key.clone(), time))
                    })
                    .collect();
                shelves::sort_inbox(&mut ordered, &returns);
            }
            let rank: BTreeMap<_, _> = ordered
                .iter()
                .enumerate()
                .map(|(i, r)| (&r.key, i))
                .collect();
            rows.sort_by_key(|r| rank[&r.thread.key]);
        } else if index == 1 {
            let order = if options.active_order == ActiveOrder::Created {
                SortOrder::CreatedAt
            } else {
                SortOrder::LatestUserMessage
            };
            sort::sort_threads(
                rows,
                order,
                |r| sort_input(&r.thread),
                |r| &r.thread.key,
                parse,
            );
        } else {
            let stamp = |r: &&ThreadInput<'_>| {
                if index == 3 {
                    r.shelf.snoozed_until.unwrap_or(0)
                } else {
                    sort::resolve_settled_thread_timestamp(
                        r.thread.shell.settled_at.as_ref().map(|d| d.0.as_str()),
                        r.thread
                            .shell
                            .latest_user_message_at
                            .as_ref()
                            .map(|d| d.0.as_str()),
                        &[
                            r.thread
                                .shell
                                .latest_run_requested_at
                                .as_ref()
                                .and_then(Option::as_ref)
                                .map(|d| d.0.as_str()),
                            r.thread
                                .shell
                                .latest_run_started_at
                                .as_ref()
                                .and_then(Option::as_ref)
                                .map(|d| d.0.as_str()),
                            r.thread
                                .shell
                                .latest_run_completed_at
                                .as_ref()
                                .and_then(Option::as_ref)
                                .map(|d| d.0.as_str()),
                        ],
                        &r.thread.shell.updated_at.0,
                        parse,
                    )
                    .and_then(parse)
                    .unwrap_or(0)
                }
            };
            rows.sort_by(|a, b| {
                (if index == 3 {
                    stamp(a).cmp(&stamp(b))
                } else {
                    stamp(b).cmp(&stamp(a))
                })
                .then(a.thread.key.thread.cmp(&b.thread.key.thread))
                .then(a.thread.key.environment.cmp(&b.thread.key.environment))
            });
        }
    }
    let rows = sections
        .into_iter()
        .enumerate()
        .flat_map(|(i, rows)| rows.into_iter().map(move |source| (i, source)))
        .map(|(i, source)| {
            let status = status::resolve_status(&source.status);
            let unread = status::has_unseen_completion(&source.status, parse);
            let settled =
                source.thread.shell.settled_override == Some(AppThreadSettledOverride::Settled);
            let woke = shelves::is_woke_unseen(
                shelves::woke_at(&source.shelf, options.now),
                settled,
                source.status.last_visited_at.and_then(parse),
                source.status.latest_run_completed_at.and_then(parse),
            );
            let active = options.route == Some(&source.thread.key);
            let selected = options.selected.contains(&source.thread.key);
            Row {
                source,
                section: [
                    Section::Pinned,
                    Section::Active,
                    Section::Working,
                    Section::Snoozed,
                    Section::Settled,
                ][i],
                status,
                pill: status::resolve_status_pill(&source.status, parse),
                unread,
                woke,
                active,
                selected,
                receded: status::should_recede(status, unread, woke, active, selected),
            }
        })
        .collect::<Vec<_>>();
    let indices = rows
        .iter()
        .enumerate()
        .map(|(i, row)| (&row.source.thread.key, i))
        .collect::<BTreeMap<_, _>>();
    let folders = group_threads_by_project(
        groups,
        &rows,
        |r| r.source.thread.project.clone(),
        keep_empty,
    )
    .into_iter()
    .map(|(group, members)| {
        let statuses = members
            .iter()
            .map(|r| r.source.status.clone())
            .collect::<Vec<_>>();
        Folder {
            group,
            rows: members
                .into_iter()
                .map(|row| indices[&row.source.thread.key])
                .collect(),
            dot: status::folder_dot(&statuses, parse),
        }
    })
    .collect();
    Sidebar {
        rows,
        folders,
        next_snooze_wake: next_wake,
    }
}
