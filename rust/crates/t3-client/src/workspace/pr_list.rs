use super::{ProjectKey, projects::EnvironmentProject};
use crate::time::epoch_ms as timestamp;
use crate::types::{EnvironmentId, ProjectId};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::pull_request::*;

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentPullRequestEntry {
    pub environment: EnvironmentId,
    pub entry: PullRequestListEntry,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ListEntryKey {
    pub environment: EnvironmentId,
    pub host: String,
    pub repository: String,
    pub number: u64,
}
impl EnvironmentPullRequestEntry {
    pub fn key(&self) -> ListEntryKey {
        ListEntryKey {
            environment: self.environment.clone(),
            host: self.entry.host.0.clone(),
            repository: self.entry.repository.0.clone(),
            number: self.entry.number,
        }
    }
}
pub type Viewers = BTreeMap<(Option<EnvironmentId>, String), String>;
fn normalize(value: &str) -> Option<String> {
    let value = value.trim().to_lowercase();
    (!value.is_empty()).then_some(value)
}
pub fn entry_viewer(entry: &EnvironmentPullRequestEntry, viewers: &Viewers) -> Option<String> {
    viewers
        .get(&(Some(entry.environment.clone()), entry.entry.host.0.clone()))
        .or_else(|| viewers.get(&(None, entry.entry.host.0.clone())))
        .and_then(|v| normalize(v))
}
pub fn matches_query(entry: &PullRequestListEntry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || format!(
            "#{} {} {} {} {}",
            entry.number,
            entry.title.0,
            entry.repository.0,
            entry.head_branch.0,
            entry.author.as_ref().map_or("", |a| a.login.0.as_str())
        )
        .to_lowercase()
        .contains(&query)
}
pub fn matches_filters(
    entry: &PullRequestListEntry,
    filters: &PullRequestListFilters,
    viewer: Option<&str>,
) -> bool {
    if let Some(draft) = filters.draft.as_ref().and_then(Option::as_ref)
        && entry.is_draft != (*draft == PullRequestListFiltersDraft::Only)
    {
        return false;
    }
    if let Some(review) = filters.review.as_ref().and_then(Option::as_ref) {
        let decision = entry.review_decision.as_ref().and_then(Option::as_ref);
        let matches = match review {
            PullRequestListFiltersReview::None => entry.review_decision.is_none(),
            PullRequestListFiltersReview::Approved => {
                decision == Some(&PullRequestReviewDecision::Approved)
            }
            PullRequestListFiltersReview::ChangesRequested => {
                decision == Some(&PullRequestReviewDecision::ChangesRequested)
            }
            PullRequestListFiltersReview::ReviewRequired => {
                decision == Some(&PullRequestReviewDecision::ReviewRequired)
            }
        };
        if !matches {
            return false;
        }
    }
    let labels: BTreeSet<_> = entry
        .labels
        .iter()
        .map(|l| l.name.0.trim().to_lowercase())
        .collect();
    if let Some(groups) = filters.labels.as_ref().and_then(Option::as_ref)
        && !groups.iter().all(|g| {
            g.iter()
                .any(|l| labels.contains(&l.0.trim().to_lowercase()))
        })
    {
        return false;
    }
    if let Some(excluded) = filters.excluded_labels.as_ref().and_then(Option::as_ref)
        && excluded
            .iter()
            .any(|l| labels.contains(&l.0.trim().to_lowercase()))
    {
        return false;
    }
    if let Some(author) = filters.author.as_ref().and_then(Option::as_ref) {
        let trimmed = author.0.trim();
        let author = if matches!(trimmed.to_lowercase().as_str(), "me" | "@me") {
            viewer.filter(|v| !v.trim().is_empty()).unwrap_or(trimmed)
        } else {
            trimmed
        };
        if !entry
            .author
            .as_ref()
            .is_some_and(|a| a.login.0.to_lowercase() == author.to_lowercase())
        {
            return false;
        }
    }
    true
}
pub fn filter_by_involvement<'a>(
    entries: &'a [EnvironmentPullRequestEntry],
    viewers: &Viewers,
    involvement: &PullRequestInvolvement,
) -> Vec<&'a EnvironmentPullRequestEntry> {
    entries
        .iter()
        .filter(|entry| match involvement {
            PullRequestInvolvement::All => true,
            PullRequestInvolvement::Reviewing => entry.entry.viewer_review_requested,
            PullRequestInvolvement::Authored => entry_viewer(entry, viewers).is_some_and(|v| {
                entry
                    .entry
                    .author
                    .as_ref()
                    .and_then(|a| normalize(&a.login.0))
                    == Some(v)
            }),
        })
        .collect()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKey {
    Authored,
    ReviewRequested,
    Others,
}
impl GroupKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Authored => "Authored",
            Self::ReviewRequested => "Review requested",
            Self::Others => "Others",
        }
    }
}
pub struct ListGroup<'a> {
    pub key: GroupKey,
    pub entries: Vec<&'a EnvironmentPullRequestEntry>,
}
pub fn group_by_involvement<'a>(
    entries: &'a [EnvironmentPullRequestEntry],
    viewers: &Viewers,
) -> Vec<ListGroup<'a>> {
    let mut groups = [
        ListGroup {
            key: GroupKey::Authored,
            entries: Vec::new(),
        },
        ListGroup {
            key: GroupKey::ReviewRequested,
            entries: Vec::new(),
        },
        ListGroup {
            key: GroupKey::Others,
            entries: Vec::new(),
        },
    ];
    for entry in entries {
        let authored = entry_viewer(entry, viewers).is_some_and(|v| {
            entry
                .entry
                .author
                .as_ref()
                .and_then(|a| normalize(&a.login.0))
                == Some(v)
        });
        groups[if authored {
            0
        } else if entry.entry.viewer_review_requested {
            1
        } else {
            2
        }]
        .entries
        .push(entry);
    }
    groups
        .into_iter()
        .filter(|g| !g.entries.is_empty())
        .collect()
}
pub fn compare_recency(left: &PullRequestListEntry, right: &PullRequestListEntry) -> Ordering {
    match (
        timestamp(&left.updated_at.0),
        timestamp(&right.updated_at.0),
    ) {
        (Some(l), Some(r)) => r.cmp(&l),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
pub fn score_match(entry: &PullRequestListEntry, query: &str) -> u8 {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return 0;
    }
    let number = needle.strip_prefix('#').unwrap_or(&needle);
    if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
        return if entry.number.to_string() == number {
            100
        } else {
            0
        };
    }
    let title = entry.title.0.to_lowercase();
    let terms: Vec<_> = needle.split_whitespace().collect();
    if title == needle {
        90
    } else if title.contains(&needle) {
        80
    } else if terms.len() > 1 && terms.iter().all(|t| title.contains(t)) {
        70
    } else if entry.head_branch.0.to_lowercase().contains(&needle) {
        60
    } else if entry
        .author
        .as_ref()
        .is_some_and(|a| a.login.0.to_lowercase().contains(&needle))
    {
        50
    } else if entry.repository.0.to_lowercase().contains(&needle) {
        40
    } else if terms.iter().any(|t| title.contains(t)) {
        30
    } else {
        10
    }
}
pub fn rank_matches(entries: &mut [EnvironmentPullRequestEntry], query: &str) {
    if !query.trim().is_empty() {
        entries.sort_by(|l, r| {
            score_match(&r.entry, query)
                .cmp(&score_match(&l.entry, query))
                .then_with(|| compare_recency(&l.entry, &r.entry))
        });
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListSort {
    Ready,
    Blocked,
    Updated,
    Newest,
    Oldest,
    Largest,
    Smallest,
}
fn readiness(entry: &PullRequestListEntry) -> u8 {
    if entry.mergeability == PullRequestMergeability::Conflicting {
        4
    } else if entry.state != PullRequestState::Open {
        3
    } else if entry.is_draft {
        2
    } else if entry.checks_state == Some(Some(PullRequestChecksState::Passing))
        && entry.review_decision == Some(Some(PullRequestReviewDecision::Approved))
    {
        0
    } else if entry.checks_state == Some(Some(PullRequestChecksState::Passing)) {
        1
    } else {
        2
    }
}
fn author_blocked(entry: &PullRequestListEntry) -> u8 {
    if entry.state != PullRequestState::Open {
        6
    } else if entry.mergeability == PullRequestMergeability::Conflicting {
        0
    } else if entry.review_decision == Some(Some(PullRequestReviewDecision::ChangesRequested)) {
        1
    } else if entry.checks_state == Some(Some(PullRequestChecksState::Failing)) {
        2
    } else if entry.is_draft {
        3
    } else if readiness(entry) == 0 {
        5
    } else {
        4
    }
}
pub fn sort_groups(
    groups: &mut [ListGroup<'_>],
    sort: ListSort,
    search: &str,
    measured: impl Fn(&EnvironmentPullRequestEntry) -> bool,
    involvement: &PullRequestInvolvement,
) {
    if sort == ListSort::Updated
        || (matches!(sort, ListSort::Ready | ListSort::Blocked) && !search.trim().is_empty())
    {
        return;
    }
    for group in groups {
        let role = match group.key {
            GroupKey::Authored => PullRequestInvolvement::Authored,
            GroupKey::ReviewRequested => PullRequestInvolvement::Reviewing,
            GroupKey::Others => involvement.clone(),
        };
        if sort == ListSort::Blocked && role == PullRequestInvolvement::All {
            continue;
        }
        group.entries.sort_by(|left, right| {
            let l = &left.entry;
            let r = &right.entry;
            let by_size = || {
                measured(right).cmp(&measured(left)).then_with(|| {
                    (u128::from(l.additions) + u128::from(l.deletions))
                        .cmp(&(u128::from(r.additions) + u128::from(r.deletions)))
                })
            };
            let fallback = || {
                timestamp(&r.updated_at.0)
                    .or_else(|| timestamp(&r.created_at.0))
                    .unwrap_or(0)
                    .cmp(
                        &timestamp(&l.updated_at.0)
                            .or_else(|| timestamp(&l.created_at.0))
                            .unwrap_or(0),
                    )
            };
            match sort {
                ListSort::Ready => readiness(l)
                    .cmp(&readiness(r))
                    .then_with(by_size)
                    .then_with(|| compare_recency(l, r)),
                ListSort::Blocked => if role == PullRequestInvolvement::Authored {
                    author_blocked(l).cmp(&author_blocked(r))
                } else {
                    (l.state != PullRequestState::Open).cmp(&(r.state != PullRequestState::Open))
                }
                .then_with(|| compare_recency(l, r)),
                ListSort::Newest | ListSort::Oldest => {
                    let lc = timestamp(&l.created_at.0);
                    let rc = timestamp(&r.created_at.0);
                    rc.is_some()
                        .cmp(&lc.is_some())
                        .then_with(|| {
                            if sort == ListSort::Newest {
                                rc.unwrap_or(0).cmp(&lc.unwrap_or(0))
                            } else {
                                lc.unwrap_or(0).cmp(&rc.unwrap_or(0))
                            }
                        })
                        .then_with(fallback)
                }
                ListSort::Largest | ListSort::Smallest => measured(right)
                    .cmp(&measured(left))
                    .then_with(|| {
                        let order = (u128::from(l.additions) + u128::from(l.deletions))
                            .cmp(&(u128::from(r.additions) + u128::from(r.deletions)));
                        if sort == ListSort::Largest {
                            order.reverse()
                        } else {
                            order
                        }
                    })
                    .then_with(fallback),
                ListSort::Updated => Ordering::Equal,
            }
        });
    }
}
pub fn find_scoped_project<'a>(
    projects: &'a [EnvironmentProject],
    environment: Option<&EnvironmentId>,
    project: Option<&ProjectId>,
) -> Option<&'a EnvironmentProject> {
    let project = project?;
    let mut matches = projects.iter().filter(|p| &p.project.id == project);
    if let Some(environment) = environment {
        matches.find(|p| &p.environment == environment)
    } else {
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }
}
pub fn resolve_project_scope(
    project: Option<ProjectId>,
    projects: &[EnvironmentProject],
    known: bool,
) -> Option<ProjectId> {
    project.filter(|id| !known || projects.iter().any(|p| p.project.id == *id))
}
pub fn query_environment_ids(
    environments: &[EnvironmentId],
    projects: &[EnvironmentProject],
    scoped: Option<&ProjectKey>,
    project: Option<&ProjectId>,
    known: bool,
) -> Vec<EnvironmentId> {
    environments
        .iter()
        .filter(|id| {
            if let Some(scoped) = scoped {
                **id == scoped.environment
            } else if !known || project.is_none() {
                true
            } else {
                projects
                    .iter()
                    .any(|p| &p.environment == *id && Some(&p.project.id) == project)
            }
        })
        .cloned()
        .collect()
}
pub fn selected_environment(
    named: Option<&EnvironmentId>,
    known: &BTreeSet<EnvironmentId>,
    fallback: Option<&EnvironmentId>,
) -> Option<EnvironmentId> {
    named.filter(|id| known.contains(*id)).or(fallback).cloned()
}
pub fn environment_set_key(environments: &[EnvironmentId]) -> Vec<EnvironmentId> {
    let mut ids = environments.to_vec();
    ids.sort();
    ids
}
pub fn label_color(color: Option<&str>) -> Option<String> {
    let color = color?.trim().strip_prefix('#').unwrap_or(color?.trim());
    (color.len() == 6 && color.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("#{color}"))
}

pub struct MergedList {
    pub viewers: Viewers,
    pub providers: Vec<PullRequestProviderSummary>,
    pub entries: Vec<EnvironmentPullRequestEntry>,
    pub errors: Vec<(EnvironmentId, PullRequestListProjectError)>,
    pub truncated: bool,
    pub next_cursors: BTreeMap<EnvironmentId, PullRequestListCursors>,
    pub truncated_environments: Vec<EnvironmentId>,
}
pub fn merge_lists(answers: &[(EnvironmentId, PullRequestListResult)]) -> Option<MergedList> {
    if answers.is_empty() {
        return None;
    }
    let mut merged = MergedList {
        viewers: BTreeMap::new(),
        providers: Vec::new(),
        entries: Vec::new(),
        errors: Vec::new(),
        truncated: false,
        next_cursors: BTreeMap::new(),
        truncated_environments: Vec::new(),
    };
    let mut entry_index = BTreeMap::new();
    let mut provider_index = BTreeMap::new();
    for (environment, answer) in answers {
        for (host, login) in &answer.viewers {
            merged
                .viewers
                .insert((Some(environment.clone()), host.0.clone()), login.0.clone());
        }
        for provider in &answer.providers {
            if let Some(&index) = provider_index.get(&provider.host) {
                let held: &mut PullRequestProviderSummary = &mut merged.providers[index];
                let total = held.project_count.saturating_add(provider.project_count);
                let searches = held.searches_on_host && provider.searches_on_host;
                let configured = held.configured || provider.configured;
                if !held.configured {
                    *held = provider.clone();
                }
                held.project_count = total;
                held.searches_on_host = searches;
                held.configured = configured;
            } else {
                provider_index.insert(provider.host.clone(), merged.providers.len());
                merged.providers.push(provider.clone());
            }
        }
        for entry in &answer.entries {
            let key = (environment.clone(), entry.url.clone());
            if let Some(&index) = entry_index.get(&key) {
                let held: &mut EnvironmentPullRequestEntry = &mut merged.entries[index];
                if entry.updated_at.0 > held.entry.updated_at.0 {
                    held.entry = entry.clone();
                }
            } else {
                entry_index.insert(key, merged.entries.len());
                merged.entries.push(EnvironmentPullRequestEntry {
                    environment: environment.clone(),
                    entry: entry.clone(),
                });
            }
        }
        merged.errors.extend(
            answer
                .errors
                .iter()
                .cloned()
                .map(|e| (environment.clone(), e)),
        );
        merged.truncated |= answer.truncated;
        if answer.truncated && !merged.truncated_environments.contains(environment) {
            merged.truncated_environments.push(environment.clone());
        }
        if !answer.next_cursors.is_empty() {
            merged
                .next_cursors
                .entry(environment.clone())
                .or_default()
                .extend(answer.next_cursors.clone());
        }
    }
    merged
        .entries
        .sort_by(|l, r| compare_recency(&l.entry, &r.entry));
    Some(merged)
}
#[derive(Debug, Clone)]
pub struct ListOverride {
    pub state: PullRequestState,
    pub is_draft: Option<bool>,
    pub updated_at: String,
    pub token: u64,
    pub at: u64,
    pub from_reply: bool,
    pub observed_at: Option<f64>,
}
fn stale_reply(entry: &PullRequestListEntry, override_: &ListOverride) -> bool {
    if entry.state == PullRequestState::Merged && override_.state != PullRequestState::Merged {
        return true;
    }
    if let (Some(shown), Some(reply)) = (
        timestamp(&entry.updated_at.0),
        timestamp(&override_.updated_at),
    ) && shown != reply
    {
        return reply < shown;
    }
    override_.observed_at.unwrap_or(f64::NEG_INFINITY)
        < entry
            .observed_at
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::NEG_INFINITY)
}
pub fn override_after_action(
    entry: &PullRequestListEntry,
    action: &PullRequestAction,
    now_epoch: u64,
    now_iso: &str,
    token: u64,
    reply: Option<&PullRequestSummary>,
) -> Option<ListOverride> {
    if let Some(reply) = reply {
        return Some(ListOverride {
            state: reply.state.clone(),
            is_draft: reply.is_draft.flatten(),
            updated_at: reply.updated_at.0.clone(),
            token,
            at: now_epoch,
            from_reply: true,
            observed_at: reply
                .observed_at
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|v| v.as_f64()),
        });
    }
    let (state, is_draft) = match action {
        PullRequestAction::Close => (PullRequestState::Closed, None),
        PullRequestAction::Reopen => (PullRequestState::Open, None),
        PullRequestAction::Merge => (PullRequestState::Merged, None),
        PullRequestAction::Draft => (entry.state.clone(), Some(true)),
        PullRequestAction::Ready => (entry.state.clone(), Some(false)),
        _ => return None,
    };
    Some(ListOverride {
        state,
        is_draft,
        updated_at: now_iso.into(),
        token,
        at: now_epoch,
        from_reply: false,
        observed_at: None,
    })
}
pub fn apply_overrides(
    entries: &[EnvironmentPullRequestEntry],
    overrides: &BTreeMap<ListEntryKey, ListOverride>,
    state: &PullRequestListState,
) -> Vec<EnvironmentPullRequestEntry> {
    entries
        .iter()
        .filter_map(|entry| {
            let held = overrides
                .get(&entry.key())
                .filter(|held| !held.from_reply || !stale_reply(&entry.entry, held));
            let mut next = entry.clone();
            if let Some(held) = held {
                next.entry.state = held.state.clone();
                if let Some(draft) = held.is_draft {
                    next.entry.is_draft = draft;
                }
                next.entry.updated_at.0 = held.updated_at.clone();
                if let Some(observed) = held
                    .observed_at
                    .and_then(t3_contracts::wire::JsNumber::from_f64)
                {
                    next.entry.observed_at = Some(Some(observed));
                }
            }
            let matches = match state {
                PullRequestListState::All => true,
                PullRequestListState::Open => next.entry.state == PullRequestState::Open,
                PullRequestListState::Closed => next.entry.state == PullRequestState::Closed,
                PullRequestListState::Merged => next.entry.state == PullRequestState::Merged,
            };
            matches.then_some(next)
        })
        .collect()
}
pub fn settle_overrides(
    overrides: &mut BTreeMap<ListEntryKey, ListOverride>,
    answered: &[EnvironmentPullRequestEntry],
    now: u64,
) {
    overrides.retain(|key, held| {
        answered.iter().find(|e| e.key() == *key).is_none_or(|row| {
            let agrees = row.entry.state == held.state
                && held
                    .is_draft
                    .is_none_or(|draft| row.entry.is_draft == draft);
            !agrees && now.saturating_sub(held.at) <= 60_000
        })
    });
}
pub fn reuse_entries(
    previous: &[Rc<EnvironmentPullRequestEntry>],
    next: Vec<Rc<EnvironmentPullRequestEntry>>,
) -> Vec<Rc<EnvironmentPullRequestEntry>> {
    let held: BTreeMap<_, _> = previous.iter().map(|e| (e.key(), e)).collect();
    next.into_iter()
        .map(|entry| match held.get(&entry.key()) {
            Some(before) if before.as_ref() == entry.as_ref() => Rc::clone(before),
            _ => entry,
        })
        .collect()
}
pub fn quick_actions(
    state: &PullRequestState,
    draft: bool,
    host_actions: &[PullRequestAction],
) -> Vec<PullRequestAction> {
    let actions = match state {
        PullRequestState::Merged => Vec::new(),
        PullRequestState::Closed => vec![PullRequestAction::Reopen],
        PullRequestState::Open => {
            if draft {
                vec![PullRequestAction::Close, PullRequestAction::Ready]
            } else {
                vec![PullRequestAction::Close, PullRequestAction::Merge]
            }
        }
    };
    actions
        .into_iter()
        .filter(|a| host_actions.contains(a))
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorFacet {
    pub actor: PullRequestActor,
    pub count: usize,
    pub merged_count: usize,
}
#[derive(Debug, Clone, PartialEq)]
pub struct LabelFacet {
    pub label: PullRequestLabel,
    pub count: usize,
}
pub fn in_list_state(state: &PullRequestState, filter: &PullRequestListState) -> bool {
    matches!(
        (state, filter),
        (_, PullRequestListState::All)
            | (PullRequestState::Open, PullRequestListState::Open)
            | (PullRequestState::Closed, PullRequestListState::Closed)
            | (PullRequestState::Merged, PullRequestListState::Merged)
    )
}
pub fn collect_facets(
    entries: &[EnvironmentPullRequestEntry],
    state: &PullRequestListState,
) -> (Vec<AuthorFacet>, Vec<LabelFacet>) {
    let mut unique = Vec::<&EnvironmentPullRequestEntry>::new();
    let mut positions = BTreeMap::new();
    for entry in entries {
        if let Some(&index) = positions.get(&entry.key()) {
            unique[index] = entry;
        } else {
            positions.insert(entry.key(), unique.len());
            unique.push(entry);
        }
    }
    let mut authors = BTreeMap::<String, AuthorFacet>::new();
    let mut labels = BTreeMap::<String, LabelFacet>::new();
    for scoped in unique {
        let entry = &scoped.entry;
        let in_state = in_list_state(&entry.state, state);
        if let Some(actor) = &entry.author
            && let Some(key) = normalize(&actor.login.0)
        {
            let facet = authors.entry(key).or_insert_with(|| AuthorFacet {
                actor: actor.clone(),
                count: 0,
                merged_count: 0,
            });
            facet.count += usize::from(in_state);
            facet.merged_count += usize::from(entry.state == PullRequestState::Merged);
        }
        if !in_state {
            continue;
        }
        for label in &entry.labels {
            if let Some(key) = normalize(&label.name.0) {
                let facet = labels.entry(key).or_insert_with(|| LabelFacet {
                    label: label.clone(),
                    count: 0,
                });
                if facet.label.color.is_none() {
                    facet.label.color = label.color.clone();
                }
                // Other label metadata follows the last occurrence, as in the source object spread.
                facet.label.description = label.description.clone();
                facet.count += 1;
            }
        }
    }
    let mut authors: Vec<_> = authors.into_values().filter(|a| a.count > 0).collect();
    authors.sort_by(|a, b| {
        b.merged_count
            .cmp(&a.merged_count)
            .then(b.count.cmp(&a.count))
            .then(a.actor.login.0.cmp(&b.actor.login.0))
    });
    let mut labels: Vec<_> = labels.into_values().collect();
    labels.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(a.label.name.0.cmp(&b.label.name.0))
    });
    (authors, labels)
}
pub fn author_choices(
    facets: &[AuthorFacet],
    hosted: &[PullRequestActor],
    query: &str,
) -> Vec<AuthorFacet> {
    let needle = normalize(query);
    let mut loaded: Vec<_> = facets
        .iter()
        .filter(|f| {
            needle.as_ref().is_none_or(|q| {
                f.actor.login.0.to_lowercase().contains(q)
                    || f.actor
                        .name
                        .as_ref()
                        .is_some_and(|n| n.to_lowercase().contains(q))
            })
        })
        .cloned()
        .collect();
    let mut seen: BTreeSet<_> = loaded.iter().map(|f| normalize(&f.actor.login.0)).collect();
    let mut added = Vec::new();
    for actor in hosted {
        let key = normalize(&actor.login.0);
        if !seen.insert(key.clone()) {
            continue;
        }
        added.push(
            facets
                .iter()
                .find(|f| normalize(&f.actor.login.0) == key)
                .cloned()
                .unwrap_or_else(|| AuthorFacet {
                    actor: actor.clone(),
                    count: 0,
                    merged_count: 0,
                }),
        );
    }
    let tail = loaded.split_off(loaded.len().min(15));
    loaded.extend(added);
    loaded.extend(tail);
    loaded
}
pub fn selected_author(
    facets: &[AuthorFacet],
    hosted: &[PullRequestActor],
    value: Option<&str>,
) -> Option<AuthorFacet> {
    let value = value?;
    let key = normalize(value);
    Some(
        facets
            .iter()
            .find(|f| normalize(&f.actor.login.0) == key)
            .cloned()
            .unwrap_or_else(|| AuthorFacet {
                actor: hosted
                    .iter()
                    .find(|a| normalize(&a.login.0) == key)
                    .cloned()
                    .unwrap_or_else(|| PullRequestActor {
                        login: t3_contracts::base_schemas::TrimmedNonEmptyString(value.into()),
                        name: None,
                        avatar_url: None,
                        is_bot: None,
                    }),
                count: 0,
                merged_count: 0,
            }),
    )
}
pub fn label_choices(
    facets: &[LabelFacet],
    hosted: &[PullRequestLabel],
    query: &str,
) -> Vec<LabelFacet> {
    let needle = normalize(query);
    let mut result: Vec<_> = facets
        .iter()
        .filter(|f| {
            needle
                .as_ref()
                .is_none_or(|q| f.label.name.0.to_lowercase().contains(q))
        })
        .cloned()
        .collect();
    let mut seen: BTreeSet<_> = result.iter().map(|f| normalize(&f.label.name.0)).collect();
    for label in hosted {
        let key = normalize(&label.name.0);
        if seen.insert(key.clone()) {
            result.push(
                facets
                    .iter()
                    .find(|f| normalize(&f.label.name.0) == key)
                    .cloned()
                    .unwrap_or_else(|| LabelFacet {
                        label: label.clone(),
                        count: 0,
                    }),
            );
        }
    }
    result
}

