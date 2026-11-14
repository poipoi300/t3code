//! Port of apps/web/src/components/chat/TimelineAssistantRows.tsx AssistantTimelineRow and meta.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::state::items;
use t3_client::thread_views::messages_timeline::{
    MessagesTimelineRow, MessagesTimelineRowContent, resolve_assistant_message_copy_state,
};
use t3_client::thread_views::threads::workflows::can_fork_projected_assistant_item;
use t3_client::types::ThreadKey;
use t3_client::view_logic::session::ChatMessage;
use t3_client::view_logic::session::timestamps::{
    CalendarTimestamp, TimestampHost, TimestampPresenter, resolve_hour12,
};
use t3_contracts::base_schemas::{RunId, ThreadId};
use t3_contracts::orchestration_v2::{
    OrchestrationV2ProjectedTurnItem, OrchestrationV2ProviderCapabilities,
    OrchestrationV2TurnItemStatus,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;
use t3_contracts::settings::TimestampFormat;
use t3_i18n::{DateFormat, Formats, TimeZone};

use crate::i18n::t;
use crate::runtime::Shared;
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::Icon;
use crate::ui::toast::{ANCHORED_COPY_TOAST_TIMEOUT_MS, show_anchored_copy_success_toast};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// Display decisions belong to the client's MessagesTimeline projection, not individual cells.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantRow {
    pub id: String,
    pub message: Rc<ChatMessage>,
    pub projected_item: Option<Rc<OrchestrationV2ProjectedTurnItem>>,
    pub suppress_empty_response: bool,
    pub show_assistant_meta: bool,
    pub show_assistant_copy_button: bool,
    pub assistant_copy_streaming: bool,
    pub assistant_turn_diff_summary: Option<serde_json::Value>,
}

impl AssistantRow {
    pub fn from_row(row: &MessagesTimelineRow) -> Option<Self> {
        let (
            message,
            projected_item,
            show_meta,
            suppress_empty,
            show_copy,
            copy_streaming,
            summary,
        ) = match &row.content {
            MessagesTimelineRowContent::Message {
                message,
                projected_item,
                show_assistant_meta,
                suppress_empty_response,
                show_assistant_copy_button,
                assistant_copy_streaming,
                assistant_turn_diff_summary,
                ..
            } => (
                message,
                projected_item,
                *show_assistant_meta,
                suppress_empty_response.unwrap_or(false),
                *show_assistant_copy_button,
                *assistant_copy_streaming,
                assistant_turn_diff_summary.clone(),
            ),
            MessagesTimelineRowContent::AssistantMeta {
                message,
                projected_item,
                show_assistant_copy_button,
                assistant_copy_streaming,
            } => (
                message,
                projected_item,
                true,
                false,
                *show_assistant_copy_button,
                *assistant_copy_streaming,
                None,
            ),
            _ => return None,
        };
        if message.role != "assistant" {
            return None;
        }
        Some(Self {
            id: row.id.clone(),
            message: message.clone(),
            projected_item: projected_item.clone(),
            suppress_empty_response: suppress_empty,
            show_assistant_meta: show_meta,
            show_assistant_copy_button: show_copy,
            assistant_copy_streaming: copy_streaming,
            assistant_turn_diff_summary: summary,
        })
    }
}

/// Inputs to ChatMarkdown, including the originating reply for relative-file lookup.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantMarkdown {
    pub text: String,
    pub is_streaming: bool,
    pub render_insights: bool,
    pub line_breaks: bool,
    pub heading_level_offset: u8,
    pub cwd: Option<String>,
    pub thread_ref: Option<ThreadKey>,
    pub projected_item: Option<Rc<OrchestrationV2ProjectedTurnItem>>,
    pub agent_directories: Option<AgentDirectories>,
    pub message_id: Option<IdOrIndex<t3_contracts::base_schemas::MessageId>>,
    pub item_key: String,
}

pub type MarkdownRenderer = Rc<dyn Fn(Memo<AssistantMarkdown>) -> AnyElement>;
pub type RowSlot = Rc<dyn Fn(Memo<Option<AssistantRow>>) -> AnyElement>;
pub type DirectoryLookup = Rc<dyn Fn(&OrchestrationV2ProjectedTurnItem) -> Vec<String>>;

#[derive(Clone)]
pub struct AgentDirectories {
    projected_item: Rc<OrchestrationV2ProjectedTurnItem>,
    lookup: DirectoryLookup,
}

