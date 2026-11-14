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

use crate::i18n::{Text, Value, t, tr};
use crate::runtime::services::assets::{AssetUrlBinding, host_asset_urls};
use crate::runtime::store::Timeline;
use crate::runtime::{Shared, rt};
use crate::screens::composer::chips::{ChipContext, ChipData, user_message_chip_renderer};
use crate::screens::settings::shell::SettingsRuntime;
use crate::screens::thread::blocks::list::{BlockListOptions, messages_timeline};
use crate::screens::thread::markdown::base::ChatMarkdownSettings;
use crate::screens::thread::rewind::{TimelineRewind, use_timeline_rewind};
use crate::screens::thread::rows::approval::{ApprovalContext, ApprovalRowsState};
use crate::screens::thread::rows::assistant::{ReplyHandler, ReplyService};
use crate::screens::thread::rows::events::EventRowContext;
use crate::screens::thread::rows::groups::WorkGroupsState;
use crate::screens::thread::rows::plan::{PlanContext, PlanRowsState};
use crate::screens::thread::rows::user::ContextChipRenderer;
use t3_client::thread_views::threads::activity_announcement::{
    ThreadActivitySnapshot, ThreadActivityStatus, ThreadAnnouncementMemory,
    next_thread_announcement,
};
use t3_client::thread_views::threads::turn_detail_announcement::{
    StepLabel, TurnDetail, TurnDetailMemory, next_turn_detail_announcement, turn_detail,
};
use t3_contracts::orchestration_v2::{RuntimeRequestKind, RuntimeRequestKindMember2};

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
    /// The thread's find (ThreadFind's state; its bar and controller mount with the block list).
    pub find_state: Signal<crate::screens::thread::find::FindState>,
    /// Find is open with a query (`findActive`).
    pub find_active: Memo<bool>,
    pub timestamp_format: Memo<TimestampFormat>,
    /// `toolCallVisibility`, with zen shown as hideAll while find is open.
    pub tool_call_visibility: Memo<String>,
    pub screen_reader_announcements: Memo<ScreenReaderAnnouncements>,
    pub math_rendering_enabled: Memo<bool>,
    pub chips: ContextChipRenderer,
    /// `onReplyToResponse`; none without a client, so Reply is hidden as the web without one.
    pub reply: Option<ReplyHandler>,
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
        let find_state = signal(crate::screens::thread::find::FindState::default());
        let find_active = computed(move || {
            find_state
                .with(|state| state.open && !t3_contracts::wire::trim_js(&state.query).is_empty())
        });
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
            reply: use_context::<ReplyService>()
                .map(|service| service.0)
                .or_else(|| crate::screens::thread::composer::reply_to_response_handler(&key)),
            key,
            run,
            working,
            find_state,
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
    thread_activity_announcer(&services, &key, entries);
    provide_context(ChatMarkdownSettings {
        math_enabled: services.math_rendering_enabled,
    });
    let mut options = BlockListOptions::for_thread(&services, approvals, plan_context(&key));
    if let Some(thread) = options.thread.as_mut() {
        let key = key.clone();
        thread.header = Some(Rc::new(move || parent_thread_divider(key.clone())));
    }
    messages_timeline(rows, options)
}

/// MessagesTimeline.tsx `listHeaderLeadingContent` with a parent link: the TimelineSystemDivider
/// "Context from" / "Subagent of" its parent, with "Open parent thread" where it can be opened.
/// The link is the client's (`resolve_parent_thread_link`); the main timeline is not embedded.
fn parent_thread_divider(key: ThreadKey) -> AnyElement {
    use crate::screens::thread::rows::events::timeline_system_divider;
    use crate::ui::icon::Icon;
    use t3_client::thread_views::parent_thread_link::resolve_parent_thread_link;
    let runtime = rt();
    let nav = crate::state::nav::navigator();
    let thread_id = key.thread.clone();
    let shell = runtime.store.shell(key.environment.clone());
    let thread = runtime
        .store
        .shell_state(key.environment.clone(), move |shell| {
            shell
                .and_then(|shell| thread_shell(shell, &thread_id))
                .cloned()
        });
    dynamic(move || {
        let thread = thread.get()?;
        let delegated = thread.delegated_from.clone().flatten();
        // A delegating environment's titles are read from its own shell, when this client has it.
        let other = delegated
            .as_ref()
            .filter(|delegated| delegated.environment_id != key.environment)
            .map(|delegated| runtime.store.shell(delegated.environment_id.clone()));
        let title_of = |target: &ThreadKey| {
            let find =
                |snapshot: Memo<Option<Shared<t3_client::state::sharing::SharedShellSnapshot>>>| {
                    snapshot.with(|shell| {
                        shell.as_ref().and_then(|shell| {
                            shell
                                .threads
                                .iter()
                                .chain(shell.archived_threads.iter())
                                .find(|row| row.id == target.thread)
                                .map(|row| row.title.clone())
                        })
                    })
                };
            if target.environment == key.environment {
                find(shell)
            } else {
                other.and_then(find)
            }
        };
        let environments = runtime.environments();
        let link = resolve_parent_thread_link(&key.environment, &thread, false, title_of, |id| {
            environments
                .iter()
                .find(|entry| &entry.id == id)
                .map(|entry| entry.label.clone())
        })?;
        let title = link.title.clone().unwrap_or_else(|| t("Parent thread"));
        let detail = match &link.environment_label {
            Some(environment) => tr(
                "{title} on {environment}",
                &[
                    ("title", Value::Text(title)),
                    ("environment", Value::Text(environment.clone())),
                ],
            ),
            None => title,
        };
        let action = link.open.clone().map(|parent| {
            (
                Text::from("Open parent thread"),
                Rc::new(move || nav.open(crate::state::nav::Route::Thread(parent.clone())))
                    as Rc<dyn Fn()>,
            )
        });
        Some(untracked(|| {
            view()
                .app_class(crate::theme::tw("w-full px-3 sm:px-12"))
                .child(
                    view()
                        .app_class(crate::theme::tw(
                            "mx-auto w-full min-w-0 max-w-(--chat-content-max-width) pt-1 sm:pt-2",
                        ))
                        .child(timeline_system_divider(
                            Text::from(if link.side_chat {
                                "Context from"
                            } else {
                                "Subagent of"
                            }),
                            Some(Text::content(detail)),
                            Some(if link.side_chat {
                                Icon::MessagesSquare
                            } else {
                                Icon::Bot
                            }),
                            false,
                            false,
                            action,
                        )),
                )
        }))
    })
    .into_element()
}

