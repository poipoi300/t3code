//! Port of apps/web/src/components/chat/MessagesTimeline.tsx MessagesTimeline and TimelineRowContent.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use super::code::{CodeHighlightState, HighlightQueueWork, code_chunk_row, highlighted_chunk};
use super::flatten::{Block, BlockId, BlockKind, FlattenWork, MessageFlattener, StreamBlocks};
use crate::i18n::t;
use crate::runtime::Shared;
use crate::screens::thread::markdown::base::{
    chat_markdown, chat_markdown_blocks, chat_markdown_leaf, starts_with_heading,
};
use crate::screens::thread::markdown::code_block::{
    CodeBlockOptions, CodeBlockProps, CodeBlockState, extract_fence_title,
    markdown_code_block_header, markdown_code_chunk_frame,
};
use crate::screens::thread::rows::assistant::{
    AssistantRowContext, AssistantRowsState, assistant_changed_files_section,
    assistant_message_meta, assistant_meta_timeline_row, assistant_row, assistant_status_badge,
    should_preserve_assistant_line_breaks,
};
use crate::screens::thread::rows::groups::{
    WorkGroupsContext, WorkGroupsState, work_group_timeline_row,
};
use crate::screens::thread::rows::user::{UserRowContext, UserRowState, user_message_row};
use crate::screens::thread::rows::work::working_row;
use crate::theme::{cn, tw};
use futures::FutureExt;
use nsh_core::prelude::*;
use t3_client::thread_views::messages_timeline::{MessagesTimelineRow, MessagesTimelineRowContent};

pub type TimelineRows = Memo<Shared<Vec<Rc<MessagesTimelineRow>>>>;

/// Work counters, not wall-clock timing; TL-BLOCKS-4 measures actual frame intervals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListWork {
    pub projection_rows: usize,
    pub changed_blocks: usize,
    pub inspected_bytes: usize,
    pub flatten: FlattenWork,
    pub highlight_fences: usize,
    pub highlight_queue: Option<HighlightQueueWork>,
}

#[derive(Clone)]
pub struct BlockLeaf {
    pub block: Rc<Block>,
    pub streaming: bool,
    pub preserve_line_breaks: bool,
    pub block_position: usize,
    pub starts_with_heading: bool,
    pub fence: Option<usize>,
    pub lexer_context: u64,
    lexer_exit: u64,
}

struct MessageDocument {
    flattener: MessageFlattener,
    document: StreamBlocks,
    leaves: BTreeMap<usize, Rc<BlockLeaf>>,
    fences: BTreeMap<usize, usize>,
    streaming: bool,
    work: ListWork,
    highlight_updates: Vec<Rc<BlockLeaf>>,
    highlight_reset: bool,
}

impl MessageDocument {
    fn new(id: &str) -> Self {
        Self {
            flattener: MessageFlattener::new(id),
            document: StreamBlocks::default(),
            leaves: BTreeMap::new(),
            fences: BTreeMap::new(),
            streaming: false,
            work: ListWork::default(),
            highlight_updates: Vec::new(),
            highlight_reset: false,
        }
    }

