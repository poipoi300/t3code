//! Port of apps/web/src/markdown-incremental.ts and apps/web/src/components/chat/MessagesTimeline.tsx block identity.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use nsh_base::text_view::{
    Block as NshBlock, MarkdownOptions, MarkdownPlugin, ParseContext, Parser, mdast,
};
use t3_client::view_logic::session::markdown::{
    IncrementalMarkdownParser, MarkdownNode, MarkdownPoint, MarkdownPosition,
};

pub const CODE_CHUNK_LINES: usize = 40;

/// Byte work includes parsing/indexing/copying; row visits include retained-row comparisons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlattenWork {
    pub indexed_bytes: usize,
    pub split_bytes: usize,
    pub parsed_bytes: usize,
    pub copied_bytes: usize,
    pub visited_rows: usize,
}

impl FlattenWork {
    pub fn byte_work(self) -> usize {
        self.indexed_bytes + self.split_bytes + self.parsed_bytes + self.copied_bytes
    }

    fn add(&mut self, other: Self) {
        self.indexed_bytes += other.indexed_bytes;
        self.split_bytes += other.split_bytes;
        self.parsed_bytes += other.parsed_bytes;
        self.copied_bytes += other.copied_bytes;
        self.visited_rows += other.visited_rows;
    }
}

#[derive(Default)]
struct StreamData {
    pieces: BTreeMap<usize, Rc<str>>,
    bytes: usize,
    utf16: usize,
    blocks: BTreeMap<usize, Rc<Block>>,
    definitions: BTreeMap<(bool, String), Rc<DefinitionFact>>,
    context_version: usize,
    reference_blocks: ReferenceBlocks,
}

#[derive(Clone, Default)]
struct ReferenceBlocks(Rc<RefCell<BTreeMap<usize, BlockId>>>);

impl PartialEq for ReferenceBlocks {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ReferenceBlocks {}

/// Only bracket-bearing prose/table/document blocks can depend on reference definitions.
#[derive(Clone, PartialEq, Eq)]
pub struct ReferenceInvalidation {
    pub identifier: String,
    pub footnote: bool,
    pub context_version: usize,
    blocks: ReferenceBlocks,
    before: usize,
}

impl ReferenceInvalidation {
    /// Enumerate affected earlier IDs only when reconciling reference bindings: O(candidates).
    pub fn for_each_block(&self, mut apply: impl FnMut(&BlockId)) {
        for id in self
            .blocks
            .0
            .borrow()
            .range(..self.before)
            .map(|(_, id)| id)
        {
            apply(id);
        }
    }

    pub fn affects(&self, id: &BlockId) -> bool {
        id.index < self.before && self.blocks.0.borrow().get(&id.index) == Some(id)
    }
}

#[derive(Clone, Debug)]
pub struct DefinitionFact {
    pub node: Rc<MarkdownNode>,
    pub source: Rc<str>,
    pub source_range: SourceRange,
}

/// A shared document, read by row index or source range without copying the message.
#[derive(Clone, Default)]
pub struct StreamBlocks(Rc<RefCell<StreamData>>);

impl PartialEq for StreamBlocks {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for StreamBlocks {}

impl StreamBlocks {
    pub fn block(&self, index: usize) -> Option<Rc<Block>> {
        self.0.borrow().blocks.get(&index).cloned()
    }

    pub fn block_count(&self) -> usize {
        self.0.borrow().blocks.len()
    }

    pub fn source_len(&self) -> usize {
        self.0.borrow().bytes
    }

    pub fn context_version(&self) -> usize {
        self.0.borrow().context_version
    }

    pub fn definition(&self, identifier: &str, footnote: bool) -> Option<Rc<DefinitionFact>> {
        self.0
            .borrow()
            .definitions
            .get(&(footnote, identifier.to_owned()))
            .cloned()
    }

    /// Materialize only a requested row/selection; never request the whole range per delta.
    pub fn source_range(&self, range: Range<usize>) -> Option<String> {
        let data = self.0.borrow();
        if range.start > range.end || range.end > data.bytes {
            return None;
        }
        if range.is_empty() {
            return Some(String::new());
        }
        let first = *data.pieces.range(..=range.start).next_back()?.0;
        let mut result = String::with_capacity(range.len());
        for (&at, text) in data.pieces.range(first..) {
            if at >= range.end {
                break;
            }
            let start = range.start.saturating_sub(at);
            let end = (range.end - at).min(text.len());
            result.push_str(text.get(start..end)?);
        }
        Some(result)
    }
}

/// Splice only this tail into the list; unchanged rows remain in `document` by identity.
#[derive(Clone, PartialEq, Eq)]
pub struct BlockDelta {
    pub document: StreamBlocks,
    pub changed_from: usize,
    pub old_count: usize,
    pub rows: Vec<Rc<Block>>,
    pub context_version: usize,
    pub invalidations: Vec<ReferenceInvalidation>,
}

struct Continuation {
    header: String,
    line_base: usize,
    value_base: usize,
}

struct AppendState {
    document: StreamBlocks,
    pending: String,
    base_byte: usize,
    base_utf16: usize,
    stable: usize,
    continuation: Option<Continuation>,
    local: Box<MessageFlattener>,
    output: bool,
    work: FlattenWork,
    output_line_base: usize,
    finished: bool,
    active_definitions: Vec<(bool, String)>,
    invalidations: Vec<ReferenceInvalidation>,
}

impl AppendState {
    fn new(message_id: &str, output: bool) -> Self {
        let document = StreamBlocks::default();
        document.0.borrow_mut().blocks.insert(
            0,
            Rc::new(Block {
                id: BlockId {
                    message_id: message_id.into(),
                    index: 0,
                },
                kind: BlockKind::Header,
                text: None,
                source: SourceRange {
                    bytes: 0..0,
                    utf16: 0..0,
                },
            }),
        );
        Self {
            document,
            pending: String::new(),
            base_byte: 0,
            base_utf16: 0,
            stable: 1,
            continuation: None,
            local: Box::new(MessageFlattener::new(message_id)),
            output,
            work: FlattenWork::default(),
            output_line_base: 0,
            finished: false,
            active_definitions: Vec::new(),
            invalidations: Vec::new(),
        }
    }

