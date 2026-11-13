#![allow(clippy::unwrap_used)]
use t3_server::{handlers, services::Domains};

#[test]
fn optional_methods_have_one_unavailable_registration_without_started_services() {
    let registry = handlers::registry_with(&Domains::default()).unwrap();
    registry.validate().unwrap();
    for method in [
        "scheduledTasks.subscribe",
        "scheduledTasks.upsert",
        "scheduledTasks.setEnabled",
        "scheduledTasks.delete",
        "scheduledTasks.runNow",
        "scheduledTasks.rotateWebhookToken",
        "scheduledTasks.listWebhookDeliveries",
        "scheduledTasks.getWebhookDelivery",
        "server.getUsageSummary",
        "server.refreshUsageRates",
        "provider.auth.start",
        "provider.install.start",
        "provider.consumeResetCredit",
    ] {
        assert_eq!(registry.disposition(method), Some(false), "{method}");
    }
}
