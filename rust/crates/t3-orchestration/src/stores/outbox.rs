use super::event_store::{decode_json, encode_json, parse_json, statement, timestamp};
use crate::{
    effects::*,
    error::{Error, Result},
};
use std::sync::OnceLock;
use t3_contracts::base_schemas::{CommandId, ThreadId};
use t3_persistence::rusqlite::{Connection, OptionalExtension, Row, params};

const COLUMNS: &str = "effect_id, command_id, thread_id, payload_json, status, attempt_count, \
    available_at, lease_owner, created_at, updated_at, completed_at, last_error";
pub const PRUNE_BATCH_SIZE: usize = 500;
pub const SETTLED_EFFECT_RETENTION: std::time::Duration =
    std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// What startup recovery does with an unsettled effect whose process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterProcessLoss {
    /// `PROCESS_BOUND_EFFECT_TYPES`: pending or running rows are cancelled.
    Cancel,
    /// `REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS`: running rows return to pending.
    Requeue,
}

/// The only source of the reconciliation lists (`EffectOutbox.ts`); `after_process_loss` is the
/// exhaustive per-variant decision, and the store tests tie the two to the TypeScript constants.
pub const PROCESS_LOSS: [(&str, AfterProcessLoss); 16] = [
    ("provider-runtime.continue", AfterProcessLoss::Requeue),
    ("provider-session.detach", AfterProcessLoss::Requeue),
    (
        "provider-session.apply-model-selection",
        AfterProcessLoss::Cancel,
    ),
    ("provider-turn.start", AfterProcessLoss::Cancel),
    ("provider-turn.interrupt", AfterProcessLoss::Cancel),
    ("provider-turn.steer", AfterProcessLoss::Cancel),
    ("provider-turn.restart", AfterProcessLoss::Cancel),
    ("runtime-request.respond", AfterProcessLoss::Cancel),
    ("provider-thread.rollback", AfterProcessLoss::Requeue),
    ("checkpoint.capture", AfterProcessLoss::Requeue),
    ("scheduled-tasks.cleanup", AfterProcessLoss::Requeue),
    ("terminal.cleanup", AfterProcessLoss::Requeue),
    ("preview.cleanup", AfterProcessLoss::Requeue),
    ("attachment.cleanup", AfterProcessLoss::Requeue),
    ("thread-title.generate", AfterProcessLoss::Requeue),
    ("delegated-tasks.stop", AfterProcessLoss::Requeue),
];

pub fn after_process_loss(request: &EffectRequest) -> AfterProcessLoss {
    use AfterProcessLoss::{Cancel, Requeue};
    match request {
        EffectRequest::ApplySelection { .. }
        | EffectRequest::Start { .. }
        | EffectRequest::Interrupt { .. }
        | EffectRequest::Steer { .. }
        | EffectRequest::Restart { .. }
        | EffectRequest::Respond { .. } => Cancel,
        EffectRequest::Continue { .. }
        | EffectRequest::Detach { .. }
        | EffectRequest::Rollback { .. }
        | EffectRequest::Capture { .. }
        | EffectRequest::CleanupScheduledTasks
        | EffectRequest::CleanupTerminals
        | EffectRequest::CleanupPreviews
        | EffectRequest::CleanupAttachments { .. }
        | EffectRequest::GenerateTitle { .. }
        | EffectRequest::StopDelegated { .. } => Requeue,
    }
}

fn effect_types(class: AfterProcessLoss) -> &'static str {
    static LISTS: OnceLock<[String; 2]> = OnceLock::new();
    let lists = LISTS.get_or_init(|| {
        let list = |wanted| {
            let types: Vec<&str> = PROCESS_LOSS
                .iter()
                .filter(|(_, class)| *class == wanted)
                .map(|(effect_type, _)| *effect_type)
                .collect();
            serde_json::Value::from(types).to_string()
        };
        [
            list(AfterProcessLoss::Cancel),
            list(AfterProcessLoss::Requeue),
        ]
    });
    match class {
        AfterProcessLoss::Cancel => &lists[0],
        AfterProcessLoss::Requeue => &lists[1],
    }
}

