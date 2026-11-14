//! Port of apps/web/src/components/chat/MessagesTimeline.tsx MessagesTimeline (the list and its
//! row frame). Rows are the client's port of `MessagesTimeline.logic.ts`
//! (`t3_client::thread_views::messages_timeline`), derived from the timeline entries and the
//! thread's run; each virtual-list cell reads its row through its own memo, so a streamed token
//! re-renders only the streaming row. User rows are TL-ROWS-USER's; assistant, assistant-meta and
//! sent-message rows TL-ROWS-ASSISTANT's; work-log, thinking, toggle and fold rows TL-ROWS-WORK-2's
//! over TL-ROWS-WORK-1's entries, whose expansions feed back into the client's row input; event
//! rows TL-EVENTS'. TL-VIEWPORT owns following and position memory; CMP-SEND's queued and failed
//! messages follow the rows inside the same scroller.
//!
//! The block-flattened timeline (TL-BLOCKS-*) replaces the cell body with blocks so long code
//! cannot dominate a frame; see ui-plan.md, NSH problem 2.

use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::services::models::{present_thread_shell, thread_runtime_is_active};
use t3_client::state::selectors::thread_shell;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRow, MessagesTimelineRowContent, MessagesTimelineRowsInput,
    MessagesTimelineRowsProjection, TimelineLatestRun, derive_messages_timeline_rows_with_state,
};
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_contracts::orchestration_v2::{OrchestrationV2ThreadShell, OrchestrationV2TurnItem};

use crate::i18n::t;
use crate::runtime::{Shared, rt};
use crate::screens::thread::rows::assistant::{
    AssistantRowContext, AssistantRowsState, assistant_meta_timeline_row, assistant_row,
    assistant_timeline_row, sent_message_timeline_row,
};
use crate::screens::composer::send::{ComposerSendClient, PendingMessages, pending_messages};
use crate::screens::thread::rows::events::{EventRowContext, v2_event_timeline_row};
use crate::screens::thread::rows::groups::{
    WorkGroupsContext, WorkGroupsState, work_group_timeline_row,
};
use crate::screens::thread::rows::user::{UserRowContext, UserRowState, user_message_row};
use crate::screens::thread::rows::work::working_row;
use crate::screens::thread::viewport::{TimelineViewport, timeline_row_keys};
use crate::theme::tw;
use crate::ui::surface::empty_state;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    User,
    Assistant,
    AssistantMeta,
    SentMessage,
    Event,
    Working,
    Work,
    Other,
}

fn kind_of(row: &MessagesTimelineRow) -> RowKind {
    use MessagesTimelineRowContent as C;
    match &row.content {
        C::Message { message, .. } if message.role == "user" => RowKind::User,
        C::Message { .. } => RowKind::Assistant,
        C::AssistantMeta { .. } => RowKind::AssistantMeta,
        C::SentMessage { .. } => RowKind::SentMessage,
        C::Event { .. } => RowKind::Event,
        C::Working { .. } => RowKind::Working,
        C::Work { .. }
        | C::WorkLive { .. }
        | C::Thinking { .. }
        | C::WorkToggle { .. }
        | C::TurnFold { .. }
        | C::AttemptFold { .. } => RowKind::Work,
        _ => RowKind::Other,
    }
}

