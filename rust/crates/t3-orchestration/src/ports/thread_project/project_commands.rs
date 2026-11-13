use crate::{error::Result, stores::projects::ProjectRow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use t3_contracts::{
    application_event::ApplicationProjectEvent,
    base_schemas::*,
    environment::ThreadEnvMode,
    model_selection::ModelSelection,
    project::{ProjectIconOverride, ProjectScript},
};

// Project commands are server-only; the canonical server command union contains thread commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum ProjectCommand {
    #[serde(rename = "project.create")]
    Create {
        command_id: CommandId,
        project_id: ProjectId,
        title: String,
        workspace_root: String,
        scripts: Option<Vec<ProjectScript>>,
    },
    #[serde(rename = "project.meta.update")]
    Update {
        command_id: CommandId,
        project_id: ProjectId,
        #[serde(flatten)]
        change: Box<ProjectChange>,
    },
    #[serde(rename = "project.delete")]
    Delete {
        command_id: CommandId,
        project_id: ProjectId,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub default_model_selection: Option<Option<ModelSelection>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub default_thread_env_mode: Option<Option<ThreadEnvMode>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pull: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub favicon_path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "t3_contracts::wire::double_option"
    )]
    pub project_icon: Option<Option<ProjectIconOverride>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scripts: Option<Vec<ProjectScript>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "_tag", rename_all_fields = "camelCase")]
pub enum ProjectCommandRejection {
    #[serde(rename = "ProjectCommandInvariantError")]
    #[error("Project command invariant failed ({command_type}): {detail}")]
    Invariant {
        command_type: String,
        detail: String,
    },
    #[serde(rename = "ProjectCommandMissingProjectError")]
    #[error("Project '{project_id}' does not exist for command '{command_type}'.", project_id = .project_id.0)]
    MissingProject {
        command_type: String,
        project_id: ProjectId,
    },
    #[serde(rename = "ProjectWorkspaceConflictError")]
    #[error("Active project '{id}' already exists for workspace root '{workspace_root}'.", id = .conflicting_project_id.0)]
    WorkspaceConflict {
        workspace_root: String,
        conflicting_project_id: ProjectId,
    },
}

pub fn encode_project_command_rejection(rejection: &ProjectCommandRejection) -> Result<String> {
    Ok(serde_json::to_string(rejection)?)
}

pub fn decode_project_command_rejection(encoded: &str) -> Option<ProjectCommandRejection> {
    serde_json::from_str(encoded).ok()
}

#[derive(Default)]
pub struct ProjectCommandState<'a> {
    pub project: Option<&'a ProjectRow>,
    pub workspace_owner: Option<&'a ProjectRow>,
}

/// `grapheme_count` must implement Unicode extended grapheme clusters, as Intl.Segmenter does.
pub fn plan_project_command(
    command: &ProjectCommand,
    state: ProjectCommandState<'_>,
    event_id: EventId,
    now: &IsoDateTime,
    grapheme_count: impl Fn(&str) -> usize,
) -> Result<std::result::Result<ApplicationProjectEvent, ProjectCommandRejection>> {
    let value = serde_json::to_value(command)?;
    let command_type = value["type"].as_str().unwrap_or_default();
    let command_id = &value["commandId"];
    let project_id: ProjectId = serde_json::from_value(value["projectId"].clone())?;
    let invariant = |detail: String| ProjectCommandRejection::Invariant {
        command_type: command_type.into(),
        detail,
    };
    let active = state.project.filter(|project| project.deleted_at.is_none());
    if command_type == "project.create" && state.project.is_some() {
        return Ok(Err(invariant(format!(
            "Project '{}' already exists and cannot be created twice.",
            project_id.0
        ))));
    }
    if command_type != "project.create" && active.is_none() {
        return Ok(Err(ProjectCommandRejection::MissingProject {
            command_type: command_type.into(),
            project_id,
        }));
    }
    if let Some(root) = value["workspaceRoot"].as_str()
        && let Some(owner) = state
            .workspace_owner
            .filter(|owner| owner.project_id != project_id)
    {
        return Ok(Err(ProjectCommandRejection::WorkspaceConflict {
            workspace_root: root.into(),
            conflicting_project_id: owner.project_id.clone(),
        }));
    }
    let (event_type, payload) = match command {
        ProjectCommand::Create {
            title,
            workspace_root,
            scripts,
            ..
        } => (
            "project.created",
            json!({
                "projectId":project_id,"title":title,"workspaceRoot":workspace_root,"defaultModelSelection":null,
                "faviconPath":null,"projectIcon":null,"scripts":scripts.as_deref().unwrap_or_default(),"createdAt":now,"updatedAt":now,
            }),
        ),
        ProjectCommand::Delete { .. } => (
            "project.deleted",
            json!({"projectId":project_id,"deletedAt":now}),
        ),
        ProjectCommand::Update { change, .. } => {
            let Some(project) = active else {
                return Ok(Err(invariant("Missing active project.".into())));
            };
            if let Some(Some(ProjectIconOverride::Monogram(icon))) = &change.project_icon
                && grapheme_count(&icon.text.0) > 2
            {
                return Ok(Err(invariant(
                    "Project monograms must contain at most two characters.".into(),
                )));
            }
            if let Some(scripts) = &change.scripts {
                for script in scripts {
                    let id = &script.id.0;
                    let valid = !id.is_empty()
                        && id.len() <= 24
                        && id
                            .bytes()
                            .next()
                            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                        && id
                            .bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
                    if !valid && !project.scripts.iter().any(|existing| existing.id.0 == *id) {
                        return Ok(Err(invariant(format!(
                            "Script IDs must be 1-24 lowercase letters, digits or hyphens, starting with a letter or digit (got {} characters).",
                            id.encode_utf16().count()
                        ))));
                    }
                }
            }
            let mut payload = serde_json::to_value(change)?;
            let object = payload
                .as_object_mut()
                .ok_or_else(|| crate::error::Error::Decode {
                    entity: "project change",
                    detail: "expected object".into(),
                })?;
            object.shift_remove("pinned");
            object.shift_remove("archived");
            object.insert("projectId".into(), json!(project_id));
            if let Some(pinned) = change.pinned {
                object.insert(
                    "pinnedAt".into(),
                    if pinned {
                        json!(project.pinned_at.as_ref().unwrap_or(now))
                    } else {
                        Value::Null
                    },
                );
            }
            if let Some(archived) = change.archived {
                object.insert(
                    "archivedAt".into(),
                    if archived {
                        json!(project.archived_at.as_ref().unwrap_or(now))
                    } else {
                        Value::Null
                    },
                );
            }
            object.insert("updatedAt".into(), json!(now));
            ("project.meta-updated", payload)
        }
    };
    Ok(Ok(serde_json::from_value(
        json!({"sequence":0,"eventId":event_id,"aggregateKind":"project","aggregateId":project_id,
        "occurredAt":now,"commandId":command_id,"causationEventId":null,"correlationId":command_id,"metadata":{},"type":event_type,"payload":payload}),
    )?))
}
