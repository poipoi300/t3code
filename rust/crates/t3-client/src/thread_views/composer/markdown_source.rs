//! Selection-driven Markdown source rewrites over editor-independent document trees.
use super::editor::{
    children, marks, name, node_size, serialize_document, splice_inline, textblocks,
    transform_marks,
};
use crate::thread_views::{
    document::{Selection, utf16_byte_offset, utf16_len},
    rich_text::inline_markdown_unit_marks,
};
use serde_json::{Value, json};

pub const SOURCE_MARK: &str = "markdownSource";
const STYLE_MARKS: [&str; 4] = ["bold", "italic", "strike", "code"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub from: usize,
    pub to: usize,
}
impl Range {
    fn touches(self, selection: Selection) -> bool {
        if selection.collapsed() {
            selection.head >= self.from && selection.head <= self.to
        } else {
            selection.from() < self.to && selection.to() > self.from
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub document: Value,
    pub selection: Selection,
    pub revealed: Vec<Range>,
}

fn has_source(node: &Value) -> bool {
    node.get("marks")
        .and_then(Value::as_array)
        .is_some_and(|marks| marks.iter().any(|mark| mark["type"] == SOURCE_MARK))
}
fn set_source(node: &mut Value) {
    let mut kept = node
        .get("marks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    kept.retain(|mark| {
        !STYLE_MARKS.contains(&mark["type"].as_str().unwrap_or_default())
            && mark["type"] != SOURCE_MARK
    });
    kept.push(json!({"type": SOURCE_MARK}));
    node["marks"] = kept.into();
}
fn source_text(text: &str) -> Value {
    json!({"type":"text", "text":text, "marks":[{"type":SOURCE_MARK}]})
}
fn slice(text: &str, from: usize, to: usize) -> Option<&str> {
    text.get(utf16_byte_offset(text, from)?..utf16_byte_offset(text, to)?)
}

pub fn source_regions(document: &Value) -> Vec<Range> {
    let mut result = Vec::<Range>::new();
    for (start, block) in textblocks(document) {
        let mut pos = start;
        let mut previous = false;
        for child in children(&block) {
            let end = pos + node_size(child);
            if has_source(child) {
                if previous {
                    if let Some(last) = result.last_mut() {
                        last.to = end;
                    }
                } else {
                    result.push(Range { from: pos, to: end });
                }
            }
            previous = has_source(child);
            pos = end;
        }
    }
    result
}

fn adjacent_delimiters(document: &Value, range: Range) -> Range {
    let delimiter = |unit: &u16| matches!(*unit, 42 | 95 | 126 | 96);
    let mut result = range;
    for (start, block) in textblocks(document) {
        let mut pos = start;
        for child in children(&block) {
            let end = pos + node_size(child);
            if name(child) == "text"
                && child
                    .get("marks")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty)
            {
                let units: Vec<_> = child["text"]
                    .as_str()
                    .unwrap_or_default()
                    .encode_utf16()
                    .collect();
                if end == range.from {
                    let mut count = units
                        .iter()
                        .rev()
                        .take_while(|unit| delimiter(unit))
                        .count();
                    let escapes = units[..units.len() - count]
                        .iter()
                        .rev()
                        .take_while(|unit| **unit == 92)
                        .count();
                    if count > 0 && escapes % 2 == 1 {
                        count -= 1;
                    }
                    result.from -= count;
                }
                if pos == range.to {
                    result.to += units.iter().take_while(|unit| delimiter(unit)).count();
                }
            }
            pos = end;
        }
    }
    result
}