/// What ChatView passes MessagesTimeline about the run (`latestRun`, `runningRunId`, `isWorking`,
/// `activeTurnStartedAt`), read from the client's presentation of the thread shell.
#[derive(Clone, Debug, Default, PartialEq)]
struct RunState {
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

pub fn timeline(key: ThreadKey) -> impl IntoElement {
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
    let entries = runtime.store.timeline(key.clone());
    // The previous projection: the client reuses unchanged rows and patches a streaming one.
    let previous: Rc<RefCell<Option<MessagesTimelineRowsProjection>>> = Rc::default();
    let groups = WorkGroupsState::default();
    let working = computed(move || run.with(|run| run.is_working));
    let rows = computed(move || {
        let run = run.get();
        let mut input = MessagesTimelineRowsInput {
            timeline_entries: entries.with(|t| t.entries.as_ref().clone()),
            latest_run: run.latest_run,
            running_run_id: run.running_run_id,
            is_working: run.is_working,
            active_turn_started_at: run.active_turn_started_at,
            tool_call_visibility: "showAll".into(),
            ..Default::default()
        };
        groups.apply_to_input(&mut input);
        let next = derive_messages_timeline_rows_with_state(&input, previous.borrow().as_ref());
        let rows = next.rows.clone();
        *previous.borrow_mut() = Some(next);
        Shared(rows)
    });
    let count = computed(move || rows.with(|r| r.0.len()));
    let keys = timeline_row_keys(rows);
    let viewport = TimelineViewport::new(key.clone(), keys);
    let shared = Rows {
        state: AssistantRowsState::default(),
        users: UserRowState::new(),
        events: EventRowContext::new(key.clone(), groups.entries),
        groups,
        working,
    };
    let pending = runtime
        .backend()
        .client()
        .map(|client| -> (PendingMessages, Rc<dyn ComposerSendClient>) {
            let client = Rc::new(client);
            (PendingMessages::attach(client.clone(), key.clone()), client)
        });
    let none_pending = {
        let pending = pending.as_ref().map(|(messages, _)| *messages);
        move || pending.is_none_or(|p| p.messages.with(|m| m.is_empty()))
    };
    let list = virtual_list(count, move |at| {
        let row: Memo<Option<Shared<MessagesTimelineRow>>> =
            computed(move || rows.with(|r| r.0.get(at.index()).cloned().map(Shared)));
        let kind = computed(move || row.with(|r| r.as_ref().map(|r| kind_of(&r.0))));
        cell(row, kind, shared.clone())
    })
    .empty(dynamic(move || {
        none_pending().then(|| {
            empty_state(
                t("No messages yet"),
                t("Send a message to start this thread."),
            )
        })
    }))
    .footer(pending.map(|(messages, client)| {
        view().app_class(tw("w-full px-3 sm:px-12")).child(
            view()
                .app_class(tw("mx-auto w-full min-w-0 max-w-(--chat-content-max-width)"))
                .child(pending_messages(client, key, messages)),
        )
    }).unwrap_or_else(view))
    .estimated_item_size(px(120.0))
    .content_padding_y(px(16.0))
    .flex_1()
    .test_id("timeline");
    let list = viewport.attach(list);
    view()
        .flex_1()
        .min_h(px(0.0))
        .selectable(true)
        .child(list)
}

/// The per-thread row state the cells share, created once above the virtual list.
#[derive(Clone)]
struct Rows {
    state: AssistantRowsState,
    users: UserRowState,
    events: EventRowContext,
    groups: WorkGroupsState,
    working: Memo<bool>,
}

fn cell(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    kind: Memo<Option<RowKind>>,
    Rows {
        state,
        users,
        events,
        groups,
        working,
    }: Rows,
) -> impl IntoElement {
    dynamic(move || {
        let body = match kind.get()? {
            // WorkingTimelineRow carries its own lane, column and padding.
            RowKind::Working => return Some(working_row().into_element()),
            RowKind::User => {
                let projection = computed(move || row.with(|r| r.as_ref().map(|r| r.0.clone())));
                let mut context = UserRowContext::new(users);
                context.working = working;
                user_message_row(projection, context).into_element()
            }
            RowKind::Assistant => {
                assistant_timeline_row(assistant_row(row), AssistantRowContext::new(state))
                    .into_element()
            }
            RowKind::AssistantMeta => {
                assistant_meta_timeline_row(assistant_row(row), AssistantRowContext::new(state))
                    .into_element()
            }
            RowKind::SentMessage => {
                sent_message_timeline_row(sent_text(row), AssistantRowContext::new(state))
                    .into_element()
            }
            RowKind::Event => v2_event_timeline_row(row, events.clone()).into_element(),
            RowKind::Work => {
                work_group_timeline_row(row, WorkGroupsContext::new(groups)).into_element()
            }
            RowKind::Other => return None,
        };
        let frame = move || Some(tw(row.with(|r| r.as_ref().map_or("pb-4", |r| frame_of(&r.0)))));
        Some(
            view()
                .app_class(tw("w-full px-3 sm:px-12"))
                .child(
                    view()
                        .app_class(tw("mx-auto w-full min-w-0 max-w-(--chat-content-max-width)"))
                        .child(view().app_class(frame).child(body)),
                )
                .into_element(),
        )
    })
}

/// The row's outer `div` classes in MessagesTimeline.tsx `TimelineRowContent`: the bottom padding
/// per row kind, and `group/assistant` on assistant messages and their meta row.
fn frame_of(row: &MessagesTimelineRow) -> &'static str {
    use MessagesTimelineRowContent as C;
    let assistant = match &row.content {
        C::Message { message, .. } => message.role == "assistant",
        C::AssistantMeta { .. } => true,
        _ => false,
    };
    match (padding_of(row), assistant) {
        ("pb-1", true) => "pb-1 group/assistant",
        ("pb-2", true) => "pb-2 group/assistant",
        ("pb-4", true) => "pb-4 group/assistant",
        (padding, _) => padding,
    }
}

fn padding_of(row: &MessagesTimelineRow) -> &'static str {
    use MessagesTimelineRowContent as C;
    match &row.content {
        C::Work { .. } | C::WorkLive { .. } | C::Thinking { .. } | C::WorkToggle { .. } => "",
        C::Event { projected_item, .. }
            if matches!(projected_item.item, OrchestrationV2TurnItem::Subagent(_)) =>
        {
            ""
        }
        C::TurnFold { .. } | C::Working { .. } => "pb-1.5",
        C::Message {
            grouped_with_next: Some(true),
            ..
        } => "pb-1",
        C::Message {
            message,
            show_assistant_meta: false,
            ..
        } if message.role == "assistant" => "pb-2",
        C::Event { .. } | C::AttemptFold { .. } | C::HtmlRender { .. } | C::McpApp { .. } => {
            "pb-2"
        }
        C::ViewedImages { .. } | C::WorktreeSetup { .. } | C::SentMessage { .. } => "pb-2",
        _ => "pb-4",
    }
}

fn sent_text(row: Memo<Option<Shared<MessagesTimelineRow>>>) -> Memo<String> {
    computed(move || {
        row.with(|r| match r.as_ref().map(|r| &r.0.content) {
            Some(MessagesTimelineRowContent::SentMessage { text }) => text.clone(),
            _ => String::new(),
        })
    })
}
