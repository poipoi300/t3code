use crate::{
    types::EnvironmentId,
    workspace::{ProjectKey, projects::EnvironmentProject},
};
use std::collections::BTreeSet;

fn archived(project: &EnvironmentProject) -> bool {
    project
        .project
        .archived_at
        .as_ref()
        .and_then(Option::as_ref)
        .is_some()
}

pub fn filtered_project(
    projects: &[EnvironmentProject],
    representative: &EnvironmentProject,
    preferred: Option<&EnvironmentId>,
    connected: &BTreeSet<EnvironmentId>,
    last: Option<&ProjectKey>,
) -> Option<ProjectKey> {
    let live = projects
        .iter()
        .filter(|project| !archived(project))
        .collect::<Vec<_>>();
    if let Some(project) = live.iter().find(|project| last == Some(&project.key())) {
        return Some(project.key());
    }
    let rank = |project: &EnvironmentProject| {
        if preferred == Some(&project.environment) {
            0
        } else if project.key() == representative.key() {
            1
        } else {
            2
        }
    };
    live.iter()
        .filter(|project| connected.contains(&project.environment))
        .min_by_key(|project| rank(project))
        .or_else(|| live.iter().min_by_key(|project| rank(project)))
        .map(|project| project.key())
}

pub fn destination(
    projects: &[EnvironmentProject],
    use_last: Option<bool>,
    current: Option<&ProjectKey>,
    filtered: Option<&ProjectKey>,
    last: Option<&ProjectKey>,
) -> Option<ProjectKey> {
    if use_last == Some(false) {
        return None;
    }
    let find =
        |key: Option<&ProjectKey>| projects.iter().find(|project| key == Some(&project.key()));
    find(current)
        .or_else(|| find(filtered))
        .or_else(|| find(last).filter(|project| !archived(project)))
        .map(EnvironmentProject::key)
}

fn repository_key(project: &EnvironmentProject) -> Option<&str> {
    let identity = project.project.repository_identity.as_ref()?.as_ref()?;
    Some(
        identity
            .origin
            .as_ref()
            .map_or(&identity.canonical_key.0, |origin| &origin.canonical_key.0),
    )
}

pub fn same_repository(
    left: Option<&EnvironmentProject>,
    right: Option<&EnvironmentProject>,
) -> bool {
    left.and_then(repository_key)
        .zip(right.and_then(repository_key))
        .is_some_and(|(left, right)| left == right)
}

pub fn environment_project_match<'a>(
    projects: &'a [EnvironmentProject],
    selected: Option<&EnvironmentProject>,
) -> Option<&'a EnvironmentProject> {
    let key = selected.and_then(repository_key);
    let basename = selected
        .and_then(|project| project.project.workspace_root.0.rsplit('/').next())
        .filter(|part| !part.is_empty());
    let mismatch = |project: &EnvironmentProject| {
        key.zip(repository_key(project))
            .is_some_and(|(left, right)| left != right)
    };
    key.and_then(|key| {
        projects
            .iter()
            .find(|project| repository_key(project) == Some(key))
    })
    .or_else(|| {
        basename.and_then(|basename| {
            projects.iter().find(|project| {
                !mismatch(project)
                    && project.project.workspace_root.0.rsplit('/').next() == Some(basename)
            })
        })
    })
    .or_else(|| {
        selected.and_then(|selected| {
            projects.iter().find(|project| {
                !mismatch(project) && project.project.title == selected.project.title
            })
        })
    })
    .or_else(|| projects.first())
}
