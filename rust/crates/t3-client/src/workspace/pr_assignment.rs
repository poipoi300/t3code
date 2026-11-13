use super::{ProjectKey, projects::EnvironmentProject};
use crate::types::{EnvironmentId, ProjectId};
use std::collections::BTreeMap;
use t3_contracts::{
    environment::EnvironmentMachineKind,
    pull_request::{PULL_REQUEST_LIST_PROJECT_LIMIT, PullRequestProviderSummary},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAvailability {
    pub configured: bool,
    pub observed_at: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHost {
    pub host: String,
    pub observed_at: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostRead {
    pub observed_at: u64,
    pub reset_at: Option<u64>,
    pub hosts: BTreeMap<String, HostAvailability>,
    pub project_hosts: BTreeMap<ProjectId, ProjectHost>,
}
pub fn reset_hosts(reads: &mut BTreeMap<EnvironmentId, HostRead>) {
    for read in reads.values_mut() {
        read.reset_at = Some(read.observed_at);
        read.hosts.clear();
        read.project_hosts.clear();
    }
}
pub struct HostObservation<'a> {
    pub environment: &'a EnvironmentId,
    pub providers: &'a [PullRequestProviderSummary],
    pub observed_at: u64,
}
pub fn remember_hosts(
    previous: &mut BTreeMap<EnvironmentId, HostRead>,
    observations: &[HostObservation<'_>],
) {
    let mut active: BTreeMap<(EnvironmentId, String), HostAvailability> = BTreeMap::new();
    for observation in observations {
        if previous
            .get(observation.environment)
            .and_then(|r| r.reset_at)
            .is_some_and(|reset| observation.observed_at <= reset)
        {
            continue;
        }
        for provider in observation.providers {
            let value = active
                .entry((
                    observation.environment.clone(),
                    provider.host.0.to_lowercase(),
                ))
                .or_insert(HostAvailability {
                    configured: false,
                    observed_at: observation.observed_at,
                });
            value.configured |= provider.configured;
            value.observed_at = value.observed_at.max(observation.observed_at);
        }
    }
    for observation in observations {
        if previous
            .get(observation.environment)
            .and_then(|r| r.reset_at)
            .is_some_and(|reset| observation.observed_at <= reset)
        {
            continue;
        }
        let mut next = previous
            .get(observation.environment)
            .cloned()
            .unwrap_or_default();
        let mut changed = false;
        let mut latest = observation.observed_at;
        for provider in observation.providers {
            let host = provider.host.0.to_lowercase();
            let Some(current) = active.get(&(observation.environment.clone(), host.clone())) else {
                continue;
            };
            if next.hosts.get(&host).is_none_or(|held| {
                current.observed_at > held.observed_at
                    || (current.observed_at == held.observed_at
                        && current.configured
                        && !held.configured)
            }) {
                next.hosts.insert(host.clone(), current.clone());
                latest = latest.max(current.observed_at);
                changed = true;
            }
            for project in provider
                .project_ids
                .as_ref()
                .and_then(Option::as_ref)
                .into_iter()
                .flatten()
            {
                if next
                    .project_hosts
                    .get(project)
                    .is_none_or(|held| observation.observed_at > held.observed_at)
                {
                    next.project_hosts.insert(
                        project.clone(),
                        ProjectHost {
                            host: host.clone(),
                            observed_at: observation.observed_at,
                        },
                    );
                    changed = true;
                }
            }
        }
        if changed {
            next.observed_at = next.observed_at.max(latest);
            previous.insert(observation.environment.clone(), next);
        }
    }
}
fn repository_key(project: &EnvironmentProject) -> Option<String> {
    project
        .project
        .repository_identity
        .as_ref()
        .and_then(Option::as_ref)
        .map(|i| i.canonical_key.0.to_lowercase())
}
pub fn assign_projects(
    projects: &[EnvironmentProject],
    environments: &[EnvironmentId],
    preferred: Option<&EnvironmentId>,
    availability: &BTreeMap<EnvironmentId, HostRead>,
    resolve_host: impl Fn(&EnvironmentProject) -> String,
) -> BTreeMap<EnvironmentId, Vec<ProjectId>> {
    let rank: BTreeMap<_, _> = environments
        .iter()
        .enumerate()
        .map(|(i, e)| (e, i))
        .collect();
    let mut owner: BTreeMap<String, &EnvironmentProject> = BTreeMap::new();
    let unreadable = |p: &EnvironmentProject| {
        let availability = availability.get(&p.environment);
        let host = availability
            .and_then(|a| a.project_hosts.get(&p.project.id))
            .map(|v| v.host.clone())
            .unwrap_or_else(|| resolve_host(p));
        availability
            .and_then(|a| a.hosts.get(&host))
            .is_some_and(|h| !h.configured)
    };
    for project in projects {
        let Some(key) = repository_key(project).filter(|k| !k.is_empty()) else {
            continue;
        };
        let Some(candidate_rank) = rank.get(&project.environment) else {
            continue;
        };
        let Some(current) = owner.get(&key) else {
            owner.insert(key, project);
            continue;
        };
        let replace = if unreadable(current) != unreadable(project) {
            unreadable(current)
        } else if Some(&current.environment) == preferred {
            false
        } else {
            Some(&project.environment) == preferred
                || candidate_rank < rank.get(&current.environment).unwrap_or(&usize::MAX)
        };
        if replace {
            owner.insert(key, project);
        }
    }
    let mut assignment: BTreeMap<EnvironmentId, Vec<ProjectId>> = BTreeMap::new();
    for project in projects {
        if !rank.contains_key(&project.environment) {
            continue;
        }
        if let Some(key) = repository_key(project).filter(|k| !k.is_empty())
            && owner
                .get(&key)
                .is_none_or(|o| o.environment != project.environment)
        {
            continue;
        }
        assignment
            .entry(project.environment.clone())
            .or_default()
            .push(project.project.id.clone());
    }
    assignment
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectQuery {
    pub environment: EnvironmentId,
    pub project_ids: Option<Vec<ProjectId>>,
}
pub fn project_queries(
    projects: &[EnvironmentProject],
    environments: &[EnvironmentId],
    availability: &BTreeMap<EnvironmentId, HostRead>,
    resolve_host: impl Fn(&EnvironmentProject) -> String,
) -> Vec<ProjectQuery> {
    let assignment = assign_projects(
        projects,
        environments,
        environments.first(),
        availability,
        resolve_host,
    );
    let mut result = Vec::new();
    for environment in environments {
        let Some(ids) = assignment.get(environment) else {
            continue;
        };
        if ids.len()
            == projects
                .iter()
                .filter(|p| &p.environment == environment)
                .count()
        {
            result.push(ProjectQuery {
                environment: environment.clone(),
                project_ids: None,
            });
        } else {
            for chunk in ids.chunks(PULL_REQUEST_LIST_PROJECT_LIMIT) {
                result.push(ProjectQuery {
                    environment: environment.clone(),
                    project_ids: Some(chunk.to_vec()),
                });
            }
        }
    }
    result
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickableEnvironment {
    pub project: ProjectKey,
    pub workspace_root: String,
    pub label: String,
    pub machine: Option<EnvironmentMachineKind>,
}
#[derive(Debug, Clone)]
pub struct EnvironmentLabel {
    pub environment: EnvironmentId,
    pub label: String,
    pub machine: Option<EnvironmentMachineKind>,
}
pub fn pickable_environments(
    current: &ProjectKey,
    projects: &[EnvironmentProject],
    environments: &[EnvironmentLabel],
) -> Vec<PickableEnvironment> {
    let Some(own) = projects.iter().find(|p| p.key() == *current) else {
        return Vec::new();
    };
    let Some(key) = repository_key(own).filter(|k| !k.is_empty()) else {
        return Vec::new();
    };
    let Some(own_env) = environments
        .iter()
        .find(|e| e.environment == current.environment)
    else {
        return Vec::new();
    };
    let pick = |p: &EnvironmentProject, e: &EnvironmentLabel| PickableEnvironment {
        project: p.key(),
        workspace_root: p.project.workspace_root.0.clone(),
        label: e.label.clone(),
        machine: e.machine.clone(),
    };
    let others: Vec<_> = environments
        .iter()
        .filter(|e| e.environment != current.environment)
        .filter_map(|e| {
            projects
                .iter()
                .find(|p| {
                    p.environment == e.environment && repository_key(p).as_ref() == Some(&key)
                })
                .map(|p| pick(p, e))
        })
        .collect();
    if others.is_empty() {
        return Vec::new();
    }
    std::iter::once(pick(own, own_env)).chain(others).collect()
}