/// `claimableCandidatePredicate`: ?1 excludes restart continuations, ?2 caps running titles.
const CANDIDATE: &str = "candidate.status = 'pending'
    AND NOT EXISTS (
        SELECT 1 FROM orchestration_v2_effect_outbox AS active
        WHERE active.thread_id = candidate.thread_id
            AND (
                active.status = 'running'
                OR (
                    active.status = 'pending'
                    AND active.rowid < candidate.rowid
                    AND (?1 = 0 OR active.effect_type != 'provider-runtime.continue')
                )
            )
            AND (
                (candidate.effect_type = 'thread-title.generate'
                    AND active.effect_type = 'thread-title.generate')
                OR (candidate.effect_type != 'thread-title.generate'
                    AND active.effect_type != 'thread-title.generate')
            )
    )
    AND (
        ?2 IS NULL
        OR candidate.effect_type != 'thread-title.generate'
        OR (SELECT COUNT(*) FROM orchestration_v2_effect_outbox AS running_title
            WHERE running_title.status = 'running'
                AND running_title.effect_type = 'thread-title.generate') < ?2
    )";

fn claim_sql() -> &'static str {
    static SQL: OnceLock<String> = OnceLock::new();
    SQL.get_or_init(|| {
        format!(
            "UPDATE orchestration_v2_effect_outbox
            SET status = 'running', attempt_count = attempt_count + 1, lease_owner = ?4,
                updated_at = ?3, last_error = NULL
            WHERE effect_id = (
                SELECT candidate.effect_id FROM orchestration_v2_effect_outbox AS candidate
                WHERE {CANDIDATE}
                    AND candidate.available_at <= ?3
                    AND (?1 = 0 OR candidate.effect_type != 'provider-runtime.continue')
                ORDER BY candidate.available_at ASC, candidate.created_at ASC,
                    candidate.effect_id ASC
                LIMIT 1
            )
            RETURNING {COLUMNS}"
        )
    })
}

fn next_claimable_sql() -> &'static str {
    static SQL: OnceLock<String> = OnceLock::new();
    SQL.get_or_init(|| {
        format!(
            "SELECT MIN(candidate.available_at) FROM orchestration_v2_effect_outbox AS candidate \
             WHERE {CANDIDATE}"
        )
    })
}

const ENQUEUE: &str = "INSERT INTO orchestration_v2_effect_outbox (
        effect_id, command_id, thread_id, effect_type, payload_json, status, attempt_count,
        available_at, created_at, updated_at
    ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, ?6, ?7, ?7)
    ON CONFLICT (effect_id) DO NOTHING";
const SUCCEED: &str = "UPDATE orchestration_v2_effect_outbox
    SET status = 'succeeded', lease_owner = NULL, completed_at = ?3, updated_at = ?3,
        last_error = NULL
    WHERE effect_id = ?1 AND status = 'running' AND lease_owner = ?2";
const FAIL: &str = "UPDATE orchestration_v2_effect_outbox
    SET status = 'failed', lease_owner = NULL, completed_at = ?3, updated_at = ?3,
        last_error = ?4
    WHERE effect_id = ?1 AND status = 'running' AND lease_owner = ?2";
const RETRY: &str = "UPDATE orchestration_v2_effect_outbox
    SET status = 'pending', available_at = ?5, lease_owner = NULL, updated_at = ?3,
        last_error = ?4
    WHERE effect_id = ?1 AND status = 'running' AND lease_owner = ?2";
const CANCEL: &str = "UPDATE orchestration_v2_effect_outbox
    SET status = 'cancelled', lease_owner = NULL, completed_at = ?3, updated_at = ?3,
        last_error = ?4
    WHERE thread_id = ?1 AND status IN ('pending', 'running')
        AND effect_type IN (SELECT value FROM json_each(?2))
    RETURNING effect_id";
const RECONCILE_CANCEL: &str = "UPDATE orchestration_v2_effect_outbox
    SET status = 'cancelled', lease_owner = NULL, completed_at = ?1, updated_at = ?1,
        last_error = 'Cancelled because the server process ended before the effect completed.'
    WHERE status IN ('pending', 'running')
        AND effect_type IN (SELECT value FROM json_each(?2))";
