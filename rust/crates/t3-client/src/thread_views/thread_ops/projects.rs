use super::ThreadOperations;
use crate::{
    ClientError,
    platform::Platform,
    types::EnvironmentId,
    view_logic::shell::ui_state::UiPreferences,
    workspace::{
        ProjectKey,
        project_ops::{ProjectEntitySnapshot, ProjectEnvironmentConfig},
        projects::EnvironmentProject,
    },
};
use futures::{FutureExt, future::LocalBoxFuture};
use serde_json::{Value, json};
use std::rc::Rc;
use t3_contracts::{project::ProjectMutation, rpc::WsProjectsMutateRpc};

impl<P: Platform + Clone + 'static> ThreadOperations<P> {
    pub fn projects(&self) -> Rc<ProjectEntitySnapshot> {
        let snapshot = self.runtime.store().snapshot();
        let configs: Vec<_> = self
            .runtime
            .environments()
            .iter()
            .map(|e| {
                let config = snapshot
                    .environments
                    .get(&e.id)
                    .and_then(|e| e.config.as_ref());
                ProjectEnvironmentConfig {
                    environment: e.id.clone(),
                    enabled: e.state.desired,
                    loaded: config.is_some(),
                    scratch_workspace_root: config
                        .and_then(|c| c.scratch_workspace_root.as_ref())
                        .map(|p| p.0.clone()),
                }
            })
            .collect();
        drop(snapshot);
        self.state
            .projects
            .refresh(self.runtime.store(), true, &configs, false);
        self.state.projects.snapshot()
    }
    pub fn project(&self, key: &ProjectKey) -> Option<Rc<EnvironmentProject>> {
        self.projects()
            .projects
            .iter()
            .find(|p| p.key() == *key)
            .cloned()
    }
    pub fn can_organize_projects(&self, members: &[ProjectKey]) -> bool {
        !members.is_empty()
            && members
                .iter()
                .all(|member| self.check_project(member, true).is_ok())
    }
    pub fn update_project_group(
        &self,
        members: Vec<ProjectKey>,
        update: Value,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            if members.is_empty() {
                return Ok(());
            }
            if !update.is_object() {
                return Err(ClientError::InvalidPayload(
                    "project update must be an object".into(),
                ));
            }
            let organizing = update.get("pinned").is_some() || update.get("archived").is_some();
            for member in &members {
                service.check_project(member, organizing)?;
            }
            for member in members {
                service.check_project(&member, organizing)?;
                let endpoint = service.endpoint(&member.environment)?;
                let generation = service.session_generation(&member.environment);
                let mut input = update.clone();
                input["type"] = json!("project.update");
                input["projectId"] = json!(member.project);
                input["commandId"] = json!(crate::operations::allocate_command_id(
                    &service.platform,
                    None
                )?);
                let mutation: ProjectMutation = serde_json::from_value(input)?;
                service
                    .state
                    .files
                    .mutate(member.environment.clone(), &endpoint, mutation)
                    .await
                    .map_err(|error| {
                        service.command_failure(
                            &member.environment,
                            "projects.mutate",
                            generation,
                            error,
                        )
                    })?;
            }
            service.state.actions.notify();
            Ok(())
        }
        .boxed_local()
    }
    fn check_project(&self, key: &ProjectKey, organizing: bool) -> Result<(), ClientError> {
        if self.unsupported(&key.environment, "projects.mutate") {
            return Err(ClientError::Unsupported("projects.mutate"));
        }
        let endpoint = self.endpoint(&key.environment)?;
        if organizing
            && !self
                .runtime
                .store()
                .snapshot()
                .environments
                .get(&key.environment)
                .and_then(|e| e.config.as_ref())
                .is_some_and(|c| c.environment.capabilities.project_organization == Some(true))
        {
            return Err(ClientError::Unsupported("project organization"));
        }
        let input: ProjectMutation = serde_json::from_value(
            json!({"type":"project.update","commandId":"availability","projectId":key.project}),
        )?;
        if !endpoint.available::<WsProjectsMutateRpc>(&input) {
            return Err(ClientError::PermissionDenied {
                method: "projects.mutate".into(),
            });
        }
        Ok(())
    }
    pub fn rename_project(
        &self,
        key: ProjectKey,
        title: String,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let title = t3_contracts::wire::trim_js(&title);
        if title.is_empty() {
            return futures::future::ready(Err(Self::refused("Project title cannot be empty.")))
                .boxed_local();
        }
        self.update_project_group(vec![key], json!({"title":title}))
    }
    pub fn set_project_group_pinned(
        &self,
        members: Vec<ProjectKey>,
        pinned: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        self.update_project_group(members, json!({"pinned":pinned}))
    }
    pub fn remove_project(
        &self,
        key: ProjectKey,
        force: bool,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            service.check_project(&key, false)?;
            let endpoint = service.endpoint(&key.environment)?;
            let generation=service.session_generation(&key.environment);
            service.state.files.mutate(key.environment.clone(), &endpoint, serde_json::from_value(json!({"type":"project.delete","commandId":crate::operations::allocate_command_id(&service.platform,None)?,"projectId":key.project,"force":force}))?).await.map_err(|error| service.command_failure(&key.environment,"projects.mutate",generation,error))?;
            service.state.actions.notify(); Ok(())
        }.boxed_local()
    }
    pub fn reorder_projects(
        &self,
        current: Vec<String>,
        dragged: Vec<String>,
        target: Vec<String>,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        let service = self.clone();
        async move {
            let mut preferences = UiPreferences::load(&service.platform).await?;
            preferences
                .state
                .reorder_projects(&current, &dragged, &target);
            preferences.save(&service.platform).await?;
            service.state.actions.notify();
            Ok(())
        }
        .boxed_local()
    }
    pub fn project_changes(
        &self,
        environment: &EnvironmentId,
    ) -> futures::stream::LocalBoxStream<'static, ()> {
        self.changes(&crate::types::ThreadKey {
            environment: environment.clone(),
            thread: crate::types::ThreadId(String::new()),
        })
    }
}