    fn push(&mut self, delta: &str) -> BlockDelta {
        let old_count = self.document.block_count();
        if delta.is_empty() {
            return BlockDelta {
                document: self.document.clone(),
                changed_from: old_count,
                old_count,
                rows: Vec::new(),
                context_version: self.document.context_version(),
                invalidations: Vec::new(),
            };
        }
        self.finished = false;
        {
            let mut data = self.document.0.borrow_mut();
            let start = data.bytes;
            data.pieces.insert(start, Rc::from(delta));
            data.bytes += delta.len();
            data.utf16 += delta.encode_utf16().count();
        }
        self.work.indexed_bytes += delta.len();
        self.work.copied_bytes += delta.len();

        if self.pending.capacity() - self.pending.len() < delta.len() {
            self.work.copied_bytes += self.pending.len();
        }
        self.pending.push_str(delta);
        self.work.copied_bytes += delta.len();
        let header = self.continuation.as_ref().map_or("", |c| c.header.as_str());
        let mut source = String::with_capacity(header.len() + self.pending.len());
        source.push_str(header);
        source.push_str(&self.pending);
        self.work.copied_bytes += source.len();
        let header_bytes = header.len();
        let header_utf16 = header.encode_utf16().count();
        let before = self.local.work();
        let local = self.local.parse_tail(&source, self.output);
        let after = self.local.work();
        self.work.add(FlattenWork {
            indexed_bytes: after.indexed_bytes - before.indexed_bytes,
            split_bytes: after.split_bytes - before.split_bytes,
            parsed_bytes: after.parsed_bytes - before.parsed_bytes,
            copied_bytes: after.copied_bytes - before.copied_bytes,
            visited_rows: after.visited_rows - before.visited_rows,
        });
        let local_stable = stable_prefix(&local.blocks, &source, self.output, false);
        let mut group_header = header.to_owned();
        self.publish_definitions(&local);
        let mut continuing = self.continuation.is_some();
        let mut freeze = None;
        let mut mapped = Vec::with_capacity(local.blocks.len().saturating_sub(1));
        for (index, block) in local.blocks.iter().enumerate().skip(1) {
            self.work.visited_rows += 1;
            let mut block = (**block).clone();
            if let BlockKind::Output { start_line, .. } = &mut block.kind {
                *start_line += self.output_line_base;
            }
            if let BlockKind::Code(code) = &mut block.kind {
                if code.first && !continuing {
                    let raw = &source[block.source.bytes.start..block.source.bytes.end];
                    group_header = if fence(raw.lines().next().unwrap_or_default()).is_some() {
                        raw[..raw.find(['\r', '\n']).map_or(raw.len(), |at| {
                            at + if raw[at..].starts_with("\r\n") { 2 } else { 1 }
                        })]
                            .into()
                    } else {
                        String::new()
                    };
                }
                if continuing && let Some(context) = &self.continuation {
                    code.first = false;
                    code.start_line += context.line_base;
                    code.value_bytes = code.value_bytes.start + context.value_base
                        ..code.value_bytes.end + context.value_base;
                }
                if index + 1 == local_stable && !code.last {
                    freeze = Some(Continuation {
                        header: group_header.clone(),
                        line_base: code.start_line - 1 + CODE_CHUNK_LINES,
                        value_base: code.value_bytes.end,
                    });
                }
                if code.last {
                    continuing = false;
                }
            }
            block.id.index = self.stable + index - 1;
            block.source = SourceRange {
                bytes: self.base_byte + block.source.bytes.start.saturating_sub(header_bytes)
                    ..self.base_byte + block.source.bytes.end.saturating_sub(header_bytes),
                utf16: self.base_utf16 + block.source.utf16.start.saturating_sub(header_utf16)
                    ..self.base_utf16 + block.source.utf16.end.saturating_sub(header_utf16),
            };
            mapped.push(Rc::new(block));
        }
        if self.output
            && local_stable > 1
            && let Some(Block {
                kind: BlockKind::Output { start_line, .. },
                ..
            }) = mapped.get(local_stable - 2).map(Rc::as_ref)
        {
            self.output_line_base = start_line - 1 + CODE_CHUNK_LINES;
        }
        let change = self.splice(old_count, mapped);
        if local_stable > 1 {
            let end = &local.blocks[local_stable - 1].source;
            let cut = end.bytes.end - header_bytes;
            self.pending = self.pending.split_off(cut);
            self.work.copied_bytes += self.pending.len();
            self.base_byte += cut;
            self.base_utf16 += end.utf16.end - header_utf16;
            self.stable += local_stable - 1;
            self.work.visited_rows += self.active_definitions.len();
            {
                let data = self.document.0.borrow();
                self.active_definitions.retain(|key| {
                    data.definitions
                        .get(key)
                        .is_some_and(|fact| fact.source_range.bytes.start >= self.base_byte)
                });
            }
            self.continuation = freeze;
            *self.local = MessageFlattener::new(&self.local.message_id);
        }
        change
    }

    fn splice(&mut self, old_count: usize, mut mapped: Vec<Rc<Block>>) -> BlockDelta {
        let mut data = self.document.0.borrow_mut();
        let mut changed = old_count.min(self.stable + mapped.len());
        for (offset, block) in mapped.iter_mut().enumerate() {
            let index = self.stable + offset;
            self.work.visited_rows += 1;
            if let Some(old) = data.blocks.get(&index)
                && old.as_ref() == block.as_ref()
            {
                *block = old.clone();
            } else {
                changed = changed.min(index);
            }
        }
        self.work.visited_rows += data.blocks.len().saturating_sub(self.stable);
        {
            let mut candidates = data.reference_blocks.0.borrow_mut();
            candidates.split_off(&self.stable);
            for block in &mapped {
                if matches!(
                    block.kind,
                    BlockKind::Prose | BlockKind::Document | BlockKind::Table
                ) {
                    let text = block.text.as_deref().unwrap_or_default();
                    self.work.split_bytes += text.len();
                    if text.contains('[') {
                        candidates.insert(block.id.index, block.id.clone());
                    }
                }
            }
        }
        let _removed = data.blocks.split_off(&self.stable);
        data.blocks.extend(
            mapped
                .into_iter()
                .enumerate()
                .map(|(offset, row)| (self.stable + offset, row)),
        );
        let rows: Vec<_> = data
            .blocks
            .range(changed..)
            .map(|(_, row)| row.clone())
            .collect();
        self.work.visited_rows += rows.len();
        BlockDelta {
            document: self.document.clone(),
            changed_from: changed,
            old_count,
            rows,
            context_version: data.context_version,
            invalidations: std::mem::take(&mut self.invalidations),
        }
    }

    fn publish_definitions(&mut self, local: &TailBlocks) {
        let header_bytes = self
            .continuation
            .as_ref()
            .map_or(0, |context| context.header.len());
        let header_utf16 = self
            .continuation
            .as_ref()
            .map_or(0, |context| context.header.encode_utf16().count());
        self.work.visited_rows += local.definition_nodes.len() * 2 + self.active_definitions.len();
        let candidates: BTreeMap<_, _> = local
            .definition_nodes
            .iter()
            .filter_map(|node| {
                let identifier = node.data.get("identifier")?.as_str()?;
                let start = node.position.as_ref()?.start.offset?;
                Some((
                    (node.kind == "footnoteDefinition", identifier.to_owned()),
                    self.base_byte + self.local.index.byte(start).saturating_sub(header_bytes),
                ))
            })
            .rev()
            .collect();
        {
            let mut data = self.document.0.borrow_mut();
            for key in self.active_definitions.drain(..) {
                if let Some(fact) = data.definitions.get(&key)
                    && fact.source_range.bytes.start >= self.base_byte
                    && candidates.get(&key) != Some(&fact.source_range.bytes.start)
                {
                    data.definitions.remove(&key);
                    data.context_version += 1;
                    self.invalidations.push(ReferenceInvalidation {
                        identifier: key.1.clone(),
                        footnote: key.0,
                        context_version: data.context_version,
                        blocks: data.reference_blocks.clone(),
                        before: self.stable,
                    });
                }
            }
        }
        for node in local.definition_nodes.iter() {
            let Some(position) = &node.position else {
                continue;
            };
            let (Some(start), Some(end)) = (position.start.offset, position.end.offset) else {
                continue;
            };
            let footnote = node.kind == "footnoteDefinition";
            let byte_end = self.local.index.byte(end);
            let Some(identifier) = node
                .data
                .get("identifier")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let key = (footnote, identifier.to_owned());
            let bytes = self.base_byte + self.local.index.byte(start).saturating_sub(header_bytes)
                ..self.base_byte + byte_end.saturating_sub(header_bytes);
            let range = SourceRange {
                bytes,
                utf16: self.base_utf16 + start.saturating_sub(header_utf16)
                    ..self.base_utf16 + end.saturating_sub(header_utf16),
            };
            let mut data = self.document.0.borrow_mut();
            if data
                .definitions
                .get(&key)
                .is_none_or(|previous| previous.source_range.bytes.start == range.bytes.start)
            {
                self.active_definitions.push(key.clone());
            }
            if let Some(previous) = data.definitions.get(&key) {
                if previous.source_range.bytes.start != range.bytes.start {
                    continue;
                }
                if previous.source_range.bytes.end == range.bytes.end
                    && previous.node.data == node.data
                {
                    continue;
                }
            }
            let source = &local.source[self.local.index.byte(start)..byte_end];
            self.work.copied_bytes += source.len();
            self.work.visited_rows += 1;
            data.context_version += 1;
            self.invalidations.push(ReferenceInvalidation {
                identifier: key.1.clone(),
                footnote: key.0,
                context_version: data.context_version,
                blocks: data.reference_blocks.clone(),
                before: self.stable,
            });
            data.definitions.insert(
                key,
                Rc::new(DefinitionFact {
                    node: node.clone(),
                    source: Rc::from(source),
                    source_range: range,
                }),
            );
        }
    }
}

#[derive(Default)]
struct SourceIndex {
    unicode: Vec<(usize, usize)>,
    lines: Vec<usize>,
    crlf: Vec<(usize, usize)>,
}

impl SourceIndex {
    fn new(source: &str) -> Self {
        let mut index = Self {
            lines: vec![0],
            ..Default::default()
        };
        let mut utf16 = 0;
        let mut removed = 0;
        let mut chars = source.char_indices().peekable();
        while let Some((at, ch)) = chars.next() {
            utf16 += ch.len_utf16();
            let end = at + ch.len_utf8();
            if !ch.is_ascii() {
                index.unicode.push((end, end - utf16));
            }
            if ch == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                chars.next();
                utf16 += 1;
                removed += 1;
                index.crlf.push((end + 1 - removed, removed));
                index.lines.push(end + 1);
            } else if matches!(ch, '\n' | '\r') {
                index.lines.push(end);
            }
        }
        index
    }

