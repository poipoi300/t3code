//! Port of apps/web/src/components/sidebar/SidebarThreadRow.tsx SidebarThreadRow.

use std::{rc::Rc, time::Duration};

use futures::{StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};

use nsh_core::prelude::*;
use t3_client::services::config::timestamp_ms;
use t3_client::services::models::{
    EnvironmentThreadShell, PendingSubagentRequest, format_working_duration_label,
    present_thread_shell, resolve_thread_provider_stack, resolve_thread_working_started_at,
};
use t3_client::store::Scope as StoreScope;
use t3_client::thread_views::{
    shelves::SnoozePreset,
    thread_ops::{
        actions::BatchReport,
        undo::{UndoNotice, UndoReport},
    },
};
use t3_client::view_logic::session::timestamps::{
    format_relative_elapsed, ms_until_relative_time_change, relative_time_elapsed_ms,
};
use t3_client::view_logic::shell::models::entries_for;
use t3_client::view_logic::shell::sidebar::presentation::ThreadInput;
use t3_client::view_logic::shell::sidebar::status::{self, Status};
use t3_client::{ClientError, types::ThreadKey, workspace::ProjectKey};
use t3_contracts::orchestration_project::OrchestrationProjectShell;
use t3_contracts::orchestration_v2::{
    AppThreadSettledOverride, OrchestrationV2ProviderGoalStatus, OrchestrationV2ThreadShell,
};
use t3_contracts::{server::ServerConfig, wire::WireDateTime};

use super::{RowVariant, SidebarItem, actions::ThreadActionMenu};
use crate::i18n::{Value, t, tr};
use crate::runtime::tasks::TaskOwner;
use crate::runtime::{Rt, Shared, rt};
use crate::state::nav::{Navigator, Route};
use crate::theme::{Scope, cn, palette, tw_with};
use crate::ui::avatar::{self, ProjectFaviconProject};
use crate::ui::icon::{Icon, icon, icon_with_class, provider_icon};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

use crate::runtime::tasks::watch;

type Thread = Memo<Option<Shared<OrchestrationV2ThreadShell>>>;

/// The client's thread operations a row uses: its action menu, its actions and rename.
pub trait RowThreadActions {
    fn menu(&self, key: &ThreadKey) -> ThreadActionMenu;

    fn sidebar_menu(
        &self,
        keys: &[ThreadKey],
        _: i64,
        _: bool,
    ) -> (ThreadActionMenu, Vec<SnoozePreset>) {
        (self.menu(&keys[0]), vec![])
    }

    fn sidebar_batch(
        &self,
        _: Vec<ThreadKey>,
        _: String,
        _: Option<WireDateTime>,
    ) -> LocalBoxFuture<'static, Result<BatchReport, ClientError>> {
        Box::pin(async { Err(ClientError::Unsupported("sidebar batch actions")) })
    }

    fn preset(
        &self,
        _: &ThreadKey,
        _: &str,
        _: &[SnoozePreset],
    ) -> Result<WireDateTime, ClientError> {
        Err(ClientError::Unsupported("snooze presets"))
    }

    fn snooze(
        &self,
        _: ThreadKey,
        _: WireDateTime,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        Box::pin(async { Err(ClientError::Unsupported("snooze")) })
    }

    fn undo_notices(&self) -> LocalBoxStream<'static, Option<UndoNotice>> {
        Box::pin(futures::stream::empty())
    }

    fn undo(&self) -> LocalBoxFuture<'static, UndoReport> {
        Box::pin(async { UndoReport::default() })
    }

    fn project(&self, _: &ThreadKey) -> Option<ProjectKey> {
        None
    }

    fn can_drag_thread(&self, _: &ThreadKey) -> bool {
        false
    }

    fn can_sidebar_drop(
        &self,
        _: &t3_client::thread_views::thread_ops::actions::SidebarDrop,
    ) -> bool {
        false
    }

    fn sidebar_drop(
        &self,
        _: t3_client::thread_views::thread_ops::actions::SidebarDrop,
    ) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        Box::pin(async { Err(ClientError::Unsupported("sidebar drag")) })
    }

    fn sidebar_preferences(
        &self,
    ) -> LocalBoxStream<'static, Rc<t3_contracts::settings::ClientSettingsSchema>> {
        Box::pin(futures::stream::empty())
    }

    fn can_pin_project(&self, _: &[ProjectKey]) -> bool {
        false
    }

    fn pin_project(
        &self,
        _: Vec<ProjectKey>,
        _: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        Box::pin(async { Err(ClientError::Unsupported("project pinning")) })
    }

    fn project_draft(&self, _: ProjectKey) -> LocalBoxFuture<'static, Result<String, ClientError>> {
        Box::pin(async { Err(ClientError::Unsupported("project drafts")) })
    }

    fn claims_undo_shortcut(&self) -> bool {
        false
    }

    fn release_undo_shortcut(&self) {}

    fn can_still_show(&self, _: &ThreadKey) -> bool {
        true
    }

    fn action_moves_thread(&self, _: &ThreadKey, _: &str) -> bool {
        true
    }

    fn perform(
        &self,
        key: ThreadKey,
        action: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>>;
    fn rename(
        &self,
        key: ThreadKey,
        title: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>>;
}

impl<P: t3_client::platform::Platform + Clone + 'static> RowThreadActions
    for t3_client::composition::ClientRuntime<P>
{
    fn menu(&self, key: &ThreadKey) -> ThreadActionMenu {
        crate::runtime::services::thread_ops::thread_action_menu(self, key)
    }
    fn perform(
        &self,
        key: ThreadKey,
        action: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.thread_actions();
        match action.as_str() {
            "archive" => service.archive_thread(key),
            "unarchive" => service.unarchive_thread(key),
            "delete" => service.delete_thread(key),
            _ => service.perform_action(key, action),
        }
    }

    fn sidebar_menu(
        &self,
        keys: &[ThreadKey],
        opened_at: i64,
        bulk: bool,
    ) -> (ThreadActionMenu, Vec<SnoozePreset>) {
        let service = self.thread_actions();
        let format = self
            .composer_preferences()
            .snapshot()
            .timestamp_format
            .clone();
        crate::screens::settings::snooze::with_snooze_calendar(opened_at, &format, |calendar| {
            let first = &keys[0];
            let menu = if bulk {
                service.sidebar_batch_menu(keys, calendar)
            } else {
                service.sidebar_action_menu(first, calendar)
            };
            let presets = service.resolve_snooze_presets(&first.environment, calendar);
            (menu.into(), presets)
        })
    }

    fn sidebar_batch(
        &self,
        keys: Vec<ThreadKey>,
        id: String,
        until: Option<WireDateTime>,
    ) -> LocalBoxFuture<'static, Result<BatchReport, ClientError>> {
        self.thread_actions().perform_sidebar_batch(keys, id, until)
    }

    fn preset(
        &self,
        key: &ThreadKey,
        id: &str,
        shown: &[SnoozePreset],
    ) -> Result<WireDateTime, ClientError> {
        use crate::screens::settings::snooze::with_snooze_calendar;
        let service = self.thread_actions();
        let format = self
            .composer_preferences()
            .snapshot()
            .timestamp_format
            .clone();
        with_snooze_calendar(nsh_core::time::unix_ms() as i64, &format, |calendar| {
            service.pick_snooze_preset(&key.environment, shown, id, calendar)
        })
        .map(|preset| WireDateTime(preset.snoozed_until))
    }

    fn snooze(
        &self,
        key: ThreadKey,
        until: WireDateTime,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.thread_actions().snooze_thread(key, until)
    }

    fn undo_notices(&self) -> LocalBoxStream<'static, Option<UndoNotice>> {
        self.thread_actions().thread_undo_notice_changes()
    }

    fn undo(&self) -> LocalBoxFuture<'static, UndoReport> {
        self.thread_actions().undo_latest_thread_actions()
    }

    fn project(&self, key: &ThreadKey) -> Option<ProjectKey> {
        let row = self.thread_actions().thread_shell(key)?;
        Some(ProjectKey {
            environment: key.environment.clone(),
            project: row.project_id.clone(),
        })
    }

    fn can_drag_thread(&self, key: &ThreadKey) -> bool {
        self.thread_actions().can_drag_thread(key)
    }

    fn can_sidebar_drop(
        &self,
        drop: &t3_client::thread_views::thread_ops::actions::SidebarDrop,
    ) -> bool {
        self.thread_actions().can_sidebar_drop(drop)
    }

    fn sidebar_drop(
        &self,
        drop: t3_client::thread_views::thread_ops::actions::SidebarDrop,
    ) -> LocalBoxFuture<'static, Result<bool, ClientError>> {
        self.thread_actions().perform_sidebar_drop(drop)
    }

    fn sidebar_preferences(
        &self,
    ) -> LocalBoxStream<'static, Rc<t3_contracts::settings::ClientSettingsSchema>> {
        let preferences = self.composer_preferences();
        let first = preferences.snapshot();
        Box::pin(
            futures::stream::once(futures::future::ready(first))
                .chain(preferences.changes().map(move |()| preferences.snapshot())),
        )
    }

    fn can_pin_project(&self, keys: &[ProjectKey]) -> bool {
        self.thread_actions().can_organize_projects(keys)
    }

    fn pin_project(
        &self,
        keys: Vec<ProjectKey>,
        pinned: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.thread_actions().set_project_group_pinned(keys, pinned)
    }

    fn project_draft(
        &self,
        project: ProjectKey,
    ) -> LocalBoxFuture<'static, Result<String, ClientError>> {
        let new_threads = self.new_threads();
        Box::pin(async move { new_threads.open_project(project, None, None).await })
    }

    fn claims_undo_shortcut(&self) -> bool {
        self.thread_actions().thread_undo_claims_shortcut()
    }

    fn release_undo_shortcut(&self) {
        self.thread_actions().release_thread_undo_shortcut();
    }

    fn can_still_show(&self, key: &ThreadKey) -> bool {
        self.thread_actions()
            .thread_shell(key)
            .is_some_and(|thread| thread.archived_at.is_none() && thread.deleted_at.is_none())
    }

    fn action_moves_thread(&self, key: &ThreadKey, id: &str) -> bool {
        let service = self.thread_actions();
        match id {
            "settle" => service
                .thread_shell(key)
                .is_some_and(|row| row.settled_override == Some(AppThreadSettledOverride::Settled)),
            "snooze" => service.snooze_menu(key).snoozed,
            "archive" | "delete" => !self.can_still_show(key),
            _ => false,
        }
    }

    fn rename(
        &self,
        key: ThreadKey,
        title: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.thread_actions().rename_thread(key, title)
    }
}