impl AgentDirectories {
    pub fn resolve(&self) -> Vec<String> {
        (self.lookup)(&self.projected_item)
    }
}

impl PartialEq for AgentDirectories {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.projected_item, &other.projected_item)
            && Rc::ptr_eq(&self.lookup, &other.lookup)
    }
}

impl std::fmt::Debug for AgentDirectories {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentDirectories")
            .field("projected_item", &self.projected_item.source_item_id)
            .finish_non_exhaustive()
    }
}
pub type CapabilityLookup =
    Rc<dyn Fn(&OrchestrationV2ProjectedTurnItem) -> Option<OrchestrationV2ProviderCapabilities>>;
pub type ForkHandler = Rc<dyn Fn(ForkRequest) -> LocalBoxFuture<'static, Result<(), String>>>;
pub type ReplyHandler = Rc<dyn Fn(ReplyRequest) -> Result<(), String>>;

#[derive(Clone, Debug, PartialEq)]
pub struct ForkRequest {
    pub source_thread_id: IdOrIndex<ThreadId>,
    pub run_id: IdOrIndex<RunId>,
}

/// The selection/composer owner captures the opening and builds the response context record.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplyRequest {
    pub thread_ref: ThreadKey,
    pub message: Rc<ChatMessage>,
    pub projected_item: Rc<OrchestrationV2ProjectedTurnItem>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ActionState {
    busy: bool,
    copied: bool,
}

/// Construct once above the virtual list; a recycled cell never owns a response's busy state.
#[derive(Clone, Copy)]
pub struct AssistantRowsState {
    actions: Signal<BTreeMap<String, ActionState>>,
    owner: Owner,
    copy_resets: Stored<BTreeMap<String, Spawned<()>>>,
}

impl Default for AssistantRowsState {
    fn default() -> Self {
        Self {
            actions: signal(BTreeMap::new()),
            owner: Owner::current(),
            copy_resets: stored(BTreeMap::new()),
        }
    }
}

impl AssistantRowsState {
    fn action(self, id: &str) -> ActionState {
        self.actions
            .with(|state| state.get(id).cloned().unwrap_or_default())
    }

    fn begin(self, id: &str) -> bool {
        let mut started = false;
        self.actions.update(|state| {
            let action = state.entry(id.into()).or_default();
            if !action.busy {
                action.busy = true;
                started = true;
            }
        });
        started
    }

    fn finish(self, id: &str, result: Result<(), String>) {
        self.actions.update(|state| {
            state.entry(id.into()).or_default().busy = false;
        });
        if let Err(error) = result {
            log::warn!("fork response failed: {error}");
        }
    }

    fn copied(self, id: &str, anchor: ElementRef) {
        if let Some(reset) = self.copy_resets.with(|resets| resets.remove(id)) {
            reset.cancel();
        }
        self.actions
            .update(|actions| actions.entry(id.into()).or_default().copied = true);
        show_anchored_copy_success_toast(Some(&anchor));
        let id = id.to_owned();
        let key = id.clone();
        let reset = self.owner.run(move || {
            set_timeout(
                Duration::from_millis(ANCHORED_COPY_TOAST_TIMEOUT_MS),
                move || {
                    self.actions
                        .update(|actions| actions.entry(id.clone()).or_default().copied = false);
                    self.copy_resets.with(|resets| resets.remove(&id));
                },
            )
        });
        self.copy_resets.with(|resets| resets.insert(key, reset));
    }
}

#[derive(Clone)]
pub struct AssistantRowContext {
    pub thread_ref: Option<ThreadKey>,
    pub markdown_cwd: Option<String>,
    markdown: MarkdownRenderer,
    pub agent_directories: Option<DirectoryLookup>,
    pub timestamp_format: TimestampFormat,
    pub provider_capabilities: Option<CapabilityLookup>,
    pub on_fork_from_run: Option<ForkHandler>,
    pub on_reply_to_response: Option<ReplyHandler>,
    pub changed_files: Option<RowSlot>,
    pub state: AssistantRowsState,
}

