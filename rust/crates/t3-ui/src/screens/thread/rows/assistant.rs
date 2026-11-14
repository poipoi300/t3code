//! Port of apps/web/src/components/chat/TimelineAssistantRows.tsx AssistantTimelineRow and meta.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use futures::FutureExt;
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

use crate::i18n::{Text as I18nText, t};
use crate::runtime::Shared;
use crate::screens::thread::blocks::code::{
    CodeHighlightState, HighlightStatus, HighlightedCode, code_chunk_row, code_syntax_theme,
};
use crate::screens::thread::blocks::flatten::{
    Block, BlockId, BlockKind, MessageFlattener, StreamBlocks,
};
use crate::screens::thread::markdown::{
    base::{chat_markdown, chat_markdown_blocks, chat_markdown_leaf, starts_with_heading},
    code_block::{
        CodeBlockOptions, CodeBlockProps, CodeBlockState, extract_fence_title, markdown_code_block,
    },
};
use crate::screens::thread::timeline::TimelineServices;

use crate::state::layout::{Pointer, pointer};
use crate::theme::{cn, tw};
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};
use crate::ui::toast::{ANCHORED_COPY_TOAST_TIMEOUT_MS, show_anchored_copy_success_toast};
use crate::ui::tooltip::{TooltipOptions, tooltip, tooltip_create_handle};

/// Display decisions belong to the client's MessagesTimeline projection, not individual cells.
#[derive(Clone, Debug)]
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

impl PartialEq for AssistantRow {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && Rc::ptr_eq(&self.message, &other.message)
            && self.projected_item.as_ref().map(Rc::as_ptr)
                == other.projected_item.as_ref().map(Rc::as_ptr)
            && self.suppress_empty_response == other.suppress_empty_response
            && self.show_assistant_meta == other.show_assistant_meta
            && self.show_assistant_copy_button == other.show_assistant_copy_button
            && self.assistant_copy_streaming == other.assistant_copy_streaming
            && self.assistant_turn_diff_summary == other.assistant_turn_diff_summary
    }
}