    fn update(&mut self, id: &str, source: &str, streaming: bool, replace: bool) -> Vec<BlockId> {
        self.highlight_updates.clear();
        self.highlight_reset = false;
        let mut delta = match source.get(self.document.source_len()..) {
            Some(tail) if self.streaming && !replace => self.flattener.append(tail),
            _ => {
                self.highlight_reset = true;
                self.flattener.replace(id, source)
            }
        };
        if !streaming && let Some(finished) = self.flattener.finish() {
            // finish may publish context without rows; it must not discard the append splice.
            delta.document = finished.document;
            delta.context_version = finished.context_version;
        }
        self.document = delta.document;
        let count = self.document.block_count();
        let old_last = self.leaves.last_key_value().map(|(&index, _)| index);
        let mut retired: Vec<_> = self
            .leaves
            .split_off(&count)
            .into_values()
            .map(|leaf| leaf.block.id.clone())
            .collect();
        self.fences.split_off(&delta.changed_from);
        for block in delta.rows {
            let index = block.id.index;
            if let Some(old) = self.leaves.get(&index)
                && std::mem::discriminant(&old.block.kind) != std::mem::discriminant(&block.kind)
            {
                retired.push(old.block.id.clone());
            }
            let fence = match &block.kind {
                BlockKind::Code(code) if code.first => {
                    self.fences.insert(index, index);
                    Some(index)
                }
                BlockKind::Code(_) => {
                    self.fences
                        .range_mut(..=index)
                        .next_back()
                        .map(|(&first, end)| {
                            *end = index;
                            first
                        })
                }
                _ => None,
            };
            if let Some(old) = self.leaves.get(&index)
                && old.fence == Some(index)
                && fence.is_some()
                && old.fence != fence
            {
                retired.push(old.block.id.clone());
            }
            if let Some(old) = self.leaves.get_mut(&index)
                && Rc::ptr_eq(&old.block, &block)
            {
                let live = streaming && index + 1 == count;
                if old.streaming != live || old.fence != fence {
                    *old = Rc::new(BlockLeaf {
                        streaming: live,
                        fence,
                        ..old.as_ref().clone()
                    });
                    if fence.is_some() {
                        self.highlight_updates.push(old.clone());
                    }
                }
                continue;
            }
            let text = block.text.as_deref().unwrap_or_default();
            let lexer_context = fence
                .and_then(|fence| {
                    index
                        .checked_sub(1)
                        .and_then(|previous| self.leaves.get(&previous))
                        .filter(|previous| previous.fence == Some(fence))
                        .map(|previous| previous.lexer_exit)
                })
                .unwrap_or_default();
            let mut lexer = DefaultHasher::new();
            lexer_context.hash(&mut lexer);
            if let BlockKind::Code(code) = &block.kind {
                code.text.hash(&mut lexer);
            }
            self.work.inspected_bytes += text.len();
            self.work.changed_blocks += 1;
            let preserve_line_breaks = matches!(block.kind, BlockKind::Prose | BlockKind::Document)
                && should_preserve_assistant_line_breaks(text);
            let block_position =
                self.leaves
                    .range(..index)
                    .next_back()
                    .map_or(0, |(_, previous)| {
                        previous.block_position + usize::from(semantic_block(&previous.block.kind))
                    });
            let starts_with_heading = matches!(block.kind, BlockKind::Prose | BlockKind::Document)
                && starts_with_heading(text);
            self.leaves.insert(
                index,
                Rc::new(BlockLeaf {
                    block,
                    streaming: streaming && index + 1 == count,
                    preserve_line_breaks,
                    block_position,
                    starts_with_heading,
                    fence,
                    lexer_context,
                    lexer_exit: lexer.finish(),
                }),
            );
            if fence.is_some() {
                self.highlight_updates.push(self.leaves[&index].clone());
            }
        }
        if let Some(index) = old_last
            && let Some(leaf) = self.leaves.get_mut(&index)
        {
            let live = streaming && index + 1 == count;
            if leaf.streaming != live {
                *leaf = Rc::new(BlockLeaf {
                    streaming: live,
                    ..leaf.as_ref().clone()
                });
                if leaf.fence.is_some() {
                    self.highlight_updates.push(leaf.clone());
                }
            }
        }
        self.streaming = streaming;
        retired
    }

    fn code_text(&self, first: usize) -> Option<String> {
        let last = *self.fences.get(&first)?;
        let mut text = String::new();
        for (_, leaf) in self.leaves.range(first..=last) {
            let BlockKind::Code(code) = &leaf.block.kind else {
                return None;
            };
            text.push_str(&code.text);
        }
        Some(text)
    }
}

fn semantic_block(kind: &BlockKind) -> bool {
    match kind {
        BlockKind::Header | BlockKind::Definition { .. } => false,
        BlockKind::Code(chunk) => chunk.first,
        _ => true,
    }
}

