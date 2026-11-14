//! The thread timeline: one virtual-list cell per timeline entry, each reading its entry through
//! its own memo, so a streamed token re-renders only the streaming row.
//!
//! Interim structure (row = entry). The block-flattened timeline (TL-BLOCKS-*) replaces the cell
//! body with blocks so long code cannot dominate a frame; see ui-plan.md, NSH problem 2.

use std::rc::Rc;

use nsh_base::text_view;
use nsh_base::text_view::{MarkdownOptions, TextViewClasses};
use nsh_core::prelude::*;
use t3_client::state::selectors::thread_shell;
use t3_client::types::ThreadKey;
use t3_client::view_logic::session::{TimelineContent, TimelineEntry};
use t3_contracts::orchestration_v2::OrchestrationV2ShellThreadStatus as Status;

use crate::i18n::t;
use crate::runtime::{Shared, rt};
use crate::theme::{palette, tw};
use crate::ui::surface::empty_state;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    UserMessage,
    AssistantMessage,
    Work,
    Other,
}

impl RowKind {
    pub fn of(entry: &TimelineEntry) -> Self {
        match &entry.content {
            TimelineContent::Message { message, .. } if message.role == "user" => Self::UserMessage,
            TimelineContent::Message { .. } => Self::AssistantMessage,
            TimelineContent::Work { .. } => Self::Work,
            _ => Self::Other,
        }
    }
}

fn message_text(entry: &TimelineEntry) -> String {
    match &entry.content {
        TimelineContent::Message { message, .. } => message.text.clone(),
        TimelineContent::Work { entry } => match &entry.detail {
            Some(detail) => format!("{} · {detail}", entry.label),
            None => entry.label.clone(),
        },
        TimelineContent::ProposedPlan { .. } => t("Proposed plan"),
        TimelineContent::HtmlRender { .. } => t("Rendered HTML"),
        TimelineContent::McpApp { .. } => t("App"),
        TimelineContent::Event { .. } => t("Event"),
    }
}

fn is_streaming(entry: &TimelineEntry) -> bool {
    matches!(&entry.content, TimelineContent::Message { message, .. } if message.streaming)
}

pub fn timeline(key: ThreadKey) -> impl IntoElement {
    let runtime = rt();
    let thread_id = key.thread.clone();
    let working = runtime
        .store
        .shell_state(key.environment.clone(), move |shell| {
            shell
                .and_then(|shell| thread_shell(shell, &thread_id))
                .is_some_and(|thread| {
                    matches!(
                        thread.status,
                        Status::Preparing | Status::Queued | Status::Starting | Status::Running
                    )
                })
        });
    let timeline = runtime.store.timeline(key);
    let count = computed(move || timeline.with(|t| t.entries.len()));
    let keys = computed(move || Shared(timeline.with(|t| t.keys.clone())));
    let list = virtual_list(count, move |at| {
        let entry: Memo<Option<Shared<TimelineEntry>>> =
            computed(move || timeline.with(|t| t.entries.get(at.index()).cloned().map(Shared)));
        let kind = computed(move || entry.get().map(|e| RowKind::of(&e)));
        row(entry, kind)
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
        .child(when(move || working.get(), working_row))
}

/// `MessagesTimeline.tsx`'s row frame: the lane inset (`[data-chat-canvas]
/// .messages-timeline-scroll`, hand-ported from `index.css`), the centred content column, and
/// the row's bottom padding (`pb-4`; `pb-2` for an assistant message without its meta row).
fn row(
    entry: Memo<Option<Shared<TimelineEntry>>>,
    kind: Memo<Option<RowKind>>,
) -> impl IntoElement {
    let content = move || entry.get().map(|e| message_text(&e)).unwrap_or_default();
    let body = dynamic(move || match kind.get() {
        Some(RowKind::UserMessage) => Some(user_message(content).into_element()),
        Some(RowKind::AssistantMessage) => Some(assistant_message(entry, content).into_element()),
        Some(RowKind::Work) | Some(RowKind::Other) => Some(work_line(content).into_element()),
        None => None,
    });
    let padding = move || {
        Some(tw(match kind.get() {
            Some(RowKind::AssistantMessage) => "pb-2",
            _ => "pb-4",
        }))
    };
    view().app_class(tw("w-full px-3 sm:px-12")).child(
        view()
            .app_class(tw(
                "mx-auto w-full min-w-0 max-w-(--chat-content-max-width)",
            ))
            .child(view().app_class(padding).child(body)),
    )
}

/// `TimelineUserMessageRow.tsx` `UserTimelineRow`, a plain message: the bubble. Departures until
/// TL-USER: the body is plain text (the web renders it as markdown with line breaks), and the
/// actions row (timestamp, copy, revert) is not built.
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

/// `TimelineAssistantRows.tsx` `AssistantTimelineRow`: the markdown body. Meta row, changed
/// files and status badges are TL-ASSIST's.
fn assistant_message(
    entry: Memo<Option<Shared<TimelineEntry>>>,
    content: impl Fn() -> String + 'static,
) -> impl IntoElement {
    let markdown = Rc::new(content);
    view().app_class(tw("relative min-w-0 px-1 py-0.5")).child(
        text_view()
            .markdown(move || Some(SharedString::from(markdown())))
            .options(move || MarkdownOptions {
                streaming: entry.get().is_some_and(|e| is_streaming(&e)),
                ..Default::default()
            })
            .selectable(true)
            .classes(TextViewClasses::default())
            // `ChatMarkdown`'s root: `text-sm text-foreground/[calc(90%+contrast/10)]` at the
            // default contrast, set on the TextView so every part inherits it.
            .text_size(px(14.0))
            .text_color(move || palette().foreground.opacity(0.9))
            .w_full(),
    )
}

/// `TimelineWorkGroupRows.tsx` `WorkingTimelineRow`, its label without a run start ("Working..."),
/// in the row frame: the timeline's last row while the thread's run is active.
fn working_row() -> impl IntoElement {
    let row = view().app_class(tw("border-b border-border/60 pb-2 pt-1")).child(
        view()
            .app_class(tw(
                "flex h-6 min-w-0 items-baseline gap-2 px-1 text-sm leading-relaxed text-muted-foreground tabular-nums",
            ))
            .child(
                text(t("Working..."))
                    .app_class(tw("relative shrink-0 overflow-hidden whitespace-nowrap")),
            ),
    );
    view().app_class(tw("w-full px-3 sm:px-12")).child(
        view()
            .app_class(tw(
                "mx-auto w-full min-w-0 max-w-(--chat-content-max-width) pb-1.5",
            ))
            .child(row),
    )
}

/// Work rows are TL-WORK's; until then one muted line.
fn work_line(content: impl Fn() -> String + 'static) -> impl IntoElement {
    text(move || SharedString::from(content()))
        .number_of_lines(2u32)
        .app_class(tw("text-xs text-muted-foreground"))
}
