//! Optional services register only when the composition root supplies a started adapter.
use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::*;

pub fn register(registry: &mut HandlerRegistry) -> crate::Result<()> {
    register_with(registry, &Domains::default())
}

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(feature = "scheduler")]
    match domains.get::<crate::adapters::scheduler::SchedulerAdapter>() {
        Some(adapter) => register_scheduler(registry, adapter)?,
        None => scheduler_unavailable(registry)?,
    }
    #[cfg(not(feature = "scheduler"))]
    scheduler_unavailable(registry)?;
    #[cfg(all(feature = "usage", feature = "local-services"))]
    match domains.get::<crate::adapters::usage::UsageAdapter>() {
        Some(adapter) => register_usage(registry, adapter)?,
        None => usage_unavailable(registry)?,
    }
    #[cfg(not(all(feature = "usage", feature = "local-services")))]
    usage_unavailable(registry)?;
    let _ = domains;
    Ok(())
}
fn scheduler_unavailable(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsScheduledTasksSubscribeRpc>()?;
    registry.unimplemented::<WsScheduledTasksUpsertRpc>()?;
    registry.unimplemented::<WsScheduledTasksSetEnabledRpc>()?;
    registry.unimplemented::<WsScheduledTasksDeleteRpc>()?;
    registry.unimplemented::<WsScheduledTasksRunNowRpc>()?;
    registry.unimplemented::<WsScheduledTasksRotateWebhookTokenRpc>()?;
    registry.unimplemented::<WsScheduledTasksListWebhookDeliveriesRpc>()?;
    registry.unimplemented::<WsScheduledTasksGetWebhookDeliveryRpc>()?;
    Ok(())
}
fn usage_unavailable(registry: &mut HandlerRegistry) -> crate::Result<()> {
    registry.unimplemented::<WsServerGetUsageSummaryRpc>()?;
    registry.unimplemented::<WsServerRefreshUsageRatesRpc>()?;
    Ok(())
}
#[cfg(feature = "scheduler")]
fn register_scheduler(
    registry: &mut HandlerRegistry,
    adapter: std::sync::Arc<crate::adapters::scheduler::SchedulerAdapter>,
) -> crate::Result<()> {
    let service = adapter.clone();
    registry.register_stream::<WsScheduledTasksSubscribeRpc, _, _, _>(move |ctx, _| {
        let service = service.clone();
        // TypeScript `withVisibleWebhookUrls`: webhook URLs start agents, so only
        // operate-scoped sessions see them; the tasks stay readable.
        let operate = ctx
            .scopes
            .contains(&t3_contracts::auth::AuthEnvironmentScope::OrchestrationOperate);
        async move {
            use futures::StreamExt;
            let stream = service.subscribe().await?;
            Ok(stream
                .map(move |list| {
                    list.map(|mut list| {
                        if !operate {
                            for task in &mut list.tasks {
                                task.webhook = None;
                            }
                        }
                        list
                    })
                })
                .boxed())
        }
    })?;
    let service = adapter.clone();
    if adapter.commands_available() {
        registry.register::<WsScheduledTasksUpsertRpc, _, _>(move |_, input| {
            let service = service.clone();
            async move { service.upsert(input).await }
        })?;
        let service = adapter.clone();
        registry.register::<WsScheduledTasksSetEnabledRpc, _, _>(move |_, input| {
            let service = service.clone();
            async move { service.set_enabled(input).await }
        })?;
        let service = adapter.clone();
        registry.register::<WsScheduledTasksRunNowRpc, _, _>(move |_, input| {
            let service = service.clone();
            async move { service.run_now(input).await }
        })?;
    } else {
        registry.unimplemented::<WsScheduledTasksUpsertRpc>()?;
        registry.unimplemented::<WsScheduledTasksSetEnabledRpc>()?;
        registry.unimplemented::<WsScheduledTasksRunNowRpc>()?;
    }
    let service = adapter.clone();
    registry.register::<WsScheduledTasksDeleteRpc, _, _>(move |_, input| {
        let service = service.clone();
        async move { service.delete(input).await }
    })?;
    let service = adapter.clone();
    registry.register::<WsScheduledTasksRotateWebhookTokenRpc, _, _>(move |_, input| {
        let service = service.clone();
        async move { service.rotate_webhook_token(input).await }
    })?;
    let service = adapter.clone();
    registry.register::<WsScheduledTasksListWebhookDeliveriesRpc, _, _>(move |_, input| {
        let service = service.clone();
        async move { service.list_webhook_deliveries(input).await }
    })?;
    registry.register::<WsScheduledTasksGetWebhookDeliveryRpc, _, _>(move |_, input| {
        let service = adapter.clone();
        async move { service.get_webhook_delivery(input).await }
    })?;
    Ok(())
}
#[cfg(all(feature = "usage", feature = "local-services"))]
fn register_usage(
    registry: &mut HandlerRegistry,
    adapter: std::sync::Arc<crate::adapters::usage::UsageAdapter>,
) -> crate::Result<()> {
    let service = adapter.clone();
    registry.register::<WsServerGetUsageSummaryRpc, _, _>(move |ctx, input| {
        let service = service.clone();
        async move { service.read_summary_for(&ctx.services, input).await }
    })?;
    if adapter.rates_available() {
        registry.register::<WsServerRefreshUsageRatesRpc, _, _>(move |_, _| {
            let service = adapter.clone();
            async move { service.refresh_rates().await }
        })?;
    } else {
        registry.unimplemented::<WsServerRefreshUsageRatesRpc>()?;
    }
    Ok(())
}