struct Entry {
    row: Rc<MessagesTimelineRow>,
    message: Option<MessageDocument>,
}

fn source(row: &MessagesTimelineRow) -> Option<(&str, bool)> {
    match &row.content {
        MessagesTimelineRowContent::Message { message, .. } if message.role == "assistant" => {
            Some((&message.text, message.streaming))
        }
        MessagesTimelineRowContent::SentMessage { text } => Some((text, false)),
        _ => None,
    }
}

/// Offsets contain one item per client row, never one item per block in a long reply.
#[derive(Default)]
pub struct BlockListModel {
    entries: BTreeMap<String, Entry>,
    order: Vec<(usize, String)>,
    count: usize,
    work: ListWork,
    highlight_updates: Vec<Rc<BlockLeaf>>,
    highlight_resets: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CellId {
    Row(String),
    Block(BlockId),
    Footer(String),
}

#[derive(Clone)]
pub struct BlockCell {
    pub id: CellId,
    pub row: Shared<MessagesTimelineRow>,
    pub leaf: Option<Shared<BlockLeaf>>,
    pub document: Option<StreamBlocks>,
    pub context_version: usize,
    pub last: bool,
}

impl PartialEq for BlockCell {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.row == other.row
            && self.leaf == other.leaf
            && self.context_version == other.context_version
            && self.last == other.last
    }
}

impl BlockListModel {
    pub fn update(&mut self, rows: &[Rc<MessagesTimelineRow>]) -> Vec<BlockId> {
        self.highlight_updates.clear();
        self.highlight_resets.clear();
        let mut removed = std::mem::take(&mut self.entries);
        let mut retired = Vec::new();
        self.order.clear();
        self.count = 0;
        for row in rows {
            self.work.projection_rows += 1;
            let mut entry = removed.remove(&row.id).unwrap_or_else(|| Entry {
                row: row.clone(),
                message: None,
            });
            if entry.message.is_none() || !Rc::ptr_eq(&entry.row, row) {
                if let Some((text, streaming)) = source(row) {
                    let same_message = match (&entry.row.content, &row.content) {
                        (
                            MessagesTimelineRowContent::Message { message: old, .. },
                            MessagesTimelineRowContent::Message { message, .. },
                        ) => Rc::ptr_eq(old, message),
                        _ => Rc::ptr_eq(&entry.row, row),
                    };
                    if entry.message.is_none() || !same_message {
                        let replace = entry.message.as_mut().is_some_and(|document| {
                            if !document.streaming {
                                return true;
                            }
                            let end = document.document.source_len();
                            let start = document
                                .leaves
                                .last_key_value()
                                .map_or(end, |(_, leaf)| leaf.block.source.bytes.start);
                            document.work.inspected_bytes += end - start;
                            let previous =
                                source(&entry.row).and_then(|(text, _)| text.get(start..end));
                            previous.is_none() || text.get(start..end) != previous
                        });
                        retired.extend(
                            entry
                                .message
                                .get_or_insert_with(|| MessageDocument::new(&row.id))
                                .update(&row.id, text, streaming, replace),
                        );
                        let document = entry.message.as_mut().expect("updated message");
                        self.highlight_updates
                            .append(&mut document.highlight_updates);
                        if document.highlight_reset {
                            self.highlight_resets.push(row.id.clone());
                        }
                    }
                } else {
                    if let Some(message) = entry.message.take() {
                        retired.extend(
                            message
                                .leaves
                                .into_values()
                                .map(|leaf| leaf.block.id.clone()),
                        );
                    }
                }
                entry.row = row.clone();
            }
            self.order.push((self.count, row.id.clone()));
            self.count += entry
                .message
                .as_ref()
                .map_or(1, |m| m.document.block_count() + 1);
            self.entries.insert(row.id.clone(), entry);
        }
        for entry in removed.into_values() {
            if let Some(message) = entry.message {
                retired.extend(
                    message
                        .leaves
                        .into_values()
                        .map(|leaf| leaf.block.id.clone()),
                );
            }
        }
        retired
    }

    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn cell(&self, index: usize) -> Option<BlockCell> {
        if index >= self.count {
            return None;
        }
        let at = self
            .order
            .partition_point(|(start, _)| *start <= index)
            .checked_sub(1)?;
        let (start, id) = &self.order[at];
        let entry = self.entries.get(id)?;
        let offset = index - start;
        let Some(message) = &entry.message else {
            return Some(BlockCell {
                id: CellId::Row(id.clone()),
                row: Shared(entry.row.clone()),
                leaf: None,
                document: None,
                context_version: 0,
                last: true,
            });
        };
        let leaf = message.leaves.get(&offset).cloned().map(Shared);
        Some(BlockCell {
            id: leaf.as_ref().map_or_else(
                || CellId::Footer(id.clone()),
                |leaf| CellId::Block(leaf.block.id.clone()),
            ),
            row: Shared(entry.row.clone()),
            leaf,
            document: Some(message.document.clone()),
            context_version: message.document.context_version(),
            last: offset == message.document.block_count(),
        })
    }

