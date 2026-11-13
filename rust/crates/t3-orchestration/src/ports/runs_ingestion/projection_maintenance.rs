use super::provider_event_ingestor::epoch_millis;
use crate::{
    api::Clock,
    error::Result,
    stores::{StoreDatabase, event_store::EVENT_LOG_PRUNED_THROUGH},
};
use async_trait::async_trait;
use std::{collections::HashSet, sync::Arc};
use t3_persistence::rusqlite::{Connection, params};
pub const EVENT_LOG_PRUNE_MARK: &str = "event-log-prune-mark";
#[derive(Debug, Default)]
pub struct PruneResult {
    pub deleted_event_count: i64,
    pub deleted_receipt_count: i64,
    pub reclaimable_bytes: i64,
}
#[async_trait]
pub trait PrunePageObserver: Send + Sync {
    async fn between_pages(&self) -> Result<()>;
}
pub struct ProjectionMaintenance {
    pub database: StoreDatabase,
    pub clock: Arc<dyn Clock>,
}
async fn rest(observer: Option<&dyn PrunePageObserver>) -> Result<()> {
    if let Some(observer) = observer {
        observer.between_pages().await?;
    }
    Ok(())
}
fn references(db: &Connection, after: i64, through: i64) -> Result<Vec<i64>> {
    let mut statement=db.prepare("SELECT json_extract(payload_json,'$.promptSequence') FROM orchestration_events WHERE sequence>?1 AND sequence<=?2 AND event_type IN ('subagent.updated','turn-item.updated') AND json_type(payload_json,'$.promptSequence')='integer'")?;
    Ok(statement
        .query_map(params![after, through], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}
impl ProjectionMaintenance {
    async fn metadata(&self, name: &'static str, sequence: i64, at: String) -> Result<()> {
        self.database.transaction(move|db|{db.execute("INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES(?1,2,?2,?3) ON CONFLICT(projection_name) DO UPDATE SET last_sequence=excluded.last_sequence,updated_at=excluded.updated_at",params![name,sequence,at])?;Ok(())}).await
    }
    async fn prune_events(
        &self,
        through: i64,
        head: i64,
        observer: Option<&dyn PrunePageObserver>,
    ) -> Result<i64> {
        let mut referenced = HashSet::new();
        let mut cursor = through;
        while cursor < head {
            let end = cursor.saturating_add(1000).min(head);
            referenced.extend(
                self.database
                    .read(move |db| references(db, cursor, end))
                    .await?,
            );
            cursor = end;
            rest(observer).await?;
        }
        referenced.extend(self.database.read(move|db|{let mut statement=db.prepare("SELECT json_extract(event.payload_json,'$.promptSequence') FROM (SELECT MAX(sequence) AS sequence FROM orchestration_events GROUP BY aggregate_kind,stream_id) heads JOIN orchestration_events event ON event.sequence=heads.sequence WHERE heads.sequence<=?1 AND event.event_type IN ('subagent.updated','turn-item.updated') AND json_type(event.payload_json,'$.promptSequence')='integer'")?;Ok(statement.query_map([through],|row|row.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)}).await?);
        rest(observer).await?;
        let mut references_through = head;
        let mut deleted = 0;
        let mut cursor = 0;
        while cursor < through {
            let rows=self.database.read(move|db|{let mut statement=db.prepare("SELECT event.sequence,CASE WHEN event.application_event_version=1 THEN 0 WHEN event.event_type='thread.created' THEN 0 WHEN event.event_type='run.updated' AND json_extract(event.payload_json,'$.status')='running' THEN 0 WHEN event.sequence=(SELECT MAX(newer.sequence) FROM orchestration_events newer WHERE newer.aggregate_kind=event.aggregate_kind AND newer.stream_id=event.stream_id) THEN 0 ELSE 1 END FROM orchestration_events event WHERE event.sequence>?1 AND event.sequence<=?2 ORDER BY event.sequence LIMIT 50")?;Ok(statement.query_map(params![cursor,through],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,bool>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)}).await?;
            let candidates = rows
                .iter()
                .filter(|(_, prunable)| *prunable)
                .map(|(sequence, _)| *sequence)
                .collect::<Vec<_>>();
            if !candidates.is_empty() {
                let result=self.database.transaction(move|db|{
                    let mut statement=db.prepare("SELECT sequence,CASE WHEN event_type IN ('subagent.updated','turn-item.updated') AND json_type(payload_json,'$.promptSequence')='integer' THEN json_extract(payload_json,'$.promptSequence') END FROM orchestration_events WHERE sequence>?1 ORDER BY sequence")?;
                    for row in statement.query_map([references_through],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Option<i64>>(1)?)))? {let (sequence,prompt)=row?;if let Some(prompt)=prompt{referenced.insert(prompt);}references_through=references_through.max(sequence);}
                    let mut count=0;let mut delete=db.prepare("DELETE FROM orchestration_events WHERE sequence=?1")?;
                    for sequence in candidates {if !referenced.contains(&sequence){count+=delete.execute([sequence])? as i64;}}
                    Ok((count,referenced,references_through))
                }).await?;
                deleted += result.0;
                referenced = result.1;
                references_through = result.2;
            }
            rest(observer).await?;
            if rows.len() < 50 {
                break;
            }
            cursor = rows.last().map_or(cursor, |row| row.0);
        }
        Ok(deleted)
    }
    async fn prune_legacy(
        &self,
        head: i64,
        receipt_head: i64,
        observer: Option<&dyn PrunePageObserver>,
    ) -> Result<(i64, i64)> {
        let mut deleted_events = 0;
        let mut cursor = 0;
        loop {
            let rows=self.database.read(move|db|{let mut statement=db.prepare("SELECT event.sequence,event.aggregate_kind='thread' AND EXISTS(SELECT 1 FROM orchestration_v2_legacy_imports legacy WHERE legacy.thread_id=event.stream_id AND legacy.transcript_imported_at IS NOT NULL) FROM orchestration_events event INDEXED BY idx_orchestration_events_application_sequence WHERE event.application_event_version=1 AND event.sequence>?1 AND event.sequence<=?2 ORDER BY event.sequence LIMIT 50")?;Ok(statement.query_map(params![cursor,head],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,bool>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)}).await?;
            let obsolete = rows
                .iter()
                .filter(|(_, imported)| *imported)
                .map(|(sequence, _)| *sequence)
                .collect::<Vec<_>>();
            if !obsolete.is_empty() {
                deleted_events += self
                    .database
                    .transaction(move |db| {
                        let mut statement =
                            db.prepare("DELETE FROM orchestration_events WHERE sequence=?1")?;
                        let mut count = 0;
                        for sequence in obsolete {
                            count += statement.execute([sequence])? as i64;
                        }
                        Ok(count)
                    })
                    .await?;
            }
            rest(observer).await?;
            if rows.len() < 50 {
                break;
            }
            cursor = rows.last().map_or(cursor, |row| row.0);
        }
        let mut deleted_receipts = 0;
        let mut cursor = 0;
        loop {
            let rows=self.database.read(move|db|{let mut statement=db.prepare("SELECT receipt.rowid,receipt.command_id,receipt.command_type='legacy' AND receipt.aggregate_kind='thread' AND EXISTS(SELECT 1 FROM orchestration_v2_legacy_imports legacy WHERE legacy.thread_id=receipt.aggregate_id AND legacy.transcript_imported_at IS NOT NULL) FROM orchestration_command_receipts receipt WHERE receipt.rowid>?1 AND receipt.rowid<=?2 ORDER BY receipt.rowid LIMIT 50")?;Ok(statement.query_map(params![cursor,receipt_head],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,bool>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)}).await?;
            let obsolete = rows
                .iter()
                .filter(|(_, _, imported)| *imported)
                .map(|(_, command, _)| command.clone())
                .collect::<Vec<_>>();
            if !obsolete.is_empty() {
                deleted_receipts += self
                    .database
                    .transaction(move |db| {
                        let mut statement = db.prepare(
                            "DELETE FROM orchestration_command_receipts WHERE command_id=?1",
                        )?;
                        let mut count = 0;
                        for command in obsolete {
                            count += statement.execute([command])? as i64;
                        }
                        Ok(count)
                    })
                    .await?;
            }
            rest(observer).await?;
            if rows.len() < 50 {
                break;
            }
            cursor = rows.last().map_or(cursor, |row| row.0);
        }
        Ok((deleted_events, deleted_receipts))
    }
    pub async fn prune_event_store(
        &self,
        observer: Option<&dyn PrunePageObserver>,
    ) -> Result<PruneResult> {
        let now = self.clock.now();
        let now_ms = epoch_millis(&now.0);
        let metadata=self.database.read(|db|{let mut statement=db.prepare("SELECT projection_name,last_sequence,updated_at FROM orchestration_v2_projection_metadata WHERE projection_name IN ('thread-projections','event-log-prune-mark','event-log-pruned-through')")?;Ok(statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)}).await?;
        let read = |name| metadata.iter().find(|(key, _, _)| key == name);
        let projected = read("thread-projections").map_or(0, |(_, sequence, _)| *sequence);
        let mark = read(EVENT_LOG_PRUNE_MARK);
        let previous = read(EVENT_LOG_PRUNED_THROUGH).map_or(0, |(_, sequence, _)| *sequence);
        let (head,receipt_head)=self.database.read(|db|Ok(db.query_row("SELECT COALESCE((SELECT MAX(sequence) FROM orchestration_events),0),COALESCE((SELECT MAX(rowid) FROM orchestration_command_receipts),0)",[],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)))?)).await?;
        let due = mark
            .and_then(|(_, _, at)| epoch_millis(at))
            .zip(now_ms)
            .is_some_and(|(at, now)| now.saturating_sub(at) >= 86_400_000);
        let mut result = PruneResult::default();
        if let Some((_, sequence, _)) = mark.filter(|_| due) {
            let through = (*sequence).min(projected).max(previous);
            if through > previous {
                self.metadata(EVENT_LOG_PRUNED_THROUGH, through, now.0.clone())
                    .await?;
            }
            result.deleted_event_count += self.prune_events(through, head, observer).await?;
        }
        if mark.is_none() || due {
            self.metadata(EVENT_LOG_PRUNE_MARK, head, now.0.clone())
                .await?;
        }
        let (events, receipts) = self.prune_legacy(head, receipt_head, observer).await?;
        result.deleted_event_count += events;
        result.deleted_receipt_count = receipts;
        result.reclaimable_bytes = self
            .database
            .read(|db| {
                let free = db.query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))?;
                let size = db.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?;
                Ok(free * size)
            })
            .await?;
        Ok(result)
    }
}
