use super::{
    shell_stream::can_resume_shell_from, thread_history_paging::decode_thread_history_cursor,
    thread_stream::SnapshotOptions,
};
use crate::{
    api::{HistoryQuery, HistoryWorkMode},
    error::Error,
    transport::service::TransportService,
};
use t3_contracts::{
    auth::AuthEnvironmentScope,
    base_schemas::ThreadId,
    environment_http::{
        EnvironmentInternalErrorReason, EnvironmentRequestInvalidReason,
        EnvironmentResourceNotFoundReason,
    },
    orchestration_v2::*,
};

#[derive(Debug, thiserror::Error)]
pub enum HttpReadError {
    #[error("orchestration read scope not granted")]
    ScopeNotGranted,
    #[error("invalid request: {0:?}")]
    Invalid(EnvironmentRequestInvalidReason),
    #[error("resource not found: {0:?}")]
    NotFound(EnvironmentResourceNotFoundReason),
    #[error("orchestration read failed: {reason:?}")]
    Internal {
        reason: EnvironmentInternalErrorReason,
        #[source]
        source: Error,
    },
}
impl HttpReadError {
    pub fn status(&self) -> u16 {
        match self {
            Self::ScopeNotGranted => 403,
            Self::Invalid(_) => 400,
            Self::NotFound(_) => 404,
            Self::Internal { .. } => 500,
        }
    }
}
pub type HttpReadResult<T> = std::result::Result<T, HttpReadError>;
fn authorize(scopes: &[AuthEnvironmentScope]) -> HttpReadResult<()> {
    if scopes.contains(&AuthEnvironmentScope::OrchestrationRead) {
        Ok(())
    } else {
        Err(HttpReadError::ScopeNotGranted)
    }
}
fn map_read_error(source: Error, reason: EnvironmentInternalErrorReason) -> HttpReadError {
    if matches!(source, Error::ThreadNotFound(_)) {
        HttpReadError::NotFound(EnvironmentResourceNotFoundReason::ThreadNotFound)
    } else {
        HttpReadError::Internal { reason, source }
    }
}

#[derive(Debug, Default)]
pub struct ShellSnapshotQuery<'a> {
    pub omit_idle_subagents: Option<&'a str>,
    pub omit_older_settled: Option<&'a str>,
    pub resume_from: Option<&'a str>,
}
#[derive(Debug, Default)]
pub struct BoundedSnapshotQuery<'a> {
    pub defer_work: Option<&'a str>,
    pub compact_turn_items: Option<&'a str>,
    pub dedupe_bodies: Option<&'a str>,
    pub compact_ids: Option<&'a str>,
}
#[derive(Debug)]
pub struct HistoryPageQuery<'a> {
    pub cursor: &'a str,
    pub through_entry_id: Option<&'a str>,
    pub view: Option<&'a str>,
    pub defer_work: Option<&'a str>,
    pub compact_ids: Option<&'a str>,
}
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ShellSnapshotAnswer {
    Resumable(OrchestrationV2ShellResumable),
    Snapshot(Box<OrchestrationV2ShellHttpSnapshot>),
}

