//! Live inline input rules run after the typed-pair handler and before ordinary insertion.
use super::{
    code_pairs::CodePairs,
    editor::{
        children, marks, name, node_size, splice_inline, textblocks, transform_marks,
    },
};
use crate::thread_views::{
    document::{Selection, utf16_byte_offset, utf16_len},
    rich_text::Mark,
};
use serde_json::{Value, json};
pub struct Rewrite {
    pub document:Value,
    pub selection:Selection,
    pub stored_marks:Vec<Value>,
}
fn mark_name(mark:Mark)->&'static str {
    match mark {Mark::Bold=>"bold",Mark::Italic=>"italic",Mark::Strike=>"strike",Mark::Code=>"code"}
}

fn source_touches(block: &Value, from: usize, to: usize) -> bool {
    let mut position = 0;
    children(block).iter().any(|node| {
        let end = position + node_size(node);
        let touches = from <= end
            && to >= position
            && node["marks"]
                .as_array()
                .is_some_and(|marks| marks.iter().any(|mark| mark["type"] == "markdownSource"));
        position = end;
        touches
    })
}
fn has_code(block: &Value, from: usize, to: usize) -> bool {
    let mut position = 0;
    children(block).iter().any(|node| {
        let end = position + node_size(node);
        let found = from < end && to > position && marks(node).contains(&Mark::Code);
        position = end;
        found
    })
}
fn rule(text: &str, marker: &str, code: bool) -> Option<(usize, usize)> {
    let close = text.len().checked_sub(marker.len())?;
    if !text.ends_with(marker) {
        return None;
    }
    for (open, _) in text[..close].match_indices(marker) {
        let start = open + marker.len();
        if start >= close || text[start..close].contains(marker.chars().next()?) {
            continue;
        }
        if code {
            if text[..open].ends_with('`') {
                continue;
            }
        } else if open > 0
            && !text[..open]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            continue;
        }
        if !code && text[start..close].chars().all(char::is_whitespace) {
            continue;
        }
        return Some((open, close));
    }
    None
}
pub fn type_text(
    pairs: &mut CodePairs,
    doc: &Value,
    selection: Selection,
    text: &str,
    stored_marks: &[Value],
    composing: bool,
) -> Option<Rewrite> {
    let known=stored_marks.iter().filter_map(|mark|match mark["type"].as_str(){Some("bold")=>Some(Mark::Bold),Some("italic")=>Some(Mark::Italic),Some("strike")=>Some(Mark::Strike),Some("code")=>Some(Mark::Code),_=>None}).collect::<Vec<_>>();
    if text == "`" && !stored_marks.iter().any(|mark|mark["type"]=="markdownSource")
        && let Some(rewrite) = pairs.type_tick(doc, selection, &known, composing)
    {
        return Some(Rewrite{document:rewrite.document,selection:rewrite.selection,stored_marks:rewrite.stored_marks.into_iter().map(|mark|json!({"type":mark_name(mark)})).collect()});
    }
    let (start, block) = textblocks(doc).into_iter().find(|(start, block)| {
        selection.from() >= *start && selection.to() <= *start + node_size(block) - 2
    })?;
    let effective = if name(&block) == "codeBlock" {
        &[][..]
    } else {
        stored_marks
    };
    let mut next = doc.clone();
    let inserted = if text.is_empty() {
        vec![]
    } else {
        vec![json!({"type":"text","text":text,"marks":effective})]
    };
    if !splice_inline(&mut next, selection.from(), selection.to(), &inserted) {
        return None;
    }
    let inserted_len = utf16_len(text);
    let deleted = selection.to() - selection.from();
    pairs.map(|position, bias| {
        if position > selection.from() && position < selection.to() {
            None
        } else if position > selection.to() || position == selection.to() && bias > 0 {
            Some(position - deleted + inserted_len)
        } else {
            Some(position)
        }
    });
    let caret = selection.from() + inserted_len;
    if composing || name(&block) == "codeBlock" {
        return Some(Rewrite {
            document: next,
            selection: Selection::caret(caret),
            stored_marks: effective.to_vec(),
        });
    }
    let (_, current) = textblocks(&next)
        .into_iter()
        .find(|(pos, _)| *pos == start)?;
    let content = children(&current)
        .iter()
        .map(|node| node["text"].as_str().unwrap_or("\u{fffc}"))
        .collect::<String>();
    let before = &content[..utf16_byte_offset(&content, caret - start)?];
    for (marker, mark) in [
        ("*", Mark::Italic),
        ("**", Mark::Bold),
        ("__", Mark::Bold),
        ("~~", Mark::Strike),
        ("`", Mark::Code),
    ] {
        let Some((open, close)) = rule(before, marker, mark == Mark::Code) else {
            continue;
        };
        let from = start + utf16_len(&before[..open]);
        let to = start + utf16_len(&before[..close]);
        let width = marker.len();
        if source_touches(&block, from - start, selection.to() - start) {
            continue;
        }
        if mark == Mark::Code {
            let mut ticks = 0;
            let mut pos = 0;
            for node in children(&block) {
                if pos >= from - start {
                    break;
                }
                if name(node) != "text" {
                    ticks = 0;
                } else if !marks(node).contains(&Mark::Code) {
                    let value = node["text"].as_str().unwrap_or_default();
                    let end = utf16_byte_offset(value, (from - start - pos).min(node_size(node)))?;
                    ticks += value[..end].chars().filter(|ch| *ch == '`').count();
                }
                pos += node_size(node);
            }
            if ticks % 2 == 1 {
                continue;
            }
        } else if has_code(&block, from - start, from - start + width)
            || has_code(&block, to - start, selection.from() - start)
        {
            continue;
        }
        if !splice_inline(&mut next, to, to + width, &[])
            || !splice_inline(&mut next, from, from + width, &[])
        {
            return None;
        }
        transform_marks(&mut next, from, to - width, |node| {
            let mut current = marks(node);
            if !current.contains(&mark) {
                current.push(mark);
            }
            current.sort_by_key(|mark| match mark {
                Mark::Strike => 0,
                Mark::Bold => 1,
                Mark::Italic => 2,
                Mark::Code => 3,
            });
            node["marks"]=current.iter().map(|mark|json!({"type":match mark{Mark::Strike=>"strike",Mark::Bold=>"bold",Mark::Italic=>"italic",Mark::Code=>"code"}})).collect::<Vec<_>>().into();
        });
        pairs.map(|position, _| {
            Some(
                position
                    - position.saturating_sub(to).min(width)
                    - position.saturating_sub(from).min(width),
            )
        });
        return Some(Rewrite {
            document: next,
            selection: Selection::caret(caret - width * 2),
            stored_marks: effective
                .iter()
                .filter(|value| value["type"] != mark_name(mark))
                .cloned()
                .collect(),
        });
    }
    Some(Rewrite {
        document: next,
        selection: Selection::caret(caret),
        stored_marks: effective.to_vec(),
    })
}
