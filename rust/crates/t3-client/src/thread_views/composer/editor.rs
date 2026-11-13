//! Editor-independent document trees and the source, flat-text and tree-position maps.
use super::super::{
    document::{self, AtomKind, BlockKind, Document, utf16_byte_offset, utf16_len},
    rich_text::{Mark, parse_inline_markdown},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Text,
    Token,
    Break,
    Prefix,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichRun {
    pub kind: RunKind,
    pub flat_start: usize,
    pub doc_len: usize,
    pub md_len: usize,
    pub open_len: usize,
    pub close_len: usize,
    pub pm_pos: usize,
    pub md_start: usize,
    pub node_name: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RichDocMap {
    pub value: String,
    pub runs: Vec<RichRun>,
    pub doc_length: usize,
    pub context_ids: Vec<String>,
}
impl RichDocMap {
    pub fn flat_to_markdown(&self, offset: usize) -> usize {
        let bounded = offset.min(self.doc_length);
        for run in &self.runs {
            let end = run.flat_start + run.doc_len;
            if bounded < end || (bounded == end && run.node_name.as_deref() == Some("codeBlock")) {
                return run.md_start
                    + if matches!(run.kind, RunKind::Text | RunKind::Token) {
                        run.open_len
                    } else {
                        0
                    }
                    + bounded.saturating_sub(run.flat_start);
            }
        }
        self.runs.last().map_or(0, |run| run.md_start + run.md_len)
    }
    pub fn markdown_to_flat(&self, offset: usize) -> usize {
        for run in &self.runs {
            if offset < run.md_start + run.md_len {
                if run.kind == RunKind::Prefix {
                    return run.flat_start;
                }
                let within = offset.saturating_sub(run.md_start);
                if matches!(run.kind, RunKind::Text | RunKind::Token) {
                    if within <= run.open_len {
                        return run.flat_start;
                    }
                    if run.kind == RunKind::Token || within >= run.open_len + run.doc_len {
                        return run.flat_start + run.doc_len;
                    }
                    return run.flat_start + within - run.open_len;
                }
                return run.flat_start + within;
            }
        }
        self.doc_length
    }
    pub fn flat_to_pm(&self, offset: usize) -> usize {
        let bounded = offset.min(self.doc_length);
        for run in &self.runs {
            if bounded < run.flat_start + run.doc_len {
                return run.pm_pos + bounded.saturating_sub(run.flat_start);
            }
        }
        self.runs.last().map_or(1, |r| r.pm_pos + r.doc_len)
    }
    pub fn pm_to_flat(&self, pos: usize) -> usize {
        for run in &self.runs {
            if pos >= run.pm_pos && pos <= run.pm_pos + run.doc_len {
                return run.flat_start + (pos - run.pm_pos).min(run.doc_len);
            }
        }
        self.runs
            .iter()
            .rfind(|r| r.pm_pos <= pos)
            .map_or(0, |r| r.flat_start + r.doc_len)
            .min(self.doc_length)
    }
    fn push(
        &mut self,
        kind: RunKind,
        source: &str,
        len: usize,
        markers: (usize, usize),
        pos: usize,
        name: Option<&str>,
    ) {
        self.runs.push(RichRun {
            kind,
            flat_start: self.doc_length,
            doc_len: len,
            md_len: utf16_len(source),
            open_len: markers.0,
            close_len: markers.1,
            pm_pos: pos,
            md_start: utf16_len(&self.value),
            node_name: name.map(str::to_string),
        });
        self.doc_length += len;
        self.value.push_str(source);
    }
    fn newline(&mut self, pos: Option<usize>) {
        let pos = pos.unwrap_or_else(|| self.runs.last().map_or(1, |r| r.pm_pos + r.doc_len));
        self.push(RunKind::Break, "\n", 1, (0, 0), pos, None);
    }
}
pub(crate) fn name(node: &Value) -> &str {
    node["type"].as_str().unwrap_or_default()
}
fn attr<'a>(node: &'a Value, key: &str, default: &'a str) -> &'a str {
    node["attrs"][key].as_str().unwrap_or(default)
}
pub(crate) fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}
fn text(node: &Value) -> String {
    if let Some(text) = node["text"].as_str() {
        return text.into();
    }
    children(node).iter().map(text).collect()
}
pub fn node_size(node: &Value) -> usize {
    match name(node) {
        "text" => utf16_len(node["text"].as_str().unwrap_or_default()),
        "doc" => children(node).iter().map(node_size).sum(),
        "hardBreak" | "horizontalRule" => 1,
        name if name.starts_with("composer-") => 1,
        _ => 2 + children(node).iter().map(node_size).sum::<usize>(),
    }
}
/// Serializes a tree selection, retaining shared list and quote structure.
pub fn serialize_selection(doc: &Value, from: usize, to: usize) -> Option<String> {
    if from > to || to > node_size(doc) {
        return None;
    }
    fn cut(node: &Value, start: usize, from: usize, to: usize) -> Option<Vec<Value>> {
        let mut result = Vec::new();
        if from == to {
            return Some(result);
        }
        let mut pos = start;
        for child in children(node) {
            let end = pos + node_size(child);
            if from < end && to > pos {
                let mut clipped = child.clone();
                if name(child) == "text" {
                    let text = child["text"].as_str()?;
                    let left = utf16_byte_offset(text, from.saturating_sub(pos))?;
                    let right = utf16_byte_offset(text, (to - pos).min(node_size(child)))?;
                    clipped["text"] = json!(&text[left..right]);
                } else if !children(child).is_empty() {
                    clipped["content"] = json!(cut(child, pos + 1, from, to)?);
                }
                result.push(clipped);
            }
            pos = end;
        }
        Some(result)
    }
    let mut shared = doc;
    let mut start = 0;
    let mut ancestors = Vec::new();
    loop {
        let mut pos = start;
        let child = children(shared).iter().find_map(|child| {
            let end = pos + node_size(child);
            let nested = !matches!(name(child), "text" | "hardBreak" | "horizontalRule")
                && !name(child).starts_with("composer-")
                && from > pos
                && to < end;
            let result = nested.then_some((child, pos + 1));
            pos = end;
            result
        });
        let Some((child, child_start)) = child else {
            break;
        };
        ancestors.push(shared);
        shared = child;
        start = child_start;
    }
    let mut content = cut(shared, start, from, to)?;
    if content.first().is_some_and(|node| {
        matches!(name(node), "text" | "hardBreak") || name(node).starts_with("composer-")
    }) {
        content = vec![json!({"type":"paragraph","content":content})];
    } else {
        while name(shared) != "doc" {
            let mut wrapper = shared.clone();
            wrapper["content"] = json!(content);
            content = vec![wrapper];
            if !matches!(name(shared), "listItem" | "taskItem") {
                break;
            }
            shared = ancestors.pop()?;
        }
    }
    Some(serialize_document(&json!({"type":"doc","content":content})).value)
}
fn mark_name(mark: Mark) -> &'static str {
    match mark {
        Mark::Bold => "bold",
        Mark::Italic => "italic",
        Mark::Strike => "strike",
        Mark::Code => "code",
    }
}
pub(crate) fn marks(node: &Value) -> Vec<Mark> {
    children_marks(node)
        .iter()
        .filter_map(|m| match m["type"].as_str() {
            Some("bold") => Some(Mark::Bold),
            Some("italic") => Some(Mark::Italic),
            Some("strike") => Some(Mark::Strike),
            Some("code") => Some(Mark::Code),
            _ => None,
        })
        .collect()
}
fn children_marks(node: &Value) -> &[Value] {
    node.get("marks")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}