impl AssistantRowContext {
    pub fn for_timeline(state: AssistantRowsState) -> Self {
        Self {
            thread_ref: None,
            markdown_cwd: None,
            markdown: Rc::new(|markdown| assistant_markdown(markdown).into_element()),
            agent_directories: None,
            timestamp_format: TimestampFormat::Locale,
            provider_capabilities: None,
            on_fork_from_run: None,
            on_reply_to_response: None,
            changed_files: None,
            state,
        }
    }

    /// Retains the integrator's Markdown/code-block root while TL-MARKDOWN-BASE is in flight.
    pub fn new(markdown: MarkdownRenderer, state: AssistantRowsState) -> Self {
        Self {
            markdown,
            ..Self::for_timeline(state)
        }
    }
}

/// Adapts the runtime's per-entry memo without cloning a message on every streamed token.
pub fn assistant_row(
    entry: Memo<Option<Shared<MessagesTimelineRow>>>,
) -> Memo<Option<AssistantRow>> {
    computed(move || {
        entry.with(|entry| {
            entry
                .as_ref()
                .and_then(|entry| AssistantRow::from_row(entry))
        })
    })
}

pub fn assistant_timeline_row(
    row: Memo<Option<AssistantRow>>,
    context: AssistantRowContext,
) -> impl IntoElement {
    let markdown_context = context.clone();
    let markdown = computed(move || {
        let row = row.get();
        let text = row.as_ref().map_or_else(String::new, |row| {
            if row.message.text.is_empty() && !row.message.streaming && !row.suppress_empty_response
            {
                t("(empty response)")
            } else {
                row.message.text.clone()
            }
        });
        markdown_input(text, row.as_ref(), &markdown_context, true)
    });
    let meta_context = context.clone();
    let changed_files = context.changed_files.clone();
    let show_meta = computed(move || row.get().is_some_and(|row| row.show_assistant_meta));
    let show_interrupted = computed(move || {
        row.get()
            .is_some_and(|row| !row.show_assistant_meta && interrupted(&row))
    });
    // MessageAuthorHeading is sr-only: NSH carries it as the parent's accessibility label.
    view()
        .label(SharedString::from(t("T3 Code")))
        .app_class(tw("relative min-w-0 px-1 py-0.5"))
        // NSH find/citation owners receive the item/message IDs instead of DOM data attributes.
        .child(view().child((context.markdown)(markdown)))
        .child(changed_files.map(|render| assistant_changed_files_section(row, render)))
        .child(dynamic(move || {
            show_interrupted.get().then(|| {
                view()
                    .app_class(tw("mt-1.5 flex"))
                    .child(assistant_status_badge(
                        OrchestrationV2TurnItemStatus::Interrupted,
                    ))
            })
        }))
        .child(dynamic(move || {
            show_meta.get().then(|| {
                untracked(|| assistant_message_meta(row, meta_context.clone(), "mt-1.5", false))
            })
        }))
}

fn markdown_input(
    text: String,
    row: Option<&AssistantRow>,
    context: &AssistantRowContext,
    assistant: bool,
) -> AssistantMarkdown {
    let projected_item = row.and_then(|row| row.projected_item.clone());
    let agent_directories = if assistant {
        context
            .thread_ref
            .as_ref()
            .and(projected_item.as_ref())
            .and_then(|item| {
                context
                    .agent_directories
                    .as_ref()
                    .map(|lookup| AgentDirectories {
                        projected_item: item.clone(),
                        lookup: lookup.clone(),
                    })
            })
    } else {
        None
    };
    AssistantMarkdown {
        line_breaks: should_preserve_assistant_line_breaks(&text),
        text,
        is_streaming: assistant && row.is_some_and(|row| row.message.streaming),
        render_insights: assistant,
        heading_level_offset: 3,
        cwd: context.markdown_cwd.clone(),
        thread_ref: context.thread_ref.clone(),
        projected_item,
        agent_directories,
        message_id: row.map(|row| row.message.id.clone()),
        item_key: row.map_or_else(String::new, |row| row.id.clone()),
    }
}

fn assistant_markdown(markdown: Memo<AssistantMarkdown>) -> impl IntoElement {
    nsh_base::text_view()
        .markdown(move || Some(SharedString::from(markdown.get().text)))
        .options(move || nsh_base::text_view::MarkdownOptions {
            streaming: markdown.get().is_streaming,
            ..Default::default()
        })
        .selectable(true)
        .app_class(tw("w-full text-sm text-foreground/90"))
}

