use std::{collections::BTreeMap, time::Duration};
use t3_config::config::{RuntimeMode, SignalExport, TelemetryConfig};
use t3_telemetry::{
    desktop::DesktopTelemetryControl, diagnostics::TraceDiagnostics, host::HostResources,
    logging::*, resources::ResourceTelemetry,
};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("logs/server.trace.ndjson");
    let handle = initialize(LoggingOptions {
        trace: TraceOptions {
            file_path: path.clone(),
            max_bytes: 1024 * 1024,
            max_files: 2,
            batch_window: Duration::from_secs(1),
        },
        log_level: "Info".into(),
        trace_min_level: "Info".into(),
        trace_timing_enabled: true,
        telemetry: TelemetryConfig {
            disabled: true,
            warnings: vec![],
            resource_attributes: BTreeMap::new(),
            traces: None,
            metrics: None,
            logs: None,
            default_export: SignalExport {
                protocol: "http/json".into(),
                headers: None,
                export_interval_ms: 10000,
            },
        },
        version: env!("CARGO_PKG_VERSION").into(),
        release_channel: "dev".into(),
        host_name: "diagnostics-example".into(),
        mode: RuntimeMode::Web,
        console: true,
    })
    .await?;
    {
        let span = tracing::info_span!("TraceDiagnostics.read");
        let _entered = span.enter();
        tracing::info!("diagnostics example ready");
    }
    let (writer, reader) = tokio::io::duplex(128);
    DesktopTelemetryControl::new(writer)
        .announce_server_ready()
        .await?;
    let mut ready_line = String::new();
    BufReader::new(reader).read_line(&mut ready_line).await?;
    let ready: serde_json::Value = serde_json::from_str(&ready_line)?;
    handle.force_flush().await?;
    let traces = TraceDiagnostics::new(path, 2).read().await?;
    let resources = ResourceTelemetry::new(u64::from(std::process::id()))
        .refresh()
        .await?;
    let host = HostResources::default().read().await?;
    println!(
        "{}",
        serde_json::json!({"desktopControl":ready,"traceRecords":traces.record_count,"parseErrors":traces.parse_error_count,"nativeStatus":resources.health.native.status,"processes":resources.processes.len(),"hostCpuCount":host.cpu_count,"hostMemorySampled":host.total_memory_bytes>0,"externalExports":false})
    );
    handle.shutdown().await?;
    Ok(())
}
