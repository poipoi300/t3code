use crate::{Error, ProjectService, Result};
use std::sync::Arc;
use t3_contracts::{
    base_schemas::{ProjectId, ThreadId},
    project::Project,
};
use t3_orchestration::api::ProjectionReader;
use t3_workspace::scripts::{RunScriptResult, ScriptInput, ScriptRunner, ScriptTrigger};

#[derive(Clone)]
pub struct ProjectSetupScriptRunner {
    pub projects: ProjectService,
    pub projections: Arc<dyn ProjectionReader>,
    pub settings: Arc<t3_config::settings::SettingsService>,
    pub runner: ScriptRunner,
}
pub struct SetupScriptInput<'a> {
    pub thread_id: &'a ThreadId,
    pub project: Option<&'a Project>,
    pub project_id: Option<&'a ProjectId>,
    pub project_cwd: Option<&'a str>,
    pub worktree_path: &'a str,
    pub trigger: ScriptTrigger,
    pub observe_completion: bool,
    pub on_output_line: Option<Arc<dyn Fn(String) + Send + Sync>>,
}
impl ProjectSetupScriptRunner {
    pub async fn run_for_thread(&self, input: SetupScriptInput<'_>) -> Result<RunScriptResult> {
        let thread = self.projections.shell(input.thread_id.clone()).await?;
        let fenced = thread
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            .and_then(|v| v.get("linkOrigin").cloned())
            .filter(|v| !v.is_null())
            .is_some_and(|v| v["access"] != "full-access");
        if fenced {
            return Ok(RunScriptResult::SkippedForLink);
        }
        let project = if let Some(project) = input.project {
            Some(project.clone())
        } else if let Some(id) = input.project_id {
            self.projects.get_by_id(id, false).await?
        } else {
            None
        };
        let project = match (project, input.project_cwd) {
            (Some(p), _) => Some(p),
            (None, Some(cwd)) => self.projects.get_by_workspace_root(cwd, false).await?,
            _ => None,
        }
        .ok_or_else(|| Error::Invalid("Project for setup script was not found.".into()))?;
        let settings = self.settings.snapshot().await?;
        let settings = serde_json::to_value(settings)?;
        let mut script_settings = t3_workspace::project::ScriptSettings {
            defaults: serde_json::from_value(settings["defaultProjectScripts"].clone())?,
            folded: settings["projectSettingsFolded"] == true,
            ..Default::default()
        };
        if let Some(overrides) = settings["projectSettingsOverrides"].as_object() {
            for (id, value) in overrides {
                if let Some(scripts) = value.get("defaultProjectScripts") {
                    script_settings
                        .overrides
                        .insert(id.clone(), serde_json::from_value(scripts.clone())?);
                }
            }
        }
        if let Some(overrides) = settings["projectScriptOverrides"].as_object() {
            for (id, value) in overrides {
                script_settings
                    .legacy_overrides
                    .insert(id.clone(), serde_json::from_value(value.clone())?);
            }
        }
        let scripts = t3_workspace::project::resolve_scripts(
            &script_settings,
            &project.id.0,
            &project.scripts,
        );
        Ok(self
            .runner
            .run_for_thread(ScriptInput {
                thread_id: &input.thread_id.0,
                project_root: &project.workspace_root.0,
                worktree_path: input.worktree_path,
                scripts,
                trigger: input.trigger,
                fenced_link: false,
                observe_completion: input.observe_completion,
                on_output_line: input.on_output_line,
            })
            .await?)
    }
}
