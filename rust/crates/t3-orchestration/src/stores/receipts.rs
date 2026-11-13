use super::event_store::{statement, timestamp};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use t3_contracts::base_schemas::{CommandId, ProjectId, ThreadId};
use t3_persistence::rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregateId {
    Thread(ThreadId),
    Project(ProjectId),
}
impl AggregateId {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Thread(_) => "thread",
            Self::Project(_) => "project",
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Thread(id) => &id.0,
            Self::Project(id) => &id.0,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptStatus {
    Accepted,
    Rejected,
}
impl ReceiptStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}
/// `accepted_at` is written in `DateTime.formatIso` form; `result_sequence` is a
/// `NonNegativeInt` in TypeScript, so a negative value is rejected on write and on read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReceipt {
    pub command_id: CommandId,
    pub aggregate: AggregateId,
    pub command_type: String,
    pub accepted_at: String,
    pub result_sequence: i64,
    pub status: ReceiptStatus,
    pub error: Option<String>,
}

const INSERT_IF_ABSENT: &str = "INSERT INTO orchestration_command_receipts (
        command_id, aggregate_kind, aggregate_id, command_type, accepted_at, result_sequence,
        status, error
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
    ON CONFLICT (command_id) DO NOTHING";
const UPSERT: &str = "INSERT INTO orchestration_command_receipts (
        command_id, aggregate_kind, aggregate_id, command_type, accepted_at, result_sequence,
        status, error
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
    ON CONFLICT (command_id) DO UPDATE SET
        aggregate_kind = excluded.aggregate_kind,
        aggregate_id = excluded.aggregate_id,
        command_type = excluded.command_type,
        accepted_at = excluded.accepted_at,
        result_sequence = excluded.result_sequence,
        status = excluded.status,
        error = excluded.error";
const SELECT: &str = "SELECT aggregate_kind, aggregate_id, command_type, accepted_at, \
    result_sequence, status, error FROM orchestration_command_receipts WHERE command_id = ?1";

pub fn insert_if_absent(db: &Connection, receipt: &CommandReceipt) -> Result<bool> {
    write(db, receipt, INSERT_IF_ABSENT)
}
pub fn upsert(db: &Connection, receipt: &CommandReceipt) -> Result<()> {
    write(db, receipt, UPSERT).map(|_| ())
}
fn write(db: &Connection, r: &CommandReceipt, sql: &str) -> Result<bool> {
    let result_sequence = non_negative(r.result_sequence)?;
    Ok(statement(db, sql)?.execute(params![
        r.command_id.0,
        r.aggregate.kind(),
        r.aggregate.id(),
        r.command_type,
        timestamp("acceptedAt", &r.accepted_at)?,
        result_sequence,
        r.status.as_str(),
        r.error
    ])? == 1)
}
fn non_negative(sequence: i64) -> Result<i64> {
    if sequence < 0 {
        return Err(Error::Decode {
            entity: "receipt",
            detail: format!("resultSequence {sequence} is negative"),
        });
    }
    Ok(sequence)
}
pub fn get(db: &Connection, command_id: &CommandId) -> Result<Option<CommandReceipt>> {
    let raw = statement(db, SELECT)?
        .query_row([&command_id.0], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get(2)?,
                r.get::<_, String>(3)?,
                r.get(4)?,
                r.get::<_, String>(5)?,
                r.get(6)?,
            ))
        })
        .optional()?;
    raw.map(
        |(kind, id, command_type, accepted_at, result_sequence, status, error)| {
            let aggregate = match kind.as_str() {
                "thread" => AggregateId::Thread(ThreadId(id)),
                "project" => AggregateId::Project(ProjectId(id)),
                _ => {
                    return Err(Error::Decode {
                        entity: "receipt aggregate",
                        detail: kind,
                    });
                }
            };
            let status = match status.as_str() {
                "accepted" => ReceiptStatus::Accepted,
                "rejected" => ReceiptStatus::Rejected,
                _ => {
                    return Err(Error::Decode {
                        entity: "receipt status",
                        detail: status,
                    });
                }
            };
            Ok(CommandReceipt {
                command_id: command_id.clone(),
                aggregate,
                command_type,
                accepted_at: timestamp("acceptedAt", &accepted_at)?,
                result_sequence: non_negative(result_sequence)?,
                status,
                error,
            })
        },
    )
    .transpose()
}
pub fn get_thread(db: &Connection, command_id: &CommandId) -> Result<Option<CommandReceipt>> {
    Ok(get(db, command_id)?.filter(|r| matches!(r.aggregate, AggregateId::Thread(_))))
}
pub fn get_project(db: &Connection, command_id: &CommandId) -> Result<Option<CommandReceipt>> {
    Ok(get(db, command_id)?.filter(|r| matches!(r.aggregate, AggregateId::Project(_))))
}
