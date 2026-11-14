//! Port of apps/web/src/components/chat/TimelineWorkEntryRow.tsx SimpleWorkEntryRow.

use std::collections::BTreeSet;
use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::view_logic::session::{
    WorkLogEntry, work_entry_display_indicates_tool_failure, work_entry_signals_severe_failure,
    work_log_entry_is_tool_like,
};
use t3_client::work_log::{
    has_question_answer, question_answer_preview, question_answer_text, question_text_preview,
    resolve_work_entry_tool_presentation, tool_group_action, turn_item_has_detail,
    turn_item_needs_detail_fetch, work_entry_viewed_image_path,
};
use t3_client::workspace::project_ops::paths::format_workspace_relative_path;
use t3_contracts::orchestration_v2::{
    OrchestrationV2TurnItem, OrchestrationV2TurnItemStatus, TurnItemError,
};
use t3_contracts::provider_policy::UserInputAttachmentAnswerPayload;

use crate::i18n::{Value, t, tr};
use crate::runtime::Shared;
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon};

/// Owned above the virtual list; entry ids retain disclosure state across cell recycling.
#[derive(Clone, Copy)]
pub struct WorkRowState(Signal<BTreeSet<String>>);

impl Default for WorkRowState {
    fn default() -> Self {
        Self(signal(BTreeSet::new()))
    }
}

impl WorkRowState {
    pub fn expanded(self, id: &str) -> bool {
        self.0.with(|entries| entries.contains(id))
    }

    pub fn set_expanded(self, id: &str, expanded: bool) {
        self.0.update(|entries| {
            if expanded {
                entries.insert(id.to_owned());
            } else {
                entries.remove(id);
            }
        });
    }
}

/// Labels and read output come from MessagesTimeline.logic.ts, owned by the client.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkEntryRow {
    pub entry: Shared<WorkLogEntry>,
    pub display_label: String,
    pub compact_reasoning: Option<String>,
    /// None selects the inspector; Some(None) is a read/skill with no plain output.
    pub plain_output: Option<Option<String>>,
    pub question_answer: Option<UserInputAttachmentAnswerPayload>,
    pub workspace_root: Option<String>,
    pub grouped: bool,
    pub pending_approval: bool,
    pub retry_workspace_preparation: bool,
    pub thread_read_label: Option<String>,
    /// The group's order, shared by the strip and expanded-entry image viewer.
    pub viewed_images: Rc<Vec<String>>,
}

impl WorkEntryRow {
    pub fn new(entry: Shared<WorkLogEntry>, display_label: String) -> Self {
        Self {
            entry,
            display_label,
            compact_reasoning: None,
            plain_output: None,
            question_answer: None,
            workspace_root: None,
            grouped: false,
            pending_approval: false,
            retry_workspace_preparation: false,
            thread_read_label: None,
            viewed_images: Rc::default(),
        }
    }
}

pub type RowSlot = Rc<dyn Fn(&WorkEntryRow) -> AnyElement>;
pub type RowAction = Rc<dyn Fn(&WorkEntryRow)>;
pub type ToggleAction = Rc<dyn Fn(&str, bool)>;
pub type ContentReady = Rc<dyn Fn(&str)>;
pub type Inspector = Rc<dyn Fn(&WorkEntryRow, ContentReady) -> AnyElement>;
pub type ResetTime = Rc<dyn Fn(&str) -> Result<String, String>>;
pub type IconRenderer = Rc<dyn Fn(&WorkLogEntry, &str, &str) -> AnyElement>;

pub type ViewedImageRenderer = Rc<dyn Fn(t3_client::work_log::ViewedImageAsset) -> AnyElement>;

/// The caller's thread reference is already resolved, never a compact wire-table index.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewedImagesRow {
    pub thread_id: Option<t3_contracts::base_schemas::ThreadId>,
    pub workspace_root: Option<String>,
    pub paths: Rc<Vec<String>>,
}

