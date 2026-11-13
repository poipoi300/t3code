use crate::services::{DomainContext,DomainRuntime};
use futures::future::BoxFuture;
use std::sync::Arc;
use t3_contracts::server::ServerTraceDiagnosticsResult;
use t3_telemetry::{diagnostics::TraceDiagnostics,host::HostResources,logging::{self,LoggingOptions,TelemetryHandle},otlp::OtlpEndpointCheck,resources::{ResourceTelemetry,ResourceWorker}};

pub struct TelemetryAdapter {
    traces:TraceDiagnostics,
    pub resources:Arc<ResourceTelemetry>,
    pub host:HostResources,
    pub endpoints:OtlpEndpointCheck,
    worker:tokio::sync::Mutex<Option<ResourceWorker>>,
    logging:tokio::sync::Mutex<Option<TelemetryHandle>>,
}
impl TelemetryAdapter {
    pub async fn start(context:&DomainContext<'_>,install_global_tracing:bool)->crate::Result<Arc<Self>> {
        let config=&context.local.config;
        let endpoints=OtlpEndpointCheck::new(config.telemetry.clone()).map_err(error)?;
        let logging=if install_global_tracing {
            Some(logging::initialize(LoggingOptions::from_config(config,env!("CARGO_PKG_VERSION"),"stable",t3_telemetry::host::hostname())).await.map_err(error)?)
        }else{None};
        let resources=ResourceTelemetry::new(u64::from(std::process::id()));
        let metrics_enabled=logging.as_ref().is_some_and(|handle|handle.meter_provider.is_some());
        let worker=resources.start(metrics_enabled);
        Ok(Arc::new(Self {
            traces:TraceDiagnostics::new(config.server_trace_path.clone(),config.trace_max_files.max(0) as usize),
            resources,host:HostResources::default(),endpoints,
            worker:tokio::sync::Mutex::new(Some(worker)),logging:tokio::sync::Mutex::new(logging),
        }))
    }
    pub async fn read_traces(&self)->t3_telemetry::Result<ServerTraceDiagnosticsResult> {
        let sink=self.logging.lock().await.as_ref().map(|logging|logging.sink.clone());
        if let Some(sink)=sink {sink.flush().await?;}
        self.traces.read().await
    }
}
impl DomainRuntime for TelemetryAdapter {
    fn name(&self)->&'static str {"telemetry"}
    fn activate(&self)->BoxFuture<'_,crate::Result<()>> {
        Box::pin(async {
            let span=tracing::info_span!("server.ready");
            let _entered=span.enter();
            tracing::info!("server ready");
            Ok(())
        })
    }
    fn shutdown(&self)->BoxFuture<'_,crate::Result<()>> {
        Box::pin(async {
            if let Some(worker)=self.worker.lock().await.take() {worker.shutdown().await;}
            if let Some(logging)=self.logging.lock().await.take() {logging.shutdown().await.map_err(error)?;}
            Ok(())
        })
    }
}
fn error(error:t3_telemetry::Error)->crate::Error {crate::Error::Service(format!("telemetry: {error}"))}