pub fn narrow_to_filters<'a>(
    entries: &'a [EnvironmentPullRequestEntry],
    state: &PullRequestListState,
    project: Option<&ProjectId>,
    host: Option<&str>,
) -> Vec<&'a EnvironmentPullRequestEntry> {
    entries
        .iter()
        .filter(|e| {
            in_list_state(&e.entry.state, state)
                && project.is_none_or(|id| &e.entry.project_id == id)
                && host.is_none_or(|host| e.entry.host.0 == host)
        })
        .collect()
}

pub fn partition_with_priority<'a>(
    entries: &'a [EnvironmentPullRequestEntry],
    authored: &'a [EnvironmentPullRequestEntry],
    reviewing: &'a [EnvironmentPullRequestEntry],
) -> Vec<ListGroup<'a>> {
    let mut author_rows: Vec<_> = Vec::new();
    let mut author_index = BTreeMap::new();
    for entry in authored {
        if let Some(&index) = author_index.get(&entry.key()) {
            author_rows[index] = entry;
        } else {
            author_index.insert(entry.key(), author_rows.len());
            author_rows.push(entry);
        }
    }
    let mut review_rows = Vec::new();
    let mut review_index = BTreeMap::new();
    for entry in reviewing {
        if author_index.contains_key(&entry.key()) {
            continue;
        }
        if let Some(&index) = review_index.get(&entry.key()) {
            review_rows[index] = entry;
        } else {
            review_index.insert(entry.key(), review_rows.len());
            review_rows.push(entry);
        }
    }
    let mut others = Vec::new();
    for entry in entries {
        if let Some(&index) = author_index.get(&entry.key()) {
            author_rows[index] = entry;
        } else if let Some(&index) = review_index.get(&entry.key()) {
            review_rows[index] = entry;
        } else {
            others.push(entry);
        }
    }
    author_rows.sort_by(|a, b| compare_recency(&a.entry, &b.entry));
    review_rows.sort_by(|a, b| compare_recency(&a.entry, &b.entry));
    [
        (GroupKey::Authored, author_rows),
        (GroupKey::ReviewRequested, review_rows),
        (GroupKey::Others, others),
    ]
    .into_iter()
    .filter(|(_, entries)| !entries.is_empty())
    .map(|(key, entries)| ListGroup { key, entries })
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiffStatKey {
    pub environment: EnvironmentId,
    pub project: ProjectId,
    pub number: u64,
}
pub fn diff_stat_key(entry: &EnvironmentPullRequestEntry) -> DiffStatKey {
    DiffStatKey {
        environment: entry.environment.clone(),
        project: entry.entry.project_id.clone(),
        number: entry.entry.number,
    }
}
pub type DiffStats = BTreeMap<DiffStatKey, (u64, u64)>;
pub fn merge_diff_stats(stats: &mut DiffStats, answers: &[(EnvironmentId, PullRequestDiffStat)]) {
    for (environment, row) in answers {
        stats.insert(
            DiffStatKey {
                environment: environment.clone(),
                project: row.project_id.clone(),
                number: row.number,
            },
            (row.additions, row.deletions),
        );
    }
}
pub fn with_diff_stat(
    entry: &EnvironmentPullRequestEntry,
    stats: &DiffStats,
) -> EnvironmentPullRequestEntry {
    let mut result = entry.clone();
    if entry.entry.additions == 0
        && entry.entry.deletions == 0
        && let Some(&(additions, deletions)) = stats.get(&diff_stat_key(entry))
    {
        result.entry.additions = additions;
        result.entry.deletions = deletions;
    }
    result
}
#[derive(Debug, Clone, PartialEq)]
pub struct StatsBatch {
    pub environment: EnvironmentId,
    pub input: PullRequestListStatsInput,
    pub keys: BTreeSet<ListEntryKey>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsPolicy {
    Visible,
    Eager,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsScope {
    pub key: String,
    pub policy: StatsPolicy,
}
pub fn stats_request_batches(
    entries: &[EnvironmentPullRequestEntry],
    candidates: &BTreeSet<ListEntryKey>,
    policy: StatsPolicy,
    active: &[StatsBatch],
    stats: &DiffStats,
    refresh: bool,
) -> Vec<StatsBatch> {
    let requested: BTreeSet<_> = active.iter().flat_map(|b| b.keys.iter()).collect();
    let mut by_environment: Vec<(EnvironmentId, Vec<&EnvironmentPullRequestEntry>)> = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in entries {
        let key = entry.key();
        if !seen.insert(key.clone())
            || (policy == StatsPolicy::Visible && !candidates.contains(&key))
        {
            continue;
        }
        if !refresh
            && (entry.entry.additions != 0
                || entry.entry.deletions != 0
                || requested.contains(&key)
                || stats.contains_key(&diff_stat_key(entry)))
        {
            continue;
        }
        if let Some((_, rows)) = by_environment
            .iter_mut()
            .find(|(env, _)| env == &entry.environment)
        {
            rows.push(entry);
        } else {
            by_environment.push((entry.environment.clone(), vec![entry]));
        }
    }
    by_environment
        .into_iter()
        .flat_map(|(environment, rows)| {
            rows.chunks(500)
                .map(|rows| StatsBatch {
                    environment: environment.clone(),
                    keys: rows.iter().map(|e| e.key()).collect(),
                    input: PullRequestListStatsInput {
                        refs: rows
                            .iter()
                            .map(|e| PullRequestRef {
                                project_id: e.entry.project_id.clone(),
                                repository: e.entry.repository.clone(),
                                number: e.entry.number,
                                host: None,
                                expected_account_id: None,
                                allow_stale: None,
                            })
                            .collect(),
                    },
                })
                .collect::<Vec<_>>()
        })
        .collect()
}
pub fn stats_refresh_batches(
    requested: &StatsScope,
    current: &StatsScope,
    entries: &[EnvironmentPullRequestEntry],
    candidates: &BTreeSet<ListEntryKey>,
    stats: &DiffStats,
) -> Option<Vec<StatsBatch>> {
    (requested == current)
        .then(|| stats_request_batches(entries, candidates, requested.policy, &[], stats, true))
}
pub fn retain_visible_stats_batches(
    batches: &mut Vec<StatsBatch>,
    visible: &BTreeSet<ListEntryKey>,
) {
    batches.retain(|batch| !batch.keys.is_disjoint(visible));
}
/// PullRequestsPage's cursor continuation, with bounded page growth for hosts without cursors.
pub fn next_list_input(
    current: &PullRequestListInput,
    answer: &PullRequestListResult,
) -> Option<PullRequestListInput> {
    if !answer.truncated {
        return None;
    }
    let mut next = current.clone();
    if answer.next_cursors.is_empty() {
        let limit = current.limit.flatten().unwrap_or(99);
        if limit >= 500 {
            return None;
        }
        next.limit = Some(Some((limit + 99).min(500)));
        next.cursors = None;
    } else {
        // A host that repeats its continuation would otherwise offer the same page forever.
        if current.cursors.as_ref().and_then(Option::as_ref) == Some(&answer.next_cursors) {
            return None;
        }
        next.cursors = Some(Some(answer.next_cursors.clone()));
    }
    Some(next)
}

/// PullRequestsPage assigns a checkout to one configured server before searching it.
/// Wait for project ownership when the route names a project and shells are still loading.
pub fn list_targets(
    environments: &[EnvironmentId],
    projects: &[EnvironmentProject],
    shells_known: bool,
    named: &str,
    project: Option<&ProjectId>,
) -> Option<Vec<super::pr_assignment::ProjectQuery>> {
    use super::pr_assignment::{ProjectQuery, project_queries};
    let named = environments.iter().find(|env| env.0 == named);
    let environments = environments
        .iter()
        .filter(|env| named.is_none_or(|id| id == *env))
        .cloned()
        .collect::<Vec<_>>();
    if let Some(project) = resolve_project_scope(project.cloned(), projects, shells_known).as_ref()
    {
        let known_owner = projects
            .iter()
            .any(|p| !p.is_scratch && &p.project.id == project);
        if !known_owner && !shells_known {
            return None;
        }
        let ids = query_environment_ids(&environments, projects, None, Some(project), true);
        return Some(
            ids.into_iter()
                .map(|environment| ProjectQuery {
                    environment,
                    project_ids: None,
                })
                .collect(),
        );
    }
    if !shells_known {
        return Some(
            environments
                .into_iter()
                .map(|environment| ProjectQuery {
                    environment,
                    project_ids: None,
                })
                .collect(),
        );
    }
    let projects: Vec<_> = projects.iter().filter(|p| !p.is_scratch).cloned().collect();
    // The initial assignment has no host availability observations yet.
    Some(project_queries(
        &projects,
        &environments,
        &BTreeMap::new(),
        |_| String::new(),
    ))
}