pub fn viewed_images_timeline_row(
    row: Memo<ViewedImagesRow>,
    render: ViewedImageRenderer,
) -> impl IntoElement {
    dynamic(move || {
        let row = row.get();
        let thread = row.thread_id.as_ref()?;
        Some(untracked(|| {
            let mut gallery = view().app_class(tw("flex flex-wrap items-start gap-2 px-0.5"));
            for path in row.paths.iter() {
                if let Some(image) = t3_client::work_log::resolve_viewed_image_asset(
                    path,
                    thread,
                    row.workspace_root.as_deref(),
                ) {
                    gallery = gallery.child(render(image));
                }
            }
            gallery
        }))
    })
}

/// The same dependencies as TimelineRowCtx; operations must be permission checked by the client.
#[derive(Clone)]
pub struct WorkRowContext {
    pub state: WorkRowState,
    pub on_toggle_entry: Option<ToggleAction>,
    pub on_content_ready: ContentReady,
    pub icon: Option<IconRenderer>,
    pub timestamp: Option<RowSlot>,
    pub markdown: Option<RowSlot>,
    pub inspector: Option<Inspector>,
    pub fetched_output: Option<Inspector>,
    pub image: Option<RowSlot>,
    pub question_attachments: Option<RowSlot>,
    pub pending_approval: Option<RowSlot>,
    pub subagent_notification: Option<RowSlot>,
    pub thread_read: Option<RowSlot>,
    pub trailing_thread_links: Option<RowSlot>,
    pub retry_workspace_preparation: Option<RowAction>,
    /// Formats in the viewer's current zone, including the zone name, on each render.
    pub reset_time: Option<ResetTime>,
}

impl WorkRowContext {
    pub fn new(state: WorkRowState) -> Self {
        Self {
            state,
            on_toggle_entry: None,
            on_content_ready: Rc::new(|_| {}),
            icon: None,
            timestamp: None,
            markdown: None,
            inspector: None,
            fetched_output: None,
            image: None,
            question_attachments: None,
            pending_approval: None,
            subagent_notification: None,
            thread_read: None,
            trailing_thread_links: None,
            retry_workspace_preparation: None,
            reset_time: None,
        }
    }
}

pub fn simple_work_entry_row(
    row: Memo<Option<WorkEntryRow>>,
    context: WorkRowContext,
) -> impl IntoElement {
    dynamic(move || {
        let row = row.get()?;
        Some(untracked(|| {
            if let Some(OrchestrationV2TurnItem::Notification(notification)) =
                row.entry.projected_item.as_ref().map(|item| &item.item)
                && t3_client::thread_views::threads::execution_presentation::notification_child_thread_id(
                    &notification.source,
                ).is_some()
                && let Some(render) = &context.subagent_notification
            {
                return render(&row);
            }
            work_entry_log_row(row, context.clone())
        }))
    })
}

