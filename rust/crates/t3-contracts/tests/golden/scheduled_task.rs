use super::super::{Registry, register};
use t3_contracts::scheduled_task::*;
pub fn register_module(registry: &mut Registry) {
    register::<ScheduledTask>(registry, "ScheduledTask");
    register::<ScheduledTaskDeleteInput>(registry, "ScheduledTaskDeleteInput");
    register::<ScheduledTaskDeleteResult>(registry, "ScheduledTaskDeleteResult");
    register::<ScheduledTaskError>(registry, "ScheduledTaskError");
    register::<ScheduledTaskGetWebhookDeliveryInput>(
        registry,
        "ScheduledTaskGetWebhookDeliveryInput",
    );
    register::<ScheduledTaskGetWebhookDeliveryResult>(
        registry,
        "ScheduledTaskGetWebhookDeliveryResult",
    );
    register::<ScheduledTaskListInput>(registry, "ScheduledTaskListInput");
    register::<ScheduledTaskListResult>(registry, "ScheduledTaskListResult");
    register::<ScheduledTaskListWebhookDeliveriesInput>(
        registry,
        "ScheduledTaskListWebhookDeliveriesInput",
    );
    register::<ScheduledTaskListWebhookDeliveriesResult>(
        registry,
        "ScheduledTaskListWebhookDeliveriesResult",
    );
    register::<ScheduledTaskMutationResult>(registry, "ScheduledTaskMutationResult");
    register::<ScheduledTaskRotateWebhookTokenInput>(
        registry,
        "ScheduledTaskRotateWebhookTokenInput",
    );
    register::<ScheduledTaskRunNowInput>(registry, "ScheduledTaskRunNowInput");
    register::<ScheduledTaskRunNowResult>(registry, "ScheduledTaskRunNowResult");
    register::<ScheduledTaskRunStatus>(registry, "ScheduledTaskRunStatus");
    register::<ScheduledTaskSchedule>(registry, "ScheduledTaskSchedule");
    register::<ScheduledTaskSetEnabledInput>(registry, "ScheduledTaskSetEnabledInput");
    register::<ScheduledTaskThreadMode>(registry, "ScheduledTaskThreadMode");
    register::<ScheduledTaskUpsertInput>(registry, "ScheduledTaskUpsertInput");
    register::<ScheduledTaskUpsertSchedule>(registry, "ScheduledTaskUpsertSchedule");
    register::<ScheduledTaskWebhookDelivery>(registry, "ScheduledTaskWebhookDelivery");
    register::<ScheduledTaskWebhookDeliveryId>(registry, "ScheduledTaskWebhookDeliveryId");
    register::<ScheduledTaskWebhookDeliveryOutcome>(
        registry,
        "ScheduledTaskWebhookDeliveryOutcome",
    );
    register::<ScheduledTaskWebhookDeliverySummary>(
        registry,
        "ScheduledTaskWebhookDeliverySummary",
    );
    register::<ScheduledTaskWebhookEndpoint>(registry, "ScheduledTaskWebhookEndpoint");
    register::<ScheduledTaskWebhookFilter>(registry, "ScheduledTaskWebhookFilter");
    register::<ScheduledTaskWebhookSignature>(registry, "ScheduledTaskWebhookSignature");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("scheduled_task", register_module)
}

#[test]
fn future_delivery_outcomes_stay_readable_and_webhook_presence_stays_distinct()
-> Result<(), serde_json::Error> {
    use serde_json::json;
    for raw in [
        json!({"type":"webhook"}),
        json!({"type":"webhook", "signature":null}),
        json!({"type":"webhook", "signature":{"header":"x-signature", "encoding":"hex", "prefix":"sha256=", "secretRef":"secret:1"}}),
    ] {
        let schedule: ScheduledTaskUpsertSchedule = serde_json::from_value(raw.clone())?;
        assert_eq!(serde_json::to_value(schedule)?, raw);
    }
    let raw = json!({"id":"delivery:1", "taskId":"task:1", "receivedAt":"", "method":"POST", "contentType":null, "bodyBytes":9007199254740991_i64, "outcome":"future-outcome", "signatureVerified":false, "missingFields":[], "error":null});
    let delivery: ScheduledTaskWebhookDeliverySummary = serde_json::from_value(raw.clone())?;
    assert!(
        matches!(&delivery.outcome, ScheduledTaskWebhookDeliverySummaryOutcome::Other(value) if value == "future-outcome")
    );
    assert_eq!(serde_json::to_value(delivery)?, raw);
    assert!(serde_json::from_value::<ScheduledTaskSchedule>(json!({"type":"webhook"})).is_err());
    Ok(())
}
