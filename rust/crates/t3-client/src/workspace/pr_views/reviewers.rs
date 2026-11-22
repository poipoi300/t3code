//! Reviewer candidate merging, host search planning, and label filtering for PR views.

use std::collections::BTreeSet;
use t3_contracts::pull_request::*;

use super::detail::{ReviewOutcome, latest_review_outcomes};

/// Normalizes a reviewer login for case-insensitive deduplication.
pub fn reviewer_key(login: &str) -> String {
    login.trim().to_lowercase()
}

/// A single reviewer entry displayed in the Summary tab meta row.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewerDisplayEntry {
    pub key: String,
    pub actor: Option<PullRequestActor>,
    pub outcome: Option<ReviewOutcome>,
    pub stale: bool,
}

/// Combines requested reviewers and submitted review verdicts deduplicated case-insensitively by login.
pub fn reviewer_entries(
    detail: &PullRequestDetail,
    comments: &[PullRequestComment],
    commits: &[PullRequestCommit],
) -> Vec<ReviewerDisplayEntry> {
    let review_outcomes = latest_review_outcomes(comments, commits);
    let mut entries = Vec::new();
    for actor in &detail.reviewers {
        let key = reviewer_key(&actor.login.0);
        let outcome = review_outcomes.iter().find(|entry| {
            entry
                .actor
                .as_ref()
                .is_some_and(|a| reviewer_key(&a.login.0) == key)
        });
        entries.push(ReviewerDisplayEntry {
            key: actor.login.0.clone(),
            actor: Some(actor.clone()),
            outcome: outcome.map(|o| o.outcome),
            stale: outcome.is_some_and(|o| o.stale),
        });
    }
    for outcome in review_outcomes {
        let already_requested = outcome.actor.as_ref().is_some_and(|actor| {
            let key = reviewer_key(&actor.login.0);
            detail
                .reviewers
                .iter()
                .any(|requested| reviewer_key(&requested.login.0) == key)
        });
        if !already_requested {
            entries.push(ReviewerDisplayEntry {
                key: outcome.key,
                actor: outcome.actor,
                outcome: Some(outcome.outcome),
                stale: outcome.stale,
            });
        }
    }
    entries
}

fn candidate_identity(candidate: &PullRequestReviewerCandidate) -> String {
    let kind = match candidate.kind {
        PullRequestReviewerKind::User => "user",
        PullRequestReviewerKind::Team => "team",
    };
    format!("{kind}:{}", candidate.id.0)
}

/// Merges initial page candidates with host search results while preserving local request state.
pub fn merge_reviewer_candidates(
    loaded: &[PullRequestReviewerCandidate],
    searched: &[PullRequestReviewerCandidate],
    query: &str,
) -> Vec<PullRequestReviewerCandidate> {
    let needle = query.trim().to_lowercase();
    let known: BTreeSet<String> = loaded.iter().map(candidate_identity).collect();
    let host_matches: BTreeSet<String> = searched.iter().map(candidate_identity).collect();
    let mut merged: Vec<PullRequestReviewerCandidate> = loaded
        .iter()
        .filter(|candidate| {
            host_matches.contains(&candidate_identity(candidate))
                || needle.is_empty()
                || candidate.login.0.to_lowercase().contains(&needle)
                || candidate
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&needle)
        })
        .cloned()
        .collect();
    for candidate in searched {
        if !known.contains(&candidate_identity(candidate)) {
            merged.push(candidate.clone());
        }
    }
    merged
}

/// Input parameters for determining whether the host needs to be searched for reviewers.
#[derive(Debug, Clone, Copy)]
pub struct ReviewerSearchInput<'a> {
    pub loaded: Option<&'a PullRequestReviewerCandidateList>,
    pub searchable: bool,
    pub query: &'a str,
    pub debounced_query: &'a str,
}

/// Returns the trimmed query to send to `pullRequests.reviewerCandidates`, or `None` if local filtering suffices.
pub fn host_reviewer_query(input: &ReviewerSearchInput<'_>) -> Option<String> {
    let debounced = input.debounced_query.trim();
    (input.searchable && input.loaded.is_some_and(|list| list.truncated) && !debounced.is_empty())
        .then(|| debounced.to_owned())
}

/// Reason why the reviewer candidate list may omit some repository collaborators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerTruncation {
    Unsearchable,
    Searchable,
    SearchFailed,
    SearchTruncated,
}

/// Current state of the debounced host reviewer search query.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReviewerSearchState<'a> {
    pub data: Option<&'a PullRequestReviewerCandidateList>,
    pub is_pending: bool,
    pub error: Option<&'a str>,
}

/// Resolved candidate list, loading indicator, error, and truncation hint for the reviewer picker.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewerCandidateView {
    pub candidates: Vec<PullRequestReviewerCandidate>,
    pub search_pending: bool,
    pub search_error: Option<String>,
    pub truncation: Option<ReviewerTruncation>,
}

/// Computes the reviewer picker view model matching `reviewerCandidateView` in client-runtime.
pub fn reviewer_candidate_view(
    input: &ReviewerSearchInput<'_>,
    search: &ReviewerSearchState<'_>,
) -> ReviewerCandidateView {
    let query = input.query.trim();
    let host_query = host_reviewer_query(input);
    let searched = host_query.as_deref() == Some(query) && host_query.is_some();
    let loaded_candidates = input.loaded.map_or(&[][..], |l| l.candidates.as_slice());
    let searched_candidates = if searched {
        search.data.map_or(&[][..], |s| s.candidates.as_slice())
    } else {
        &[]
    };
    let candidates = merge_reviewer_candidates(loaded_candidates, searched_candidates, query);
    let truncated_loaded = input.loaded.is_some_and(|l| l.truncated);
    let searching = input.searchable && truncated_loaded && !query.is_empty();
    let search_error = if searched {
        search.error.map(str::to_owned)
    } else {
        None
    };
    let truncation = if !truncated_loaded {
        None
    } else if !input.searchable {
        Some(ReviewerTruncation::Unsearchable)
    } else if query.is_empty() {
        Some(ReviewerTruncation::Searchable)
    } else if !searched {
        None
    } else if search_error.is_some() {
        Some(ReviewerTruncation::SearchFailed)
    } else if search.data.is_some_and(|s| s.truncated) {
        Some(ReviewerTruncation::SearchTruncated)
    } else {
        None
    };
    ReviewerCandidateView {
        search_pending: candidates.is_empty() && searching && (!searched || search.is_pending),
        candidates,
        search_error,
        truncation,
    }
}

/// Matches a label candidate by name or description, case-insensitively.
pub fn candidate_matches_label(candidate: &PullRequestLabelCandidate, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty()
        || candidate.name.0.to_lowercase().contains(&needle)
        || candidate
            .description
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains(&needle)
}
