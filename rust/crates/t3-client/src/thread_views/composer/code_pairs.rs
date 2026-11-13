//! Only explicitly typed backtick pairs opt into caret-triggered code conversion.
use super::editor::{
    children, marks, name, node_size, splice_inline, text_node, textblocks, transform_marks,
};
use crate::thread_views::{
    document::{Selection, utf16_byte_offset, utf16_len},
    rich_text::Mark,
};
use serde_json::{Value, json};

#[derive(Default)]
pub struct CodePairs {
    pairs: Vec<(usize, usize)>,
}
pub struct Rewrite {
    pub document: Value,
    pub selection: Selection,
    pub stored_marks: Vec<Mark>,
}
fn block_at(doc: &Value, pos: usize) -> Option<(usize, Value)> {
    textblocks(doc).into_iter().find(|(start, block)| {
        pos >= *start && pos <= start + node_size(block) - 2 && name(block) != "codeBlock"
    })
}
fn content_text(block: &Value) -> String {
    children(block)
        .iter()
        .map(|node| {
            if name(node) == "text" {
                node["text"].as_str().unwrap_or_default()
            } else {
                "\n"
            }
        })
        .collect()
}
fn before_node(block: &Value, offset: usize) -> Option<(&Value, &str)> {
    let mut pos = 0;
    for child in children(block) {
        let end = pos + node_size(child);
        if offset > pos && offset <= end {
            let text = child["text"].as_str()?;
            return Some((child, &text[..utf16_byte_offset(text, offset - pos)?]));
        }
        pos = end;
    }
    None
}
fn closes(block: &Value, offset: usize) -> bool {
    let Some((_, before)) = before_node(block, offset) else {
        return false;
    };
    if !before.ends_with('`') || before.ends_with("``") {
        return false;
    }
    let mut count = 0;
    let mut pos = 0;
    for node in children(block) {
        if pos >= offset {
            break;
        }
        if name(node) != "text" {
            count = 0;
            pos += node_size(node);
            continue;
        }
        if !marks(node).contains(&Mark::Code) {
            let text = node["text"].as_str().unwrap_or_default();
            let end =
                utf16_byte_offset(text, (offset - pos).min(node_size(node))).unwrap_or(text.len());
            let bytes = &text.as_bytes()[..end];
            count += bytes
                .iter()
                .enumerate()
                .filter(|(index, byte)| {
                    **byte == b'`'
                        && (*index == 0 || bytes[*index - 1] != b'`')
                        && bytes.get(*index + 1) != Some(&b'`')
                })
                .count();
        }
        pos += node_size(node);
    }
    count % 2 == 1
}
fn plain(block: &Value, from: usize, to: usize) -> bool {
    let mut pos = 0;
    for node in children(block) {
        let end = pos + node_size(node);
        if from < end
            && to > pos
            && (name(node) != "text"
                || node["marks"]
                    .as_array()
                    .is_some_and(|marks| !marks.is_empty()))
        {
            return false;
        }
        pos = end;
    }
    true
}
fn shown_source(block: &Value, offset: usize) -> bool {
    let mut pos = 0;
    children(block).iter().any(|node| {
        let end = pos + node_size(node);
        let touches = offset >= pos
            && offset <= end
            && node["marks"]
                .as_array()
                .is_some_and(|marks| marks.iter().any(|mark| mark["type"] == "markdownSource"));
        pos = end;
        touches
    })
}
impl CodePairs {
    pub fn map(&mut self, mut map: impl FnMut(usize, i8) -> Option<usize>) {
        self.pairs = self
            .pairs
            .iter()
            .filter_map(|(from, to)| Some((map(*from, 1)?, map(*to, -1)?)))
            .collect();
    }
    pub fn composed_tick(&mut self, before: &Value, from: usize, to: usize) {
        if from != to {
            return;
        }
        if let Some((start, block)) = block_at(before, from)
            && closes(&block, from - start)
        {
            self.pairs.push((from - 1, from + 1));
        }
    }
    pub fn type_tick(
        &mut self,
        doc: &Value,
        selection: Selection,
        stored_marks: &[Mark],
        composing: bool,
    ) -> Option<Rewrite> {
        if composing || !selection.collapsed() {
            return None;
        }
        let from = selection.head;
        let (start, block) = block_at(doc, from)?;
        let offset = from - start;
        if shown_source(&block, offset) {
            return None;
        }
        if closes(&block, offset) {
            let mut next = doc.clone();
            if !splice_inline(&mut next, from, from, &[text_node("`", stored_marks)]) {
                return None;
            }
            self.map(|pos, bias| Some(pos + usize::from(pos > from || (pos == from && bias > 0))));
            self.pairs.push((from - 1, from + 1));
            return Some(Rewrite {
                document: next,
                selection: Selection::caret(from + 1),
                stored_marks: stored_marks.to_vec(),
            });
        }
        if before_node(&block, offset).is_some_and(|(_, text)| text.ends_with('`')) {
            return None;
        }
        let text = content_text(&block);
        let byte = utf16_byte_offset(&text, offset)?;
        let tail = &text[byte..];
        let close = tail.find(['`', '\n'])?;
        if close == 0
            || tail.as_bytes()[close] != b'`'
            || tail.as_bytes().get(close + 1) == Some(&b'`')
        {
            return None;
        }
        let length = utf16_len(&tail[..close]);
        let end = from + length;
        if !plain(&block, offset, offset + length + 1) {
            return None;
        }
        let mut next = doc.clone();
        if !splice_inline(&mut next, end, end + 1, &[]) {
            return None;
        }
        transform_marks(&mut next, from, end, |node| {
            node["marks"] = json!([{"type":"code"}])
        });
        self.map(|pos, _| {
            if pos == end + 1 {
                None
            } else {
                Some(pos - usize::from(pos > end))
            }
        });
        Some(Rewrite {
            document: next,
            selection: Selection::caret(from),
            stored_marks: vec![Mark::Code],
        })
    }
    pub fn caret_moved(
        &self,
        doc: &Value,
        previous: Selection,
        selection: Selection,
        document_changed: bool,
    ) -> Option<Rewrite> {
        if document_changed || !selection.collapsed() || previous.from() == selection.from() {
            return None;
        }
        let &(from, to) = self.pairs.iter().find(|(_, to)| *to == selection.from())?;
        let (start, block) = block_at(doc, from)?;
        if to > start + node_size(&block) - 2 || !plain(&block, from - start, to - start) {
            return None;
        }
        let text = content_text(&block);
        let a = utf16_byte_offset(&text, from - start)?;
        let b = utf16_byte_offset(&text, to - start)?;
        let pair = &text[a..b];
        if pair.len() <= 2
            || !pair.starts_with('`')
            || !pair.ends_with('`')
            || pair[1..pair.len() - 1].contains('`')
            || text[..a].ends_with('`')
            || text[b..].starts_with('`')
        {
            return None;
        }
        let mut next = doc.clone();
        if !splice_inline(&mut next, to - 1, to, &[])
            || !splice_inline(&mut next, from, from + 1, &[])
        {
            return None;
        }
        transform_marks(&mut next, from, to - 2, |node| {
            node["marks"] = json!([{"type":"code"}])
        });
        Some(Rewrite {
            document: next,
            selection: Selection::caret(to - 2),
            stored_marks: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread_views::composer::editor::{BuildOptions, build_document, serialize_document};
    fn literal(text: &str) -> Value {
        build_document(
            text,
            BuildOptions {
                literal: true,
                ..Default::default()
            },
            |_| None,
            |_, _| false,
        )
    }
    #[test]
    fn only_tracked_typed_pairs_convert_on_a_caret_exit() {
        let mut pairs = CodePairs::default();
        let typed = pairs
            .type_tick(&literal("`"), Selection::caret(2), &[], false)
            .unwrap();
        assert_eq!(serialize_document(&typed.document).value, "``");
        pairs.map(|pos, _| Some(pos + usize::from(pos >= 2)));
        let filled = literal("`x`");
        assert!(
            pairs
                .caret_moved(&filled, Selection::caret(3), Selection::caret(4), true)
                .is_none()
        );
        let converted = pairs
            .caret_moved(&filled, Selection::caret(3), Selection::caret(4), false)
            .unwrap();
        assert_eq!(converted.selection, Selection::caret(2));
        assert_eq!(
            converted.document["content"][0]["content"][0]["marks"],
            json!([{"type":"code"}])
        );
        assert!(
            CodePairs::default()
                .caret_moved(&filled, Selection::caret(3), Selection::caret(4), false)
                .is_none()
        );
    }
    #[test]
    fn composition_and_opening_after_existing_closing_ticks_follow_the_same_boundary() {
        let mut pairs = CodePairs::default();
        assert!(
            pairs
                .type_tick(&literal("`"), Selection::caret(2), &[], true)
                .is_none()
        );
        pairs.composed_tick(&literal("`"), 2, 2);
        pairs.map(|pos, _| Some(pos + usize::from(pos >= 2)));
        assert!(
            pairs
                .caret_moved(
                    &literal("`x`"),
                    Selection::caret(3),
                    Selection::caret(4),
                    false
                )
                .is_some()
        );
        let opened = CodePairs::default()
            .type_tick(&literal("x` tail"), Selection::caret(1), &[], false)
            .unwrap();
        assert_eq!(serialize_document(&opened.document).value, "`x` tail");
        assert_eq!(opened.selection, Selection::caret(1));
        assert_eq!(opened.stored_marks, vec![Mark::Code]);
        for value in ["``x`", "x``", "x\ny`"] {
            assert!(
                CodePairs::default()
                    .type_tick(&literal(value), Selection::caret(1), &[], false)
                    .is_none()
            );
        }
    }
}