#[derive(Clone)]
struct Marker {
    at: usize,
    closing: bool,
    text: String,
}
struct StyledRange {
    range: Range,
    escaped: bool,
    markers: Vec<Marker>,
}
fn styled_ranges(document: &Value) -> Vec<StyledRange> {
    let map = serialize_document(document);
    let mut result = vec![];
    let mut current: Option<StyledRange> = None;
    let mut opened = 0usize;
    for run in &map.runs {
        if run.node_name.as_deref() == Some("codeBlock") {
            continue;
        }
        if run.open_len > 0 {
            let before = slice(&map.value, 0, run.md_start).unwrap_or_default();
            let escaped = before.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1;
            let range = current.get_or_insert_with(|| StyledRange {
                range: Range {
                    from: run.pm_pos,
                    to: run.pm_pos,
                },
                escaped,
                markers: vec![],
            });
            range.markers.push(Marker {
                at: run.pm_pos,
                closing: false,
                text: slice(&map.value, run.md_start, run.md_start + run.open_len)
                    .unwrap_or_default()
                    .into(),
            });
        }
        let Some(range) = current.as_mut() else {
            continue;
        };
        range.range.to = run.pm_pos + run.doc_len;
        if run.close_len > 0 {
            let end = run.md_start + run.md_len;
            range.markers.push(Marker {
                at: range.range.to,
                closing: true,
                text: slice(&map.value, end - run.close_len, end)
                    .unwrap_or_default()
                    .into(),
            });
        }
        opened = (opened + run.open_len).saturating_sub(run.close_len);
        if opened == 0
            && let Some(range) = current.take()
        {
            result.push(range);
        }
    }
    result
}

fn faithful(document: &Value, styled: &StyledRange) -> bool {
    if styled.escaped || adjacent_delimiters(document, styled.range) != styled.range {
        return false;
    }
    let mut markers = styled.markers.clone();
    markers.sort_by_key(|marker| (marker.at, !marker.closing));
    let mut next = 0;
    let mut source = String::new();
    let mut expected = vec![];
    let mut append = |pos, text: &str, styles: Option<Vec<_>>| {
        while next < markers.len() && markers[next].at <= pos {
            let marker = &markers[next];
            source.push_str(&marker.text);
            expected.extend(std::iter::repeat_n(None, utf16_len(&marker.text)));
            next += 1;
        }
        source.push_str(text);
        expected.extend(std::iter::repeat_n(styles, utf16_len(text)));
    };
    for (start, block) in textblocks(document) {
        let mut pos = start;
        for child in children(&block) {
            let from = pos.max(styled.range.from);
            let to = (pos + node_size(child)).min(styled.range.to);
            if from < to {
                let value = if name(child) == "text" {
                    slice(
                        child["text"].as_str().unwrap_or_default(),
                        from - pos,
                        to - pos,
                    )
                    .unwrap_or_default()
                } else {
                    "\u{fffc}"
                };
                append(from, value, Some(marks(child)));
            }
            pos += node_size(child);
        }
    }
    append(styled.range.to, "", None);
    let parsed = inline_markdown_unit_marks(&source);
    parsed.len() == expected.len()
        && parsed
            .iter()
            .zip(expected)
            .all(|(left, right)| match (left, right) {
                (Some(left), Some(right)) => {
                    left.len() == right.len() && left.iter().all(|mark| right.contains(mark))
                }
                (None, None) => true,
                _ => false,
            })
}

/// A typed caret stays inside its active markers; an arriving caret stays outside.
pub fn reveal(document: &Value, selection: Selection, typed: bool, takes_before: bool) -> Rewrite {
    let ranges: Vec<_> = styled_ranges(document)
        .into_iter()
        .filter(|range| range.range.touches(selection) && faithful(document, range))
        .collect();
    let mut next = document.clone();
    let mut insertions = vec![];
    for styled in ranges.iter().rev() {
        transform_marks(&mut next, styled.range.from, styled.range.to, set_source);
        let mut markers = styled.markers.clone();
        markers.sort_by_key(|marker| (std::cmp::Reverse(marker.at), marker.closing));
        let mut length = 0;
        for marker in markers {
            let size = utf16_len(&marker.text);
            splice_inline(
                &mut next,
                marker.at,
                marker.at,
                &[source_text(&marker.text)],
            );
            insertions.push((marker.at, size, marker.closing));
            length += size;
        }
        transform_marks(
            &mut next,
            styled.range.from,
            styled.range.to + length,
            set_source,
        );
    }
    let map = |pos| {
        pos + insertions
            .iter()
            .filter(|(at, _, closing)| {
                let inside = typed
                    && selection.collapsed()
                    && if *closing {
                        takes_before
                    } else {
                        !takes_before
                    };
                *at < pos || (*at == pos && *closing != inside)
            })
            .map(|(_, length, _)| length)
            .sum::<usize>()
    };
    let revealed = ranges
        .iter()
        .map(|range| Range {
            from: range.range.from
                + insertions
                    .iter()
                    .filter(|(at, _, _)| *at < range.range.from)
                    .map(|(_, length, _)| length)
                    .sum::<usize>(),
            to: range.range.to
                + insertions
                    .iter()
                    .filter(|(at, _, _)| *at <= range.range.to)
                    .map(|(_, length, _)| length)
                    .sum::<usize>(),
        })
        .collect();
    Rewrite {
        document: next,
        selection: Selection {
            anchor: map(selection.anchor),
            head: map(selection.head),
        },
        revealed,
    }
}

