//! Port of apps/web/src/components/chat/MessagesTimeline.tsx MessagesTimeline (the list and its
//! row frame). Rows are the client's port of `MessagesTimeline.logic.ts`
//! (`t3_client::thread_views::messages_timeline`), derived from the timeline entries and the
//! thread's run, then split into block cells by TL-BLOCKS-3's list (`blocks::list`), so a long
//! reply is many cells and a streamed token re-renders only the streaming block. Rows read what
//! ChatView hands every row from [`TimelineServices`], built once per thread above the list.
//! TL-VIEWPORT owns following and position memory; requests whose turn item has not arrived and
//! CMP-SEND's queued and failed messages follow the rows inside the same scroller.

use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt;
use nsh_core::prelude::*;
use t3_client::services::models::{present_thread_shell, thread_runtime_is_active};
use t3_client::state::selectors::thread_shell;
use t3_client::thread_views::composer::target::ComposerTarget;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRowsInput, MessagesTimelineRowsProjection, TimelineLatestRun,
    derive_messages_timeline_rows_with_state,
};
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_contracts::base_schemas::RunId;
use t3_contracts::orchestration_v2::OrchestrationV2ThreadShell;
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::settings::{
    ClientSettingsSchema, ScreenReaderAnnouncements, TimestampFormat, ToolCallVisibility,
};

use crate::i18n::Text;
use crate::runtime::services::assets::{AssetUrlBinding, host_asset_urls};
use crate::runtime::{Shared, rt};
use crate::screens::composer::chips::{ChipContext, ChipData, user_message_chip_renderer};
use crate::screens::settings::shell::SettingsRuntime;
use crate::screens::thread::blocks::list::{BlockListOptions, messages_timeline};
use crate::screens::thread::markdown::base::ChatMarkdownSettings;
use crate::screens::thread::rewind::{TimelineRewind, use_timeline_rewind};
use crate::screens::thread::rows::approval::{ApprovalContext, ApprovalRowsState};
use crate::screens::thread::rows::events::EventRowContext;
use crate::screens::thread::rows::groups::WorkGroupsState;
use crate::screens::thread::rows::plan::{PlanContext, PlanRowsState};
use crate::screens::thread::rows::user::ContextChipRenderer;
use t3_client::thread_views::thread_ops::approvals::ApprovalMap;

/// What ChatView passes MessagesTimeline about the run (`latestRun`, `runningRunId`, `isWorking`,
/// `activeTurnStartedAt`), read from the client's presentation of the thread shell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunState {
    latest_run: Option<TimelineLatestRun>,
    running_run_id: Option<String>,
    is_working: bool,
    active_turn_started_at: Option<String>,
}

fn run_state(environment: &EnvironmentId, thread: &OrchestrationV2ThreadShell) -> RunState {
    let view = present_thread_shell(environment.clone(), Rc::new(thread.clone()), None);
    let latest_run = view.latest_run.as_ref().map(|run| TimelineLatestRun {
        run_id: run.run_id.0.clone(),
        status: serde_json::to_value(&run.status)
            .ok()
            .and_then(|status| status.as_str().map(str::to_owned))
            .unwrap_or_default(),
        started_at: run.started_at.as_ref().map(|at| at.0.clone()),
        completed_at: run.completed_at.as_ref().map(|at| at.0.clone()),
        turn_ended: run.turn_ended,
    });
    let runtime = view.runtime.as_ref();
    RunState {
        latest_run,
        running_run_id: runtime.and_then(|r| r.active_run_id.as_ref().map(|id| id.0.clone())),
        is_working: thread_runtime_is_active(runtime),
        active_turn_started_at: runtime
            .and_then(|r| r.activity_started_at.clone().flatten())
            .map(|at| at.0),
    }
}

