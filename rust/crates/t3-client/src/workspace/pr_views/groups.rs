use super::super::pr_list::*;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::pull_request::*;

pub struct GroupInput<'a> {
    pub known: &'a [EnvironmentPullRequestEntry],
    pub baseline: Option<&'a [EnvironmentPullRequestEntry]>,
    pub providers: &'a [PullRequestProviderSummary],
    pub viewers: &'a Viewers,
    pub involvement: PullRequestInvolvement,
    pub local_filters: &'a PullRequestListFilters,
    pub has_local_filters: bool,
    pub text: &'a str,
    pub answered_locally: bool,
    pub baseline_empty: bool,
    pub held: Option<(
        &'a [EnvironmentPullRequestEntry],
        &'a [EnvironmentPullRequestEntry],
    )>,
    pub authored: Option<&'a [EnvironmentPullRequestEntry]>,
    pub reviewing: Option<&'a [EnvironmentPullRequestEntry]>,
    pub partitions_wanted: bool,
}
#[derive(Debug, Clone)]
pub struct OwnedGroup {
    pub key: GroupKey,
    pub label: String,
    pub entries: Vec<EnvironmentPullRequestEntry>,
}
pub struct GroupResult {
    pub entries: Vec<EnvironmentPullRequestEntry>,
    pub listed_statuses: BTreeMap<String, (PullRequestState, bool)>,
    pub groups: Vec<OwnedGroup>,
    pub entries_by_stats_key: BTreeMap<ListEntryKey, EnvironmentPullRequestEntry>,
    pub held_by_surface: BTreeMap<String, EnvironmentPullRequestEntry>,
}
fn component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
pub fn surface_id(entry: &EnvironmentPullRequestEntry, normalize_repository: bool) -> String {
    format!(
        "pull-request:{}:{}:{}:{}:{}",
        component(&entry.environment.0),
        component(&entry.entry.project_id.0),
        component(&entry.entry.host.0.to_lowercase()),
        component(&if normalize_repository {
            entry.entry.repository.0.to_lowercase()
        } else {
            entry.entry.repository.0.clone()
        }),
        entry.entry.number
    )
}
pub fn pull_request_groups(input: GroupInput<'_>) -> GroupResult {
    let searching: BTreeSet<_> = input
        .providers
        .iter()
        .filter(|p| p.searches_on_host)
        .map(|p| &p.host)
        .collect();
    let narrow = |rows: &[EnvironmentPullRequestEntry]| {
        filter_by_involvement(rows, input.viewers, &input.involvement)
            .into_iter()
            .filter(|entry| {
                !input.has_local_filters
                    || matches_filters(
                        &entry.entry,
                        input.local_filters,
                        entry_viewer(entry, input.viewers).as_deref(),
                    )
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let mut entries = narrow(input.known);
    if input.text.is_empty() {
        entries.sort_by(|a, b| compare_recency(&a.entry, &b.entry));
    } else {
        entries.retain(|e| {
            input.answered_locally && searching.contains(&e.entry.host)
                || matches_query(&e.entry, input.text)
        });
        let listed: BTreeSet<_> = entries
            .iter()
            .map(EnvironmentPullRequestEntry::key)
            .collect();
        let mut missed: Vec<_> = narrow(input.baseline.unwrap_or_default())
            .into_iter()
            .filter(|e| !listed.contains(&e.key()) && matches_query(&e.entry, input.text))
            .collect();
        rank_matches(&mut missed, input.text);
        entries.extend(missed);
    }
    let listed_statuses = entries
        .iter()
        .map(|e| {
            (
                surface_id(e, false),
                (e.entry.state.clone(), e.entry.is_draft),
            )
        })
        .collect();
    let partition_filters = |rows: &[EnvironmentPullRequestEntry]| {
        rows.iter()
            .filter(|e| {
                !input.has_local_filters
                    || matches_filters(
                        &e.entry,
                        input.local_filters,
                        entry_viewer(e, input.viewers).as_deref(),
                    )
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let authored = input
        .authored
        .or(input.held.map(|h| h.0))
        .map(partition_filters);
    let reviewing = input
        .reviewing
        .or(input.held.map(|h| h.1))
        .map(partition_filters);
    let groups = if input.involvement != PullRequestInvolvement::All {
        vec![OwnedGroup {
            key: GroupKey::Others,
            label: String::new(),
            entries: entries.clone(),
        }]
    } else {
        let groups = if !input.baseline_empty && input.partitions_wanted {
            match (&authored, &reviewing) {
                (Some(authored), Some(reviewing)) => {
                    partition_with_priority(&entries, authored, reviewing)
                }
                _ => group_by_involvement(&entries, input.viewers),
            }
        } else {
            group_by_involvement(&entries, input.viewers)
        };
        groups
            .into_iter()
            .map(|g| OwnedGroup {
                key: g.key,
                label: g.key.label().into(),
                entries: g.entries.into_iter().cloned().collect(),
            })
            .collect()
    };
    let entries_by_stats_key = groups
        .iter()
        .flat_map(|g| g.entries.iter())
        .map(|e| (e.key(), e.clone()))
        .collect();
    let held_by_surface = groups
        .iter()
        .flat_map(|g| g.entries.iter())
        .map(|e| (surface_id(e, true), e.clone()))
        .collect();
    GroupResult {
        entries,
        listed_statuses,
        groups,
        entries_by_stats_key,
        held_by_surface,
    }
}
