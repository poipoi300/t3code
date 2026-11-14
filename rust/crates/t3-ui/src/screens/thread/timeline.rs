//! Port of apps/web/src/components/chat/MessagesTimeline.tsx MessagesTimeline (the list and its
//! row frame). Rows are the client's port of `MessagesTimeline.logic.ts`
//! (`t3_client::thread_views::messages_timeline`), derived from the timeline entries and the
//! thread's run; each virtual-list cell reads its row through its own memo, so a streamed token
//! re-renders only the streaming row. Assistant rows are TL-ROWS-ASSISTANT's; user and work rows
//! keep interim renderers until TL-ROWS-USER and TL-ROWS-WORK-* can be mounted (see
//! `one-change/integration-1/README.md`).
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
    work_entry_display_label,
};
use t3_client::types::{EnvironmentId, ThreadKey};
use t3_contracts::orchestration_v2::OrchestrationV2ThreadShell;

use crate::i18n::t;
use crate::runtime::{Shared, rt};
use crate::screens::thread::rows::assistant::{
    AssistantRowContext, AssistantRowsState, assistant_row, assistant_timeline_row,
};
use crate::theme::tw;
use crate::ui::surface::empty_state;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    User,
    Assistant,
    Working,
    Work,
    Other,
}

fn kind_of(row: &MessagesTimelineRow) -> RowKind {
    use MessagesTimelineRowContent as C;
    match &row.content {
        C::Message { message, .. } if message.role == "user" => RowKind::User,
        C::Message { .. } => RowKind::Assistant,
        C::Working { .. } => RowKind::Working,
        C::Work { .. } | C::WorkLive { .. } | C::Thinking { .. } | C::WorkToggle { .. } => {
            RowKind::Work
        }
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
    let entries = runtime.store.timeline(key);
    // The previous projection: the client reuses unchanged rows and patches a streaming one.
    let previous: Rc<RefCell<Option<MessagesTimelineRowsProjection>>> = Rc::default();
    let rows = computed(move || {
        let run = run.get();
        let input = MessagesTimelineRowsInput {
            timeline_entries: entries.with(|t| t.entries.as_ref().clone()),
            latest_run: run.latest_run,
            running_run_id: run.running_run_id,
            is_working: run.is_working,
            active_turn_started_at: run.active_turn_started_at,
            tool_call_visibility: "showAll".into(),
            ..Default::default()
        };
        let next = derive_messages_timeline_rows_with_state(&input, previous.borrow().as_ref());
        let rows = next.rows.clone();
        *previous.borrow_mut() = Some(next);
        Shared(rows)
    });
    let count = computed(move || rows.with(|r| r.0.len()));
    let keys = computed(move || {
        rows.with(|r| {
            Shared(Rc::new(
                r.0.iter()
                    .map(|row| SharedString::from(row.id.clone()))
                    .collect::<Vec<_>>(),
            ))
        })
    });
    let state = AssistantRowsState::default();
    let list = virtual_list(count, move |at| {
        let row: Memo<Option<Shared<MessagesTimelineRow>>> =
            computed(move || rows.with(|r| r.0.get(at.index()).cloned().map(Shared)));
        let kind = computed(move || row.with(|r| r.as_ref().map(|r| kind_of(&r.0))));
        cell(row, kind, state)
    })
    .keys(move || keys.get().0.as_ref().clone())
    .alignment(ListAlignment::Bottom)
    .empty(empty_state(
        t("No messages yet"),
        t("Send a message to start this thread."),
    ))
    .estimated_item_size(px(120.0))
    .content_padding_y(px(16.0))
    .flex_1()
    .test_id("timeline");
    view()
        .flex_1()
        .min_h(px(0.0))
        .selectable(true)
        .child(list)
}

/// `MessagesTimeline.tsx`'s row frame: the lane inset (`[data-chat-canvas]
/// .messages-timeline-scroll`, hand-ported from `index.css`), the centred content column, and the
/// row's bottom padding as the web chooses it per row kind.
fn cell(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    kind: Memo<Option<RowKind>>,
    state: AssistantRowsState,
) -> impl IntoElement {
    let body = dynamic(move || match kind.get() {
        Some(RowKind::User) => Some(user_message(move || user_text(row)).into_element()),
        Some(RowKind::Assistant) => Some(
            assistant_timeline_row(
                assistant_row(row),
                AssistantRowContext::new(state),
            )
            .into_element(),
        ),
        Some(RowKind::Working) => Some(working_row().into_element()),
        Some(RowKind::Work) => Some(work_lines(row).into_element()),
        Some(RowKind::Other) | None => None,
    });
    let padding = move || Some(tw(row.with(|r| r.as_ref().map_or("pb-4", |r| padding_of(&r.0)))));
    view().app_class(tw("w-full px-3 sm:px-12")).child(
        view()
            .app_class(tw("mx-auto w-full min-w-0 max-w-(--chat-content-max-width)"))
            .child(view().app_class(padding).child(body)),
    )
}

/// The web's per-row bottom padding (MessagesTimeline.tsx, the row's outer `div`).
fn padding_of(row: &MessagesTimelineRow) -> &'static str {
    use MessagesTimelineRowContent as C;
    match &row.content {
        C::Work { .. } | C::WorkLive { .. } | C::Thinking { .. } | C::WorkToggle { .. } => "",
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
        C::ViewedImages { .. } | C::WorktreeSetup { .. } => "pb-2",
        _ => "pb-4",
    }
}