    fn utf16(&self, byte: usize) -> usize {
        let at = self.unicode.partition_point(|(end, _)| *end <= byte);
        byte - at.checked_sub(1).map_or(0, |at| self.unicode[at].1)
    }

    fn byte(&self, utf16: usize) -> usize {
        let at = self
            .unicode
            .partition_point(|(end, delta)| end - delta <= utf16);
        utf16 + at.checked_sub(1).map_or(0, |at| self.unicode[at].1)
    }

    fn original_byte(&self, normalized: usize) -> usize {
        let at = self.crlf.partition_point(|(end, _)| *end <= normalized);
        normalized + at.checked_sub(1).map_or(0, |at| self.crlf[at].1)
    }

    fn point(&self, byte: usize) -> MarkdownPoint {
        let line = self.lines.partition_point(|start| *start <= byte);
        MarkdownPoint {
            line,
            column: self.utf16(byte) - self.utf16(self.lines[line - 1]) + 1,
            offset: Some(self.utf16(byte)),
        }
    }
}

/// Scoped to a message (or work entry), independent of virtual-list positions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId {
    pub message_id: String,
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRange {
    pub bytes: Range<usize>,
    pub utf16: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeChunk {
    pub language: Option<String>,
    pub meta: Option<String>,
    /// Exact parsed code, including the newline between adjacent chunks.
    pub text: String,
    /// One-based line in this code block, not in the message.
    pub start_line: usize,
    pub first: bool,
    pub last: bool,
    /// Range in the code node's value; appends skip completed chunks.
    pub value_bytes: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Header,
    Prose,
    /// The web keeps document-wide syntax and Insight callouts together.
    Document,
    Table,
    Definition {
        footnote: bool,
    },
    Code(CodeChunk),
    Output {
        text: String,
        start_line: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
    /// Disjoint ranges cover the original source, including fences and separators.
    pub source: SourceRange,
    /// Owned Markdown/code for this row, independent of message snapshots.
    pub text: Option<Rc<str>>,
}

/// Cached parser data for the open tail window.
#[derive(Clone, Debug)]
struct TailBlocks {
    pub source: Rc<str>,
    pub blocks: Rc<Vec<Rc<Block>>>,
    /// Resolve links/footnotes against this context, not a row in isolation.
    pub definitions: Rc<str>,
    pub definition_nodes: Rc<Vec<Rc<MarkdownNode>>>,
}

/// Keep one instance per message outside recycled cells; initialize plain output with replace_output.
pub struct MessageFlattener {
    message_id: String,
    parser: IncrementalMarkdownParser,
    previous: Option<TailBlocks>,
    stable: usize,
    output: bool,
    index: SourceIndex,
    work: FlattenWork,
    stream: Option<AppendState>,
}

impl MessageFlattener {
    pub fn new(message_id: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            parser: IncrementalMarkdownParser::default(),
            previous: None,
            stable: 0,
            output: false,
            index: SourceIndex::default(),
            work: FlattenWork::default(),
            stream: None,
        }
    }

    pub fn work(&self) -> FlattenWork {
        let mut work = self.work;
        if let Some(stream) = &self.stream {
            work.add(stream.work);
        }
        work
    }

    /// Borrow a projection's text; compare/reparse only the open tail, never its settled prefix.
    pub fn ingest(&mut self, message_id: &str, source: &str) -> BlockDelta {
        self.ingest_mode(message_id, source, false)
    }

    pub fn ingest_output(&mut self, message_id: &str, source: &str) -> BlockDelta {
        self.ingest_mode(message_id, source, true)
    }

    /// Use this for edits to settled content; an append guard cannot inspect that prefix.
    pub fn replace(&mut self, message_id: &str, source: &str) -> BlockDelta {
        self.replace_mode(message_id, source, false)
    }

    pub fn replace_output(&mut self, message_id: &str, source: &str) -> BlockDelta {
        self.replace_mode(message_id, source, true)
    }

    fn replace_mode(&mut self, message_id: &str, source: &str, output: bool) -> BlockDelta {
        let old_count = self
            .stream
            .as_ref()
            .map_or(0, |state| state.document.block_count());
        self.message_id = message_id.into();
        self.stream = Some(AppendState::new(message_id, output));
        let mut result = self.append(source);
        result.old_count = old_count;
        result.changed_from = 0;
        result
            .rows
            .insert(0, result.document.block(0).expect("header"));
        result
    }

    /// A validated append from a delta-aware caller; no prefix comparison or snapshot copy.
    pub fn append(&mut self, delta: &str) -> BlockDelta {
        let initializing = self.stream.is_none();
        let mut result = self
            .stream
            .get_or_insert_with(|| AppendState::new(&self.message_id, false))
            .push(delta);
        if initializing {
            result.changed_from = 0;
            result.old_count = 0;
            result
                .rows
                .insert(0, result.document.block(0).expect("header"));
        }
        result
    }

    /// Publish an unterminated final definition block once, without rereading settled source.
    pub fn finish(&mut self) -> Option<BlockDelta> {
        let state = self.stream.as_mut()?;
        if state.finished {
            return None;
        }
        let header = state
            .continuation
            .as_ref()
            .map_or("", |continuation| continuation.header.as_str());
        let source = format!("{header}{}", state.pending);
        state.work.copied_bytes += source.len();
        let before = state.local.work();
        let local = state.local.parse_tail(&source, state.output);
        let after = state.local.work();
        state.work.add(FlattenWork {
            indexed_bytes: after.indexed_bytes - before.indexed_bytes,
            split_bytes: after.split_bytes - before.split_bytes,
            parsed_bytes: after.parsed_bytes - before.parsed_bytes,
            copied_bytes: after.copied_bytes - before.copied_bytes,
            visited_rows: after.visited_rows - before.visited_rows,
        });
        state.publish_definitions(&local);
        state.finished = true;
        let count = state.document.block_count();
        Some(BlockDelta {
            document: state.document.clone(),
            changed_from: count,
            old_count: count,
            rows: Vec::new(),
            context_version: state.document.context_version(),
            invalidations: std::mem::take(&mut state.invalidations),
        })
    }

    fn ingest_mode(&mut self, message_id: &str, source: &str, output: bool) -> BlockDelta {
        let append = self.message_id == message_id
            && self.stream.as_mut().is_some_and(|state| {
                if state.output != output || source.len() < state.document.source_len() {
                    return false;
                }
                let end = state.document.source_len();
                state.work.split_bytes += end - state.base_byte;
                source.get(state.base_byte..end) == Some(state.pending.as_str())
            });
        if append {
            let state = self.stream.as_mut().expect("checked stream");
            let previous = state.document.source_len();
            state.push(&source[previous..])
        } else {
            let old_count = self
                .stream
                .as_ref()
                .map_or(0, |state| state.document.block_count());
            self.message_id = message_id.into();
            let state = self.stream.insert(AppendState::new(message_id, output));
            let mut result = state.push(source);
            result.old_count = old_count;
            result.changed_from = 0;
            result
                .rows
                .insert(0, result.document.block(0).expect("header"));
            result
        }
    }

    fn parse_tail(&mut self, source: &str, output: bool) -> TailBlocks {
        if self.output == output
            && let Some(previous) = &self.previous
            && previous.source.as_ref() == source
        {
            return previous.clone();
        }
        let append = self.output == output
            && self
                .previous
                .as_ref()
                .is_some_and(|p| source.starts_with(p.source.as_ref()));
        self.work.indexed_bytes += source.len();
        self.index = SourceIndex::new(source);
        self.work.split_bytes += if output { 0 } else { source.len() };
        let document_wide = !output && split_markdown_blocks(source).is_none();
        // A late definition can change earlier rendering without changing those blocks' source.
        let stable = if append && !document_wide {
            self.stable
        } else {
            1
        };
        let mut blocks = self
            .previous
            .as_ref()
            .filter(|_| append)
            .map_or_else(Vec::new, |p| {
                p.blocks.iter().take(stable).cloned().collect()
            });
        self.work.visited_rows += blocks.len();
        if blocks.is_empty() {
            blocks.push(Rc::new(Block {
                id: BlockId {
                    message_id: self.message_id.clone(),
                    index: 0,
                },
                kind: BlockKind::Header,
                text: None,
                source: SourceRange {
                    bytes: 0..0,
                    utf16: 0..0,
                },
            }));
        }
        let resume = blocks.last().map_or(0, |b| b.source.bytes.end);
        let mut definitions = String::new();
        let mut definition_nodes = Vec::new();
        if output {
            self.output_chunks(source, resume, &mut blocks);
        } else {
            let work = &mut self.work;
            let root = self.parser.parse(source, |text| {
                work.parsed_bytes += text.len();
                work.indexed_bytes += text.len();
                parse_markdown(text)
            });
            collect_definitions(&root, source, &self.index, &mut definitions);
            collect_definition_nodes(&root, &mut definition_nodes);
            let nodes: Vec<_> = root
                .children
                .iter()
                .filter(|n| {
                    matches!(
                        n.kind.as_str(),
                        "code" | "table" | "definition" | "footnoteDefinition"
                    )
                })
                .filter_map(|node| {
                    let position = node.position.as_ref()?;
                    Some((
                        self.index.byte(position.start.offset?)
                            ..self.index.byte(position.end.offset?),
                        node,
                    ))
                })
                .collect();
            let parts = split_structural_blocks(source);
            let mut offset = 0;
            let mut node_at = 0;
            for part in parts {
                let end = offset + part.len();
                if end > resume {
                    while node_at < nodes.len() && nodes[node_at].0.end <= offset.max(resume) {
                        node_at += 1;
                    }
                    let first = node_at;
                    while node_at < nodes.len() && nodes[node_at].0.start < end {
                        node_at += 1;
                    }
                    if flags_outside_code(source, offset..end, &nodes[first..node_at]).1
                        || part.split('\n').any(|line| insight(line, true).is_some())
                    {
                        self.push(
                            source,
                            offset.max(resume)..end,
                            BlockKind::Document,
                            &mut blocks,
                        );
                    } else {
                        self.markdown_part(
                            source,
                            &nodes[first..node_at],
                            offset..end,
                            resume,
                            &mut blocks,
                        );
                    }
                }
                offset = end;
            }
        }
        self.stable = stable_prefix(&blocks, source, output, document_wide);
        self.work.copied_bytes += blocks
            .iter()
            .skip(stable.min(blocks.len()))
            .map(|block| block.text.as_ref().map_or(0, |text| text.len()))
            .sum::<usize>();
        self.work.visited_rows += blocks.len().saturating_sub(stable);
        if let Some(previous) = &self.previous {
            self.work.visited_rows += blocks.len().min(previous.blocks.len());
            for (block, old) in blocks.iter_mut().zip(previous.blocks.iter()) {
                if block.as_ref() == old.as_ref() && previous.definitions.as_ref() == definitions {
                    *block = old.clone();
                }
            }
        }
        self.work.copied_bytes += source.len();
        let next = TailBlocks {
            source: Rc::from(source),
            blocks: Rc::new(blocks),
            definitions: Rc::from(definitions),
            definition_nodes: Rc::new(definition_nodes),
        };
        self.output = output;
        self.previous = Some(next.clone());
        next
    }

    fn push(
        &self,
        source: &str,
        range: Range<usize>,
        kind: BlockKind,
        blocks: &mut Vec<Rc<Block>>,
    ) {
        let text = match &kind {
            BlockKind::Header | BlockKind::Definition { .. } => None,
            BlockKind::Code(code) => Some(Rc::from(code.text.as_str())),
            BlockKind::Output { text, .. } => Some(Rc::from(text.as_str())),
            _ => Some(Rc::from(&source[range.clone()])),
        };
        blocks.push(Rc::new(Block {
            id: BlockId {
                message_id: self.message_id.clone(),
                index: blocks.len(),
            },
            source: SourceRange {
                utf16: self.index.utf16(range.start)..self.index.utf16(range.end),
                bytes: range,
            },
            kind,
            text,
        }));
    }

    fn markdown_part(
        &self,
        source: &str,
        nodes: &[(Range<usize>, &MarkdownNode)],
        range: Range<usize>,
        resume: usize,
        blocks: &mut Vec<Rc<Block>>,
    ) {
        let mut cursor = range.start.max(resume);
        for (node_range, node) in nodes {
            let start = node_range.start;
            let end = node_range.end;
            if start < range.start
                || end > range.end
                || end <= resume
                || !matches!(
                    node.kind.as_str(),
                    "code" | "table" | "definition" | "footnoteDefinition"
                )
            {
                continue;
            }
            if cursor < start {
                if source[cursor..start].trim().is_empty() && blocks.len() > 1 {
                    let previous = blocks.pop().expect("content row");
                    self.push(
                        source,
                        previous.source.bytes.start..start,
                        previous.kind.clone(),
                        blocks,
                    );
                } else {
                    self.push(source, cursor..start, BlockKind::Prose, blocks);
                }
                cursor = start;
            }
            if matches!(node.kind.as_str(), "definition" | "footnoteDefinition") {
                self.push(
                    source,
                    cursor..end,
                    BlockKind::Definition {
                        footnote: node.kind == "footnoteDefinition",
                    },
                    blocks,
                );
            } else if node.kind == "table" {
                self.push(source, cursor..end, BlockKind::Table, blocks);
            } else {
                self.code_chunks(source, node, start..end, cursor, blocks);
            }
            cursor = end;
        }
        if cursor < range.end {
            let tail = &source[cursor..range.end];
            let whitespace = tail
                .bytes()
                .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
                .count();
            if let Some(newline) = tail[..whitespace].rfind(['\r', '\n'])
                && blocks.len() > 1
                && !tail.trim().is_empty()
            {
                let previous = blocks.pop().expect("content row");
                cursor += newline + 1;
                self.push(
                    source,
                    previous.source.bytes.start..cursor,
                    previous.kind.clone(),
                    blocks,
                );
            }
            if source[cursor..range.end].trim().is_empty() && blocks.len() > 1 {
                let last = blocks.pop().expect("a content block exists");
                self.push(
                    source,
                    last.source.bytes.start..range.end,
                    last.kind.clone(),
                    blocks,
                );
            } else {
                self.push(source, cursor..range.end, BlockKind::Prose, blocks);
            }
        }
    }

    fn code_chunks(
        &self,
        source: &str,
        node: &MarkdownNode,
        range: Range<usize>,
        resume: usize,
        blocks: &mut Vec<Rc<Block>>,
    ) {
        let value = node
            .data
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let language = node
            .data
            .get("lang")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let meta = node
            .data
            .get("meta")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let previous = blocks.last().and_then(|b| match &b.kind {
            BlockKind::Code(c) if b.source.bytes.end == resume && !c.last => Some(c),
            _ => None,
        });
        let mut value_at = previous.map_or(0, |c| c.value_bytes.end);
        let mut line = previous.map_or(1, |c| c.start_line + CODE_CHUNK_LINES);
        let mut source_at = resume;
        let mut raw_at = if previous.is_some() {
            resume
        } else {
            let raw = &source[range.clone()];
            if fence(raw.lines().next().unwrap_or_default()).is_some() {
                range.start
                    + raw.find(['\r', '\n']).map_or(raw.len(), |at| {
                        at + if raw[at..].starts_with("\r\n") { 2 } else { 1 }
                    })
            } else {
                range.start
            }
        };
        loop {
            let end = chunk_end(value, value_at);
            let last = end == value.len();
            let raw_end = if last {
                range.end
            } else {
                raw_chunk_end(&source[..range.end], raw_at)
            };
            self.push(
                source,
                source_at..raw_end,
                BlockKind::Code(CodeChunk {
                    language: language.clone(),
                    meta: meta.clone(),
                    text: value[value_at..end].into(),
                    start_line: line,
                    first: value_at == 0,
                    last,
                    value_bytes: value_at..end,
                }),
                blocks,
            );
            if last {
                break;
            }
            source_at = raw_end;
            raw_at = raw_end;
            value_at = end;
            line += CODE_CHUNK_LINES;
        }
    }

    fn output_chunks(&self, source: &str, resume: usize, blocks: &mut Vec<Rc<Block>>) {
        let mut at = resume;
        let mut line = blocks
            .last()
            .and_then(|b| match b.kind {
                BlockKind::Output { start_line, .. } => Some(start_line + CODE_CHUNK_LINES),
                _ => None,
            })
            .unwrap_or(1);
        while at < source.len() {
            let end = raw_chunk_end(source, at);
            self.push(
                source,
                at..end,
                BlockKind::Output {
                    text: source[at..end].into(),
                    start_line: line,
                },
                blocks,
            );
            at = end;
            line += CODE_CHUNK_LINES;
        }
    }
}

fn chunk_end(source: &str, start: usize) -> usize {
    source[start..]
        .match_indices('\n')
        .nth(CODE_CHUNK_LINES - 1)
        .map_or(source.len(), |(at, _)| start + at + 1)
}

fn raw_chunk_end(source: &str, start: usize) -> usize {
    let mut at = start;
    for _ in 0..CODE_CHUNK_LINES {
        let Some(next) = source[at..].find(['\r', '\n']) else {
            return source.len();
        };
        at += next;
        at += if source[at..].starts_with("\r\n") {
            2
        } else {
            1
        };
    }
    at
}

fn stable_prefix(blocks: &[Rc<Block>], source: &str, output: bool, document_wide: bool) -> usize {
    if document_wide {
        return 1;
    }
    let mut stable = blocks.len().saturating_sub(1).max(1);
    if !output {
        while stable > 1 {
            let block = &blocks[stable - 1];
            if let BlockKind::Code(code) = &block.kind
                && code.last
                && !ends_with_blank_line(&source[block.source.bytes.clone()])
            {
                stable -= 1;
            } else {
                break;
            }
        }
        // A following partial fence can retract the preceding chunk's newline and `last` flag.
        if stable > 1
            && matches!(
                blocks.last().map(|block| &block.kind),
                Some(BlockKind::Code(_))
            )
            && matches!(&blocks[stable - 1].kind, BlockKind::Code(code) if !code.last)
        {
            stable -= 1;
        }
    }
    stable
}

fn ends_with_blank_line(source: &str) -> bool {
    let source = source.trim_end_matches([' ', '\t']);
    let Some(before) = source
        .strip_suffix("\r\n")
        .or_else(|| source.strip_suffix(['\r', '\n']))
    else {
        return false;
    };
    before.trim_end_matches([' ', '\t']).ends_with(['\r', '\n'])
}

fn collect_definitions(node: &MarkdownNode, source: &str, index: &SourceIndex, out: &mut String) {
    if matches!(node.kind.as_str(), "definition" | "footnoteDefinition") {
        if let Some(position) = &node.position
            && let (Some(start), Some(end)) = (position.start.offset, position.end.offset)
        {
            out.push_str(&source[index.byte(start)..index.byte(end)]);
            out.push_str("\n\n");
        }
    } else {
        for child in &node.children {
            collect_definitions(child, source, index, out);
        }
    }
}

fn collect_definition_nodes(node: &MarkdownNode, out: &mut Vec<Rc<MarkdownNode>>) {
    if matches!(node.kind.as_str(), "definition" | "footnoteDefinition") {
        out.push(Rc::new(node.clone()));
    }
    for child in &node.children {
        collect_definition_nodes(child, out);
    }
}

fn flags_outside_code(
    source: &str,
    range: Range<usize>,
    nodes: &[(Range<usize>, &MarkdownNode)],
) -> (bool, bool) {
    let mut opaque = false;
    let mut compound = false;
    let mut scan = |part: &str| {
        opaque |= part.contains("[^") || has_document_syntax(part);
        compound |= part
            .split(['\n', '\r', '\u{2028}', '\u{2029}'])
            .any(|line| document_syntax(line) && !line.trim_start_matches(' ').starts_with('['));
    };
    let mut at = range.start;
    for (code, node) in nodes {
        if node.kind != "code" || code.start < at || code.end > range.end {
            continue;
        }
        scan(&source[at..code.start]);
        at = code.end;
    }
    scan(&source[at..range.end]);
    (opaque, compound)
}

// NSH exposes mdast through a plugin hook. Stop conversion there: the client owns the cache,
// and flattening must not build TextViews, syntax spans or native views.
fn parse_markdown(source: &str) -> MarkdownNode {
    #[derive(Clone)]
    struct Capture(Rc<RefCell<Vec<MarkdownNode>>>, Rc<SourceIndex>);
    impl MarkdownPlugin for Capture {
        fn name(&self) -> &str {
            "t3-timeline-blocks"
        }
        fn block(&self, node: &mdast::Node, _cx: &mut ParseContext<'_>) -> Option<NshBlock> {
            self.0.borrow_mut().push(convert_node(node, &self.1));
            Some(NshBlock::paragraph(Vec::new()))
        }
    }
    let nodes = Rc::new(RefCell::new(Vec::new()));
    let index = Rc::new(SourceIndex::new(source));
    Parser::new()
        .options(MarkdownOptions {
            // NSH's math rewriter moves source offsets; math groups go whole to their renderer.
            math: false,
            ..Default::default()
        })
        .plugin(Capture(nodes.clone(), index.clone()))
        .markdown(source);
    MarkdownNode {
        kind: "root".into(),
        position: Some(MarkdownPosition {
            start: MarkdownPoint {
                line: 1,
                column: 1,
                offset: Some(0),
            },
            end: index.point(source.len()),
        }),
        children: nodes.take(),
        data: BTreeMap::new(),
    }
}

fn convert_node(node: &mdast::Node, index: &SourceIndex) -> MarkdownNode {
    let kind = match node {
        mdast::Node::Code(_) => "code",
        mdast::Node::Table(_) => "table",
        mdast::Node::Definition(_) => "definition",
        mdast::Node::FootnoteDefinition(_) => "footnoteDefinition",
        _ => "prose",
    };
    let point = |offset| index.point(index.original_byte(offset));
    let position = node.position().map(|p| MarkdownPosition {
        start: point(p.start.offset),
        end: point(p.end.offset),
    });
    let mut data = BTreeMap::new();
    if let mdast::Node::Code(code) = node {
        data.insert("value".into(), code.value.clone().into());
        if let Some(lang) = &code.lang {
            data.insert("lang".into(), lang.clone().into());
        }
        if let Some(meta) = &code.meta {
            data.insert("meta".into(), meta.clone().into());
        }
    }
    if let mdast::Node::Definition(definition) = node {
        data.insert("identifier".into(), definition.identifier.clone().into());
        data.insert("url".into(), definition.url.clone().into());
        if let Some(title) = &definition.title {
            data.insert("title".into(), title.clone().into());
        }
    }
    if let mdast::Node::FootnoteDefinition(definition) = node {
        data.insert("identifier".into(), definition.identifier.clone().into());
    }
    MarkdownNode {
        kind: kind.into(),
        position,
        data,
        children: node.children().map_or_else(Vec::new, |children| {
            children.iter().map(|n| convert_node(n, index)).collect()
        }),
    }
}

fn fence(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let len = trimmed.bytes().take_while(|b| b == marker).count();
    (len >= 3).then_some(&trimmed[..len])
}

fn continuation(line: &str) -> bool {
    if line.starts_with([' ', '\t']) {
        return true;
    }
    let bytes = line.as_bytes();
    let marker = if bytes
        .first()
        .is_some_and(|b| matches!(b, b'-' | b'*' | b'+'))
    {
        1
    } else {
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=9).contains(&digits)
            || !bytes.get(digits).is_some_and(|b| matches!(b, b'.' | b')'))
        {
            return false;
        }
        digits + 1
    };
    bytes.get(marker).is_none_or(|b| matches!(b, b' ' | b'\t'))
}

fn document_syntax(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    trimmed.starts_with(":::")
        || trimmed.starts_with("$$")
        || trimmed
            .strip_prefix('<')
            .and_then(|s| s.chars().next())
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '!' | '?' | '/'))
        || trimmed
            .strip_prefix('[')
            .and_then(|s| s.find("]:").map(|at| at > 0 && !s[..at].contains(']')))
            .unwrap_or(false)
}

