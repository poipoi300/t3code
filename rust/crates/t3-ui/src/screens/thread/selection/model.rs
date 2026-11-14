//! Port of apps/web/src/components/chat/MessagesTimeline.tsx useSelectedTimelineRowIds and timelineRowSpanIndices.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use super::super::blocks::flatten::{
    Block, BlockDelta, BlockId, BlockKind, SourceRange, StreamBlocks,
};

/// An offset in a block's original source, in UTF-16 units, never rendered/native copy text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockPosition {
    pub block: BlockId,
    pub offset: usize,
}

/// Captures document identity as well as position, so a recycled cell cannot select an old reply.
#[derive(Clone, PartialEq, Eq)]
pub struct SelectionPoint {
    position: BlockPosition,
    source_utf16: usize,
    document: StreamBlocks,
}

impl SelectionPoint {
    pub fn position(&self) -> &BlockPosition {
        &self.position
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub anchor: BlockPosition,
    pub focus: BlockPosition,
}

/// TL-SELECT-2 consumes source and context, not a concatenation of mounted text widgets.
#[derive(Clone)]
pub struct SelectedSource {
    pub message_id: String,
    pub document: StreamBlocks,
    pub range: SourceRange,
}

impl SelectedSource {
    pub fn source(&self) -> Option<String> {
        self.document.source_range(self.range.bytes.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionError {
    UnknownMessage,
    UnknownBlock,
    UnselectableBlock,
    InvalidOffset,
    StaleDocument,
    DuplicateMessage,
}

impl std::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for SelectionError {}

/// Lives above the virtual list, one per thread. Mounting/unmounting does not alter this state.
// The web pins DOM containers; NSH rows may recycle, so retain source positions instead.
#[derive(Default)]
pub struct SelectionDocument {
    documents: BTreeMap<String, StreamBlocks>,
    order: Vec<String>,
    indices: BTreeMap<String, usize>,
    selection: Option<(SelectionPoint, SelectionPoint)>,
}

impl SelectionDocument {
    /// Apply the flattener's append/replace result without reading source or visiting settled blocks.
    pub fn apply_delta(
        &mut self,
        message_id: &str,
        delta: &BlockDelta,
    ) -> Result<(), SelectionError> {
        self.retain_document(message_id, delta.document.clone())
    }

    /// Attach TL-BLOCKS-3's existing document; do not create a second flattener for selection.
    pub fn retain_document(
        &mut self,
        message_id: &str,
        document: StreamBlocks,
    ) -> Result<(), SelectionError> {
        if document
            .block(0)
            .is_none_or(|b| b.id.message_id != message_id)
        {
            return Err(SelectionError::UnknownMessage);
        }
        let replaced = self
            .documents
            .get(message_id)
            .is_some_and(|d| d != &document);
        if replaced && self.selection_touches(message_id) {
            self.clear();
        }
        if !self.documents.contains_key(message_id) {
            self.indices.insert(message_id.into(), self.order.len());
            self.order.push(message_id.into());
        }
        self.documents.insert(message_id.into(), document);
        Ok(())
    }

    /// Projection order, not BlockId's lexical order. O(messages), only on a structural change.
    /// Absent messages are retired; an off-screen message is still present in this order.
    pub fn set_order(&mut self, order: &[String]) -> Result<(), SelectionError> {
        let mut seen = BTreeSet::new();
        for id in order {
            if !self.documents.contains_key(id) {
                return Err(SelectionError::UnknownMessage);
            }
            if !seen.insert(id) {
                return Err(SelectionError::DuplicateMessage);
            }
        }
        if self.selection.as_ref().is_some_and(|(a, f)| {
            !seen.contains(&a.position.block.message_id)
                || !seen.contains(&f.position.block.message_id)
        }) {
            self.clear();
        }
        self.documents.retain(|id, _| seen.contains(id));
        self.order = order.to_vec();
        self.indices = order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        Ok(())
    }

    pub fn document(&self, message_id: &str) -> Option<StreamBlocks> {
        self.documents.get(message_id).cloned()
    }

    /// Call only with source-mapped endpoints; NSH's `{text, start}` is not such a mapping.
    pub fn point(&self, position: BlockPosition) -> Result<SelectionPoint, SelectionError> {
        let document = self
            .documents
            .get(&position.block.message_id)
            .ok_or(SelectionError::UnknownMessage)?;
        let block = document
            .block(position.block.index)
            .filter(|b| b.id == position.block)
            .ok_or(SelectionError::UnknownBlock)?;
        if !selectable(&block) {
            return Err(SelectionError::UnselectableBlock);
        }
        let source_utf16 = block
            .source
            .utf16
            .start
            .checked_add(position.offset)
            .ok_or(SelectionError::InvalidOffset)?;
        if source_utf16 > block.source.utf16.end {
            return Err(SelectionError::InvalidOffset);
        }
        // Endpoint validation is user-input work, never part of apply_delta.
        byte_at(document, &block.source, source_utf16)?;
        Ok(SelectionPoint {
            position,
            source_utf16,
            document: document.clone(),
        })
    }

    pub fn select(
        &mut self,
        anchor: SelectionPoint,
        focus: SelectionPoint,
    ) -> Result<(), SelectionError> {
        for point in [&anchor, &focus] {
            if self.documents.get(&point.position.block.message_id) != Some(&point.document) {
                return Err(SelectionError::StaleDocument);
            }
        }
        self.selection = Some((anchor, focus));
        Ok(())
    }

    pub fn clear(&mut self) {
        self.selection = None;
    }

    /// Snapshot the whole retained thread, including blocks that have never mounted.
    pub fn select_all(&mut self) -> bool {
        let first = self.order.iter().find_map(|id| self.edge(id, false));
        let last = self.order.iter().rev().find_map(|id| self.edge(id, true));
        self.selection = first.zip(last);
        self.has_selection()
    }

    pub fn selection(&self) -> Option<SelectionRange> {
        let (anchor, focus) = self.selection.as_ref()?;
        Some(SelectionRange {
            anchor: self.resolve(anchor)?,
            focus: self.resolve(focus)?,
        })
    }

    pub fn has_selection(&self) -> bool {
        self.bounds().is_some_and(|(a, f)| a != f)
    }

    /// Source-relative highlight for a remounted block; the renderer maps source to its glyphs.
    pub fn block_range(&self, id: &BlockId) -> Option<Range<usize>> {
        let (start, end) = self.bounds()?;
        let at = *self.indices.get(&id.message_id)?;
        if at < start.0 || at > end.0 || start == end {
            return None;
        }
        let block = self.documents.get(&id.message_id)?.block(id.index)?;
        if !selectable(&block) {
            return None;
        }
        let lo = if at == start.0 {
            start.1.max(block.source.utf16.start)
        } else {
            block.source.utf16.start
        };
        let hi = if at == end.0 {
            end.1.min(block.source.utf16.end)
        } else {
            block.source.utf16.end
        };
        (lo < hi).then(|| lo - block.source.utf16.start..hi - block.source.utf16.start)
    }

    /// In document order. Full source is materialized by the serializer only during Copy.
    pub fn selected_sources(&self) -> Result<Vec<SelectedSource>, SelectionError> {
        let Some((start, end)) = self.bounds().filter(|(a, f)| a != f) else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        for at in start.0..=end.0 {
            let id = &self.order[at];
            let document = &self.documents[id];
            let utf16_end = document
                .block_count()
                .checked_sub(1)
                .and_then(|i| document.block(i))
                .map_or(0, |b| b.source.utf16.end);
            let lo = if at == start.0 { start.1 } else { 0 };
            let hi = if at == end.0 { end.1 } else { utf16_end };
            if lo == hi {
                continue;
            }
            let bytes = self.source_byte(document, lo)?..self.source_byte(document, hi)?;
            result.push(SelectedSource {
                message_id: id.clone(),
                document: document.clone(),
                range: SourceRange {
                    bytes,
                    utf16: lo..hi,
                },
            });
        }
        Ok(result)
    }

    fn source_byte(&self, document: &StreamBlocks, utf16: usize) -> Result<usize, SelectionError> {
        let block = locate(document, utf16).ok_or(SelectionError::InvalidOffset)?;
        byte_at(document, &block.source, utf16)
    }

    fn selection_touches(&self, id: &str) -> bool {
        self.selection.as_ref().is_some_and(|(a, f)| {
            a.position.block.message_id == id || f.position.block.message_id == id
        })
    }

    fn edge(&self, id: &str, last: bool) -> Option<SelectionPoint> {
        let document = self.documents.get(id)?;
        let mut indices = 0..document.block_count();
        let block = if last {
            indices
                .rev()
                .find_map(|i| document.block(i).filter(|b| selectable(b)))
        } else {
            indices.find_map(|i| document.block(i).filter(|b| selectable(b)))
        }?;
        Some(SelectionPoint {
            position: BlockPosition {
                block: block.id.clone(),
                offset: if last { block.source.utf16.len() } else { 0 },
            },
            source_utf16: if last {
                block.source.utf16.end
            } else {
                block.source.utf16.start
            },
            document: document.clone(),
        })
    }

    fn resolve(&self, point: &SelectionPoint) -> Option<BlockPosition> {
        let document = self.documents.get(&point.position.block.message_id)?;
        if document != &point.document {
            return None;
        }
        let block = document
            .block(point.position.block.index)
            .filter(|b| selectable(b) && b.source.utf16.contains(&point.source_utf16))
            .or_else(|| locate(document, point.source_utf16).filter(|b| selectable(b)))?;
        Some(BlockPosition {
            block: block.id.clone(),
            offset: point.source_utf16 - block.source.utf16.start,
        })
    }

    fn bounds(&self) -> Option<((usize, usize), (usize, usize))> {
        let (anchor, focus) = self.selection.as_ref()?;
        self.resolve(anchor)?;
        self.resolve(focus)?;
        let a = (
            *self.indices.get(&anchor.position.block.message_id)?,
            anchor.source_utf16,
        );
        let f = (
            *self.indices.get(&focus.position.block.message_id)?,
            focus.source_utf16,
        );
        Some(if a <= f { (a, f) } else { (f, a) })
    }
}

fn selectable(block: &Block) -> bool {
    !matches!(block.kind, BlockKind::Header | BlockKind::Definition { .. })
        && !block.source.utf16.is_empty()
}

// Source offsets survive a streaming tail being repartitioned into different block indices.
fn locate(document: &StreamBlocks, utf16: usize) -> Option<std::rc::Rc<Block>> {
    let (mut lo, mut hi) = (0, document.block_count());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if document.block(mid)?.source.utf16.end < utf16 {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let block = document.block(lo)?;
    if matches!(block.kind, BlockKind::Header) && block.source.utf16.end == utf16 {
        return document
            .block(lo + 1)
            .filter(|b| b.source.utf16.start == utf16);
    }
    (block.source.utf16.start <= utf16).then_some(block)
}

fn byte_at(
    document: &StreamBlocks,
    range: &SourceRange,
    utf16: usize,
) -> Result<usize, SelectionError> {
    if utf16 == range.utf16.start {
        return Ok(range.bytes.start);
    }
    if utf16 == range.utf16.end {
        return Ok(range.bytes.end);
    }
    let source = document
        .source_range(range.bytes.clone())
        .ok_or(SelectionError::InvalidOffset)?;
    let mut at = range.utf16.start;
    for (byte, ch) in source.char_indices() {
        if at == utf16 {
            return Ok(range.bytes.start + byte);
        }
        at += ch.len_utf16();
        if at > utf16 {
            break;
        }
    }
    Err(SelectionError::InvalidOffset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::thread::blocks::flatten::MessageFlattener;

    fn register(model: &mut SelectionDocument, id: &str, source: &str) -> MessageFlattener {
        let mut flattener = MessageFlattener::new(id);
        model
            .apply_delta(id, &flattener.replace(id, source))
            .expect("document");
        flattener
    }

    fn point(model: &SelectionDocument, id: &str, index: usize, offset: usize) -> SelectionPoint {
        model
            .point(BlockPosition {
                block: BlockId {
                    message_id: id.into(),
                    index,
                },
                offset,
            })
            .expect("source point")
    }

    fn sources(model: &SelectionDocument) -> Vec<String> {
        model
            .selected_sources()
            .expect("source ranges")
            .iter()
            .map(|s| s.source().expect("retained source"))
            .collect()
    }

    #[test]
    fn reversed_selection_covers_unmounted_intermediate_rows_in_projection_order() {
        let mut model = SelectionDocument::default();
        register(&mut model, "z-first", "First 🦀");
        register(&mut model, "a-middle", "- A\n- B");
        register(&mut model, "b-last", "Last");
        let anchor = point(&model, "b-last", 1, 2);
        let focus = point(&model, "z-first", 1, 6);
        model.select(anchor, focus).expect("backwards drag");
        assert_eq!(sources(&model), ["🦀", "- A\n- B", "La"]);
        assert_eq!(
            model.block_range(&BlockId {
                message_id: "a-middle".into(),
                index: 1
            }),
            Some(0..7)
        );
        assert_eq!(
            model
                .selection()
                .expect("direction")
                .anchor
                .block
                .message_id,
            "b-last"
        );
        model
            .set_order(&["b-last".into(), "a-middle".into(), "z-first".into()])
            .expect("reorder");
        assert_eq!(sources(&model), ["st", "- A\n- B", "First "]);
    }

    #[test]
    fn utf16_endpoints_reject_surrogate_halves_and_out_of_range_without_changing_selection() {
        let mut model = SelectionDocument::default();
        register(&mut model, "unicode", "é🦀e\u{301}\r\n終");
        model
            .select(
                point(&model, "unicode", 1, 1),
                point(&model, "unicode", 1, 5),
            )
            .expect("unicode selection");
        assert_eq!(sources(&model), ["🦀e\u{301}"]);
        for offset in [2, 100, usize::MAX] {
            assert!(matches!(
                model.point(BlockPosition {
                    block: BlockId {
                        message_id: "unicode".into(),
                        index: 1
                    },
                    offset
                }),
                Err(SelectionError::InvalidOffset)
            ));
        }
        assert_eq!(sources(&model), ["🦀e\u{301}"]);
        model
            .select(
                point(&model, "unicode", 1, 3),
                point(&model, "unicode", 1, 3),
            )
            .expect("caret");
        assert!(!model.has_selection());
        assert!(sources(&model).is_empty());
    }

    #[test]
    fn select_all_covers_two_thousand_retained_messages_and_all_code_chunks() {
        let mut model = SelectionDocument::default();
        let fence = format!(
            "Before\n\n```rust\n{}```\n\nAfter",
            "let 🦀 = 1;\n".repeat(500)
        );
        let mut expected = Vec::new();
        for index in 0..2000 {
            let source = if index == 1000 {
                fence.clone()
            } else {
                format!("Message {index}")
            };
            register(&mut model, &format!("message-{index}"), &source);
            expected.push(source);
        }
        assert!(model.select_all());
        assert_eq!(sources(&model), expected);
        let document = model.document("message-1000").expect("off-screen document");
        let code = (0..document.block_count())
            .filter_map(|i| document.block(i))
            .filter(|b| matches!(b.kind, BlockKind::Code(_)))
            .collect::<Vec<_>>();
        assert_eq!(code.len(), 13);
        for block in code {
            assert_eq!(
                model.block_range(&block.id),
                Some(0..block.source.utf16.len())
            );
        }
        model.clear();
        assert!(!model.has_selection());
        assert!(sources(&model).is_empty());
    }

    #[test]
    fn appends_repartition_the_tail_without_moving_the_selected_source_or_extending_select_all() {
        let mut model = SelectionDocument::default();
        let mut flattener = register(&mut model, "stream", "Before\n\n```rust\nlet a = 1;\n");
        assert!(model.select_all());
        let selected = sources(&model);
        for tail in ["let b = 2;\n", "```\n\n", "After 🦀"] {
            model
                .apply_delta("stream", &flattener.append(tail))
                .expect("append");
            assert_eq!(sources(&model), selected);
        }
        model
            .apply_delta("stream", &flattener.finish().expect("finish"))
            .expect("finish delta");
        assert_eq!(sources(&model), selected);
        assert!(model.select_all());
        assert_eq!(
            sources(&model),
            ["Before\n\n```rust\nlet a = 1;\nlet b = 2;\n```\n\nAfter 🦀"]
        );
    }

    #[test]
    fn replace_and_remove_invalidate_endpoints_and_reject_a_recycled_cells_old_document() {
        let mut model = SelectionDocument::default();
        let mut flattener = register(&mut model, "same-id", "Original");
        let old = point(&model, "same-id", 1, 0);
        assert!(model.select_all());
        model
            .apply_delta("same-id", &flattener.replace("same-id", "Replacement"))
            .expect("replace");
        assert!(!model.has_selection());
        assert_eq!(
            model.select(old, point(&model, "same-id", 1, 4)),
            Err(SelectionError::StaleDocument)
        );
        assert!(sources(&model).is_empty());
        register(&mut model, "other", "Other");
        assert!(model.select_all());
        model.set_order(&["other".into()]).expect("remove");
        assert!(!model.has_selection());
        assert!(model.document("same-id").is_none());
        assert!(model.select_all());
        assert_eq!(sources(&model), ["Other"]);
    }

    #[test]
    fn empty_metadata_and_unknown_inputs_do_not_fabricate_selectable_content() {
        let mut model = SelectionDocument::default();
        assert!(!model.select_all());
        register(&mut model, "empty", "");
        register(&mut model, "definitions", "[ref]: https://example.test\n");
        assert!(!model.select_all());
        assert!(matches!(
            model.point(BlockPosition {
                block: BlockId {
                    message_id: "empty".into(),
                    index: 0
                },
                offset: 0
            }),
            Err(SelectionError::UnselectableBlock)
        ));
        let mut foreign = MessageFlattener::new("foreign");
        assert_eq!(
            model.apply_delta("empty", &foreign.append("wrong reply")),
            Err(SelectionError::UnknownMessage)
        );
        assert_eq!(
            model.set_order(&["missing".into()]),
            Err(SelectionError::UnknownMessage)
        );
        assert_eq!(
            model.set_order(&["empty".into(), "empty".into()]),
            Err(SelectionError::DuplicateMessage)
        );
        assert!(model.document("definitions").is_some());
        assert!(sources(&model).is_empty());
    }
}