const RECONCILE_REQUEUE: &str = "UPDATE orchestration_v2_effect_outbox
    SET status = 'pending', lease_owner = NULL, available_at = ?1, updated_at = ?1,
        last_error = 'Requeued after the previous server process ended.'
    WHERE status = 'running' AND effect_type IN (SELECT value FROM json_each(?2))";
const PRUNE: &str = "DELETE FROM orchestration_v2_effect_outbox WHERE rowid IN (
        SELECT rowid FROM orchestration_v2_effect_outbox
        WHERE status IN ('succeeded', 'cancelled') AND completed_at < ?1
        LIMIT ?2
    )";

/// A claimed row whose payload or status this build cannot decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndecodableEffect {
    pub effect_id: String,
    pub thread_id: ThreadId,
    pub detail: String,
}

/// The decodable claim, if any, plus the claimed rows that could not be decoded.
#[derive(Debug, Default)]
pub struct Claim {
    pub effect: Option<EffectRow>,
    pub undecodable: Vec<UndecodableEffect>,
}

struct RawEffect {
    id: String,
    thread_id: String,
    decoded: Result<EffectRow>,
}

fn decode(row: &Row<'_>) -> t3_persistence::rusqlite::Result<RawEffect> {
    let id: String = row.get(0)?;
    let command_id: String = row.get(1)?;
    let thread_id: String = row.get(2)?;
    let payload: String = row.get(3)?;
    let status: String = row.get(4)?;
    let attempt_count = row.get(5)?;
    let available_at = row.get(6)?;
    let lease_owner = row.get(7)?;
    let created_at = row.get(8)?;
    let updated_at = row.get(9)?;
    let completed_at = row.get(10)?;
    let last_error = row.get(11)?;
    let decoded = (|| {
        Ok(EffectRow {
            id: id.clone(),
            command_id: CommandId(command_id),
            thread_id: ThreadId(thread_id.clone()),
            request: decode_json(parse_json(&payload)?)?,
            status: decode_json(status.into())?,
            attempt_count,
            available_at,
            lease_owner,
            created_at,
            updated_at,
            completed_at,
            last_error,
        })
    })();
    Ok(RawEffect {
        id,
        thread_id,
        decoded,
    })
}

pub fn enqueue(db: &Connection, effects: &[PendingEffect], now: &str) -> Result<()> {
    let now = timestamp("now", now)?;
    let mut insert = statement(db, ENQUEUE)?;
    for effect in effects {
        let available_at = match &effect.available_at {
            Some(available_at) => timestamp("availableAt", available_at)?,
            None => now.clone(),
        };
        insert.execute(params![
            effect.id,
            effect.command_id.0,
            effect.thread_id.0,
            effect.request.effect_type(),
            encode_json(&t3_contracts::stored::to_value(&effect.request)?)?,
            available_at,
            now
        ])?;
    }
    Ok(())
}

pub fn get(db: &Connection, effect_id: &str) -> Result<Option<EffectRow>> {
    let sql = format!("SELECT {COLUMNS} FROM orchestration_v2_effect_outbox WHERE effect_id = ?1");
    statement(db, &sql)?
        .query_row([effect_id], decode)
        .optional()?
        .map(|raw| raw.decoded)
        .transpose()
}

pub fn list_by_command(db: &Connection, command_id: &CommandId) -> Result<Vec<EffectRow>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM orchestration_v2_effect_outbox WHERE command_id = ?1 \
         ORDER BY created_at ASC, effect_id ASC"
    );
    statement(db, &sql)?
        .query_map([&command_id.0], decode)?
        .map(|raw| raw.map_err(Error::from).and_then(|raw| raw.decoded))
        .collect()
}

/// Claims the next decodable effect. See `claim_next_reporting`.
pub fn claim_next(db: &Connection, options: &ClaimOptions, now: &str) -> Result<Option<EffectRow>> {
    Ok(claim_next_reporting(db, options, now)?.effect)
}