/// MessagesTimeline's `TimelineRowSharedState` and the client settings it reads, for one thread.
/// Construct once above the list; every row constructor takes it, so no row is built without
/// its actions.
#[derive(Clone)]
pub struct TimelineServices {
    pub key: ThreadKey,
    pub run: Memo<RunState>,
    pub working: Memo<bool>,
    pub rewind: TimelineRewind,
    /// ThreadFind has no production mount yet (TL-FIND); closed is the web's state without find.
    pub find_active: Memo<bool>,
    pub timestamp_format: Memo<TimestampFormat>,
    /// `toolCallVisibility`, with zen shown as hideAll while find is open.
    pub tool_call_visibility: Memo<String>,
    pub screen_reader_announcements: Memo<ScreenReaderAnnouncements>,
    pub math_rendering_enabled: Memo<bool>,
    pub chips: ContextChipRenderer,
    pub assets: Option<AssetUrlBinding>,
    pub groups: WorkGroupsState,
    pub events: EventRowContext,
}

impl TimelineServices {
    pub fn new(key: ThreadKey) -> Self {
        let runtime = rt();
        let thread_id = key.thread.clone();
        let environment = key.environment.clone();
        let run = runtime
            .store
            .shell_state(key.environment.clone(), move |shell| {
                shell
                    .and_then(|shell| thread_shell(shell, &thread_id))
                    .map(|thread| run_state(&environment, thread))
                    .unwrap_or_default()
            });
        let working = computed(move || run.with(|run| run.is_working));
        let settings = client_settings();
        let find_active = computed(|| false);
        let timestamp_format = computed(move || settings.with(|s| s.timestamp_format.clone()));
        let preferred = computed(move || settings.with(|s| s.tool_call_visibility.clone()));
        let tool_call_visibility = computed(move || {
            let preferred = preferred.get();
            let shown = if find_active.get() && preferred == ToolCallVisibility::Zen {
                ToolCallVisibility::HideAll
            } else {
                preferred
            };
            serde_json::to_value(shown)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default()
        });
        let screen_reader_announcements =
            computed(move || settings.with(|s| s.screen_reader_announcements.clone()));
        let math_rendering_enabled = computed(move || settings.with(|s| s.math_rendering_enabled));
        let groups = WorkGroupsState::default();
        let mut events = EventRowContext::new(key.clone(), groups.entries);
        events.timestamp_format = timestamp_format;
        Self {
            rewind: use_timeline_rewind(key.clone()),
            chips: user_message_chip_renderer(ChipContext::for_thread(
                chip_data(&key),
                key.clone(),
            )),
            assets: use_context::<AssetUrlBinding>().or_else(host_asset_urls),
            key,
            run,
            working,
            find_active,
            timestamp_format,
            tool_call_visibility,
            screen_reader_announcements,
            math_rendering_enabled,
            groups,
            events,
        }
    }
}

/// `useClientSettings`: the client's settings, followed while the timeline is mounted. The
/// demo backend has none, so it shows the defaults.
fn client_settings() -> Signal<Rc<ClientSettingsSchema>> {
    let service = use_context::<SettingsRuntime>().or_else(|| {
        rt().backend()
            .client()
            .map(|client| crate::runtime::services::settings::settings(&client))
    });
    let settings = signal_with(
        service
            .as_ref()
            .map_or_else(Rc::default, |service| service.0.client_settings()),
        SignalEquals::Never,
    );
    if let Some(service) = service {
        spawn(async move {
            let mut changes = service.0.changes();
            while changes.next().await.is_some() {
                let _ = settings.try_set(service.0.client_settings());
            }
        });
    }
    settings
}

/// Skill and provider catalogues for sent context chips, read once per mount: a sent chip needs
/// them only to describe a skill, and following the composer would re-read on every token.
fn chip_data(key: &ThreadKey) -> Memo<ChipData> {
    let session = rt()
        .backend()
        .client()
        .map(|client| client.composer_chips(ComposerTarget::Thread(key.clone())));
    let data = untracked(|| {
        session
            .and_then(|session| session.snapshot().ok())
            .map(|view| ChipData {
                skills: view.skills.clone(),
                providers: view.providers.clone(),
                ..Default::default()
            })
            .unwrap_or_default()
    });
    computed(move || data.clone())
}