fn message_key(row: &AssistantRow) -> String {
    match &row.message.id {
        IdOrIndex::Id(id) => format!("id:{}", id.0),
        IdOrIndex::Index(index) => format!("index:{index}"),
    }
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

pub type RowSlot = Rc<dyn Fn(Memo<Option<AssistantRow>>) -> AnyElement>;
pub type DirectoryLookup = Rc<dyn Fn(&OrchestrationV2ProjectedTurnItem) -> Vec<String>>;

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

#[derive(Clone)]
struct MarkdownLeaf {
    block: Rc<Block>,
    text: Rc<str>,
    streaming: bool,
    line_breaks: bool,
    starts_with_heading: bool,
    fence: Option<BlockId>,
}

#[derive(Default)]
struct CompletedCode {
    value: Option<(Rc<Block>, String, Shared<HighlightedCode>)>,
}

impl CompletedCode {
    fn resolve(
        &mut self,
        leaf: &MarkdownLeaf,
        theme: &str,
        read: impl FnOnce() -> Shared<HighlightedCode>,
    ) -> Shared<HighlightedCode> {
        if !leaf.streaming
            && let Some((block, previous_theme, value)) = &self.value
            && Rc::ptr_eq(block, &leaf.block)
            && previous_theme == theme
        {
            return value.clone();
        }
        let value = read();
        self.value = (!leaf.streaming
            && matches!(
                value.status,
                HighlightStatus::Highlighted | HighlightStatus::Plain
            ))
        .then(|| (leaf.block.clone(), theme.to_owned(), value.clone()));
        value
    }
}

struct MessageRenderer {
    id: String,
    flattener: MessageFlattener,
    document: StreamBlocks,
    leaves: Vec<Rc<MarkdownLeaf>>,
    inspected_bytes: usize,
    line_break_blocks: usize,
}

impl MessageRenderer {
    fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            flattener: MessageFlattener::new(id),
            document: StreamBlocks::default(),
            leaves: Vec::new(),
            inspected_bytes: 0,
            line_break_blocks: 0,
        }
    }

    fn update(
        &mut self,
        source: &str,
        streaming: bool,
        replace: bool,
    ) -> Vec<(usize, Rc<MarkdownLeaf>)> {
        let mut delta = if replace {
            self.flattener.replace(&self.id, source)
        } else {
            self.flattener.ingest(&self.id, source)
        };
        if !streaming && let Some(finished) = self.flattener.finish() {
            if finished.rows.is_empty() {
                delta.document = finished.document;
                delta.context_version = finished.context_version;
            } else {
                delta = finished;
            }
        }
        self.document = delta.document;
        let count = self.document.block_count();
        let old_last = self.leaves.len().checked_sub(1);
        let mut updates = Vec::new();
        for block in delta.rows {
            let index = block.id.index;
            let leaf = self.leaf(block, streaming && index + 1 == count);
            self.line_break_blocks += usize::from(leaf.line_breaks);
            if index == self.leaves.len() {
                self.leaves.push(leaf.clone());
            } else {
                self.line_break_blocks -= usize::from(self.leaves[index].line_breaks);
                self.leaves[index] = leaf.clone();
            }
            updates.push((index, leaf));
        }
        for leaf in self.leaves.drain(count..) {
            self.line_break_blocks -= usize::from(leaf.line_breaks);
        }
        if let Some(index) =
            old_last.filter(|index| *index < count && !updates.iter().any(|(at, _)| at == index))
        {
            let live = streaming && index + 1 == count;
            if self.leaves[index].streaming != live {
                let leaf = self.leaf(self.leaves[index].block.clone(), live);
                self.leaves[index] = leaf.clone();
                updates.push((index, leaf));
            }
        }
        updates
    }

    fn leaf(&mut self, block: Rc<Block>, streaming: bool) -> Rc<MarkdownLeaf> {
        if let Some(old) = self
            .leaves
            .get(block.id.index)
            .filter(|old| Rc::ptr_eq(&old.block, &block))
        {
            if old.streaming == streaming {
                return old.clone();
            }
            if old.block.text.is_some() && block.text.is_some() {
                return Rc::new(MarkdownLeaf {
                    streaming,
                    ..old.as_ref().clone()
                });
            }
        }
        let text = block.text.clone().unwrap_or_else(|| Rc::from(""));
        self.inspected_bytes += text.len();
        let line_breaks = should_preserve_assistant_line_breaks(&text);
        let starts_with_heading = starts_with_heading(&text);
        let fence = match &block.kind {
            BlockKind::Code(code) if code.first => Some(block.id.clone()),
            BlockKind::Code(_) => block
                .id
                .index
                .checked_sub(1)
                .and_then(|index| self.leaves.get(index))
                .and_then(|leaf| leaf.fence.clone()),
            _ => None,
        };
        Rc::new(MarkdownLeaf {
            block,
            text,
            streaming,
            line_breaks,
            starts_with_heading,
            fence,
        })
    }

    fn code_text(&self, first: usize) -> String {
        let mut code = String::new();
        for index in first..self.document.block_count() {
            let Some(block) = self.document.block(index) else {
                break;
            };
            let BlockKind::Code(chunk) = &block.kind else {
                break;
            };
            if index > first && chunk.first {
                break;
            }
            code.push_str(&chunk.text);
            if chunk.last {
                break;
            }
        }
        // mdast-to-hast adds one LF to a nonempty code value before extractCodeBlock copies it.
        if !code.is_empty() {
            code.push('\n');
        }
        code
    }
}

#[derive(Clone, Copy)]
struct MessageHandle {
    keys: Signal<Vec<BlockId>>,
    slots: Stored<Vec<Signal<Shared<MarkdownLeaf>>>>,
    line_breaks: Signal<bool>,
}

impl PartialEq for MessageHandle {
    fn eq(&self, other: &Self) -> bool {
        self.keys == other.keys
    }
}

struct MessageRenderState {
    renderer: MessageRenderer,
    handle: MessageHandle,
    last_message: Option<Rc<ChatMessage>>,
    last_fallback: Option<String>,
}

impl MessageHandle {
    fn view(self, state: AssistantRowsState) -> impl IntoElement {
        chat_markdown_blocks().child(each(
            move || {
                self.keys
                    .get()
                    .into_iter()
                    .filter(|id| id.index != 0)
                    .collect::<Vec<_>>()
            },
            |id| id.clone(),
            move |id, _| {
                let slot = self.slots.with(|slots| slots[id.get_untracked().index]);
                render_markdown_leaf(slot, self.line_breaks, state)
            },
        ))
    }
}