fn work_entry_log_row(row: WorkEntryRow, context: WorkRowContext) -> AnyElement {
    if row.pending_approval {
        return slot_or_unsupported(
            &context.pending_approval,
            &row,
            "Pending approval controls are unavailable.",
        );
    }
    if let Some(OrchestrationV2TurnItem::Error(error)) =
        row.entry.projected_item.as_ref().map(|item| &item.item)
        && error.status == OrchestrationV2TurnItemStatus::Failed
    {
        return failed_work_entry(&row, error, &context);
    }
    let row = Rc::new(row);
    let state = context.state;
    let id = row.entry.id.clone();
    let expanded = computed(move || state.expanded(&id));
    let rendered = row.clone();
    dynamic(move || {
        let expanded = expanded.get();
        untracked(|| {
            let row = rendered.clone();
            let entry = &row.entry;
            let warning = entry.source_activity_kind.as_deref() == Some("runtime.warning");
            let failed = !warning && work_entry_display_indicates_tool_failure(entry);
            let destructive = !warning && failed
                && (work_entry_signals_severe_failure(entry) || !work_log_entry_is_tool_like(entry));
            let reasoning = entry.item_type.as_deref() == Some("reasoning");
            let question = row.question_answer.as_ref();
            let question_heading = question.map(question_text_preview).unwrap_or_default();
            let preview = if reasoning && expanded {
                t(if entry.tool_lifecycle_status.as_deref() == Some("inProgress") { "Thinking" } else { "Thought" })
            } else if !question_heading.is_empty() {
                question_heading
            } else {
                t(&row.display_label)
            };
            let answer = question.filter(|answer| has_question_answer(answer))
                .map(question_answer_preview);
            let accessible = [row.thread_read_label.as_deref().unwrap_or(&preview), answer.as_deref().unwrap_or_default()]
                .into_iter().filter(|value| !value.is_empty()).collect::<Vec<_>>().join(": ");
            let accessible = if failed {
                tr("{preview}, tool call failed", &[("preview", Value::from(accessible))])
            } else { accessible };
            let can_expand = can_expand_projected_item(&row, &preview);
            let heading = if warning { "font-medium text-warning" }
                else if destructive { "font-medium text-destructive" }
                else if work_log_entry_is_tool_like(entry) { "text-secondary-label" }
                else { "text-foreground/80" };
            let icon_class = if warning { "text-warning" }
                else if destructive { "text-destructive" }
                else if failed { "text-tool-error-icon/40" }
                else if entry.tone == "tool" { "text-icon-muted" }
                else { work_tone_icon(&entry.tone).1 };
            let icon_name = if warning || destructive { "circle-alert".to_owned() } else { work_entry_icon_name(entry) };
            let glyph = context.icon.as_ref().map_or_else(
                || fallback_icon(&icon_name, icon_class, true),
                |render| render(entry, &icon_name, icon_class),
            );
            let label = if reasoning && !expanded {
                row.compact_reasoning.as_ref().map_or_else(
                    || unsupported("Compact reasoning preview is unavailable.").into_element(),
                    |preview| text(t(preview)).app_class(tw(heading)).number_of_lines(1u32).into_element(),
                )
            } else if let Some(label) = &row.thread_read_label {
                text(t(label)).app_class(tw(heading)).number_of_lines(1u32).into_element()
            } else {
                text(preview.clone()).app_class(tw(heading)).number_of_lines(1u32).into_element()
            };
            let label = view().app_class(tw("min-w-0 flex-1 overflow-hidden")).child(
                view().app_class(tw("flex min-w-0 w-full items-baseline gap-1.5 text-sm leading-relaxed"))
                    .child(view().app_class(tw(&cn([Some("min-w-0 truncate"), answer.is_none().then_some("flex-1")]))).child(label))
                    .child(answer.map(|answer| text(t(&answer)).number_of_lines(1u32).app_class(tw(&cn([
                        Some("min-w-0 truncate"), Some(if !expanded { "text-foreground" } else { "text-muted-foreground" }),
                    ]))))),
            );
            let trailing = view().app_class(tw("flex items-center gap-1.5"))
                .child(can_expand.then(|| view().app_class(tw("flex size-4 shrink-0 items-center justify-center"))
                    .child(icon(if expanded { Icon::ChevronDown } else { Icon::ChevronRight }, 12.0,
                        || crate::theme::palette().icon_muted.opacity(0.7)))))
                .child(context.timestamp.as_ref().map(|render| render(&row)));
            let line = work_log_line(glyph, label.into_element(), trailing.into_element(), false);
            // NSH's button owns keyboard activation; details and nested links are siblings so
            // selecting output or following a link cannot toggle the disclosure.
            let header = if can_expand {
                let clicked = row.clone();
                let toggled = context.on_toggle_entry.clone();
                nsh_base::button().accessibility_label(Some(SharedString::from(accessible)))
                    .app_class(tw("w-full cursor-pointer text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70"))
                    .test_id(&format!("work-entry-{}", entry.id))
                    .on_press(move || {
                        let next = !state.expanded(&clicked.entry.id);
                        state.set_expanded(&clicked.entry.id, next);
                        if let Some(toggle) = &toggled { toggle(&clicked.entry.id, !next); }
                    }).child(line).into_element()
            } else { line.into_element() };
            let mut root = view().app_class(tw(&cn([
                Some("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 text-left transition-colors"),
                Some(if row.grouped { "py-0" } else { "py-0.5" }),
            ]))).child(header)
                .child(context.trailing_thread_links.as_ref().map(|render| render(&row)))
                .child(context.trailing_thread_links.is_none().then(|| unavailable_thread_link(&row)).flatten())
                .child(row.thread_read_label.as_ref().map(|_| slot_or_unsupported(&context.thread_read, &row, "Thread link is unavailable.")));
            if expanded {
                if work_entry_viewed_image_path(entry).is_some() {
                    root = root.child(work_log_details("media", slot_or_unsupported(&context.image, &row, "Image preview is unavailable.")));
                }
                if let Some(answer) = question {
                    root = root.child(question_answer_history(answer, &row, &context));
                }
                if reasoning {
                    root = root.child(reasoning_trace_content(&row, &context));
                } else if question.is_none() && can_expand {
                    root = root.child(expanded_tool_content(&row, &context, &preview));
                }
            }
            root.into_element()
        })
    }).into_element()
}

