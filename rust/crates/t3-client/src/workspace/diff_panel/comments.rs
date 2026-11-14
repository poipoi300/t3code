//! Review comments on the diff panel's lines: AnnotatableCodeView.tsx's use of
//! reviewCommentContext.ts (`buildDiffReviewComment`, `restoreDiffReviewCommentRange`) and of the
//! composer draft's `reviewComments`.
//!
//! The view numbers lines from 0 across the shown files' excerpts (each side holds a hunk's
//! context lines and its deleted or added lines); comments carry the files' own line numbers.
//! [`ExcerptMap`] converts between the two.

use super::files::{HunkLines, PatchFile};
use crate::view_logic::session::review::{
    FileDiffMetadata, HunkContent, ReviewCommentContext, ReviewHunk, SelectedLineRange,
    SelectionSide, build_diff_review_comment, restore_diff_review_comment_range,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

impl Side {
    fn selection(self) -> SelectionSide {
        match self {
            Self::Old => SelectionSide::Deletions,
            Self::New => SelectionSide::Additions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileExcerpt {
    old_at: usize,
    new_at: usize,
    old_len: usize,
    new_len: usize,
    hunks: Vec<HunkLines>,
}

/// Where each shown file's lines are in the view's joined texts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExcerptMap {
    files: Vec<FileExcerpt>,
}

impl ExcerptMap {
    pub fn new<'a>(files: impl IntoIterator<Item = &'a PatchFile>) -> Self {
        let (mut old_at, mut new_at) = (0, 0);
        let files = files
            .into_iter()
            .map(|file| {
                let old_len = file.hunk_lines.iter().map(|h| h.old_lines as usize).sum();
                let new_len = file.hunk_lines.iter().map(|h| h.new_lines as usize).sum();
                let excerpt = FileExcerpt {
                    old_at,
                    new_at,
                    old_len,
                    new_len,
                    hunks: file.hunk_lines.clone(),
                };
                old_at += old_len;
                new_at += new_len;
                excerpt
            })
            .collect();
        Self { files }
    }

    /// The shown file and its own line number at the view's `line` of `side`.
    pub fn to_file(&self, side: Side, line: usize) -> Option<(usize, usize)> {
        self.files.iter().enumerate().find_map(|(index, file)| {
            let (at, len) = match side {
                Side::Old => (file.old_at, file.old_len),
                Side::New => (file.new_at, file.new_len),
            };
            if line < at || line >= at + len {
                return None;
            }
            let mut offset = line - at;
            for hunk in &file.hunks {
                let (start, count) = match side {
                    Side::Old => (hunk.old_start, hunk.old_lines),
                    Side::New => (hunk.new_start, hunk.new_lines),
                };
                if offset < count as usize {
                    return Some((index, start as usize + offset));
                }
                offset -= count as usize;
            }
            None
        })
    }

    /// The view's line of a shown file's own line number on `side`.
    pub fn to_view(&self, file: usize, side: Side, number: usize) -> Option<usize> {
        let excerpt = self.files.get(file)?;
        let mut at = match side {
            Side::Old => excerpt.old_at,
            Side::New => excerpt.new_at,
        };
        for hunk in &excerpt.hunks {
            let (start, count) = match side {
                Side::Old => (hunk.old_start as usize, hunk.old_lines as usize),
                Side::New => (hunk.new_start as usize, hunk.new_lines as usize),
            };
            if number >= start && number < start + count {
                return Some(at + number - start);
            }
            at += count;
        }
        None
    }

    /// A selection of the view's lines `start..=end` on `side`, as the file's selected range.
    /// Lines in two files are no selection.
    pub fn selection(
        &self,
        side: Side,
        start: usize,
        end: usize,
    ) -> Option<(usize, SelectedLineRange)> {
        let (file, first) = self.to_file(side, start)?;
        let (end_file, last) = self.to_file(side, end)?;
        (file == end_file).then_some((
            file,
            SelectedLineRange {
                start: first,
                end: last,
                side: Some(side.selection()),
                end_side: Some(side.selection()),
            },
        ))
    }
}

/// The file's hunks as @pierre/diffs reads a partial patch, for `buildDiffReviewComment`.
pub fn file_diff_metadata(patch: &str) -> FileDiffMetadata {
    let mut metadata = FileDiffMetadata {
        hunks: Vec::new(),
        addition_lines: Vec::new(),
        deletion_lines: Vec::new(),
        is_partial: true,
    };
    let mut current: Option<(ReviewHunk, u64, u64)> = None;
    let finish = |metadata: &mut FileDiffMetadata, current: Option<(ReviewHunk, u64, u64)>| {
        if let Some((hunk, _, _)) = current {
            metadata.hunks.push(hunk);
        }
    };
    for line in patch.split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("@@ -") {
            finish(&mut metadata, current.take());
            let Some((old, rest)) = header.split_once(" +") else {
                continue;
            };
            let Some((new, _)) = rest.split_once(" @@") else {
                continue;
            };
            let range = |value: &str| {
                let (start, count) = value.split_once(',').unwrap_or((value, "1"));
                Some((start.parse::<u64>().ok()?, count.parse::<u64>().ok()?))
            };
            let (Some((old_start, old_count)), Some((new_start, new_count))) =
                (range(old), range(new))
            else {
                continue;
            };
            current = Some((
                ReviewHunk {
                    deletion_start: old_start as usize,
                    deletion_count: old_count as usize,
                    addition_start: new_start as usize,
                    addition_count: new_count as usize,
                    deletion_line_index: metadata.deletion_lines.len(),
                    addition_line_index: metadata.addition_lines.len(),
                    hunk_content: Vec::new(),
                },
                old_count,
                new_count,
            ));
            continue;
        }
        let Some((hunk, old_left, new_left)) = current.as_mut() else {
            continue;
        };
        let content = |line: &str| {
            let rest = &line[1..];
            if rest.is_empty() {
                "\n".to_owned()
            } else {
                rest.to_owned()
            }
        };
        match line.as_bytes().first() {
            Some(b' ') if *old_left > 0 && *new_left > 0 => {
                *old_left -= 1;
                *new_left -= 1;
                metadata.deletion_lines.push(content(line));
                metadata.addition_lines.push(content(line));
                match hunk.hunk_content.last_mut() {
                    Some(HunkContent::Context { lines }) => *lines += 1,
                    _ => hunk.hunk_content.push(HunkContent::Context { lines: 1 }),
                }
            }
            Some(b'-') if *old_left > 0 => {
                *old_left -= 1;
                metadata.deletion_lines.push(content(line));
                match hunk.hunk_content.last_mut() {
                    Some(HunkContent::Change { deletions, .. }) => *deletions += 1,
                    _ => hunk.hunk_content.push(HunkContent::Change {
                        deletions: 1,
                        additions: 0,
                    }),
                }
            }
            Some(b'+') if *new_left > 0 => {
                *new_left -= 1;
                metadata.addition_lines.push(content(line));
                match hunk.hunk_content.last_mut() {
                    Some(HunkContent::Change { additions, .. }) => *additions += 1,
                    _ => hunk.hunk_content.push(HunkContent::Change {
                        deletions: 0,
                        additions: 1,
                    }),
                }
            }
            _ => {}
        }
    }
    finish(&mut metadata, current);
    metadata
}

/// An annotation the view draws under a line: a saved comment, or the draft being written.
#[derive(Debug, Clone, PartialEq)]
pub struct CommentAnnotation {
    pub id: String,
    pub side: Side,
    /// The view's line, from 0.
    pub line: usize,
    pub range_label: String,
    pub text: String,
}

/// `annotationSide`: a comment shows under its last line's side, the new one by default.
fn annotation_side(range: &SelectedLineRange) -> Side {
    match range.end_side.or(range.side) {
        Some(SelectionSide::Deletions) => Side::Old,
        _ => Side::New,
    }
}

/// The saved comments of a section as the view's annotations (AnnotatableCodeView.tsx's
/// `persisted`): diff comments on shown files whose range still maps to a shown line.
pub fn comment_annotations(
    comments: &[ReviewCommentContext],
    section_id: &str,
    shown: &[&PatchFile],
    source: &str,
    map: &ExcerptMap,
) -> Vec<CommentAnnotation> {
    comments
        .iter()
        .filter(|c| {
            c.section_id == section_id && c.fence_language.as_deref().unwrap_or("diff") == "diff"
        })
        .filter_map(|comment| {
            let index = shown.iter().position(|f| f.path() == comment.file_path)?;
            let range = match &comment.selection {
                Some(selection) => selection.clone(),
                None => {
                    let metadata = file_diff_metadata(&source[shown[index].range.clone()]);
                    restore_diff_review_comment_range(&metadata, comment)?
                }
            };
            let side = annotation_side(&range);
            Some(CommentAnnotation {
                id: comment.id.clone(),
                side,
                line: map.to_view(index, side, range.end)?,
                range_label: comment.range_label.clone(),
                text: comment.text.clone(),
            })
        })
        .collect()
}

/// `buildDiffReviewComment` for the view's lines `start..=end` of `side` in a shown file.
#[allow(clippy::too_many_arguments)]
pub fn review_comment(
    id: &str,
    section_id: &str,
    section_title: &str,
    shown: &[&PatchFile],
    source: &str,
    map: &ExcerptMap,
    side: Side,
    (start, end): (usize, usize),
    text: &str,
) -> Option<ReviewCommentContext> {
    let (file, range) = map.selection(side, start.min(end), start.max(end))?;
    let file = shown.get(file)?;
    let metadata = file_diff_metadata(&source[file.range.clone()]);
    build_diff_review_comment(
        id,
        section_id,
        section_title,
        file.path(),
        &metadata,
        &range,
        text,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::diff_panel::files::{RenderablePatch, renderable_patch};
    use std::rc::Rc;

    const PATCH: &str = "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -10,3 +10,3 @@\n keep\n-old\n+new\n tail\ndiff --git a/b.ts b/b.ts\n--- a/b.ts\n+++ b/b.ts\n@@ -1,0 +1,2 @@\n+one\n+two\n";

    fn files() -> Rc<crate::workspace::diff_panel::files::PatchFiles> {
        match renderable_patch(&Rc::from(PATCH)) {
            Some(RenderablePatch::Files(files)) => files,
            _ => panic!("files"),
        }
    }

    #[test]
    fn view_lines_map_to_each_files_own_numbers_and_back() {
        let files = files();
        let map = ExcerptMap::new(&files.files);
        // a.ts: old shows keep/old/tail (10..12), new shows keep/new/tail (10..12).
        assert_eq!(map.to_file(Side::New, 1), Some((0, 11)));
        assert_eq!(map.to_file(Side::Old, 2), Some((0, 12)));
        // b.ts's new lines follow a.ts's three.
        assert_eq!(map.to_file(Side::New, 3), Some((1, 1)));
        assert_eq!(map.to_file(Side::New, 4), Some((1, 2)));
        assert_eq!(map.to_file(Side::Old, 3), None);
        assert_eq!(map.to_view(1, Side::New, 2), Some(4));
        assert_eq!(map.to_view(0, Side::Old, 11), Some(1));
        assert_eq!(map.selection(Side::New, 2, 3), None);
    }

    #[test]
    fn a_comment_round_trips_through_the_view_lines() {
        let files = files();
        let map = ExcerptMap::new(&files.files);
        let shown: Vec<_> = files.files.iter().collect();
        let comment = review_comment(
            "c1",
            "turn:r1",
            "Turn 1",
            &shown,
            &files.source,
            &map,
            Side::New,
            (1, 1),
            "  Why this?  ",
        )
        .expect("a comment on a changed line");
        assert_eq!(comment.file_path, "a.ts");
        assert_eq!(comment.text, "Why this?");
        assert_eq!(comment.range_label, "+11");
        assert!(comment.diff.contains("+new"));
        let annotations = comment_annotations(
            std::slice::from_ref(&comment),
            "turn:r1",
            &shown,
            &files.source,
            &map,
        );
        assert_eq!(
            annotations,
            vec![CommentAnnotation {
                id: "c1".into(),
                side: Side::New,
                line: 1,
                range_label: "+11".into(),
                text: "Why this?".into(),
            }]
        );
        assert!(
            comment_annotations(&[comment], "turn:other", &shown, &files.source, &map).is_empty()
        );
    }

    #[test]
    fn file_metadata_follows_the_partial_patch_reading() {
        let metadata = file_diff_metadata("@@ -10,3 +10,3 @@\n keep\n-old\n+new\n tail\n");
        assert_eq!(metadata.deletion_lines, vec!["keep\n", "old\n", "tail\n"]);
        assert_eq!(metadata.addition_lines, vec!["keep\n", "new\n", "tail\n"]);
        assert_eq!(metadata.hunks.len(), 1);
        assert_eq!(metadata.hunks[0].hunk_content.len(), 3);
    }
}
