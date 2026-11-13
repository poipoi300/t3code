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