pub(crate) fn text_node(text: &str, marks: &[Mark]) -> Value {
    let mut node = json!({"type":"text","text":text});
    if !marks.is_empty() {
        node["marks"] = ordered_marks(marks);
    }
    node
}
fn ordered_marks(marks: &[Mark]) -> Value {
    [Mark::Strike, Mark::Bold, Mark::Italic, Mark::Code]
        .into_iter()
        .filter(|mark| marks.contains(mark))
        .map(|mark| json!({"type":mark_name(mark)}))
        .collect()
}
pub(crate) fn textblocks(doc: &Value) -> Vec<(usize, Value)> {
    fn walk(node: &Value, pos: usize, result: &mut Vec<(usize, Value)>) {
        if matches!(name(node), "paragraph" | "heading" | "codeBlock") {
            result.push((pos + 1, node.clone()));
            return;
        }
        let mut child_pos = pos + usize::from(name(node) != "doc");
        for child in children(node) {
            walk(child, child_pos, result);
            child_pos += node_size(child);
        }
    }
    let mut result = vec![];
    walk(doc, 0, &mut result);
    result
}
pub(crate) fn cut(node: &Value, from: usize, to: usize) -> Option<Value> {
    if from == to {
        return None;
    }
    if name(node) != "text" {
        return (from == 0 && to == node_size(node)).then(|| node.clone());
    }
    let value = node["text"].as_str()?;
    let mut result = node.clone();
    result["text"] = json!(&value[utf16_byte_offset(value, from)?..utf16_byte_offset(value, to)?]);
    Some(result)
}
pub(crate) fn splice_inline(
    doc: &mut Value,
    from: usize,
    to: usize,
    replacement: &[Value],
) -> bool {
    fn walk(node: &mut Value, pos: usize, from: usize, to: usize, replacement: &[Value]) -> bool {
        if matches!(name(node), "paragraph" | "heading" | "codeBlock") {
            let start = pos + 1;
            let len = node_size(node) - 2;
            if from < start || to > start + len || from > to {
                return false;
            }
            let mut before = vec![];
            let mut after = vec![];
            let mut offset = start;
            for child in children(node) {
                let end = offset + node_size(child);
                if offset < from
                    && let Some(part) = cut(child, 0, (from - offset).min(node_size(child)))
                {
                    before.push(part);
                }
                if end > to
                    && let Some(part) = cut(child, to.saturating_sub(offset), node_size(child))
                {
                    after.push(part);
                }
                offset = end;
            }
            before.extend_from_slice(replacement);
            before.extend(after);
            node["content"] = before.into();
            return true;
        }
        let mut offset = pos + usize::from(name(node) != "doc");
        if let Some(content) = node.get_mut("content").and_then(Value::as_array_mut) {
            for child in content {
                let size = node_size(child);
                if walk(child, offset, from, to, replacement) {
                    return true;
                }
                offset += size;
            }
        }
        false
    }
    walk(doc, 0, from, to, replacement)
}
pub(crate) fn transform_marks(
    doc: &mut Value,
    from: usize,
    to: usize,
    mut transform: impl FnMut(&mut Value),
) {
    for (start, block) in textblocks(doc).into_iter().rev() {
        if name(&block) == "codeBlock" {
            continue;
        }
        let mut at = start;
        let positions: Vec<_> = children(&block)
            .iter()
            .map(|child| {
                let pos = at;
                at += node_size(child);
                (pos, child)
            })
            .collect();
        for (pos, child) in positions.into_iter().rev() {
            let a = from.max(pos);
            let b = to.min(pos + node_size(child));
            if a >= b {
                continue;
            }
            if let Some(mut clipped) = cut(child, a - pos, b - pos) {
                transform(&mut clipped);
                splice_inline(doc, a, b, &[clipped]);
            }
        }
    }
}
pub fn step_caret_across_styled_edge(
    before: &[Mark],
    after: &[Mark],
    current: &[Mark],
    backward: bool,
) -> Option<Vec<Mark>> {
    let equal = |a: &[Mark], b: &[Mark]| a.len() == b.len() && a.iter().all(|m| b.contains(m));
    if equal(before, after) || (!equal(current, before) && !equal(current, after)) {
        return None;
    }
    let target = if backward { before } else { after };
    (!equal(current, target)).then(|| target.to_vec())
}
pub fn surround_selection(doc: &Value, from: usize, to: usize, mark: Mark) -> Option<Value> {
    if from >= to {
        return None;
    }
    let (start, block) = textblocks(doc).into_iter().find(|(start, b)| {
        from >= *start && to <= *start + node_size(b) - 2 && name(b) != "codeBlock"
    })?;
    let mut pos = start;
    for child in children(&block) {
        let end = pos + node_size(child);
        if from < end
            && to > pos
            && (name(child) == "hardBreak"
                || children_marks(child)
                    .iter()
                    .any(|mark| mark["type"] == "markdownSource"))
        {
            return None;
        }
        pos = end;
    }
    let mut next = doc.clone();
    transform_marks(&mut next, from, to, |node| {
        let mut node_marks = children_marks(node).to_vec();
        if !node_marks.iter().any(|m| m["type"] == mark_name(mark)) {
            node_marks.push(json!({"type":mark_name(mark)}));
        }
        node["marks"] = node_marks.into();
    });
    let (_, modified) = textblocks(&next)
        .into_iter()
        .find(|(pos, _)| *pos == start)?;
    let inline = children(&modified)
        .iter()
        .map(|child| {
            if name(child).starts_with("composer-") {
                let mut node = json!({"type":"text","text":"\u{fffc}"});
                if let Some(marks) = child.get("marks") {
                    node["marks"] = marks.clone();
                }
                node
            } else {
                child.clone()
            }
        })
        .collect::<Vec<_>>();
    let paragraph = json!({"type":"paragraph","content":inline});
    let serialized = serialize_document(&json!({"type":"doc","content":[paragraph.clone()]}));
    let rebuilt = build_document(
        &serialized.value,
        BuildOptions {
            blocks: false,
            ..Default::default()
        },
        |_| None,
        |_, _| false,
    );
    fn signature(node: &Value) -> Vec<(String, Vec<Value>)> {
        let mut result: Vec<(String, Vec<Value>)> = Vec::new();
        for child in children(node) {
            let mut marks = children_marks(child).to_vec();
            marks.sort_by_key(|mark| mark["type"].as_str().unwrap_or_default().to_owned());
            let text = child["text"].as_str().unwrap_or_default();
            if let Some((prior, _)) = result
                .last_mut()
                .filter(|(_, prior_marks)| *prior_marks == marks)
            {
                prior.push_str(text);
            } else {
                result.push((text.into(), marks));
            }
        }
        result
    }
    (children(&rebuilt)
        .first()
        .is_some_and(|node| signature(node) == signature(&paragraph)))
    .then_some(next)
}
fn atom_source(node: &Value) -> String {
    if name(node) == "composer-skill" {
        let skill = attr(node, "skillName", "");
        if skill.is_empty() {
            String::new()
        } else {
            format!("${skill}")
        }
    } else {
        attr(node, "source", "").into()
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ChipReplacement {
    pub from: usize,
    pub to: usize,
    pub node: Value,
}

pub fn skill_chip_replacements(
    doc: &Value,
    skill: impl Fn(&str) -> Option<(String, Option<String>)>,
    model: impl Fn(&str, &str) -> bool,
) -> Vec<ChipReplacement> {
    let mut replacements = Vec::new();
    let mut inline = Vec::new();
    for (start, block) in textblocks(doc) {
        let mut pos = start;
        for child in children(&block) {
            let stale = match name(child) {
                "composer-skill" => {
                    let id = attr(child, "skillName", "");
                    !id.is_empty() && skill(id).is_none()
                }
                "composer-model" => !model(attr(child, "instanceId", ""), attr(child, "model", "")),
                _ => false,
            };
            if stale {
                let mut node = json!({"type":"text","text":atom_source(child)});
                if let Some(marks) = child.get("marks") {
                    node["marks"] = marks.clone();
                }
                replacements.push(ChipReplacement {
                    from: pos,
                    to: pos + node_size(child),
                    node,
                });
            }
            inline.push((pos, child.clone()));
            pos += node_size(child);
        }
    }
    let map = serialize_document(doc);
    for atom in document::collect_atoms(&map.value) {
        if !matches!(atom.kind, AtomKind::Skill | AtomKind::Model) {
            continue;
        }
        let Some(mut node) = atom_node(&atom, &skill, &model) else {
            continue;
        };
        let Some(run) = map.runs.iter().find(|run| {
            run.kind == RunKind::Text
                && run.node_name.as_deref() != Some("codeBlock")
                && atom.range.start >= run.md_start + run.open_len
                && atom.range.end <= run.md_start + run.open_len + run.doc_len
        }) else {
            continue;
        };
        let from = run.pm_pos + atom.range.start - run.md_start - run.open_len;
        if let Some((_, child)) = inline
            .iter()
            .find(|(pos, child)| *pos <= from && from < *pos + node_size(child))
            && let Some(marks) = child.get("marks")
        {
            node["marks"] = marks.clone();
        }
        replacements.push(ChipReplacement {
            from,
            to: from + atom.range.end - atom.range.start,
            node,
        });
    }
    replacements.sort_by_key(|replacement| std::cmp::Reverse(replacement.from));
    replacements
}

pub fn reconcile_chips(
    doc: &Value,
    skill: impl Fn(&str) -> Option<(String, Option<String>)>,
    model: impl Fn(&str, &str) -> bool,
) -> Value {
    let mut next = doc.clone();
    for replacement in skill_chip_replacements(doc, skill, model) {
        splice_inline(
            &mut next,
            replacement.from,
            replacement.to,
            &[replacement.node],
        );
    }
    next
}
fn inline_runs(container: &Value, start: usize, map: &mut RichDocMap) {
    let mut nodes = vec![];
    for child in children(container) {
        if name(child) != "text" || marks(child).contains(&Mark::Code) {
            nodes.push(child.clone());
            continue;
        }
        let value = child["text"].as_str().unwrap_or_default();
        let begin = value.len() - value.trim_start().len();
        let end = begin.max(value.trim_end().len());
        let mut from = 0;
        for to in [begin, end, value.len()] {
            if to > from {
                let mut part = child.clone();
                part["text"] = json!(&value[from..to]);
                nodes.push(part);
            }
            from = to;
        }
    }
    let order = [Mark::Strike, Mark::Bold, Mark::Italic, Mark::Code];
    let mut all: Vec<_> = nodes.iter().map(marks).collect();
    for mark in order.iter().filter(|m| **m != Mark::Code) {
        for reverse in [false, true] {
            let mut has_content = false;
            for j in 0..nodes.len() {
                let index = if reverse { nodes.len() - 1 - j } else { j };
                let node = &nodes[index];
                if name(node) == "hardBreak" || !all[index].contains(mark) {
                    has_content = false;
                } else if name(node) == "text"
                    && !all[index].contains(&Mark::Code)
                    && node["text"]
                        .as_str()
                        .is_some_and(|s| s.chars().all(char::is_whitespace))
                {
                    if !has_content {
                        all[index].retain(|m| m != mark);
                    }
                } else {
                    has_content = true;
                }
            }
        }
    }
    let mut ends = [None; 4];
    let mut ordered = all.clone();
    for index in (0..nodes.len()).rev() {
        if name(&nodes[index]) == "hardBreak" {
            all[index].clear();
        }
        for (i, mark) in order.iter().enumerate() {
            if !all[index].contains(mark) {
                ends[i] = None;
            } else if ends[i].is_none() {
                ends[i] = Some(index);
            }
        }
        ordered[index] = all[index].clone();
        ordered[index].sort_by_key(|mark| {
            let i = order.iter().position(|m| m == mark).unwrap_or_default();
            (std::cmp::Reverse(ends[i]), i)
        });
    }
    for index in 1..ordered.len() {
        let retained: Vec<_> = ordered[index - 1]
            .iter()
            .take_while(|m| ordered[index].contains(m))
            .copied()
            .collect();
        let mut next = retained.clone();
        next.extend(
            ordered[index]
                .iter()
                .filter(|m| !retained.contains(m))
                .copied(),
        );
        ordered[index] = next;
    }
    let common =
        |left: &[Mark], right: &[Mark]| left.iter().zip(right).take_while(|(a, b)| a == b).count();
    let mut offset = 0;
    for (index, node) in nodes.iter().enumerate() {
        let pos = start + offset;
        offset += node_size(node);
        if name(node) == "hardBreak" {
            map.newline(Some(pos));
            continue;
        }
        let previous = index
            .checked_sub(1)
            .map_or(&[][..], |i| ordered[i].as_slice());
        let next = ordered.get(index + 1).map_or(&[][..], Vec::as_slice);
        let open: String = ordered[index][common(&ordered[index], previous)..]
            .iter()
            .map(|m| m.delimiter())
            .collect();
        let close: String = ordered[index][common(&ordered[index], next)..]
            .iter()
            .rev()
            .map(|m| m.delimiter())
            .collect();
        let source = if name(node) == "text" {
            node["text"].as_str().unwrap_or_default().into()
        } else {
            atom_source(node)
        };
        map.push(
            if name(node) == "text" {
                RunKind::Text
            } else {
                RunKind::Token
            },
            &format!("{open}{source}{close}"),
            if name(node) == "text" {
                utf16_len(&source)
            } else {
                1
            },
            (utf16_len(&open), utf16_len(&close)),
            pos,
            if name(node) == "text" {
                None
            } else {
                Some(name(node))
            },
        );
    }
    if nodes.is_empty() {
        map.push(RunKind::Text, "", 0, (0, 0), start, None);
    }
}
fn is_list(node: &Value) -> bool {
    matches!(name(node), "bulletList" | "orderedList" | "taskList")
}
fn list_prefix(item: &Value, empty: bool) -> String {
    let indent = attr(item, "indent", "");
    if name(item) == "taskItem" {
        let space = attr(item, "contentSpace", if empty { "" } else { " " });
        let space = if space.is_empty() && !empty {
            " "
        } else {
            space
        };
        format!(
            "{indent}-{}[{}]{space}",
            attr(item, "markerSpace", " "),
            if item["attrs"]["checked"] == true {
                "x"
            } else {
                " "
            }
        )
    } else {
        let marker = attr(item, "marker", "-");
        let marker = if marker.is_empty() { "-" } else { marker };
        let space = attr(item, "space", if empty { "" } else { " " });
        let space = if space.is_empty() && !empty {
            " "
        } else {
            space
        };
        format!("{indent}{marker}{space}")
    }
}
fn walk_list(list: &Value, start: usize, map: &mut RichDocMap) {
    let mut item_pos = start + 1;
    for (i, item) in children(list).iter().enumerate() {
        if i > 0 {
            map.newline(None);
        }
        let empty = children(item)
            .first()
            .is_some_and(|p| name(p) == "paragraph" && children(p).is_empty());
        map.push(
            RunKind::Prefix,
            &list_prefix(item, empty),
            0,
            (0, 0),
            item_pos + 2,
            None,
        );
        let mut child_pos = item_pos + 1;
        for (index, child) in children(item).iter().enumerate() {
            if index > 0 {
                map.newline(None);
            }
            if is_list(child) {
                walk_list(child, child_pos, map);
            } else if name(child) == "paragraph" {
                inline_runs(child, child_pos + 1, map);
            }
            child_pos += node_size(child);
        }
        item_pos += node_size(item);
    }
}
pub fn serialize_document(doc: &Value) -> RichDocMap {
    let mut map = RichDocMap::default();
    let mut block_pos = 0;
    let blocks = children(doc);
    for (i, block) in blocks.iter().enumerate() {
        let trailing_rule = i > 0
            && i == blocks.len() - 1
            && name(&blocks[i - 1]) == "horizontalRule"
            && name(block) == "paragraph"
            && children(block).is_empty();
        if i > 0 && !trailing_rule {
            map.newline(None);
        }
        if i > 0
            && name(&blocks[i - 1]) == "blockquote"
            && name(block) == "paragraph"
            && !children(block).is_empty()
        {
            map.push(RunKind::Prefix, "\n", 0, (0, 0), block_pos + 1, None);
        }
        if is_list(block) {
            walk_list(block, block_pos, &mut map);
        } else {
            match name(block) {
                "codeBlock" => {
                    let code = super::super::code::CodeBlock {
                        opening: document::Fence {
                            fence: attr(block, "fence", "```").into(),
                            language: attr(block, "language", "").into(),
                        },
                        text: text(block),
                        close: attr(block, "close", "").into(),
                    };
                    let source = code.source(i < blocks.len() - 1);
                    let content_len = utf16_len(&code.text);
                    let close_len = if source.ends_with(&code.close) && !code.close.is_empty() {
                        utf16_len(&code.close)
                    } else {
                        source
                            .rfind('\n')
                            .filter(|_| source.ends_with('`') || source.ends_with('~'))
                            .map_or(0, |at| utf16_len(&source[at..]))
                    };
                    let open_len = utf16_len(&source).saturating_sub(content_len + close_len);
                    map.push(
                        RunKind::Text,
                        &source,
                        content_len,
                        (open_len, close_len),
                        block_pos + 1,
                        Some("codeBlock"),
                    );
                }
                "blockquote" => {
                    let mut pos = block_pos + 1;
                    for (line, child) in children(block).iter().enumerate() {
                        if line > 0 {
                            map.newline(None);
                        }
                        map.push(
                            RunKind::Prefix,
                            attr(block, "prefix", "> "),
                            0,
                            (0, 0),
                            pos + 1,
                            None,
                        );
                        if name(child) == "paragraph" {
                            inline_runs(child, pos + 1, &mut map);
                        }
                        pos += node_size(child);
                    }
                }
                "heading" => {
                    let level = block["attrs"]["level"].as_u64().unwrap_or(1).min(6) as usize;
                    let prefix = format!("{}{}", "#".repeat(level), attr(block, "space", " "));
                    map.push(RunKind::Prefix, &prefix, 0, (0, 0), block_pos + 1, None);
                    inline_runs(block, block_pos + 1, &mut map);
                }
                "horizontalRule" => {
                    let source = attr(block, "source", "---");
                    map.push(
                        RunKind::Prefix,
                        if source.is_empty() { "---" } else { source },
                        0,
                        (0, 0),
                        block_pos + 1,
                        None,
                    );
                }
                "paragraph" => inline_runs(block, block_pos + 1, &mut map),
                _ => {}
            }
        }
        block_pos += node_size(block);
    }
    let mut seen = BTreeSet::new();
    for reference in crate::view_logic::context_references::collect_references(&map.value) {
        if seen.insert(reference.context_id.to_owned()) {
            map.context_ids.push(reference.context_id.to_owned());
        }
    }
    map
}
fn atom_node(
    atom: &document::Atom,
    skill: &impl Fn(&str) -> Option<(String, Option<String>)>,
    model: &impl Fn(&str, &str) -> bool,
) -> Option<Value> {
    Some(match atom.kind {
        AtomKind::Mention => {
            json!({"type":"composer-mention","attrs":{"path":atom.value,"source":atom.source}})
        }
        AtomKind::Skill => {
            let (label, description) = skill(&atom.value)?;
            json!({"type":"composer-skill","attrs":{"skillName":atom.value,"skillLabel":label,"skillDescription":description}})
        }
        AtomKind::Model => {
            let (instance, slug) = atom.value.split_once('/')?;
            if !model(instance, slug) {
                return None;
            }
            json!({"type":"composer-model","attrs":{"instanceId":instance,"model":slug,"source":atom.source}})
        }
        AtomKind::Citation => {
            let citation = super::citations::collect(&atom.source).pop()?.citation;
            json!({"type":"composer-citation","attrs":{"citation":citation,"source":atom.source}})
        }
        AtomKind::ContextReference => {
            let label = atom.source.split_once('[')?.1.split_once("](")?.0;
            let href = atom.source.split_once("](")?.1.strip_suffix(')')?;
            let (kind, id) = document::parse_context_href(href)?;
            json!({"type":"composer-context-reference","attrs":{"kind":kind,"contextId":id,"label":label,"source":atom.source}})
        }
    })
}
fn inline_content(
    source: &str,
    styling: bool,
    skill: &impl Fn(&str) -> Option<(String, Option<String>)>,
    model: &impl Fn(&str, &str) -> bool,
) -> Vec<Value> {
    let atoms = document::collect_atoms(source);
    let mut nodes = vec![];
    let mut masked = String::new();
    let mut from = 0;
    let mut sentinel = '\u{fffc}';
    if source.contains(sentinel) {
        sentinel = (0xe000..=0x10ffff)
            .filter_map(char::from_u32)
            .find(|c| !source.contains(*c))
            .unwrap_or('\u{fffc}');
    }
    for atom in atoms {
        let Some(node) = atom_node(&atom, skill, model) else {
            continue;
        };
        let (Some(start), Some(end)) = (
            utf16_byte_offset(source, atom.range.start),
            utf16_byte_offset(source, atom.range.end),
        ) else {
            continue;
        };
        masked.push_str(&source[from..start]);
        masked.push(sentinel);
        from = end;
        nodes.push(node);
    }
    masked.push_str(&source[from..]);
    let mut atoms = nodes.into_iter();
    let mut result = vec![];
    let spans = if styling {
        parse_inline_markdown(&masked)
    } else {
        vec![super::super::rich_text::Span {
            text: masked,
            marks: vec![],
        }]
    };
    for span in spans {
        for (i, piece) in span.text.split(sentinel).enumerate() {
            if i > 0
                && let Some(mut node) = atoms.next()
            {
                if !span.marks.is_empty() {
                    node["marks"] = ordered_marks(&span.marks);
                }
                result.push(node);
            }
            if !piece.is_empty() {
                result.push(text_node(piece, &span.marks));
            }
        }
    }
    result
}
#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    pub styling: bool,
    pub blocks: bool,
    pub literal: bool,
    pub literal_length: usize,
}
impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            styling: true,
            blocks: true,
            literal: false,
            literal_length: 0,
        }
    }
}
pub fn build_document(
    source: &str,
    options: BuildOptions,
    skill: impl Fn(&str) -> Option<(String, Option<String>)>,
    model: impl Fn(&str, &str) -> bool,
) -> Value {
    if !options.literal && options.literal_length > 0 {
        let boundary = (0..=options.literal_length.min(utf16_len(source)))
            .rev()
            .find_map(|position| utf16_byte_offset(source, position))
            .unwrap_or(0);
        let (literal, remainder) = source.split_at(boundary);
        let (head, tail) = remainder
            .split_once('\n')
            .map_or((remainder, None), |(head, tail)| (head, Some(tail)));
        let mut content = vec![];
        if !literal.is_empty() {
            content.push(text_node(literal, &[]));
        }
        content.extend(inline_content(head, options.styling, &skill, &model));
        let mut blocks = vec![json!({"type":"paragraph","content":content})];
        if let Some(tail) = tail {
            let rest = build_document(
                tail,
                BuildOptions {
                    literal_length: 0,
                    ..options
                },
                skill,
                model,
            );
            blocks.extend_from_slice(children(&rest));
        }
        return json!({"type":"doc","content":blocks});
    }
    let mut formulas = if options.literal {
        vec![]
    } else {
        document::math_ranges(source)
            .into_iter()
            .map(|(start, end)| (start, end, None))
            .collect::<Vec<_>>()
    };
    if !options.literal {
        for atom in document::collect_atoms(source)
            .into_iter()
            .filter(|atom| atom.source.contains('\n'))
        {
            if let (Some(start), Some(end), Some(node)) = (
                utf16_byte_offset(source, atom.range.start),
                utf16_byte_offset(source, atom.range.end),
                atom_node(&atom, &skill, &model),
            ) && !formulas
                .iter()
                .any(|(from, to, _)| start < *to && end > *from)
            {
                formulas.push((start, end, Some(node)));
            }
        }
        formulas.sort_by_key(|(start, _, _)| *start);
    }
    if !formulas.is_empty() {
        let mut available = (0xe000..=0x10ffff)
            .filter_map(char::from_u32)
            .filter(|candidate| !source.contains(*candidate));
        let mut replacements = vec![];
        let mut masked = String::new();
        let mut from = 0;
        for (start, end, node) in formulas {
            let Some(marker) = available.next() else {
                break;
            };
            masked.push_str(&source[from..start]);
            masked.push(marker);
            replacements.push((marker, &source[start..end], node));
            from = end;
        }
        masked.push_str(&source[from..]);
        if replacements.is_empty() {
            return build_document(
                source,
                BuildOptions {
                    literal: true,
                    ..options
                },
                skill,
                model,
            );
        }
        let mut document = build_document(&masked, options, skill, model);
        fn restore(node: &mut Value, replacements: &[(char, &str, Option<Value>)], code: bool) {
            let code = code || name(node) == "codeBlock";
            match node {
                Value::String(text) => {
                    for (marker, source, _) in replacements {
                        if text.contains(*marker) {
                            *text = text.replace(*marker, source);
                        }
                    }
                }
                Value::Array(values) => {
                    let mut expanded = Vec::with_capacity(values.len());
                    for mut value in std::mem::take(values) {
                        if name(&value) == "text"
                            && let Some(text) = value["text"].as_str()
                        {
                            let mut from = 0;
                            for (at, ch) in text.char_indices() {
                                let Some((_, formula, replacement)) =
                                    replacements.iter().find(|(marker, _, _)| *marker == ch)
                                else {
                                    continue;
                                };
                                if from < at {
                                    let mut before = value.clone();
                                    before["text"] = json!(&text[from..at]);
                                    expanded.push(before);
                                }
                                let mut node = if !code {
                                    replacement.as_ref().unwrap_or(&value).clone()
                                } else {
                                    value.clone()
                                };
                                if name(&node) == "text" {
                                    node["text"] = json!(formula);
                                }
                                if let Some(marks) = value.get("marks") {
                                    node["marks"] = marks.clone();
                                }
                                expanded.push(node);
                                from = at + ch.len_utf8();
                            }
                            if from > 0 {
                                if from < text.len() {
                                    let mut after = value.clone();
                                    after["text"] = json!(&text[from..]);
                                    expanded.push(after);
                                }
                                continue;
                            }
                        }
                        restore(&mut value, replacements, code);
                        expanded.push(value);
                    }
                    *values = expanded;
                }
                Value::Object(values) => {
                    for value in values.values_mut() {
                        restore(value, replacements, code);
                    }
                }
                _ => {}
            }
        }
        restore(&mut document, &replacements, false);
        return document;
    }
    let parsed = Document::parse(
        source,
        options.literal || !options.styling || !options.blocks,
    );
    let mut blocks: Vec<Value> = vec![];
    for block in &parsed.blocks {
        let start = utf16_byte_offset(source, block.start).unwrap_or_default();
        let end = utf16_byte_offset(source, block.end).unwrap_or(start);
        let line = &source[start..end];
        let make_inline = |text: &str| {
            if options.literal {
                if text.is_empty() {
                    vec![]
                } else {
                    vec![text_node(text, &[])]
                }
            } else {
                inline_content(text, options.styling, &skill, &model)
            }
        };
        match &block.kind {
            BlockKind::Code{opening,content_start,content_end,close}=>{
                let from=utf16_byte_offset(source,*content_start).unwrap_or(start);let to=utf16_byte_offset(source,*content_end).unwrap_or(from);
                blocks.push(json!({"type":"codeBlock","attrs":{"fence":opening.fence,"language":opening.language,"close":close},"content":if from<to{vec![text_node(&source[from..to],&[])]}else{vec![]}}));
            },
            BlockKind::Rule=>blocks.push(json!({"type":"horizontalRule","attrs":{"source":line}})),
            BlockKind::Heading{level,prefix}=>blocks.push(json!({"type":"heading","attrs":{"level":level,"space":&prefix[*level..]},"content":make_inline(&line[prefix.len()..])})),
            BlockKind::Quote{prefix}=>{
                let paragraph=json!({"type":"paragraph","content":make_inline(&line[prefix.len()..])});
                if let Some(last)=blocks.last_mut().filter(|b|name(b)=="blockquote" && attr(b,"prefix","")==prefix){if let Some(content)=last["content"].as_array_mut(){content.push(paragraph);}}
                else{blocks.push(json!({"type":"blockquote","attrs":{"prefix":prefix},"content":[paragraph]}));}
            },
            BlockKind::ListItem{prefix}=>{
                let indent_len=prefix.bytes().take_while(|b|matches!(b,b' '|b'\t')).count();let indent=&prefix[..indent_len];let rest=&prefix[indent_len..];
                let digits=rest.bytes().take_while(u8::is_ascii_digit).count();let task=rest.starts_with('-')&&rest.contains('[');let list_type=if task{"taskList"}else if digits>0{"orderedList"}else{"bulletList"};
                let marker_len=if digits>0{digits+1}else{1};
                let attrs=if task{let bracket=rest.find('[').unwrap_or(1);json!({"indent":indent,"checked":rest.as_bytes().get(bracket+1).is_some_and(|b|matches!(b,b'x'|b'X')),"markerSpace":&rest[1..bracket],"contentSpace":&rest[bracket+3..]})}else{json!({"indent":indent,"marker":&rest[..marker_len],"space":&rest[marker_len..]})};
                let item=json!({"type":if task{"taskItem"}else{"listItem"},"attrs":attrs,"content":[{"type":"paragraph","content":make_inline(&line[prefix.len()..])}]});
                append_list_item(&mut blocks,item,list_type,indent,digits);
            },
            BlockKind::Paragraph=>blocks.push(json!({"type":"paragraph","content":make_inline(line)}))
        }
    }
    if blocks.last().is_some_and(|b| name(b) == "horizontalRule") {
        blocks.push(json!({"type":"paragraph","content":[]}));
    }
    json!({"type":"doc","content":blocks})
}
fn append_list_item(
    blocks: &mut Vec<Value>,
    item: Value,
    list_type: &str,
    indent: &str,
    digits: usize,
) {
    let new_list = |item: &Value| {
        let mut list = json!({"type":list_type,"content":[]});
        if digits > 0 {
            list["attrs"] = json!({"start":attr(item,"marker","1").trim_end_matches(['.',')']).parse::<u64>().unwrap_or(1)});
        }
        list
    };
    if let Some(last) = blocks.last_mut().filter(|b| is_list(b)) {
        let first = children(last).first();
        let prior_indent = children(last).last().map_or("", |i| attr(i, "indent", ""));
        if indent != prior_indent && indent.starts_with(prior_indent) {
            if let Some(parent) = last["content"].as_array_mut().and_then(|c| c.last_mut())
                && let Some(content) = parent["content"].as_array_mut()
            {
                append_list_item(content, item, list_type, indent, digits);
                return;
            }
        } else if name(last) == list_type
            && first.is_some_and(|prior| {
                list_type == "taskList"
                    || attr(prior, "marker", "").trim_matches(|c: char| c.is_ascii_digit())
                        == attr(&item, "marker", "").trim_matches(|c: char| c.is_ascii_digit())
            })
            && let Some(content) = last["content"].as_array_mut()
        {
            content.push(item);
            return;
        }
    }
    let mut list = new_list(&item);
    list["content"] = json!([item]);
    blocks.push(list);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loaded_and_removed_resolvers_reconcile_only_chips_preserving_marks_and_citations() {
        let source =
            "Ask $browser and model:codex/gpt-6.1 now\n```\n$browser model:codex/gpt-6.1 \n```";
        let mut document = build_document(source, BuildOptions::default(), |_| None, |_, _| false);
        let reference =
            json!({"type":"composer-citation","attrs":{"source":"citation","citeKey":"stable"}});
        document["content"][0]["content"]
            .as_array_mut()
            .unwrap()
            .push(reference.clone());
        let before = serialize_document(&document).value;
        let changes = skill_chip_replacements(
            &document,
            |id| Some((id.into(), None)),
            |instance, model| instance == "codex" && model == "gpt-6.1",
        );
        assert_eq!(changes.len(), 2);
        assert!(changes[0].from > changes[1].from);
        let mut loaded = reconcile_chips(&document, |id| Some((id.into(), None)), |_, _| true);
        assert_eq!(serialize_document(&loaded).value, before);
        assert_eq!(children(&loaded)[1], children(&document)[1]);
        assert_eq!(children(&children(&loaded)[0]).last(), Some(&reference));
        transform_marks(&mut loaded, 5, 6, |node| {
            node["marks"] = json!([{"type":"bold"}])
        });
        let before = serialize_document(&loaded).value;
        let removed = reconcile_chips(&loaded, |_| None, |_, _| false);
        assert_eq!(serialize_document(&removed).value, before);
        assert!(children(&children(&removed)[0]).iter().any(|node| node["text"] == "$browser" && node["marks"] == json!([{"type":"bold"}])));
        assert_eq!(children(&children(&removed)[0]).last(), Some(&reference));
    }
    fn build(source: &str) -> Value {
        build_document(
            source,
            BuildOptions::default(),
            |s| Some((s.into(), None)),
            |_, _| true,
        )
    }
    #[test]
    fn pasted_path_prefix_is_literal_while_the_rest_of_the_paste_keeps_markdown() {
        for path in [
            "src/__test__.tsx",
            "src/__tests__/a.ts",
            "pkg/__init__.py",
            "a/**b**/c",
        ] {
            let document = build_document(
                path,
                BuildOptions {
                    literal_length: utf16_len(path),
                    ..Default::default()
                },
                |_| None,
                |_, _| false,
            );
            assert_eq!(
                document["content"][0]["content"],
                json!([{"type":"text","text":path}])
            );
            assert_eq!(serialize_document(&document).value, path);
        }
        let path = "src/__test__.tsx";
        let text = format!("{path} and **this**\n- [ ] then __that__");
        let document = build_document(
            &text,
            BuildOptions {
                literal_length: utf16_len(path),
                ..Default::default()
            },
            |_| None,
            |_, _| false,
        );
        assert_eq!(document["content"][1]["type"], "taskList");
        assert_eq!(
            serialize_document(&document).value,
            format!("{path} and **this**\n- [ ] then **that**")
        );
        assert_eq!(
            document["content"][0]["content"][0],
            json!({"type":"text","text":path})
        );
    }
    #[test]
    fn formula_commands_and_markers_remain_plain_editable_source() {
        for source in [
            "$x_i + **bold**$",
            r"\(\text{use @README.md and $browser} + **x**\)",
            "\\[\n**formula**\n\\]",
            "before $$a * b$$ after",
        ] {
            let document = build_document(
                source,
                BuildOptions::default(),
                |name| Some((name.into(), None)),
                |_, _| true,
            );
            assert_eq!(serialize_document(&document).value, source);
            fn plain(node: &Value) {
                assert!(
                    node.get("marks")
                        .is_none_or(|marks| marks.as_array().is_none_or(Vec::is_empty))
                );
                assert!(!name(node).starts_with("composer-"));
                for child in children(node) {
                    plain(child);
                }
            }
            plain(&document);
        }
    }
    #[test]
    fn structural_roundtrips_and_maps() {
        for source in [
            "",
            "\n\n",
            "plain 😀",
            "**bold** and *italic*",
            "- one\n- two",
            "1) a\n  - b\n2) c",
            "- [ ] a\n- [x] b",
            "> a\n> b\n\ntext",
            "## Heading",
            "```rs\nfn main() {}\n```",
            "---",
            "@file.rs $skill model:codex/custom \nnext",
            "hello **bold** world",
            "a *italic* word and `code` here",
            "struck ~~out~~ now",
            "**`x`**",
            "*`x`*",
            "~~`x`~~",
            "**a `code` c**",
            "***bold italic*** keeps nesting",
            "trailing newline\n",
            "1. foo\n2. asdf\n",
            "-   [ ]  buy milk",
            "\t-\t[x]\t\titem",
            "- [ ]  ",
            "- [ ]\n  - [ ] child",
            "  - [ ] first\n - [ ] second\n  - [ ] child",
            "**before @README.md after**",
            "*a **b** c*",
            "*a**b***",
            "**a*b***",
            "**a *b* c**",
            "literal \u{fffc} **before @README.md after**",
            "- [ ] parent\n  - [ ] child\n  - [ ] sibling\n- [ ] uncle",
            "- [ ] empty task follows\n- [ ]",
            "- [ ] **bold** task with @README.md",
            "para\n- [ ] task\npara",
            "- [ ]No space stays literal",
            "-[ ] also literal",
            "@README.md explain this",
            "@\"docs/My File.md\" and $my-skill please",
            "snake_case stays literal",
            "unmatched ** stays literal",
            "**bold** then @README.md then *italic*",
        ] {
            let doc = build(source);
            let map = serialize_document(&doc);
            assert_eq!(map.value, source, "{source}");
            for offset in 0..=map.doc_length {
                assert_eq!(
                    map.pm_to_flat(map.flat_to_pm(offset)),
                    offset,
                    "{source} at {offset}"
                );
            }
        }
    }
    #[test]
    fn markdown_positions_clamp_markers_and_expand_atoms() {
        let map = serialize_document(&build("**😀** @foo "));
        assert_eq!(map.doc_length, 5);
        assert_eq!(map.markdown_to_flat(0), 0);
        assert_eq!(map.markdown_to_flat(2), 0);
        assert_eq!(map.markdown_to_flat(3), 1);
        assert_eq!(map.markdown_to_flat(5), 2);
        assert_eq!(map.markdown_to_flat(8), 4);
        assert_eq!(map.flat_to_markdown(0), 2);
        assert_eq!(map.flat_to_pm(0), 1);
    }
    #[test]
    fn crossing_marks_and_literal_mode() {
        for source in ["**a *b***", "**a*b***", "*a**b***", "*a **b** c*"] {
            let map = serialize_document(&build(source));
            assert_eq!(map.value, source);
        }
        let literal = build_document(
            "**a**\n@foo ",
            BuildOptions {
                literal: true,
                ..Default::default()
            },
            |_| None,
            |_, _| false,
        );
        assert_eq!(serialize_document(&literal).value, "**a**\n@foo ");
        assert!(
            serialize_document(&literal)
                .runs
                .iter()
                .all(|r| r.kind != RunKind::Token)
        );
    }
}
