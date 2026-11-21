//! Port of useProjectScriptSettings.ts for settings scoped to checkouts.
use super::{SettingsScopeView, SettingsService, SettingsUpdateOutcome};
use crate::{
    ClientError,
    platform::Platform,
    rpc::{RpcCallError, RpcEndpoint},
    services::settings::{
        scope::{ScopeMember, SettingsScopeSearch},
        scoped::{ScopedServerWrite, ScopedSettingsPlan, SettingsText},
    },
    types::EnvironmentId,
    workspace::{
        git_panel::scripts::{
            parse_project_file, resolve_project_scripts, script_keybinding_value,
        },
        project_ops::scripts::{
            ProjectScriptInput, build_project_script, next_project_script_id, release_claimed_roles,
        },
        projects::EnvironmentProject,
    },
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};
use serde_json::{Map, Value, json};
use std::rc::Rc;
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    project::ProjectScript,
    rpc::{WsServerRemoveKeybindingRpc, WsServerUpsertKeybindingRpc},
    server::{ServerRemoveKeybindingInput, ServerUpsertKeybindingInput},
    t3_project_file::T3ProjectFileScript,
};

#[derive(Clone)]
pub enum ProjectActionEdit {
    Upsert {
        id: Option<String>,
        input: ProjectScriptInput,
        keybinding: Option<Option<String>>,
    },
    Remove(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ProjectActionImports {
    #[default]
    Missing,
    Invalid,
    Available(Vec<T3ProjectFileScript>),
}

enum ShortcutWrite {
    Upsert(ServerUpsertKeybindingInput),
    Remove(ServerRemoveKeybindingInput),
}

impl<P: Platform + Clone + 'static> SettingsService<P> {
    pub fn watch_project_action_imports(
        &self,
        member: ScopeMember,
    ) -> LocalBoxStream<'static, ProjectActionImports> {
        let file = self.runtime.project_files().read_file(
            &member.environment_id,
            &member.workspace_root,
            "t3.json",
        );
        let changes = file.changes();
        futures::stream::once(async {})
            .chain(changes)
            .map(move |()| {
                let view = file.view();
                let Some(data) = view.data else {
                    return ProjectActionImports::Missing;
                };
                if data.truncated {
                    return ProjectActionImports::Invalid;
                }
                parse_project_file(&data.contents).map_or(ProjectActionImports::Invalid, |file| {
                    ProjectActionImports::Available(file.scripts.unwrap_or_default())
                })
            })
            .boxed_local()
    }

