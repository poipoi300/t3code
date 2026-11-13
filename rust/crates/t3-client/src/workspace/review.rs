use super::pull_requests::PullRequestKey;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::pull_request::{
    PullRequestFileViewedState, PullRequestFilesViewedResult, PullRequestReviewCommentDraft,
    PullRequestSetFilesViewedInputFiles,
};

#[derive(Debug, Clone, PartialEq)]
pub struct PendingReviewComment {
    pub id: String,
    pub comment: PullRequestReviewCommentDraft,
}
#[derive(Default)]
pub struct PullRequestReviewStore {
    sequence: u64,
    drafts: BTreeMap<PullRequestKey, Vec<PendingReviewComment>>,
    summaries: BTreeMap<PullRequestKey, String>,
}
impl PullRequestReviewStore {
    pub fn remove_environment(&mut self, environment: &crate::types::EnvironmentId) {
        self.drafts
            .retain(|key, _| &key.project.environment != environment);
        self.summaries
            .retain(|key, _| &key.project.environment != environment);
    }
    pub fn add_comment(
        &mut self,
        key: PullRequestKey,
        comment: PullRequestReviewCommentDraft,
    ) -> String {
        self.sequence += 1;
        let id = format!("pending-review-comment-{}", self.sequence);
        self.drafts
            .entry(key)
            .or_default()
            .push(PendingReviewComment {
                id: id.clone(),
                comment,
            });
        id
    }
    pub fn comments(&self, key: &PullRequestKey) -> &[PendingReviewComment] {
        self.drafts.get(key).map_or(&[], Vec::as_slice)
    }
    pub fn remove_comments(&mut self, key: &PullRequestKey, ids: &BTreeSet<String>) {
        if let Some(comments) = self.drafts.get_mut(key) {
            comments.retain(|c| !ids.contains(&c.id));
            if comments.is_empty() {
                self.drafts.remove(key);
            }
        }
    }
    pub fn clear_comments(&mut self, key: &PullRequestKey) {
        self.drafts.remove(key);
    }
    pub fn summary(&self, key: &PullRequestKey) -> Option<&str> {
        self.summaries.get(key).map(String::as_str)
    }
    pub fn set_summary(&mut self, key: PullRequestKey, body: String) {
        self.summaries.insert(key, body);
    }
    pub fn clear_submitted_summary(&mut self, key: &PullRequestKey, submitted: &str) {
        if self.summary(key) == Some(submitted) {
            self.summaries.remove(key);
        }
    }
}