fn render_markdown_leaf(
    leaf: Signal<Shared<MarkdownLeaf>>,
    line_breaks: Signal<bool>,
    state: AssistantRowsState,
) -> impl IntoElement {
    let kind = computed(move || {
        leaf.with(|leaf| match &leaf.block.kind {
            BlockKind::Header | BlockKind::Definition { .. } => 0,
            BlockKind::Code(code) if code.first => 1,
            BlockKind::Code(_) => 2,
            BlockKind::Output { .. } => 3,
            BlockKind::Table => 4,
            _ => 5,
        })
    });
    dynamic(move || {
        let kind = kind.get();
        untracked(|| {
            (kind != 0).then(|| match kind {
                1 | 2 => render_code_leaf(leaf, state),
                3 => text(move || SharedString::from(leaf.with(|leaf| leaf.text.to_string())))
                    .select_text()
                    .app_class(tw("font-mono"))
                    .into_element(),
                4 => {
                    let id = leaf.with_untracked(|leaf| leaf.block.id.clone());
                    let source = computed(move || leaf.with(|leaf| leaf.text.clone()));
                    crate::screens::thread::markdown::links::markdown_table_leaf(
                        source,
                        id,
                        state.tables,
                    )
                    .into_element()
                }
                _ => render_prose_leaf(leaf, line_breaks).into_element(),
            })
        })
    })
}

struct PreserveAssistantBreaks(bool);

impl nsh_base::text_view::MarkdownPlugin for PreserveAssistantBreaks {
    fn name(&self) -> &str {
        "assistant-line-breaks"
    }

    fn inline(
        &self,
        node: &nsh_base::text_view::mdast::Node,
        _context: &mut nsh_base::text_view::ParseContext<'_>,
    ) -> Option<nsh_base::text_view::Inline> {
        if self.0
            && let nsh_base::text_view::mdast::Node::Text(text) = node
        {
            return Some(nsh_base::text_view::Inline::text(text.value.clone()));
        }
        None
    }
}

fn render_prose_leaf(
    leaf: Signal<Shared<MarkdownLeaf>>,
    line_breaks: Signal<bool>,
) -> impl IntoElement {
    let presentation = computed(move || {
        (
            line_breaks.get(),
            leaf.with(|leaf| leaf.starts_with_heading),
        )
    });
    let plugins = use_context::<nsh_base::text_view::MarkdownPlugins>().unwrap_or_default();
    dynamic(move || {
        let (preserve, heading) = presentation.get();
        untracked(|| {
            provide_context(plugins.clone().with(PreserveAssistantBreaks(preserve)));
            chat_markdown_leaf(
                heading,
                chat_markdown(
                    move || leaf.with(|leaf| leaf.text.to_string()),
                    move || leaf.with(|leaf| leaf.streaming),
                ),
            )
        })
    })
}

fn render_code_leaf(leaf: Signal<Shared<MarkdownLeaf>>, state: AssistantRowsState) -> AnyElement {
    let initial = leaf.get_untracked();
    let BlockKind::Code(code) = &initial.block.kind else {
        return view().into_element();
    };
    let Some(fence) = initial.fence.clone() else {
        return view().into_element();
    };
    let copy_id = fence.clone();
    let code_state = state.owner.run(|| {
        state
            .code_states
            .with(|states| *states.entry(fence.clone()).or_default())
    });
    let wrapped = computed(move || code_state.wrap_override.get().unwrap_or(true));
    let chunk = computed(move || {
        leaf.with(|leaf| match &leaf.block.kind {
            BlockKind::Code(code) => Some(code.clone()),
            _ => None,
        })
    });
    let highlight_state = state.owner.run(|| {
        state
            .highlights
            .with(|documents| documents.entry(fence).or_default().clone())
    });
    let completed = stored(CompletedCode::default());
    let highlighted = computed(move || {
        leaf.with(|leaf| {
            let BlockKind::Code(chunk) = &leaf.block.kind else {
                return None;
            };
            // Completed chunks stop observing the fence's revision once their colors settle.
            Some(completed.with(|cache| {
                cache.resolve(leaf, &code_syntax_theme().name, || {
                    highlight_state.highlighted(chunk)
                })
            }))
        })
    });
    if !code.first {
        return view().direction(LayoutDirection::Ltr).writing_direction(WritingDirection::Ltr)
            // A fence's continuation has no inter-block margin in the web's single pre.
            .app_class(move || Some(tw(&cn([
                Some("-mt-[0.65rem] min-w-0 text-start overflow-hidden border-x border-border/70 bg-secondary leading-snug dark:border-transparent dark:bg-input/32"),
                crate::theme::has_theme_id().then_some("bg-(--code-background) border-contrast-border text-code-foreground dark:border-contrast-border dark:bg-(--code-background)"),
            ]))))
            .child(code_chunk_row(chunk, highlighted, wrapped)).into_element();
    }
    let props = computed(move || {
        leaf.with(|leaf| {
            let BlockKind::Code(code) = &leaf.block.kind else {
                return CodeBlockProps::default();
            };
            CodeBlockProps {
                code: code.text.clone(),
                language: code.language.clone().unwrap_or_else(|| "text".into()),
                fence_title: extract_fence_title(code.meta.as_deref()),
                is_streaming: leaf.streaming,
                themed: crate::theme::has_theme_id(),
                ..Default::default()
            }
        })
    });
    let options = CodeBlockOptions {
        copy: Some(Rc::new(move |_| {
            let text = state
                .messages
                .with(|messages| {
                    messages
                        .get(&copy_id.message_id)
                        .map(|message| message.renderer.code_text(copy_id.index))
                })
                .unwrap_or_default();
            async move {
                set_clipboard(text);
                Ok(())
            }
            .boxed_local()
        })),
        ..Default::default()
    };
    markdown_code_block(props, code_state, options, move |wrapped| {
        code_chunk_row(chunk, highlighted, wrapped).into_element()
    })
    .into_element()
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
    messages: Stored<BTreeMap<String, MessageRenderState>>,
    code_states: Stored<BTreeMap<BlockId, CodeBlockState>>,
    tables: crate::screens::thread::markdown::links::MarkdownTablesState,
    highlights: Stored<BTreeMap<BlockId, CodeHighlightState>>,
}

