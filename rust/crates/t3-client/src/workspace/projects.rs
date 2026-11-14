use super::ProjectKey;
use crate::time::epoch_ms as timestamp;
use crate::types::EnvironmentId;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    environment::RepositoryIdentity, orchestration_project::OrchestrationProjectShell,
};

pub const SCRATCH_PROJECT_SCOPE_KEY: &str = "\0no-project";

pub fn is_windows_drive_path(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() >= 2
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b.len() == 2 || matches!(b[2], b'/' | b'\\'))
}
pub fn normalize_project_path_for_dispatch(value: &str) -> String {
    let value = value.trim();
    if matches!(value, "/" | "\\") || (value.len() == 3 && is_windows_drive_path(value)) {
        return value.into();
    }
    let trimmed = if value.starts_with('/') {
        value.trim_end_matches('/')
    } else {
        value.trim_end_matches(['/', '\\'])
    };
    if trimmed.is_empty() {
        return value.into();
    }
    if trimmed.len() == 2 && is_windows_drive_path(trimmed) {
        format!("{trimmed}\\")
    } else {
        trimmed.into()
    }
}
pub fn normalize_project_path_for_comparison(value: &str) -> String {
    let path = normalize_project_path_for_dispatch(value);
    if is_windows_drive_path(&path) || path.starts_with("\\\\") {
        path.replace('/', "\\").to_lowercase()
    } else {
        path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysicalProjectKey {
    pub environment: EnvironmentId,
    pub path: String,
}
impl PhysicalProjectKey {
    pub fn new(environment: EnvironmentId, path: &str) -> Self {
        Self {
            environment,
            path: normalize_project_path_for_comparison(path),
        }
    }
    // Existing preferences use this historical format; runtime maps use the structured key.
    pub fn legacy_preference_key(&self) -> String {
        format!("{}:{}", self.environment.0, self.path)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GroupingMode {
    #[default]
    Repository,
    RepositoryPath,
    Separate,
}
#[derive(Debug, Clone, Default)]
pub struct GroupingSettings {
    pub mode: GroupingMode,
    pub overrides: BTreeMap<PhysicalProjectKey, GroupingMode>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentProject {
    pub environment: EnvironmentId,
    pub project: OrchestrationProjectShell,
    pub is_scratch: bool,
}
impl EnvironmentProject {
    pub fn key(&self) -> ProjectKey {
        ProjectKey {
            environment: self.environment.clone(),
            project: self.project.id.clone(),
        }
    }
    pub fn physical_key(&self) -> PhysicalProjectKey {
        PhysicalProjectKey::new(self.environment.clone(), &self.project.workspace_root.0)
    }
    fn identity(&self) -> Option<&RepositoryIdentity> {
        self.project
            .repository_identity
            .as_ref()
            .and_then(Option::as_ref)
    }
    fn freshness(&self) -> i64 {
        timestamp(&self.project.updated_at.0)
            .or_else(|| timestamp(&self.project.created_at.0))
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogicalProjectKey {
    Scratch,
    Repository(String),
    Physical(PhysicalProjectKey),
}
fn repository_key(project: &EnvironmentProject, mode: GroupingMode) -> Option<String> {
    let identity = project.identity()?;
    let canonical = identity
        .origin
        .as_ref()
        .map_or(&identity.canonical_key.0, |o| &o.canonical_key.0);
    if canonical.is_empty() || canonical.contains('\0') {
        return None;
    }
    if mode == GroupingMode::Repository {
        return Some(canonical.clone());
    }
    let root = identity
        .root_path
        .as_ref()
        .map(|v| normalize_project_path_for_comparison(&v.0));
    let path = normalize_project_path_for_comparison(&project.project.workspace_root.0);
    let relative = root.filter(|r| !r.is_empty()).and_then(|root| {
        if root == path {
            return Some(String::new());
        }
        path.strip_prefix(&format!(
            "{root}{}",
            if root.contains('\\') { "\\" } else { "/" }
        ))
        .map(|v| v.replace('\\', "/"))
    });
    Some(match relative {
        Some(r) if !r.is_empty() => format!("{canonical}::{r}"),
        _ => canonical.clone(),
    })
}
pub fn logical_project_key(
    project: &EnvironmentProject,
    settings: &GroupingSettings,
    group_scratch: bool,
) -> LogicalProjectKey {
    if group_scratch && project.is_scratch {
        return LogicalProjectKey::Scratch;
    }
    let physical = project.physical_key();
    let mode = settings
        .overrides
        .get(&physical)
        .copied()
        .unwrap_or(settings.mode);
    if mode != GroupingMode::Separate
        && let Some(key) = repository_key(project, mode)
    {
        return LogicalProjectKey::Repository(key);
    }
    LogicalProjectKey::Physical(physical)
}
pub fn resolve_scratch_scope(
    key: Option<LogicalProjectKey>,
    projects: &[EnvironmentProject],
    settings: &GroupingSettings,
) -> Option<LogicalProjectKey> {
    let key = key?;
    if key == LogicalProjectKey::Scratch
        || projects
            .iter()
            .any(|p| logical_project_key(p, settings, true) == key)
    {
        return Some(key);
    }
    if projects.iter().any(|p| {
        p.is_scratch
            && (key == LogicalProjectKey::Physical(p.physical_key())
                || logical_project_key(p, settings, false) == key)
    }) {
        Some(LogicalProjectKey::Scratch)
    } else {
        Some(key)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectGroup {
    pub key: LogicalProjectKey,
    pub label: String,
    pub representative: EnvironmentProject,
    pub members: Vec<EnvironmentProject>,
    pub member_project_refs: Vec<ProjectKey>,
    pub pinned: bool,
    pub archived: bool,
}
fn unique(values: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && seen.insert(v.clone()))
        .collect()
}
fn group_label(members: &[EnvironmentProject], representative: &EnvironmentProject) -> String {
    let titles = unique(members.iter().map(|p| p.project.title.0.clone()));
    let displays = unique(members.iter().filter_map(|p| p.identity()).filter_map(|i| {
        i.origin
            .as_ref()
            .map(|o| {
                o.display_name
                    .as_ref()
                    .unwrap_or(&o.canonical_key)
                    .0
                    .clone()
            })
            .or_else(|| i.display_name.as_ref().map(|v| v.0.clone()))
    }));
    let names = unique(
        members
            .iter()
            .filter_map(|p| p.identity()?.name.as_ref().map(|v| v.0.clone())),
    );
    if titles.len() == 1
        && (members
            .iter()
            .any(|p| p.project.has_custom_title == Some(Some(true)))
            || (!displays.contains(&titles[0]) && !names.contains(&titles[0])))
    {
        return titles[0].clone();
    }
    if displays.len() == 1 {
        return displays[0].clone();
    }
    if names.len() == 1 {
        return names[0].clone();
    }
    representative.project.title.0.clone()
}
pub fn build_project_groups(
    projects: &[EnvironmentProject],
    settings: &GroupingSettings,
    preferred: Option<&EnvironmentId>,
    group_scratch: bool,
) -> Vec<ProjectGroup> {
    let mut physical: Vec<(PhysicalProjectKey, Vec<&EnvironmentProject>)> = Vec::new();
    let mut physical_index = BTreeMap::new();
    for p in projects {
        let key = p.physical_key();
        let index = *physical_index.entry(key.clone()).or_insert_with(|| {
            physical.push((key, Vec::new()));
            physical.len() - 1
        });
        physical[index].1.push(p);
    }
    let mut groups: Vec<ProjectGroup> = Vec::new();
    let mut index_by_logical = BTreeMap::new();
    let mut logical_by_physical = BTreeMap::new();
    for (physical_key, candidates) in physical {
        let compare = |a: &&EnvironmentProject, b: &&EnvironmentProject| {
            a.freshness()
                .cmp(&b.freshness())
                .then_with(|| a.project.id.cmp(&b.project.id))
        };
        let Some(winner) = candidates
            .iter()
            .copied()
            .reduce(|a, b| if compare(&a, &b).is_lt() { b } else { a })
        else {
            continue;
        };
        let identity = if winner.identity().is_some() {
            winner
        } else {
            candidates
                .iter()
                .copied()
                .filter(|p| p.identity().is_some())
                .reduce(|a, b| if compare(&a, &b).is_lt() { b } else { a })
                .unwrap_or(winner)
        };
        let mode = settings
            .overrides
            .get(&physical_key)
            .copied()
            .unwrap_or(settings.mode);
        let key = logical_project_key(
            identity,
            &GroupingSettings {
                mode,
                ..GroupingSettings::default()
            },
            group_scratch,
        );
        logical_by_physical.insert(physical_key, key.clone());
        let i = *index_by_logical.entry(key.clone()).or_insert_with(|| {
            groups.push(ProjectGroup {
                key,
                label: String::new(),
                representative: winner.clone(),
                members: Vec::new(),
                member_project_refs: Vec::new(),
                pinned: false,
                archived: true,
            });
            groups.len() - 1
        });
        groups[i].members.push(winner.clone());
    }
    let mut seen = BTreeSet::new();
    for p in projects {
        if !seen.insert(p.key()) {
            continue;
        }
        if let Some(i) = logical_by_physical
            .get(&p.physical_key())
            .and_then(|key| index_by_logical.get(key))
        {
            groups[*i].member_project_refs.push(p.key());
        }
    }
    for g in &mut groups {
        if let Some(p) = g.members.iter().find(|p| Some(&p.environment) == preferred) {
            g.representative = p.clone();
        }
        g.label = if group_scratch && g.representative.is_scratch {
            "No project".into()
        } else if g.members.len() > 1 {
            group_label(&g.members, &g.representative)
        } else {
            g.representative.project.title.0.clone()
        };
        g.pinned = g.members.iter().any(|p| {
            p.project
                .pinned_at
                .as_ref()
                .and_then(Option::as_ref)
                .is_some()
        });
        g.archived = g.members.iter().all(|p| {
            p.project
                .archived_at
                .as_ref()
                .and_then(Option::as_ref)
                .is_some()
        });
    }
    groups
}
pub fn pickable_project_groups(groups: &[ProjectGroup]) -> Vec<&ProjectGroup> {
    groups
        .iter()
        .filter(|g| !g.archived && g.pinned)
        .chain(groups.iter().filter(|g| !g.archived && !g.pinned))
        .collect()
}
pub fn home_project_filter_scope<'a>(
    groups: &'a [ProjectGroup],
    projects: &[EnvironmentProject],
    selected: Option<LogicalProjectKey>,
    mode: GroupingMode,
) -> Option<&'a ProjectGroup> {
    let settings = GroupingSettings {
        mode,
        overrides: BTreeMap::new(),
    };
    let key = resolve_scratch_scope(selected, projects, &settings)?;
    pickable_project_groups(groups)
        .into_iter()
        .find(|group| group.key == key)
}

pub fn sort_projects_by_group_order(projects: &mut [EnvironmentProject], groups: &[ProjectGroup]) {
    let mut rank = BTreeMap::new();
    for (i, g) in groups.iter().enumerate() {
        for key in &g.member_project_refs {
            rank.entry(key.clone()).or_insert(i);
        }
    }
    projects.sort_by_key(|p| rank.get(&p.key()).copied().unwrap_or(groups.len()));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentPresence {
    LocalOnly,
    RemoteOnly,
    Mixed,
}
pub fn environment_presence(
    group: &ProjectGroup,
    primary: Option<&EnvironmentId>,
) -> EnvironmentPresence {
    let local = group
        .members
        .iter()
        .any(|p| Some(&p.environment) == primary);
    let remote = primary.is_some()
        && group
            .members
            .iter()
            .any(|p| Some(&p.environment) != primary);
    match (local, remote) {
        (true, true) => EnvironmentPresence::Mixed,
        (_, true) => EnvironmentPresence::RemoteOnly,
        _ => EnvironmentPresence::LocalOnly,
    }
}
pub fn picker_target<'a>(
    group: &'a ProjectGroup,
    preferred: Option<&ProjectKey>,
) -> &'a EnvironmentProject {
    preferred
        .and_then(|key| {
            group.members.iter().find(|p| p.key() == *key).or_else(|| {
                group
                    .members
                    .iter()
                    .find(|p| p.environment == key.environment)
            })
        })
        .unwrap_or(&group.representative)
}