fn has_document_syntax(text: &str) -> bool {
    text.split('\n').any(|line| {
        document_syntax(line)
            || line.char_indices().any(|(at, ch)| {
                matches!(ch, '\r' | '\u{2028}' | '\u{2029}')
                    && document_syntax(&line[at + ch.len_utf8()..])
            })
    })
}

fn insight(line: &str, opening: bool) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    let indent = line.len() - trimmed.len();
    if indent > 3 {
        return None;
    }
    let trimmed = trimmed.trim_end_matches([' ', '\t']);
    let body = if trimmed.starts_with('`') && trimmed.ends_with('`') && trimmed.len() > 1 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    let rule = if opening {
        let rest = body.strip_prefix("★ Insight")?;
        if !rest.starts_with([' ', '\t']) {
            return None;
        }
        rest.trim_start_matches([' ', '\t'])
    } else {
        body
    };
    (rule.chars().count() >= 3 && rule.chars().all(|c| c == '─')).then_some(&line[..indent])
}

/// Port of apps/web/src/components/chat/markdownStreamBlocks.ts splitMarkdownBlocks.
pub fn split_markdown_blocks(text: &str) -> Option<Vec<&str>> {
    if text.contains('\r') || text.contains("[^") || has_document_syntax(text) {
        return None;
    }
    Some(split_structural_blocks(text))
}

