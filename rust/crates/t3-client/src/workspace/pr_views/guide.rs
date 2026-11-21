use super::super::pull_requests::PullRequestKey;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::pull_request_review_guide::PullRequestReviewGuideExcerpt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideRunWatch {
    pub key: PullRequestKey,
    pub title: Option<String>,
    pub previous_guide_at: Option<String>,
    pub previous_failed_at: Option<String>,
    pub since: u64,
    pub answered_running: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuideProgress {
    pub position: usize,
    pub seen: BTreeSet<usize>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GuideProgressKey {
    pub run: PullRequestKey,
    pub head_sha: String,
}
#[derive(Default)]
pub struct GuideRunStore {
    pub watches: BTreeMap<PullRequestKey, GuideRunWatch>,
    pub page_detail_tab: Option<(PullRequestKey, String)>,
    pub progress: BTreeMap<GuideProgressKey, GuideProgress>,
}
impl GuideRunStore {
    pub fn watch(&mut self, watch: GuideRunWatch) {
        self.watches.entry(watch.key.clone()).or_insert(watch);
    }
    pub fn settle(&mut self, key: &PullRequestKey) {
        self.watches.remove(key);
    }
    pub fn set_progress(&mut self, key: GuideProgressKey, change: impl FnOnce(&mut GuideProgress)) {
        change(self.progress.entry(key).or_default());
    }
    pub fn show_page_detail_tab(&mut self, key: PullRequestKey, tab: String) {
        self.page_detail_tab = Some((key, tab));
    }
    pub fn remove_environment(&mut self, environment: &crate::types::EnvironmentId) {
        self.watches
            .retain(|key, _| &key.project.environment != environment);
        self.progress
            .retain(|key, _| &key.run.project.environment != environment);
        if self
            .page_detail_tab
            .as_ref()
            .is_some_and(|(key, _)| &key.project.environment == environment)
        {
            self.page_detail_tab = None;
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcerptRow {
    pub hunk: usize,
    pub new_at: usize,
    pub old_at: usize,
    pub new_line: Option<usize>,
    pub old_line: Option<usize>,
    pub text: String,
}
pub fn excerpt_expansion_old_contents(new_contents: &str, rows: &[ExcerptRow]) -> Option<String> {
    let head = rows.first()?;
    if rows.iter().any(|row| row.hunk != head.hunk) {
        return None;
    }
    let new_lines: Vec<_> = new_contents.split_inclusive('\n').collect();
    let before = head.new_at.saturating_sub(1).min(new_lines.len());
    let old_before = head.old_at.saturating_sub(1);
    let count = rows.iter().filter(|row| row.new_line.is_some()).count();
    let mut result = String::new();
    if old_before > before {
        result.push_str(&"\n".repeat(old_before - before));
    }
    for line in &new_lines[before.saturating_sub(old_before)..before] {
        result.push_str(line);
    }
    let old_rows: Vec<_> = rows.iter().filter(|row| row.old_line.is_some()).collect();
    for row in &old_rows {
        result.push_str(&row.text);
    }
    let suffix = &new_lines[(before + count).min(new_lines.len())..];
    if !suffix.is_empty() && old_rows.last().is_some_and(|row| !row.text.ends_with('\n')) {
        result.push('\n');
    }
    for line in suffix {
        result.push_str(line);
    }
    Some(result)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFile {
    pub name: String,
    pub contents: String,
    pub cache_key: Option<String>,
}
pub fn excerpt_loader_allowed(change_type: &str, rows: &[ExcerptRow]) -> bool {
    !matches!(change_type, "new" | "deleted")
        && rows
            .iter()
            .all(|row| rows.first().is_none_or(|first| row.hunk == first.hunk))
}
pub fn excerpt_loaded_files(
    excerpt: &PullRequestReviewGuideExcerpt,
    rows: &[ExcerptRow],
    new_file: LoadedFile,
    old_file: Option<LoadedFile>,
) -> (LoadedFile, Option<LoadedFile>) {
    let Some(contents) = excerpt_expansion_old_contents(&new_file.contents, rows) else {
        return (new_file, old_file);
    };
    let side = match excerpt.side {
        t3_contracts::pull_request_review_guide::PullRequestReviewGuideExcerptSide::New => "new",
        _ => "old",
    };
    let key = format!(
        "{}:excerpt:{side}:{}-{}",
        new_file.cache_key.as_deref().unwrap_or(&new_file.name),
        excerpt.start,
        excerpt.end
    );
    let name = old_file.map_or_else(|| new_file.name.clone(), |file| file.name);
    (
        new_file,
        Some(LoadedFile {
            name,
            contents,
            cache_key: Some(key),
        }),
    )
}

/// Select the same rows as reviewGuideTour.ts, including the other half of a replacement.
pub fn excerpt_text(
    file: &crate::view_logic::session::review::FileDiffMetadata,
    side: &t3_contracts::pull_request_review_guide::PullRequestReviewGuideExcerptSide,
    start: u64,
    end: u64,
) -> Option<String> {
    use crate::view_logic::session::review::HunkContent;
    use t3_contracts::pull_request_review_guide::PullRequestReviewGuideExcerptSide;
    let new_side = *side == PullRequestReviewGuideExcerptSide::New;
    let mut rows = Vec::new();
    for (i, hunk) in file.hunks.iter().enumerate() {
        let (mut old, mut new) = (hunk.deletion_line_index, hunk.addition_line_index);
        for part in &hunk.hunk_content {
            let (context, deletions, additions) = match *part {
                HunkContent::Context { lines } => (lines, 0, 0),
                HunkContent::Change {
                    deletions,
                    additions,
                } => (0, deletions, additions),
            };
            for _ in 0..context {
                let at = if new_side {
                    hunk.addition_start
                        .checked_add(new - hunk.addition_line_index)?
                } else {
                    hunk.deletion_start
                        .checked_add(old - hunk.deletion_line_index)?
                };
                rows.push((i, ' ', Some(at as u64), file.addition_lines.get(new)?));
                old += 1;
                new += 1;
            }
            for _ in 0..deletions {
                let at = hunk
                    .deletion_start
                    .checked_add(old - hunk.deletion_line_index)?;
                rows.push((
                    i,
                    '-',
                    (!new_side).then_some(at as u64),
                    file.deletion_lines.get(old)?,
                ));
                old += 1;
            }
            for _ in 0..additions {
                let at = hunk
                    .addition_start
                    .checked_add(new - hunk.addition_line_index)?;
                rows.push((
                    i,
                    '+',
                    new_side.then_some(at as u64),
                    file.addition_lines.get(new)?,
                ));
                new += 1;
            }
        }
    }
    let mut matched = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.2.is_some_and(|n| (start..=end).contains(&n)));
    let (mut first, _) = matched.next()?;
    let mut last = matched.next_back().map_or(first, |(i, _)| i);
    if new_side && rows[first].1 == '+' {
        while first > 0 && rows[first - 1].0 == rows[first].0 && rows[first - 1].1 == '-' {
            first -= 1;
        }
    } else if !new_side && rows[last].1 == '-' {
        while last + 1 < rows.len() && rows[last + 1].0 == rows[last].0 && rows[last + 1].1 == '+' {
            last += 1;
        }
    }
    Some(
        rows[first..=last]
            .iter()
            .map(|(_, prefix, _, line)| format!("{prefix}{}", line.trim_end_matches(['\r', '\n'])))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::diff_panel::comments::file_diff_metadata;
    use t3_contracts::pull_request_review_guide::PullRequestReviewGuideExcerptSide::{New, Old};

    #[test]
    fn excerpt_includes_the_replacement_on_either_side_without_unrelated_context() {
        let file = file_diff_metadata("@@ -3,3 +8,3 @@\n before\n-old\n+new\n after\n");
        assert_eq!(
            excerpt_text(&file, &New, 9, 9).as_deref(),
            Some("-old\n+new")
        );
        assert_eq!(
            excerpt_text(&file, &Old, 4, 4).as_deref(),
            Some("-old\n+new")
        );
        assert_eq!(excerpt_text(&file, &Old, 3, 3).as_deref(), Some(" before"));
        assert_eq!(excerpt_text(&file, &New, 1, 3), None);
    }
}