    /// Selection/find/copy can read unmounted source; rendering never calls this for a document.
    pub fn document(&self, row_id: &str) -> Option<StreamBlocks> {
        Some(self.entries.get(row_id)?.message.as_ref()?.document.clone())
    }

    /// Explicit user action only: materializes a complete fence including unmounted chunks.
    pub fn code_text(&self, fence: &BlockId) -> Option<String> {
        self.entries
            .get(&fence.message_id)?
            .message
            .as_ref()?
            .code_text(fence.index)
    }

    pub fn work(&self) -> ListWork {
        let mut work = self.work;
        for entry in self.entries.values() {
            if let Some(message) = &entry.message {
                work.changed_blocks += message.work.changed_blocks;
                work.inspected_bytes += message.work.inspected_bytes;
                let f = message.flattener.work();
                work.flatten.indexed_bytes += f.indexed_bytes;
                work.flatten.split_bytes += f.split_bytes;
                work.flatten.parsed_bytes += f.parsed_bytes;
                work.flatten.copied_bytes += f.copied_bytes;
                work.flatten.visited_rows += f.visited_rows;
            }
        }
        work
    }
}

/// Created above the list; recycling never owns parse, wrap, copy or highlight state.
#[derive(Clone, Copy)]
pub struct BlockListState {
    model: Stored<BlockListModel>,
    highlights: Stored<BTreeMap<BlockId, CodeHighlightState>>,
    fences: Stored<BTreeMap<BlockId, CodeBlockState>>,
    tables: crate::screens::thread::markdown::links::MarkdownTablesState,
    owner: Owner,
}

impl Default for BlockListState {
    fn default() -> Self {
        Self {
            model: stored(BlockListModel::default()),
            highlights: stored(BTreeMap::new()),
            fences: stored(BTreeMap::new()),
            tables: Default::default(),
            owner: Owner::current(),
        }
    }
}

impl BlockListState {
    fn highlighter(self, id: BlockId) -> CodeHighlightState {
        self.owner.run(|| {
            self.highlights
                .with(|states| states.entry(id).or_default().clone())
        })
    }

    fn fence(self, id: BlockId) -> CodeBlockState {
        self.owner
            .run(|| self.fences.with(|states| *states.entry(id).or_default()))
    }

    pub fn work(self) -> ListWork {
        let mut work = self.model.with(|model| model.work());
        self.highlights.with(|states| {
            work.highlight_fences = states.len();
            work.highlight_queue = states.values().next().map(CodeHighlightState::queue_work);
        });
        work
    }
    pub fn len(self) -> usize {
        self.model.with(|model| model.len())
    }
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
    pub fn document(self, id: &str) -> Option<StreamBlocks> {
        self.model.with(|model| model.document(id))
    }
}