fn can_expand_projected_item(row: &WorkEntryRow, preview: &str) -> bool {
    let entry = &row.entry;
    let image = work_entry_viewed_image_path(entry).is_some();
    let item = entry.projected_item.as_ref().map(|item| &item.item);
    if let Some(output) = &row.plain_output {
        return output.as_ref().is_some_and(|text| !text.is_empty())
            || image
            || row.question_answer.is_some()
            || item.is_some_and(turn_item_needs_detail_fetch);
    }
    if let Some(item) = item {
        return if entry.item_type.as_deref() == Some("reasoning") {
            entry
                .detail
                .as_ref()
                .is_some_and(|detail| !detail.trim().is_empty())
        } else {
            image || row.question_answer.is_some() || turn_item_has_detail(item)
        };
    }
    !preview.trim().is_empty()
        || (entry.item_type.as_deref() == Some("dynamic_tool") && entry.tool_data.is_some())
        || work_entry_raw_command(entry).is_some()
        || entry
            .command
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty())
        || entry
            .detail
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty())
        || entry
            .changed_files
            .as_ref()
            .is_some_and(|files| !files.is_empty())
        || image
}

fn expanded_tool_content(
    row: &WorkEntryRow,
    context: &WorkRowContext,
    preview: &str,
) -> AnyElement {
    if row.entry.projected_item.is_some() && row.plain_output.is_none() {
        return work_log_details(
            "panel",
            context.inspector.as_ref().map_or_else(
                || unsupported("Tool details are unavailable.").into_element(),
                |render| render(row, context.on_content_ready.clone()),
            ),
        )
        .into_element();
    }
    let body = row.plain_output.clone().unwrap_or_else(|| {
        build_tool_call_expanded_body(
            &row.entry,
            row.workspace_root.as_deref(),
            preview,
            work_entry_viewed_image_path(&row.entry),
        )
    });
    let fetch =
        row.entry.projected_item.as_ref().is_some_and(|item| {
            row.plain_output.is_some() && turn_item_needs_detail_fetch(&item.item)
        });
    // Timeline rows cannot contain nested scrollers; TL-BLOCKS-3 opens long output in a panel.
    // NSH cannot parse the CSS var fallback; resolve those same two size tokens explicitly.
    let content = view()
        .child(body.map(|body| {
            text(t(&body))
                .select_text()
                .app_class(tw(
                    "cursor-text font-mono text-secondary-label leading-relaxed select-text",
                ))
                .text_size(|| {
                    let tokens = crate::theme::current_tokens();
                    px(tokens
                        .number("font-size-code")
                        .or_else(|| tokens.number("text-2xs"))
                        .unwrap_or(11.0))
                })
        }))
        .child(fetch.then(|| {
            context.fetched_output.as_ref().map_or_else(
                || unsupported("Tool output retrieval is unavailable.").into_element(),
                |render| render(row, context.on_content_ready.clone()),
            )
        }));
    work_log_details("panel", content.into_element()).into_element()
}

pub fn reasoning_trace_content(row: &WorkEntryRow, context: &WorkRowContext) -> impl IntoElement {
    work_log_details(
        "text",
        slot_or_unsupported(&context.markdown, row, "Reasoning markdown is unavailable."),
    )
}