pub fn parse_resume_from(value: Option<&str>) -> Option<u64> {
    let value = t3_contracts::wire::trim_js(value?);
    let number = if value.is_empty() {
        0.0
    } else if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()? as f64
    } else if let Some(octal) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        u64::from_str_radix(octal, 8).ok()? as f64
    } else if let Some(binary) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        u64::from_str_radix(binary, 2).ok()? as f64
    } else {
        value.parse::<f64>().ok()?
    };
    (number.is_finite()
        && number.fract() == 0.0
        && (0.0..=9_007_199_254_740_991.0).contains(&number))
    .then_some(number as u64)
}
pub struct OrchestrationHttpRead<'a> {
    pub service: &'a TransportService,
}
impl OrchestrationHttpRead<'_> {
    pub async fn thread_transcript(
        &self,
        scopes: &[AuthEnvironmentScope],
        management: &crate::ports::thread_project::thread_management_service::ThreadManagementService,
        thread_id: ThreadId,
        environment_id: t3_contracts::base_schemas::EnvironmentId,
        max_bytes: u64,
    ) -> HttpReadResult<OrchestrationV2ThreadTranscript> {
        authorize(scopes)?;
        management
            .transcript(thread_id, environment_id, max_bytes)
            .await
            .map_err(|error| {
                if matches!(
                    &error,
                    Error::Service {
                        operation: "ThreadTranscriptTooLargeError",
                        ..
                    }
                ) {
                    HttpReadError::Invalid(
                        EnvironmentRequestInvalidReason::ThreadTranscriptTooLarge,
                    )
                } else {
                    map_read_error(
                        error,
                        EnvironmentInternalErrorReason::OrchestrationThreadSnapshotFailed,
                    )
                }
            })
    }
    pub async fn shell_snapshot(
        &self,
        scopes: &[AuthEnvironmentScope],
        query: ShellSnapshotQuery<'_>,
    ) -> HttpReadResult<ShellSnapshotAnswer> {
        authorize(scopes)?;
        let failure = EnvironmentInternalErrorReason::OrchestrationSnapshotFailed;
        if let Some(after) = parse_resume_from(query.resume_from) {
            let head = self
                .service
                .database
                .read(|db| {
                    crate::stores::event_store::EventStore::new().latest_application_sequence(db)
                })
                .await
                .map_err(|e| map_read_error(e, failure))? as u64;
            if can_resume_shell_from(&self.service.database, after, head)
                .await
                .map_err(|e| map_read_error(e, failure))?
            {
                return Ok(ShellSnapshotAnswer::Resumable(
                    OrchestrationV2ShellResumable {
                        resumable: LiteralTrue,
                    },
                ));
            }
        }
        let mut snapshot = self
            .service
            .subscriber_shell_snapshot(super::shell_stream::ShellSubscriberOptions {
                omit_idle_subagents: query.omit_idle_subagents == Some("1"),
                omit_older_settled: query.omit_older_settled == Some("1"),
            })
            .await
            .map_err(|e| map_read_error(e, failure))?;
        snapshot.archived_threads.clear();
        let identities = super::shell_stream::project_identities_digest(&snapshot.projects)
            .map_err(|e| map_read_error(e, failure))?;
        Ok(ShellSnapshotAnswer::Snapshot(Box::new(
            OrchestrationV2ShellHttpSnapshot {
                schema_version: snapshot.schema_version,
                snapshot_sequence: snapshot.snapshot_sequence,
                threads: snapshot.threads,
                archived_threads: snapshot.archived_threads,
                projects: snapshot.projects,
                older_settled: snapshot.older_settled,
                project_identities: Some(identities),
            },
        )))
    }
    pub async fn thread_snapshot(
        &self,
        scopes: &[AuthEnvironmentScope],
        thread_id: ThreadId,
    ) -> HttpReadResult<OrchestrationV2ThreadDetailSnapshot> {
        authorize(scopes)?;
        let snapshot = self.service.thread_snapshot(thread_id).await.map_err(|e| {
            map_read_error(
                e,
                EnvironmentInternalErrorReason::OrchestrationThreadSnapshotFailed,
            )
        })?;
        Ok(OrchestrationV2ThreadDetailSnapshot {
            snapshot_sequence: snapshot.snapshot_sequence,
            projection: snapshot.projection,
            history_cursor: None,
            has_more_history: None,
            latest_local_turn_ordinal: None,
        })
    }
    pub async fn bounded_thread_snapshot(
        &self,
        scopes: &[AuthEnvironmentScope],
        thread_id: ThreadId,
        query: BoundedSnapshotQuery<'_>,
    ) -> HttpReadResult<OrchestrationV2ThreadBoundedSnapshot> {
        authorize(scopes)?;
        self.service
            .bounded_thread_snapshot(
                thread_id,
                query.defer_work == Some("1"),
                SnapshotOptions {
                    compact_turn_items: query.compact_turn_items == Some("1"),
                    dedupe_bodies: query.dedupe_bodies == Some("1"),
                    compact_ids: query.compact_ids == Some("1"),
                },
            )
            .await
            .map_err(|e| {
                map_read_error(
                    e,
                    EnvironmentInternalErrorReason::OrchestrationThreadHistoryFailed,
                )
            })
    }
    pub async fn history_page(
        &self,
        scopes: &[AuthEnvironmentScope],
        thread_id: ThreadId,
        query: HistoryPageQuery<'_>,
    ) -> HttpReadResult<OrchestrationV2ThreadHistoryPage> {
        authorize(scopes)?;
        decode_thread_history_cursor(query.cursor).map_err(|_| {
            HttpReadError::Invalid(EnvironmentRequestInvalidReason::InvalidHistoryCursor)
        })?;
        self.service
            .history_page(
                HistoryQuery {
                    thread_id,
                    cursor: query.cursor.into(),
                    through_entry_id: query.through_entry_id.map(Into::into),
                    conversation_only: query.view == Some("conversation"),
                    work_mode: if query.view == Some("work") {
                        Some(HistoryWorkMode::Work)
                    } else if query.defer_work == Some("1") {
                        Some(HistoryWorkMode::Deferred)
                    } else {
                        None
                    },
                },
                query.compact_ids == Some("1"),
            )
            .await
            .map_err(|e| {
                map_read_error(
                    e,
                    EnvironmentInternalErrorReason::OrchestrationThreadHistoryFailed,
                )
            })
    }
}