/// Rows use the host's client; a mount over another client (a fake environment) provides it.
#[derive(Clone)]
pub struct RowThreadActionsRuntime(pub std::rc::Rc<dyn RowThreadActions>);

pub(super) fn thread_actions() -> Option<std::rc::Rc<dyn RowThreadActions>> {
    use_context::<RowThreadActionsRuntime>()
        .map(|runtime| runtime.0)
        .or_else(|| {
            rt().backend()
                .client()
                .map(|client| std::rc::Rc::new(client) as std::rc::Rc<dyn RowThreadActions>)
        })
}
type Project = Memo<Option<Shared<OrchestrationProjectShell>>>;

/// Supplied by the sidebar's selection/draft/snooze stores, never inferred from run state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowExtras {
    pub selected: bool,
    pub unsent_draft: bool,
    pub woke_at: Option<String>,
    pub snooze_wake_label: Option<String>,
    pub local_last_visited_at: Option<String>,
    pub running_terminal_ids: Vec<String>,
    pub scratch_machine_label: Option<String>,
    /// Environment clock converted by t3-client and refreshed by the shared second tick.
    pub server_now_ms: Option<i64>,
    pub pending_subagent_request: Option<PendingSubagentRequest>,
    pub environment_label: Option<String>,
}

/// Only the fields the row reads; provider status checks cannot rebuild its glyphs.
#[derive(Clone, Debug, PartialEq)]
struct RowProvider {
    pub instance_id: String,
    pub display_name: String,
    pub driver: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopStatusIcon {
    Working,
    Waiting,
    Input,
    Request,
    Approval,
    Failed,
    Done,
    Woke,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TopStatus {
    pub label: &'static str,
    pub icon: TopStatusIcon,
    pub class_name: &'static str,
}

/// The component's label precedence, after resolveSidebarThreadStatus.
pub fn top_status(
    status: Status,
    goal_active: bool,
    woke: bool,
    unread: bool,
) -> Option<TopStatus> {
    let (label, icon, class_name) = match status {
        Status::Preparing => ("Setting up", TopStatusIcon::Working, "text-info-foreground"),
        Status::Working => (
            if goal_active { "Goal" } else { "Working" },
            TopStatusIcon::Working,
            "text-info-foreground",
        ),
        Status::Waiting => ("Waiting", TopStatusIcon::Waiting, "text-muted-foreground"),
        Status::Approval => (
            "Approval",
            TopStatusIcon::Approval,
            "text-warning-foreground",
        ),
        Status::Input => (
            "Input",
            TopStatusIcon::Input,
            "text-indigo-600 dark:text-indigo-300",
        ),
        Status::Request => ("Request", TopStatusIcon::Request, "text-guest-foreground"),
        Status::Limited => ("Limited", TopStatusIcon::Failed, "text-warning-foreground"),
        Status::Failed => ("Failed", TopStatusIcon::Failed, "text-error-foreground"),
        Status::Ready if woke => ("Woke", TopStatusIcon::Woke, "text-warning-foreground"),
        Status::Ready if unread => ("Done", TopStatusIcon::Done, "text-success-foreground"),
        Status::Ready => return None,
    };
    Some(TopStatus {
        label,
        icon,
        class_name,
    })
}

#[derive(Clone, Debug, PartialEq)]
struct Presentation {
    status: Status,
    top: Option<TopStatus>,
    unread: bool,
    woke: bool,
    receded: bool,
}

fn presentation(thread: &EnvironmentThreadShell, extras: &RowExtras, active: bool) -> Presentation {
    let input = ThreadInput::from_environment_shell(
        thread,
        extras.local_last_visited_at.as_deref(),
        &timestamp_ms,
    );
    let status = status::resolve_status(&input.status);
    let unread = status::has_unseen_completion(&input.status, &timestamp_ms);
    let woke = extras
        .woke_at
        .as_deref()
        .and_then(timestamp_ms)
        .is_some_and(|wake| {
            thread.source.settled_override != Some(AppThreadSettledOverride::Settled)
                && input
                    .status
                    .last_visited_at
                    .and_then(timestamp_ms)
                    .is_none_or(|visit| visit < wake)
        });
    let goal_active = thread
        .source
        .goal
        .as_ref()
        .and_then(Option::as_ref)
        .is_some_and(|goal| goal.status == OrchestrationV2ProviderGoalStatus::Active);
    Presentation {
        status,
        top: top_status(status, goal_active, woke, unread),
        unread,
        woke,
        receded: status::should_recede(status, unread, woke, active, extras.selected),
    }
}

fn working_duration_label(
    thread: &EnvironmentThreadShell,
    status: Status,
    host_now: Option<i64>,
) -> String {
    let started = match status {
        Status::Preparing | Status::Working => {
            resolve_thread_working_started_at(thread.latest_run.as_ref(), thread.runtime.as_ref())
        }
        Status::Waiting => thread
            .latest_run
            .as_ref()
            .and_then(|run| run.completed_at.as_ref())
            .map(|at| at.0.as_str()),
        _ => None,
    }
    .and_then(timestamp_ms);
    started
        .zip(host_now)
        .map(|(started, now)| format_working_duration_label((now as f64) - (started as f64)))
        .unwrap_or_default()
}

/// compactSidebarTimeLabel(formatRelativeTimeLabel(..)).
pub fn relative_label(iso: &str, now_ms: i64) -> String {
    let Some(date) = timestamp_ms(iso) else {
        return String::new();
    };
    let label = format_relative_elapsed(relative_time_elapsed_ms(date, now_ms)).value;
    compact_sidebar_time_label(&label, "just now", &t("now"))
}

fn compact_sidebar_time_label(label: &str, just_now: &str, now: &str) -> String {
    if label == just_now {
        return now.to_owned();
    }
    label.strip_suffix(" ago").unwrap_or(label).to_owned()
}

fn class(classes: &str) -> SharedString {
    tw_with(Scope::Sidebar, classes, &[])
}

/// RelativeTimeLabel schedules only the next boundary returned by the client.
fn relative_time_label(timestamp: impl Fn() -> Option<String> + 'static) -> impl IntoElement {
    let now = signal(nsh_core::time::unix_ms() as i64);
    let timestamp = computed(timestamp);
    // The timer belongs to the effect: NSH cancels it when the effect runs again or the row goes.
    effect(move || {
        let at = timestamp.get().as_deref().and_then(timestamp_ms);
        let current = now.get();
        if let Some(at) = at {
            let delay = ms_until_relative_time_change(at, current).max(1) as u64;
            set_timeout(Duration::from_millis(delay), move || {
                now.set(nsh_core::time::unix_ms() as i64);
            });
        }
    });
    text(move || {
        SharedString::from(
            timestamp
                .get()
                .as_deref()
                .map(|at| relative_label(at, now.get()))
                .unwrap_or_default(),
        )
    })
}

// Runtime has no config selector yet. Observe only config publications, outside render bindings.
fn environment_config(key: Memo<Option<ThreadKey>>) -> Memo<Option<Shared<ServerConfig>>> {
    let config = signal(None);
    if let (Some(runtime), Some(key)) = (use_context::<Rt>(), key.get_untracked()) {
        let store = runtime.store.store();
        let environment = key.environment;
        let mut changes = store.subscribe_environment(&environment);
        watch(async move {
            loop {
                config.set(
                    store
                        .snapshot()
                        .environments
                        .get(&environment)
                        .and_then(|state| state.config.clone())
                        .map(Shared),
                );
                loop {
                    let Some(change) = changes.next().await else {
                        return;
                    };
                    if change
                        .scopes
                        .iter()
                        .any(|scope| matches!(scope, StoreScope::All | StoreScope::Environment(_)))
                    {
                        break;
                    }
                }
            }
        });
    }
    computed(move || config.get())
}

fn row_providers(config: Option<&ServerConfig>) -> Vec<RowProvider> {
    config
        .map(|config| {
            entries_for(&config.providers.0, &config.settings, None)
                .into_iter()
                .map(|entry| RowProvider {
                    instance_id: entry.snapshot.instance_id.0.clone(),
                    display_name: entry.display_name,
                    driver: entry.snapshot.driver.0.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

// SidebarThreadRow.tsx currently returns null for every thread; no diff count can be shown.
fn latest_run_diff(_thread: &OrchestrationV2ThreadShell) -> Option<(u64, u64)> {
    None
}

fn meta_line_empty(thread: Option<&OrchestrationV2ThreadShell>, extras: &RowExtras) -> bool {
    thread.is_none_or(|thread| {
        thread.branch.is_none()
            && extras.scratch_machine_label.is_none()
            && thread
                .linked_pull_request
                .as_ref()
                .and_then(Option::as_ref)
                .is_none()
            && thread
                .pull_requests
                .as_ref()
                .and_then(Option::as_ref)
                .is_none_or(Vec::is_empty)
            && latest_run_diff(thread).is_none()
            && extras.running_terminal_ids.is_empty()
    })
}

pub fn thread_row(item: Memo<Option<SidebarItem>>, nav: Navigator) -> impl IntoElement {
    // SB-LIST has not exposed its draft/selection/wake observations through SidebarItem yet.
    thread_row_with_extras(item, computed(RowExtras::default), nav)
}

pub fn thread_row_with_extras(
    item: Memo<Option<SidebarItem>>,
    extras: Memo<RowExtras>,
    nav: Navigator,
) -> impl IntoElement {
    let runtime = rt();
    let identity = computed(move || item.with(|i| i.as_ref().map(|i| (i.key.clone(), i.variant))));
    dynamic(move || {
        identity.get().map(|(key, _)| {
            untracked(move || {
                let shell = runtime.store.shell(key.environment.clone());
                let id = key.thread;
                let thread = computed(move || {
                    shell.with(|shell| {
                        shell.as_ref().and_then(|shell| {
                            shell
                                .threads
                                .iter()
                                .find(|row| row.id.0 == id.0)
                                .cloned()
                                .map(Shared)
                        })
                    })
                });
                let project = computed(move || {
                    thread.with(|thread| {
                        thread.as_ref().and_then(|thread| {
                            shell.with(|shell| {
                                shell.as_ref().and_then(|shell| {
                                    shell
                                        .projects
                                        .iter()
                                        .find(|p| p.id == thread.project_id)
                                        .cloned()
                                        .map(Shared)
                                })
                            })
                        })
                    })
                });
                sidebar_thread_row(item, thread, project, extras, nav).into_element()
            })
        })
    })
}

pub fn sidebar_thread_row(
    item: Memo<Option<SidebarItem>>,
    thread: Thread,
    project: Project,
    extras: Memo<RowExtras>,
    nav: Navigator,
) -> impl IntoElement {
    let key = computed(move || item.with(|i| i.as_ref().map(|i| i.key.clone())));
    let interactions = use_context::<super::selection::Interactions>();
    let menu_open = signal(false);
    let opened_at = signal(nsh_core::time::unix_ms() as i64);
    effect(move || {
        if menu_open.get() {
            opened_at.set(nsh_core::time::unix_ms() as i64);
        }
    });
    let targets = move || {
        let key = key.get()?;
        Some(interactions.map_or_else(
            || (vec![key.clone()], false),
            |interactions| interactions.targets(&key),
        ))
    };
    // SB-ACTIONS-1's thread menu over the client's operations (thread_ops binding). The menu's
    // availability follows the thread's shell (pin, settle, unread), which this memo reads.
    let client = thread_actions();
    let menu_client = client.clone();
    let prepared_menu = computed(move || {
        thread.with(|_| ());
        let client = menu_client.as_ref()?;
        let (keys, bulk) = targets()?;
        (!keys.is_empty()).then(|| client.sidebar_menu(&keys, opened_at.get(), bulk))
    });
    let menu =
        computed(move || prepared_menu.with(|menu| menu.as_ref().map(|(menu, _)| menu.clone())));
    // The rename input and the action menu go as their work starts; the row outlives both.
    let tasks = interactions.map_or_else(TaskOwner::here, |interactions| interactions.tasks);
    let renaming = signal(false);
    let rename_draft = signal(String::new());
    let config = environment_config(key);
    let providers = computed(move || {
        config.with(|config| row_providers(config.as_ref().map(|config| config.0.as_ref())))
    });
    let pending_request = computed(move || extras.with(|extras| extras.pending_subagent_request));
    let project_thread = move || {
        thread.with(|thread| {
            thread.as_ref().and_then(|thread| {
                key.get().map(|key| {
                    present_thread_shell(key.environment, thread.0.clone(), pending_request.get())
                })
            })
        })
    };
    // The model is shared by independent status, title, tooltip and provider bindings.
    // Give them a settled snapshot: pulling stale shell memos here can retire this cell.
    let model = signal(untracked(project_thread));
    effect(move || model.set(project_thread()));
    let model = model.read_only();
    // A navigation can recycle this cell as a shelf marker. Read selection from the
    // same projected row as its content so the retired row cannot react to navigation.
    let active = move || item.with(|item| item.as_ref().is_some_and(|row| row.active));
    let is_selected = computed(move || extras.with(|extras| extras.selected));
    let has_draft = computed(move || extras.with(|extras| extras.unsent_draft) && !active());
    let observations = computed(move || {
        extras.with(|extras| RowExtras {
            selected: extras.selected,
            woke_at: extras.woke_at.clone(),
            local_last_visited_at: extras.local_last_visited_at.clone(),
            ..Default::default()
        })
    });
    let snooze_wake_label =
        computed(move || extras.with(|extras| extras.snooze_wake_label.clone()));
    let presented = computed(move || {
        model.with(|model| {
            model
                .as_ref()
                .map(|model| presentation(model, &observations.get(), active()))
        })
    });
    let card = move || item.with(|i| i.as_ref().is_some_and(|i| i.variant == RowVariant::Card));
    let title = move || {
        model
            .with(|model| model.as_ref().map(|model| model.title.clone()))
            .unwrap_or_else(|| {
                item.with(|i| i.as_ref().map(|i| i.title.clone()).unwrap_or_default())
            })
    };
    let project_name =
        move || item.with(|i| i.as_ref().map(|i| i.project.clone()).unwrap_or_default());
    let settled = move || {
        thread.with(|thread| {
            thread.as_ref().is_some_and(|thread| {
                thread.settled_override == Some(AppThreadSettledOverride::Settled)
            })
        })
    };
    let surface = move || {
        Some(class(&cn([
        Some("group/sidebar-row relative w-full cursor-pointer overflow-hidden rounded-md text-left outline-none select-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"),
        Some(if active() {
            "bg-sidebar-row-active text-sidebar-foreground light:ring-1 light:ring-inset light:ring-sidebar-border/70"
        } else if is_selected.get() {
            "bg-sidebar-row-selected text-sidebar-foreground light:ring-1 light:ring-inset light:ring-sidebar-border/70"
        } else if has_draft.get() {
            "bg-warning/4 hover:bg-warning/8 text-sidebar-foreground"
        } else if presented.get().is_some_and(|p| p.receded) {
            "text-sidebar-muted-foreground/75 hover:bg-sidebar-row-hover hover:text-sidebar-foreground"
        } else {
            "bg-transparent text-sidebar-foreground hover:bg-sidebar-row-hover"
        }),
        presented.get().filter(|p| p.receded && matches!(p.status, Status::Preparing | Status::Working)).map(|_| "opacity-70 transition-opacity hover:opacity-100 focus-within:opacity-100 motion-reduce:transition-none"),
        (!card()).then_some("flex h-9 items-center gap-2.5 px-2.5"),
    ])))
    };
    let action_client = client.clone();
    let archive_return = global::<super::selection::ArchiveReturn>();
    let on_action = move |id: String| {
        let (Some(client), Some((keys, bulk))) = (action_client.clone(), untracked(targets)) else {
            return;
        };
        if keys.is_empty() {
            return;
        }
        if id == "rename" {
            rename_draft.set(title());
            renaming.set(true);
            return;
        }
        let choice = match id.strip_prefix("snooze:") {
            Some("custom") => {
                let Some(choice) = super::snooze::request() else {
                    return;
                };
                Some(choice)
            }
            Some(preset) => {
                let until = prepared_menu.with_untracked(|menu| {
                    let shown = menu.as_ref().map_or(&[][..], |(_, shown)| shown.as_slice());
                    client.preset(&keys[0], preset, shown)
                });
                match until {
                    Ok(until) => {
                        Some(Box::pin(futures::future::ready(Some(until))) as super::snooze::Choice)
                    }
                    Err(error) => {
                        super::actions::action_failed("snooze", error);
                        return;
                    }
                }
            }
            None => None,
        };
        let selection_revision = interactions.map(|interactions| interactions.store.revision());
        tasks.spawn(async move {
            let (id, until) = if let Some(choice) = choice {
                let Some(until) = choice.await else {
                    return;
                };
                (String::from("snooze"), Some(until))
            } else {
                (id, None)
            };
            let mut navigation =
                interactions.and_then(|interactions| interactions.plan_navigation(nav, &id, &keys));
            if let Some(plan) = &mut navigation {
                plan.project = plan.requested.thread().and_then(|key| client.project(key));
            }
            let archive = navigation
                .as_ref()
                .filter(|_| id == "archive")
                .and_then(|plan| {
                    let key = plan.requested.thread()?.clone();
                    Some((plan.requested.clone(), plan.project.clone()?, key))
                });
            let result = if bulk {
                client.sidebar_batch(keys.clone(), id.clone(), until).await
            } else {
                let key = keys[0].clone();
                let operation = match until {
                    Some(until) => client.snooze(key.clone(), until),
                    None => client.perform(key.clone(), id.clone()),
                };
                operation.await.map(|()| BatchReport {
                    completed: vec![key],
                    ..Default::default()
                })
            };
            match result {
                Ok(report) => {
                    if let Some(interactions) = interactions {
                        interactions.finish(
                            nav,
                            &id,
                            &keys,
                            &report.completed,
                            client.as_ref(),
                            navigation,
                        );
                    }
                    if bulk
                        && !matches!(id.as_str(), "archive" | "delete")
                        && let Some(interactions) = interactions
                        && selection_revision == Some(interactions.store.revision())
                    {
                        interactions.store.remove_from_selection(
                            &report
                                .completed
                                .iter()
                                .map(super::selection::row_key)
                                .collect::<Vec<_>>(),
                        );
                    }
                    if matches!(id.as_str(), "archive" | "unarchive" | "delete") {
                        archive_return.update(|state| {
                            if state
                                .0
                                .as_ref()
                                .is_some_and(|key| report.completed.contains(key))
                            {
                                state.0 = None;
                            }
                        });
                    }
                    if let Some((requested, project, key)) = archive
                        && report.completed.contains(&key)
                    {
                        archive_return.update(|state| state.0 = Some(key.clone()));
                        if nav.top() == requested && !client.can_still_show(&key) {
                            match client.project_draft(project).await {
                                Ok(draft)
                                    if nav.top() == requested && !client.can_still_show(&key) =>
                                {
                                    nav.open(Route::Draft(draft));
                                }
                                Err(error) => super::actions::action_failed("archive-draft", error),
                                _ => {}
                            }
                        }
                    }
                    if bulk {
                        super::actions::batch_failed(&id, report.failed);
                    } else if let Some((_, error)) = report.failed.into_iter().next() {
                        super::actions::action_failed(&id, error);
                    }
                }
                Err(error) => super::actions::action_failed(&id, error),
            }
        });
    };
    let rename_client = client.clone();
    // Stored, so the title closure stays Copy for both row layouts.
    let commit_rename: Stored<std::rc::Rc<dyn Fn()>> = stored(std::rc::Rc::new(move || {
        renaming.set(false);
        let (Some(client), Some(key)) = (rename_client.clone(), key.get_untracked()) else {
            return;
        };
        let title = rename_draft.get_untracked();
        tasks.spawn(async move {
            if let Err(error) = client.rename(key, title).await {
                super::actions::action_failed("rename", error);
            }
        });
    }));
    let title_text = move || {
        text(move || SharedString::from(title())).app_class(move || {
        let p = presented.get();
        let receded = p.as_ref().is_some_and(|p| p.receded);
        Some(class(&cn([
            Some("min-w-0 flex-1 text-start text-sm transition-opacity motion-reduce:transition-none"),
            Some(if receded { "font-normal" } else { "font-medium" }),
            Some(if card() {
                if receded { "truncate text-foreground/75 group-hover/sidebar-row:text-foreground group-focus-visible/sidebar-row:text-foreground" }
                else if p.as_ref().is_some_and(|p| p.status == Status::Failed && !p.unread && !p.woke) { "truncate text-foreground/95" }
                else { "truncate text-foreground" }
            } else if receded { "truncate group-focus-within/sidebar-row:text-foreground group-hover/sidebar-row:text-foreground text-secondary-label" }
            else if active() || p.as_ref().is_some_and(|p| p.woke || matches!(p.status, Status::Input | Status::Request)) { "truncate group-focus-within/sidebar-row:text-foreground group-hover/sidebar-row:text-foreground text-foreground" }
            else if p.as_ref().is_some_and(|p| p.unread) { "truncate group-focus-within/sidebar-row:text-foreground group-hover/sidebar-row:text-foreground text-muted-foreground" }
            else { "truncate group-focus-within/sidebar-row:text-foreground group-hover/sidebar-row:text-foreground text-secondary-label/70" }),
            thread.with(|thread| thread.as_ref().is_some_and(|thread| thread.title_regeneration.as_ref().is_some_and(Option::is_some))).then_some("opacity-55"),
        ])))
    })
    };
    // SidebarThreadRow.tsx: the title becomes ThreadRenameInput for one rename session.
    let title_element = move || {
        dynamic(move || {
            let commit_rename = commit_rename.with(|commit| commit.clone());
            Some(if renaming.get() {
                untracked(|| {
                    super::actions::thread_rename_input(
                        rename_draft,
                        move || commit_rename(),
                        move || renaming.set(false),
                    )
                    .into_element()
                })
            } else {
                untracked(|| title_text().into_element())
            })
        })
    };
    let meta_empty = move || {
        thread.with(|thread| {
            extras.with(|extras| {
                meta_line_empty(thread.as_ref().map(|thread| thread.0.as_ref()), extras)
            })
        })
    };
    let age = move || {
        relative_time_label(move || {
            let activity = item.with(|i| i.as_ref().and_then(|i| i.activity_at.clone()));
            if !card() && settled() {
                thread
                    .with(|thread| {
                        thread.as_ref().and_then(|thread| {
                            thread
                                .settled_at
                                .as_ref()
                                .filter(|at| timestamp_ms(&at.0).is_some())
                                .map(|at| at.0.clone())
                        })
                    })
                    .or(activity)
            } else {
                activity
            }
        })
    };
    let details = tooltip_create_handle();
    let card_body = move || {
        view()
        .app_class(move || Some(class(&cn([
            Some("relative z-10 h-[4.875rem] px-(--sidebar-row-content-inset) py-(--sidebar-content-inset)"),
            meta_empty().then_some("h-auto"),
        ]))))
        .child(view().app_class(class("flex h-5 min-w-0 items-center gap-1.5"))
            .child(when(move || has_draft.get(), || {
                view().role(AccessibilityRole::Text).label(t("Unsent draft")).child(icon(Icon::SquarePen, 12.0, move || palette().warning_foreground))
            }))
            .child(project_favicon(project, key))
            .child(pin_indicator(thread))
            .child(text(move || SharedString::from(project_name())).app_class(class("min-w-0 flex-1 truncate font-normal text-secondary-label text-xs")))
            .child(view().app_class(class("group/sidebar-status-slot relative ml-auto flex h-5 min-w-8 shrink-0 items-stretch justify-end text-xs"))
                // NSH has no focus-visible descendant selector; group hover swaps controls.
                .child(view().app_class(move || Some(class(&cn([
                    Some(if presented.get().is_some_and(|p| p.top.is_some_and(|top| top.icon == TopStatusIcon::Woke)) { "pointer-events-auto" } else { "pointer-events-none group-any-hover/sidebar-row:absolute group-any-hover/sidebar-row:right-0 group-any-hover/sidebar-row:opacity-0" }),
                    Some("flex items-center self-center justify-self-end tabular-nums text-secondary-label"),
                ]))))
                    .child(dynamic(move || presented.get().and_then(|p| p.top).map(|top| {
                        if top.icon == TopStatusIcon::Woke { return woke_button(false).into_element(); }
                        view().app_class(class(&cn([Some("inline-flex items-center gap-1 font-medium"), Some(top.class_name)])))
                            .child(top_status_icon(top)).child(text(crate::i18n::Text::from(top.label).resolve()))
                            .child(text(move || SharedString::from(model.with(|model| model.as_ref().map(|model| working_duration_label(model, presented.get().map_or(Status::Ready, |p| p.status), extras.with(|extras| extras.server_now_ms))).unwrap_or_default()))).app_class(class("tabular-nums")))
                            .into_element()
                    })))
                    .child(when(move || presented.get().is_none_or(|p| p.top.is_none()), age)))
))
        .child(view().app_class(class("mt-1 flex min-w-0 items-center gap-1.5")).child(title_element())
            .child(when(meta_empty, move || sidebar_provider_stack(model, providers))))
        .child(when(move || !meta_empty(), move || meta_line(thread, model, extras, providers)))
    };
    let slim_body = move || {
        dynamic(move || {
            untracked(|| {
                vec![
        view().app_class(move || Some(class(&cn([
            Some("flex shrink-0 items-center transition-opacity"),
            (!active() || settled()).then_some("opacity-40 grayscale group-focus-within/sidebar-row:opacity-100 group-focus-within/sidebar-row:grayscale-0 group-hover/sidebar-row:opacity-100 group-hover/sidebar-row:grayscale-0"),
        ])))).child(project_favicon(project, key)).into_element(),
        when(move || has_draft.get(), || view().role(AccessibilityRole::Text).label(t("Unsent draft")).child(icon(Icon::SquarePen, 12.0, move || palette().warning_foreground))).into_element(),
        title_element().into_element(), pin_indicator(thread).into_element(),
        view().app_class(class("relative ml-auto flex h-6 min-w-8 shrink-0 items-center justify-end"))
            .child(view().app_class(move || Some(class(&cn([Some("inline-flex justify-end tabular-nums text-secondary-label transition-opacity text-xs"), (!presented.get().is_some_and(|p| p.woke)).then_some("group-any-hover/sidebar-row:opacity-0")]))))
                .child(when(move || snooze_wake_label.get().is_some(), move || view().app_class(class("inline-flex items-center gap-1 text-xs text-info-foreground tabular-nums")).child(icon_with_class(Icon::AlarmClock, "size-3 text-info-foreground")).child(text(move || SharedString::from(snooze_wake_label.get().unwrap_or_default())))).otherwise(move || when(move || presented.get().is_some_and(|p| p.woke), || woke_button(true)).otherwise(age))))
.into_element(),
    ]
            })
        })
    };
    // list-none/content-visibility/contain-intrinsic-size are native list/virtualisation hints.
    view()
        .app_class(move || Some(class(if card() { "py-0.5" } else { "" })))
        .tooltip(tooltip(
            details,
            TooltipOptions {
                variant: crate::ui::tooltip::Variant::Glass,
                side: Placement::Right,
                align: Align::Start,
                ..Default::default()
            },
            sidebar_thread_tooltip(model, project_name, extras, providers),
        ))
        .child(super::actions::sidebar_thread_context_menu_with_open(
            view()
                .role(AccessibilityRole::Button)
                .focusable(true)
                .test_id(if untracked(card) {
                    "sidebar-row-card"
                } else {
                    "sidebar-row-slim"
                })
                .label(move || {
                    SharedString::from(
                        [
                            Some(title()),
                            presented.get().and_then(|p| {
                                p.top
                                    .map(|s| crate::i18n::Text::from(s.label).resolve().to_string())
                            }),
                            Some(project_name()),
                        ]
                        .into_iter()
                        .flatten()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(", "),
                    )
                })
                .accessibility_state(move || nsh_core::AccessibilityState {
                    selected: active() || is_selected.get(),
                    ..Default::default()
                })
                .on_press(move |event: &PressEvent| {
                    let Some(key) = key.get_untracked() else {
                        return;
                    };
                    if let Some(interactions) = interactions {
                        interactions.press(key, nav, event);
                    } else {
                        nav.open(Route::Thread(key));
                    }
                })
                .app_class(surface)
                .child(when(card, card_body).otherwise(slim_body)),
            menu,
            on_action,
            Some(menu_open),
        ))
}

fn pin_indicator(thread: Thread) -> impl IntoElement {
    when(
        move || {
            thread.with(|thread| {
                thread
                    .as_ref()
                    .is_some_and(|thread| thread.pinned_at.as_ref().is_some_and(Option::is_some))
            })
        },
        || {
            view()
                .role(AccessibilityRole::Text)
                .label(t("Pinned"))
                .child(icon_with_class(
                    Icon::Pin,
                    "size-3 shrink-0 text-muted-foreground/65",
                ))
        },
    )
}

fn woke_button(slim: bool) -> impl IntoElement {
    // C1 has no acknowledge-wake operation yet; preserve the source affordance without dispatch.
    nsh_base::button().disabled(true).accessibility_label(Some(t("Dismiss Woke notification").into()))
        .app_class(class(if slim {
            "inline-flex cursor-pointer items-center gap-1 rounded-sm text-xs font-medium text-warning-foreground outline-none hover:underline focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        } else {
            "inline-flex cursor-pointer items-center gap-1 rounded-sm font-medium outline-none hover:underline focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring text-warning-foreground"
        }))
        .child(icon_with_class(Icon::AlarmClock, if slim { "size-3" } else { "size-4 shrink-0" }))
        .child(text(t("Woke")))
}

fn meta_line(
    thread: Thread,
    model: ReadSignal<Option<EnvironmentThreadShell>>,
    extras: Memo<RowExtras>,
    providers: Memo<Vec<RowProvider>>,
) -> impl IntoElement {
    view()
        .app_class(class(
            "mt-0.5 flex min-w-0 items-center gap-1.5 text-secondary-label text-xs",
        ))
        .child(when(
            move || thread.with(|t| t.as_ref().is_some_and(|t| t.worktree_path.is_some())),
            || icon_with_class(Icon::FolderGit2, "size-3.5 shrink-0 text-icon-muted"),
        ))
        // UI-TRUNCATE has no MiddleTruncate yet; the tooltip retains the full branch.
        .child(
            view().app_class(class("flex min-w-0 flex-1")).child(
                text(move || {
                    SharedString::from(
                        thread
                            .with(|thread| {
                                thread.as_ref().and_then(|thread| {
                                    thread.branch.as_ref().map(|branch| branch.0.clone())
                                })
                            })
                            .or_else(|| extras.with(|extras| extras.scratch_machine_label.clone()))
                            .unwrap_or_default(),
                    )
                })
                .app_class(class("min-w-0 truncate")),
            ),
        )
        // Terminal reveal needs the client's session registry; preserve its meta slot meanwhile.
        // ThreadPullRequestBadgeControl is a later package; no substitute PR control.
        .child(
            view()
                .app_class(class(
                    "pointer-events-none ml-auto inline-flex shrink-0 items-center gap-1",
                ))
                .child(sidebar_provider_stack(model, providers)),
        )
}

fn sidebar_provider_stack(
    model: ReadSignal<Option<EnvironmentThreadShell>>,
    providers: Memo<Vec<RowProvider>>,
) -> impl IntoElement {
    dynamic(move || {
        let model = model.get()?;
        let providers = providers.get();
        let stack = resolve_thread_provider_stack(&model);
        let current = stack.last()?;
        let current_entry = providers
            .iter()
            .find(|entry| entry.instance_id == current.0)?;
        if stack.len() == 1 {
            return Some(
                view()
                    .test_id("sidebar-provider-stack")
                    .app_class(class("inline-flex shrink-0 items-center"))
                    .child(provider_icon(
                        &current_entry.driver,
                        &current_entry.display_name,
                        "size-3.5 opacity-60",
                    )),
            );
        }
        Some(
            view()
                .test_id("sidebar-provider-stack")
                .app_class(class("inline-flex shrink-0 items-center gap-1.5"))
                .child(
                    stack
                        .iter()
                        .take(stack.len().saturating_sub(1))
                        .filter_map(|id| providers.iter().find(|entry| entry.instance_id == id.0))
                        .map(|entry| {
                            provider_icon(
                                &entry.driver,
                                &entry.display_name,
                                "size-3 opacity-35 grayscale",
                            )
                            .into_element()
                        })
                        .collect::<Vec<_>>(),
                )
                .child(
                    view()
                        .app_class(class("relative z-10 inline-flex items-center"))
                        .child(provider_icon(
                            &current_entry.driver,
                            &current_entry.display_name,
                            "size-3.5 opacity-60",
                        )),
                ),
        )
    })
}

fn project_favicon(project: Project, key: Memo<Option<ThreadKey>>) -> impl IntoElement {
    dynamic(move || {
        let project = project.get()?;
        let environment = key.get()?.environment;
        Some(untracked(|| {
            avatar::project_favicon(
                move || ProjectFaviconProject::from_shell(environment.clone(), &project),
                // The runtime has no shared automatic-favicon resource reader yet.
                |_| None,
                "size-4 shrink-0",
                None,
            )
        }))
    })
}

fn sidebar_thread_tooltip(
    model: ReadSignal<Option<EnvironmentThreadShell>>,
    project: impl Fn() -> String + 'static,
    extras: Memo<RowExtras>,
    providers: Memo<Vec<RowProvider>>,
) -> impl IntoElement {
    view().app_class(class("flex min-w-0 max-w-80 flex-col gap-2 px-1 py-2"))
        .child(text(move || SharedString::from(model.with(|model| model.as_ref().map(|model| model.title.clone()).unwrap_or_default()))).app_class(class("min-w-0 truncate text-xs leading-tight font-medium text-foreground")))
        // Flex column replaces CSS grid, preserving sibling order and spacing.
        .child(view().app_class(class("flex flex-col gap-1.5 pl-0.5 text-xs text-muted-foreground"))
            .child(text(move || SharedString::from(project())).app_class(class("min-w-0 truncate text-foreground/75")))
            .child(when(move || model.with(|v| v.as_ref().is_some_and(|v| v.source.branch.is_some())), move || {
                view().app_class(class("flex min-w-0 items-center gap-2"))
                    .child(icon(Icon::GitBranch, 12.0, move || palette().muted_foreground))
                    .child(text(move || SharedString::from(model.with(|v| v.as_ref().and_then(|v| v.source.branch.as_ref().map(|b| b.0.clone())).unwrap_or_default()))))
            }))
            .child(when(move || extras.with(|e| e.environment_label.is_some()), move || text(move || SharedString::from(extras.with(|e| e.environment_label.clone().unwrap_or_default()))).app_class(class("min-w-0 truncate text-foreground/75"))))
            .child(dynamic(move || {
                let model = model.get()?;
                let entries = providers.get();
                let stack = resolve_thread_provider_stack(&model);
                let current = stack.last()?;
                let entry = entries.iter().find(|entry| entry.instance_id == current.0)?;
                Some(view().app_class(class("flex min-w-0 items-center gap-2"))
                    .child(provider_icon(&entry.driver, &entry.display_name, "size-3 shrink-0 grayscale opacity-60"))
                    .child(text(model.source.model_selection.model.clone()).app_class(class("min-w-0 truncate text-foreground/75"))))
            }))
            .child(dynamic(move || {
                let model = model.get()?;
                let entries = providers.get();
                let current = model.runtime.as_ref().map_or(&model.source.model_selection.instance_id, |runtime| &runtime.provider_instance_id);
                let previous = model.source.provider_instance_history.as_ref().and_then(Option::as_ref).into_iter().flatten()
                    .filter(|id| *id != current)
                    .map(|id| entries.iter().find(|entry| entry.instance_id == id.0).map_or_else(|| id.0.clone(), |entry| entry.display_name.clone())).collect::<Vec<_>>();
                (!previous.is_empty()).then(|| text(tr("Handed off from {providers}", &[("providers", Value::Text(previous.join(", ")))]))
                    .app_class(class("min-w-0 truncate text-foreground/75")))
            }))
            .child(when(move || model.with(|v| v.as_ref().is_some_and(|v| v.runtime.as_ref().is_some_and(|r| r.last_error.is_some()))), move || {
                text(move || t(if model.with(|v| v.as_ref().and_then(|v| v.runtime.as_ref()).is_some_and(|r| r.last_error_class == Some(t3_contracts::orchestration_v2::OrchestrationV2ProviderFailureClass::UsageLimit))) { "Usage limit reached" } else { "Error occurred" }))
                    .app_class(move || Some(class(if model.with(|v| v.as_ref().and_then(|v| v.runtime.as_ref()).is_some_and(|r| r.last_error_class == Some(t3_contracts::orchestration_v2::OrchestrationV2ProviderFailureClass::UsageLimit))) { "min-w-0 truncate text-warning" } else { "min-w-0 truncate text-error" })))
            })))
}

fn top_status_icon(top: TopStatus) -> impl IntoElement {
    let which = match top.icon {
        TopStatusIcon::Working => Icon::CircleDashed,
        TopStatusIcon::Waiting => Icon::CircleDotDashed,
        TopStatusIcon::Input => Icon::MessageCircleQuestionMark,
        TopStatusIcon::Request => Icon::UserRound,
        TopStatusIcon::Approval => Icon::ShieldQuestionMark,
        TopStatusIcon::Failed => Icon::CircleAlert,
        TopStatusIcon::Done => Icon::CircleCheck,
        TopStatusIcon::Woke => Icon::AlarmClock,
    };
    icon_with_class(which, "shrink-0 size-4").into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::rc::Rc;
    use t3_client::types::EnvironmentId;

    fn thread(extra: Value) -> EnvironmentThreadShell {
        let mut value = json!({
            "id":"thread", "projectId":"project", "title":"Port the row",
            "providerInstanceId":"codex", "modelSelection":{"instanceId":"codex","model":"gpt-5.4"},
            "runtimeMode":"full-access", "interactionMode":"default", "branch":null,"worktreePath":null,
            "lineage":{"rootThreadId":"thread","parentThreadId":null,"relationshipToParent":null},
            "forkedFrom":null,"activeProviderThreadId":null,"latestRunId":null,"activeRunId":null,
            "status":"idle","pendingRuntimeRequest":null,"latestVisibleMessage":null,"latestUserMessageAt":null,
            "hasActionableProposedPlan":false,"itemCount":0,"visibleItemCount":0,
            "createdBy":"user","creationSource":"web","createdAt":"2026-11-13T10:00:00.000Z",
            "updatedAt":"2026-11-13T12:00:00.000Z","archivedAt":null,"settledOverride":null,"settledAt":null,"deletedAt":null
        });
        value
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        present_thread_shell(
            EnvironmentId("host".into()),
            Rc::new(serde_json::from_value(value).expect("shell")),
            None,
        )
    }

    #[test]
    fn status_priority_and_component_labels_match_sidebar_thread_row() {
        for (status, goal, woke, unread, label) in [
            (Status::Preparing, false, true, true, "Setting up"),
            (Status::Working, false, true, true, "Working"),
            (Status::Working, true, true, true, "Goal"),
            (Status::Waiting, false, true, true, "Waiting"),
            (Status::Approval, false, true, true, "Approval"),
            (Status::Input, false, true, true, "Input"),
            (Status::Request, false, true, true, "Request"),
            (Status::Limited, false, true, true, "Limited"),
            (Status::Failed, false, true, true, "Failed"),
            (Status::Ready, false, true, true, "Woke"),
            (Status::Ready, false, false, true, "Done"),
        ] {
            assert_eq!(
                top_status(status, goal, woke, unread).expect("label").label,
                label
            );
        }
        assert!(top_status(Status::Ready, false, false, false).is_none());
    }

    #[test]
    fn bridges_the_clients_status_without_losing_requests_or_usage_limits() {
        for (extra, expected) in [
            (
                json!({"latestRunId":"run","status":"running","blockedOnThreadWait":true}),
                "Waiting",
            ),
            (
                json!({"latestRunId":"run","status":"preparing"}),
                "Setting up",
            ),
            (
                json!({"latestRunId":"run","status":"running","pendingGuestMessageCount":1}),
                "Request",
            ),
            (
                json!({"latestRunId":"run","status":"failed","lastErrorClass":"usage_limit"}),
                "Limited",
            ),
            (
                json!({"latestRunId":"run","status":"running","lastErrorClass":"usage_limit"}),
                "Working",
            ),
            (json!({"latestRunId":"run","status":"failed"}), "Failed"),
        ] {
            assert_eq!(
                presentation(&thread(extra), &RowExtras::default(), false)
                    .top
                    .expect("status")
                    .label,
                expected
            );
        }
        let running = thread(json!({"latestRunId":"run","status":"running"}));
        for (approval, input, expected) in [(false, true, "Input"), (true, true, "Approval")] {
            let mut model = running.clone();
            model.has_pending_user_input = input;
            model.has_pending_approvals = approval;
            assert_eq!(
                presentation(&model, &RowExtras::default(), false)
                    .top
                    .expect("status")
                    .label,
                expected
            );
        }
    }

    #[test]
    fn unseen_completion_obeys_server_watermarks_and_legacy_fallback() {
        for (extra, local, unread) in [
            (json!({}), None, false),
            (json!({}), Some("2026-11-13T10:00:00.000Z"), true),
            (
                json!({"lastVisitedAt":null}),
                Some("2026-11-13T10:00:00.000Z"),
                false,
            ),
            (
                json!({"lastVisitedAt":"2026-11-13T12:00:00.000Z"}),
                Some("2026-11-13T10:00:00.000Z"),
                false,
            ),
            (json!({"lastVisitedAt":"invalid"}), None, true),
        ] {
            let mut base = json!({"latestRunId":"run","status":"completed","latestRunCompletedAt":"2026-11-13T11:00:00.000Z"});
            base.as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            let extras = RowExtras {
                local_last_visited_at: local.map(str::to_owned),
                ..Default::default()
            };
            let result = presentation(&thread(base), &extras, false);
            assert_eq!(result.unread, unread);
            assert_eq!(result.receded, !unread);
        }
    }

    #[test]
    fn completed_background_rosters_only_wait_when_awaited() {
        for (kind, awaited, label, receded) in [
            ("command", false, "Done", false),
            ("monitor", false, "Done", false),
            ("command", true, "Waiting", true),
        ] {
            let mut task = json!({"taskId":"background","kind":kind});
            if awaited {
                task["awaited"] = json!(true);
            }
            let model = thread(
                json!({"latestRunId":"run","status":"completed","latestRunCompletedAt":"2026-11-13T11:00:00.000Z","lastVisitedAt":"2026-11-13T10:00:00.000Z","pendingBackgroundTasks":[task]}),
            );
            let result = presentation(&model, &RowExtras::default(), false);
            assert_eq!(result.top.expect("status").label, label);
            assert_eq!(result.receded, receded);
        }
    }

    #[test]
    fn waking_requires_a_newer_unread_wake_and_never_overrides_settled() {
        let extras = RowExtras {
            woke_at: Some("2026-11-13T11:00:00.000Z".into()),
            ..Default::default()
        };
        for (extra, woke) in [
            (json!({}), true),
            (json!({"lastVisitedAt":"2026-11-13T11:00:00.000Z"}), false),
            (json!({"lastVisitedAt":"invalid"}), true),
            (json!({"settledOverride":"settled"}), false),
        ] {
            assert_eq!(presentation(&thread(extra), &extras, false).woke, woke);
        }
        let quiet = thread(json!({}));
        assert!(presentation(&quiet, &RowExtras::default(), false).receded);
        assert!(!presentation(&quiet, &RowExtras::default(), true).receded);
        assert!(
            !presentation(
                &quiet,
                &RowExtras {
                    selected: true,
                    ..Default::default()
                },
                false
            )
            .receded
        );
    }

    #[test]
    fn duration_uses_host_clock_and_never_invents_an_activity_start() {
        let now = timestamp_ms("2026-11-13T11:05:00.000Z").expect("date");
        let completed = thread(
            json!({"latestRunId":"run","activeRunId":"run","status":"completed","latestRunStartedAt":"2026-11-13T11:00:00.000Z","latestRunCompletedAt":"2026-11-13T11:02:00.000Z"}),
        );
        assert_eq!(
            working_duration_label(&completed, Status::Working, Some(now)),
            ""
        );
        assert_eq!(
            working_duration_label(&completed, Status::Waiting, Some(now)),
            "3m"
        );
        for state in ["queued", "cancelled"] {
            let model = thread(
                json!({"latestRunId":"newer","activeRunId":"older","status":state,"activityRunStatus":"running","activityRunStartedAt":"2026-11-13T11:00:00.000Z"}),
            );
            assert_eq!(
                working_duration_label(&model, Status::Working, Some(now)),
                "5m"
            );
            assert_eq!(working_duration_label(&model, Status::Working, None), "");
            assert_eq!(
                working_duration_label(&model, Status::Working, Some(now - 3_600_000)),
                "0s"
            );
            assert_eq!(
                working_duration_label(&model, Status::Working, Some(now + 3_600_000)),
                "1h 5m"
            );
        }
    }

    #[test]
    fn relative_labels_and_boundary_schedule_use_the_clients_rules() {
        let now = timestamp_ms("2026-11-13T12:10:00.000Z").expect("date");
        for (at, label) in [
            ("2026-11-13T12:05:00.000Z", "5m"),
            ("2026-11-13T10:00:00.000Z", "2h"),
            ("2026-11-13T12:09:59.000Z", "now"),
            ("not a date", ""),
        ] {
            assert_eq!(relative_label(at, now), label);
        }
        assert_eq!(ms_until_relative_time_change(now - 59_999, now), 1);
        assert_eq!(
            ms_until_relative_time_change(now - 3_600_001, now),
            3_599_999
        );
        assert_eq!(
            compact_sidebar_time_label("3d ago", "just now", "now"),
            "3d"
        );
    }

    #[test]
    fn metadata_is_retained_for_scratch_machines_and_running_terminals() {
        let model = thread(json!({}));
        assert!(meta_line_empty(Some(&model.source), &RowExtras::default()));
        assert!(!meta_line_empty(
            Some(&model.source),
            &RowExtras {
                scratch_machine_label: Some("Example-PC".into()),
                ..Default::default()
            }
        ));
        assert!(!meta_line_empty(
            Some(&model.source),
            &RowExtras {
                running_terminal_ids: vec!["terminal-1".into()],
                ..Default::default()
            }
        ));
        let branch = thread(json!({"branch":"main"}));
        assert!(!meta_line_empty(
            Some(&branch.source),
            &RowExtras::default()
        ));
    }
}
