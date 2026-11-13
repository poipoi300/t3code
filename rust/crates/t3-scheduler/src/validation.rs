use crate::{Error, Result, schedule::parse_date};
use chrono::Datelike;
use regex::Regex;
use std::sync::LazyLock;
use t3_contracts::scheduled_task::*;

static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([01]?\d|2[0-3]):([0-5]\d)$").expect("static time pattern"));
static ONCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T([01]\d|2[0-3]):[0-5]\d(:[0-5]\d(\.\d+)?)?(Z|[+-]\d{2}:\d{2})$")
        .expect("static date pattern")
});
static DEDUPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{\{\s*(?:(?:body|headers|query)(?:\.[^{}.\s]+)*|request)\s*\}\}")
        .expect("static dedupe pattern")
});

pub(crate) fn validate(input: &ScheduledTaskUpsertInput) -> Result<()> {
    let invalid = |s: &str| Error::Invalid(s.into());
    if input.title.0.trim().is_empty() || input.prompt.0.trim().is_empty() {
        return Err(invalid("Task title and prompt must not be empty."));
    }
    validate_schedule(&input.schedule, MIN_SCHEDULED_TASK_INTERVAL_MS)
}

fn validate_schedule(schedule: &ScheduledTaskUpsertSchedule, minimum_interval: i64) -> Result<()> {
    let invalid = |s: &str| Error::Invalid(s.into());
    match schedule {
        ScheduledTaskUpsertSchedule::Interval(s) if s.every_ms < minimum_interval => {
            return Err(invalid("Minimum scheduled interval is 60000 ms."));
        }
        ScheduledTaskUpsertSchedule::FixedTime(s) => {
            if !TIME.is_match(s.time_of_day.0.trim())
                || s.weekdays
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|v| v.iter().any(|d| !(0..=6).contains(d)))
            {
                return Err(invalid("Invalid fixed-time schedule."));
            }
        }
        ScheduledTaskUpsertSchedule::Once(s) => {
            let date = parse_date(&s.run_at.0);
            if !ONCE.is_match(s.run_at.0.trim())
                || date.is_none_or(|d| !(0..=9999).contains(&d.year()))
            {
                return Err(invalid("Expected an ISO date and time with an offset."));
            }
        }
        ScheduledTaskUpsertSchedule::Webhook(s) => {
            if s.max_delivery_age_minutes
                .flatten()
                .is_some_and(|v| !(1..=MAX_WEBHOOK_DELIVERY_AGE_MINUTES).contains(&v))
            {
                return Err(invalid("Invalid webhook maximum delivery age."));
            }
            if let Some(Some(d)) = &s.dedupe {
                for template in [&d.replay_key, &d.coalesce_key]
                    .into_iter()
                    .filter_map(|t| t.as_ref().and_then(Option::as_ref))
                {
                    let remainder = DEDUPE.replace_all(template, "");
                    if !DEDUPE.is_match(template)
                        || remainder.contains("{{")
                        || remainder.contains("}}")
                    {
                        return Err(invalid("Invalid webhook dedupe template."));
                    }
                }
                if d.coalesce_window_seconds
                    .flatten()
                    .is_some_and(|v| !(10..=3600).contains(&v))
                {
                    return Err(invalid(
                        "Webhook coalescing window must be 10 to 3600 seconds.",
                    ));
                }
            }
            if let Some(Some(filters)) = &s.filters {
                if filters.len() > MAX_WEBHOOK_FILTERS {
                    return Err(invalid("Too many webhook filters."));
                }
                for f in filters {
                    let path = match f {
                        ScheduledTaskWebhookFilter::Equals(f)
                        | ScheduledTaskWebhookFilter::NotEquals(f)
                        | ScheduledTaskWebhookFilter::StartsWith(f) => &f.path.0,
                        ScheduledTaskWebhookFilter::In(f) => {
                            if f.value.is_empty()
                                || f.value
                                    .iter()
                                    .any(|v| v.is_empty() || v.trim() != v || v.contains(','))
                            {
                                return Err(invalid("Invalid webhook filter list."));
                            }
                            &f.path.0
                        }
                        ScheduledTaskWebhookFilter::Exists(f) => &f.path.0,
                    };
                    if !["body", "headers", "query"]
                        .iter()
                        .any(|root| path == root || path.starts_with(&format!("{root}.")))
                    {
                        return Err(invalid("Invalid webhook filter path."));
                    }
                }
            }
            if let Some(Some(signature)) = &s.signature {
                if signature.header.0.trim().is_empty() {
                    return Err(invalid("Signature header must not be empty."));
                }
                if signature
                    .secret
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_some_and(|s| s.0.trim().is_empty())
                {
                    return Err(invalid("Signing secret must not be empty."));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate_persisted(task: &ScheduledTask) -> Result<()> {
    if task.run_count < 0 {
        return Err(Error::Invalid("Invalid stored run count.".into()));
    }
    if task.title.0.trim().is_empty() || task.prompt.0.trim().is_empty() {
        return Err(Error::Invalid(
            "Task title and prompt must not be empty.".into(),
        ));
    }
    let schedule = match &task.schedule {
        ScheduledTaskSchedule::Interval(s) => ScheduledTaskUpsertSchedule::Interval(s.clone()),
        ScheduledTaskSchedule::FixedTime(s) => ScheduledTaskUpsertSchedule::FixedTime(s.clone()),
        ScheduledTaskSchedule::Once(s) => ScheduledTaskUpsertSchedule::Once(s.clone()),
        ScheduledTaskSchedule::Webhook(s) => {
            ScheduledTaskUpsertSchedule::Webhook(Box::new(ScheduledTaskUpsertScheduleWebhook {
                signature: s.signature.as_ref().map(|s| {
                    Some(ScheduledTaskUpsertScheduleWebhookSignature {
                        header: s.header.clone(),
                        encoding: s.encoding.clone(),
                        prefix: s.prefix.clone(),
                        secret: None,
                        secret_ref: None,
                    })
                }),
                max_delivery_age_minutes: s.max_delivery_age_minutes,
                dedupe: s.dedupe.clone(),
                filters: s.filters.clone(),
            }))
        }
    };
    validate_schedule(&schedule, 1)
}