/// Port of packages/shared/src/markdownPipeline.ts shouldPreserveAssistantLineBreaks.
pub fn should_preserve_assistant_line_breaks(text: &str) -> bool {
    text.split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .any(|line| {
            line.strip_prefix("★ Insight")
                .and_then(|tail| tail.chars().next())
                .is_some_and(|next| next == '─' || js_whitespace(next))
        })
}

pub fn sent_message_timeline_row(
    content: Memo<String>,
    context: AssistantRowContext,
) -> impl IntoElement {
    let markdown_context = context.clone();
    let markdown = computed(move || markdown_input(content.get(), None, &markdown_context, false));
    view()
        .label(SharedString::from(t("T3 Code")))
        .app_class(tw("min-w-0 px-1 py-0.5"))
        .child((context.markdown)(markdown))
}

pub fn assistant_meta_timeline_row(
    row: Memo<Option<AssistantRow>>,
    context: AssistantRowContext,
) -> impl IntoElement {
    view()
        .app_class(tw("px-1"))
        .child(assistant_message_meta(row, context, "mt-0.5", true))
}

fn interrupted(row: &AssistantRow) -> bool {
    row.projected_item.as_ref().is_some_and(|item| {
        *items::status(&item.item) == OrchestrationV2TurnItemStatus::Interrupted
    })
}

pub fn assistant_message_meta(
    row: Memo<Option<AssistantRow>>,
    context: AssistantRowContext,
    class_name: &'static str,
    always_visible: bool,
) -> impl IntoElement {
    let copy_context = context.clone();
    let fork_context = context.clone();
    let reply_context = context.clone();
    let timestamp_format = context.timestamp_format.clone();
    // Group hover belongs to this cell's ancestor; the timeline supplies group/assistant.
    // NSH has no pointer-coarse variant, so resolve that class from the shared pointer signal.
    view()
        .app_class(move || {
            Some(tw(&cn([
                Some(
                    "flex items-center gap-2 text-xs tabular-nums transition-opacity duration-200",
                ),
                Some(if always_visible || pointer() == Pointer::Coarse {
                    "opacity-100"
                } else {
                    "opacity-0 focus-within:opacity-100 group-hover/assistant:opacity-100"
                }),
                Some(class_name),
                reduce_motion().then_some("duration-0"),
            ])))
        })
        .child(dynamic(move || {
            row.get()
                .and_then(|row| row.projected_item)
                .map(|item| items::status(&item.item).clone())
                .filter(|status| *status != OrchestrationV2TurnItemStatus::Completed)
                .map(assistant_status_badge)
        }))
        .child(assistant_copy_button(row, copy_context))
        .child(assistant_fork_button(row, fork_context))
        .child(assistant_reply_button(row, reply_context))
        .child(dynamic(move || {
            row.get().filter(|row| !row.message.streaming).map(|row| {
                let presenter = TimestampPresenter(UiTimestampHost);
                let label = presenter.day_aware(
                    &row.message.updated_at,
                    &timestamp_format,
                    nsh_core::time::unix_ms() as i64,
                    None,
                    false,
                );
                let hint = presenter.tooltip(&row.message.updated_at, &timestamp_format);
                text(SharedString::from(label))
                    .app_class(tw("text-muted-foreground text-xs tabular-nums"))
                    .tooltip(tooltip(
                        tooltip_create_handle(),
                        TooltipOptions::default(),
                        text(SharedString::from(hint)),
                    ))
            })
        }))
}

pub fn assistant_status_badge(status: OrchestrationV2TurnItemStatus) -> impl IntoElement {
    let status = match status {
        OrchestrationV2TurnItemStatus::Idle => "idle",
        OrchestrationV2TurnItemStatus::Pending => "pending",
        OrchestrationV2TurnItemStatus::Running => "running",
        OrchestrationV2TurnItemStatus::Waiting => "waiting",
        OrchestrationV2TurnItemStatus::Completed => "completed",
        OrchestrationV2TurnItemStatus::Failed => "failed",
        OrchestrationV2TurnItemStatus::Cancelled => "cancelled",
        OrchestrationV2TurnItemStatus::Interrupted => "interrupted",
    };
    text(t(status)).app_class(tw("rounded-full border border-border/70 px-1.5 py-0.5 font-mono text-3xs text-muted-foreground"))
}

