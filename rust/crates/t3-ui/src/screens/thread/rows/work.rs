//! Port of apps/web/src/components/chat/TimelineWorkEntryRow.tsx SimpleWorkEntryRow.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;
use t3_client::thread_views::messages_timeline::work_entry::{
    action as tool_group_action, has_detail, plain_output, trim as js_trim,
};
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRow, MessagesTimelineRowContent, live_work_entry_label,
    work_entry_display_label, work_entry_is_visible_in_group,
};
use t3_client::view_logic::session::timestamps::{
    CalendarTimestamp, TimestampHost, TimestampPresenter, resolve_hour12,
};
use t3_client::view_logic::session::{
    WorkLogEntry, work_entry_display_indicates_tool_failure, work_entry_signals_severe_failure,
    work_log_entry_is_tool_like,
};
use t3_client::work_log::{
    has_question_answer, live_thought_line, question_answer_preview, question_answer_text,
    question_text_preview, resolve_work_entry_tool_presentation,
    turn_item_needs_detail_fetch, work_entry_viewed_image_path,
};
use t3_client::workspace::project_ops::paths::format_workspace_relative_path;
use t3_contracts::orchestration_v2::{
    OrchestrationV2TurnItem, OrchestrationV2TurnItemStatus, TurnItemError,
};
use t3_contracts::provider_policy::UserInputAttachmentAnswerPayload;
use t3_contracts::settings::TimestampFormat;
use t3_i18n::{DateFormat, Formats, TimeZone};

use crate::i18n::{Text as I18nText, Value, t, tr};
use crate::runtime::Shared;
use crate::screens::thread::blocks::flatten::{BlockId, BlockKind, MessageFlattener};
use crate::screens::thread::markdown::base::chat_markdown;
use crate::theme::{cn, tw};
use crate::ui::icon::{Icon, icon};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// Construct above the virtual list; both disclosure and markdown state survive recycling.
#[derive(Clone, Copy)]
pub struct WorkRowState {
    expanded: Signal<BTreeSet<String>>,
    reasoning: Stored<BTreeMap<String, ReasoningRender>>,
    owner: Owner,
}

impl Default for WorkRowState {
    fn default() -> Self {
        Self {
            expanded: signal(BTreeSet::new()),
            reasoning: stored(BTreeMap::new()),
            owner: Owner::current(),
        }
    }
}

impl WorkRowState {
    pub fn expanded(self, id: &str) -> bool {
        self.expanded.with(|entries| entries.contains(id))
    }

    pub fn set_expanded(self, id: &str, expanded: bool) {
        self.expanded.update(|entries| {
            if expanded {
                entries.insert(id.to_owned());
            } else {
                entries.remove(id);
            }
        });
    }

