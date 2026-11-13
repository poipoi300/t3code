use crate::{Error, Result};
use serde_json::{Value, json};
use t3_contracts::{scheduled_task::ScheduledTask, stored};
use t3_persistence::{
    Database,
    rusqlite::{Connection, Row, TransactionBehavior, params},
};
#[derive(Clone)]
pub(crate) struct Store(pub Database);
impl Store {
    pub async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.0.reader(move |c| Ok(f(c))).await?
    }
    pub async fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.0
            .writer(move |c| {
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let result = f(&tx);
                if result.is_ok() {
                    tx.commit()?;
                }
                Ok(result)
            })
            .await?
    }
}
pub(crate) fn decode_task(row: &Row<'_>) -> Result<ScheduledTask> {
    let text = |key| row.get::<_, String>(key);
    let nullable = |key| row.get::<_, Option<String>>(key);
    let json_column = |key| -> Result<Value> { Ok(serde_json::from_str(&text(key)?)?) };
    let id = text("task_id")?;
    let mut value = json!({
            "id": id, "title": text("title")?, "prompt": text("prompt")?,
            "enabled": row.get::<_, i64>("enabled")? == 1,
            "schedule": json_column("schedule_json")?, "projectId": text("project_id")?,
            "threadId": nullable("thread_id")?, "threadMode": text("thread_mode")?,
            "workspaceStrategy": json_column("workspace_strategy_json")?,
            "modelSelection": json_column("model_selection_json")?,
            "runtimeMode": text("runtime_mode")?, "interactionMode": text("interaction_mode")?,
            "createdBy": text("created_by")?, "creationSource": text("creation_source")?,
            "createdAt": text("created_at")?, "updatedAt": text("updated_at")?,
            "nextRunAt": nullable("next_run_at")?, "lastRunAt": nullable("last_run_at")?,
            "lastRunStatus": text("last_run_status")?, "lastRunError": nullable("last_run_error")?,
            "runCount": row.get::<_, i64>("run_count")?
        }
    );
    if value["schedule"]["type"] == "webhook"
        && let Some(token) = nullable("webhook_token")?
    {
        let encoded = encode_component(&id);
        value["webhook"] = json!({
        "path": format!("/api/hooks/{encoded}/{token}"), "url": null, "hasSecret": nullable("webhook_secret")?.is_some()}
        );
    }
    let task = serde_json::from_value(value)?;
    crate::validation::validate_persisted(&task)?;
    Ok(task)
}
pub(crate) fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    encoded
}
pub(crate) fn load(c: &Connection, id: &str) -> Result<ScheduledTask> {
    let mut stmt = c.prepare("SELECT * FROM scheduled_tasks WHERE task_id=?1")?;
    let mut rows = stmt.query([id])?;
    rows.next()?
        .map(decode_task)
        .unwrap_or_else(|| Err(Error::NotFound(id.to_owned())))
}
pub(crate) fn find(c: &Connection, id: &str) -> Result<Option<ScheduledTask>> {
    match load(c, id) {
        Ok(t) => Ok(Some(t)),
        Err(Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}
pub(crate) fn all(c: &Connection) -> Result<Vec<ScheduledTask>> {
    let mut stmt =
        c.prepare("SELECT * FROM scheduled_tasks ORDER BY created_at DESC, task_id ASC")?;
    let mut rows = stmt.query([])?;
    let mut tasks = Vec::new();
    while let Some(row) = rows.next()? {
        tasks.push(decode_task(row)?);
    }
    Ok(tasks)
}
pub(crate) fn due(c: &Connection, now: &str) -> Result<Vec<ScheduledTask>> {
    let mut stmt = c.prepare("SELECT * FROM scheduled_tasks WHERE enabled=1 AND next_run_at IS NOT NULL AND next_run_at<=?1 AND last_run_status<>'running' ORDER BY next_run_at, task_id")?;
    let mut rows = stmt.query([now])?;
    let mut tasks = Vec::new();
    while let Some(row) = rows.next()? {
        if let Ok(task) = decode_task(row)
            && task
                .next_run_at
                .as_ref()
                .is_some_and(|v| crate::schedule::parse_date(&v.0).is_some())
        {
            tasks.push(task);
        }
    }
    Ok(tasks)
}
pub(crate) fn save(
    c: &Connection,
    task: &ScheduledTask,
    token: Option<&str>,
    secret: Option<&str>,
) -> Result<()> {
    let v = stored::to_value(task)?;
    let enum_string = |key: &str| -> Result<&str> {
        v[key]
            .as_str()
            .ok_or_else(|| Error::Invalid(format!("Invalid {key}")))
    };
    c.execute(
        "INSERT INTO scheduled_tasks (task_id,title,prompt,enabled,schedule_json,project_id,thread_id,thread_mode,workspace_strategy_json,model_selection_json,runtime_mode,interaction_mode,created_by,creation_source,created_at,updated_at,next_run_at,last_run_at,last_run_status,last_run_error,run_count,webhook_token,webhook_secret)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)
        ON CONFLICT(task_id) DO UPDATE SET title=excluded.title,prompt=excluded.prompt,enabled=excluded.enabled,schedule_json=excluded.schedule_json,project_id=excluded.project_id,thread_id=CASE WHEN excluded.thread_mode='new_thread' THEN NULL WHEN excluded.project_id!=scheduled_tasks.project_id THEN excluded.thread_id ELSE COALESCE(excluded.thread_id,scheduled_tasks.thread_id) END,thread_mode=excluded.thread_mode,workspace_strategy_json=excluded.workspace_strategy_json,model_selection_json=excluded.model_selection_json,runtime_mode=excluded.runtime_mode,interaction_mode=excluded.interaction_mode,creation_source=excluded.creation_source,updated_at=excluded.updated_at,next_run_at=excluded.next_run_at,webhook_token=excluded.webhook_token,webhook_secret=excluded.webhook_secret",
        params![
            task.id.0,
            task.title.0,
            task.prompt.0,
            task.enabled,
            stored::to_string(&task.schedule)?,
            task.project_id.0,
            task.thread_id.as_ref().map(|v| &v.0),
            enum_string("threadMode")?,
            stored::to_string(&task.workspace_strategy)?,
            stored::to_string(&task.model_selection)?,
            enum_string("runtimeMode")?,
            enum_string("interactionMode")?,
            enum_string("createdBy")?,
            enum_string("creationSource")?,
            task.created_at.0,
            task.updated_at.0,
            task.next_run_at.as_ref().map(|v| &v.0),
            task.last_run_at.as_ref().map(|v| &v.0),
            enum_string("lastRunStatus")?,
            task.last_run_error,
            task.run_count,
            token,
            secret
        ],
    )?;
    Ok(())
}
