//! Compatibility types over the canonical shared palette matcher.
use crate::view_logic::TextRange;
pub use shared::{Field, WeightedField, fold_in_place, fold_query, match_class, picked_query};
use std::cmp::Ordering;
use t3_shared::palette_search as shared;
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchMatch {
    pub score: f64,
    pub quality: f64,
    pub whole_words: bool,
    pub strong: bool,
    pub ranges: Vec<TextRange>,
    pub group_quality: Option<f64>,
    pub picked_at: Option<f64>,
    pub picked_exactly: bool,
    pub hint: Option<MatchHint>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchHint {
    pub label: String,
    pub text: String,
    pub ranges: Vec<TextRange>,
}
#[derive(Debug, Clone, Default)]
pub struct SearchPullRequest {
    pub number: u64,
    pub title: Option<String>,
    pub references: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct ThreadSubject {
    pub id: Option<String>,
    pub title: String,
    pub branch: Option<String>,
    pub project_title: Option<String>,
    pub environment_label: Option<String>,
    pub pull_requests: Vec<SearchPullRequest>,
    pub activity_at: Option<f64>,
    pub opens: Option<t3_contracts::orchestration_v2::OrchestrationV2ThreadOpens>,
}
fn ranges(value: Vec<shared::TextRange>) -> Vec<TextRange> {
    value
        .into_iter()
        .map(|r| TextRange {
            start: r.start,
            end: r.end,
        })
        .collect()
}
fn shared_ranges(value: &[TextRange]) -> Vec<shared::TextRange> {
    value
        .iter()
        .map(|r| shared::TextRange {
            start: r.start,
            end: r.end,
        })
        .collect()
}
impl From<shared::SearchMatch> for SearchMatch {
    fn from(value: shared::SearchMatch) -> Self {
        Self {
            score: value.score,
            quality: value.quality,
            whole_words: value.whole_words,
            strong: value.strong,
            ranges: ranges(value.ranges),
            group_quality: value.group_quality,
            picked_at: value.picked_at,
            picked_exactly: value.picked_exactly,
            hint: value.hint.map(|h| MatchHint {
                label: h.label,
                text: h.text,
                ranges: ranges(h.ranges),
            }),
        }
    }
}
fn canonical_match(value: &SearchMatch) -> shared::SearchMatch {
    shared::SearchMatch {
        score: value.score,
        quality: value.quality,
        whole_words: value.whole_words,
        strong: value.strong,
        ranges: shared_ranges(&value.ranges),
        group_quality: value.group_quality,
        picked_at: value.picked_at,
        picked_exactly: value.picked_exactly,
        hint: value.hint.as_ref().map(|h| shared::MatchHint {
            label: h.label.clone(),
            text: h.text.clone(),
            ranges: shared_ranges(&h.ranges),
        }),
    }
}
fn subject(value: &ThreadSubject) -> shared::ThreadSubject {
    shared::ThreadSubject {
        id: value.id.clone(),
        title: value.title.clone(),
        branch: value.branch.clone(),
        project_title: value.project_title.clone(),
        environment_label: value.environment_label.clone(),
        activity_at: value.activity_at,
        opens: value.opens.clone(),
        pull_requests: value
            .pull_requests
            .iter()
            .map(|p| shared::SearchPullRequest {
                number: p.number,
                title: p.title.clone(),
                references: p.references.clone(),
            })
            .collect(),
    }
}
pub fn merge_ranges(value: Vec<TextRange>) -> Vec<TextRange> {
    ranges(shared::merge_ranges(shared_ranges(&value)))
}
pub fn match_fields(query: &str, fields: &[WeightedField], typos: bool) -> Option<SearchMatch> {
    shared::match_fields(query, fields, typos).map(Into::into)
}
pub fn score_terms(query: &str, terms: &[String], typos: bool) -> Option<SearchMatch> {
    shared::score_terms(query, terms, typos).map(Into::into)
}
pub fn compare_matches(a: &SearchMatch, b: &SearchMatch) -> Ordering {
    shared::compare_matches(&canonical_match(a), &canonical_match(b))
}
pub fn rank_items<T>(
    items: &[T],
    score: impl Fn(&T, bool) -> Option<SearchMatch>,
) -> Vec<(&T, SearchMatch)> {
    shared::rank_items(items, |item, typos| {
        score(item, typos).as_ref().map(canonical_match)
    })
    .into_iter()
    .map(|(item, score)| (item, score.into()))
    .collect()
}
pub fn recent_use(thread: &ThreadSubject) -> f64 {
    shared::recent_use(&subject(thread))
}
pub fn score_thread(
    query: &str,
    thread: &ThreadSubject,
    now: f64,
    typos: bool,
    include_context: bool,
) -> Option<SearchMatch> {
    shared::score_thread(query, &subject(thread), now, typos, include_context).map(Into::into)
}