/// Existing row contexts carry actions, never caller-supplied row renderers or presentation logic.
#[derive(Clone)]
pub struct BlockListOptions {
    pub state: BlockListState,
    pub assistant: AssistantRowContext,
    pub user: UserRowContext,
    pub work: WorkGroupsContext,
    pub wrapped: Memo<bool>,
    pub themed: Memo<bool>,
}

impl Default for BlockListOptions {
    fn default() -> Self {
        Self {
            state: BlockListState::default(),
            assistant: AssistantRowContext::new(AssistantRowsState::default()),
            user: UserRowContext::new(UserRowState::new()),
            work: WorkGroupsContext::new(WorkGroupsState::default()),
            wrapped: computed(|| true),
            themed: computed(crate::theme::has_theme_id),
        }
    }
}

pub fn messages_timeline(rows: TimelineRows, options: BlockListOptions) -> impl IntoElement {
    let state = options.state;
    let revision = computed(move || {
        let retired = rows.with(|rows| untracked(|| state.model.with(|model| model.update(rows))));
        state.highlights.with(|documents| {
            for id in &retired {
                documents.remove(id);
            }
        });
        state.fences.with(|fences| {
            for id in &retired {
                fences.remove(id);
            }
        });
        state.model.with(|model| {
            for message in model.highlight_resets.drain(..) {
                state.highlights.with(|documents| {
                    for (id, document) in documents.iter() {
                        if id.message_id == message {
                            document.clear();
                        }
                    }
                });
            }
            for leaf in model.highlight_updates.drain(..) {
                if let (Some(first), BlockKind::Code(chunk)) = (leaf.fence, &leaf.block.kind) {
                    state
                        .highlighter(BlockId {
                            message_id: leaf.block.id.message_id.clone(),
                            index: first,
                        })
                        .upsert(chunk, leaf.streaming);
                }
            }
        });
        // Only the projection handle is copied; blocks stay in the persistent documents.
        rows.get()
    });
    let count = computed(move || {
        revision.with(|_| ());
        state.len()
    });
    let list = virtual_list(count, move |at| {
        let cell = computed(move || {
            revision.with(|_| ());
            state.model.with(|model| model.cell(at.index()))
        });
        let options = options.clone();
        view().child(each(
            move || cell.get().into_iter().collect::<Vec<_>>(),
            |cell| cell.id.clone(),
            move |cell, _| timeline_row_content(cell, options.clone()),
        ))
    })
    // NSH keys() re-hashes the entire reply on a new tail block; key only mounted cells above.
    .initial_scroll_at_end()
    .follow_end(px(1.0))
    .estimated_item_size(px(90.0))
    .header(view().app_class(tw("h-3 sm:h-4")))
    .footer(view().app_class(tw("h-3 sm:h-4")))
    .empty(
        view()
            .app_class(tw("flex h-full items-center justify-center"))
            .child(
                text(move || SharedString::from(t("Send a message to start the conversation.")))
                    .app_class(tw("text-sm text-muted-foreground")),
            ),
    )
    .flex_1()
    .test_id("timeline");
    view().flex_1().min_h(px(0.0)).selectable(true).child(list)
}

