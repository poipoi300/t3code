use crate::{registry::HandlerRegistry,services::Domains};
use t3_contracts::rpc::*;

pub fn register_with(registry:&mut HandlerRegistry,domains:&Domains)->crate::Result<()> {
    #[cfg(feature="local-services")]
    if let Some(adapter)=domains.get::<crate::adapters::telemetry::TelemetryAdapter>() {
        return register(registry,adapter);
    }
    let _=domains;
    registry.unimplemented::<WsServerGetTraceDiagnosticsRpc>()?;
    registry.unimplemented::<WsServerGetProcessDiagnosticsRpc>()?;
    registry.unimplemented::<WsServerGetHostResourcesRpc>()?;
    registry.unimplemented::<WsServerGetProcessResourceHistoryRpc>()?;
    registry.unimplemented::<WsServerGetResourceTelemetryHistoryRpc>()?;
    registry.unimplemented::<WsServerRetryResourceTelemetryRpc>()?;
    registry.unimplemented::<WsServerSignalProcessRpc>()?;
    registry.unimplemented::<WsServerCheckOtlpEndpointRpc>()?;
    registry.unimplemented::<WsSubscribeResourceTelemetryRpc>()?;
    Ok(())
}

#[cfg(feature="local-services")]
fn register(registry:&mut HandlerRegistry,adapter:std::sync::Arc<crate::adapters::telemetry::TelemetryAdapter>)->crate::Result<()> {
    let traces=adapter.clone();
    registry.register::<WsServerGetTraceDiagnosticsRpc,_,_>(move|_,_| {
        let adapter=traces.clone();async move {adapter.read_traces().await.map_err(internal)}
    })?;
    let process=adapter.clone();
    registry.register::<WsServerGetProcessDiagnosticsRpc,_,_>(move|_,_| {
        let adapter=process.clone();async move {Ok(adapter.resources.process_diagnostics().await)}
    })?;
    let host=adapter.clone();
    registry.register::<WsServerGetHostResourcesRpc,_,_>(move|_,_| {
        let adapter=host.clone();async move {adapter.host.read().await.map_err(internal)}
    })?;
    let process_history=adapter.clone();
    registry.register::<WsServerGetProcessResourceHistoryRpc,_,_>(move|_,input| {
        let adapter=process_history.clone();async move {Ok(adapter.resources.read_process_history(&input).await)}
    })?;
    let history=adapter.clone();
    registry.register::<WsServerGetResourceTelemetryHistoryRpc,_,_>(move|_,input| {
        let adapter=history.clone();async move {Ok(adapter.resources.read_history(&input).await)}
    })?;
    let retry=adapter.clone();
    registry.register::<WsServerRetryResourceTelemetryRpc,_,_>(move|_,_| {
        let adapter=retry.clone();async move {Ok(adapter.resources.retry().await)}
    })?;
    let signal=adapter.clone();
    registry.register::<WsServerSignalProcessRpc,_,_>(move|_,input| {
        let adapter=signal.clone();async move {Ok(adapter.resources.signal_process(&input).await)}
    })?;
    let endpoints=adapter.clone();
    registry.register::<WsServerCheckOtlpEndpointRpc,_,_>(move|_,input| {
        let adapter=endpoints.clone();async move {Ok(adapter.endpoints.check(&input).await)}
    })?;
    registry.register_stream::<WsSubscribeResourceTelemetryRpc,_,_,_>(move|_,_| {
        let adapter=adapter.clone();async move {
            let mut subscription=adapter.resources.subscribe().await;
            Ok(async_stream::stream! {
                yield Ok(subscription.latest.clone());
                while let Some(snapshot)=subscription.recv().await {yield Ok(snapshot);}
            })
        }
    })?;
    Ok(())
}

#[cfg(feature="local-services")]
fn internal<E>(_:t3_telemetry::Error)->crate::registry::Failure<E> {
    crate::registry::Failure::Defect(crate::registry::ServerDefect::Internal {message:"Telemetry operation failed.".into()})
}