/// The TypeScript claim autocommits before decoding, so an undecodable row stays `running` under
/// this worker's lease and blocks only its own lane. Here the caller's transaction would roll
/// such a claim back and select the same row forever, so the row keeps its claim and the next
/// candidate (necessarily another lane) is claimed in the same call; the skipped rows are
/// reported so the worker can log them.
pub fn claim_next_reporting(db: &Connection, options: &ClaimOptions, now: &str) -> Result<Claim> {
    let now = timestamp("now", now)?;
    let mut claim = Claim::default();
    let mut claim_statement = statement(db, claim_sql())?;
    loop {
        let raw = claim_statement
            .query_row(
                params![
                    options.exclude_restart_continuations,
                    options.max_running_title_generations,
                    now,
                    options.worker_id
                ],
                decode,
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(claim);
        };
        match raw.decoded {
            Ok(effect) => {
                claim.effect = Some(effect);
                return Ok(claim);
            }
            Err(error) => claim.undecodable.push(UndecodableEffect {
                effect_id: raw.id,
                thread_id: ThreadId(raw.thread_id),
                detail: error.to_string(),
            }),
        }
    }
}

/// Like `nextClaimableAt`, a stored `available_at` that is not a date is an error.
pub fn next_claimable_at(
    db: &Connection,
    max_running_titles: Option<i64>,
) -> Result<Option<String>> {
    let next: Option<String> = statement(db, next_claimable_sql())?
        .query_row(params![false, max_running_titles], |row| row.get(0))?;
    next.map(|next| timestamp("availableAt", &next)).transpose()
}

pub fn succeed(db: &Connection, effect_id: &str, worker_id: &str, now: &str) -> Result<bool> {
    let now = timestamp("now", now)?;
    Ok(statement(db, SUCCEED)?.execute(params![effect_id, worker_id, now])? == 1)
}

pub fn fail(
    db: &Connection,
    effect_id: &str,
    worker_id: &str,
    error: &str,
    now: &str,
) -> Result<bool> {
    let now = timestamp("now", now)?;
    Ok(statement(db, FAIL)?.execute(params![effect_id, worker_id, now, error])? == 1)
}

/// The worker computes available_at from its injected clock and clamped retry delay.
pub fn retry(
    db: &Connection,
    effect_id: &str,
    worker_id: &str,
    error: &str,
    now: &str,
    available_at: &str,
) -> Result<bool> {
    let now = timestamp("now", now)?;
    let available_at = timestamp("availableAt", available_at)?;
    Ok(
        statement(db, RETRY)?.execute(params![effect_id, worker_id, now, error, available_at])?
            == 1,
    )
}

pub fn cancel_unsettled(
    db: &Connection,
    thread_id: &ThreadId,
    effect_types: &[&str],
    reason: &str,
    now: &str,
) -> Result<Vec<String>> {
    if effect_types.is_empty() {
        return Ok(Vec::new());
    }
    let now = timestamp("now", now)?;
    let types = serde_json::to_string(effect_types)?;
    Ok(statement(db, CANCEL)?
        .query_map(params![thread_id.0, types, now, reason], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Run inside startup recovery's transaction, before any worker can claim.
pub fn reconcile_after_process_loss(db: &Connection, now: &str) -> Result<Reconciliation> {
    let now = timestamp("now", now)?;
    let cancelled = statement(db, RECONCILE_CANCEL)?
        .execute(params![now, effect_types(AfterProcessLoss::Cancel)])?;
    let requeued = statement(db, RECONCILE_REQUEUE)?
        .execute(params![now, effect_types(AfterProcessLoss::Requeue)])?;
    Ok(Reconciliation {
        requeued,
        cancelled,
    })
}

/// One transaction-sized batch; callers release the writer between batches.
pub fn prune_settled_batch(db: &Connection, cutoff: &str) -> Result<usize> {
    let cutoff = timestamp("cutoff", cutoff)?;
    Ok(statement(db, PRUNE)?.execute(params![cutoff, PRUNE_BATCH_SIZE as i64])?)
}

pub async fn prune_settled(database: &super::StoreDatabase, cutoff: &str) -> Result<usize> {
    let mut pruned = 0;
    loop {
        let cutoff = cutoff.to_owned();
        let deleted = database
            .transaction(move |tx| prune_settled_batch(tx, &cutoff))
            .await?;
        pruned += deleted;
        if deleted < PRUNE_BATCH_SIZE {
            return Ok(pruned);
        }
        tokio::task::yield_now().await;
    }
}
