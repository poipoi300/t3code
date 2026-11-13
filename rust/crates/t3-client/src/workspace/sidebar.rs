use super::{ProjectKey, projects::*};
use crate::types::EnvironmentId;
use std::collections::{BTreeMap, BTreeSet};

pub fn grouping_mode_label(mode: GroupingMode) -> &'static str {
    match mode {
        GroupingMode::Repository => "Group by repository",
        GroupingMode::RepositoryPath => "Group by repository path",
        GroupingMode::Separate => "Keep separate",
    }
}
pub fn with_grouping_override(
    overrides: &BTreeMap<PhysicalProjectKey, GroupingMode>,
    key: PhysicalProjectKey,
    selection: Option<GroupingMode>,
) -> BTreeMap<PhysicalProjectKey, GroupingMode> {
    let mut next = overrides.clone();
    if let Some(mode) = selection {
        next.insert(key, mode);
    } else {
        next.remove(&key);
    }
    next
}
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarProjectSnapshot {
    pub group: ProjectGroup,
    pub environment_presence: EnvironmentPresence,
    pub environment_labels: BTreeMap<PhysicalProjectKey, Option<String>>,
    pub remote_environment_labels: Vec<String>,
    pub all_remote_members_are_desktop_local: bool,
    pub all_remote_members_are_wsl: bool,
}
pub fn groups_span_environments(groups: &[SidebarProjectSnapshot]) -> bool {
    let mut environments = BTreeSet::new();
    for group in groups {
        for member in &group.group.members {
            environments.insert(&member.environment);
            if environments.len() > 1 {
                return true;
            }
        }
    }
    false
}
pub fn physical_to_logical_map(
    projects: &[EnvironmentProject],
    settings: &GroupingSettings,
    primary: Option<&EnvironmentId>,
) -> BTreeMap<PhysicalProjectKey, LogicalProjectKey> {
    build_project_groups(projects, settings, primary, false)
        .into_iter()
        .flat_map(|group| {
            group
                .members
                .into_iter()
                .map(move |member| (member.physical_key(), group.key.clone()))
        })
        .collect()
}
pub fn settings_key(
    group: &SidebarProjectSnapshot,
    settings: &GroupingSettings,
    projects: &[EnvironmentProject],
    project: Option<&ProjectKey>,
) -> LogicalProjectKey {
    if !group.group.representative.is_scratch {
        return group.group.key.clone();
    }
    let member = project
        .and_then(|key| {
            group
                .group
                .members
                .iter()
                .find(|p| p.key() == *key)
                .or_else(|| {
                    group
                        .group
                        .members
                        .iter()
                        .find(|p| p.environment == key.environment)
                })
        })
        .unwrap_or(&group.group.representative);
    physical_to_logical_map(projects, settings, None)
        .remove(&member.physical_key())
        .unwrap_or_else(|| group.group.key.clone())
}
pub fn resolve_scope(
    groups: &[SidebarProjectSnapshot],
    key: Option<LogicalProjectKey>,
    settings: &GroupingSettings,
    clear_missing: bool,
) -> Option<LogicalProjectKey> {
    let key = key?;
    let group = groups.iter().find(|g| g.group.key == key).or_else(|| {
        groups.iter().find(|g| {
            g.group.representative.is_scratch
                && resolve_scratch_scope(Some(key.clone()), &g.group.members, settings)
                    == Some(g.group.key.clone())
        })
    });
    group
        .map(|g| g.group.key.clone())
        .or_else(|| (!clear_missing).then_some(key))
}
pub fn resolve_scopes(
    groups: &[SidebarProjectSnapshot],
    keys: &[LogicalProjectKey],
    settings: &GroupingSettings,
    clear_missing: bool,
) -> Vec<LogicalProjectKey> {
    let mut seen = BTreeSet::new();
    keys.iter()
        .filter_map(|key| resolve_scope(groups, Some(key.clone()), settings, clear_missing))
        .filter(|key| seen.insert(key.clone()))
        .collect()
}
pub fn build_sidebar_snapshots(
    projects: &[EnvironmentProject],
    settings: &GroupingSettings,
    group_scratch: bool,
    primary: Option<&EnvironmentId>,
    label: impl Fn(&EnvironmentId) -> Option<String>,
    desktop_local: impl Fn(&EnvironmentId) -> bool,
    wsl: impl Fn(&EnvironmentId) -> bool,
) -> Vec<SidebarProjectSnapshot> {
    build_project_groups(projects, settings, primary, group_scratch)
        .into_iter()
        .map(|group| {
            let environment_presence = environment_presence(&group, primary);
            let environment_labels: BTreeMap<_, _> = group
                .members
                .iter()
                .map(|p| (p.physical_key(), label(&p.environment)))
                .collect();
            let remote: Vec<_> = group
                .members
                .iter()
                .filter(|p| primary.is_some() && Some(&p.environment) != primary)
                .collect();
            let mut labels_seen = BTreeSet::new();
            let remote_environment_labels = remote
                .iter()
                .filter_map(|p| environment_labels.get(&p.physical_key()).cloned().flatten())
                .filter(|l| !l.is_empty() && labels_seen.insert(l.clone()))
                .collect();
            let all_remote_members_are_desktop_local =
                !remote.is_empty() && remote.iter().all(|p| desktop_local(&p.environment));
            let all_remote_members_are_wsl =
                !remote.is_empty() && remote.iter().all(|p| wsl(&p.environment));
            SidebarProjectSnapshot {
                group,
                environment_presence,
                environment_labels,
                remote_environment_labels,
                all_remote_members_are_desktop_local,
                all_remote_members_are_wsl,
            }
        })
        .collect()
}
pub struct SidebarPickerEntry<'a> {
    pub group: &'a SidebarProjectSnapshot,
    pub target: &'a EnvironmentProject,
    pub preferred: bool,
}
pub fn picker_entries<'a>(
    groups: &'a [SidebarProjectSnapshot],
    preferred: Option<&ProjectKey>,
) -> Vec<SidebarPickerEntry<'a>> {
    let mut entries: Vec<_> = groups
        .iter()
        .map(|group| SidebarPickerEntry {
            group,
            target: picker_target(&group.group, preferred),
            preferred: preferred.is_some_and(|key| group.group.member_project_refs.contains(key)),
        })
        .collect();
    if let Some(index) = entries.iter().position(|e| e.preferred) {
        let entry = entries.remove(index);
        entries.insert(0, entry);
    }
    entries
}