fn question_answer_history(
    answer: &UserInputAttachmentAnswerPayload,
    row: &WorkEntryRow,
    context: &WorkRowContext,
) -> impl IntoElement {
    let questions = answer.question_text_by_id.as_ref().and_then(Option::as_ref);
    let ids: BTreeSet<_> = questions
        .into_iter()
        .flat_map(|map| map.keys())
        .chain(answer.answers.keys())
        .chain(answer.attachments_by_question_id.keys())
        .collect();
    let mut root = view().app_class(tw("ms-7 mt-2 space-y-2"));
    for id in ids {
        let question = questions.and_then(|map| map.get(id));
        let response = answer
            .answers
            .get(id)
            .map(question_answer_text)
            .unwrap_or_default();
        root = root.child(
            view()
                .app_class(tw("space-y-1"))
                .child(question.map(|question| {
                    text(t(question)).app_class(tw("text-sm text-muted-foreground"))
                }))
                .child((!response.is_empty()).then(|| {
                    text(t(&response)).app_class(tw("ms-3 text-sm text-muted-foreground"))
                })),
        );
    }
    if answer
        .attachments_by_question_id
        .values()
        .any(|attachments| !attachments.is_empty())
    {
        root = root.child(slot_or_unsupported(
            &context.question_attachments,
            row,
            "Answer attachments are unavailable.",
        ));
    }
    root
}

fn failed_work_entry(
    row: &WorkEntryRow,
    error: &TurnItemError,
    context: &WorkRowContext,
) -> AnyElement {
    let warning = error.failure.class
        == t3_contracts::orchestration_v2::OrchestrationV2ProviderFailureClass::UsageLimit;
    let reset = error
        .failure
        .reset_at
        .as_ref()
        .and_then(Option::as_ref)
        .map(|date| date.0.as_str());
    let mut reset_error = None;
    let label = if warning {
        match reset {
            Some(reset) => match context.reset_time.as_ref().map(|format| format(reset)) {
                Some(Ok(time)) => tr(
                    "Usage limit reached. Retry after {time}.",
                    &[("time", Value::from(time))],
                ),
                result => {
                    reset_error = Some(
                        result
                            .and_then(Result::err)
                            .unwrap_or_else(|| t("Local retry time is unavailable.")),
                    );
                    t("Usage limit reached.")
                }
            },
            None => t("Usage limit reached."),
        }
    } else {
        t(&row.entry.label)
    };
    let color = if warning {
        "text-warning"
    } else {
        "text-destructive"
    };
    let trailing = context.timestamp.as_ref().map(|render| render(row));
    let mut root = view().app_class(tw("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 py-0.5 text-left transition-colors"))
        .child(work_log_line(fallback_icon("circle-alert", color, false),
            text(label).app_class(tw(&cn([Some("text-sm font-medium"), Some(color)]))).into_element(),
            view().child(trailing).into_element(), true))
        .child(reset_error.map(|reason| unsupported(&reason).into_element()))
        .child((!warning).then(|| text(t(&error.failure.message.0)).app_class(tw("ms-7 py-1 text-sm leading-relaxed text-foreground/80"))));
    if row.retry_workspace_preparation {
        let action = context.retry_workspace_preparation.clone();
        let disabled = action.is_none();
        let row = row.clone();
        root = root.child(view().app_class(tw("ms-7 pb-1")).child(button(
            ButtonContent::Label(t("Retry")),
            ButtonVariant::Outline,
            ButtonSize::Xs,
            move || disabled,
            move || {
                if let Some(retry) = &action {
                    retry(&row);
                }
            },
        )));
        if disabled {
            root = root.child(unsupported("Workspace preparation retry is unavailable."));
        }
    }
    root.into_element()
}