fn split_structural_blocks(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut block_start = 0;
    let mut line_start = 0;
    let mut open_fence: Option<&str> = None;
    let mut fence_indent = 0;
    let mut after_blank = false;
    let mut insight_indent = None;
    let mut compound: Option<CompoundBlock> = None;
    let mut lines = text.split_inclusive(['\r', '\n']).peekable();
    while let Some(raw) = lines.next() {
        let line = raw.strip_suffix(['\r', '\n']).unwrap_or(raw);
        let mut line_bytes = raw.len();
        if raw.ends_with('\r') && lines.peek() == Some(&"\n") {
            lines.next();
            line_bytes += 1;
        }
        let opener = fence(line);
        if let Some(current) = &mut compound {
            if current.closes(line) {
                compound = None;
                after_blank = false;
            }
        } else if let Some(current) = open_fence {
            if line.chars().take(fence_indent).any(|c| !js_whitespace(c)) {
                break;
            }
            if let Some(opener) = opener
                && opener.as_bytes()[0] == current.as_bytes()[0]
                && opener.len() >= current.len()
                && line.trim_matches(js_whitespace) == opener
            {
                open_fence = None;
            }
        } else if insight_indent.is_some() && insight(line, false) == insight_indent {
            insight_indent = None;
            after_blank = false;
        } else if line.trim_matches(js_whitespace).is_empty() {
            after_blank = true;
        } else {
            if after_blank
                && insight_indent.is_none()
                && line_start > block_start
                && !continuation(line)
            {
                blocks.push(&text[block_start..line_start]);
                block_start = line_start;
            }
            after_blank = false;
            if insight_indent.is_none() {
                insight_indent = insight(line, true);
            }
            open_fence = opener;
            if let Some(opener) = opener {
                fence_indent = line.find(opener).unwrap_or(0);
            }
            if open_fence.is_none() && insight_indent.is_none() {
                compound = CompoundBlock::open(line);
            }
        }
        line_start += line_bytes;
    }
    blocks.push(&text[block_start..]);
    blocks
}

