use super::event_store::{decode_json, encode_json, parse_json, statement};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use t3_contracts::{
    application_event::ApplicationProjectEvent,
    base_schemas::{IsoDateTime, ProjectId, TrimmedNonEmptyString},
    environment::ThreadEnvMode,
    model_selection::ModelSelection,
    orchestration_project::OrchestrationProjectShell,
    project::{ProjectScript, ReceivedProjectIcon, StoredProjectIcon},
    wire::{WireDateTime, trim_js},
};
use t3_persistence::rusqlite::{Connection, Row, params};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRow {
    pub project_id: ProjectId,
    pub title: String,
    pub has_custom_title: bool,
    pub workspace_root: String,
    pub default_model_selection: Option<ModelSelection>,
    pub default_thread_env_mode: Option<ThreadEnvMode>,
    pub auto_pull: bool,
    pub favicon_path: Option<String>,
    pub project_icon: Option<StoredProjectIcon>,
    pub scripts: Vec<ProjectScript>,
    pub pinned_at: Option<IsoDateTime>,
    pub archived_at: Option<IsoDateTime>,
    pub created_at: IsoDateTime,
    pub updated_at: IsoDateTime,
    pub deleted_at: Option<IsoDateTime>,
}

const SELECT: &str = "SELECT project_id, title, has_custom_title, workspace_root, \
    default_model_selection_json, default_thread_env_mode, auto_pull, favicon_path, \
    project_icon_json, scripts_json, pinned_at, archived_at, created_at, updated_at, deleted_at \
    FROM projection_projects";
const ORDER: &str = "ORDER BY created_at ASC, project_id ASC";
const UPSERT: &str = "INSERT INTO projection_projects (
        project_id, title, has_custom_title, workspace_root, default_model_selection_json,
        default_thread_env_mode, auto_pull, favicon_path, project_icon_json, scripts_json,
        pinned_at, archived_at, created_at, updated_at, deleted_at
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
    ON CONFLICT (project_id) DO UPDATE SET
        title = CASE
            WHEN projection_projects.has_custom_title = 1 AND excluded.has_custom_title = 0
                THEN projection_projects.title
            ELSE excluded.title
        END,
        has_custom_title = MAX(projection_projects.has_custom_title, excluded.has_custom_title),
        workspace_root = excluded.workspace_root,
        default_model_selection_json = excluded.default_model_selection_json,
        default_thread_env_mode = excluded.default_thread_env_mode,
        auto_pull = excluded.auto_pull,
        favicon_path = excluded.favicon_path,
        project_icon_json = excluded.project_icon_json,
        scripts_json = excluded.scripts_json,
        pinned_at = excluded.pinned_at,
        archived_at = excluded.archived_at,
        created_at = excluded.created_at,
        updated_at = excluded.updated_at,
        deleted_at = excluded.deleted_at";