fn timeline_row_content(
    cell: ReadSignal<BlockCell>,
    options: BlockListOptions,
) -> impl IntoElement {
    if matches!(
        cell.get_untracked().row.content,
        MessagesTimelineRowContent::Working { .. }
    ) {
        return working_row().into_element();
    }
    let row = computed(move || Some(cell.with(|cell| cell.row.clone())));
    let leaf = computed(move || cell.with(|cell| cell.leaf.clone()));
    let kind = computed(move || {
        leaf.with(|leaf| {
            leaf.as_ref().map(|leaf| match leaf.block.kind {
                BlockKind::Header => 0,
                BlockKind::Definition { .. } => 5,
                BlockKind::Code(_) => 1,
                BlockKind::Output { .. } => 2,
                BlockKind::Table => 4,
                _ => 3,
            })
        })
    });
    let footer = computed(move || cell.with(|cell| matches!(cell.id, CellId::Footer(_))));
    let fence = computed(move || leaf.with(|leaf| leaf.as_ref().and_then(|leaf| leaf.fence)));
    let spacing = computed(move || {
        leaf.with(|leaf| {
            leaf.as_ref().map(|leaf| {
                (
                    leaf.block_position > 0 && semantic_block(&leaf.block.kind),
                    leaf.starts_with_heading,
                )
            })
        })
    });
    view()
        // index.css .messages-timeline-scroll and .chat-content-lane; no nested scroller.
        .app_class(tw("w-full px-3 sm:px-12"))
        .child(view().app_class(tw("mx-auto w-full min-w-0 max-w-(--chat-content-max-width) overflow-x-clip"))
            .child(view().app_class(move || Some(tw(&cn([
                cell.with(|cell| cell.last).then_some(padding_of(&cell.with(|cell| cell.row.clone()))),
                row.with(|row| row.as_ref().is_some_and(|row| matches!(&row.content,
                    MessagesTimelineRowContent::Message { message, .. } if message.role == "assistant")
                    || matches!(row.content, MessagesTimelineRowContent::AssistantMeta { .. })))
                    .then_some("group/assistant"),
            ]))))
                .child(dynamic(move || {
                    let kind = kind.get();
                    let footer = footer.get();
                    fence.with(|_| ());
                    let spacing = spacing.get();
                    untracked(|| {
                        let body = match kind {
                        Some(0) => empty_response(row).into_element(),
                        Some(1) => code_cell(cell, leaf, options.clone()),
                        Some(2) => text(move || leaf.with(|leaf| leaf.as_ref().and_then(|leaf| leaf.block.text.as_ref()).map_or_else(String::new, |text| text.to_string())))
                            .select_text().app_class(tw("font-mono")).into_element(),
                        Some(4) => {
                            let Some(table_leaf) = leaf.get_untracked() else { return view().into_element(); };
                            let id = table_leaf.block.id.clone();
                            let source = computed(move || leaf.with(|leaf| leaf.as_ref()
                                .and_then(|leaf| leaf.block.text.clone()).unwrap_or_else(|| Rc::from(""))));
                            crate::screens::thread::markdown::links::markdown_table_leaf(source, id, options.state.tables).into_element()
                        }
                        Some(5) => view().into_element(),
                        Some(_) => prose_cell(leaf).into_element(),
                        None if footer => assistant_footer(row, options.assistant.clone()).into_element(),
                        None => standalone_row(row, options.clone()),
                        };
                        if !matches!(kind, Some(1..=4)) { return body; }
                        let (gap, heading) = spacing.unwrap_or_default();
                        // Virtual cells have separate parents; a zero-height sibling preserves the shared gap and first: rules.
                        let blocks = chat_markdown_blocks();
                        let blocks = if gap { blocks.child(view().h(px(0.0))) } else { blocks };
                        view().app_class(tw("min-w-0 px-1"))
                            .child(blocks
                                .child(chat_markdown_leaf(heading, body)))
                            .into_element()
                    })
                })))).into_element()
}

fn padding_of(row: &MessagesTimelineRow) -> &'static str {
    use MessagesTimelineRowContent as C;
    match &row.content {
        C::Work { .. } | C::WorkLive { .. } | C::WorkToggle { .. } | C::Thinking { .. } => "",
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
        C::WorktreeSetup { .. }
        | C::Event { .. }
        | C::AttemptFold { .. }
        | C::HtmlRender { .. }
        | C::SentMessage { .. }
        | C::McpApp { .. }
        | C::ViewedImages { .. } => "pb-2",
        _ => "pb-4",
    }
}

