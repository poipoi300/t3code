use super::{
    resources::ResourcePolicy,
    scheduling::{CommandLane, Concurrency},
};
use crate::{ClientError, types::EnvironmentId};
use serde_json::{Value, json};

pub fn resource_policy(method: &str) -> ResourcePolicy {
    let mut policy = ResourcePolicy::default();
    match method {
        "assets.createUrl" => return super::assets::ASSET_URL_POLICY,
        "filesystem.browse" => {
            policy.stale_ms = 0;
            policy.idle_ttl_ms = 5_000;
        }
        "filesystem.getMetadata" | "server.getUsageSummary" => policy.stale_ms = 60_000,
        "peerLinks.list" => policy.stale_ms = 10_000,
        "review.getBranchCommits" | "review.getDiffPreview" | "preview.list" => {
            policy.stale_ms = 5_000
        }
        "server.getNpmRelease" => {
            policy.stale_ms = 60_000;
            policy.refresh_ms = Some(300_000);
        }
        "server.getHostResources" => {
            policy.stale_ms = 5_000;
            policy.idle_ttl_ms = 0;
        }
        "server.getProcessResourceHistory" | "server.getResourceTelemetryHistory" => {
            policy.stale_ms = 5_000;
            policy.idle_ttl_ms = 0;
            policy.refresh_ms = Some(5_000);
        }
        "server.searchAcpRegistry" => policy.stale_ms = 300_000,
        "server.getDatabaseStorage"
        | "server.getStorageCleanupReport"
        | "provider.auth.subscribe"
        | "provider.install.subscribe"
        | "provider.chatgpt.handoff.subscribe"
        | "subscribeResourceTelemetry"
        | "subscribeDiscoveredLocalServers" => policy.idle_ttl_ms = 0,
        _ => {}
    }
    policy
}
pub fn command_lane(
    environment: EnvironmentId,
    method: &str,
    input: &Value,
) -> Result<(CommandLane, Concurrency), ClientError> {
    let field = |name: &str| input.get(name).cloned().unwrap_or(Value::Null);
    let (group, arguments, mode) = match method {
        "server.updateSettings"
        | "server.upsertKeybinding"
        | "server.removeKeybinding"
        | "scheduledTasks.upsert"
        | "scheduledTasks.setEnabled"
        | "scheduledTasks.delete"
        | "scheduledTasks.rotateWebhookToken" => ("config", Value::Null, Concurrency::Serial),
        "server.updateServer" => ("update", Value::Null, Concurrency::Serial),
        "terminal.open" | "terminal.close" | "terminal.clear" => {
            ("terminal-lifecycle", field("threadId"), Concurrency::Serial)
        }
        "terminal.resize" => (
            "terminal-resize",
            json!([field("threadId"), field("terminalId")]),
            Concurrency::Latest,
        ),
        "preview.open" | "preview.resize" | "preview.adjust" | "preview.close" => {
            ("preview-lifecycle", field("threadId"), Concurrency::Serial)
        }
        "preview.reportStatus" => (
            "preview-status",
            json!([field("threadId"), field("tabId")]),
            Concurrency::Latest,
        ),
        "preview.reportProfiles" => (method, Value::Null, Concurrency::Latest),
        "device.configure" | "device.list" | "device.close" | "device.shutdown"
        | "device.detail" | "device.action" => ("device", Value::Null, Concurrency::Serial),
        "device.open" => (
            "device",
            json!([
                "open",
                field("threadId"),
                field("hostId"),
                field("deviceId")
            ]),
            Concurrency::Serial,
        ),
        "server.runStorageCleanup"
        | "server.prepareTriage"
        | "server.reclaimDatabaseSpace"
        | "server.refreshUsageRates"
        | "server.retryResourceTelemetry" => (method, Value::Null, Concurrency::SingleFlight),
        "server.prepareAcpRegistryAgent" | "server.uninstallAcpRegistryManagedBinary" => {
            (method, field("agentId"), Concurrency::SingleFlight)
        }
        "server.acceptAcpRegistryUrlAuth" => (
            method,
            json!([field("instanceId"), field("elicitationId")]),
            Concurrency::SingleFlight,
        ),
        "server.listAcpRegistrySessions" => (
            method,
            json!([
                field("instanceId"),
                field("projectId"),
                input
                    .get("cursor")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .unwrap_or(json!("first"))
            ]),
            Concurrency::SingleFlight,
        ),
        "server.importAcpRegistrySession" | "server.deleteAcpRegistrySession" => (
            method,
            json!([field("instanceId"), field("projectId"), field("sessionId")]),
            Concurrency::SingleFlight,
        ),
        "server.listAcpRegistryProviders" => (
            method,
            json!([field("instanceId"), field("projectId")]),
            Concurrency::SingleFlight,
        ),
        "server.setAcpRegistryProvider" | "server.disableAcpRegistryProvider" => (
            method,
            json!([field("instanceId"), field("projectId"), field("providerId")]),
            Concurrency::SingleFlight,
        ),
        "server.logoutAcpRegistry" => (method, field("instanceId"), Concurrency::SingleFlight),
        "server.refreshProviders" => (
            method,
            json!([
                field("instanceId"),
                field("cwd"),
                field("fresh").as_bool().unwrap_or(false),
                field("refreshModels").as_bool().unwrap_or(false),
                field("modelCatalogOnly").as_bool().unwrap_or(false)
            ]),
            Concurrency::SingleFlight,
        ),
        "provider.auth.start" | "provider.install.start" | "provider.consumeResetCredit" => {
            (method, input.clone(), Concurrency::SingleFlight)
        }
        "peerLinks.answerRequest" | "secrets.answerRequest" => (
            method,
            json!([field("threadId"), field("turnItemId")]),
            Concurrency::SingleFlight,
        ),
        "review.getDiffFileContents" => (
            method,
            json!([
                field("cwd"),
                field("sourceKind"),
                field("baseRef"),
                field("headRef"),
                field("oldPath"),
                field("newPath"),
                field("changeType")
            ]),
            Concurrency::SingleFlight,
        ),
        _ => (method, Value::Null, Concurrency::Parallel),
    };
    Ok((
        CommandLane {
            environment,
            group: group.into(),
            arguments: serde_json::to_string(&arguments)?,
        },
        mode,
    ))
}
pub fn invalidated_methods(method: &str) -> &'static [&'static str] {
    match method {
        "peerLinks.link" | "peerLinks.unlink" | "peerLinks.answerRequest" => &["peerLinks.list"],
        _ => &[],
    }
}