    pub fn edit_project_actions(
        &self,
        search: SettingsScopeSearch,
        edit: ProjectActionEdit,
    ) -> LocalBoxFuture<'static, Result<SettingsUpdateOutcome, ClientError>> {
        let view = self.scope_view(crate::navigation::SettingsPage::Projects, &search);
        let this = self.clone();
        async move {
            if !view.scope.is_project_scope()
                || view.targets.len() != view.scope.members.len()
                || view.targets.is_empty()
                || view
                    .targets
                    .iter()
                    .any(|target| !this.can_write_settings(&target.environment_id))
            {
                return Err(ClientError::PermissionDenied {
                    method: "server.updateSettings".into(),
                });
            }
            let projects = this.runtime.project_actions().projects();
            let id = match &edit {
                ProjectActionEdit::Upsert {
                    id: None, input, ..
                } => next_project_script_id(
                    &input.name.0,
                    projects
                        .projects
                        .iter()
                        .flat_map(|project| &project.project.scripts)
                        .map(|script| script.id.0.as_str())
                        .chain(
                            view.selected
                                .environments
                                .iter()
                                .filter_map(|environment| environment.config.as_ref())
                                .flat_map(|config| {
                                    config.settings.default_project_scripts.iter().chain(
                                        config
                                            .settings
                                            .project_settings_overrides
                                            .values()
                                            .filter_map(|entry| {
                                                entry.default_project_scripts.as_ref()
                                            })
                                            .flatten(),
                                    )
                                })
                                .map(|script| script.id.0.as_str()),
                        ),
                    this.platform.now_epoch_ms(),
                ),
                ProjectActionEdit::Upsert { id: Some(id), .. } | ProjectActionEdit::Remove(id) => {
                    id.clone()
                }
            };
            let writes = action_writes(&view, &projects.projects, &id, &edit)?;
            let shortcuts = shortcut_writes(&view, &projects.projects, &id, &edit)?;
            let outcome = this
                .run(ScopedSettingsPlan {
                    client_patch: Map::new(),
                    server_writes: writes,
                    unavailable_reason: None,
                })
                .await;
            if outcome.failed {
                return Ok(outcome);
            }
            for (environment, shortcut) in shortcuts {
                match this.save_action_shortcut(&environment, shortcut).await {
                    Ok(()) => {}
                    Err(error) => {
                        return Err(ClientError::Rejected {
                            message: "The action was saved, but its shortcut could not be updated."
                                .into(),
                            detail: Some(error.to_string()),
                        });
                    }
                }
            }
            Ok(outcome)
        }
        .boxed_local()
    }

    async fn save_action_shortcut(
        &self,
        environment: &EnvironmentId,
        shortcut: ShortcutWrite,
    ) -> Result<(), ClientError> {
        if !self.can_write_settings(environment) {
            return Err(ClientError::PermissionDenied {
                method: "action shortcut".into(),
            });
        }
        let session = self.session(environment).ok_or(ClientError::Disconnected)?;
        match shortcut {
            ShortcutWrite::Upsert(input) => {
                session.command::<WsServerUpsertKeybindingRpc>(input).await
            }
            ShortcutWrite::Remove(input) => {
                session.command::<WsServerRemoveKeybindingRpc>(input).await
            }
        }
        .map(|_| ())
        .map_err(RpcCallError::into_client_error)
    }
}

fn shortcut_writes(
    view: &SettingsScopeView,
    projects: &[Rc<EnvironmentProject>],
    id: &str,
    edit: &ProjectActionEdit,
) -> Result<Vec<(EnvironmentId, ShortcutWrite)>, ClientError> {
    let selection = match edit {
        ProjectActionEdit::Upsert { keybinding, .. } => keybinding.as_ref(),
        ProjectActionEdit::Remove(_) => Some(&None),
    };
    let Some(next) = selection else {
        return Ok(Vec::new());
    };
    if next
        .as_ref()
        .is_some_and(|key| crate::view_logic::shell::keybindings::parse_shortcut(key).is_none())
    {
        return Err(ClientError::InvalidPayload("Invalid keybinding.".into()));
    }
    let command = format!("script.{id}.run");
    let mut writes = Vec::new();
    for environment in &view.selected.connected {
        let Some(config) = &environment.config else {
            continue;
        };
        let previous = script_keybinding_value(&config.keybindings, id);
        if previous.as_ref() == next.as_ref() {
            continue;
        }
        let shortcut = if let Some(key) = next {
            let mut rule = json!({ "command": command, "key": key });
            if let Some(previous) = previous {
                rule["replace"] = json!({ "command": command, "key": previous });
            }
            ShortcutWrite::Upsert(serde_json::from_value(rule)?)
        } else if let Some(key) = previous {
            if matches!(edit, ProjectActionEdit::Remove(_))
                && action_used_elsewhere(
                    view,
                    projects,
                    &environment.environment_id,
                    &config.settings,
                    id,
                )
            {
                continue;
            }
            ShortcutWrite::Remove(serde_json::from_value(
                json!({ "command": command, "key": key }),
            )?)
        } else {
            continue;
        };
        writes.push((environment.environment_id.clone(), shortcut));
    }
    Ok(writes)
}