fn user_text(row: Memo<Option<Shared<MessagesTimelineRow>>>) -> String {
    row.with(|r| match r.as_ref().map(|r| &r.0.content) {
        Some(MessagesTimelineRowContent::Message { message, .. }) => message.text.clone(),
        _ => String::new(),
    })
}

/// `TimelineUserMessageRow.tsx` `UserTimelineRow`, a plain message: the bubble. Interim until
/// TL-ROWS-USER's row can mount (it needs the client's `resolveUserMessagePresentation`): the
/// body is plain text and the actions row is not built.
fn user_message(content: impl Fn() -> String + 'static) -> impl IntoElement {
    view()
        .app_class(tw("group flex flex-col items-end gap-1"))
        .child(
            view()
                .app_class(tw(
                    "relative min-w-0 max-w-[80%] rounded-2xl text-message-foreground bg-message p-3",
                ))
                .child(
                    text(move || SharedString::from(content()))
                        .select_text()
                        .app_class(tw("text-sm leading-relaxed text-message-foreground")),
                ),
        )
}

/// Interim work rows until TL-ROWS-WORK-1 and -2 mount: one muted line per entry, with the
/// client's labels (`work_entry_display_label`, the work toggle's summary).
fn work_lines(row: Memo<Option<Shared<MessagesTimelineRow>>>) -> impl IntoElement {
    let labels = computed(move || row.with(|r| r.as_ref().map(|r| work_labels(&r.0)).unwrap_or_default()));
    view().app_class(tw("flex flex-col gap-1 pb-2")).child(dynamic(move || {
        let lines = labels.get();
        Some(
            lines
                .into_iter()
                .fold(view().app_class(tw("flex flex-col gap-1")), |column, label| {
                    column.child(
                        text(SharedString::from(label))
                            .number_of_lines(2u32)
                            .app_class(tw("text-xs text-muted-foreground")),
                    )
                })
                .into_element(),
        )
    }))
}

fn work_labels(row: &MessagesTimelineRow) -> Vec<String> {
    use MessagesTimelineRowContent as C;
    match &row.content {
        C::Work { grouped_entries, .. } => grouped_entries
            .iter()
            .map(|entry| work_entry_display_label(entry, None))
            .collect(),
        C::WorkLive { entry, .. } => vec![work_entry_display_label(entry, None)],
        C::Thinking { label, .. } => vec![label.clone().unwrap_or_else(|| t("Thinking"))],
        C::WorkToggle { summary, .. } => vec![summary.clone()],
        _ => Vec::new(),
    }
}

/// `TimelineWorkGroupRows.tsx` `WorkingTimelineRow`, its label without a run start
/// ("Working..."): the client's `working` row while the run is active. Owned by the integrator
/// until TL-ROWS-WORK-1 takes it with the work rows.
fn working_row() -> impl IntoElement {
    view().app_class(tw("border-b border-border/60 pb-2 pt-1")).child(
        view()
            .app_class(tw(
                "flex h-6 min-w-0 items-baseline gap-2 px-1 text-sm leading-relaxed text-muted-foreground tabular-nums",
            ))
            .child(
                text(t("Working..."))
                    .app_class(tw("relative shrink-0 overflow-hidden whitespace-nowrap")),
            ),
    )
}
