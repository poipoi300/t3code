//! The part of `persistence/ProviderSessionRuntime.ts` an agent-session import needs, over the
//! existing `provider_session_runtime` table (no schema change), plus the inventory's host:
//! settings and the imported-transcript records it reads.
use crate::{
    Error, Result,
    inventory::InventoryHost,
    sessions::SessionImportStore,
    transcript::SessionThread,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use t3_contracts::{
    agent_sessions::{AgentSessionImportSource, AgentSessionSource},
    base_schemas::{ProjectId, ThreadId},
    provider_policy::RuntimeMode,
};
use t3_orchestration::stores::StoreDatabase;
use t3_persistence::rusqlite::{OptionalExtension, params};

pub struct RuntimeImportStore {
    database: StoreDatabase,
    settings: Arc<t3_config::settings::SettingsService>,
    /// The organisation's runtime-mode ceiling; `None` imports in full access.
    ceiling: Option<RuntimeMode>,
}

/// `decodeImportedTranscriptPayload`: `{cwd?, importedTranscripts?}`, or nothing when the payload
/// does not decode as a whole.
fn imported_payload(payload: Option<&str>) -> Option<(Option<String>, Vec<AgentSessionImportSource>)> {
    let value: Value = serde_json::from_str(payload?).ok()?;
    let object = value.as_object()?;
    let cwd = match object.get("cwd") {
        None => None,
        Some(Value::String(cwd)) => Some(cwd.clone()),
        Some(_) => return None,
    };
    let sources = match object.get("importedTranscripts") {
        None => vec![],
        Some(sources) => serde_json::from_value(sources.clone()).ok()?,
    };
    Some((cwd, sources))
}

/// `normalizeProjectPathForComparison`.
fn comparison_key(value: &str) -> String {
    let value = value.replace('\\', "/");
    let value = if value.len() > 1 {
        value.trim_end_matches('/').to_owned()
    } else {
        value
    };
    if cfg!(windows) { value.to_lowercase() } else { value }
}

impl RuntimeImportStore {
    pub fn new(
        database: StoreDatabase,
        settings: Arc<t3_config::settings::SettingsService>,
        ceiling: Option<RuntimeMode>,
    ) -> Self {
        Self {
            database,
            settings,
            ceiling,
        }
    }

    /// `ProviderSessionRuntimeRepository.list`: every row's runtime payload.
    async fn payloads(&self) -> Result<Vec<Option<String>>> {
        Ok(self
            .database
            .read(|db| {
                let mut statement = db.prepare(
                    "SELECT runtime_payload_json FROM provider_session_runtime
                     ORDER BY last_seen_at ASC, thread_id ASC",
                )?;
                let rows = statement
                    .query_map([], |row| row.get::<_, Option<String>>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await
            .map_err(|_| Error::Operation {
                operation: "read-projects",
                detail: "unavailable".into(),
            })?)
    }
}

#[async_trait]
impl SessionImportStore for RuntimeImportStore {
    async fn completed_sources(&self, workspace_root: &str) -> Result<Vec<AgentSessionImportSource>> {
        let root = comparison_key(workspace_root);
        Ok(self
            .payloads()
            .await?
            .iter()
            .filter_map(|payload| imported_payload(payload.as_deref()))
            .filter(|(cwd, _)| cwd.as_deref().is_some_and(|cwd| comparison_key(cwd) == root))
            .flat_map(|(_, sources)| sources)
            .collect())
    }

    async fn recorded_sources(&self, thread: &ThreadId) -> Result<Vec<AgentSessionImportSource>> {
        let id = thread.0.clone();
        let payload: Option<Option<String>> = self
            .database
            .read(move |db| {
                Ok(db
                    .query_row(
                        "SELECT runtime_payload_json FROM provider_session_runtime WHERE thread_id = ?1",
                        [id],
                        |row| row.get(0),
                    )
                    .optional()?)
            })
            .await?;
        Ok(imported_payload(payload.flatten().as_deref())
            .map(|(_, sources)| sources)
            .unwrap_or_default())
    }

    /// `upsert(.., { onConflict: "ignore" })` with TypeScript's import row.
    async fn persist_runtime(
        &self,
        thread: &ThreadId,
        source: &AgentSessionImportSource,
        session: &SessionThread,
        workspace_root: &str,
        mode: RuntimeMode,
    ) -> Result<()> {
        let driver = match session.source {
            AgentSessionSource::ClaudeAgent => "claudeAgent",
            AgentSessionSource::Codex => "codex",
            AgentSessionSource::Pi => "pi",
        };
        // Pi resumes by session file path; its session UUID only names the thread.
        let native = if session.source == AgentSessionSource::Pi {
            source.file_path.0.clone()
        } else {
            session.provider_session_id.clone()
        };
        let cursor = if session.source == AgentSessionSource::ClaudeAgent {
            json!({"threadId": thread, "resume": session.provider_session_id})
        } else {
            json!({"threadId": native})
        };
        let row = (
            thread.0.clone(),
            driver,
            session.provider_instance_id.0.clone(),
            serde_json::to_value(mode)?
                .as_str()
                .unwrap_or("full-access")
                .to_owned(),
            session.updated_at.clone(),
            cursor.to_string(),
            json!({"cwd": workspace_root}).to_string(),
        );
        self.database
            .transaction(move |db| {
                db.execute(
                    "INSERT INTO provider_session_runtime (
                       thread_id, provider_name, provider_instance_id, adapter_key, runtime_mode,
                       status, last_seen_at, resume_cursor_json, runtime_payload_json
                     ) VALUES (
                       ?1, ?2, ?3, ?2, ?4, 'stopped', ?5, ?6,
                       CASE WHEN json_type(?7) = 'object'
                         THEN json_remove(?7, '$.importedTranscripts') ELSE ?7 END
                     )
                     ON CONFLICT (thread_id) DO NOTHING",
                    params![row.0, row.1, row.2, row.3, row.4, row.5, row.6],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// `recordImportedTranscript`: replaces the entry for the same instance and file, appends.
    async fn record_imported_transcript(
        &self,
        thread: &ThreadId,
        source: &AgentSessionImportSource,
    ) -> Result<()> {
        let id = thread.0.clone();
        let source = serde_json::to_string(source)?;
        self.database
            .transaction(move |db| {
                db.execute(
                    "WITH current_runtime AS (
                       SELECT CASE
                         WHEN json_valid(runtime_payload_json) THEN CASE
                           WHEN json_type(runtime_payload_json) = 'object' THEN runtime_payload_json
                           ELSE '{}'
                         END
                         ELSE '{}'
                       END AS payload
                       FROM provider_session_runtime
                       WHERE thread_id = ?1
                     )
                     UPDATE provider_session_runtime
                     SET runtime_payload_json = (
                       SELECT json_set(
                         payload,
                         '$.importedTranscripts',
                         json((
                           SELECT json_group_array(json(value))
                           FROM (
                             SELECT value
                             FROM json_each(CASE
                               WHEN json_type(payload, '$.importedTranscripts') = 'array'
                               THEN json_extract(payload, '$.importedTranscripts')
                               ELSE '[]'
                             END)
                             WHERE CASE
                               WHEN type = 'object' THEN
                                 json_extract(value, '$.providerInstanceId')
                                   IS NOT json_extract(?2, '$.providerInstanceId')
                                 OR json_extract(value, '$.filePath') IS NOT json_extract(?2, '$.filePath')
                               ELSE 0
                             END
                             UNION ALL
                             SELECT ?2 AS value
                           )
                         ))
                       )
                       FROM current_runtime
                     )
                     WHERE thread_id = ?1",
                    params![id, source],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Imports run in full access, except under a policy ceiling: then in the project's default,
    /// which the policy keeps under it.
    async fn policy_runtime_mode(&self, project: &ProjectId) -> Result<Option<RuntimeMode>> {
        let Some(ceiling) = self.ceiling else {
            return Ok(None);
        };
        let settings = self.settings.snapshot().await.map_err(|_| Error::Operation {
            operation: "read-settings",
            detail: "unavailable".into(),
        })?;
        let resolved =
            t3_shared::project_settings::resolve_project_settings(&settings, Some(project), None)
                .map_err(|_| Error::Operation {
                    operation: "read-settings",
                    detail: "unavailable".into(),
                })?;
        let mode = resolved
            .settings
            .default_runtime_mode
            .unwrap_or(RuntimeMode::FullAccess);
        Ok(Some(
            if t3_orchestration::ports::decisions::dispatch_mode_limit::is_runtime_mode_above(
                mode, ceiling,
            ) {
                ceiling
            } else {
                mode
            },
        ))
    }
}

#[async_trait]
impl InventoryHost for RuntimeImportStore {
    async fn settings(&self) -> Result<Value> {
        let settings = self.settings.snapshot().await.map_err(|_| Error::Operation {
            operation: "read-settings",
            detail: "unavailable".into(),
        })?;
        Ok(serde_json::to_value(settings)?)
    }
    async fn imported_transcripts(&self) -> Result<Vec<(String, AgentSessionImportSource)>> {
        Ok(self
            .payloads()
            .await?
            .iter()
            .filter_map(|payload| imported_payload(payload.as_deref()))
            .filter_map(|(cwd, sources)| cwd.map(|cwd| (cwd, sources)))
            .flat_map(|(cwd, sources)| sources.into_iter().map(move |s| (cwd.clone(), s)))
            .collect())
    }
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}
