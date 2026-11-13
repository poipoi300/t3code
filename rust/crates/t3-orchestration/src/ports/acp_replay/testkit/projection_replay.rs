use crate::{
    api::{Clock, ProjectionWriter},
    error::Result,
    projections::read::unreadable_thread_ids,
    stores::{
        StoreDatabase,
        event_store::{EventFilter, EventStore},
    },
};
use std::{collections::BTreeSet, sync::Arc};
use t3_contracts::base_schemas::ThreadId;
use t3_persistence::rusqlite::{Connection, OptionalExtension, params};

const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionReplayVerification {
    pub valid: bool,
    pub schema_version: i64,
    pub expected_sequence: i64,
    pub projection_sequence: i64,
    pub unreadable_thread_ids: Vec<ThreadId>,
    pub missing_thread_ids: Vec<ThreadId>,
    pub unexpected_thread_ids: Vec<ThreadId>,
}

/// Test oracle for unpruned logs; production projections cannot be rebuilt after retention.
pub struct ProjectionReplay {
    pub database: StoreDatabase,
    pub writer: Arc<dyn ProjectionWriter>,
    pub clock: Arc<dyn Clock>,
}

fn verify(db: &Connection) -> Result<ProjectionReplayVerification> {
    let ids = |sql: &str| -> Result<BTreeSet<String>> {
        Ok(db
            .prepare(sql)?
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?)
    };
    let expected = ids(
        "SELECT DISTINCT stream_id FROM orchestration_events INDEXED BY orchestration_events_v2_created_threads_idx WHERE application_event_version=2 AND aggregate_kind='thread' AND event_type='thread.created' ORDER BY stream_id",
    )?;
    let actual =
        ids("SELECT thread_id FROM orchestration_v2_projection_threads ORDER BY thread_id")?;
    let missing_thread_ids: Vec<_> = expected
        .difference(&actual)
        .cloned()
        .map(ThreadId)
        .collect();
    let unexpected_thread_ids: Vec<_> = actual
        .difference(&expected)
        .cloned()
        .map(ThreadId)
        .collect();
    let unreadable_thread_ids = unreadable_thread_ids(db)?;
    let (schema_version, projection_sequence) = db.query_row(
        "SELECT schema_version,last_sequence FROM orchestration_v2_projection_metadata WHERE projection_name='thread-projections'",
        [], |row| Ok((row.get::<_, i64>(0)?,row.get::<_, i64>(1)?)),
    ).optional()?.unwrap_or((0,0));
    let expected_sequence = EventStore::new().latest_agent_sequence(db, None)?;
    Ok(ProjectionReplayVerification {
        valid: schema_version == SCHEMA_VERSION
            && projection_sequence == expected_sequence
            && unreadable_thread_ids.is_empty()
            && missing_thread_ids.is_empty()
            && unexpected_thread_ids.is_empty(),
        schema_version,
        expected_sequence,
        projection_sequence,
        unreadable_thread_ids,
        missing_thread_ids,
        unexpected_thread_ids,
    })
}

impl ProjectionReplay {
    pub async fn verify(&self) -> Result<ProjectionReplayVerification> {
        self.database.read(verify).await
    }

    pub async fn rebuild(&self) -> Result<ProjectionReplayVerification> {
        let writer = self.writer.clone();
        let now = self.clock.now();
        self.database.transaction(move |tx| {
            let store = EventStore::new();
            let through_sequence = store.latest_agent_sequence(tx, None)?;
            tx.execute_batch(
                "DELETE FROM orchestration_v2_projection_context_transfers;
                 DELETE FROM orchestration_v2_projection_context_handoffs;
                 DELETE FROM orchestration_v2_projection_checkpoints;
                 DELETE FROM orchestration_v2_projection_checkpoint_scopes;
                 DELETE FROM orchestration_v2_projection_turn_items;
                 DELETE FROM orchestration_v2_projection_plans;
                 DELETE FROM orchestration_v2_projection_messages;
                 DELETE FROM orchestration_v2_projection_runtime_requests;
                 DELETE FROM orchestration_v2_projection_provider_turns;
                 DELETE FROM orchestration_v2_projection_provider_threads;
                 DELETE FROM orchestration_v2_projection_provider_session_bindings;
                 DELETE FROM orchestration_v2_projection_provider_sessions;
                 DELETE FROM orchestration_v2_projection_subagents;
                 DELETE FROM orchestration_v2_projection_nodes;
                 DELETE FROM orchestration_v2_projection_run_attempts;
                 DELETE FROM orchestration_v2_projection_runs;
                 DELETE FROM orchestration_v2_projection_threads;
                 DELETE FROM orchestration_v2_turn_item_positions;",
            )?;
            let mut last_sequence = 0;
            loop {
                let page = store.read(tx, &EventFilter {
                    after_sequence: last_sequence,
                    through_sequence: Some(through_sequence),
                    limit: Some(500),
                    ..Default::default()
                })?;
                for stored in &page {
                    writer.apply(tx, stored)?;
                    if let t3_contracts::orchestration_v2::OrchestrationV2DomainEvent::TurnItemUpdated(event) = &stored.event {
                        let payload = serde_json::to_value(&event.payload)?;
                        tx.execute(
                            "INSERT INTO orchestration_v2_turn_item_positions(thread_id,turn_item_id,ordinal) VALUES(?1,?2,?3) ON CONFLICT(thread_id,turn_item_id) DO UPDATE SET ordinal=excluded.ordinal",
                            params![event.thread_id.0, payload["id"].as_str(), payload["ordinal"].as_i64()],
                        )?;
                    }
                    last_sequence = stored.sequence as i64;
                }
                if page.len() < 500 { break; }
            }
            tx.execute(
                "INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES('thread-projections',?1,?2,?3) ON CONFLICT(projection_name) DO UPDATE SET schema_version=excluded.schema_version,last_sequence=excluded.last_sequence,updated_at=excluded.updated_at",
                params![SCHEMA_VERSION, last_sequence, now.0],
            )?;
            Ok(())
        }).await?;
        self.verify().await
    }
}