fn assistant_footer(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    context: AssistantRowContext,
) -> impl IntoElement {
    let assistant = assistant_row(row);
    let meta_context = context.clone();
    let files = context.changed_files;
    view()
        // The message's py-0.5 is split between its header and footer, never repeated per block.
        .app_class(tw("relative min-w-0 px-1 pb-0.5"))
        .child(files.map(|render| assistant_changed_files_section(assistant, render)))
        .child(dynamic(move || {
            assistant.with(|row| row.as_ref().filter(|row| !row.show_assistant_meta)
                .and_then(|row| row.projected_item.as_ref())
                .filter(|item| *t3_client::state::items::status(&item.item) == t3_contracts::orchestration_v2::OrchestrationV2TurnItemStatus::Interrupted)
                .map(|_| view().app_class(tw("mt-1.5 flex"))
                    .child(assistant_status_badge(t3_contracts::orchestration_v2::OrchestrationV2TurnItemStatus::Interrupted))))
        }))
        .child(dynamic(move || {
            assistant
                .with(|row| row.as_ref().is_some_and(|row| row.show_assistant_meta))
                .then(|| {
                    untracked(|| {
                        assistant_message_meta(assistant, meta_context.clone(), "mt-1.5", false)
                    })
                })
        }))
}

fn empty_response(row: Memo<Option<Shared<MessagesTimelineRow>>>) -> impl IntoElement {
    view().app_class(tw("min-w-0 px-1 pt-0.5")).child(when(
        move || {
            row.with(|row| {
                row.as_ref().is_some_and(|row| {
                    matches!(&row.content, MessagesTimelineRowContent::Message { message,
            suppress_empty_response, .. } if message.text.is_empty() && !message.streaming
                && !suppress_empty_response.unwrap_or(false))
                })
            })
        },
        || text(move || SharedString::from(t("(empty response)"))),
    ))
}

fn standalone_row(
    row: Memo<Option<Shared<MessagesTimelineRow>>>,
    options: BlockListOptions,
) -> AnyElement {
    row.with(|value| match value.as_ref().map(|row| &row.content) {
        Some(MessagesTimelineRowContent::Message { message, .. }) if message.role == "user" => {
            let projection =
                computed(move || row.with(|row| row.as_ref().map(|row| row.0.clone())));
            user_message_row(projection, options.user).into_element()
        }
        Some(MessagesTimelineRowContent::AssistantMeta { .. }) => {
            assistant_meta_timeline_row(assistant_row(row), options.assistant).into_element()
        }
        Some(
            MessagesTimelineRowContent::Work { .. }
            | MessagesTimelineRowContent::WorkLive { .. }
            | MessagesTimelineRowContent::Thinking { .. }
            | MessagesTimelineRowContent::WorkToggle { .. }
            | MessagesTimelineRowContent::TurnFold { .. }
            | MessagesTimelineRowContent::AttemptFold { .. },
        ) => work_group_timeline_row(row, options.work).into_element(),
        Some(MessagesTimelineRowContent::Working { .. }) => working_row().into_element(),
        // TL-EVENTS, TL-EMBED, TL-PLAN and TL-MARKDOWN-3 own the remaining standalone bodies.
        _ => view().into_element(),
    })
}

struct PreserveLineBreaks(bool);

impl nsh_base::text_view::MarkdownPlugin for PreserveLineBreaks {
    fn name(&self) -> &str {
        "assistant-line-breaks"
    }
    fn inline(
        &self,
        node: &nsh_base::text_view::mdast::Node,
        _: &mut nsh_base::text_view::ParseContext<'_>,
    ) -> Option<nsh_base::text_view::Inline> {
        if self.0
            && let nsh_base::text_view::mdast::Node::Text(text) = node
        {
            Some(nsh_base::text_view::Inline::text(text.value.clone()))
        } else {
            None
        }
    }
}