fn action_used_elsewhere(
    view: &SettingsScopeView,
    projects: &[Rc<EnvironmentProject>],
    environment: &EnvironmentId,
    settings: &t3_contracts::settings::ServerSettings,
    id: &str,
) -> bool {
    let selected = |project| {
        view.scope
            .members
            .iter()
            .any(|member| &member.environment_id == environment && &member.id == project)
    };
    let has_id = |scripts: &[ProjectScript]| scripts.iter().any(|script| script.id.0 == id);
    has_id(&settings.default_project_scripts)
        || settings
            .project_settings_overrides
            .iter()
            .any(|(project, entry)| {
                !selected(project) && entry.default_project_scripts.as_deref().is_some_and(has_id)
            })
        || projects.iter().any(|project| {
            &project.environment == environment
                && !selected(&project.project.id)
                && has_id(&resolve_project_scripts(settings, &project.project))
        })
}

fn action_writes(
    view: &SettingsScopeView,
    projects: &[Rc<crate::workspace::projects::EnvironmentProject>],
    id: &str,
    edit: &ProjectActionEdit,
) -> Result<Vec<ScopedServerWrite>, ClientError> {
    let mut writes: Vec<ScopedServerWrite> = Vec::new();
    for member in &view.scope.members {
        let config = view
            .selected
            .connected
            .iter()
            .find(|environment| environment.environment_id == member.environment_id)
            .and_then(|environment| environment.config.as_ref())
            .ok_or(ClientError::Disconnected)?;
        if config.environment.capabilities.project_settings_overrides != Some(true) {
            return Err(ClientError::Unsupported("project settings overrides"));
        }
        let project = projects
            .iter()
            .find(|project| {
                project.environment == member.environment_id && project.project.id == member.id
            })
            .ok_or_else(|| {
                ClientError::InvalidPayload(
                    SettingsText::key("This project is no longer available.").english(),
                )
            })?;
        let original = resolve_project_scripts(&config.settings, &project.project);
        let scripts = edit_scripts(original, id, edit)?;
        let mut entry = config
            .settings
            .project_settings_overrides
            .get(&member.id)
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or_else(|| json!({}));
        entry["defaultProjectScripts"] = serde_json::to_value(scripts)?;
        let write = match writes
            .iter()
            .position(|write| write.environment_id == member.environment_id)
        {
            Some(index) => &mut writes[index],
            None => {
                let previous = Rc::new(serde_json::to_value(&config.settings)?);
                writes.push(ScopedServerWrite {
                    environment_id: member.environment_id.clone(),
                    label: view
                        .selected
                        .connected
                        .iter()
                        .find(|environment| environment.environment_id == member.environment_id)
                        .map(|environment| environment.label.clone())
                        .unwrap_or_else(|| member.environment_id.0.clone()),
                    patch: Map::from_iter([("projectSettingsOverrides".into(), json!({}))]),
                    previous,
                });
                writes.last_mut().ok_or(ClientError::Disconnected)?
            }
        };
        if let Some(overrides) = write
            .patch
            .get_mut("projectSettingsOverrides")
            .and_then(Value::as_object_mut)
        {
            overrides.insert(member.id.0.clone(), entry);
        }
    }
    Ok(writes)
}

fn edit_scripts(
    mut scripts: Vec<ProjectScript>,
    id: &str,
    edit: &ProjectActionEdit,
) -> Result<Vec<ProjectScript>, ClientError> {
    match edit {
        ProjectActionEdit::Remove(_) => scripts.retain(|script| script.id.0 != id),
        ProjectActionEdit::Upsert {
            id: previous_id,
            input,
            ..
        } => {
            let replacement = build_project_script(TrimmedNonEmptyString(id.into()), input.clone());
            if previous_id.is_some() && !scripts.iter().any(|script| script.id.0 == id) {
                return Err(ClientError::InvalidPayload(
                    "Action no longer exists.".into(),
                ));
            }
            for script in &mut scripts {
                *script = if script.id.0 == id {
                    replacement.clone()
                } else {
                    release_claimed_roles(script, input)
                };
            }
            if previous_id.is_none() {
                scripts.push(replacement);
            }
        }
    }
    Ok(scripts)
}
