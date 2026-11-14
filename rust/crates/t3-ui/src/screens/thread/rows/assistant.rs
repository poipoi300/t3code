//! Port of apps/web/src/components/chat/TimelineAssistantRows.tsx AssistantTimelineRow and meta.

use std::collections::BTreeMap;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::state::items;
use t3_client::thread_views::threads::workflows::can_fork_projected_assistant_item;
use t3_client::types::ThreadKey;
use t3_client::view_logic::session::{ChatMessage, TimelineContent, TimelineEntry};
use t3_contracts::base_schemas::{RunId, ThreadId};
use t3_contracts::orchestration_v2::{
    OrchestrationV2Checkpoint, OrchestrationV2ProjectedTurnItem,
    OrchestrationV2ProviderCapabilities, OrchestrationV2TurnItemStatus,
};
use t3_contracts::orchestration_v2_id_table::IdOrIndex;

use crate::i18n::t;
use crate::runtime::Shared;
use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
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
    pub assistant_turn_diff_summary: Option<Rc<OrchestrationV2Checkpoint>>,
}

impl AssistantRow {
    pub fn from_entry(entry: &TimelineEntry) -> Option<Self> {
        let TimelineContent::Message {
            message,
            projected_item,
        } = &entry.content
        else {
            return None;
        };
        if message.role != "assistant" {
            return None;
        }
        Some(Self {
            id: entry.id.clone(),
            message: message.clone(),
            projected_item: projected_item.clone(),
            suppress_empty_response: false,
            show_assistant_meta: false,
            show_assistant_copy_button: false,
            assistant_copy_streaming: message.streaming,
            assistant_turn_diff_summary: None,
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
pub type CopyRenderer = Rc<dyn Fn(&str) -> String>;
pub type TimestampFormatter = Rc<dyn Fn(&str) -> (String, String)>;
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
pub type ActionControlRenderer = Rc<dyn Fn(AssistantActionControl) -> AnyElement>;

pub struct AssistantActionControl {
    /// English source key; the control renderer translates its accessibility label.
    pub label: &'static str,
    pub variant: ButtonVariant,
    pub size: ButtonSize,
    pub disabled: Rc<dyn Fn() -> bool>,
    pub on_press: Rc<dyn Fn()>,
}

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
    error: Option<String>,
}

/// Construct once above the virtual list; a recycled cell never owns a response's busy state.
#[derive(Clone, Copy)]
pub struct AssistantRowsState {
    actions: Signal<BTreeMap<String, ActionState>>,
    owner: Owner,
}

impl Default for AssistantRowsState {
    fn default() -> Self {
        Self {
            actions: signal(BTreeMap::new()),
            owner: Owner::current(),
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
                action.error = None;
                started = true;
            }
        });
        started
    }

    fn finish(self, id: &str, result: Result<(), String>) {
        self.actions.update(|state| {
            if let Err(error) = result {
                state.insert(
                    id.into(),
                    ActionState {
                        busy: false,
                        error: Some(error),
                    },
                );
            } else {
                state.remove(id);
            }
        });
    }

    fn reply_result(self, id: &str, result: Result<(), String>) {
        self.actions.update(|state| {
            state.entry(id.into()).or_default().error = result.err();
        });
    }
}

#[derive(Clone)]
pub struct AssistantRowContext {
    pub thread_ref: Option<ThreadKey>,
    pub markdown_cwd: Option<String>,
    pub markdown: MarkdownRenderer,
    pub agent_directories: Option<DirectoryLookup>,
    pub copy_text: Option<CopyRenderer>,
    pub timestamp: Option<TimestampFormatter>,
    pub provider_capabilities: Option<CapabilityLookup>,
    pub on_fork_from_run: Option<ForkHandler>,
    pub on_reply_to_response: Option<ReplyHandler>,
    /// UI-ICONS must supply GitFork/Reply through the button primitive before these mount.
    pub action_control: Option<ActionControlRenderer>,
    pub changed_files: Option<RowSlot>,
    pub state: AssistantRowsState,
}

impl AssistantRowContext {
    pub fn new(markdown: MarkdownRenderer, state: AssistantRowsState) -> Self {
        Self {
            thread_ref: None,
            markdown_cwd: None,
            markdown,
            agent_directories: None,
            copy_text: None,
            timestamp: None,
            provider_capabilities: None,
            on_fork_from_run: None,
            on_reply_to_response: None,
            action_control: None,
            changed_files: None,
            state,
        }
    }
}

/// Adapts the runtime's per-entry memo without cloning a message on every streamed token.
pub fn assistant_row(entry: Memo<Option<Shared<TimelineEntry>>>) -> Memo<Option<AssistantRow>> {
    computed(move || {
        entry.with(|entry| {
            entry
                .as_ref()
                .and_then(|entry| AssistantRow::from_entry(entry))
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
    let timestamp = context.timestamp.clone();
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
                let (label, tooltip) = timestamp.as_ref().map_or_else(
                    || {
                        (
                            t("Timestamp unavailable"),
                            t("Timestamp formatting is not available yet."),
                        )
                    },
                    |format| format(&row.message.updated_at),
                );
                text(t(&label))
                    .app_class(tw("text-muted-foreground text-xs tabular-nums"))
                    .tooltip(assistant_tooltip(&tooltip))
            })
        }))
        .child(dynamic(move || {
            row.get()
                .and_then(|row| context.state.action(&row.id).error)
                .map(|error| text(t(&error)).app_class(tw("text-xs text-destructive")))
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

#[derive(Debug, PartialEq)]
pub struct AssistantMessageCopyState {
    pub text: Option<String>,
    pub visible: bool,
}

/// Port of MessagesTimeline.logic.ts resolveAssistantMessageCopyState; the client supplies
/// renderCodexDirectivesForCopy rather than the UI implementing a second directive parser.
pub fn resolve_assistant_message_copy_state(
    text: Option<&str>,
    show_copy_button: bool,
    streaming: bool,
    render_codex_directives_for_copy: impl FnOnce(&str) -> String,
) -> AssistantMessageCopyState {
    let text = text.filter(|text| has_text(text));
    let visible = show_copy_button && text.is_some() && !streaming;
    AssistantMessageCopyState {
        text: text.map(|text| {
            if visible {
                render_codex_directives_for_copy(text)
            } else {
                text.into()
            }
        }),
        visible,
    }
}

pub fn assistant_copy_button(
    row: Memo<Option<AssistantRow>>,
    context: AssistantRowContext,
) -> impl IntoElement {
    dynamic(move || {
        row.get()
            .filter(|row| {
                resolve_assistant_message_copy_state(
                    Some(&row.message.text),
                    row.show_assistant_copy_button,
                    row.assistant_copy_streaming,
                    |text| text.into(),
                )
                .visible
            })
            .map(|_| {
                let renderer = context.copy_text.clone();
                let can_copy = renderer.is_some();
                let hint = if can_copy {
                    "Copy message"
                } else {
                    "Copy formatting is not available yet."
                };
                view().tooltip(assistant_tooltip(hint)).child(button(
                    ButtonContent::Icon {
                        icon: crate::ui::icon::Icon::Copy,
                        label: t("Copy message"),
                    },
                    ButtonVariant::GhostMuted,
                    ButtonSize::Xs,
                    move || !can_copy,
                    move || {
                        let Some(row) = row.get() else { return };
                        let Some(renderer) = &renderer else { return };
                        let copy = resolve_assistant_message_copy_state(
                            Some(&row.message.text),
                            row.show_assistant_copy_button,
                            row.assistant_copy_streaming,
                            |text| renderer(text),
                        );
                        if copy.visible
                            && let Some(text) = copy.text
                        {
                            // NSH clipboard writes have no acknowledgement: never claim Copied.
                            set_clipboard(text);
                        }
                    },
                ))
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
        let render_control = context.action_control.clone()?;
        let state = context.state;
        let id = current.id.clone();
        let provider_capabilities = context.provider_capabilities.clone();
        // The frozen button primitive lacks GitFork/Reply; its owner supplies these controls.
        Some(
            view()
                .tooltip(assistant_tooltip("Fork from this response"))
                .child(render_control(AssistantActionControl {
                    label: "Fork from this response",
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Xs,
                    disabled: Rc::new(move || {
                        row.get().is_none_or(|row| state.action(&row.id).busy)
                    }),
                    on_press: Rc::new(move || {
                        let Some(current) = row.get() else { return };
                        let Some(projected) = &current.projected_item else {
                            return;
                        };
                        let capabilities = provider_capabilities
                            .as_ref()
                            .and_then(|lookup| lookup(projected));
                        if !can_fork_projected_assistant_item(projected, capabilities.as_ref()) {
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
                        // The list owner retains the operation across cell recycling.
                        state.owner.run(move || {
                            spawn(async move {
                                state.finish(&id, future.await);
                            })
                        });
                    }),
                })),
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
        let render_control = context.action_control.clone()?;
        let state = context.state;
        Some(
            view()
                .tooltip(assistant_tooltip("Reply to this response"))
                .child(render_control(AssistantActionControl {
                    label: "Reply to this response",
                    variant: ButtonVariant::GhostMuted,
                    size: ButtonSize::Xs,
                    disabled: Rc::new(|| false),
                    on_press: Rc::new(move || {
                        let Some(current) = row.get() else { return };
                        if current.message.streaming || !has_text(&current.message.text) {
                            return;
                        }
                        let Some(projected_item) = current.projected_item else {
                            return;
                        };
                        let result = on_reply(ReplyRequest {
                            thread_ref: thread_ref.clone(),
                            message: current.message,
                            projected_item,
                        });
                        state.reply_result(&current.id, result);
                    }),
                })),
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
                .is_some_and(|summary| !summary.files.is_empty())
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
    fn copy_visibility_matches_the_typescript_conditions_and_keeps_the_source_when_hidden() {
        for text in [None, Some(""), Some(" \t\n\u{feff}")] {
            for show in [false, true] {
                for streaming in [false, true] {
                    assert_eq!(
                        resolve_assistant_message_copy_state(text, show, streaming, |_| panic!(
                            "empty text must never reach the directive renderer"
                        )),
                        AssistantMessageCopyState {
                            text: None,
                            visible: false
                        }
                    );
                }
            }
        }
        for show in [false, true] {
            for streaming in [false, true] {
                let visible = show && !streaming;
                let copy = resolve_assistant_message_copy_state(
                    Some(" **Reply** \n"),
                    show,
                    streaming,
                    |source| {
                        assert!(visible);
                        format!("rendered:{source}")
                    },
                );
                assert_eq!(copy.visible, visible);
                assert_eq!(
                    copy.text.as_deref(),
                    Some(if visible {
                        "rendered: **Reply** \n"
                    } else {
                        " **Reply** \n"
                    })
                );
            }
        }
    }

    #[test]
    fn copy_matches_javascript_whitespace_rather_than_rust_trim() {
        for (text, visible) in [
            ("\u{feff}", false),
            ("\u{0085}", true),
            ("\u{200b}", true),
            ("😀", true),
        ] {
            let state =
                resolve_assistant_message_copy_state(Some(text), true, false, |text| text.into());
            assert_eq!(state.visible, visible, "{text:?}");
            assert_eq!(state.text.as_deref(), visible.then_some(text));
        }
    }

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