fn prose_cell(leaf: Memo<Option<Shared<BlockLeaf>>>) -> impl IntoElement {
    let preserve = computed(move || {
        leaf.with(|leaf| leaf.as_ref().is_some_and(|leaf| leaf.preserve_line_breaks))
    });
    let plugins = use_context::<nsh_base::text_view::MarkdownPlugins>().unwrap_or_default();
    view()
        .app_class(tw("min-w-0"))
        .label(move || SharedString::from(t("T3 Code")))
        .child(dynamic(move || {
            let preserve = preserve.get();
            untracked(|| {
                provide_context(plugins.clone().with(PreserveLineBreaks(preserve)));
                chat_markdown(
                    move || {
                        leaf.with(|leaf| {
                            leaf.as_ref()
                                .and_then(|leaf| leaf.block.text.as_ref())
                                .map_or_else(String::new, |text| text.to_string())
                        })
                    },
                    move || leaf.with(|leaf| leaf.as_ref().is_some_and(|leaf| leaf.streaming)),
                )
            })
        }))
}

fn code_cell(
    cell: ReadSignal<BlockCell>,
    leaf: Memo<Option<Shared<BlockLeaf>>>,
    options: BlockListOptions,
) -> AnyElement {
    let Some(initial) = leaf.get_untracked() else {
        return view().into_element();
    };
    let id = initial.block.id.clone();
    let Some(first) = initial.fence else {
        return view().into_element();
    };
    let fence_id = BlockId {
        message_id: id.message_id.clone(),
        index: first,
    };
    let state = options.state;
    let fence_state = state.fence(fence_id.clone());
    let marker = format!("timeline-code-chunk-{}", id.index);
    let wrapped = computed(move || {
        fence_state
            .wrap_override
            .get()
            .unwrap_or_else(|| options.wrapped.get())
    });
    let chunk = computed(move || {
        leaf.with(|leaf| {
            leaf.as_ref().and_then(|leaf| match &leaf.block.kind {
                BlockKind::Code(code) => Some(code.clone()),
                _ => None,
            })
        })
    });
    let streaming =
        computed(move || leaf.with(|leaf| leaf.as_ref().is_some_and(|leaf| leaf.streaming)));
    let highlighted = highlighted_chunk(chunk, streaming, state.highlighter(fence_id.clone()));
    let first_chunk =
        computed(move || chunk.with(|chunk| chunk.as_ref().is_some_and(|chunk| chunk.first)));
    let last_chunk =
        computed(move || chunk.with(|chunk| chunk.as_ref().is_some_and(|chunk| chunk.last)));
    if first != id.index {
        return view()
            .test_id(&marker)
            .child(markdown_code_chunk_frame(
                options.themed,
                first_chunk,
                last_chunk,
                code_chunk_row(chunk, highlighted, wrapped),
            ))
            .into_element();
    }
    let message_streaming = computed(move || {
        cell.with(|cell| source(&cell.row).is_some_and(|(_, streaming)| streaming))
    });
    let props = computed(move || {
        chunk.with(|chunk| {
            let Some(chunk) = chunk else {
                return CodeBlockProps::default();
            };
            CodeBlockProps {
                code: chunk.text.clone(),
                language: chunk.language.clone().unwrap_or_else(|| "text".into()),
                fence_title: extract_fence_title(chunk.meta.as_deref()),
                is_streaming: message_streaming.get(),
                default_wrapped: options.wrapped.get(),
                themed: options.themed.get(),
                ..Default::default()
            }
        })
    });
    let copy = CodeBlockOptions {
        copy: Some(Rc::new(move |_| {
            let code = state.model.with(|model| model.code_text(&fence_id));
            async move {
                let mut code = code.ok_or_else(|| String::from("Code block no longer exists."))?;
                // ReactMarkdown's pre/code child includes one final LF beyond mdast Code.value.
                code.push('\n');
                set_clipboard(code);
                Ok(())
            }
            .boxed_local()
        })),
        ..Default::default()
    };
    view()
        .test_id(&marker)
        .child(markdown_code_chunk_frame(
            options.themed,
            first_chunk,
            last_chunk,
            view()
                .child(markdown_code_block_header(props, fence_state, copy))
                .child(code_chunk_row(chunk, highlighted, wrapped)),
        ))
        .into_element()
}