fn region_content(document: &Value, range: Range) -> Option<Vec<Value>> {
    let (start, block) = textblocks(document)
        .into_iter()
        .find(|(start, block)| range.from >= *start && range.to <= *start + node_size(block) - 2)?;
    let mut result = vec![];
    let mut pos = start;
    for child in children(&block) {
        let end = pos + node_size(child);
        let from = range.from.max(pos);
        let to = range.to.min(end);
        if from < to {
            let mut clipped = child.clone();
            if name(child) == "text" {
                clipped["text"] = json!(slice(child["text"].as_str()?, from - pos, to - pos)?);
            }
            result.push(clipped);
        }
        pos = end;
    }
    Some(result)
}

fn collapse_region(document: &mut Value, range: Range) -> Vec<Range> {
    let Some(content) = region_content(document, range) else {
        return vec![];
    };
    let source: String = content
        .iter()
        .map(|node| {
            if name(node) == "text" {
                node["text"].as_str().unwrap_or_default()
            } else {
                "\u{fffc}"
            }
        })
        .collect();
    let units = inline_markdown_unit_marks(&source);
    let mut offset = 0;
    let mut replacement = vec![];
    let mut deleted = vec![];
    for node in content {
        let len = node_size(&node);
        let mut start = 0;
        while start < len {
            let mut end = start + 1;
            while end < len && units[offset + end] == units[offset + start] {
                end += 1;
            }
            if let Some(styles) = &units[offset + start] {
                let mut part = node.clone();
                if name(&node) == "text" {
                    part["text"] = json!(
                        slice(node["text"].as_str().unwrap_or_default(), start, end)
                            .unwrap_or_default()
                    );
                }
                let mut node_marks = part
                    .get("marks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                node_marks.retain(|mark| mark["type"] != SOURCE_MARK);
                for style in styles {
                    let mark = match style {
                        crate::thread_views::rich_text::Mark::Bold => "bold",
                        crate::thread_views::rich_text::Mark::Italic => "italic",
                        crate::thread_views::rich_text::Mark::Strike => "strike",
                        crate::thread_views::rich_text::Mark::Code => "code",
                    };
                    if !node_marks.iter().any(|value| value["type"] == mark) {
                        node_marks.push(json!({"type":mark}));
                    }
                }
                if node_marks.is_empty() {
                    if let Some(object) = part.as_object_mut() {
                        object.remove("marks");
                    }
                } else {
                    part["marks"] = node_marks.into();
                }
                replacement.push(part);
            } else {
                deleted.push(Range {
                    from: range.from + offset + start,
                    to: range.from + offset + end,
                });
            }
            start = end;
        }
        offset += len;
    }
    splice_inline(document, range.from, range.to, &replacement);
    deleted
}

pub fn collapse(document: &Value, selection: Selection, keep_touched: bool) -> Rewrite {
    let mut regions = Vec::<Range>::new();
    for range in source_regions(document) {
        let range = adjacent_delimiters(document, range);
        if let Some(last) = regions.last_mut().filter(|last| range.from <= last.to) {
            last.to = last.to.max(range.to);
        } else {
            regions.push(range);
        }
    }
    let mut next = document.clone();
    let mut deleted = vec![];
    let mut kept = vec![];
    for range in regions.into_iter().rev() {
        if keep_touched && range.touches(selection) {
            transform_marks(&mut next, range.from, range.to, set_source);
            kept.push(range);
        } else {
            deleted.extend(collapse_region(&mut next, range));
        }
    }
    let map = |pos: usize| {
        pos - deleted
            .iter()
            .map(|range| pos.saturating_sub(range.from).min(range.to - range.from))
            .sum::<usize>()
    };
    kept.reverse();
    Rewrite {
        document: next,
        selection: Selection {
            anchor: map(selection.anchor),
            head: map(selection.head),
        },
        revealed: kept
            .into_iter()
            .map(|range| Range {
                from: map(range.from),
                to: map(range.to),
            })
            .collect(),
    }
}

pub fn normalize(
    document: &Value,
    selection: Selection,
    typed: bool,
    takes_before: bool,
) -> Rewrite {
    let collapsed = collapse(document, selection, true);
    let mut next = reveal(
        &collapsed.document,
        collapsed.selection,
        typed,
        takes_before,
    );
    next.revealed = source_regions(&next.document);
    next
}

#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    document: Value,
    selection: Selection,
}