fn decode(row: &Row<'_>) -> t3_persistence::rusqlite::Result<Result<ProjectRow>> {
    let model: Option<String> = row.get(4)?;
    let mode: Option<String> = row.get(5)?;
    let icon: Option<String> = row.get(8)?;
    let scripts: String = row.get(9)?;
    let date = |index| row.get::<_, String>(index).map(WireDateTime);
    let nullable_date = |index| {
        row.get::<_, Option<String>>(index)
            .map(|d| d.map(WireDateTime))
    };
    let (project_id, title, has_custom_title, workspace_root) =
        (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
    let (auto_pull, favicon_path) = (row.get(6)?, row.get(7)?);
    let (pinned_at, archived_at) = (nullable_date(10)?, nullable_date(11)?);
    let (created_at, updated_at, deleted_at) = (date(12)?, date(13)?, nullable_date(14)?);
    Ok((|| {
        Ok(ProjectRow {
            project_id: ProjectId(project_id),
            title,
            has_custom_title,
            workspace_root,
            default_model_selection: model
                .map(|json| decode_json(parse_json(&json)?))
                .transpose()?,
            default_thread_env_mode: mode.map(|mode| decode_json(mode.into())).transpose()?,
            auto_pull,
            favicon_path,
            project_icon: icon
                .map(|json| decode_json(parse_json(&json)?))
                .transpose()?,
            scripts: decode_json(parse_json(&scripts)?)?,
            pinned_at,
            archived_at,
            created_at,
            updated_at,
            deleted_at,
        })
    })())
}

fn collect(
    rows: impl Iterator<Item = t3_persistence::rusqlite::Result<Result<ProjectRow>>>,
) -> Result<Vec<ProjectRow>> {
    rows.map(|row| row.map_err(Into::into).and_then(|row| row))
        .collect()
}

pub fn list(
    db: &Connection,
    project_ids: Option<&[ProjectId]>,
    include_deleted: bool,
) -> Result<Vec<ProjectRow>> {
    let deleted = if include_deleted {
        "1 = 1"
    } else {
        "deleted_at IS NULL"
    };
    match project_ids {
        Some(ids) => {
            let ids = serde_json::to_string(ids)?;
            let sql = format!(
                "{SELECT} WHERE {deleted} \
                 AND project_id IN (SELECT value FROM json_each(?1)) {ORDER}"
            );
            collect(statement(db, &sql)?.query_map([ids], decode)?)
        }
        None => {
            let sql = format!("{SELECT} WHERE {deleted} {ORDER}");
            collect(statement(db, &sql)?.query_map([], decode)?)
        }
    }
}

pub fn get(
    db: &Connection,
    project_id: &ProjectId,
    include_deleted: bool,
) -> Result<Option<ProjectRow>> {
    let deleted = if include_deleted {
        "1 = 1"
    } else {
        "deleted_at IS NULL"
    };
    let sql = format!("{SELECT} WHERE {deleted} AND project_id = ?1 {ORDER}");
    Ok(
        collect(statement(db, &sql)?.query_map([&project_id.0], decode)?)?
            .into_iter()
            .next(),
    )
}

pub fn find_active_by_workspace_root(db: &Connection, root: &str) -> Result<Option<ProjectRow>> {
    let sql = format!("{SELECT} WHERE deleted_at IS NULL AND workspace_root = ?1 {ORDER}");
    Ok(collect(statement(db, &sql)?.query_map([root], decode)?)?
        .into_iter()
        .next())
}

fn json_column<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(|value| encode_json(&t3_contracts::stored::to_value(value)?))
        .transpose()
}

fn enum_string<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(|value| match serde_json::to_value(value)? {
            Value::String(text) => Ok(text),
            _ => Err(Error::Decode {
                entity: "project enum",
                detail: "expected string".into(),
            }),
        })
        .transpose()
}

pub fn upsert(db: &Connection, r: &ProjectRow) -> Result<()> {
    statement(db, UPSERT)?.execute(params![
        r.project_id.0,
        r.title,
        r.has_custom_title,
        r.workspace_root,
        json_column(&r.default_model_selection)?,
        enum_string(&r.default_thread_env_mode)?,
        r.auto_pull,
        r.favicon_path,
        json_column(&r.project_icon)?,
        encode_json(&t3_contracts::stored::to_value(&r.scripts)?)?,
        r.pinned_at.as_ref().map(|d| &d.0),
        r.archived_at.as_ref().map(|d| &d.0),
        r.created_at.0,
        r.updated_at.0,
        r.deleted_at.as_ref().map(|d| &d.0),
    ])?;
    Ok(())
}

fn text(value: &TrimmedNonEmptyString) -> String {
    trim_js(&value.0).to_owned()
}

/// `null` for a column the TypeScript row schema declares non-nullable fails its row encode.
fn required<T>(field: &'static str, value: Option<T>) -> Result<T> {
    value.ok_or_else(|| Error::Decode {
        entity: "project event",
        detail: format!("{field} cannot be null"),
    })
}

