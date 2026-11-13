use crate::view_logic::JsTrim;
use serde::{Deserialize, Serialize};
use serde_json::json;
use t3_contracts::{
    composer_context::PullRequestContextMetadata, pull_request::PullRequestReviewPosition,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectionSide {
    Additions,
    Deletions,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedLineRange {
    pub start: usize,
    pub end: usize,
    pub side: Option<SelectionSide>,
    pub end_side: Option<SelectionSide>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCommentContext {
    pub id: String,
    pub section_id: String,
    pub section_title: String,
    pub file_path: String,
    pub start_index: usize,
    pub end_index: usize,
    pub range_label: String,
    pub text: String,
    pub diff: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectedLineRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequestContextMetadata>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HunkContent {
    Context { lines: usize },
    Change { deletions: usize, additions: usize },
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewHunk {
    pub deletion_start: usize,
    pub deletion_count: usize,
    pub addition_start: usize,
    pub addition_count: usize,
    pub deletion_line_index: usize,
    pub addition_line_index: usize,
    pub hunk_content: Vec<HunkContent>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffMetadata {
    pub hunks: Vec<ReviewHunk>,
    pub addition_lines: Vec<String>,
    pub deletion_lines: Vec<String>,
    #[serde(default)]
    pub is_partial: bool,
}
#[derive(Debug, Clone, Copy)]
struct RowSpan {
    index: usize,
    count: usize,
    old: Option<usize>,
    new: Option<usize>,
    old_index: usize,
    new_index: usize,
}
fn spans(file: &FileDiffMetadata, expanded: bool) -> Vec<RowSpan> {
    let mut result = vec![];
    let (mut index, mut old, mut new) = (0, 1_usize, 1_usize);
    let mut push = |count, old, new, old_index, new_index| {
        result.push(RowSpan {
            index,
            count,
            old,
            new,
            old_index,
            new_index,
        });
        index += count;
    };
    for h in &file.hunks {
        if expanded {
            let count = (h.deletion_start + usize::from(h.deletion_count == 0))
                .saturating_sub(old)
                .min((h.addition_start + usize::from(h.addition_count == 0)).saturating_sub(new));
            push(
                count,
                Some(old),
                Some(new),
                old.saturating_sub(1),
                new.saturating_sub(1),
            );
        }
        let (mut a, mut b, mut ai, mut bi) = (
            h.deletion_start,
            h.addition_start,
            h.deletion_line_index,
            h.addition_line_index,
        );
        for segment in &h.hunk_content {
            match *segment {
                HunkContent::Context { lines } => {
                    push(lines, Some(a), Some(b), ai, bi);
                    a += lines;
                    b += lines;
                    ai += lines;
                    bi += lines;
                }
                HunkContent::Change {
                    deletions,
                    additions,
                } => {
                    push(deletions, Some(a), None, ai, bi);
                    push(additions, None, Some(b), ai + deletions, bi);
                    a += deletions;
                    b += additions;
                    ai += deletions;
                    bi += additions;
                }
            }
        }
        old = h.deletion_start + h.deletion_count + usize::from(h.deletion_count == 0);
        new = h.addition_start + h.addition_count + usize::from(h.addition_count == 0);
    }
    if expanded {
        push(
            (file.deletion_lines.len() + 1)
                .saturating_sub(old)
                .min((file.addition_lines.len() + 1).saturating_sub(new)),
            Some(old),
            Some(new),
            old.saturating_sub(1),
            new.saturating_sub(1),
        );
    }
    result
}
#[derive(Debug)]
struct Row {
    old: Option<usize>,
    new: Option<usize>,
    content: String,
}
fn row(file: &FileDiffMetadata, index: usize) -> Option<Row> {
    let span = spans(file, !file.is_partial)
        .into_iter()
        .find(|s| index >= s.index && index - s.index < s.count)?;
    Some(span_row(file, &span, index))
}
fn span_row(file: &FileDiffMetadata, span: &RowSpan, index: usize) -> Row {
    let offset = index - span.index;
    let content = span
        .new
        .and_then(|_| file.addition_lines.get(span.new_index + offset))
        .or_else(|| {
            span.old
                .and_then(|_| file.deletion_lines.get(span.old_index + offset))
        })
        .map_or("", String::as_str);
    Row {
        old: span.old.map(|n| n + offset),
        new: span.new.map(|n| n + offset),
        content: content.strip_suffix('\n').unwrap_or(content).into(),
    }
}
fn find_index(file: &FileDiffMetadata, line: usize, side: Option<SelectionSide>) -> Option<usize> {
    let spans = spans(file, !file.is_partial);
    let find = |side| {
        spans.iter().find_map(|s| {
            let start = if side == SelectionSide::Deletions {
                s.old
            } else {
                s.new
            }?;
            (line >= start && line - start < s.count).then(|| s.index + line - start)
        })
    };
    let side = side.unwrap_or(SelectionSide::Additions);
    find(side).or_else(|| {
        find(if side == SelectionSide::Additions {
            SelectionSide::Deletions
        } else {
            SelectionSide::Additions
        })
    })
}
pub fn infer_review_comment_fence_language(path: &str) -> String {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_lowercase();
    name.rsplit_once('.')
        .filter(|(_, ext)| !ext.is_empty())
        .map_or("text".into(), |(_, ext)| ext.into())
}
pub fn format_review_comment_fence(language: &str, contents: &str) -> String {
    let longest = contents
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(3.max(longest + 1));
    format!("{fence}{language}\n{}\n{fence}", contents.js_trim_end())
}
pub fn build_file_review_comment(
    id: &str,
    path: &str,
    start: i64,
    end: i64,
    text: &str,
    contents: &str,
) -> ReviewCommentContext {
    let first = start.min(end).max(1) as usize;
    let last = start.max(end).max(first as i64) as usize;
    ReviewCommentContext {
        id: id.into(),
        section_id: format!("file:{path}"),
        section_title: "File comment".into(),
        file_path: path.into(),
        start_index: first - 1,
        end_index: last - 1,
        range_label: if first == last {
            format!("L{first}")
        } else {
            format!("L{first} to L{last}")
        },
        text: text.js_trim().into(),
        diff: contents
            .split('\n')
            .skip(first - 1)
            .take(last - first + 1)
            .collect::<Vec<_>>()
            .join("\n"),
        fence_language: Some(infer_review_comment_fence_language(path)),
        selection: None,
        pull_request: None,
    }
}
pub fn restore_diff_review_comment_range(
    file: &FileDiffMetadata,
    comment: &ReviewCommentContext,
) -> Option<SelectedLineRange> {
    if let Some(selection) = &comment.selection {
        return Some(selection.clone());
    }
    let point = |index| {
        let r = row(file, index)?;
        r.new
            .map(|n| (n, SelectionSide::Additions))
            .or_else(|| r.old.map(|n| (n, SelectionSide::Deletions)))
    };
    let (start, side) = point(comment.start_index)?;
    let (end, end_side) = point(comment.end_index)?;
    Some(SelectedLineRange {
        start,
        end,
        side: Some(side),
        end_side: Some(end_side),
    })
}
pub fn resolve_diff_review_position(
    file: &FileDiffMetadata,
    line: usize,
    side: Option<SelectionSide>,
) -> Option<PullRequestReviewPosition> {
    let r = row(file, find_index(file, line, side)?)?;
    if (side == Some(SelectionSide::Additions) && r.new != Some(line))
        || (side == Some(SelectionSide::Deletions) && r.old != Some(line))
    {
        return None;
    }
    let value = match (r.old, r.new) {
        (None, Some(new)) => json!({"kind":"added","newLine":new}),
        (Some(old), None) => json!({"kind":"deleted","oldLine":old}),
        (Some(old), Some(new))
            if file
                .hunks
                .iter()
                .any(|h| new >= h.addition_start && new - h.addition_start < h.addition_count) =>
        {
            json!({"kind":"context","oldLine":old,"newLine":new,"side":if side==Some(SelectionSide::Deletions) {"left"} else {"right"}})
        }
        _ => return None,
    };
    serde_json::from_value(value).ok()
}
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffReviewAnchor {
    pub position: PullRequestReviewPosition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_position: Option<PullRequestReviewPosition>,
}
fn hunk_index(file: &FileDiffMetadata, position: &PullRequestReviewPosition) -> Option<usize> {
    let value = super::work::wire_value(position);
    let left = value["kind"] == "deleted";
    let line = value[if left { "oldLine" } else { "newLine" }].as_u64()? as usize;
    file.hunks.iter().position(|h| {
        let (start, count) = if left {
            (h.deletion_start, h.deletion_count)
        } else {
            (h.addition_start, h.addition_count)
        };
        line >= start && line - start < count
    })
}
pub fn resolve_diff_review_anchor(
    file: &FileDiffMetadata,
    range: &SelectedLineRange,
    ranged: bool,
) -> Option<DiffReviewAnchor> {
    let last = resolve_diff_review_position(file, range.end, range.end_side.or(range.side))?;
    let first = ranged
        .then(|| resolve_diff_review_position(file, range.start, range.side))
        .flatten();
    let fallback = || DiffReviewAnchor {
        position: last.clone(),
        start_position: None,
    };
    let Some(first) = first else {
        return Some(fallback());
    };
    let (Some(start_index), Some(end_index)) = (
        find_index(file, range.start, range.side),
        find_index(file, range.end, range.end_side.or(range.side)),
    ) else {
        return Some(fallback());
    };
    if start_index == end_index {
        return Some(fallback());
    }
    let (start, end) = if start_index < end_index {
        (first, last.clone())
    } else {
        (last.clone(), first)
    };
    Some(
        if hunk_index(file, &end).is_some() && hunk_index(file, &end) == hunk_index(file, &start) {
            DiffReviewAnchor {
                position: end,
                start_position: Some(start),
            }
        } else {
            fallback()
        },
    )
}
pub fn build_diff_review_comment(
    id: &str,
    section_id: &str,
    section_title: &str,
    path: &str,
    file: &FileDiffMetadata,
    range: &SelectedLineRange,
    text: &str,
) -> Option<ReviewCommentContext> {
    let a = find_index(file, range.start, range.side)?;
    let b = find_index(file, range.end, range.end_side.or(range.side))?;
    let (start, end) = (a.min(b), a.max(b));
    let rows: Vec<_> = spans(file, !file.is_partial)
        .iter()
        .flat_map(|span| {
            (start.max(span.index)..end.saturating_add(1).min(span.index + span.count))
                .map(move |index| span_row(file, span, index))
        })
        .collect();
    let first = rows.first()?;
    let last = rows.last()?;
    let marker = |r: &Row| match (r.old, r.new) {
        (None, _) => "+",
        (_, None) => "-",
        _ => " ",
    };
    let mark = if rows.iter().all(|r| marker(r) == marker(first)) {
        marker(first).js_trim()
    } else {
        ""
    };
    let label = match (first.new.or(first.old), last.new.or(last.old)) {
        (Some(a), Some(b)) if a == b => format!("{mark}{a}"),
        (Some(a), Some(b)) => format!("{mark}{a} to {mark}{b}"),
        _ => "line".into(),
    };
    let old: Vec<_> = rows.iter().filter_map(|r| r.old).collect();
    let new: Vec<_> = rows.iter().filter_map(|r| r.new).collect();
    let diff = std::iter::once(format!(
        "@@ -{},{} +{},{} @@",
        old.first().copied().unwrap_or(0),
        old.len(),
        new.first().copied().unwrap_or(0),
        new.len()
    ))
    .chain(rows.iter().map(|r| format!("{}{}", marker(r), r.content)))
    .collect::<Vec<_>>()
    .join("\n");
    Some(ReviewCommentContext {
        id: id.into(),
        section_id: section_id.into(),
        section_title: section_title.into(),
        file_path: path.into(),
        start_index: start,
        end_index: end,
        range_label: label,
        text: text.js_trim().into(),
        diff,
        fence_language: Some("diff".into()),
        selection: Some(SelectedLineRange {
            side: Some(range.side.unwrap_or(SelectionSide::Additions)),
            end_side: Some(
                range
                    .end_side
                    .or(range.side)
                    .unwrap_or(SelectionSide::Additions),
            ),
            ..range.clone()
        }),
        pull_request: None,
    })
}
pub fn build_review_comment_renderable_patch(comment: &ReviewCommentContext) -> String {
    if comment.fence_language.as_deref().unwrap_or("diff") != "diff"
        || comment.diff.js_trim().is_empty()
    {
        return String::new();
    }
    let diff = comment.diff.js_trim_start().trim_end_matches('\n');
    if diff.starts_with("diff --git ") {
        return diff.into();
    }
    let path = comment.file_path.replace('\\', "/");
    format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{diff}")
}