/// History stores collapsed trees, so caret-only source rewrites cannot invalidate redo steps.
pub struct SourceEditor {
    view: Rewrite,
    history: crate::thread_views::undo::History<Snapshot>,
}
impl SourceEditor {
    pub fn new(document: Value, selection: Selection) -> Self {
        Self {
            view: normalize(&document, selection, false, true),
            history: Default::default(),
        }
    }
    pub fn view(&self) -> &Rewrite {
        &self.view
    }
    pub fn select(&mut self, selection: Selection, takes_before: bool) {
        self.view = normalize(&self.view.document, selection, false, takes_before);
    }
    pub fn apply(
        &mut self,
        document: Value,
        selection: Selection,
        transaction: crate::thread_views::undo::Transaction<'_>,
        now_ms: u64,
        takes_before: bool,
    ) {
        let before = collapse(&self.view.document, self.view.selection, false);
        self.history.record(
            &Snapshot {
                document: before.document,
                selection: before.selection,
            },
            transaction,
            now_ms,
        );
        self.view = if transaction.composition {
            Rewrite {
                revealed: source_regions(&document),
                document,
                selection,
            }
        } else {
            normalize(&document, selection, transaction.changed, takes_before)
        };
    }
    pub fn finish_composition(&mut self, takes_before: bool) {
        self.view = normalize(&self.view.document, self.view.selection, true, takes_before);
    }
    fn canonical(&self) -> Snapshot {
        let collapsed = collapse(&self.view.document, self.view.selection, false);
        Snapshot {
            document: collapsed.document,
            selection: collapsed.selection,
        }
    }
    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.history.undo(&self.canonical()) else {
            return false;
        };
        self.view = normalize(&snapshot.document, snapshot.selection, false, true);
        true
    }
    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.history.redo(&self.canonical()) else {
            return false;
        };
        self.view = normalize(&snapshot.document, snapshot.selection, false, true);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread_views::{
        composer::editor::{BuildOptions, build_document},
        rich_text::Mark,
    };
    fn build(source: &str) -> Value {
        build_document(
            source,
            BuildOptions::default(),
            |name| Some((name.into(), None)),
            |_, _| true,
        )
    }
    #[test]
    fn reveal_collapse_keeps_markdown_atoms_and_unicode_positions() {
        for source in [
            "**hello** tail",
            "*😀 hello* end",
            "~~**nested**~~ after",
            "**@src/main.rs** rest",
            "**a** and *b*",
        ] {
            let document = build(source);
            let shown = reveal(&document, Selection::caret(2), false, true);
            assert!(!shown.revealed.is_empty(), "{source}");
            assert_eq!(serialize_document(&shown.document).value, source);
            let collapsed = collapse(&shown.document, shown.selection, false);
            assert_eq!(serialize_document(&collapsed.document).value, source);
            assert!(source_regions(&collapsed.document).is_empty());
            assert_eq!(collapsed.selection, Selection::caret(2));
        }
    }
    #[test]
    fn arriving_and_typing_carets_choose_opposite_marker_sides() {
        let document = build("**word**");
        assert_eq!(
            reveal(&document, Selection::caret(1), false, false)
                .selection
                .head,
            1
        );
        assert_eq!(
            reveal(&document, Selection::caret(1), true, false)
                .selection
                .head,
            3
        );
        assert_eq!(
            reveal(&document, Selection::caret(5), false, true)
                .selection
                .head,
            9
        );
        assert_eq!(
            reveal(&document, Selection::caret(5), true, true)
                .selection
                .head,
            7
        );
    }
    #[test]
    fn leaving_source_parses_edits_and_adjacent_retyped_markers() {
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[source_text("**word"),{"type":"text","text":"** tail"}]}]});
        let result = normalize(&document, Selection::caret(13), false, true);
        assert_eq!(serialize_document(&result.document).value, "**word** tail");
        assert_eq!(
            marks(&children(&children(&result.document)[0])[0]),
            vec![Mark::Bold]
        );
        assert!(source_regions(&result.document).is_empty());
        assert_eq!(result.selection.head, 9);
    }
    #[test]
    fn non_faithful_delimiters_and_code_blocks_stay_styled() {
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"\\"},{"type":"text","text":"word","marks":[{"type":"bold"}]}]}]});
        assert!(
            reveal(&document, Selection::caret(3), false, true)
                .revealed
                .is_empty()
        );
        let code = build("```\n**word**\n```");
        assert!(
            reveal(&code, Selection::caret(4), false, true)
                .revealed
                .is_empty()
        );
    }
    #[test]
    fn repeated_selection_normalization_is_stable_and_backward_ranges_survive() {
        let document = build("**word** tail");
        let selection = Selection { anchor: 4, head: 2 };
        let shown = normalize(&document, selection, false, true);
        let again = normalize(&shown.document, shown.selection, false, true);
        assert_eq!(shown, again);
        let restored = collapse(&shown.document, shown.selection, false);
        assert_eq!(restored.selection, selection);
    }
    #[test]
    fn source_navigation_after_undo_preserves_redo_and_atom_payloads() {
        use crate::thread_views::undo::{Replace, Transaction};
        let document = build("**one @file** tail");
        let mut editor = SourceEditor::new(document, Selection::caret(2));
        let mut changed = editor.view().document.clone();
        assert!(splice_inline(&mut changed, 4, 4, &[source_text("new ")]));
        editor.apply(
            changed,
            Selection::caret(8),
            Transaction {
                changed: true,
                steps: &[Some(Replace {
                    deleted: 0,
                    inserted: 4,
                })],
                ..Default::default()
            },
            0,
            true,
        );
        let written = serialize_document(&editor.view().document).value;
        assert!(written.contains("new "));
        assert!(editor.undo());
        assert_eq!(
            serialize_document(&editor.view().document).value,
            "**one @file** tail"
        );
        let end = node_size(&editor.view().document) - 1;
        editor.select(Selection::caret(end), true);
        assert!(editor.redo());
        assert_eq!(serialize_document(&editor.view().document).value, written);
        assert!(
            serialize_document(&editor.view().document)
                .runs
                .iter()
                .any(|run| run.node_name.as_deref() == Some("composer-mention"))
        );
        assert!(!editor.redo());
    }
    #[test]
    fn composition_defers_source_rewrites_until_commit() {
        use crate::thread_views::undo::Transaction;
        let mut editor = SourceEditor::new(build("text"), Selection::caret(2));
        let composed = build("**word**");
        editor.apply(
            composed.clone(),
            Selection::caret(3),
            Transaction {
                changed: true,
                composition: true,
                ..Default::default()
            },
            0,
            true,
        );
        assert_eq!(editor.view().document, composed);
        editor.finish_composition(true);
        assert!(!editor.view().revealed.is_empty());
        assert!(editor.undo());
        assert_eq!(serialize_document(&editor.view().document).value, "text");
    }
}