pub fn assistant_copy_button(
    row: Memo<Option<AssistantRow>>,
    context: AssistantRowContext,
) -> impl IntoElement {
    let state = context.state;
    let visible = computed(move || {
        row.with(|row| {
            row.as_ref().is_some_and(|row| {
                row.show_assistant_copy_button
                    && !row.assistant_copy_streaming
                    && resolve_assistant_message_copy_state(Some(&row.message.text), true, false)
                        .visible
            })
        })
    });
    dynamic(move || {
        visible.get().then(|| {
            untracked(|| {
                let anchor = element_ref();
                let copied = computed(move || {
                    row.with(|row| row.as_ref().is_some_and(|row| state.action(&row.id).copied))
                });
                view()
                    .element_ref(&anchor)
                    .tooltip(assistant_tooltip("Copy message"))
                    .child(dynamic(move || {
                        let glyph = if copied.get() {
                            Icon::Check
                        } else {
                            Icon::Copy
                        };
                        button(
                            ButtonContent::Icon {
                                icon: glyph,
                                label: t("Copy message"),
                            },
                            ButtonVariant::GhostMuted,
                            ButtonSize::Xs,
                            move || copied.get(),
                            move || {
                                let Some(row) = row.get() else { return };
                                if state.action(&row.id).copied
                                    || !row.show_assistant_copy_button
                                    || row.assistant_copy_streaming
                                {
                                    return;
                                }
                                let copy = resolve_assistant_message_copy_state(
                                    Some(&row.message.text),
                                    true,
                                    false,
                                );
                                if copy.visible
                                    && let Some(text) = copy.text
                                {
                                    set_clipboard(text);
                                    state.copied(&row.id, anchor);
                                }
                            },
                        )
                    }))
            })
        })
    })
}

pub fn assistant_fork_button(
    row: Memo<Option<AssistantRow>>,
    context: AssistantRowContext,
) -> impl IntoElement {
    dynamic(move || {
        let current = row.get()?;
        let projected = current.projected_item.as_ref()?;
        let capabilities = context
            .provider_capabilities
            .as_ref()
            .and_then(|lookup| lookup(projected));
        if !can_fork_projected_assistant_item(projected, capabilities.as_ref()) {
            return None;
        }
        let on_fork = context.on_fork_from_run.clone()?;
        let state = context.state;
        let id = current.id.clone();
        let provider_capabilities = context.provider_capabilities.clone();
        let busy = computed(move || {
            row.with(|row| row.as_ref().is_some_and(|row| state.action(&row.id).busy))
        });
        Some(
            view()
                .tooltip(assistant_tooltip("Fork from this response"))
                // The primitive owns its glyph; pulsing the transparent wrapper preserves its look.
                .child(
                    view()
                        .app_class(move || {
                            Some(tw(if busy.get() && !reduce_motion() {
                                "animate-pulse"
                            } else {
                                "animate-none"
                            }))
                        })
                        .child(button(
                            ButtonContent::Icon {
                                icon: Icon::GitFork,
                                label: t("Fork from this response"),
                            },
                            ButtonVariant::Ghost,
                            ButtonSize::Xs,
                            move || busy.get(),
                            move || {
                                let Some(current) = row.get() else { return };
                                let Some(projected) = &current.projected_item else {
                                    return;
                                };
                                let capabilities = provider_capabilities
                                    .as_ref()
                                    .and_then(|lookup| lookup(projected));
                                if !can_fork_projected_assistant_item(
                                    projected,
                                    capabilities.as_ref(),
                                ) {
                                    return;
                                }
                                let Some(run_id) = items::run_id(&projected.item).clone() else {
                                    return;
                                };
                                if !state.begin(&id) {
                                    return;
                                }
                                let future = on_fork(ForkRequest {
                                    source_thread_id: projected.source_thread_id.clone(),
                                    run_id,
                                });
                                let id = id.clone();
                                state.owner.run(move || {
                                    spawn(async move {
                                        state.finish(&id, future.await);
                                    })
                                });
                            },
                        )),
                ),
        )
    })
}