pub fn timeline(key: ThreadKey) -> impl IntoElement {
    let runtime = rt();
    let services = TimelineServices::new(key.clone());
    let entries = runtime.store.timeline(key.clone());
    // The previous projection: the client reuses unchanged rows and patches a streaming one.
    let previous: Rc<RefCell<Option<MessagesTimelineRowsProjection>>> = Rc::default();
    let groups = services.groups;
    let approvals = ApprovalContext::new(key.clone(), ApprovalRowsState::default());
    let pending_approvals = approvals.pending;
    let run = services.run;
    let visibility = services.tool_call_visibility;
    let rows = computed(move || {
        let run = run.get();
        let mut input = MessagesTimelineRowsInput {
            timeline_entries: entries.with(|t| t.entries.as_ref().clone()),
            latest_run: run.latest_run,
            running_run_id: run.running_run_id,
            is_working: run.is_working,
            active_turn_started_at: run.active_turn_started_at,
            tool_call_visibility: visibility.get(),
            ..Default::default()
        };
        groups.apply_to_input(&mut input);
        // Pending approvals stay outside collapsed work (MessagesTimeline's pendingApprovalRequestIds).
        input.pending_approval_request_ids = pending_approvals.with(|pending| {
            pending
                .0
                .values()
                .filter_map(|pending| match &pending.request.id {
                    IdOrIndex::Id(id) => Some(id.0.clone()),
                    _ => None,
                })
                .collect()
        });
        let next = derive_messages_timeline_rows_with_state(&input, previous.borrow().as_ref());
        let rows = next.rows.clone();
        *previous.borrow_mut() = Some(next);
        Shared(rows)
    });
    thread_activity_announcer(&services, approvals.pending);
    provide_context(ChatMarkdownSettings {
        math_enabled: services.math_rendering_enabled,
    });
    let options = BlockListOptions::for_thread(&services, approvals, plan_context(&key));
    messages_timeline(rows, options)
}

/// Port of ThreadActivityAnnouncer.tsx at its "status" level: working, needs approval and the
/// run's outcome, through the platform's announcer (the web's polite live region). The
/// "messages" and "everything" levels also read replies and steps (`turnDetailAnnouncement.ts`,
/// not ported), so at those levels only the status is announced.
fn thread_activity_announcer(services: &TimelineServices, approvals: Memo<Shared<ApprovalMap>>) {
    use t3_client::thread_views::threads::activity_announcement::{
        ThreadActivitySnapshot, ThreadActivityStatus, ThreadAnnouncementMemory,
        next_thread_announcement,
    };
    let run = services.run;
    let working = services.working;
    let snapshot = computed(move || {
        let request_id = approvals.with(|pending| {
            pending
                .0
                .values()
                .find_map(|pending| match &pending.request.id {
                    IdOrIndex::Id(id) => Some(id.0.clone()),
                    IdOrIndex::Index(_) => None,
                })
        });
        let latest = run.with(|run| run.latest_run.clone());
        ThreadActivitySnapshot {
            status: if request_id.is_some() {
                ThreadActivityStatus::Approval
            } else if working.get() {
                ThreadActivityStatus::Working
            } else {
                ThreadActivityStatus::Idle
            },
            request_id,
            run_id: latest
                .as_ref()
                .map(|run| IdOrIndex::Id(RunId(run.run_id.clone()))),
            run_status: latest
                .and_then(|run| serde_json::from_value(serde_json::Value::String(run.status)).ok()),
        }
    });
    let memory: Stored<Option<ThreadAnnouncementMemory>> = stored(None);
    // Writes to the platform's screen-reader announcer; the first snapshot is the baseline.
    effect(move || {
        let next = snapshot.get();
        let announcement = memory.with(|memory| {
            let (next, announcement) = next_thread_announcement(memory.as_ref(), next);
            *memory = Some(next);
            announcement
        });
        if let Some(announcement) = announcement {
            announce_for_accessibility(Text::client_key(announcement).resolve().to_string());
        }
    });
}

/// TL-PLAN's context, with the plan's workspace root followed from the client (thread_ops).
fn plan_context(key: &ThreadKey) -> PlanContext {
    let mut context = PlanContext::new(key.clone(), PlanRowsState::default());
    if let Some(client) = rt().backend().client() {
        let root = signal(client.plan_services().plan_workspace_root(key));
        let mut roots = client.plan_services().watch_plan_workspace(key.clone());
        spawn(async move {
            while let Some(value) = roots.next().await {
                root.set(value);
            }
        });
        context.workspace_root = computed(move || root.get());
    }
    context
}