enum CompoundBlock {
    Math(usize),
    Directive(usize),
    Html { tag: String, depth: usize },
    Until(&'static str),
}

impl CompoundBlock {
    fn open(line: &str) -> Option<Self> {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            return None;
        }
        if let Some(tail) = trimmed.strip_prefix("$$") {
            return (!tail.contains("$$"))
                .then(|| Self::Math(trimmed.bytes().take_while(|b| *b == b'$').count()));
        }
        if let Some(tail) = trimmed.strip_prefix(":::") {
            return (!tail.trim().is_empty()).then_some(Self::Directive(1));
        }
        for (opener, closer) in [("<!--", "-->"), ("<?", "?>"), ("<![CDATA[", "]]>")] {
            if trimmed.starts_with(opener) {
                return (!trimmed.contains(closer)).then_some(Self::Until(closer));
            }
        }
        let tag = trimmed
            .strip_prefix('<')?
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | ':'))
            .collect::<String>()
            .to_ascii_lowercase();
        if tag.is_empty()
            || matches!(
                tag.as_str(),
                "area"
                    | "base"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "link"
                    | "meta"
                    | "param"
                    | "source"
                    | "track"
                    | "wbr"
            )
            || trimmed.trim_end().ends_with("/>")
        {
            return None;
        }
        let (opens, closes) = html_tags(line, &tag);
        (opens > closes).then_some(Self::Html {
            tag,
            depth: opens.saturating_sub(closes),
        })
    }

    fn closes(&mut self, line: &str) -> bool {
        let trimmed = line.trim();
        match self {
            Self::Math(length) => trimmed.len() >= *length && trimmed.bytes().all(|b| b == b'$'),
            Self::Directive(depth) => {
                if trimmed.starts_with(":::") {
                    if trimmed.bytes().all(|b| b == b':') {
                        *depth = depth.saturating_sub(1);
                    } else {
                        *depth += 1;
                    }
                }
                *depth == 0
            }
            Self::Html { tag, depth } => {
                let (opens, closes) = html_tags(line, tag);
                *depth = depth.saturating_add(opens).saturating_sub(closes);
                *depth == 0
            }
            Self::Until(closer) => line.contains(*closer),
        }
    }
}

fn html_tags(line: &str, tag: &str) -> (usize, usize) {
    let line = line.to_ascii_lowercase();
    let opener = format!("<{tag}");
    let opens = line
        .match_indices(&opener)
        .filter(|(at, _)| {
            line[at + opener.len()..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_whitespace() || matches!(ch, '>' | '/'))
        })
        .count();
    let closer = format!("</{tag}>");
    (opens, line.matches(closer.as_str()).count())
}

fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "# Before\n\n```ts\nconst values = [1, 2];\n```\n\n";
    const STREAM_CASES: &[&str] = &[
        "a\n===\n\nb\n---\n",
        "- first\n\n  continued\n\n- next\n",
        "> quoted\n>\n> ```js\n> abc\n> ```\n\nend",
        "<div>\nhello\n\n</div>\n\nend",
        "[ref]\n\n[ref]: /later",
        "a[^x]\n\n[^x]: note",
        "a | b\n--|--\na | b\n",
        "```\na\n```\n\nnext\n\n~~~\nb\n~~~\n\nmore",
        "\n\n\tcode\n\nmore",
        "text <https://example.com> *bold*",
        "> [!NOTE]\n> alert\n\n- [ ] task",
        "\u{feff}text after a byte-order mark",
    ];

    struct TestBlocks {
        source: String,
        blocks: Vec<Rc<Block>>,
        definitions: BTreeMap<(bool, String), (Rc<str>, SourceRange)>,
    }

    impl TestBlocks {
        fn source_of(&self, block: &Block) -> Option<&str> {
            self.source.get(block.source.bytes.clone())
        }
    }

    fn snapshot(document: &StreamBlocks) -> TestBlocks {
        TestBlocks {
            source: document
                .source_range(0..document.source_len())
                .expect("source"),
            blocks: (0..document.block_count())
                .map(|index| document.block(index).expect("block"))
                .collect(),
            definitions: document
                .0
                .borrow()
                .definitions
                .iter()
                .map(|(key, fact)| {
                    (
                        key.clone(),
                        (fact.source.clone(), fact.source_range.clone()),
                    )
                })
                .collect(),
        }
    }

    fn replacement(source: &str) -> TestBlocks {
        snapshot(
            &MessageFlattener::new("message")
                .replace("message", source)
                .document,
        )
    }

    fn assert_document(message: &TestBlocks) {
        let mut byte = 0;
        let mut utf16 = 0;
        for (index, block) in message.blocks.iter().enumerate() {
            assert_eq!(block.id.index, index);
            assert_eq!(block.source.bytes.start, byte);
            assert_eq!(block.source.utf16.start, utf16);
            let text = message.source_of(block).expect("valid UTF-8 boundaries");
            byte += text.len();
            utf16 += text.encode_utf16().count();
            assert_eq!(block.source.bytes.end, byte);
            assert_eq!(block.source.utf16.end, utf16);
            if let BlockKind::Code(code) = &block.kind {
                assert!(code.text.lines().count() <= CODE_CHUNK_LINES);
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                assert!(
                    normalized.lines().count() <= CODE_CHUNK_LINES + 3,
                    "a code chunk's raw source must also be bounded: {:?}",
                    block.source
                );
            }
        }
        assert_eq!(byte, message.source.len());
        assert_eq!(utf16, message.source.encode_utf16().count());
    }

    fn assert_stream(source: &str) {
        let mut parser = IncrementalMarkdownParser::default();
        let mut flattener = MessageFlattener::new("message");
        let mut seen = 0;
        flattener.replace("message", "");
        for end in source
            .char_indices()
            .map(|(at, _)| at)
            .chain([source.len()])
        {
            let text = &source[..end];
            assert_eq!(
                parser.parse(text, parse_markdown),
                parse_markdown(text),
                "parse at {end}: {text:?}"
            );
            let delta = flattener.append(&source[seen..end]);
            seen = end;
            let incremental = snapshot(&delta.document);
            let full = replacement(text);
            assert_eq!(
                incremental.blocks, full.blocks,
                "flatten at {end}: {text:?}"
            );
            assert_eq!(
                incremental.definitions, full.definitions,
                "definitions at {end}: {text:?}"
            );
            assert_document(&incremental);
        }
    }

    #[test]
    fn web_incremental_streaming_cases_preserve_tree_positions_and_blocks() {
        for tail in STREAM_CASES {
            assert_stream(&format!("{PREFIX}{tail}"));
        }
    }

    #[test]
    fn appended_ranges_and_code_values_match_replacements() {
        let code = format!(
            "Intro\n\n```rust title=x\n{}tail\n```\n\nAfter",
            "  😀 = 1;\n".repeat(125)
        );
        for source in [
            code,
            format!("    {}\n\nend", "x\n    ".repeat(85)),
            format!("```\n{}```\n\nend", "x\n".repeat(40)),
            format!("```\n{}```\n\nend", "x\n".repeat(80)),
        ] {
            assert_stream(&source);
        }
    }

    #[test]
    fn web_partial_cr_and_crlf_line_endings_preserve_original_offsets() {
        for newline in ["\r\n", "\r"] {
            assert_stream(&format!("{PREFIX}next\n\n```\nlast\n```\n\nend").replace('\n', newline));
        }
    }

    #[test]
    fn web_unsafe_fences_are_never_frozen() {
        for start in [
            "```\nopen\n\n",
            "````\n```\n\n",
            "> ```\n> code\n> ```\n\n",
            "- ```\n  code\n  ```\n\n",
            "    ```\n    code\n    ```\n\n",
            "<script>\n```\ncode\n```\n\n",
        ] {
            for tail in ["", "text", "\n```\n", "\n```\n\nnext"] {
                assert_stream(&format!("{start}{tail}"));
            }
        }
    }

    #[test]
    fn web_definitions_arriving_late_invalidate_bindings_and_retain_blocks() {
        let source = format!("[later] and footnote[^note]\n\n{PREFIX}text");
        let mut flattener = MessageFlattener::new("message");
        let initial = flattener.replace("message", &source);
        let reference = initial.document.block(1).expect("settled references");
        let mut source = source;
        for tail in ["\n\n[later]: /target", "\n\n[^note]: a note"] {
            source.push_str(tail);
            let next = flattener.append(tail);
            assert_eq!(snapshot(&next.document).blocks, replacement(&source).blocks);
            assert!(Rc::ptr_eq(
                &reference,
                &next.document.block(1).expect("retained references")
            ));
            assert!(
                next.invalidations
                    .iter()
                    .any(|invalidation| invalidation.affects(&reference.id))
            );
            assert_eq!(
                next.document
                    .definition("later", false)
                    .expect("reference definition")
                    .node
                    .data["url"],
                "/target"
            );
        }
        assert!(initial.document.definition("note", true).is_some());
    }

    #[test]
    fn web_edits_replacements_and_repeated_renders_do_not_leak_transformed_nodes() {
        let mut flattener = MessageFlattener::new("message");
        for source in [
            format!("{PREFIX}- first\n - second"),
            format!("{PREFIX}> [!NOTE]\n> transformed alert"),
            format!("{PREFIX}plain text"),
            "replacement without fences".into(),
            format!("{}edited prefix", PREFIX.replace("Before", "Edited")),
            format!("{PREFIX}plain text"),
        ] {
            let next = snapshot(&flattener.replace("message", &source).document);
            assert_eq!(next.blocks, replacement(&source).blocks);
            assert_document(&next);
        }
        let before = flattener.replace("message", "same");
        let retained = before.document.block(1).expect("text");
        let after = flattener.append("");
        assert!(after.rows.is_empty());
        assert!(Rc::ptr_eq(
            &retained,
            &after.document.block(1).expect("same text")
        ));
        let mut parser = IncrementalMarkdownParser::default();
        let mut root = parser.parse(PREFIX, parse_markdown);
        root.children.clear();
        assert_eq!(
            parser.parse(&format!("{PREFIX}more"), parse_markdown),
            parse_markdown(&format!("{PREFIX}more"))
        );
    }

    #[test]
    fn web_recovery_parses_do_not_replace_the_document_cache() {
        let mut parser = IncrementalMarkdownParser::default();
        parser.parse(PREFIX, parse_markdown);
        let mut render = parser.begin_render();
        render.parse(&format!("{PREFIX}tail"), parse_markdown);
        render.parse(
            "t3-markdown-inline-prefix:\n```ts\nrecovery\n```\n\n",
            parse_markdown,
        );
        let mut calls = Vec::new();
        parser.parse(&format!("{PREFIX}tail more"), |source| {
            calls.push(source.to_owned());
            parse_markdown(source)
        });
        assert_eq!(calls, ["tail more"]);
    }

    #[test]
    fn web_splitter_preserves_closed_blocks_lists_and_insights() {
        assert_eq!(
            split_markdown_blocks("a\n\n```\nb\n\nc\n```\n\n- d\n\n- e\n\nf"),
            Some(vec!["a\n\n", "```\nb\n\nc\n```\n\n- d\n\n- e\n\n", "f"])
        );
        for source in [
            "# Title\n\nFirst paragraph with `code` and **bold**.\n\nSecond [link](https://example.com).",
            "- one\n- two\n\n- loose three\n\n  continued\n\nAfter the list.",
            "1. first\n\n2. second\n\nText\n\n3) restarted",
            "```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\nafter\n\n~~~\nx\n\n~~~~\n\ny\n~~~\n\nend",
            "> quote\n\n> another quote\n\n> [!NOTE]\n> alert",
            "| a | b |\n| - | - |\n| 1 | 2 |\n\nAfter the table.",
            "para\n\n    indented code\n\nnext",
            "Setext\n===\n\nnext\n---\n\n- [ ] task\n- [x] done",
            "- a\n\n  ```\n  x\n\n```\n\nb",
            "- a\n\n  ```\n  x\n\ny\n\n  ```\n\nb",
            "intro\n\n★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\nafter\n\n`★ Insight ─────`\nsecond\n`─────`\n\nend",
        ] {
            for end in source
                .char_indices()
                .map(|(at, _)| at)
                .chain([source.len()])
            {
                let source = &source[..end];
                let blocks = split_markdown_blocks(source).expect("safe syntax");
                assert_eq!(blocks.concat(), source);
                // Compare actual parsed documents, not just a lossless string join.
                let joined: Vec<_> = blocks
                    .iter()
                    .flat_map(|s| Parser::new().markdown(s).blocks)
                    .collect();
                assert_eq!(joined, Parser::new().markdown(source).blocks, "{source:?}");
            }
        }
    }

    #[test]
    fn web_document_wide_syntax_keeps_the_full_parse_path() {
        for source in [
            "[ref]\n\n[ref]: /later",
            "a[^x]\n\n[^x]: note",
            "<details>\n\nbody\n\n</details>",
            ":::note\n\ncontent\n\n:::",
            "$$\na\n\nb\n$$",
            "a\r\n\r\nb",
            "text\u{2028}$$",
            "text\u{2029}<details>body</details>",
            "[label\u{2028}continued]: /target",
        ] {
            assert!(split_markdown_blocks(source).is_none());
        }
    }

    #[test]
    fn document_syntax_and_insight_callouts_retain_their_rendering_context() {
        for source in [
            "$$\n```\nmath, not a code row\n```\n$$",
            ":::note\n\n```\ncode in a directive\n```\n\n:::",
            "<details>\n\n```\ncode in HTML\n```\n\n</details>",
            "★ Insight ───\n\n```\ncode in a callout\n```\n\n───",
        ] {
            let message = replacement(source);
            assert_eq!(message.blocks.len(), 2);
            assert_eq!(message.blocks[1].kind, BlockKind::Document);
            assert_eq!(message.source_of(&message.blocks[1]), Some(source));
            assert_stream(source);
        }
    }

    #[test]
    fn insight_backticks_must_touch_the_rule_as_in_the_web_pattern() {
        assert_eq!(
            split_markdown_blocks("`★ Insight ─── `\ntext\n\nafter"),
            Some(vec!["`★ Insight ─── `\ntext\n\n", "after"])
        );
        assert_eq!(
            split_markdown_blocks("`★ Insight ───`\ntext\n`─── `\n\nafter"),
            Some(vec!["`★ Insight ───`\ntext\n`─── `\n\nafter"])
        );
    }

    #[test]
    fn five_hundred_code_lines_form_thirteen_chunks_with_exact_values_and_metadata() {
        let value = (0..500)
            .map(|n| format!("let v{n} = {n};"))
            .collect::<Vec<_>>()
            .join("\n");
        let source = format!("Before\n\n```rust title=example\n{value}\n```\n\nAfter");
        let blocks = replacement(&source);
        assert_document(&blocks);
        let chunks: Vec<_> = blocks
            .blocks
            .iter()
            .filter_map(|b| {
                if let BlockKind::Code(c) = &b.kind {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(chunks.len(), 13);
        assert_eq!(
            chunks.iter().map(|c| c.text.as_str()).collect::<String>(),
            value
        );
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.start_line, index * 40 + 1);
            assert_eq!(chunk.first, index == 0);
            assert_eq!(chunk.last, index == 12);
            assert_eq!(chunk.language.as_deref(), Some("rust"));
            assert_eq!(chunk.meta.as_deref(), Some("title=example"));
        }
    }

    #[test]
    fn streaming_reuses_completed_code_chunks_and_only_reflattens_the_tail() {
        let prefix = format!("Intro\n\n```rs\n{}", "x\n".repeat(100));
        let mut flattener = MessageFlattener::new("message");
        let before = snapshot(&flattener.replace("message", &prefix).document);
        let after = snapshot(&flattener.append("tail").document);
        assert_document(&after);
        assert_eq!(after.blocks, replacement(&format!("{prefix}tail")).blocks);
        for index in 0..4 {
            assert!(Rc::ptr_eq(&before.blocks[index], &after.blocks[index]));
        }
        assert!(!Rc::ptr_eq(
            before.blocks.last().expect("tail"),
            after.blocks.last().expect("tail")
        ));
        assert!(flattener.stream.as_ref().expect("stream").stable >= 3);
        assert_stream(&format!("```\n{}tail\n```\n\nend", "x\n".repeat(82)));
    }

    #[test]
    fn unicode_and_indented_code_preserve_utf16_ranges_and_chunk_values() {
        for newline in ["\n", "\r", "\r\n"] {
            let source = format!(
                "😀 café\n\n  ~~~~python extra\n{}  ~~~~\n\nfin",
                "  😀\n".repeat(85)
            )
            .replace('\n', newline);
            let blocks = replacement(&source);
            assert_document(&blocks);
            let code: String = blocks
                .blocks
                .iter()
                .filter_map(|b| {
                    if let BlockKind::Code(c) = &b.kind {
                        Some(c.text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(code, vec!["😀"; 85].join("\n"));
        }
    }

    #[test]
    fn plain_output_is_lossless_bounded_and_has_no_markdown_semantics() {
        let source = format!("```\n{}", "[ref]: 😀\r\n".repeat(100));
        let mut flattener = MessageFlattener::new("work:tool");
        let first = snapshot(&flattener.replace_output("work:tool", &source).document);
        assert_document(&first);
        let text: String = first
            .blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Output { text, .. } => {
                    assert!(text.lines().count() <= 40);
                    Some(text.as_str())
                }
                BlockKind::Header => None,
                _ => panic!("plain output only"),
            })
            .collect();
        assert_eq!(text, source);
        let next = snapshot(&flattener.append("tail").document);
        assert!(Rc::ptr_eq(&first.blocks[1], &next.blocks[1]));
        let full = snapshot(
            &MessageFlattener::new("work:tool")
                .replace_output("work:tool", &format!("{source}tail"))
                .document,
        );
        assert_eq!(next.blocks, full.blocks);
        assert!(matches!(
            flattener.replace("work:tool", "```rs\nx\n```").rows[1].kind,
            BlockKind::Code(_)
        ));
    }
}

#[cfg(test)]
mod package_tests {
    use crate as t3_ui;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/ui/tl_blocks_1.rs"
    ));
}