impl Default for AssistantRowsState {
    fn default() -> Self {
        Self {
            actions: signal(BTreeMap::new()),
            owner: Owner::current(),
            copy_resets: stored(BTreeMap::new()),
            messages: stored(BTreeMap::new()),
            code_states: stored(BTreeMap::new()),
            tables: Default::default(),
            highlights: stored(BTreeMap::new()),
        }
    }
}

impl AssistantRowsState {
    fn render_message(self, row: &AssistantRow, fallback: Option<&str>) -> MessageHandle {
        self.owner.run(|| {
            batch(|| {
                self.messages.with(|messages| {
                    let message =
                        messages
                            .entry(message_key(row))
                            .or_insert_with(|| MessageRenderState {
                                renderer: MessageRenderer::new(&message_key(row)),
                                handle: MessageHandle {
                                    keys: signal(Vec::new()),
                                    slots: stored(Vec::new()),
                                    line_breaks: signal(false),
                                },
                                last_message: None,
                                last_fallback: None,
                            });
                    if message
                        .last_message
                        .as_ref()
                        .is_some_and(|old| Rc::ptr_eq(old, &row.message))
                        && message.last_fallback.as_deref() == fallback
                    {
                        return message.handle;
                    }
                    let replace = message
                        .last_message
                        .as_ref()
                        .is_some_and(|old| !old.streaming && !row.message.streaming);
                    let updates = message.renderer.update(
                        fallback.unwrap_or(row.message.text.as_str()),
                        row.message.streaming,
                        replace,
                    );
                    let line_breaks = message.renderer.line_break_blocks != 0;
                    if message.handle.line_breaks.get_untracked() != line_breaks {
                        message.handle.line_breaks.set(line_breaks);
                    }
                    self.highlights.with(|documents| {
                        if updates.first().is_some_and(|(index, _)| *index == 0) {
                            for (id, document) in documents.iter() {
                                if id.message_id == message.renderer.id {
                                    document.clear();
                                }
                            }
                        }
                        for (_, leaf) in &updates {
                            if let BlockKind::Code(code) = &leaf.block.kind
                                && let Some(fence) = &leaf.fence
                            {
                                documents
                                    .entry(fence.clone())
                                    .or_default()
                                    .upsert(code, leaf.streaming);
                            }
                        }
                    });
                    let count = message.renderer.leaves.len();
                    message.handle.slots.with(|slots| {
                        while slots.len() < count {
                            slots
                                .push(signal(Shared(message.renderer.leaves[slots.len()].clone())));
                        }
                        for (index, leaf) in updates {
                            slots[index].set(Shared(leaf));
                        }
                        slots.truncate(count);
                    });
                    if message.handle.keys.with_untracked(Vec::len) != count {
                        message.handle.keys.set(
                            message
                                .renderer
                                .leaves
                                .iter()
                                .map(|leaf| leaf.block.id.clone())
                                .collect(),
                        );
                    }
                    message.last_message = Some(row.message.clone());
                    message.last_fallback = fallback.map(str::to_owned);
                    message.handle
                })
            })
        })
    }

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

/// ChatView's `onReplyToResponse` (the composer's `replyToResponse`); overrides the host client's
/// in tests. Production reads `screens::thread::composer::reply_to_response_handler`.
#[derive(Clone)]
pub struct ReplyService(pub ReplyHandler);

/// MessagesTimeline's row context for assistant rows, from the thread's [`TimelineServices`].
/// The changed-files card (`AssistantChangedFilesSection`) is not ported by the diff lane yet.
#[derive(Clone)]
pub struct AssistantRowContext {
    pub thread_ref: Option<ThreadKey>,
    pub markdown_cwd: Option<String>,
    pub agent_directories: Option<DirectoryLookup>,
    pub timestamp_format: Memo<TimestampFormat>,
    pub provider_capabilities: Option<CapabilityLookup>,
    on_fork_from_run: Option<ForkHandler>,
    on_reply_to_response: Option<ReplyHandler>,
    pub state: AssistantRowsState,
}

impl AssistantRowContext {
    pub fn new(state: AssistantRowsState, services: &TimelineServices) -> Self {
        Self {
            thread_ref: Some(services.key.clone()),
            markdown_cwd: None,
            agent_directories: None,
            timestamp_format: services.timestamp_format,
            provider_capabilities: None,
            on_fork_from_run: services.rewind.fork_action(),
            on_reply_to_response: services.reply.clone(),
            state,
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
    let state = context.state;
    let rendered = computed(move || {
        row.with(|row| {
            row.as_ref().map(|row| {
                let fallback = (row.message.text.is_empty()
                    && !row.message.streaming
                    && !row.suppress_empty_response)
                    .then(|| t("(empty response)"));
                untracked(|| state.render_message(row, fallback.as_deref()))
            })
        })
    });
    let meta_context = context.clone();
    let show_meta =
        computed(move || row.with(|row| row.as_ref().is_some_and(|row| row.show_assistant_meta)));
    let show_interrupted = computed(move || {
        row.with(|row| {
            row.as_ref()
                .is_some_and(|row| !row.show_assistant_meta && interrupted(row))
        })
    });
    // MessageAuthorHeading is sr-only; its source key belongs on the NSH accessibility label.
    view()
        .label(SharedString::from(t("T3 Code")))
        .app_class(tw("relative min-w-0 px-1 py-0.5"))
        .child(view().child(dynamic(move || {
            rendered
                .get()
                .map(|rendered| untracked(|| rendered.view(state)))
        })))
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
    _context: AssistantRowContext,
) -> impl IntoElement {
    let breaks = computed(move || content.with(|text| should_preserve_assistant_line_breaks(text)));
    let plugins = use_context::<nsh_base::text_view::MarkdownPlugins>().unwrap_or_default();
    view()
        .label(SharedString::from(t("T3 Code")))
        .app_class(tw("min-w-0 px-1 py-0.5"))
        .child(dynamic(move || {
            let preserve = breaks.get();
            untracked(|| {
                provide_context(plugins.clone().with(PreserveAssistantBreaks(preserve)));
                chat_markdown(move || content.get(), || false)
            })
        }))
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
    let timestamp_format = context.timestamp_format;
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
                let format = timestamp_format.get();
                let label = presenter.day_aware(
                    &row.message.updated_at,
                    &format,
                    nsh_core::time::unix_ms() as i64,
                    None,
                    false,
                );
                let hint = presenter.tooltip(&row.message.updated_at, &format);
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
    text(I18nText::client_key(status).resolve()).app_class(tw("rounded-full border border-border/70 px-1.5 py-0.5 font-mono text-3xs text-muted-foreground"))
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
                    row.with(|row| {
                        row.as_ref()
                            .is_some_and(|row| state.action(&message_key(row)).copied)
                    })
                });
                view()
                    .element_ref(&anchor)
                    .app_class(tw("group/assistant-copy"))
                    .tooltip(assistant_tooltip("Copy message"))
                    .child(button(
                        ButtonContent::Custom {
                            child: dynamic(move || {
                                let copied = copied.get();
                                icon_with_class(
                                    if copied { Icon::Check } else { Icon::Copy },
                                    if copied { "size-3 text-primary" } else { "size-3 text-muted-foreground group-hover/assistant-copy:text-foreground" },
                                )
                            }).into_element(),
                            label: t("Copy message"),
                        },
                            ButtonVariant::GhostMuted,
                            ButtonSize::Xs,
                            move || copied.get(),
                            move || {
                                let Some(row) = row.get() else { return };
                                if state.action(&message_key(&row)).copied
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
                                    state.copied(&message_key(&row), anchor);
                                }
                            },
                    ))
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
        let id = message_key(&current);
        let provider_capabilities = context.provider_capabilities.clone();
        let busy = computed(move || {
            row.with(|row| {
                row.as_ref()
                    .is_some_and(|row| state.action(&message_key(row)).busy)
            })
        });
        Some(
            view()
                .tooltip(assistant_tooltip("Fork from this response"))
                .child(button(
                    ButtonContent::Custom {
                        child: dynamic(move || {
                            icon_with_class(
                                Icon::GitFork,
                                if busy.get() && !reduce_motion() {
                                    "size-3 text-contrast-muted-foreground animate-pulse"
                                } else {
                                    "size-3 text-contrast-muted-foreground"
                                },
                            )
                        })
                        .into_element(),
                        label: t("Fork from this response"),
                    },
                    ButtonVariant::Ghost,
                    ButtonSize::Xs,
                    move || busy.get(),
                    move || {
                        let Some(current) = row.get() else { return };
                        if message_key(&current) != id {
                            return;
                        }
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
                        state.owner.run(move || {
                            spawn(async move {
                                state.finish(&id, future.await);
                            })
                        });
                    },
                )),
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
                .app_class(tw("group/assistant-reply"))
                .tooltip(assistant_tooltip("Reply to this response"))
                .child(button(
                    ButtonContent::Custom {
                        child: icon_with_class(Icon::Reply, "size-3 text-muted-foreground group-hover/assistant-reply:text-foreground")
                            .into_element(),
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

fn assistant_tooltip(label: &'static str) -> impl IntoElement + nsh_core::element::IntoTooltip {
    tooltip(
        tooltip_create_handle(),
        TooltipOptions::default(),
        text(I18nText::from(label).resolve()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[component]
    fn HostTimestampFixture() -> impl IntoElement {
        crate::theme::install(&Default::default());
        assert_eq!(
            UiTimestampHost::zone(None),
            TimeZone::Iana("America/New_York".into())
        );
        let entry = Rc::new(serde_json::from_value(serde_json::json!({
            "id":"zone-answer", "kind":"message", "createdAt":"2026-11-13T12:34:00Z",
            "message":{"id":"zone-answer","role":"assistant","text":"Host timezone answer","streaming":false,"runId":"run","createdAt":"2026-11-13T12:34:00Z","updatedAt":"2026-11-13T12:34:00Z"}
        })).expect("canonical host-zone entry"));
        let rows = t3_client::thread_views::messages_timeline::derive_messages_timeline_rows(
            &t3_client::thread_views::messages_timeline::MessagesTimelineRowsInput {
                timeline_entries: vec![entry],
                latest_run: Some(
                    t3_client::thread_views::messages_timeline::TimelineLatestRun {
                        run_id: "run".into(),
                        status: "completed".into(),
                        started_at: Some("2026-11-13T12:34:00Z".into()),
                        completed_at: Some("2026-11-13T12:34:00Z".into()),
                        turn_ended: None,
                    },
                ),
                ..Default::default()
            },
        );
        let source = rows
            .into_iter()
            .find(|row| AssistantRow::from_row(row).is_some())
            .expect("client assistant row");
        let row = assistant_row(computed(move || Some(Shared(source.clone()))));
        provide_context(crate::runtime::Rt::new(
            t3_client::ClientStore::default(),
            Rc::new(crate::runtime::backend::Unavailable("fixture".into())),
            crate::runtime::host::Choice::Demo,
        ));
        provide_context(crate::state::nav::Navigator::new(
            crate::state::nav::Route::Home,
        ));
        let services = TimelineServices::new(ThreadKey {
            environment: t3_client::types::EnvironmentId("environment".into()),
            thread: ThreadId("thread".into()),
        });
        let mut context = AssistantRowContext::new(AssistantRowsState::default(), &services);
        context.timestamp_format = computed(|| TimestampFormat::V24Hour);
        view()
            .app_class(tw("group/assistant"))
            .child(assistant_timeline_row(row, context))
    }

    #[test]
    fn host_timezone_is_forwarded_and_renders_its_non_utc_offset() {
        use nsh_engine::{EngineConfig, Root};
        use nsh_testing::Headless;
        use std::time::SystemTime;
        let mut config = EngineConfig {
            ui_sets: crate::registries(),
            viewport: size(px(390.0), px(844.0)),
            clock: nsh_core::time::Clock::manual(
                SystemTime::UNIX_EPOCH
                    + Duration::from_millis(
                        t3_client::time::epoch_ms("2026-11-13T12:34:00Z").expect("fixture time")
                            as u64,
                    ),
            ),
            ..EngineConfig::new(Root::of::<HostTimestampFixture>())
        };
        config.environment.locale_settings.time_zone =
            nsh_core::platform::TimeZone::parse("America/New_York").expect("host IANA name");
        assert!(
            TimeZone::Iana("America/New_York".into()).is_known(),
            "the UI enables platform timezone data"
        );
        let expected = "07:34";
        eprintln!("host America/New_York: expected rendered time {expected}");
        let mut app = Headless::start(config).expect("host timezone fixture");
        app.wait_for("Host timezone answer")
            .expect("actual assistant row");
        app.wait_for(expected)
            .expect("the actual row converts UTC to the host's New York zone");
    }

    #[test]
    fn completed_colors_skip_fence_reads_but_refresh_edits_themes_and_pending_results() {
        use std::cell::Cell;
        fn value(
            leaf: &MarkdownLeaf,
            status: HighlightStatus,
            reads: &Cell<usize>,
        ) -> Shared<HighlightedCode> {
            reads.set(reads.get() + 1);
            Shared(Rc::new(HighlightedCode {
                text: leaf.text.clone(),
                lines: Vec::new(),
                status,
                foreground: None,
            }))
        }
        let mut renderer = MessageRenderer::new("colors");
        renderer.update("```rust\nlet value = 1;\n```", false, false);
        let leaf = renderer.leaves[1].clone();
        for status in [HighlightStatus::Highlighted, HighlightStatus::Plain] {
            let reads = Cell::new(0);
            let mut cache = CompletedCode::default();
            for _ in 0..128 {
                let colored =
                    cache.resolve(&leaf, "light", || value(&leaf, status.clone(), &reads));
                assert_eq!(colored.text.as_ref(), "let value = 1;");
            }
            assert_eq!(
                reads.get(),
                1,
                "a tail's fence revisions do not revisit the completed prefix"
            );
            cache.resolve(&leaf, "dark", || value(&leaf, status.clone(), &reads));
            assert_eq!(reads.get(), 2, "a theme change refreshes colors");
            renderer.update("```rust\nlet value = 2;\n```", false, true);
            let edited = renderer.leaves[1].clone();
            let colored = cache.resolve(&edited, "dark", || value(&edited, status.clone(), &reads));
            assert_eq!(colored.text.as_ref(), "let value = 2;");
            assert_eq!(
                reads.get(),
                3,
                "the same BlockId with a new source is not stale"
            );
            let live = MarkdownLeaf {
                streaming: true,
                ..edited.as_ref().clone()
            };
            for _ in 0..2 {
                cache.resolve(&live, "dark", || value(&live, status.clone(), &reads));
            }
            assert_eq!(
                reads.get(),
                5,
                "the streaming tail keeps observing its tokenizer"
            );
            let pending = cache.resolve(&edited, "dark", || {
                value(&edited, HighlightStatus::Unavailable, &reads)
            });
            assert_eq!(pending.status, HighlightStatus::Unavailable);
            let ready = cache.resolve(&edited, "dark", || value(&edited, status.clone(), &reads));
            assert_eq!(ready.status, status);
            assert_eq!(
                reads.get(),
                7,
                "an unavailable result cannot freeze out arriving tokens"
            );
        }
    }

    #[test]
    fn appends_retain_completed_leaf_identity_and_inspect_only_the_open_tail() {
        let mut source = "A completed paragraph.\n\n".repeat(1000);
        source.push_str("live");
        let mut renderer = MessageRenderer::new("reply");
        renderer.update(&source, true, false);
        let first = renderer.leaves[1].clone();
        let before = renderer.inspected_bytes;
        source.push_str(" 😀");
        let changed = renderer.update(&source, true, false);
        assert!(Rc::ptr_eq(&first, &renderer.leaves[1]));
        assert_eq!(changed.len(), 1);
        assert!(renderer.inspected_bytes - before <= "live 😀".len());
        assert_eq!(
            renderer.leaves.last().expect("tail").text.as_ref(),
            "live 😀"
        );
    }

    #[test]
    fn definitions_keep_their_shared_identity_while_only_the_live_tail_changes() {
        let mut source = "[ref]: https://example.test\n\n[Result][ref]\n\nlive".to_owned();
        let mut renderer = MessageRenderer::new("reply");
        renderer.update(&source, true, false);
        let first = renderer.leaves[1].clone();
        let definition = renderer
            .document
            .definition("ref", false)
            .expect("definition fact");
        for _ in 0..100 {
            let before = renderer.inspected_bytes;
            source.push_str("more ");
            renderer.update(&source, true, false);
            assert!(Rc::ptr_eq(&first, &renderer.leaves[1]));
            assert!(Rc::ptr_eq(
                &definition,
                &renderer
                    .document
                    .definition("ref", false)
                    .expect("retained definition")
            ));
            assert!(
                renderer.inspected_bytes - before
                    <= renderer.leaves.last().expect("tail").text.len()
            );
        }
        let before = renderer.inspected_bytes;
        renderer.update(&source, false, false);
        assert_eq!(renderer.inspected_bytes, before);
        let mut snapshot = MessageRenderer::new("settled");
        snapshot.update("Settled reply", false, false);
        assert_eq!(
            snapshot.leaves.last().expect("settled leaf").text.as_ref(),
            "Settled reply"
        );
    }

    #[test]
    fn code_chunks_keep_their_completed_prefix_and_copy_the_entire_code_on_demand() {
        let mut code: String = (0..85)
            .map(|line| format!("let value_{line} = {line};\n"))
            .collect();
        let mut source = format!("```rs\n{code}");
        let mut renderer = MessageRenderer::new("reply");
        renderer.update(&source, true, false);
        let first = renderer
            .leaves
            .iter()
            .find(|leaf| matches!(&leaf.block.kind, BlockKind::Code(code) if code.first))
            .expect("first code chunk")
            .clone();
        assert!(
            renderer
                .leaves
                .iter()
                .filter(|leaf| matches!(leaf.block.kind, BlockKind::Code(_)))
                .count()
                >= 3
        );
        for leaf in &renderer.leaves {
            if let BlockKind::Code(code) = &leaf.block.kind {
                assert!(code.text.lines().count() <= 40);
            }
        }
        code.push_str("last_line\n");
        source.push_str("last_line\n");
        renderer.update(&source, true, false);
        assert!(Rc::ptr_eq(&first, &renderer.leaves[first.block.id.index]));
        assert_eq!(renderer.code_text(first.block.id.index), code);
        for (source, expected) in [
            ("```rust\nlast_line", "last_line\n"),
            ("```rust\nlast_line\n```", "last_line\n"),
            ("```rust\nlast_line\n\n```", "last_line\n\n"),
            ("```rust\n```", ""),
        ] {
            let mut renderer = MessageRenderer::new("clipboard");
            renderer.update(source, false, false);
            let first = renderer
                .leaves
                .iter()
                .find(|leaf| matches!(&leaf.block.kind, BlockKind::Code(code) if code.first))
                .expect("code leaf");
            assert_eq!(
                renderer.code_text(first.block.id.index),
                expected,
                "{source:?}"
            );
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
        let mut renderer = MessageRenderer::new("insight-facts");
        let mut source = "Before\nwith breaks\n\n★ Insight ─\nBody\n\nAfter".to_owned();
        renderer.update(&source, true, false);
        assert_eq!(renderer.line_break_blocks, 1);
        let before = renderer.leaves[1].clone();
        assert!(
            !before.line_breaks,
            "the reply-wide flag also applies to other blocks"
        );
        for _ in 0..100 {
            let prior_work = renderer.inspected_bytes;
            source.push('x');
            renderer.update(&source, true, false);
            assert_eq!(renderer.line_break_blocks, 1);
            assert!(Rc::ptr_eq(&before, &renderer.leaves[1]));
            assert!(renderer.inspected_bytes - prior_work < 110);
        }
        renderer.update("Before\nwith breaks", false, true);
        assert_eq!(
            renderer.line_break_blocks, 0,
            "an edit removes the document fact"
        );
    }
}