/// Where the timeline's announcements go: the platform's screen-reader announcer, or a test's
/// recorder provided as context.
#[derive(Clone)]
pub struct AnnouncementSink(pub Rc<dyn Fn(String)>);

// Long enough for a screen reader to finish a short label before the next one.
const ACTIVITY_GAP_MS: u64 = 2_000;
// A polite region queues what it cannot say yet; steps written meanwhile wait this long.
const MESSAGE_HOLD_MS_PER_CHARACTER: u64 = 50;
const MESSAGE_HOLD_MAX_MS: u64 = 15_000;

fn hold_after(text: &str) -> u64 {
    (text.chars().count() as u64 * MESSAGE_HOLD_MS_PER_CHARACTER)
        .clamp(ACTIVITY_GAP_MS, MESSAGE_HOLD_MAX_MS)
}

/// `asOneAnnouncement`: one write, each part but the last closed with a full stop.
fn as_one_announcement(texts: &[String]) -> String {
    let last = texts.len().saturating_sub(1);
    texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            if index == last || text.ends_with(['.', '!', '?', ':', '…']) {
                text.clone()
            } else {
                format!("{text}.")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn step_text(
    step: &t3_client::thread_views::threads::turn_detail_announcement::StepLabel,
) -> String {
    use t3_client::thread_views::threads::turn_detail_announcement::StepLabel;
    match step {
        StepLabel::Thinking => t("Thinking"),
        StepLabel::Editing(subject) => tr(
            "Editing {subject}",
            &[("subject", Value::Text(subject.clone()))],
        ),
        StepLabel::SearchingFor(subject) => tr(
            "Searching for {subject}",
            &[("subject", Value::Text(subject.clone()))],
        ),
        StepLabel::SearchingTheWebFor(subject) => tr(
            "Searching the web for {subject}",
            &[("subject", Value::Text(subject.clone()))],
        ),
        StepLabel::Plain(text) => text.clone(),
    }
}

/// What the announcer reads of the thread: its status, request, run and settings level.
#[derive(Clone, Debug, PartialEq)]
struct ActivityInputs {
    status: ThreadActivityStatus,
    request_id: Option<String>,
    run_id: Option<String>,
    run_status: Option<String>,
    workspace_root: Option<String>,
}

#[derive(Default)]
struct AnnouncerMemory {
    turn: Option<ThreadAnnouncementMemory>,
    detail: Option<(ScreenReaderAnnouncements, TurnDetailMemory)>,
    quiet_until: u64,
    waiting: Option<StepLabel>,
}

/// Port of ThreadActivityAnnouncer.tsx: the thread's status ("Needs approval", "Needs your
/// input", the run's outcome) at every level; at "messages" the agent's finished messages; at
/// "everything" also its current step, held back while a message is still being read. Speaks
/// through the platform announcer (the web's polite live regions).
fn thread_activity_announcer(
    services: &TimelineServices,
    key: &ThreadKey,
    entries: Memo<Shared<Timeline>>,
) {
    let runtime = rt();
    let sink = use_context::<AnnouncementSink>()
        .map(|sink| sink.0)
        .unwrap_or_else(|| {
            Rc::new(|text: String| announce_for_accessibility(text)) as Rc<dyn Fn(String)>
        });
    let thread_id = key.thread.clone();
    let working = services.working;
    let run = services.run;
    let level = services.screen_reader_announcements;
    let shell_facts = runtime
        .store
        .shell_state(key.environment.clone(), move |shell| {
            let shell = shell?;
            let thread = thread_shell(shell, &thread_id)?;
            let request = thread.pending_runtime_request.as_ref().map(|request| {
            let input = matches!(
                &request.kind,
                RuntimeRequestKind::Member2(kind) if **kind == RuntimeRequestKindMember2::UserInput
            );
            (request.id.0.clone(), input)
        });
            let root = thread
                .worktree_path
                .as_ref()
                .map(|path| path.0.clone())
                .or_else(|| {
                    shell
                        .snapshot
                        .as_ref()?
                        .projects
                        .iter()
                        .find(|project| project.id == thread.project_id)
                        .map(|project| project.workspace_root.0.clone())
                });
            Some((request, root))
        });
    let inputs = computed(move || {
        let (request, workspace_root) = shell_facts.get().unwrap_or_default();
        let latest = run.with(|run| run.latest_run.clone());
        ActivityInputs {
            status: match &request {
                Some((_, true)) => ThreadActivityStatus::Input,
                Some((_, false)) => ThreadActivityStatus::Approval,
                None if working.get() => ThreadActivityStatus::Working,
                None => ThreadActivityStatus::Idle,
            },
            request_id: request.map(|(id, _)| id),
            run_id: latest.as_ref().map(|run| run.run_id.clone()),
            run_status: latest.map(|run| run.status),
            workspace_root,
        }
    });
    let memory: Stored<AnnouncerMemory> = stored(AnnouncerMemory::default());
    // Writes the status, messages and steps to the platform's announcer after each change; the
    // first snapshot of a thread is a silent baseline. A step's delay timer belongs to this run.
    effect(move || {
        let inputs = inputs.get();
        let level = level.get();
        let snapshot = ThreadActivitySnapshot {
            status: inputs.status,
            request_id: inputs.request_id.clone(),
            run_id: inputs.run_id.clone().map(|id| IdOrIndex::Id(RunId(id))),
            run_status: inputs
                .run_status
                .clone()
                .and_then(|status| serde_json::from_value(serde_json::Value::String(status)).ok()),
        };
        let (detail_level_memory, detail) = memory.with(|memory| {
            let previous = memory
                .detail
                .as_ref()
                .filter(|(previous, _)| *previous == level)
                .map(|(_, detail)| detail.clone());
            let detail = if level == ScreenReaderAnnouncements::Status {
                TurnDetail::default()
            } else {
                entries.with(|timeline| {
                    turn_detail(
                        &timeline.entries,
                        inputs.workspace_root.as_deref(),
                        previous
                            .as_ref()
                            .and_then(|memory| memory.tail_id.as_deref()),
                    )
                })
            };
            (previous, detail)
        });
        let waiting = memory.with(|memory| memory.waiting.take());
        let (first, turn) = memory.with(|memory| {
            let first = memory.turn.is_none();
            let (next, announcement) = next_thread_announcement(memory.turn.as_ref(), snapshot);
            memory.turn = Some(next);
            (first, announcement)
        });
        let (detail_memory, messages, activity) = next_turn_detail_announcement(
            detail_level_memory.as_ref(),
            &detail,
            &level,
            inputs.status,
        );
        let step_active = detail_memory.activity.is_some();
        memory.with(|memory| memory.detail = Some((level.clone(), detail_memory)));
        if first {
            memory.with(|memory| memory.quiet_until = 0);
            return;
        }
        let now = nsh_core::time::unix_ms() as u64;
        if !messages.is_empty() {
            let mut texts: Vec<String> = messages
                .iter()
                .map(|message| {
                    let text = if message.truncated {
                        tr(
                            "{message} More in the thread.",
                            &[("message", Value::Text(message.text.clone()))],
                        )
                    } else {
                        message.text.clone()
                    };
                    tr("T3 Code: {message}", &[("message", Value::Text(text))])
                })
                .collect();
            // A status landing with a message is read as its last sentence.
            texts.extend(
                turn.map(|announcement| Text::client_key(announcement).resolve().to_string()),
            );
            let text = as_one_announcement(&texts);
            memory.with(|memory| memory.quiet_until = now + hold_after(&text));
            sink(text);
        } else if let Some(announcement) = turn {
            sink(Text::client_key(announcement).resolve().to_string());
            memory
                .with(|memory| memory.quiet_until = memory.quiet_until.max(now + ACTIVITY_GAP_MS));
        }
        // A waiting step is stale once its step is over or the level no longer reads steps.
        let step = if level != ScreenReaderAnnouncements::Everything || !step_active {
            None
        } else {
            activity.or(waiting)
        };
        let Some(step) = step else {
            return;
        };
        let wait = memory.with(|memory| memory.quiet_until.saturating_sub(now));
        let speak = {
            let sink = sink.clone();
            move |step: &StepLabel| {
                let text = step_text(step);
                memory.with(|memory| {
                    memory.waiting = None;
                    memory.quiet_until = nsh_core::time::unix_ms() as u64 + ACTIVITY_GAP_MS;
                });
                sink(text);
            }
        };
        if wait == 0 {
            speak(&step);
            return;
        }
        memory.with(|memory| memory.waiting = Some(step.clone()));
        set_timeout(std::time::Duration::from_millis(wait), move || speak(&step));
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