/// Port of apps/web/src/components/chat/WorkLog.tsx WorkLogLine (selectors moved to children).
fn work_log_line(
    glyph: AnyElement,
    label: AnyElement,
    trailing: AnyElement,
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
                .child(
                    view()
                        .app_class(tw("relative w-6 shrink-0 leading-0"))
                        // NSH has no cap-relative inline baseline; align the fixed icon slot to the row.
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

/// Port of apps/web/src/components/chat/WorkLog.tsx WorkLogDetails, without nested scroll views.
fn work_log_details(kind: &str, content: AnyElement) -> impl IntoElement {
    let class = match kind {
        "panel" => "mt-0.5 mb-1.5 pb-2",
        "media" => "mt-1",
        _ => "ms-7 flex flex-col gap-3 px-0.5 py-1 select-text",
    };
    view()
        .app_class(tw(&cn([Some("cursor-auto"), Some(class)])))
        .child(content)
}

fn slot_or_unsupported(slot: &Option<RowSlot>, row: &WorkEntryRow, reason: &str) -> AnyElement {
    slot.as_ref()
        .map_or_else(|| unsupported(reason).into_element(), |render| render(row))
}

fn unsupported(reason: &str) -> impl IntoElement {
    text(t(reason)).app_class(tw("text-sm text-muted-foreground"))
}

fn fallback_icon(name: &str, class: &str, muted: bool) -> AnyElement {
    let glyph = match name {
        "circle-alert" => Some(Icon::CircleAlert),
        "check" => Some(Icon::Check),
        "square-pen" => Some(Icon::SquarePen),
        "search" => Some(Icon::Search),
        _ => None,
    };
    // UI-ICONS has not supplied the remaining work glyphs; never substitute a different icon.
    view()
        .app_class(tw(&cn([
            Some("flex size-4 items-center justify-center"),
            Some(class),
        ])))
        .child(glyph.map(|glyph| {
            let class = class.to_owned();
            icon(glyph, 16.0, move || {
                let palette = crate::theme::palette();
                let color = match class.as_str() {
                    "text-warning" => palette.warning,
                    "text-destructive" => palette.destructive,
                    "text-muted-foreground" => palette.muted_foreground,
                    "text-foreground/92" => palette.foreground.opacity(0.92),
                    "text-tool-error-icon/40" => crate::theme::current_tokens()
                        .color("tool-error-icon")
                        .unwrap_or(palette.destructive)
                        .opacity(0.4),
                    _ => palette.icon_muted,
                };
                if muted { color.opacity(0.7) } else { color }
            })
        }))
        .into_element()
}

fn unavailable_thread_link(row: &WorkEntryRow) -> Option<AnyElement> {
    let label = match row.entry.projected_item.as_ref().map(|item| &item.item) {
        Some(OrchestrationV2TurnItem::ThreadCreated(_)) => "Open chat",
        Some(OrchestrationV2TurnItem::Notification(notification))
            if t3_client::thread_views::threads::execution_presentation::notification_child_thread_id(&notification.source).is_some() => "Open subagent",
        _ => return None,
    };
    Some(
        view()
            .child(
                nsh_base::button()
                    .disabled(true)
                    .accessibility_label(Some(SharedString::from(t(label))))
                    .child(text(t(label))),
            )
            .child(unsupported("Thread navigation is unavailable."))
            .into_element(),
    )
}

/// Port of TimelineWorkEntryRow.logic.ts workToneIcon.
pub fn work_tone_icon(tone: &str) -> (&'static str, &'static str) {
    match tone {
        "error" => ("circle-alert", "text-foreground/92"),
        "thinking" => ("brain", "text-icon-muted"),
        "info" => ("check", "text-muted-foreground"),
        _ => ("zap", "text-foreground/92"),
    }
}

/// Port of TimelineWorkEntryRow.logic.ts workEntryIconName.
pub fn work_entry_icon_name(entry: &WorkLogEntry) -> String {
    if let Some(OrchestrationV2TurnItem::Notification(notification)) =
        entry.structured_payload.as_deref()
    {
        let notification = serde_json::to_value(notification).unwrap_or_default();
        if notification["outcome"] == "failed" {
            return "circle-alert".into();
        }
        return match notification["source"]["kind"].as_str() {
            Some("subagent" | "delegated_task") => "bot",
            Some("command") => "terminal",
            Some("monitor") => "eye",
            Some("system") => "t3-code",
            _ => "zap",
        }
        .into();
    }
    match entry.item_type.as_deref() {
        Some("user_input_request" | "approval_request") => return "message-circle".into(),
        Some("todo_list") => return "list-todo".into(),
        _ => {}
    }
    if let Some(surface) = entry
        .presentation
        .get("toolSurface")
        .and_then(serde_json::Value::as_str)
    {
        return surface.into();
    }
    if let Some(presentation) = resolve_work_entry_tool_presentation(entry, None) {
        return presentation.icon;
    }
    let action = tool_group_action(entry);
    let name = match action {
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
        "dynamic-tool" => "hammer",
        "reasoning" => "brain",
        "agent-tool" => "bot",
        "tone-tool" => "zap",
        "update" | "mixed" => "hammer",
        _ => match entry.item_type.as_deref() {
            Some("dynamic_tool") => "wrench",
            Some("subagent") => "bot",
            _ => work_tone_icon(&entry.tone).0,
        },
    };
    name.into()
}

/// Port of TimelineWorkEntryRow.logic.ts workEntryRawCommand.
pub fn work_entry_raw_command(entry: &WorkLogEntry) -> Option<&str> {
    let raw = entry.raw_command.as_deref()?.trim();
    let command = entry.command.as_deref()?;
    (!raw.is_empty() && raw != command.trim()).then_some(raw)
}

/// Port of TimelineWorkEntryRow.logic.ts buildToolCallExpandedBody.
pub fn build_tool_call_expanded_body(
    entry: &WorkLogEntry,
    workspace: Option<&str>,
    visible: &str,
    viewed_image: Option<&str>,
) -> Option<String> {
    let mut seen = BTreeSet::from([visible.trim().to_owned()]);
    let mut blocks = Vec::new();
    let mut add = |value: &str| {
        let text = value.trim();
        if !text.is_empty() && seen.insert(text.to_owned()) {
            blocks.push(text.to_owned());
        }
    };
    if entry.item_type.as_deref() == Some("dynamic_tool") && entry.tool_data.is_some() {
        let input = match entry.structured_payload.as_deref() {
            Some(OrchestrationV2TurnItem::DynamicTool(tool)) => Some(&tool.input),
            _ => entry.tool_data.as_ref().and_then(|data| data.get("input")),
        };
        if let Some(input) = input
            && let Ok(input) = serde_json::to_string_pretty(input)
        {
            add(&tr("Tool input\n{input}", &[("input", Value::from(input))]));
        }
    }
    let command = entry.command.as_deref().map(str::trim);
    if command != Some(visible.trim())
        && let Some(command) = work_entry_raw_command(entry).or(command)
    {
        add(command);
    }
    let detail = entry.detail.as_deref().map(str::trim);
    if detail != viewed_image.map(str::trim)
        && let Some(detail) = detail
    {
        add(detail);
    }
    let image_paths: BTreeSet<_> = viewed_image
        .into_iter()
        .flat_map(|path| {
            [
                path.trim().to_owned(),
                format_workspace_relative_path(path, workspace),
            ]
        })
        .collect();
    let mut files = Vec::new();
    for path in entry.changed_files.iter().flatten() {
        let formatted = format_workspace_relative_path(path, workspace);
        if !image_paths.contains(path)
            && !image_paths.contains(&formatted)
            && Some(path.trim()) != detail
            && Some(formatted.as_str()) != detail
            && !files.contains(&formatted)
        {
            files.push(formatted);
        }
    }
    if !files.is_empty() {
        add(&files.join("\n"));
    }
    (!blocks.is_empty()).then(|| blocks.join("\n\n"))
}

/// Port of TimelineWorkGroupRows.tsx WorkingTimelineRow (the existing no-start-time variant).
pub fn working_row() -> impl IntoElement {
    let row = view().app_class(tw("border-b border-border/60 pb-2 pt-1")).child(
        view().app_class(tw("flex h-6 min-w-0 items-baseline gap-2 px-1 text-sm leading-relaxed text-muted-foreground tabular-nums"))
            .child(text(t("Working...")).app_class(tw("relative shrink-0 overflow-hidden whitespace-nowrap"))),
    );
    view().app_class(tw("w-full px-3 sm:px-12")).child(
        view()
            .app_class(tw(
                "mx-auto w-full min-w-0 max-w-(--chat-content-max-width) pb-1.5",
            ))
            .child(row),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn raw_command_requires_a_display_command_and_a_distinct_nonempty_raw_value() {
        for (command, raw, expected) in [
            (None, Some("raw"), None),
            (Some("shown"), None, None),
            (Some("shown"), Some(" \n"), None),
            (Some(" shown "), Some("shown"), None),
            (Some("shown"), Some(" raw \n"), Some("raw")),
        ] {
            let entry = WorkLogEntry {
                command: command.map(str::to_owned),
                raw_command: raw.map(str::to_owned),
                ..Default::default()
            };
            assert_eq!(work_entry_raw_command(&entry), expected);
        }
    }

    #[test]
    fn expanded_body_preserves_source_order_and_omits_visible_and_duplicate_blocks() {
        let mut entry = WorkLogEntry {
            command: Some("shown command".into()),
            raw_command: Some("raw command".into()),
            detail: Some(" output \n".into()),
            changed_files: Some(vec![
                "/repo/a.rs".into(),
                "/repo/b.rs".into(),
                "/repo/a.rs".into(),
            ]),
            ..Default::default()
        };
        assert_eq!(
            build_tool_call_expanded_body(&entry, Some("/repo"), "shown command", None),
            Some("output\n\nrepo/a.rs\nrepo/b.rs".into())
        );
        assert_eq!(
            build_tool_call_expanded_body(&entry, Some("/repo"), "heading", None),
            Some("raw command\n\noutput\n\nrepo/a.rs\nrepo/b.rs".into())
        );
        entry.detail = Some("raw command".into());
        entry.changed_files = None;
        assert_eq!(
            build_tool_call_expanded_body(&entry, None, "heading", None),
            Some("raw command".into())
        );
        assert_eq!(
            build_tool_call_expanded_body(&WorkLogEntry::default(), None, "heading", None),
            None
        );
    }

    #[test]
    fn expanded_body_keeps_null_tool_input_but_suppresses_the_image_path() {
        let entry = WorkLogEntry {
            item_type: Some("dynamic_tool".into()),
            tool_data: Some(json!({"input": null})),
            detail: Some("/repo/image.png".into()),
            changed_files: Some(vec!["/repo/image.png".into(), "other.rs".into()]),
            ..Default::default()
        };
        assert_eq!(
            build_tool_call_expanded_body(
                &entry,
                Some("/repo"),
                "Viewed image",
                Some("/repo/image.png")
            ),
            Some("Tool input\nnull\n\nrepo/other.rs".into())
        );
    }

    #[test]
    fn explicitly_empty_read_output_does_not_offer_an_empty_disclosure() {
        let entry = Shared(Rc::new(WorkLogEntry {
            label: "Read file".into(),
            ..Default::default()
        }));
        let mut row = WorkEntryRow::new(entry, "Read file".into());
        assert!(can_expand_projected_item(&row, "Read file"));
        row.plain_output = Some(None);
        assert!(!can_expand_projected_item(&row, "Read file"));
        row.plain_output = Some(Some("/repo/file.rs".into()));
        assert!(can_expand_projected_item(&row, "Read file"));
    }

    #[test]
    fn work_icons_keep_explicit_surface_and_question_precedence() {
        let mut entry = WorkLogEntry {
            tone: "thinking".into(),
            ..Default::default()
        };
        assert_eq!(work_entry_icon_name(&entry), "brain");
        entry
            .presentation
            .insert("toolSurface".into(), json!("browser"));
        assert_eq!(work_entry_icon_name(&entry), "browser");
        entry.item_type = Some("user_input_request".into());
        assert_eq!(work_entry_icon_name(&entry), "message-circle");
        assert_eq!(
            work_tone_icon("error"),
            ("circle-alert", "text-foreground/92")
        );
    }
}
