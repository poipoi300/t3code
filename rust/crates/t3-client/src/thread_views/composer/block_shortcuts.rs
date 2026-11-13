//! Block gestures normalize changed top-level paragraphs while keeping inline payloads intact.
use super::editor::{self, BuildOptions, children, name, node_size, serialize_document};
use crate::thread_views::document::{BlockKind, Document, Selection, utf16_len};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn suppress_block_chord(chord: &str) -> bool {
    matches!(
        chord,
        "Mod-Shift-8"
            | "Mod-Shift-7"
            | "Mod-Shift-9"
            | "Mod-Shift-b"
            | "Mod-Alt-c"
            | "Mod-Alt-1"
            | "Mod-Alt-2"
            | "Mod-Alt-3"
            | "Mod-Alt-4"
            | "Mod-Alt-5"
            | "Mod-Alt-6"
    )
}
fn paragraph_text(node: &Value) -> String {
    children(node)
        .iter()
        .map(|child| {
            if name(child) == "text" {
                child["text"].as_str().unwrap_or_default().to_owned()
            } else {
                "\u{fffc}".into()
            }
        })
        .collect()
}
fn prefix(node: &Value) -> Option<(String, usize, bool)> {
    let text = paragraph_text(node);
    let document = Document::parse(&text, false);
    let BlockKind::ListItem { prefix } = &document.blocks.first()?.kind else {
        return None;
    };
    if prefix.starts_with([' ', '\t']) {
        return None;
    }
    let task = prefix.starts_with('-') && prefix.contains('[');
    Some((prefix.clone(), utf16_len(prefix), task))
}
fn compatible(before: &Value, after: &Value) -> bool {
    let delimiter = |node: &Value| {
        children(node)
            .first()
            .and_then(|item| item["attrs"]["marker"].as_str())
            .and_then(|marker| marker.chars().next_back())
    };
    matches!(name(before), "bulletList" | "orderedList" | "taskList")
        && name(before) == name(after)
        && delimiter(before) == delimiter(after)
}
pub struct BlockRewrite {
    pub document: Value,
    pub selection: Selection,
}
#[derive(Default)]
pub struct ListNormalizer {
    pending: BTreeSet<usize>,
}

