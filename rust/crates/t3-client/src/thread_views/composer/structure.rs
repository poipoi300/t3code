//! Structural boundary keys operate on trees so inline payloads keep their identities.
use super::editor::{children, name, node_size};
use crate::thread_views::{document::Selection, lists::next_ordered_marker_text};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub document: Value,
    pub selection: Selection,
}
fn at<'a>(doc: &'a Value, path: &[usize]) -> Option<&'a Value> {
    path.iter()
        .try_fold(doc, |node, index| children(node).get(*index))
}
fn at_mut<'a>(doc: &'a mut Value, path: &[usize]) -> Option<&'a mut Value> {
    path.iter().try_fold(doc, |node, index| {
        node.get_mut("content")?.as_array_mut()?.get_mut(*index)
    })
}
fn replace(doc: &mut Value, path: &[usize], nodes: Vec<Value>) -> Option<()> {
    let (index, parent) = path.split_last()?;
    at_mut(doc, parent)?
        .get_mut("content")?
        .as_array_mut()?
        .splice(*index..=*index, nodes);
    Some(())
}
fn blocks(doc: &Value) -> Vec<(Vec<usize>, usize)> {
    fn walk(
        node: &Value,
        pos: usize,
        path: &mut Vec<usize>,
        output: &mut Vec<(Vec<usize>, usize)>,
    ) {
        if matches!(name(node), "paragraph" | "heading" | "codeBlock") {
            output.push((path.clone(), pos + 1));
            return;
        }
        let mut pos = pos + usize::from(name(node) != "doc");
        for (index, child) in children(node).iter().enumerate() {
            path.push(index);
            walk(child, pos, path, output);
            path.pop();
            pos += node_size(child);
        }
    }
    let mut result = Vec::new();
    walk(doc, 0, &mut Vec::new(), &mut result);
    result
}
fn is_item(node: &Value) -> bool {
    matches!(name(node), "listItem" | "taskItem")
}
fn is_list(node: &Value) -> bool {
    matches!(name(node), "bulletList" | "orderedList" | "taskList")
}
fn list_item_path<'a>(doc: &Value, path: &'a [usize]) -> Option<&'a [usize]> {
    let parent = path.get(..path.len().checked_sub(1)?)?;
    is_item(at(doc, parent)?).then_some(parent)
}
fn prune(node: &mut Value) {
    if let Some(content) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in content.iter_mut() {
            prune(child)
        }
        content.retain(|child| !((is_item(child) || is_list(child)) && children(child).is_empty()));
    }
}
fn join(doc: &Value, left: &(Vec<usize>, usize), right: &(Vec<usize>, usize)) -> Option<Rewrite> {
    let mut next = doc.clone();
    let left_node = at(doc, &left.0)?;
    let right_node = at(doc, &right.0)?;
    let caret = left.1 + node_size(left_node) - 2;
    let mut content = children(left_node).to_vec();
    content.extend_from_slice(children(right_node));
    at_mut(&mut next, &left.0)?["content"] = json!(content);
    replace(&mut next, &right.0, Vec::new())?;
    prune(&mut next);
    Some(Rewrite {
        document: next,
        selection: Selection::caret(caret),
    })
}
fn align(list: &mut Value, index: usize) -> Option<()> {
    let items = list.get_mut("content")?.as_array_mut()?;
    let sibling = items
        .get(if index > 0 { index - 1 } else { index + 1 })?
        .clone();
    let item = items.get_mut(index)?;
    item["attrs"]["indent"] = sibling["attrs"]["indent"].clone();
    if name(item) == "taskItem" {
        item["attrs"]["markerSpace"] = sibling["attrs"]["markerSpace"].clone()
    } else {
        let marker = sibling["attrs"]["marker"].as_str().unwrap_or("-");
        let ordered = marker.as_bytes().first().is_some_and(u8::is_ascii_digit);
        item["attrs"]["marker"] = json!(if index > 0 && ordered {
            next_ordered_marker_text(marker)
        } else {
            marker.into()
        });
        if ordered {
            let mut prior = item["attrs"]["marker"].as_str()?.to_owned();
            for item in items.iter_mut().skip(index + 1) {
                prior = next_ordered_marker_text(&prior);
                item["attrs"]["marker"] = json!(prior);
            }
        }
    }
    Some(())
}
fn lift(doc: &Value, path: &[usize]) -> Option<Rewrite> {
    let item_path = list_item_path(doc, path)?;
    if path.last() != Some(&0) {
        return None;
    }
    let (&index, list_path) = item_path.split_last()?;
    let list = at(doc, list_path)?;
    let item = at(doc, item_path)?;
    let mut lifted = item.clone();
    let before = children(list)[..index].to_vec();
    let after = children(list)[index + 1..].to_vec();
    let mut next = doc.clone();
    let new_path = if list_path.len() >= 2 && is_item(at(doc, &list_path[..list_path.len() - 1])?) {
        let parent_path = &list_path[..list_path.len() - 1];
        let parent = at(doc, parent_path)?;
        if name(parent) != name(item) {
            return None;
        }
        let (&parent_index, outer_path) = parent_path.split_last()?;
        let mut parent = parent.clone();
        if before.is_empty() {
            replace(&mut parent, &[*list_path.last()?], Vec::new())?;
        } else {
            at_mut(&mut parent, &[*list_path.last()?])?["content"] = json!(before);
        }
        if !after.is_empty() {
            let mut remainder = list.clone();
            remainder["content"] = json!(after);
            lifted.get_mut("content")?.as_array_mut()?.push(remainder);
        }
        let outer = at_mut(&mut next, outer_path)?;
        let items = outer.get_mut("content")?.as_array_mut()?;
        items.splice(parent_index..=parent_index, [parent, lifted]);
        align(outer, parent_index + 1);
        let mut p = outer_path.to_vec();
        p.extend([parent_index + 1, 0]);
        p
    } else {
        let (&list_index, container) = list_path.split_last()?;
        let mut replacements = Vec::new();
        if !before.is_empty() {
            let mut prior = list.clone();
            prior["content"] = json!(before);
            replacements.push(prior);
        }
        let paragraph_index = list_index + replacements.len();
        replacements.extend_from_slice(children(item));
        if !after.is_empty() {
            let mut rest = list.clone();
            rest["content"] = json!(after);
            replacements.push(rest);
        }
        replace(&mut next, list_path, replacements)?;
        let mut p = container.to_vec();
        p.push(paragraph_index);
        p
    };
    let caret = blocks(&next).into_iter().find(|(p, _)| *p == new_path)?.1;
    Some(Rewrite {
        document: next,
        selection: Selection::caret(caret),
    })
}

