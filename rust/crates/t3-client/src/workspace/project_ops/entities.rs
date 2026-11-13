use super::super::{ProjectKey, projects::EnvironmentProject};
use crate::{
    ClientStore,
    services::resources::{Resource, ResourcePolicy, ResourceSubscription},
    types::{EnvironmentId, Generation, ProjectId, ThreadKey},
};
use futures::{Stream, StreamExt};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_contracts::{
    orchestration_project::OrchestrationProjectShell, orchestration_v2::OrchestrationV2ThreadShell,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectEntitySnapshot {
    pub refs: Rc<Vec<ProjectKey>>,
    pub projects: Rc<Vec<Rc<EnvironmentProject>>>,
    pub configs_ready: bool,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvironmentProjectSnapshot {
    pub refs: Rc<Vec<ProjectKey>>,
    pub projects: Rc<Vec<Rc<EnvironmentProject>>>,
    pub index: Rc<BTreeMap<ProjectId, Rc<EnvironmentProject>>>,
}
#[derive(Debug, Clone)]
pub struct ProjectEntityInputs {
    pub catalog_ready: bool,
    pub configs: Vec<ProjectEnvironmentConfig>,
    pub require_config: bool,
}
#[derive(Debug, Clone)]
pub struct ProjectEnvironmentConfig {
    pub environment: EnvironmentId,
    pub enabled: bool,
    pub loaded: bool,
    pub scratch_workspace_root: Option<String>,
}
#[derive(Clone)]
pub struct ProjectEntities {
    state: Resource<ProjectEntitySnapshot>,
    sources: Rc<RefCell<BTreeMap<ProjectKey, Rc<OrchestrationProjectShell>>>>,
    environments: Rc<RefCell<BTreeMap<EnvironmentId, Resource<EnvironmentProjectSnapshot>>>>,
}
impl Default for ProjectEntities {
    fn default() -> Self {
        let state = Resource::new(ResourcePolicy::default(), Generation::default());
        let token = state.begin();
        state.finish(token, ProjectEntitySnapshot::default(), 0, false);
        Self {
            state,
            sources: Rc::default(),
            environments: Rc::default(),
        }
    }
}
impl ProjectEntities {
    pub fn snapshot(&self) -> Rc<ProjectEntitySnapshot> {
        self.state.snapshot().value.unwrap_or_default()
    }
    pub fn observe(&self) -> ResourceSubscription<ProjectEntitySnapshot> {
        self.state.observe(0)
    }
    fn environment_resource(
        &self,
        environment: &EnvironmentId,
    ) -> Resource<EnvironmentProjectSnapshot> {
        self.environments
            .borrow_mut()
            .entry(environment.clone())
            .or_insert_with(|| {
                let resource = Resource::new(ResourcePolicy::default(), Generation::default());
                let token = resource.begin();
                resource.finish(token, EnvironmentProjectSnapshot::default(), 0, false);
                resource
            })
            .clone()
    }
    pub fn environment_snapshot(
        &self,
        environment: &EnvironmentId,
    ) -> Rc<EnvironmentProjectSnapshot> {
        self.environment_resource(environment)
            .snapshot()
            .value
            .unwrap_or_default()
    }
    pub fn observe_environment(
        &self,
        environment: &EnvironmentId,
    ) -> ResourceSubscription<EnvironmentProjectSnapshot> {
        self.environment_resource(environment).observe(0)
    }
    pub fn project(&self, key: &ProjectKey) -> Option<Rc<EnvironmentProject>> {
        self.environment_snapshot(&key.environment)
            .index
            .get(&key.project)
            .cloned()
    }
    pub fn environment_projects(&self, environment: &EnvironmentId) -> Vec<Rc<EnvironmentProject>> {
        self.environment_snapshot(environment)
            .projects
            .as_ref()
            .clone()
    }
    pub fn refresh(
        &self,
        store: &ClientStore,
        catalog_ready: bool,
        configs: &[ProjectEnvironmentConfig],
        require_config: bool,
    ) {
        let previous = self.snapshot();
        let mut projects = Vec::new();
        let mut sources = BTreeMap::new();
        for config in configs {
            let old_environment = self.environment_snapshot(&config.environment);
            let by_key: BTreeMap<_, _> = old_environment
                .projects
                .iter()
                .map(|p| (p.key(), p))
                .collect();
            let mut environment_projects = Vec::new();
            let shell = store.shared_shell(&config.environment);
            for source in shell.iter().flat_map(|shell| shell.projects.iter()) {
                let key = ProjectKey {
                    environment: config.environment.clone(),
                    project: source.id.clone(),
                };
                let scratch = config
                    .scratch_workspace_root
                    .as_deref()
                    .is_some_and(|root| {
                        super::paths::find_project_by_path(
                            std::slice::from_ref(source),
                            root,
                            |p| &p.workspace_root.0,
                        )
                        .is_some()
                    });
                let project = by_key
                    .get(&key)
                    .filter(|p| {
                        p.is_scratch == scratch
                            && self
                                .sources
                                .borrow()
                                .get(&key)
                                .is_some_and(|old| Rc::ptr_eq(old, source))
                    })
                    .map_or_else(
                        || {
                            Rc::new(EnvironmentProject {
                                environment: config.environment.clone(),
                                project: source.as_ref().clone(),
                                is_scratch: scratch,
                            })
                        },
                        |p| (*p).clone(),
                    );
                sources.insert(key, source.clone());
                environment_projects.push(project);
            }
            let refs: Vec<_> = environment_projects.iter().map(|p| p.key()).collect();
            let refs = if old_environment.refs.as_ref() == &refs {
                old_environment.refs.clone()
            } else {
                Rc::new(refs)
            };
            let environment_projects = if environment_projects.len()
                == old_environment.projects.len()
                && environment_projects
                    .iter()
                    .zip(old_environment.projects.iter())
                    .all(|(a, b)| Rc::ptr_eq(a, b))
            {
                old_environment.projects.clone()
            } else {
                Rc::new(environment_projects)
            };
            if config.enabled {
                projects.extend(environment_projects.iter().cloned());
            }
            let index = if Rc::ptr_eq(&environment_projects, &old_environment.projects) {
                old_environment.index.clone()
            } else {
                Rc::new(
                    environment_projects
                        .iter()
                        .map(|project| (project.project.id.clone(), project.clone()))
                        .collect(),
                )
            };
            let next = EnvironmentProjectSnapshot {
                refs,
                projects: environment_projects,
                index,
            };
            if old_environment.as_ref() != &next {
                let resource = self.environment_resource(&config.environment);
                let token = resource.begin();
                resource.finish(token, next, 0, false);
            }
        }
        let retained: BTreeSet<_> = configs.iter().map(|c| &c.environment).collect();
        for (_, resource) in self
            .environments
            .borrow()
            .iter()
            .filter(|(environment, _)| !retained.contains(environment))
        {
            if resource
                .snapshot()
                .value
                .is_some_and(|value| !value.projects.is_empty())
            {
                let token = resource.begin();
                resource.finish(token, EnvironmentProjectSnapshot::default(), 0, false);
            }
        }
        let refs: Vec<_> = projects.iter().map(|p| p.key()).collect();
        let refs = if previous.refs.as_ref() == &refs {
            previous.refs.clone()
        } else {
            Rc::new(refs)
        };
        let projects = if projects.len() == previous.projects.len()
            && projects
                .iter()
                .zip(previous.projects.iter())
                .all(|(a, b)| Rc::ptr_eq(a, b))
        {
            previous.projects.clone()
        } else {
            Rc::new(projects)
        };
        *self.sources.borrow_mut() = sources;
        let next = ProjectEntitySnapshot {
            refs,
            projects,
            configs_ready: catalog_ready
                && (!require_config || configs.iter().filter(|c| c.enabled).all(|c| c.loaded)),
        };
        if previous.as_ref() != &next {
            let token = self.state.begin();
            self.state.finish(token, next, 0, false);
        }
    }
    pub async fn follow_store(
        &self,
        store: &ClientStore,
        catalog: impl Fn() -> (bool, Vec<ProjectEnvironmentConfig>, bool),
    ) {
        let mut changes = store.subscribe();
        while changes.next().await.is_some() {
            let (ready, configs, require) = catalog();
            self.refresh(store, ready, &configs, require);
        }
    }
    pub async fn follow_inputs(
        &self,
        store: &ClientStore,
        inputs: impl Stream<Item = ProjectEntityInputs>,
    ) {
        let changes = store.subscribe().map(|_| None);
        let mut changes = Box::pin(futures::stream::select(changes, inputs.map(Some)));
        let mut latest = None;
        while let Some(change) = changes.next().await {
            if let Some(change) = change {
                latest = Some(change);
            }
            if let Some(input) = &latest {
                self.refresh(
                    store,
                    input.catalog_ready,
                    &input.configs,
                    input.require_config,
                );
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProjectRemovalMember {
    pub key: ProjectKey,
    pub title: String,
    pub workspace_root: String,
    pub environment_label: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRemovalPlan {
    pub project_threads: Vec<ThreadKey>,
    pub is_whole_group: bool,
    pub target_kind: &'static str,
    pub confirmation_message: String,
}
pub fn plan_project_removal(
    members: &[ProjectRemovalMember],
    group_member_count: usize,
    has_other_members: bool,
    display_name: &str,
    threads: &[(EnvironmentId, OrchestrationV2ThreadShell)],
    unloaded_thread_count: usize,
) -> ProjectRemovalPlan {
    let member_keys: BTreeSet<_> = members.iter().map(|m| m.key.clone()).collect();
    let matching: Vec<_> = threads
        .iter()
        .filter(|(env, t)| {
            member_keys.contains(&ProjectKey {
                environment: env.clone(),
                project: t.project_id.clone(),
            })
        })
        .collect();
    let count = matching
        .iter()
        .filter(|(_, t)| {
            t.lineage.relationship_to_parent
                != Some(
                    t3_contracts::orchestration_v2::AppThreadLineageRelationshipToParent::Subagent,
                )
                && t.lineage.side_chat != Some(Some(true))
        })
        .count()
        + unloaded_thread_count;
    let is_whole_group = members.len() == group_member_count;
    let target_kind = if has_other_members || !is_whole_group {
        "checkout"
    } else {
        "project"
    };
    let single = (members.len() == 1).then(|| &members[0]);
    let label = single.map_or(display_name, |m| &m.title);
    let mut lines = vec![if count > 0 {
        format!(
            "Remove {target_kind} \"{label}\" and delete its {count} thread{}?",
            if count == 1 { "" } else { "s" }
        )
    } else {
        format!("Remove {target_kind} \"{label}\"?")
    }];
    if let Some(member) = single {
        lines.push(format!("Path: {}", member.workspace_root));
        if let Some(label) = member
            .environment_label
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            lines.push(format!("Environment: {label}"));
        }
    } else {
        lines.push(format!(
            "This removes {} grouped project entries.",
            members.len()
        ));
    }
    lines.push(if count > 0 { "Those threads and any archived threads are deleted from T3 Code. Provider sessions are kept." } else { "Any archived threads are deleted from T3 Code. Provider sessions are kept." }.into());
    lines.push(
        if is_whole_group && !has_other_members {
            "This removes only the project entries, not the files on disk."
        } else {
            "Other entries in this grouped project are unaffected."
        }
        .into(),
    );
    lines.push("This action cannot be undone.".into());
    ProjectRemovalPlan {
        project_threads: matching
            .iter()
            .map(|(environment, t)| ThreadKey {
                environment: environment.clone(),
                thread: t.id.clone(),
            })
            .collect(),
        is_whole_group,
        target_kind,
        confirmation_message: lines.join("\n"),
    }
}