    fn render_reasoning(self, entry: &Rc<WorkLogEntry>) -> ReasoningHandle {
        self.owner.run(|| {
            self.reasoning.with(|states| {
                let render = states
                    .entry(entry.id.clone())
                    .or_insert_with(|| ReasoningRender::new(&entry.id));
                render.update(entry);
                render.handle
            })
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkEntryRow {
    pub entry: Shared<WorkLogEntry>,
    pub display_label: I18nText,
    pub compact_reasoning: Option<String>,
    /// None selects the inspector; Some(None) is a read/skill with no plain output.
    pub plain_output: Option<Option<String>>,
    pub question_answer: Option<UserInputAttachmentAnswerPayload>,
    pub workspace_root: Option<String>,
    pub grouped: bool,
    pub pending_approval: bool,
    pub thread_read_label: Option<I18nText>,
    pub viewed_images: Rc<Vec<String>>,
}

impl WorkEntryRow {
    pub fn new(entry: Shared<WorkLogEntry>, display_label: I18nText) -> Self {
        Self {
            entry,
            display_label,
            compact_reasoning: None,
            plain_output: None,
            question_answer: None,
            workspace_root: None,
            grouped: false,
            pending_approval: false,
            thread_read_label: None,
            viewed_images: Rc::default(),
        }
    }

    /// The primary entry; work_timeline_row also mounts every sibling of a grouped Work row.
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let entry = match &row.content {
            MessagesTimelineRowContent::Work {
                grouped_entries, ..
            } => grouped_entries.first()?,
            MessagesTimelineRowContent::WorkLive { entry, .. } => entry,
            _ => return None,
        };
        Some(Self::from_entry(row, entry.clone()))
    }

    fn from_entry(row: &MessagesTimelineRow, entry: Rc<WorkLogEntry>) -> Self {
        let (entries, grouped, display, active) = match &row.content {
            MessagesTimelineRowContent::Work {
                grouped_entries,
                is_expanded_tool_group,
                display_label,
            } => (
                grouped_entries.as_slice(),
                *is_expanded_tool_group,
                display_label.as_deref(),
                None,
            ),
            MessagesTimelineRowContent::WorkLive {
                grouped_entries,
                active,
                ..
            } => (grouped_entries.as_slice(), false, None, Some(*active)),
            _ => (&[][..], false, None, None),
        };
        let reasoning = entry.item_type.as_deref() == Some("reasoning");
        // The compact preview is bounded by the web's UTF-16 limits, even for a live trace.
        let compact_reasoning = reasoning.then(|| {
            let prefix = utf16_prefix(entry.detail.as_deref().unwrap_or(&entry.label), 8192);
            let plain = live_thought_line(prefix);
            let plain = utf16_prefix(&plain, 2048);
            if plain.is_empty() {
                if entry.tool_lifecycle_status.as_deref() == Some("inProgress") {
                    t("Thinking")
                } else {
                    t("Thought")
                }
            } else {
                plain.to_owned()
            }
        });
        let label = if let Some(display) = display {
            display.to_owned()
        } else if let Some(preview) = &compact_reasoning {
            preview.clone()
        } else if let Some(active) = active {
            live_work_entry_label(&entry, None, active)
        } else {
            work_entry_display_label(&entry, None)
        };
        let question_answer = entry
            .presentation
            .get("questionAnswer")
            .filter(|answer| !answer.is_null())
            .and_then(|answer| serde_json::from_value(answer.clone()).ok());
        let plain_output = plain_output(&entry, None);
        let pending_approval = entry.projected_item.as_ref().is_some_and(|item| matches!(
            &item.item, OrchestrationV2TurnItem::ApprovalRequest(request)
                if matches!(request.status, OrchestrationV2TurnItemStatus::Pending | OrchestrationV2TurnItemStatus::Waiting)
        ));
        Self {
            entry: Shared(entry),
            display_label: I18nText::content(label),
            compact_reasoning,
            plain_output,
            question_answer,
            workspace_root: None,
            grouped,
            pending_approval,
            thread_read_label: None,
            viewed_images: Rc::new({
                let mut seen = BTreeSet::new();
                entries
                    .iter()
                    .filter_map(|entry| work_entry_viewed_image_path(entry))
                    .filter(|path| seen.insert(*path))
                    .map(str::to_owned)
                    .collect()
            }),
        }
    }
}

fn utf16_prefix(source: &str, limit: usize) -> &str {
    let mut units = 0;
    for (index, ch) in source.char_indices() {
        units += ch.len_utf16();
        if units > limit {
            return &source[..index];
        }
    }
    source
}

pub type RowSlot = Rc<dyn Fn(&WorkEntryRow) -> AnyElement>;
pub type ToggleAction = Rc<dyn Fn(&str, bool)>;
pub type ContentReady = Rc<dyn Fn(&str)>;
pub type IconRenderer = Rc<dyn Fn(&WorkLogEntry, &str, &str) -> AnyElement>;
pub type ViewedImageRenderer = Rc<dyn Fn(t3_client::work_log::ViewedImageAsset) -> AnyElement>;

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

/// Only later-package renderers remain optional; row presentation, markdown and time are owned here.
#[derive(Clone)]
pub struct WorkRowContext {
    pub state: WorkRowState,
    pub timestamp_format: Memo<TimestampFormat>,
    pub on_toggle_entry: Option<ToggleAction>,
    pub on_content_ready: ContentReady,
    pub icon: Option<IconRenderer>,
    pub image: Option<RowSlot>,
    pub question_attachments: Option<RowSlot>,
    pub pending_approval: Option<RowSlot>,
    pub subagent_notification: Option<RowSlot>,
    pub thread_read: Option<RowSlot>,
    pub trailing_thread_links: Option<RowSlot>,
}

impl WorkRowContext {
    pub fn new(state: WorkRowState) -> Self {
        Self {
            state,
            timestamp_format: computed(|| TimestampFormat::Locale),
            on_toggle_entry: None,
            on_content_ready: Rc::new(|_| {}),
            icon: None,
            image: None,
            question_attachments: None,
            pending_approval: None,
            subagent_notification: None,
            thread_read: None,
            trailing_thread_links: None,
        }
    }
}

pub fn work_entry_row(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
) -> Memo<Option<WorkEntryRow>> {
    computed(move || row.with(|row| row.as_ref().and_then(|row| WorkEntryRow::from_row(row))))
}

/// Mount directly from the client's cell memo; callers never assemble WorkEntryRow fields.
pub fn work_timeline_row(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    context: WorkRowContext,
) -> impl IntoElement {
    let entries = computed(move || {
        row.with(|row| match row.as_ref().map(|row| &row.content) {
            Some(MessagesTimelineRowContent::Work {
                grouped_entries,
                is_expanded_tool_group,
                ..
            }) => grouped_entries
                .iter()
                .filter(|entry| work_entry_is_visible_in_group(entry, *is_expanded_tool_group))
                .cloned()
                .collect::<Vec<_>>(),
            Some(MessagesTimelineRowContent::WorkLive { entry, .. }) => vec![entry.clone()],
            _ => Vec::new(),
        })
    });
    view().child(each(
        move || entries.get(),
        |entry| entry.id.clone(),
        move |entry, _| {
            let prepared = computed(move || {
                row.with(|row| {
                    row.as_ref()
                        .map(|row| WorkEntryRow::from_entry(row, entry.get()))
                })
            });
            simple_work_entry_row(prepared, context.clone())
        },
    ))
}

pub fn simple_work_entry_row(
    row: Memo<Option<WorkEntryRow>>,
    context: WorkRowContext,
) -> impl IntoElement {
    let shape = computed(move || {
        row.with(|row| row.as_ref().map(|row| {
        if row.pending_approval { 1 }
        else if matches!(row.entry.projected_item.as_ref().map(|item| &item.item), Some(OrchestrationV2TurnItem::Error(error)) if error.status == OrchestrationV2TurnItemStatus::Failed) { 2 }
        else { 0 }
    }))
    });
    dynamic(move || match shape.get()? {
        1 => {
            Some(untracked(|| optional_slot(row, context.pending_approval.clone())).into_element())
        }
        2 => Some(
            untracked(|| {
                let context = context.clone();
                dynamic(move || {
                    row.get().and_then(|row| {
                        match row.entry.projected_item.as_ref().map(|item| &item.item) {
                            Some(OrchestrationV2TurnItem::Error(error)) => {
                                Some(failed_work_entry(&row, error, &context))
                            }
                            _ => None,
                        }
                    })
                })
            })
            .into_element(),
        ),
        _ => Some(untracked(|| work_entry_log_row(row, context.clone())).into_element()),
    })
}

fn optional_slot(row: Memo<Option<WorkEntryRow>>, slot: Option<RowSlot>) -> impl IntoElement {
    dynamic(move || {
        row.get()
            .and_then(|row| slot.as_ref().map(|render| render(&row)))
    })
}

fn work_entry_log_row(
    row: Memo<Option<WorkEntryRow>>,
    context: WorkRowContext,
) -> impl IntoElement {
    let state = context.state;
    let expanded = computed(move || {
        row.with(|row| {
            row.as_ref()
                .is_some_and(|row| state.expanded(&row.entry.id))
        })
    });
    let header_context = context.clone();
    let header = dynamic(move || {
        let row = row.get()?;
        let open = expanded.get();
        Some(work_entry_header(&row, open, &header_context))
    });
    let detail_kind = computed(move || {
        row.with(|row| {
            row.as_ref().map(|row| {
                (
                    row.entry.id.clone(),
                    expanded.get(),
                    row.entry.item_type.as_deref() == Some("reasoning"),
                )
            })
        })
    });
    view().app_class(move || row.with(|row| row.as_ref().map(|row| tw(&cn([
        Some("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 text-left transition-colors"),
        Some(if row.grouped { "py-0" } else { "py-0.5" }),
    ]))))).child(header).child(dynamic(move || {
        let (_, open, reasoning) = detail_kind.get()?;
        if !open { return None; }
        Some(untracked(|| {
            let mut details = view();
            if reasoning {
                details = details.child(reasoning_trace_content(row, context.state));
            } else {
                let context = context.clone();
                details = details.child(dynamic(move || row.get().map(|row| {
                    let mut root = view();
                    if work_entry_viewed_image_path(&row.entry).is_some() {
                        root = root.child(context.image.as_ref().map(|render| work_log_details("media", render(&row))));
                    }
                    if let Some(answer) = &row.question_answer {
                        root = root.child(question_answer_history(answer, &row, &context));
                    } else {
                        root = root.child(expanded_tool_content(&row));
                    }
                    root
                })));
            }
            details
        }).into_element())
    }))
}

fn work_entry_header(row: &WorkEntryRow, expanded: bool, context: &WorkRowContext) -> AnyElement {
    let entry = &row.entry;
    let warning = entry.source_activity_kind.as_deref() == Some("runtime.warning");
    let failed = !warning && work_entry_display_indicates_tool_failure(entry);
    let destructive = !warning
        && failed
        && (work_entry_signals_severe_failure(entry) || !work_log_entry_is_tool_like(entry));
    let reasoning = entry.item_type.as_deref() == Some("reasoning");
    let question = row.question_answer.as_ref();
    let question_heading = question.map(question_text_preview).unwrap_or_default();
    let preview = if reasoning && expanded {
        I18nText::from(
            if entry.tool_lifecycle_status.as_deref() == Some("inProgress") {
                "Thinking"
            } else {
                "Thought"
            },
        )
        .resolve()
        .to_string()
    } else if !question_heading.is_empty() {
        question_heading
    } else {
        row.display_label.resolve().to_string()
    };
    let answer = question
        .filter(|answer| has_question_answer(answer))
        .map(question_answer_preview);
    let accessible = [
        row.thread_read_label
            .as_ref()
            .map(I18nText::resolve)
            .map(|label| label.to_string())
            .unwrap_or(preview.clone()),
        answer.clone().unwrap_or_default(),
    ]
    .into_iter()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(": ");
    let accessible = if failed {
        tr(
            "{preview}, tool call failed",
            &[("preview", Value::from(accessible))],
        )
    } else {
        accessible
    };
    let can_expand = can_expand_projected_item(row, &preview);
    let heading = if warning {
        "font-medium text-warning"
    } else if destructive {
        "font-medium text-destructive"
    } else if work_log_entry_is_tool_like(entry) {
        "text-secondary-label"
    } else {
        "text-foreground/80"
    };
    let icon_class = if warning {
        "text-warning"
    } else if destructive {
        "text-destructive"
    } else if failed {
        "text-tool-error-icon/40"
    } else if entry.tone == "tool" {
        "text-icon-muted"
    } else {
        work_tone_icon(&entry.tone).1
    };
    let icon_name = if warning || destructive {
        "circle-alert".to_owned()
    } else {
        work_entry_icon_name(entry)
    };
    let glyph = context.icon.as_ref().map_or_else(
        || fallback_icon(&icon_name, icon_class, true),
        |render| render(entry, &icon_name, icon_class),
    );
    let label = if reasoning && !expanded {
        row.compact_reasoning
            .as_deref()
            .unwrap_or(&preview)
            .to_owned()
    } else {
        preview.clone()
    };
    let label = view()
        .app_class(tw("min-w-0 flex-1 overflow-hidden"))
        .child(
            view()
                .app_class(tw(
                    "flex min-w-0 w-full items-baseline gap-1.5 text-sm leading-relaxed",
                ))
                .child(
                    text(SharedString::from(label))
                        .number_of_lines(1u32)
                        .app_class(tw(&cn([
                            Some("min-w-0 truncate"),
                            answer.is_none().then_some("flex-1"),
                            Some(heading),
                        ]))),
                )
                .child(answer.map(|answer| {
                    text(SharedString::from(answer))
                        .number_of_lines(1u32)
                        .app_class(tw(&cn([
                            Some("min-w-0 truncate"),
                            Some(if expanded {
                                "text-muted-foreground"
                            } else {
                                "text-foreground"
                            }),
                        ])))
                })),
        );
    let trailing = view()
        .app_class(tw("flex items-center gap-1.5"))
        .child(can_expand.then(|| {
            view()
                .app_class(tw("flex size-4 shrink-0 items-center justify-center"))
                .child(icon(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    12.0,
                    || crate::theme::palette().icon_muted.opacity(0.7),
                ))
        }))
        .child(timeline_row_timestamp(
            entry.created_at.clone(),
            context.timestamp_format,
            false,
        ));
    let line = work_log_line(glyph, label.into_element(), trailing.into_element(), false);
    let header = if can_expand {
        let entry_id = entry.id.clone();
        let state = context.state;
        let toggle = context.on_toggle_entry.clone();
        nsh_base::button().accessibility_label(Some(SharedString::from(accessible)))
            .app_class(tw("w-full cursor-pointer text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70"))
            .test_id(&format!("work-entry-{}", entry.id)).on_press(move || {
                let next = !state.expanded(&entry_id);
                state.set_expanded(&entry_id, next);
                if let Some(toggle) = &toggle { toggle(&entry_id, !next); }
            }).child(line).into_element()
    } else {
        line.into_element()
    };
    // Native button activation excludes selectable details and independently interactive links.
    view()
        .child(header)
        .child(
            context
                .trailing_thread_links
                .as_ref()
                .map(|render| render(row)),
        )
        .child(
            context
                .thread_read
                .as_ref()
                .filter(|_| row.thread_read_label.is_some())
                .map(|render| render(row)),
        )
        .into_element()
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
    if item.is_some() {
        return if entry.item_type.as_deref() == Some("reasoning") {
            entry
                .detail
                .as_ref()
                .is_some_and(|detail| !js_trim(detail).is_empty())
        } else {
            image || row.question_answer.is_some() || has_detail(entry)
        };
    }
    !js_trim(preview).is_empty()
        || (entry.item_type.as_deref() == Some("dynamic_tool") && entry.tool_data.is_some())
        || work_entry_raw_command(entry).is_some()
        || entry
            .command
            .as_ref()
            .is_some_and(|text| !js_trim(text).is_empty())
        || entry
            .detail
            .as_ref()
            .is_some_and(|text| !js_trim(text).is_empty())
        || entry
            .changed_files
            .as_ref()
            .is_some_and(|files| !files.is_empty())
        || image
}

fn expanded_tool_content(row: &WorkEntryRow) -> AnyElement {
    // TL-OUTPUT's inspector/full output and retry wait for ClientRuntime/Backend U8.
    if row.entry.projected_item.is_some() && row.plain_output.is_none() {
        return view().into_element();
    }
    let body = row.plain_output.clone().unwrap_or_else(|| {
        build_tool_call_expanded_body(
            &row.entry,
            row.workspace_root.as_deref(),
            &row.display_label.resolve(),
            work_entry_viewed_image_path(&row.entry),
        )
    });
    work_log_details(
        "panel",
        view()
            .child(body.map(|body| {
                text(SharedString::from(body))
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
            .into_element(),
    )
    .into_element()
}

#[derive(Clone, PartialEq)]
struct ReasoningLeaf {
    text: Rc<str>,
    streaming: bool,
}

#[derive(Clone, Copy)]
struct ReasoningHandle {
    keys: Signal<Vec<BlockId>>,
    slots: Stored<Vec<Signal<Shared<ReasoningLeaf>>>>,
}

impl PartialEq for ReasoningHandle {
    fn eq(&self, other: &Self) -> bool {
        self.keys == other.keys
    }
}

struct ReasoningRender {
    flattener: MessageFlattener,
    handle: ReasoningHandle,
    last_entry: Option<Rc<WorkLogEntry>>,
}

impl ReasoningRender {
    fn new(id: &str) -> Self {
        Self {
            flattener: MessageFlattener::new(id),
            handle: ReasoningHandle {
                keys: signal(Vec::new()),
                slots: stored(Vec::new()),
            },
            last_entry: None,
        }
    }

    fn update(&mut self, entry: &Rc<WorkLogEntry>) {
        if self
            .last_entry
            .as_ref()
            .is_some_and(|old| Rc::ptr_eq(old, entry))
        {
            return;
        }
        let source = entry.detail.as_deref().unwrap_or_default();
        let streaming = entry.tool_lifecycle_status.as_deref() == Some("inProgress");
        let mut delta = if self
            .last_entry
            .as_ref()
            .is_some_and(|old| old.tool_lifecycle_status.as_deref() != Some("inProgress"))
            && !streaming
        {
            self.flattener.replace(&entry.id, source)
        } else {
            self.flattener.ingest(&entry.id, source)
        };
        if !streaming && let Some(finished) = self.flattener.finish() {
            delta = finished;
        }
        let count = delta.document.block_count();
        let old_last = self.handle.slots.with(|slots| slots.len().checked_sub(1));
        let changed: BTreeSet<_> = delta.rows.iter().map(|block| block.id.index).collect();
        self.handle.slots.with(|slots| {
            for block in &delta.rows {
                let text: Rc<str> = match &block.kind {
                    BlockKind::Header => Rc::from(""),
                    BlockKind::Document if streaming => Rc::from(""),
                    BlockKind::Code(code) => Rc::from(format!(
                        "\x60\x60\x60{}\n{}\n\x60\x60\x60",
                        code.language.as_deref().unwrap_or_default(),
                        code.text
                    )),
                    _ => Rc::from(
                        delta
                            .document
                            .source_range(block.source.bytes.clone())
                            .unwrap_or_default(),
                    ),
                };
                let value = Shared(Rc::new(ReasoningLeaf {
                    text,
                    streaming: streaming && block.id.index + 1 == count,
                }));
                if block.id.index == slots.len() {
                    slots.push(signal(value));
                } else {
                    slots[block.id.index].set(value);
                }
            }
            slots.truncate(count);
            if let Some(index) = old_last.filter(|index| *index < count && !changed.contains(index))
            {
                let live = streaming && index + 1 == count;
                slots[index].update(|leaf| {
                    if leaf.streaming != live {
                        *leaf = Shared(Rc::new(ReasoningLeaf {
                            text: leaf.text.clone(),
                            streaming: live,
                        }));
                    }
                });
            }
        });
        if self.handle.keys.with_untracked(Vec::len) != count {
            self.handle.keys.set(
                (0..count)
                    .filter_map(|index| delta.document.block(index).map(|block| block.id.clone()))
                    .collect(),
            );
        }
        self.last_entry = Some(entry.clone());
    }
}

pub fn reasoning_trace_content(
    row: Memo<Option<WorkEntryRow>>,
    state: WorkRowState,
) -> impl IntoElement {
    let handle = computed(move || {
        row.with(|row| row.as_ref().map(|row| state.render_reasoning(&row.entry.0)))
    });
    work_log_details(
        "text",
        dynamic(move || {
            handle.get().map(|handle| {
                untracked(|| {
                    each(
                        move || handle.keys.get(),
                        |id| id.clone(),
                        move |id, _| {
                            let slot = handle.slots.with(|slots| slots[id.get_untracked().index]);
                            chat_markdown(
                                move || slot.with(|leaf| leaf.text.to_string()),
                                move || slot.with(|leaf| leaf.streaming),
                            )
                        },
                    )
                })
            })
        })
        .into_element(),
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
                    text(SharedString::from(question.clone()))
                        .app_class(tw("text-sm text-muted-foreground"))
                }))
                .child((!response.is_empty()).then(|| {
                    text(SharedString::from(response))
                        .app_class(tw("ms-3 text-sm text-muted-foreground"))
                })),
        );
    }
    if answer
        .attachments_by_question_id
        .values()
        .any(|attachments| !attachments.is_empty())
    {
        root = root.child(
            context
                .question_attachments
                .as_ref()
                .map(|render| render(row)),
        );
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
    let reset = error.failure.reset_at.as_ref().and_then(Option::as_ref);
    let label = if warning {
        let time = reset
            .map(|reset| {
                let zone = locale_settings().time_zone.to_string();
                let label = TimestampPresenter(WorkTimestampHost).day_aware(
                    &reset.0,
                    &context.timestamp_format.get(),
                    nsh_core::time::unix_ms() as i64,
                    Some(&zone),
                    true,
                );
                if label.is_empty() {
                    String::new()
                } else {
                    format!("{label} ({zone})")
                }
            })
            .filter(|time| !time.is_empty());
        match time {
            Some(time) => tr(
                "Usage limit reached. Retry after {time}.",
                &[("time", Value::from(time))],
            ),
            None => t("Usage limit reached."),
        }
    } else {
        I18nText::client_key(row.entry.label.clone())
            .resolve()
            .to_string()
    };
    let color = if warning {
        "text-warning"
    } else {
        "text-destructive"
    };
    let detail = (!warning).then(|| {
        text(SharedString::from(error.failure.message.0.clone()))
            .app_class(tw("ms-7 py-1 text-sm leading-relaxed text-foreground/80"))
    });
    view().app_class(tw("group/timeline-row relative w-full min-w-0 rounded-md px-0.5 py-0.5 text-left transition-colors"))
        .child(work_log_line(fallback_icon("circle-alert", color, false), text(SharedString::from(label)).app_class(tw(&cn([Some("text-sm font-medium"), Some(color)]))).into_element(),
            timeline_row_timestamp(row.entry.created_at.clone(), context.timestamp_format, true).into_element(), true))
        .child(detail).into_element()
}

pub fn timeline_row_timestamp(
    created_at: String,
    format: Memo<TimestampFormat>,
    always_visible: bool,
) -> impl IntoElement {
    let tooltip_at = created_at.clone();
    text(move || SharedString::from(TimestampPresenter(WorkTimestampHost).day_aware(&created_at, &format.get(), nsh_core::time::unix_ms() as i64, None, false)))
        .app_class(tw(&cn([
            Some("pointer-events-none absolute me-1 shrink-0 whitespace-nowrap rounded-md text-muted-foreground text-xs tabular-nums opacity-0 group-hover/timeline-row:pointer-events-auto group-hover/timeline-row:static group-hover/timeline-row:opacity-100 group-focus-within/timeline-row:pointer-events-auto group-focus-within/timeline-row:static group-focus-within/timeline-row:opacity-100"),
            always_visible.then_some("pointer-events-auto static opacity-100"),
        ])))
        .tooltip(tooltip(tooltip_create_handle(), TooltipOptions::default(), text(move || SharedString::from(TimestampPresenter(WorkTimestampHost).tooltip(&tooltip_at, &format.get())))))
}

struct WorkTimestampHost;
impl WorkTimestampHost {
    fn zone(zone: Option<&str>) -> TimeZone {
        let settings = locale_settings();
        let zone = zone.unwrap_or_else(|| settings.time_zone.as_str());
        if matches!(zone, "UTC" | "Etc/UTC" | "GMT") {
            TimeZone::Utc
        } else {
            TimeZone::Iana(zone.into())
        }
    }
    fn formats() -> Formats {
        Formats::new(locale().as_str())
    }
    fn hour12(format: &TimestampFormat) -> bool {
        resolve_hour12(format, locale_settings().hour_cycle.twelve_hour())
    }
}
impl TimestampHost for WorkTimestampHost {
    fn parse_epoch_ms(&self, iso: &str) -> Option<i64> {
        WorkTimestampHost::zone(None)
            .is_known()
            .then(|| t3_client::time::epoch_ms(iso))
            .flatten()
    }
    fn calendar(&self, epoch_ms: i64, zone: Option<&str>) -> CalendarTimestamp {
        let day = t3_i18n::format::civil(epoch_ms, &Self::zone(zone)).day;
        CalendarTimestamp {
            epoch_ms,
            day_index: day.index,
            year: day.year,
            month: day.month,
            day: day.day,
        }
    }
    fn short_time(&self, epoch_ms: i64, format: &TimestampFormat, zone: Option<&str>) -> String {
        Self::formats().date(
            DateFormat::TimeHm,
            epoch_ms,
            &Self::zone(zone),
            Some(Self::hour12(format)),
        )
    }
    fn numeric_date(&self, epoch_ms: i64, year: bool, zone: Option<&str>) -> String {
        Self::formats().date(
            if year {
                DateFormat::MonthDayYearNumeric
            } else {
                DateFormat::MonthDayNumeric
            },
            epoch_ms,
            &Self::zone(zone),
            None,
        )
    }
    fn full_timestamp(&self, iso: &str, format: &TimestampFormat) -> String {
        self.parse_epoch_ms(iso)
            .map(|epoch| {
                Self::formats().date(
                    DateFormat::DefaultDateTime,
                    epoch,
                    &Self::zone(None),
                    Some(Self::hour12(format)),
                )
            })
            .unwrap_or_default()
    }
    fn locale_hour12(&self) -> bool {
        locale_settings().hour_cycle.twelve_hour()
    }
}

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

fn work_log_details(kind: &str, content: AnyElement) -> impl IntoElement {
    // Native timeline rows never own an inner scroll container; TL-BLOCKS-3 owns virtualization.
    let class = match kind {
        "panel" => "mt-0.5 mb-1.5 pb-2",
        "media" => "mt-1",
        _ => "ms-7 flex flex-col gap-3 px-0.5 py-1 select-text",
    };
    view()
        .app_class(tw(&cn([Some("cursor-auto"), Some(class)])))
        .child(content)
}

fn fallback_icon(name: &str, class: &str, muted: bool) -> AnyElement {
    let class = class.to_owned();
    let source = match name {
        "browser" | "computer" => Some(browser_app_icon()),
        "t3-code" => Some(t3_wordmark()),
        name => Icon::from_name(match name {
            "code" => "code-xml",
            "device" => "smartphone",
            "pull-request" => "git-pull-request-arrow",
            name => name,
        })
        .map(Icon::asset),
    };
    view()
        .app_class(tw("flex size-4 items-center justify-center"))
        .child(source.map(|source| {
            image(source)
                .w(px(16.0))
                .h(px(16.0))
                .flex_shrink_0()
                .accessible(false)
                .tint_color(move || {
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
                    if !muted {
                        return color;
                    }
                    // NSH cannot resolve light:brightness-60; apply the same light RGB multiplier.
                    let Rgba { r, g, b, a } = color.light.into();
                    AdaptiveColor {
                        light: Rgba {
                            r: r * 0.6,
                            g: g * 0.6,
                            b: b * 0.6,
                            a,
                        }
                        .into(),
                        dark: color.dark,
                    }
                    .opacity(0.7)
                })
        }))
        .into_element()
}

/// Port of TimelineToolActivityIcon.tsx BrowserAppIcon.
fn browser_app_icon() -> ImageSrc {
    ImageSrc::svg_markup(
        r##"<svg xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.9"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden
    >
      <path d="M8.5 19H7.2C4.4 19 3 17.5 3 14.6V7.4C3 4.5 4.5 3 7.4 3h8.2C18.5 3 20 4.5 20 7.4v2.4" />
      <circle cx="7.4" cy="7.2" r="0.75" fill="currentColor" stroke="none" />
      <path d="M11.2 7.2h4.3" />
      <path d="m12.4 11.4 7.5 2.6-3.4 1.6-1.5 3.6z" fill="currentColor" stroke="none" />
    </svg>"##,
    )
}

/// Port of apps/web/src/components/T3Wordmark.tsx T3Wordmark.
fn t3_wordmark() -> ImageSrc {
    ImageSrc::svg_markup(
        r##"<svg  viewBox="15.5309 37 94.3941 56.96" xmlns="http://www.w3.org/2000/svg">
      <path
        d="M33.4509 93V47.56H15.5309V37H64.3309V47.56H46.4109V93H33.4509ZM86.7253 93.96C82.832 93.96 78.9653 93.4533 75.1253 92.44C71.2853 91.3733 68.032 89.88 65.3653 87.96L70.4053 78.04C72.5386 79.5867 75.0186 80.8133 77.8453 81.72C80.672 82.6267 83.5253 83.08 86.4053 83.08C89.6586 83.08 92.2186 82.44 94.0853 81.16C95.952 79.88 96.8853 78.12 96.8853 75.88C96.8853 73.7467 96.0586 72.0667 94.4053 70.84C92.752 69.6133 90.0853 69 86.4053 69H80.4853V60.44L96.0853 42.76L97.5253 47.4H68.1653V37H107.365V45.4L91.8453 63.08L85.2853 59.32H89.0453C95.9253 59.32 101.125 60.8667 104.645 63.96C108.165 67.0533 109.925 71.0267 109.925 75.88C109.925 79.0267 109.099 81.9867 107.445 84.76C105.792 87.48 103.259 89.6933 99.8453 91.4C96.432 93.1067 92.0586 93.96 86.7253 93.96Z"
        fill="currentColor"
      />
    </svg>"##,
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
    if let Some(OrchestrationV2TurnItem::DynamicTool(tool)) = entry.structured_payload.as_deref()
        && t3_client::thread_views::messages_timeline::work_entry::is_javascript_repl_tool(
            tool.tool_name.as_ref().map(|name| name.0.as_str()),
        )
    {
        return "code".into();
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
    let raw = js_trim(entry.raw_command.as_deref()?);
    let command = entry.command.as_deref()?;
    (!raw.is_empty() && raw != js_trim(command)).then_some(raw)
}

/// Port of TimelineWorkEntryRow.logic.ts buildToolCallExpandedBody.
pub fn build_tool_call_expanded_body(
    entry: &WorkLogEntry,
    workspace: Option<&str>,
    visible: &str,
    viewed_image: Option<&str>,
) -> Option<String> {
    let mut seen = BTreeSet::from([js_trim(visible).to_owned()]);
    let mut blocks = Vec::new();
    let mut add = |value: &str| {
        let text = js_trim(value);
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
    let command = entry.command.as_deref().map(js_trim);
    if command != Some(js_trim(visible))
        && let Some(command) = work_entry_raw_command(entry).or(command)
    {
        add(command);
    }
    let detail = entry.detail.as_deref().map(js_trim);
    if detail != viewed_image.map(js_trim)
        && let Some(detail) = detail
    {
        add(detail);
    }
    let image_paths: BTreeSet<_> = viewed_image
        .into_iter()
        .flat_map(|path| {
            [
                js_trim(path).to_owned(),
                format_workspace_relative_path(path, workspace),
            ]
        })
        .collect();
    let mut files = Vec::new();
    for path in entry.changed_files.iter().flatten() {
        let formatted = format_workspace_relative_path(path, workspace);
        if !image_paths.contains(path)
            && !image_paths.contains(&formatted)
            && Some(js_trim(path)) != detail
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
            (Some("shown"), Some("\u{feff}"), None),
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