pub fn backspace(doc: &Value, selection: Selection) -> Option<Rewrite> {
    if !selection.collapsed() {
        return None;
    }
    let rows = blocks(doc);
    let index = rows
        .iter()
        .position(|(_, start)| *start == selection.head)?;
    let row = &rows[index];
    let node = at(doc, &row.0)?;
    if name(node) == "codeBlock" {
        let text = children(node)
            .iter()
            .filter_map(|node| node["text"].as_str())
            .collect::<String>();
        let paragraphs=text.split('\n').map(|line|json!({"type":"paragraph","content":if line.is_empty(){vec![]}else{vec![json!({"type":"text","text":line})]}})).collect();
        let mut next = doc.clone();
        replace(&mut next, &row.0, paragraphs)?;
        return Some(Rewrite {
            document: next,
            selection,
        });
    }
    if let Some(result) = lift(doc, &row.0) {
        return Some(result);
    }
    let previous = rows.get(index.checked_sub(1)?)?;
    if list_item_path(doc, &row.0).is_some()
        || (row.0.len() == 1
            && rows
                .get(index - 1)
                .is_some_and(|(path, _)| list_item_path(doc, path).is_some()))
    {
        return join(doc, previous, row);
    }
    None
}
pub fn delete(doc: &Value, selection: Selection) -> Option<Rewrite> {
    if !selection.collapsed() {
        return None;
    }
    let rows = blocks(doc);
    let index = rows.iter().position(|(path, start)| {
        at(doc, path).is_some_and(|node| start + node_size(node) - 2 == selection.head)
    })?;
    let current = &rows[index];
    let node = at(doc, &current.0)?;
    let Some(next) = rows.get(index + 1) else {
        return (node_size(node) == 2 && list_item_path(doc, &current.0).is_some()).then(|| {
            Rewrite {
                document: doc.clone(),
                selection,
            }
        });
    };
    if name(at(doc, &next.0)?) == "codeBlock" {
        return (name(node) != "codeBlock").then(|| Rewrite {
            document: doc.clone(),
            selection,
        });
    }
    if list_item_path(doc, &current.0).is_none() && list_item_path(doc, &next.0).is_none() {
        return None;
    }
    join(doc, current, next)
}
pub fn enter(doc: &Value, selection: Selection) -> Option<Rewrite> {
    use crate::thread_views::{
        code::{CodeBlock, CodeResult},
        document::Fence,
    };
    let rows = blocks(doc);
    let (path, start) = rows.iter().find(|(path, start)| {
        at(doc, path).is_some_and(|node| {
            selection.from() >= *start && selection.to() <= start + node_size(node) - 2
        })
    })?;
    let node = at(doc, path)?;
    let offset = selection.from() - start;
    let end = selection.to() - start;
    if name(node) == "codeBlock" {
        let block = CodeBlock {
            opening: Fence {
                fence: node["attrs"]["fence"].as_str().unwrap_or("```").into(),
                language: node["attrs"]["language"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            },
            text: children(node)
                .iter()
                .filter_map(|node| node["text"].as_str())
                .collect(),
            close: node["attrs"]["close"].as_str().unwrap_or_default().into(),
        };
        let relative = Selection {
            anchor: selection.anchor - start,
            head: selection.head - start,
        };
        let mut next = doc.clone();
        let code_node = |block: &CodeBlock| json!({"type":"codeBlock","attrs":{"fence":block.opening.fence,"language":block.opening.language,"close":block.close},"content":if block.text.is_empty(){vec![]}else{vec![json!({"type":"text","text":block.text})]}});
        let paragraph = |line: &str| json!({"type":"paragraph","content":if line.is_empty(){vec![]}else{vec![json!({"type":"text","text":line})]}});
        let caret = match block.enter(relative)? {
            CodeResult::Edited {
                block,
                selection: relative,
            } => {
                replace(&mut next, path, vec![code_node(&block)])?;
                Selection {
                    anchor: start + relative.anchor,
                    head: start + relative.head,
                }
            }
            CodeResult::Exited { block, paragraphs } => {
                let code = code_node(&block);
                let caret = start + node_size(&code);
                let mut nodes = vec![code];
                nodes.extend(paragraphs.iter().map(|line| paragraph(line)));
                replace(&mut next, path, nodes)?;
                Selection::caret(caret)
            }
        };
        return Some(Rewrite {
            document: next,
            selection: caret,
        });
    }
    if node_size(node) == 2 && selection.collapsed() && list_item_path(doc, path).is_some() {
        if let Some(result) = lift(doc, path) {
            return Some(result);
        }
        if path.len() != 5 {
            return None;
        }
        let outer = at(doc, &path[..1])?;
        let parent = at(doc, &path[..2])?;
        let nested = at(doc, &path[..3])?;
        if path[3] + 1 != children(nested).len() || path[2] + 1 != children(parent).len() {
            return None;
        }
        let mut before = outer.clone();
        before["content"] = json!(&children(outer)[..=path[1]]);
        replace(&mut before, &path[1..4], Vec::new())?;
        prune(&mut before);
        let mut nodes = vec![before.clone(), json!({"type":"paragraph","content":[]})];
        if path[1] + 1 < children(outer).len() {
            let mut after = outer.clone();
            after["content"] = json!(&children(outer)[path[1] + 1..]);
            nodes.push(after);
        }
        let mut next = doc.clone();
        replace(&mut next, &path[..1], nodes)?;
        let preceding = children(doc)[..path[0]]
            .iter()
            .map(node_size)
            .sum::<usize>();
        return Some(Rewrite {
            document: next,
            selection: Selection::caret(preceding + node_size(&before) + 1),
        });
    }
    fn content(node: &Value, from: usize, to: usize) -> Option<Vec<Value>> {
        let mut result = Vec::new();
        let mut pos = 0;
        for child in children(node) {
            let size = node_size(child);
            if from < pos + size && to > pos {
                result.push(super::editor::cut(
                    child,
                    from.saturating_sub(pos),
                    (to - pos).min(size),
                )?);
            }
            pos += size;
        }
        Some(result)
    }
    let left = content(node, 0, offset)?;
    let right = content(node, end, node_size(node) - 2)?;
    let mut before = node.clone();
    before["content"] = json!(left);
    let mut after = node.clone();
    after["content"] = json!(right);
    let mut next = doc.clone();
    let mut new_path = path.clone();
    if let Some(item_path) = list_item_path(doc, path) {
        let (&index, list_path) = item_path.split_last()?;
        let item = at(doc, item_path)?;
        let mut prior = item.clone();
        prior["content"] = json!([before]);
        let mut following = item.clone();
        let mut following_content = vec![after];
        following_content.extend_from_slice(&children(item)[1..]);
        following["content"] = json!(following_content);
        if name(item) == "taskItem" {
            following["attrs"]["checked"] = json!(false)
        } else {
            following["attrs"]["space"] = json!(" ")
        }
        replace(&mut next, item_path, vec![prior, following])?;
        let _ = align(at_mut(&mut next, list_path)?, index + 1);
        new_path = list_path.to_vec();
        new_path.extend([index + 1, 0]);
    } else {
        if name(node) == "heading" && offset == 0 {
            before = json!({"type":"paragraph","content":[]});
        }
        if name(node) == "heading" && offset == node_size(node) - 2 {
            after = json!({"type":"paragraph","content":[]});
        }
        replace(&mut next, path, vec![before, after])?;
        *new_path.last_mut()? += 1;
    }
    let caret = blocks(&next)
        .into_iter()
        .find(|(path, _)| *path == new_path)?
        .1;
    Some(Rewrite {
        document: next,
        selection: Selection::caret(caret),
    })
}