pub type FileViewedOverlay = BTreeMap<String, bool>;
pub type FileViewedStates = BTreeMap<String, PullRequestFileViewedState>;
pub fn to_file_viewed_states(
    result: Option<&PullRequestFilesViewedResult>,
) -> Option<FileViewedStates> {
    result.map(|result| {
        result
            .files
            .iter()
            .map(|f| (f.path.clone(), f.state.clone()))
            .collect()
    })
}
pub fn is_file_viewed(
    path: &str,
    states: Option<&FileViewedStates>,
    overlay: &FileViewedOverlay,
) -> bool {
    overlay.get(path).copied().unwrap_or_else(|| {
        states.and_then(|s| s.get(path)) == Some(&PullRequestFileViewedState::Viewed)
    })
}
pub fn is_stale_viewed_state(state: Option<&PullRequestFileViewedState>) -> bool {
    state == Some(&PullRequestFileViewedState::Dismissed)
}
pub fn settle_file_viewed_overlay(
    overlay: &mut FileViewedOverlay,
    states: Option<&FileViewedStates>,
    pending: &BTreeSet<String>,
    answered: &BTreeSet<String>,
) {
    let Some(states) = states else {
        return;
    };
    overlay.retain(|path, pressed| {
        pending.contains(path)
            || (!answered.contains(path)
                && (states.get(path) == Some(&PullRequestFileViewedState::Viewed)) != *pressed)
    });
}
pub fn revert_file_viewed_overlay(
    overlay: &mut FileViewedOverlay,
    batch: &[PullRequestSetFilesViewedInputFiles],
    owned: &BTreeSet<String>,
) {
    for file in batch {
        if owned.contains(&file.path) && overlay.get(&file.path) == Some(&file.viewed) {
            overlay.remove(&file.path);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ViewedBatch {
    pub key: PullRequestKey,
    pub allow_stale: Option<bool>,
    pub scope_generation: u64,
    pub request: u64,
    pub files: Vec<PullRequestSetFilesViewedInputFiles>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewedWriteResult {
    Success,
    Failure,
    Interrupted,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewedCompletion {
    pub refresh: bool,
    pub report_failure: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ViewedScopeKey {
    pub pull_request: PullRequestKey,
    pub allow_stale: Option<bool>,
}
struct ViewedScope {
    generation: u64,
    subscribers: usize,
    overlay: FileViewedOverlay,
    queued: Vec<PullRequestSetFilesViewedInputFiles>,
    sent_by: BTreeMap<String, u64>,
    requests: u64,
    due: Option<u64>,
    answer: Option<Rc<PullRequestFilesViewedResult>>,
    answered_from: BTreeMap<String, Option<Rc<PullRequestFilesViewedResult>>>,
}
#[derive(Default)]
pub struct FilesViewedStore {
    generation: u64,
    scopes: BTreeMap<ViewedScopeKey, ViewedScope>,
}
fn same_answer(
    a: &Option<Rc<PullRequestFilesViewedResult>>,
    b: &Option<Rc<PullRequestFilesViewedResult>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}
impl FilesViewedStore {
    pub fn remove_environment(
        &mut self,
        environment: &crate::types::EnvironmentId,
    ) -> Vec<ViewedBatch> {
        let keys: Vec<_> = self
            .scopes
            .keys()
            .filter(|key| &key.pull_request.project.environment == environment)
            .cloned()
            .collect();
        let batches = keys.iter().filter_map(|key| self.flush(key)).collect();
        self.scopes
            .retain(|key, _| &key.pull_request.project.environment != environment);
        batches
    }
    pub const FLUSH_DELAY_MS: u64 = 400;
    pub fn subscribe(
        &mut self,
        key: ViewedScopeKey,
        initial: Option<Rc<PullRequestFilesViewedResult>>,
    ) {
        let scope = self.scopes.entry(key).or_insert_with(|| {
            self.generation += 1;
            ViewedScope {
                generation: self.generation,
                subscribers: 0,
                overlay: BTreeMap::new(),
                queued: Vec::new(),
                sent_by: BTreeMap::new(),
                requests: 0,
                due: None,
                answer: initial,
                answered_from: BTreeMap::new(),
            }
        });
        scope.subscribers += 1;
    }
    pub fn overlay(&self, key: &ViewedScopeKey) -> Option<&FileViewedOverlay> {
        self.scopes.get(key).map(|s| &s.overlay)
    }
    pub fn set_viewed(
        &mut self,
        key: &ViewedScopeKey,
        file: PullRequestSetFilesViewedInputFiles,
        now: u64,
    ) {
        let Some(scope) = self.scopes.get_mut(key) else {
            return;
        };
        scope.overlay.insert(file.path.clone(), file.viewed);
        if let Some(previous) = scope.queued.iter_mut().find(|f| f.path == file.path) {
            *previous = file;
        } else {
            scope.queued.push(file);
        }
        scope.due = Some(now.saturating_add(Self::FLUSH_DELAY_MS));
    }
    pub fn answer(
        &mut self,
        key: &ViewedScopeKey,
        answer: Option<Rc<PullRequestFilesViewedResult>>,
    ) {
        let Some(scope) = self.scopes.get_mut(key) else {
            return;
        };
        if same_answer(&scope.answer, &answer) {
            return;
        }
        scope.answer = answer;
        let pending: BTreeSet<String> = scope
            .queued
            .iter()
            .map(|f| f.path.clone())
            .chain(scope.sent_by.keys().cloned())
            .collect();
        let mut answered = BTreeSet::new();
        scope.answered_from.retain(|path, from| {
            if pending.contains(path) || same_answer(from, &scope.answer) {
                true
            } else {
                answered.insert(path.clone());
                false
            }
        });
        let states = to_file_viewed_states(scope.answer.as_deref());
        settle_file_viewed_overlay(&mut scope.overlay, states.as_ref(), &pending, &answered);
    }
    pub fn flush(&mut self, key: &ViewedScopeKey) -> Option<ViewedBatch> {
        let scope = self.scopes.get_mut(key)?;
        scope.due = None;
        if scope.queued.is_empty() {
            return None;
        }
        scope.requests += 1;
        let files = std::mem::take(&mut scope.queued);
        for f in &files {
            scope.sent_by.insert(f.path.clone(), scope.requests);
        }
        Some(ViewedBatch {
            key: key.pull_request.clone(),
            allow_stale: key.allow_stale,
            scope_generation: scope.generation,
            request: scope.requests,
            files,
        })
    }
    pub fn take_due(&mut self, now: u64) -> Vec<ViewedBatch> {
        let keys: Vec<_> = self
            .scopes
            .iter()
            .filter(|(_, s)| s.due.is_some_and(|due| now >= due))
            .map(|(key, _)| key.clone())
            .collect();
        keys.into_iter()
            .filter_map(|key| self.flush(&key))
            .collect()
    }
    pub fn unsubscribe(&mut self, key: &ViewedScopeKey) -> Option<ViewedBatch> {
        let scope = self.scopes.get_mut(key)?;
        scope.subscribers = scope.subscribers.saturating_sub(1);
        if scope.subscribers > 0 {
            return None;
        }
        let batch = self.flush(key);
        self.scopes.remove(key);
        batch
    }
    pub fn complete(&mut self, batch: &ViewedBatch, result: ViewedWriteResult) -> ViewedCompletion {
        let key = ViewedScopeKey {
            pull_request: batch.key.clone(),
            allow_stale: batch.allow_stale,
        };
        let Some(scope) = self
            .scopes
            .get_mut(&key)
            .filter(|s| s.generation == batch.scope_generation)
        else {
            return ViewedCompletion {
                refresh: result != ViewedWriteResult::Interrupted,
                report_failure: false,
            };
        };
        let mine: BTreeSet<_> = batch
            .files
            .iter()
            .filter(|f| scope.sent_by.get(&f.path) == Some(&batch.request))
            .map(|f| f.path.clone())
            .collect();
        for path in &mine {
            scope.sent_by.remove(path);
        }
        if result == ViewedWriteResult::Success {
            for path in mine {
                scope.answered_from.insert(path, scope.answer.clone());
            }
            return ViewedCompletion {
                refresh: true,
                report_failure: false,
            };
        }
        let owned = mine
            .into_iter()
            .filter(|path| !scope.queued.iter().any(|f| &f.path == path))
            .collect::<BTreeSet<_>>();
        revert_file_viewed_overlay(&mut scope.overlay, &batch.files, &owned);
        let failed = !owned.is_empty() && result != ViewedWriteResult::Interrupted;
        ViewedCompletion {
            refresh: failed,
            report_failure: failed,
        }
    }
    pub fn next_due(&self) -> Option<u64> {
        self.scopes.values().filter_map(|s| s.due).min()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffFoldOverride {
    Expanded,
    Folded,
}
pub fn is_file_diff_collapsed(
    file: &str,
    override_: Option<DiffFoldOverride>,
    choices: &BTreeMap<String, bool>,
    viewed: bool,
) -> bool {
    choices
        .get(file)
        .copied()
        .unwrap_or(override_ == Some(DiffFoldOverride::Folded) || viewed)
}
pub fn file_viewed_range<'a, T>(
    files: &'a [T],
    key: &str,
    anchor: Option<&str>,
    file_key: impl Fn(&T) -> &str,
) -> &'a [T] {
    let Some(index) = files.iter().position(|f| file_key(f) == key) else {
        return &[];
    };
    let anchor = anchor
        .and_then(|a| files.iter().position(|f| file_key(f) == a))
        .unwrap_or(index);
    &files[index.min(anchor)..=index.max(anchor)]
}
#[derive(Debug, Clone, Copy)]
pub struct HunkRange {
    pub deletion_start: u64,
    pub deletion_count: u64,
    pub addition_start: u64,
    pub addition_count: u64,
}
pub fn is_line_in_file_diff(
    hunks: &[HunkRange],
    side: &t3_contracts::pull_request::PullRequestDiffSide,
    line: u64,
) -> bool {
    use t3_contracts::pull_request::PullRequestDiffSide;
    hunks.iter().any(|h| {
        let (start, count) = if *side == PullRequestDiffSide::Left {
            (h.deletion_start, h.deletion_count)
        } else {
            (h.addition_start, h.addition_count)
        };
        line >= start && line - start < count
    })
}