impl ListNormalizer {
    pub fn map_pending(&mut self, map: impl Fn(usize) -> Option<usize>) {
        self.pending = self
            .pending
            .iter()
            .filter_map(|position| map(*position))
            .collect();
    }
    pub fn apply(
        &mut self,
        doc: &Value,
        selection: Selection,
        changed: &[(usize, usize)],
        composing: bool,
        history: bool,
    ) -> BlockRewrite {
        if history {
            self.pending.clear();
            return BlockRewrite {
                document: doc.clone(),
                selection,
            };
        }
        let mut pos = 0;
        for node in children(doc) {
            if name(node) == "paragraph"
                && let Some((_, length, _)) = prefix(node)
                && changed
                    .iter()
                    .any(|(from, to)| *from <= pos + 1 + length && *to >= pos)
            {
                self.pending.insert(pos);
            }
            pos += node_size(node);
        }
        if composing {
            return BlockRewrite {
                document: doc.clone(),
                selection,
            };
        }
        if self.pending.is_empty() {
            return BlockRewrite {
                document: doc.clone(),
                selection,
            };
        }
        let before = serialize_document(doc);
        let mut result = vec![];
        let mut last_changed = false;
        pos = 0;
        let pending = std::mem::take(&mut self.pending);
        for node in children(doc) {
            let mut converted = None;
            if pending.contains(&pos)
                && name(node) == "paragraph"
                && let Some((prefix, length, task)) = prefix(node)
            {
                let text = paragraph_text(node);
                let active = selection.from() > pos && selection.from() < pos + node_size(node);
                let suffix = &text[prefix.len()..];
                let unfinished = matches!(
                    suffix,
                    "" | "[" | "[ " | "[x" | "[X" | "[ ]" | "[x]" | "[X]"
                );
                let defer = active
                    && if task {
                        !prefix.ends_with([' ', '\t'])
                    } else {
                        prefix.starts_with('-') && unfinished
                    };
                let mut offset = 0;
                let marked = children(node).iter().any(|child| {
                    let start = offset;
                    offset += node_size(child);
                    start < length
                        && child["marks"]
                            .as_array()
                            .is_some_and(|marks| !marks.is_empty())
                });
                if defer {
                    self.pending.insert(pos);
                } else if !marked && prefix.ends_with([' ', '\t']) {
                    let built = editor::build_document(
                        &text,
                        BuildOptions::default(),
                        |_| None,
                        |_, _| false,
                    );
                    if let Some(mut list) = children(&built)
                        .first()
                        .filter(|list| {
                            matches!(name(list), "bulletList" | "orderedList" | "taskList")
                        })
                        .cloned()
                    {
                        let mut paragraph = node.clone();
                        editor::splice_inline(&mut paragraph, 1, 1 + length, &[]);
                        list["content"][0]["content"] = json!([paragraph]);
                        converted = Some(list);
                    }
                }
            }
            let changed = converted.is_some();
            let next = converted.unwrap_or_else(|| node.clone());
            if let Some(prior) = result
                .last_mut()
                .filter(|prior| (last_changed || changed) && compatible(prior, &next))
            {
                if let Some(items) = prior["content"].as_array_mut() {
                    items.extend_from_slice(children(&next));
                }
                last_changed |= changed;
            } else {
                result.push(next);
                last_changed = changed;
            }
            pos += node_size(node);
        }
        let mut document = doc.clone();
        document["content"] = result.into();
        let after = serialize_document(&document);
        let map = |position| {
            after.flat_to_pm(
                after.markdown_to_flat(before.flat_to_markdown(before.pm_to_flat(position))),
            )
        };
        self.pending = self
            .pending
            .iter()
            .map(|position| map(position + 1).saturating_sub(1))
            .collect();
        let selection = if document == *doc {
            selection
        } else {
            Selection {
                anchor: map(selection.anchor),
                head: map(selection.head),
            }
        };
        BlockRewrite {
            document,
            selection,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn paragraph(text: &str) -> Value {
        json!({"type":"paragraph","content":[{"type":"text","text":text}]})
    }
    fn doc(nodes: Vec<Value>) -> Value {
        json!({"type":"doc","content":nodes})
    }
    #[test]
    fn source_list_gestures_and_bracket_prefixed_content() {
        for (marker, kind) in [
            ("- ", "bulletList"),
            ("* ", "bulletList"),
            ("+ ", "bulletList"),
            ("3) ", "orderedList"),
            ("4. ", "orderedList"),
            ("- [ ] ", "taskList"),
            ("- [x] ", "taskList"),
        ] {
            for suffix in ["existing text", "[ordinary text]", "[link](url)", "[x]text"] {
                let text = format!("{marker}{suffix}");
                let mut normalizer = ListNormalizer::default();
                let before = doc(vec![paragraph(&text)]);
                let result = normalizer.apply(
                    &before,
                    Selection::caret(utf16_len(&text) + 1),
                    &[(1, 1 + marker.len())],
                    false,
                    false,
                );
                assert_eq!(result.document["content"][0]["type"], kind, "{text}");
                assert_eq!(serialize_document(&result.document).value, text);
            }
        }
        for text in ["a - b", "-x", "-", "  - nested"] {
            let before = doc(vec![paragraph(text)]);
            assert_eq!(
                ListNormalizer::default()
                    .apply(&before, Selection::caret(1), &[(1, 10)], false, false)
                    .document,
                before
            );
        }
    }
    #[test]
    fn dash_task_and_composition_wait_for_completion_and_history_restores_literal_markers() {
        let mut normalizer = ListNormalizer::default();
        for text in ["- ", "- [", "- [x", "- [x]"] {
            let before = doc(vec![paragraph(text)]);
            assert_eq!(
                normalizer
                    .apply(
                        &before,
                        Selection::caret(1 + utf16_len(text)),
                        &[(1, 20)],
                        false,
                        false
                    )
                    .document,
                before
            );
        }
        let before = doc(vec![paragraph("- [x] done")]);
        assert_eq!(
            normalizer
                .apply(&before, Selection::caret(11), &[(1, 11)], true, false)
                .document,
            before
        );
        let result = normalizer.apply(&before, Selection::caret(11), &[], false, false);
        assert_eq!(result.document["content"][0]["type"], "taskList");
        let before = doc(vec![paragraph("- text")]);
        assert_eq!(
            normalizer
                .apply(&before, Selection::caret(7), &[(1, 7)], false, true)
                .document,
            before
        );
        assert_eq!(
            normalizer
                .apply(&before, Selection::caret(1), &[], false, false)
                .document,
            before
        );
    }
    #[test]
    fn only_changed_unstyled_top_level_markers_convert_and_compatible_neighbors_join() {
        let mut first = paragraph("- styled");
        first["content"][0]["marks"] = json!([{"type":"bold"}]);
        let before = doc(vec![
            first.clone(),
            paragraph("- untouched"),
            paragraph("- changed"),
        ]);
        let start = node_size(&first) + node_size(&before["content"][1]);
        let result = ListNormalizer::default().apply(
            &before,
            Selection::caret(start + 3),
            &[(start + 1, start + 3)],
            false,
            false,
        );
        assert_eq!(result.document["content"][0], first);
        assert_eq!(result.document["content"][1]["type"], "paragraph");
        assert_eq!(result.document["content"][2]["type"], "bulletList");
        for (prefix, expected) in [("- ", 1), ("* ", 2)] {
            let list = editor::build_document(
                "- a",
                BuildOptions::default(),
                |_| None,
                |_, _| false,
            )["content"][0]
                .clone();
            let pos = node_size(&list);
            let before = doc(vec![list, paragraph(&format!("{prefix}b"))]);
            let result = ListNormalizer::default().apply(
                &before,
                Selection::caret(pos + 4),
                &[(pos + 1, pos + 3)],
                false,
                false,
            );
            assert_eq!(children(&result.document).len(), expected);
            assert_eq!(
                serialize_document(&result.document).value,
                format!("- a\n{prefix}b")
            );
        }
        for chord in [
            "Mod-Shift-8",
            "Mod-Shift-7",
            "Mod-Shift-9",
            "Mod-Shift-b",
            "Mod-Alt-c",
            "Mod-Alt-1",
            "Mod-Alt-3",
        ] {
            assert!(suppress_block_chord(chord));
        }
    }
}