pub fn assistant_reply_button(
    row: Memo<Option<AssistantRow>>,
    context: AssistantRowContext,
) -> impl IntoElement {
    dynamic(move || {
        let current = row.get()?;
        if current.message.streaming || !has_text(&current.message.text) {
            return None;
        }
        current.projected_item.as_ref()?;
        let thread_ref = context.thread_ref.clone()?;
        let on_reply = context.on_reply_to_response.clone()?;
        Some(
            view()
                .tooltip(assistant_tooltip("Reply to this response"))
                .child(button(
                    ButtonContent::Icon {
                        icon: Icon::Reply,
                        label: t("Reply to this response"),
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::Xs,
                    || false,
                    move || {
                        let Some(current) = row.get() else { return };
                        if current.message.streaming || !has_text(&current.message.text) {
                            return;
                        }
                        let Some(projected_item) = current.projected_item else {
                            return;
                        };
                        if let Err(error) = on_reply(ReplyRequest {
                            thread_ref: thread_ref.clone(),
                            message: current.message,
                            projected_item,
                        }) {
                            log::warn!("reply to response failed: {error}");
                        }
                    },
                )),
        )
    })
}

pub fn proposed_plan_timeline_row(card: impl IntoElement) -> impl IntoElement {
    view().app_class(tw("min-w-0 px-1 py-0.5")).child(card)
}

pub fn html_render_timeline_row(frame: impl IntoElement) -> impl IntoElement {
    view().app_class(tw("min-w-0 px-1")).child(frame)
}

pub fn mcp_app_timeline_row(frame: impl IntoElement) -> impl IntoElement {
    view().app_class(tw("min-w-0 px-1")).child(frame)
}

fn assistant_tooltip(label: &str) -> impl IntoElement + nsh_core::element::IntoTooltip {
    tooltip(
        tooltip_create_handle(),
        TooltipOptions::default(),
        text(t(label)),
    )
}

pub fn assistant_changed_files_section(
    row: Memo<Option<AssistantRow>>,
    render_card: RowSlot,
) -> impl IntoElement {
    let has_files = computed(move || {
        row.get().is_some_and(|row| {
            row.assistant_turn_diff_summary
                .as_ref()
                .is_some_and(|summary| {
                    summary["files"]
                        .as_array()
                        .is_some_and(|files| !files.is_empty())
                })
        })
    });
    dynamic(move || {
        has_files
            .get()
            .then(|| untracked(|| assistant_changed_files_section_inner(row, render_card.clone())))
    })
}

pub fn assistant_changed_files_section_inner(
    row: Memo<Option<AssistantRow>>,
    render_card: RowSlot,
) -> impl IntoElement {
    // The card owner supplies persisted expansion, file-menu context and turn-diff operations.
    render_card(row)
}

// JavaScript's trim/\s excludes U+0085 and includes BOM; Rust's str::trim differs on both.
fn js_whitespace(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn has_text(text: &str) -> bool {
    text.chars().any(|ch| !js_whitespace(ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insight_line_breaks_match_the_source_multiline_regex() {
        for (text, expected) in [
            ("★ Insight ─────\nBody", true),
            ("Paragraph\n★ Insight─", true),
            ("Paragraph\r★ Insight\t", true),
            ("★ Insight\u{feff}", true),
            ("Paragraph\u{2028}★ Insight ─", true),
            ("Paragraph\u{2029}★ Insight ─", true),
            ("★ Insight", false),
            (" ★ Insight ─", false),
            ("**★ Insight ─**", false),
            ("★ Insights ─", false),
            ("★ Insight\u{0085}", false),
            ("Normal\nMarkdown", false),
        ] {
            assert_eq!(
                should_preserve_assistant_line_breaks(text),
                expected,
                "{text:?}"
            );
        }
    }
}

struct UiTimestampHost;

impl UiTimestampHost {
    fn zone(zone: Option<&str>) -> TimeZone {
        let settings = locale_settings();
        let name = zone.unwrap_or_else(|| settings.time_zone.as_str());
        if matches!(name, "UTC" | "Etc/UTC" | "GMT") {
            TimeZone::Utc
        } else {
            TimeZone::Iana(name.into())
        }
    }
    fn formats() -> Formats {
        Formats::new(locale().as_str())
    }
    fn hour12(format: &TimestampFormat) -> bool {
        resolve_hour12(format, locale_settings().hour_cycle.twelve_hour())
    }
}

impl TimestampHost for UiTimestampHost {
    fn parse_epoch_ms(&self, iso: &str) -> Option<i64> {
        t3_client::time::epoch_ms(iso)
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
    fn numeric_date(&self, epoch_ms: i64, include_year: bool, zone: Option<&str>) -> String {
        Self::formats().date(
            if include_year {
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
