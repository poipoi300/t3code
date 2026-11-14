use crate::{
    Error, Result, SchedulerSource,
    schedule::{iso, parse_date},
    service::{Inner, ScheduledTaskService, Trigger, run_task},
    store,
    webhook::*,
};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};
use t3_contracts::{base_schemas::ScheduledTaskId, scheduled_task::*};
use t3_persistence::rusqlite::{Connection, OptionalExtension, params};
pub const WEBHOOK_MAX_BODY_BYTES: usize = 1024 * 1024;
const LOG_LIMIT: usize = 64 * 1024;
const PROMPT_LIMIT: usize = 120_000;
const QUEUE_LIMIT: usize = 20;
/// Construct only after the transport validates RelayDeliveryProof's signed, bound JWT.
#[derive(Debug, Clone)]
pub struct VerifiedRelayDelivery {
    pub(crate) delivery_id: String,
    pub(crate) received_at: String,
}
impl VerifiedRelayDelivery {
    pub fn delivery_id(&self) -> &str {
        &self.delivery_id
    }
    pub fn received_at(&self) -> &str {
        &self.received_at
    }
}
#[derive(Debug, Clone)]
pub struct WebhookTriggerRequest {
    pub hook_id: String,
    pub token: String,
    pub request: WebhookRequest,
    /// Exact bytes used for authentication; body_text is decoded from these by the service.
    pub body: Vec<u8>,
    pub relay: Option<VerifiedRelayDelivery>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerResult {
    Accepted {
        delivery_id: String,
        outcome: String,
    },
    NotFound,
    RejectedSignature,
    Disabled,
    BlockedByPolicy,
    RateLimited {
        outcome: String,
    },
    Expired,
}
impl TriggerResult {
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Accepted { .. } => 202,
            Self::NotFound => 404,
            Self::RejectedSignature => 401,
            Self::Disabled => 409,
            Self::BlockedByPolicy => 403,
            Self::RateLimited { .. } => 429,
            Self::Expired => 410,
        }
    }
    pub fn outcome(&self) -> &str {
        match self {
            Self::Accepted { outcome, .. } | Self::RateLimited { outcome } => outcome,
            Self::NotFound => "not_found",
            Self::RejectedSignature => "rejected_signature",
            Self::Disabled => "disabled",
            Self::BlockedByPolicy => "blocked_by_policy",
            Self::Expired => "expired",
        }
    }
}
#[derive(Default)]
pub(crate) struct Rates {
    accepted: Window,
    rejected: Window,
    dedupe: Window,
    filtered: Window,
}
#[derive(Default)]
struct Window {
    slots: VecDeque<i64>,
    rejected: bool,
}
#[derive(PartialEq)]
enum Slot {
    Allowed,
    FirstRejected,
    Rejected,
}
impl Window {
    fn take(&mut self, now: i64, cap: usize) -> Slot {
        self.slots.retain(|t| now.saturating_sub(*t) < 60_000);
        if self.slots.len() < cap {
            self.rejected = false;
            self.slots.push_back(now);
            Slot::Allowed
        } else if !self.rejected {
            self.rejected = true;
            Slot::FirstRejected
        } else {
            Slot::Rejected
        }
    }
    fn refund(&mut self, now: i64) {
        if let Some(index) = self.slots.iter().rposition(|v| *v == now) {
            self.slots.remove(index);
        }
    }
}
#[derive(Clone)]
pub(crate) struct Immediate {
    task: ScheduledTask,
    delivery_id: String,
    prompt: String,
}
pub(crate) enum QueuedWork {
    Immediate(Box<Immediate>),
    Pending,
}
#[derive(Serialize, Deserialize)]
struct Pending {
    delivery_id: String,
    task_json: String,
    request_json: String,
    state: String,
}
impl ScheduledTaskService {
    /// Authentication and durable admission finish even if the HTTP caller disconnects.
    pub async fn trigger_webhook(&self, request: WebhookTriggerRequest) -> Result<TriggerResult> {
        if self.inner.cancel.is_cancelled() {
            return Err(Error::Closed);
        }
        let inner = self.inner.clone();
        let (send, recv) = tokio::sync::oneshot::channel();
        self.inner.tracker.spawn(async move {
            let result = admit(inner.clone(), request).await;
            inner
                .pending_hint
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .invalidate();
            let _ = send.send(result);
        });
        recv.await.map_err(|_| Error::Closed)?
    }
    pub async fn rotate_webhook_token(&self, id: &ScheduledTaskId) -> Result<ScheduledTask> {
        let token = crate::service::new_token()?;
        let id = id.0.clone();
        let now = iso(self.inner.clock.now());
        let task = self
            .inner
            .store
            .write(move |c| {
                let task = store::load(c, &id)?;
                if !matches!(task.schedule, ScheduledTaskSchedule::Webhook(_)) {
                    return Err(Error::Invalid(
                        "Only webhook tasks have a URL token.".into(),
                    ));
                }
                c.execute(
                    "UPDATE scheduled_tasks SET webhook_token=?2,updated_at=?3 WHERE task_id=?1",
                    params![id, token, now],
                )?;
                store::load(c, &id)
            })
            .await?;
        Self::notify(&self.inner);
        Ok(Self::present(&self.inner, task))
    }
    pub async fn list_webhook_deliveries(
        &self,
        input: ScheduledTaskListWebhookDeliveriesInput,
    ) -> Result<ScheduledTaskListWebhookDeliveriesResult> {
        self.inner
            .store
            .read(move |c| {
                let mut stmt = c.prepare("SELECT d.*,EXISTS (SELECT 1 FROM scheduled_task_webhook_pending p WHERE p.delivery_id=d.delivery_id AND p.state IN ('pending','dispatching')) AS pending_window FROM scheduled_task_webhook_deliveries d WHERE task_id=?1 ORDER BY received_at DESC,rowid DESC")?;
                let mut rows = stmt.query([input.id.0])?;
                let mut deliveries = Vec::new();
                while let Some(row) = rows.next()? {
                    let delivery = decode_delivery(row)?;
                    if input.accept_any_outcome.flatten() == Some(true) || legacy_outcome(&delivery.outcome) {
                        deliveries.push(serde_json::from_value(serde_json::to_value(delivery)?)?);
                    }
                }
                Ok(ScheduledTaskListWebhookDeliveriesResult { deliveries })
            })
            .await
    }
    pub async fn get_webhook_delivery(
        &self,
        input: ScheduledTaskGetWebhookDeliveryInput,
    ) -> Result<ScheduledTaskGetWebhookDeliveryResult> {
        self.inner
            .store
            .read(move |c| {
                let mut stmt = c.prepare("SELECT d.*,EXISTS (SELECT 1 FROM scheduled_task_webhook_pending p WHERE p.delivery_id=d.delivery_id AND p.state IN ('pending','dispatching')) AS pending_window FROM scheduled_task_webhook_deliveries d WHERE task_id=?1 AND delivery_id=?2")?;
                let mut rows = stmt.query(params![input.id.0, input.delivery_id.0.0])?;
                let row = rows.next()?.ok_or_else(|| Error::NotFound("Webhook delivery".into()))?;
                let delivery = decode_delivery(row)?;
                if input.accept_any_outcome.flatten() != Some(true) && !legacy_outcome(&delivery.outcome) {
                    return Err(Error::NotFound("Webhook delivery".into()));
                }
                Ok(ScheduledTaskGetWebhookDeliveryResult { delivery })
            })
            .await
    }
    pub async fn dispatch_pending_webhooks(&self) -> Result<()> {
        dispatch_pending(self.inner.clone()).await
    }
}
fn legacy_outcome(outcome: &ScheduledTaskWebhookDeliverySummaryOutcome) -> bool {
    matches!(
        outcome,
        ScheduledTaskWebhookDeliverySummaryOutcome::Known(
            ScheduledTaskWebhookDeliveryOutcome::Accepted
                | ScheduledTaskWebhookDeliveryOutcome::DispatchFailed
                | ScheduledTaskWebhookDeliveryOutcome::RejectedSignature
                | ScheduledTaskWebhookDeliveryOutcome::Disabled
                | ScheduledTaskWebhookDeliveryOutcome::RateLimited
                | ScheduledTaskWebhookDeliveryOutcome::Expired
        )
    )
}
fn decode_delivery(
    row: &t3_persistence::rusqlite::Row<'_>,
) -> Result<ScheduledTaskWebhookDelivery> {
    let headers: String = row.get("headers_json")?;
    let headers: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&headers).unwrap_or_default();
    let missing: String = row.get("missing_fields_json")?;
    let missing: Vec<String> = serde_json::from_str(&missing).unwrap_or_default();
    let outcome: String = row.get("outcome")?;
    Ok(serde_json::from_value(json!({
            "id":row.get::<_,String>("delivery_id")?,"taskId":row.get::<_,String>("task_id")?,"receivedAt":row.get::<_,String>("received_at")?,
            "method":row.get::<_,String>("method")?,"contentType":headers.get("content-type"),"bodyBytes":row.get::<_,i64>("body_bytes")?,
            "pendingWindow":outcome=="accepted" && row.get::<_,bool>("pending_window")?,"outcome":outcome,
            "signatureVerified":row.get::<_,bool>("signature_verified")?,"missingFields":missing,"error":row.get::<_,Option<String>>("error")?,
            "query":row.get::<_,String>("query")?,
            "headers":headers,
            "body":row.get::<_,String>("body")?,
            "bodyTruncated":row.get::<_,bool>("body_truncated")?,
            "renderedPrompt":row.get::<_,Option<String>>("rendered_prompt")?
        }
    ))?)
}
struct Log<'a> {
    id: &'a str,
    task_id: &'a str,
    received: DateTime<Utc>,
    request: &'a WebhookRequest,
    body: &'a [u8],
    outcome: &'a str,
    verified: bool,
    missing: &'a [String],
    prompt: Option<&'a str>,
    error: Option<&'a str>,
}
fn delivery_body(body: &[u8]) -> String {
    let mut bytes = &body[..body.len().min(LOG_LIMIT)];
    let mut text = String::new();
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(error) => {
                text.push_str(
                    std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("valid UTF-8 prefix"),
                );
                match error.error_len() {
                    Some(length) => {
                        text.push('\u{fffd}');
                        bytes = &bytes[error.valid_up_to() + length..];
                    }
                    None => {
                        if body.len() <= LOG_LIMIT {
                            text.push('\u{fffd}');
                        }
                        break;
                    }
                }
            }
        }
    }
    text
}
fn truncate_utf16(s: &str, limit: usize) -> String {
    let mut len = 0;
    s.chars()
        .take_while(|c| {
            len += c.len_utf16();
            len <= limit
        })
        .collect()
}
fn record(c: &Connection, log: Log<'_>) -> Result<()> {
    let headers = redact_headers(&log.request.headers);
    let query = redact_query(&log.request.query);
    let body = delivery_body(log.body);
    c.execute(
        "INSERT INTO scheduled_task_webhook_deliveries(delivery_id,task_id,received_at,method,query,headers_json,body,body_bytes,body_truncated,outcome,signature_verified,missing_fields_json,rendered_prompt,error) SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14 WHERE EXISTS (SELECT 1 FROM scheduled_tasks WHERE task_id=?2) ON CONFLICT(delivery_id) DO UPDATE SET outcome=excluded.outcome,signature_verified=excluded.signature_verified,missing_fields_json=excluded.missing_fields_json,rendered_prompt=excluded.rendered_prompt,error=excluded.error",
        params![
            log.id,
            log.task_id,
            iso(log.received),
            log.request.method,
            query,
            serde_json::to_string(&headers)?,
            body,
            log.body.len() as i64,
            log.body.len() > LOG_LIMIT,
            log.outcome,
            log.verified,
            serde_json::to_string(log.missing)?,
            log.prompt.map(|s| truncate_utf16(s, LOG_LIMIT)),
            log.error
        ],
    )?;
    let bucket = if log.outcome == "filtered" {
        "filtered"
    } else if ["duplicate", "coalesced", "rate_limited"].contains(&log.outcome)
        || log.error == Some("The filled-in prompt is too long.")
    {
        "dedupe"
    } else {
        "main"
    };
    let row_bucket = "CASE WHEN outcome='filtered' THEN 'filtered' WHEN outcome IN ('duplicate','coalesced','rate_limited') OR error='The filled-in prompt is too long.' THEN 'dedupe' ELSE 'main' END";
    let keep = if bucket == "main" { 50 } else { 20 };
    c.execute(&format!("DELETE FROM scheduled_task_webhook_deliveries WHERE task_id=?1 AND ({row_bucket})=?2 AND delivery_id NOT IN (SELECT delivery_id FROM scheduled_task_webhook_deliveries WHERE task_id=?1 AND ({row_bucket})=?2 ORDER BY received_at DESC,rowid DESC LIMIT ?3)"), params![log.task_id, bucket, keep])?;
    Ok(())
}
fn release_claim(c: &Connection, relay: Option<&VerifiedRelayDelivery>) -> Result<()> {
    if let Some(relay) = relay {
        c.execute(
            "DELETE FROM scheduled_task_webhook_relay_deliveries WHERE relay_delivery_id=?1",
            [&relay.delivery_id],
        )?;
    }
    Ok(())
}
async fn admit(inner: Arc<Inner>, mut input: WebhookTriggerRequest) -> Result<TriggerResult> {
    inner
        .pending_hint
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .invalidate();
    if inner.options.webhook_policy_block.is_some() {
        return Ok(TriggerResult::BlockedByPolicy);
    }
    if !["GET", "POST", "PUT", "PATCH"].contains(&input.request.method.as_str()) {
        return Err(Error::Invalid(
            "Method not allowed; use GET, POST, PUT or PATCH.".into(),
        ));
    }
    if input.body.len() > WEBHOOK_MAX_BODY_BYTES {
        return Err(Error::Invalid("Webhook body exceeds 1 MiB.".into()));
    }
    input.request.headers = input
        .request
        .headers
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect();
    let _admission = inner.admission.lock().await;
    let now = inner.clock.now();
    let now_ms = now.timestamp_millis();
    let received = input
        .relay
        .as_ref()
        .and_then(|r| parse_date(&r.received_at))
        .unwrap_or(now)
        .min(now);
    let id = input.relay.as_ref().map_or_else(
        || format!("delivery:{}", uuid::Uuid::new_v4()),
        |r| format!("delivery:relay:{}", r.delivery_id),
    );
    let hook = input.hook_id.clone();
    let key = hook.clone();
    let supplied_token = input.token.clone();
    let authenticated = inner
        .store
        .read(move |c| {
            let credentials: Option<(Option<String>, Option<String>)> = c
                .query_row(
                    "SELECT webhook_token,webhook_secret FROM scheduled_tasks WHERE task_id=?1",
                    [&key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((Some(token), secret)) = credentials else {
                return Ok(None);
            };
            if !constant_time_equals(&token, &supplied_token) {
                return Ok(None);
            }
            Ok(store::find(c, &key)?.map(|task| (task, secret)))
        })
        .await?;
    let Some((task, secret)) = authenticated else {
        return Ok(TriggerResult::NotFound);
    };
    let ScheduledTaskSchedule::Webhook(schedule) = task.schedule.clone() else {
        return Ok(TriggerResult::NotFound);
    };
    let key = hook.clone();
    let delivery = id.clone();
    let queue_key = hook.clone();
    let queued = inner.queues.lock().unwrap_or_else(|e| e.into_inner()).get(&queue_key).map_or(0, |queue| {
        queue
            .iter()
            .filter(|work| matches!(work,QueuedWork::Immediate(immediate) if immediate.task.created_at==task.created_at))
            .count()
    });
    let busy = usize::from(
        inner
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&hook),
    );
    let shared = inner.clone();
    let task_for_db = task.clone();
    let result = inner
        .store
        .write(move |c| {
            if let Some(relay) = &input.relay {
                let claimed = c.execute("INSERT INTO scheduled_task_webhook_relay_deliveries(relay_delivery_id,task_id,seen_at) VALUES (?1,?2,?3) ON CONFLICT(relay_delivery_id) DO NOTHING", params![relay.delivery_id, key, iso(now)])?;
                if claimed == 0 {
                    return Ok((
                        TriggerResult::Accepted {
                            delivery_id: delivery,
                            outcome: "duplicate".into(),
                        },
                        None,
                    ));
                }
                c.execute("DELETE FROM scheduled_task_webhook_relay_deliveries WHERE seen_at<?1", [iso(now - TimeDelta::hours(48))])?;
            }
            let rejected = {
                let mut rates = shared.rates.lock().unwrap_or_else(|e| e.into_inner());
                rates.entry(key.clone()).or_default().rejected.take(now_ms, 60)
            };
            if rejected == Slot::Allowed && task_for_db.enabled {
                input.request.body_text = String::from_utf8_lossy(&input.body).into_owned();
            }
            let verified = std::cell::Cell::new(false);
            let log = |outcome: &str, missing: &[String], prompt: Option<&str>, error: Option<&str>| {
                record(
                    c,
                    Log {
                        id: &delivery,
                        task_id: &key,
                        received,
                        request: &input.request,
                        body: &input.body,
                        outcome,
                        verified: verified.get(),
                        missing,
                        prompt,
                        error,
                    },
                )
            };
            let mut rates = shared.rates.lock().unwrap_or_else(|e| e.into_inner());
            let rate = rates.entry(key.clone()).or_default();
            if rejected != Slot::Allowed {
                if rejected == Slot::FirstRejected {
                    log("rate_limited", &[], None, None)?;
                }
                release_claim(c, input.relay.as_ref())?;
                return Ok((TriggerResult::RateLimited { outcome: "rate_limited".into() }, None));
            }
            if !task_for_db.enabled {
                log("disabled", &[], None, None)?;
                return Ok((TriggerResult::Disabled, None));
            }
            if let Some(signature) = &schedule.signature {
                if !secret.as_deref().is_some_and(|secret| verify_webhook_signature(signature, secret, &input.request.headers, &input.body)) {
                    log("rejected_signature", &[], None, None)?;
                    return Ok((TriggerResult::RejectedSignature, None));
                }
                verified.set(true);
            }
            if schedule
                .max_delivery_age_minutes
                .flatten()
                .is_some_and(|age| now.signed_duration_since(received) > TimeDelta::minutes(age))
            {
                log("expired", &[], None, None)?;
                return Ok((TriggerResult::Expired, None));
            }
            let context = PreparedRequest::new(&input.request);
            let failed_filter = context.failed_filter(schedule.filters.as_ref().and_then(Option::as_ref).map_or(&[], Vec::as_slice));
            if let Some((index, path)) = failed_filter {
                if rate.filtered.take(now_ms, 10) == Slot::Allowed {
                    log("filtered", &[], None, Some(&format!("Filter {} ({path}) did not match.", index + 1)))?;
                }
                rate.rejected.refund(now_ms);
                release_claim(c, input.relay.as_ref())?;
                return Ok((
                    TriggerResult::Accepted {
                        delivery_id: delivery,
                        outcome: "filtered".into(),
                    },
                    None,
                ));
            }
            if schedule.dedupe.as_ref().and_then(Option::as_ref).is_none() && rate.accepted.slots.iter().filter(|at| now_ms.saturating_sub(**at) < 60_000).count() >= 60 {
                if rate.accepted.take(now_ms, 60) == Slot::FirstRejected {
                    log("rate_limited", &[], None, None)?;
                }
                release_claim(c, input.relay.as_ref())?;
                return Ok((TriggerResult::RateLimited { outcome: "rate_limited".into() }, None));
            }
            let dedupe = schedule.dedupe.as_ref().and_then(Option::as_ref);
            let replay = context.dedupe_key(dedupe.and_then(|d| d.replay_key.as_ref().and_then(Option::as_ref)).map(String::as_str));
            let coalesce = context.dedupe_key(dedupe.and_then(|d| d.coalesce_key.as_ref().and_then(Option::as_ref)).map(String::as_str));
            let window = dedupe.and_then(|d| d.coalesce_window_seconds.flatten()).unwrap_or(120);
            let duplicate = !replay.is_empty() && c.query_row("SELECT EXISTS (SELECT 1 FROM scheduled_task_webhook_replay_keys WHERE task_id=?1 AND replay_key=?2 AND expires_at>?3)", params![key, replay, now_ms], |r| r.get::<_, bool>(0))?;
            if duplicate {
                if rate.dedupe.take(now_ms, 10) == Slot::Allowed {
                    log("duplicate", &[], None, None)?;
                }
                rate.rejected.refund(now_ms);
                release_claim(c, input.relay.as_ref())?;
                return Ok((
                    TriggerResult::Accepted {
                        delivery_id: delivery,
                        outcome: "duplicate".into(),
                    },
                    None,
                ));
            }
            let owner: Option<String> = if coalesce.is_empty() {
                None
            } else {
                c.query_row("SELECT delivery_id FROM scheduled_task_webhook_pending WHERE task_id=?1 AND coalesce_key=?2 AND state='pending' AND due_at>?3", params![key, coalesce, now_ms], |r| r.get(0)).optional()?
            };
            let count: i64 = c.query_row("SELECT COUNT(*) FROM scheduled_task_webhook_pending WHERE task_id=?1 AND state IN ('pending','dispatching','running')", [&key], |r| r.get(0))?;
            let persisted_running: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM scheduled_task_webhook_pending WHERE task_id=?1 AND state='running')", [&key], |r| r.get(0))?;
            let immediate_running = if persisted_running { 0 } else { busy };
            if owner.is_none() && count + (queued + immediate_running) as i64 >= QUEUE_LIMIT as i64 {
                rate.rejected.refund(now_ms);
                release_claim(c, input.relay.as_ref())?;
                return Ok((TriggerResult::RateLimited { outcome: "queue_full".into() }, None));
            }
            if owner.is_none() && !coalesce.is_empty() {
                let slot = rate.accepted.take(now_ms, 60);
                if slot != Slot::Allowed {
                    if slot == Slot::FirstRejected {
                        log("rate_limited", &[], None, None)?;
                    }
                    rate.rejected.refund(now_ms);
                    release_claim(c, input.relay.as_ref())?;
                    return Ok((TriggerResult::RateLimited { outcome: "rate_limited".into() }, None));
                }
            }
            if !replay.is_empty() {
                c.execute("DELETE FROM scheduled_task_webhook_replay_keys WHERE expires_at<=?1", [now_ms])?;
                c.execute("INSERT INTO scheduled_task_webhook_replay_keys(task_id,replay_key,delivery_id,expires_at) VALUES (?1,?2,?3,?4)", params![key, replay, owner.as_ref().unwrap_or(&delivery), now_ms + 7 * 24 * 60 * 60 * 1000])?;
            }
            if let Some(owner) = owner {
                c.execute("UPDATE scheduled_task_webhook_pending SET request_json=?2,delivery_ids_json=json_insert(delivery_ids_json,'$[#]',?3) WHERE delivery_id=?1", params![owner, serde_json::to_string(&input.request)?, delivery])?;
                let headers = serde_json::to_string(&redact_headers(&input.request.headers))?;
                let body = delivery_body(&input.body);
                c.execute(
                    "UPDATE scheduled_task_webhook_deliveries SET query=?2,headers_json=?3,body=?4,body_bytes=?5,body_truncated=?6,method=?7 WHERE delivery_id=?1",
                    params![
                        owner,
                        redact_query(&input.request.query),
                        headers,
                        body,
                        input.body.len() as i64,
                        input.body.len() > LOG_LIMIT,
                        input.request.method
                    ],
                )?;
                if rate.dedupe.take(now_ms, 10) == Slot::Allowed {
                    log("coalesced", &[], None, None)?;
                }
                return Ok((
                    TriggerResult::Accepted {
                        delivery_id: delivery,
                        outcome: "coalesced".into(),
                    },
                    None,
                ));
            }
            if !coalesce.is_empty() {
                rate.rejected.refund(now_ms);
                c.execute("UPDATE scheduled_task_webhook_pending SET state='dispatching' WHERE task_id=?1 AND coalesce_key=?2 AND state='pending' AND due_at<=?3", params![key, coalesce, now_ms])?;
                c.execute(
                    "INSERT INTO scheduled_task_webhook_pending(delivery_id,task_id,coalesce_key,due_at,task_json,request_json,delivery_ids_json) VALUES (?1,?2,?3,?4,?5,?6,json_array(?1))",
                    params![
                        delivery,
                        key,
                        coalesce,
                        now_ms + window * 1000,
                        t3_contracts::stored::to_string(&task_for_db)?,
                        serde_json::to_string(&input.request)?
                    ],
                )?;
                log("accepted", &[], None, None)?;
                return Ok((
                    TriggerResult::Accepted {
                        delivery_id: delivery,
                        outcome: "accepted".into(),
                    },
                    None,
                ));
            }
            let rendered = context.render(&task_for_db.prompt.0, &crate::service::frame_id()?, false);
            if rendered.prompt.trim().encode_utf16().count() > PROMPT_LIMIT {
                log("dispatch_failed", &rendered.missing, Some(&rendered.prompt), Some("The filled-in prompt is too long."))?;
                c.execute("DELETE FROM scheduled_task_webhook_replay_keys WHERE delivery_id=?1", [&delivery])?;
                release_claim(c, input.relay.as_ref())?;
                return Ok((
                    TriggerResult::Accepted {
                        delivery_id: delivery,
                        outcome: "prompt_too_long".into(),
                    },
                    None,
                ));
            }
            rate.rejected.refund(now_ms);
            if coalesce.is_empty() {
                let slot = rate.accepted.take(now_ms, 60);
                if slot != Slot::Allowed {
                    if slot == Slot::FirstRejected {
                        log("rate_limited", &[], None, None)?;
                    }
                    release_claim(c, input.relay.as_ref())?;
                    return Ok((TriggerResult::RateLimited { outcome: "rate_limited".into() }, None));
                }
            }
            log("accepted", &rendered.missing, Some(&rendered.prompt), None)?;
            Ok((
                TriggerResult::Accepted {
                    delivery_id: delivery.clone(),
                    outcome: "accepted".into(),
                },
                Some(Immediate {
                    task: task_for_db,
                    delivery_id: delivery,
                    prompt: rendered.prompt,
                }),
            ))
        })
        .await?;
    if let Some(immediate) = result.1 {
        inner
            .queues
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(queue_key)
            .or_default()
            .push_back(QueuedWork::Immediate(Box::new(immediate)));
        ensure_worker(inner.clone(), hook);
    }
    Ok(result.0)
}
pub(crate) struct WebhookSource(pub Weak<Inner>);
#[async_trait]
impl SchedulerSource for WebhookSource {
    async fn run_due_work(&self) -> Result<()> {
        if let Some(inner) = self.0.upgrade() {
            dispatch_pending(inner).await
        } else {
            Ok(())
        }
    }
}
async fn dispatch_pending(inner: Arc<Inner>) -> Result<()> {
    let now = inner.clock.now().timestamp_millis();
    let version = {
        let hint = inner.pending_hint.lock().unwrap_or_else(|e| e.into_inner());
        if hint.is_idle(now) {
            return Ok(());
        }
        hint.version()
    };
    let (tasks, next) = inner
        .store
        .write(move |c| {
            c.execute("UPDATE scheduled_task_webhook_pending SET state='dispatching' WHERE state='pending' AND due_at<=?1", [now])?;
            let tasks = c.prepare("SELECT DISTINCT task_id FROM scheduled_task_webhook_pending WHERE state IN ('dispatching','running','completed','failed') ORDER BY task_id")?.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
            let next: Option<i64> = c.query_row("SELECT MIN(due_at) FROM scheduled_task_webhook_pending WHERE state='pending'", [], |r| r.get(0))?;
            Ok((tasks, next.unwrap_or(i64::MAX)))
        })
        .await?;
    if tasks.is_empty() {
        inner
            .pending_hint
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record(version, next, now);
    }
    let _admission = inner.admission.lock().await;
    for task in tasks {
        if inner
            .pending_workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(task.clone())
        {
            inner
                .queues
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(task.clone())
                .or_default()
                .push_back(QueuedWork::Pending);
        }
        ensure_worker(inner.clone(), task);
    }
    Ok(())
}
struct WorkerGuard {
    inner: Arc<Inner>,
    id: String,
    armed: bool,
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.inner
            .webhook_workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.inner
            .pending_workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
fn ensure_worker(inner: Arc<Inner>, id: String) {
    if !inner
        .webhook_workers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone())
    {
        return;
    }
    let cloned = inner.clone();
    inner.tracker.spawn(async move {
        let mut guard = WorkerGuard {
            inner: cloned.clone(),
            id: id.clone(),
            armed: true,
        };
        tokio::select! {
        _=cloned.cancel.cancelled()=>{
        }
        ,result=worker(cloned.clone(),id,&mut guard)=>{
        if let Err(error)=result{
        tracing::warn!(%error,"Webhook worker failed");
        }
        }
        }
    });
}
async fn worker(inner: Arc<Inner>, id: String, guard: &mut WorkerGuard) -> Result<()> {
    loop {
        let admission = inner.admission.lock().await;
        let work = {
            let mut queues = inner.queues.lock().unwrap_or_else(|e| e.into_inner());
            let work = queues.get_mut(&id).and_then(VecDeque::pop_front);
            if queues.get(&id).is_some_and(VecDeque::is_empty) {
                queues.remove(&id);
            }
            work
        };
        let Some(work) = work else {
            guard.armed = false;
            inner
                .webhook_workers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Ok(());
        };
        drop(admission);
        match work {
            QueuedWork::Immediate(immediate) => {
                if let Err(error) = execute(inner.clone(), *immediate, false).await {
                    tracing::warn!(%error,"Webhook dispatch failed");
                }
            }
            QueuedWork::Pending => {
                let result = drain_pending(inner.clone(), id.clone()).await;
                inner
                    .pending_workers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                if let Err(error) = result {
                    tracing::warn!(%error,"Pending webhook dispatch failed");
                }
            }
        }
    }
}
async fn drain_pending(inner: Arc<Inner>, id: String) -> Result<()> {
    loop {
        let key = id.clone();
        let pending = inner
            .store
            .read(move |c| {
                Ok(c.query_row("SELECT delivery_id,task_json,request_json,state FROM scheduled_task_webhook_pending WHERE task_id=?1 AND state IN ('dispatching','running','completed','failed') ORDER BY due_at,rowid LIMIT 1", [key], |r| {
                    Ok(Pending {
                        delivery_id: r.get(0)?,
                        task_json: r.get(1)?,
                        request_json: r.get(2)?,
                        state: r.get(3)?,
                    })
                })
                .optional()?)
            })
            .await?;
        let Some(pending) = pending else {
            return Ok(());
        };
        let delivery = pending.delivery_id.clone();
        if pending.state == "completed" || pending.state == "failed" {
            let failed = pending.state == "failed";
            inner
                .store
                .write(move |c| {
                    if failed {
                        fail_delivery(c, &delivery, "The run failed to start.")?;
                    }
                    c.execute(
                        "DELETE FROM scheduled_task_webhook_pending WHERE delivery_id=?1",
                        [delivery],
                    )?;
                    Ok(())
                })
                .await?;
            continue;
        }
        let prepared: Result<(ScheduledTask, WebhookRequest)> = (|| {
            let task: ScheduledTask = serde_json::from_str(&pending.task_json)?;
            crate::validation::validate_persisted(&task)?;
            let request: WebhookRequest = serde_json::from_str(&pending.request_json)?;
            Ok((task, request))
        })();
        let prepared = match prepared {
            Ok((mut task, request)) => {
                if pending.state == "dispatching" {
                    let key = id.clone();
                    if let Some(current) = inner.store.read(move |c| store::find(c, &key)).await?
                        && current.created_at == task.created_at
                    {
                        task.prompt = current.prompt;
                    }
                }
                let rendered = render_webhook_prompt(
                    &task.prompt.0,
                    &request,
                    &crate::service::frame_id()?,
                    false,
                );
                if rendered.prompt.trim().encode_utf16().count() > PROMPT_LIMIT {
                    inner
                        .store
                        .write(move |c| {
                            fail_delivery(c, &delivery, "The filled-in prompt is too long.")?;
                            c.execute(
                                "DELETE FROM scheduled_task_webhook_pending WHERE delivery_id=?1",
                                [delivery],
                            )?;
                            Ok(())
                        })
                        .await?;
                    continue;
                }
                let delivery = pending.delivery_id.clone();
                let task_json = t3_contracts::stored::to_string(&task)?;
                let prompt = truncate_utf16(&rendered.prompt, LOG_LIMIT);
                let missing = serde_json::to_string(&rendered.missing)?;
                inner
                    .store
                    .write(move |c| {
                        c.execute("UPDATE scheduled_task_webhook_pending SET task_json=?2 WHERE delivery_id=?1", params![delivery, task_json])?;
                        c.execute("UPDATE scheduled_task_webhook_deliveries SET outcome='accepted',error=NULL,rendered_prompt=?2,missing_fields_json=?3 WHERE delivery_id=?1", params![delivery, prompt, missing])?;
                        Ok(())
                    })
                    .await?;
                Immediate {
                    task,
                    delivery_id: pending.delivery_id.clone(),
                    prompt: rendered.prompt,
                }
            }
            Err(_) => {
                inner
                    .store
                    .write(move |c| {
                        fail_delivery(
                            c,
                            &delivery,
                            "The pending webhook payload could not be prepared.",
                        )?;
                        c.execute(
                            "DELETE FROM scheduled_task_webhook_pending WHERE delivery_id=?1",
                            [delivery],
                        )?;
                        Ok(())
                    })
                    .await?;
                continue;
            }
        };
        execute(inner.clone(), prepared, true).await?;
    }
}
async fn execute(inner: Arc<Inner>, immediate: Immediate, persisted: bool) -> Result<()> {
    let delivery = immediate.delivery_id;
    let result = run_task(
        inner.clone(),
        immediate.task,
        Trigger::Webhook,
        Some((delivery.clone(), immediate.prompt)),
    )
    .await;
    if matches!(result, Err(Error::CompletionAfterDispatch(_)))
        || (persisted && matches!(result, Err(Error::AlreadyRunning(_))))
    {
        let key = delivery.clone();
        inner
            .store
            .write(move |c| {
                c.execute("UPDATE scheduled_task_webhook_deliveries SET outcome='dispatch_failed',error='The run started, but completion could not be recorded.' WHERE delivery_id=?1", [key])?;
                Ok(())
            })
            .await?;
        return result.map(|_| ());
    }
    let failed = match &result {
        Ok(result) => result.dispatch_failed,
        Err(_) => true,
    };
    let error = match &result {
        Err(Error::Skipped(reason)) => reason.clone(),
        _ => "The run failed to start.".into(),
    };
    inner
        .store
        .write(move |c| {
            if failed {
                fail_delivery(c, &delivery, &error)?;
            }
            if persisted {
                c.execute(
                    "DELETE FROM scheduled_task_webhook_pending WHERE delivery_id=?1",
                    [delivery],
                )?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}
fn fail_delivery(c: &Connection, delivery: &str, error: &str) -> Result<()> {
    let members: Option<String> = c
        .query_row(
            "SELECT delivery_ids_json FROM scheduled_task_webhook_pending WHERE delivery_id=?1",
            [delivery],
            |r| r.get(0),
        )
        .optional()?;
    let members: Vec<String> = members
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| vec![delivery.into()]);
    for member in members {
        c.execute("UPDATE scheduled_task_webhook_deliveries SET outcome='dispatch_failed',error=?2 WHERE delivery_id=?1", params![member, error])?;
        if let Some(relay) = member.strip_prefix("delivery:relay:") {
            c.execute(
                "DELETE FROM scheduled_task_webhook_relay_deliveries WHERE relay_delivery_id=?1",
                [relay],
            )?;
        }
    }
    c.execute(
        "DELETE FROM scheduled_task_webhook_replay_keys WHERE delivery_id=?1",
        [delivery],
    )?;
    Ok(())
}
pub fn relay_hook_base_url(relay_url: &str, tunnel_name: Option<&str>) -> Option<String> {
    let key = tunnel_name?.rsplit('-').next()?;
    if relay_url.trim_end_matches('/').is_empty()
        || key.len() != 16
        || !key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    Some(format!(
        "{}/v1/hooks/{key}",
        relay_url.trim_end_matches('/')
    ))
}
#[cfg(test)]
mod log_tests {
    use super::*;
    #[tokio::test]
    async fn delivery_upserts_preserve_request_and_require_a_live_task() {
        let dir = tempfile::tempdir().expect("create webhook fixture directory");
        let db = t3_persistence::Database::open(dir.path().join("test.sqlite"))
            .expect("open webhook fixture database");
        let request = WebhookRequest {
            method: "POST".into(),
            path: "/api/hooks/test/token".into(),
            query: "".into(),
            headers: Default::default(),
            body_text: "first".into(),
        };
        let now = parse_date("2026-07-01T08:00:00Z").expect("parse webhook fixture time");
        store::Store(db)
            .write(move |c| {
                // A real schema row, isolated from application state.
                c.execute("INSERT INTO scheduled_tasks(task_id,title,prompt,enabled,schedule_json,project_id,workspace_strategy_json,model_selection_json,runtime_mode,interaction_mode,created_by,creation_source,created_at,updated_at,last_run_status,run_count) VALUES ('test','Test','Test',1,'{}','p','{}','{}','full-access','default','user','web',?1,?1,'never',0)", [iso(now)])?;
                let make = |request, received, outcome| Log {
                    id: "delivery",
                    task_id: "test",
                    received,
                    request,
                    body: b"first",
                    outcome,
                    verified: false,
                    missing: &[],
                    prompt: None,
                    error: None,
                };
                record(c, make(&request, now, "rate_limited"))?;
                let mut retry = request.clone();
                retry.body_text = "retry".into();
                record(c, make(&retry, now + TimeDelta::minutes(1), "accepted"))?;
                let (body, received, outcome): (String, String, String) = c.query_row("SELECT body,received_at,outcome FROM scheduled_task_webhook_deliveries", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                assert_eq!(body, "first");
                assert_eq!(received, iso(now));
                assert_eq!(outcome, "accepted");
                c.execute("DELETE FROM scheduled_tasks", [])?;
                let mut log = make(&request, now, "accepted");
                log.id = "orphan";
                record(c, log)?;
                let count: i64 = c.query_row("SELECT COUNT(*) FROM scheduled_task_webhook_deliveries WHERE delivery_id='orphan'", [], |r| r.get(0))?;
                assert_eq!(count, 0);
                Ok(())
            })
            .await
            .expect("verify persisted webhook delivery updates");
    }
}