/// `ProjectStoreV2.apply`: absent keys keep the row's value, explicit null clears it.
pub fn apply(db: &Connection, event: &ApplicationProjectEvent) -> Result<()> {
    let project_id = match event {
        ApplicationProjectEvent::Created(event) => {
            let payload = &event.payload;
            return upsert(
                db,
                &ProjectRow {
                    project_id: payload.project_id.clone(),
                    title: text(&payload.title),
                    has_custom_title: false,
                    workspace_root: text(&payload.workspace_root),
                    default_model_selection: payload.default_model_selection.clone(),
                    default_thread_env_mode: payload.default_thread_env_mode.clone().flatten(),
                    auto_pull: false,
                    favicon_path: payload
                        .favicon_path
                        .as_ref()
                        .and_then(|p| p.as_ref().map(text)),
                    project_icon: payload.project_icon.clone().flatten(),
                    scripts: payload.scripts.clone(),
                    pinned_at: None,
                    archived_at: None,
                    created_at: payload.created_at.clone(),
                    updated_at: payload.updated_at.clone(),
                    deleted_at: None,
                },
            );
        }
        ApplicationProjectEvent::MetaUpdated(event) => &event.payload.project_id,
        ApplicationProjectEvent::Deleted(event) => &event.payload.project_id,
    };
    let Some(mut row) = get(db, project_id, true)? else {
        return Ok(());
    };
    match event {
        ApplicationProjectEvent::Created(_) => {}
        ApplicationProjectEvent::Deleted(event) => {
            row.deleted_at = Some(event.payload.deleted_at.clone());
            row.updated_at = event.payload.deleted_at.clone();
        }
        ApplicationProjectEvent::MetaUpdated(event) => {
            let payload = &event.payload;
            if let Some(title) = &payload.title {
                row.title = text(required("title", title.as_ref())?);
                row.has_custom_title = true;
            }
            if let Some(root) = &payload.workspace_root {
                row.workspace_root = text(required("workspaceRoot", root.as_ref())?);
            }
            if let Some(selection) = &payload.default_model_selection {
                row.default_model_selection.clone_from(selection);
            }
            if let Some(mode) = &payload.default_thread_env_mode {
                row.default_thread_env_mode.clone_from(mode);
            }
            if let Some(auto_pull) = payload.auto_pull {
                row.auto_pull = required("autoPull", auto_pull)?;
            }
            if let Some(path) = &payload.favicon_path {
                row.favicon_path = path.as_ref().map(text);
            }
            if let Some(icon) = &payload.project_icon {
                row.project_icon.clone_from(icon);
            }
            if let Some(scripts) = &payload.scripts {
                row.scripts = required("scripts", scripts.clone())?;
            }
            if let Some(pinned_at) = &payload.pinned_at {
                row.pinned_at.clone_from(pinned_at);
            }
            if let Some(archived_at) = &payload.archived_at {
                row.archived_at.clone_from(archived_at);
            }
            row.updated_at = payload.updated_at.clone();
        }
    }
    upsert(db, &row)
}

pub fn to_shell(row: &ProjectRow) -> Result<OrchestrationProjectShell> {
    Ok(OrchestrationProjectShell {
        id: row.project_id.clone(),
        title: TrimmedNonEmptyString(row.title.clone()),
        has_custom_title: Some(Some(row.has_custom_title)),
        workspace_root: TrimmedNonEmptyString(row.workspace_root.clone()),
        repository_identity: Some(None),
        default_model_selection: row.default_model_selection.clone(),
        default_thread_env_mode: Some(row.default_thread_env_mode.clone()),
        auto_pull: Some(Some(row.auto_pull)),
        favicon_path: Some(row.favicon_path.clone().map(TrimmedNonEmptyString)),
        project_icon: Some(
            row.project_icon
                .clone()
                .map(|icon| ReceivedProjectIcon(Some(icon.0))),
        ),
        scripts: row.scripts.clone(),
        pinned_at: Some(row.pinned_at.clone()),
        archived_at: Some(row.archived_at.clone()),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    })
}

pub fn get_shell(
    db: &Connection,
    project_id: &ProjectId,
) -> Result<Option<OrchestrationProjectShell>> {
    get(db, project_id, false)?
        .as_ref()
        .map(to_shell)
        .transpose()
}

pub fn list_shells(
    db: &Connection,
    project_ids: Option<&[ProjectId]>,
) -> Result<Vec<OrchestrationProjectShell>> {
    list(db, project_ids, false)?.iter().map(to_shell).collect()
}
