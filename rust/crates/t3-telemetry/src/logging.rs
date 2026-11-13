use crate::{Error, Result, otlp, privacy};
use opentelemetry::{
    KeyValue, Value as OtelValue,
    logs::{AnyValue, LogRecord, Logger, LoggerProvider, Severity},
    trace::{Status, TracerProvider},
};
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    logs::{BatchLogProcessor, SdkLogger, SdkLoggerProvider},
    metrics::{PeriodicReader, SdkMeterProvider},
    trace::{BatchSpanProcessor, Sampler, SdkTracerProvider, SpanData, SpanExporter},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fmt::Debug,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use t3_config::config::{ResolvedConfig, RuntimeMode, TelemetryConfig};
use tracing_subscriber::{Layer, layer::Context, prelude::*, registry::LookupSpan};

#[derive(Clone)]
pub struct TraceSink {
    inner: Arc<SinkState>,
}
struct SinkState {
    options: TraceOptions,
    buffer: Mutex<Buffer>,
    write: Mutex<()>,
    dropped: AtomicU64,
    notify: tokio::sync::Notify,
}
struct Buffer {
    records: Vec<Vec<u8>>,
    bytes: usize,
}
#[derive(Clone)]
pub struct TraceOptions {
    pub file_path: PathBuf,
    pub max_bytes: usize,
    pub max_files: usize,
    pub batch_window: Duration,
}
impl Debug for TraceSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceSink").finish_non_exhaustive()
    }
}

impl TraceSink {
    pub fn new(options: TraceOptions) -> Result<Self> {
        if options.max_bytes == 0 || options.max_files == 0 || options.batch_window.is_zero() {
            return Err(Error::Configuration(
                "trace limits and batch window must be positive".into(),
            ));
        }
        if let Some(parent) = options.file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        prune(&options)?;
        Ok(Self {
            inner: Arc::new(SinkState {
                options,
                buffer: Mutex::new(Buffer {
                    records: vec![],
                    bytes: 0,
                }),
                write: Mutex::new(()),
                dropped: AtomicU64::new(0),
                notify: tokio::sync::Notify::new(),
            }),
        })
    }
    pub fn push(&self, mut record: Value) {
        privacy::sanitize(&mut record);
        let Ok(mut bytes) = serde_json::to_vec(&record) else {
            return;
        };
        bytes.push(b'\n');
        if bytes.len() > self.inner.options.max_bytes {
            self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let Ok(mut buffer) = self.inner.buffer.lock() else {
            return;
        };
        if buffer.bytes + bytes.len() > self.inner.options.max_bytes.max(4 * 1024 * 1024) {
            self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        buffer.bytes += bytes.len();
        buffer.records.push(bytes);
        if buffer.records.len() >= 256 {
            self.inner.notify.notify_one();
        }
    }
    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }
    pub fn flush_sync(&self) -> Result<()> {
        let _write = self.inner.write.lock().map_err(|_| Error::WorkerStopped)?;
        let records = {
            let mut buffer = self.inner.buffer.lock().map_err(|_| Error::WorkerStopped)?;
            buffer.bytes = 0;
            std::mem::take(&mut buffer.records)
        };
        let mut records = records.into_iter().peekable();
        while records.peek().is_some() {
            let mut chunk = Vec::new();
            while let Some(record) = records.peek() {
                if !chunk.is_empty() && chunk.len() + record.len() > self.inner.options.max_bytes {
                    break;
                }
                if let Some(record) = records.next() {
                    chunk.extend(record);
                }
            }
            if let Err(error) = append_rotating(&self.inner.options, &chunk) {
                self.inner
                    .dropped
                    .fetch_add(1 + records.count() as u64, Ordering::Relaxed);
                return Err(error);
            }
        }
        Ok(())
    }
    pub async fn flush(&self) -> Result<()> {
        let sink = self.clone();
        tokio::task::spawn_blocking(move || sink.flush_sync())
            .await
            .map_err(|_| Error::WorkerStopped)?
    }
    pub fn record_browser(&self, records: impl IntoIterator<Item = Value>) {
        for record in records {
            self.push(record);
        }
    }
}

fn suffix(path: &Path, index: usize) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(format!(".{index}"));
    PathBuf::from(p)
}
fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
fn prune(options: &TraceOptions) -> Result<()> {
    let Some(parent) = options.file_path.parent() else {
        return Ok(());
    };
    let base = options
        .file_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::Configuration("trace filename is not UTF-8".into()))?;
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(tail) = name.strip_prefix(&format!("{base}."))
            && let Ok(index) = tail.parse::<usize>()
            && index > options.max_files
            && index.to_string() == tail
        {
            remove_if_present(&entry.path())?;
        }
    }
    Ok(())
}
fn append_rotating(options: &TraceOptions, bytes: &[u8]) -> Result<()> {
    let size = match fs::metadata(&options.file_path) {
        Ok(m) => m.len() as usize,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e.into()),
    };
    if size > 0 && size + bytes.len() > options.max_bytes {
        remove_if_present(&suffix(&options.file_path, options.max_files))?;
        for index in (1..options.max_files).rev() {
            match fs::rename(
                suffix(&options.file_path, index),
                suffix(&options.file_path, index + 1),
            ) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                result => result?,
            }
        }
        fs::rename(&options.file_path, suffix(&options.file_path, 1))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&options.file_path)?
        .write_all(bytes)?;
    Ok(())
}

fn ns(time: SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}
fn attribute_value(value: &OtelValue) -> Value {
    match value {
        OtelValue::Bool(v) => json!(v),
        OtelValue::I64(v) => json!(v),
        OtelValue::F64(v) => json!(v),
        OtelValue::String(v) => json!(v.as_str()),
        _ => json!(value.to_string()),
    }
}
fn attributes(values: &[KeyValue]) -> Value {
    Value::Object(
        values
            .iter()
            .map(|v| (v.key.to_string(), attribute_value(&v.value)))
            .collect(),
    )
}
fn clean_attributes(values: &mut Vec<KeyValue>) {
    for value in values {
        if privacy::private_field(value.key.as_str()) {
            value.value = OtelValue::String("<redacted>".into());
        } else if matches!(&value.value, OtelValue::String(_) | OtelValue::Array(_)) {
            let mut v = Value::String(value.value.to_string());
            privacy::sanitize(&mut v);
            if let Some(text) = v.as_str() {
                value.value = OtelValue::String(text.to_owned().into());
            }
        }
    }
}
fn clean_span(span: &mut SpanData, timing: bool) {
    if !timing {
        span.start_time = UNIX_EPOCH;
        span.end_time = UNIX_EPOCH;
        for event in &mut span.events.events {
            event.timestamp = UNIX_EPOCH;
        }
    }
    span.name = privacy::redact_text(&span.name).into();
    clean_attributes(&mut span.attributes);
    for event in &mut span.events.events {
        event.name = privacy::redact_text(&event.name).into();
        clean_attributes(&mut event.attributes);
    }
    for link in &mut span.links.links {
        clean_attributes(&mut link.attributes);
    }
    if let Status::Error { description } = &mut span.status {
        *description = privacy::redact_text(description).into();
    }
}

#[derive(Debug)]
struct LocalExporter {
    sink: TraceSink,
    timing: bool,
}
impl SpanExporter for LocalExporter {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        for mut span in batch {
            clean_span(&mut span, self.timing);
            let exit = match &span.status {
                Status::Error { description } => {
                    json!({"_tag":"Failure","cause":if description.is_empty(){"Failure"}else{description.as_ref()}})
                }
                _ => json!({"_tag":"Success"}),
            };
            let duration = span
                .end_time
                .duration_since(span.start_time)
                .unwrap_or_default()
                .as_secs_f64()
                * 1000.0;
            let mut record = json!({"type":"effect-span","name":span.name,"traceId":span.span_context.trace_id().to_string(),"spanId":span.span_context.span_id().to_string(),
                "sampled":span.span_context.is_sampled(),"kind":format!("{:?}",span.span_kind).to_lowercase(),"startTimeUnixNano":ns(span.start_time),"endTimeUnixNano":ns(span.end_time),"durationMs":duration,
                "attributes":attributes(&span.attributes),"events":span.events.events.iter().map(|e| {
                    let mut attributes=attributes(&e.attributes); if let Some(level)=attributes.get("level").cloned() { attributes["effect.logLevel"]=level; }
                    json!({"name":e.name,"timeUnixNano":ns(e.timestamp),"attributes":attributes})
                }).collect::<Vec<_>>(),"links":span.links.links.iter().map(|l|json!({"traceId":l.span_context.trace_id().to_string(),"spanId":l.span_context.span_id().to_string(),"attributes":attributes(&l.attributes)})).collect::<Vec<_>>(),"exit":exit});
            if span.parent_span_id != opentelemetry::trace::SpanId::INVALID {
                record["parentSpanId"] = json!(span.parent_span_id.to_string());
            }
            self.sink.push(record);
        }
        Ok(())
    }
}
struct SafeExporter<E>(E, bool);
impl<E> Debug for SafeExporter<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedactingSpanExporter")
    }
}
impl<E: SpanExporter> SpanExporter for SafeExporter<E> {
    async fn export(&self, mut batch: Vec<SpanData>) -> OTelSdkResult {
        for span in &mut batch {
            clean_span(span, self.1);
        }
        self.0.export(batch).await
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.0.set_resource(resource);
    }
    fn shutdown_with_timeout(&mut self, timeout: Duration) -> OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }
    fn force_flush(&mut self) -> OTelSdkResult {
        self.0.force_flush()
    }
}

#[derive(Default)]
struct Visitor {
    fields: BTreeMap<String, Value>,
}
impl tracing::field::Visit for Visitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn Debug) {
        self.fields
            .insert(field.name().to_owned(), json!(format!("{value:?}")));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }
}
struct LogLayer {
    logger: Option<SdkLogger>,
    minimum: tracing_subscriber::filter::LevelFilter,
    console: bool,
}
impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for LogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().level() > &self.minimum {
            return;
        }
        let mut visitor = Visitor::default();
        event.record(&mut visitor);
        let mut fields = json!(visitor.fields);
        privacy::sanitize(&mut fields);
        let message = fields
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(event.metadata().name());
        if self.console {
            eprintln!(
                "{} {} {}",
                crate::datetime(crate::unix_millis()).0,
                event.metadata().level(),
                fields
            );
        }
        if let Some(logger) = self
            .logger
            .as_ref()
            .filter(|_| !event.metadata().target().starts_with("opentelemetry"))
        {
            let mut record = logger.create_log_record();
            record.set_timestamp(SystemTime::now());
            record.set_body(AnyValue::String(message.to_owned().into()));
            let severity = match *event.metadata().level() {
                tracing::Level::ERROR => Severity::Error,
                tracing::Level::WARN => Severity::Warn,
                tracing::Level::INFO => Severity::Info,
                tracing::Level::DEBUG => Severity::Debug,
                tracing::Level::TRACE => Severity::Trace,
            };
            record.set_severity_number(severity);
            for (key, value) in fields.as_object().into_iter().flatten() {
                if key != "message" {
                    record
                        .add_attribute(key.to_owned(), AnyValue::String(value.to_string().into()));
                }
            }
            logger.emit(record);
        }
    }
}

pub struct LoggingOptions {
    pub trace: TraceOptions,
    pub log_level: String,
    pub trace_min_level: String,
    pub trace_timing_enabled: bool,
    pub telemetry: TelemetryConfig,
    pub version: String,
    pub release_channel: String,
    pub host_name: String,
    pub mode: RuntimeMode,
    pub console: bool,
}
impl LoggingOptions {
    pub fn from_config(
        config: &ResolvedConfig,
        version: impl Into<String>,
        release_channel: impl Into<String>,
        host_name: impl Into<String>,
    ) -> Self {
        Self {
            trace: TraceOptions {
                file_path: config.server_trace_path.clone(),
                max_bytes: config.trace_max_bytes.max(1) as usize,
                max_files: config.trace_max_files.max(1) as usize,
                batch_window: Duration::from_millis(config.trace_batch_window_ms.max(1) as u64),
            },
            log_level: config.log_level.clone(),
            trace_min_level: config.trace_min_level.clone(),
            trace_timing_enabled: config.trace_timing_enabled,
            telemetry: config.telemetry.clone(),
            version: version.into(),
            release_channel: release_channel.into(),
            host_name: host_name.into(),
            mode: config.mode,
            console: true,
        }
    }
}

pub struct TelemetryHandle {
    pub sink: TraceSink,
    pub tracer_provider: SdkTracerProvider,
    pub meter_provider: Option<SdkMeterProvider>,
    pub logger_provider: Option<SdkLoggerProvider>,
    cancel: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl TelemetryHandle {
    pub async fn force_flush(&self) -> Result<()> {
        let trace = self.tracer_provider.clone();
        let metrics = self.meter_provider.clone();
        let logs = self.logger_provider.clone();
        let exported = tokio::task::spawn_blocking(move || {
            let mut failed = trace.force_flush().is_err();
            if let Some(metrics) = metrics {
                failed |= metrics.force_flush().is_err();
            }
            if let Some(logs) = logs {
                failed |= logs.force_flush().is_err();
            }
            if failed { Err(Error::Export) } else { Ok(()) }
        })
        .await
        .map_err(|_| Error::WorkerStopped)?;
        let local = self.sink.flush().await;
        exported.and(local)
    }
    pub async fn shutdown(mut self) -> Result<()> {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        let trace = self.tracer_provider.clone();
        let metrics = self.meter_provider.clone();
        let logs = self.logger_provider.clone();
        let exported = tokio::task::spawn_blocking(move || {
            let mut failed = trace.shutdown().is_err();
            if let Some(metrics) = metrics {
                failed |= metrics.shutdown().is_err();
            }
            if let Some(logs) = logs {
                failed |= logs.shutdown().is_err();
            }
            if failed { Err(Error::Export) } else { Ok(()) }
        })
        .await
        .map_err(|_| Error::WorkerStopped)?;
        let local = self.sink.flush().await;
        exported.and(local)
    }
}
impl Drop for TelemetryHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = self.sink.flush_sync();
    }
}

fn level(level: &str) -> Result<tracing_subscriber::filter::LevelFilter> {
    use tracing_subscriber::filter::LevelFilter as L;
    Ok(match level.to_ascii_lowercase().as_str() {
        "all" | "trace" => L::TRACE,
        "debug" => L::DEBUG,
        "info" => L::INFO,
        "warn" | "warning" => L::WARN,
        "error" | "fatal" => L::ERROR,
        "none" | "off" => L::OFF,
        _ => return Err(Error::Configuration("unknown log level".into())),
    })
}
fn resource(options: &LoggingOptions) -> Resource {
    let mut attributes = options.telemetry.resource_attributes.clone();
    attributes
        .entry("service.namespace".into())
        .or_insert("t3code".into());
    attributes
        .entry("service.runtime".into())
        .or_insert("t3-server".into());
    attributes
        .entry("service.instance.id".into())
        .or_insert_with(|| uuid::Uuid::new_v4().to_string());
    attributes
        .entry("host.name".into())
        .or_insert(options.host_name.clone());
    attributes
        .entry("t3code.server.managed_by".into())
        .or_insert(
            if options.mode == RuntimeMode::Desktop {
                "desktop"
            } else {
                "standalone"
            }
            .into(),
        );
    attributes
        .entry("deployment.environment.name".into())
        .or_insert_with(|| {
            match options.release_channel.as_str() {
                "dev" => "development",
                "preview" => "staging",
                _ => "production",
            }
            .into()
        });
    attributes
        .entry("t3code.release.channel".into())
        .or_insert(options.release_channel.clone());
    attributes.insert("service.name".into(), "t3code-server".into());
    attributes.insert("service.version".into(), options.version.clone());
    attributes.insert("process.runtime.name".into(), "rust".into());
    Resource::builder_empty()
        .with_attributes(attributes.into_iter().map(|(k, v)| {
            let value = if privacy::private_field(&k) {
                "<redacted>".into()
            } else {
                privacy::redact_text(&v)
            };
            KeyValue::new(k, value)
        }))
        .build()
}

pub async fn build(options: LoggingOptions) -> Result<(tracing::Dispatch, TelemetryHandle)> {
    let sink = TraceSink::new(options.trace.clone())?;
    let resource = resource(&options);
    let config = options.telemetry.clone();
    let local = sink.clone();
    let timing = options.trace_timing_enabled;
    let (provider, meter_provider, logger_provider) =
        tokio::task::spawn_blocking(move || -> Result<_> {
            let client = reqwest::blocking::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|_| Error::Export)?;
            let mut provider = SdkTracerProvider::builder()
                .with_sampler(Sampler::AlwaysOn)
                .with_resource(resource.clone())
                .with_simple_exporter(LocalExporter {
                    sink: local,
                    timing,
                });
            if let Some(endpoint) = otlp::endpoint(&config, OtlpSignal::Traces) {
                let exporter = opentelemetry_otlp::SpanExporter::builder()
                    .with_http()
                    .with_endpoint(&endpoint.url)
                    .with_protocol(otlp::protocol(&endpoint.export)?)
                    .with_http_client(otlp::ResolvedHttpClient::new(client.clone(), endpoint)?)
                    .with_headers(
                        endpoint
                            .export
                            .headers
                            .clone()
                            .unwrap_or_default()
                            .into_iter()
                            .collect(),
                    )
                    .build()
                    .map_err(|_| Error::Export)?;
                let batch = opentelemetry_sdk::trace::BatchConfigBuilder::default()
                    .with_scheduled_delay(Duration::from_millis(
                        endpoint.export.export_interval_ms.max(1) as u64,
                    ))
                    .build();
                provider = provider.with_span_processor(
                    BatchSpanProcessor::builder(SafeExporter(exporter, timing))
                        .with_batch_config(batch)
                        .build(),
                );
            }
            let meter_provider =
                if let Some(endpoint) = otlp::endpoint(&config, OtlpSignal::Metrics) {
                    let exporter = opentelemetry_otlp::MetricExporter::builder()
                        .with_http()
                        .with_endpoint(&endpoint.url)
                        .with_protocol(otlp::protocol(&endpoint.export)?)
                        .with_http_client(otlp::ResolvedHttpClient::new(client.clone(), endpoint)?)
                        .with_headers(
                            endpoint
                                .export
                                .headers
                                .clone()
                                .unwrap_or_default()
                                .into_iter()
                                .collect(),
                        )
                        .build()
                        .map_err(|_| Error::Export)?;
                    Some(
                        SdkMeterProvider::builder()
                            .with_resource(resource.clone())
                            .with_reader(
                                PeriodicReader::builder(exporter)
                                    .with_interval(Duration::from_millis(
                                        endpoint.export.export_interval_ms.max(1) as u64,
                                    ))
                                    .build(),
                            )
                            .build(),
                    )
                } else {
                    None
                };
            let logger_provider = if let Some(endpoint) = otlp::endpoint(&config, OtlpSignal::Logs)
            {
                let exporter = opentelemetry_otlp::LogExporter::builder()
                    .with_http()
                    .with_endpoint(&endpoint.url)
                    .with_protocol(otlp::protocol(&endpoint.export)?)
                    .with_http_client(otlp::ResolvedHttpClient::new(client, endpoint)?)
                    .with_headers(
                        endpoint
                            .export
                            .headers
                            .clone()
                            .unwrap_or_default()
                            .into_iter()
                            .collect(),
                    )
                    .build()
                    .map_err(|_| Error::Export)?;
                Some(
                    SdkLoggerProvider::builder()
                        .with_resource(resource)
                        .with_log_processor(
                            BatchLogProcessor::builder(exporter)
                                .with_batch_config(
                                    opentelemetry_sdk::logs::BatchConfigBuilder::default()
                                        .with_scheduled_delay(Duration::from_millis(
                                            endpoint.export.export_interval_ms.max(1) as u64,
                                        ))
                                        .build(),
                                )
                                .build(),
                        )
                        .build(),
                )
            } else {
                None
            };
            Ok((provider.build(), meter_provider, logger_provider))
        })
        .await
        .map_err(|_| Error::WorkerStopped)??;
    let log_level = level(&options.log_level)?;
    let trace_level = level(&options.trace_min_level)?;
    let logs_exported = logger_provider.is_some();
    let telemetry = tracing_opentelemetry::layer()
        .with_tracer(provider.tracer("t3code-server"))
        .with_location(false)
        .with_threads(false)
        .with_target(false)
        .with_tracked_inactivity(false)
        .with_error_events_to_status(false)
        .with_filter(tracing_subscriber::filter::filter_fn(move |metadata| {
            !metadata.target().starts_with("opentelemetry")
                && if metadata.is_event() {
                    !logs_exported && metadata.level() <= &log_level
                } else {
                    metadata.level() <= &trace_level
                }
        }));
    let subscriber = tracing_subscriber::registry()
        .with(telemetry)
        .with(LogLayer {
            logger: logger_provider.as_ref().map(|p| p.logger("t3code-server")),
            minimum: log_level,
            console: options.console,
        });
    let dispatch = tracing::Dispatch::new(subscriber);
    let cancel = tokio_util::sync::CancellationToken::new();
    let token = cancel.clone();
    let writer = sink.clone();
    let interval = options.trace.batch_window;
    let task = tokio::spawn(async move {
        loop {
            tokio::select! { _=token.cancelled()=>break,_=tokio::time::sleep(interval)=>{},_=writer.inner.notify.notified()=>{} }
            let _ = writer.flush().await;
        }
        let _ = writer.flush().await;
    });
    Ok((
        dispatch,
        TelemetryHandle {
            sink,
            tracer_provider: provider,
            meter_provider,
            logger_provider,
            cancel,
            task: Some(task),
        },
    ))
}

pub async fn initialize(options: LoggingOptions) -> Result<TelemetryHandle> {
    let warnings=options.telemetry.warnings.clone();
    let (dispatch, handle) = build(options).await?;
    tracing::dispatcher::set_global_default(dispatch)
        .map_err(|_| Error::SubscriberAlreadyInstalled)?;
    if let Some(provider) = &handle.meter_provider {
        opentelemetry::global::set_meter_provider(provider.clone());
    }
    GLOBAL_INITIALIZED.store(true,Ordering::Release);
    for warning in warnings {tracing::warn!(message=%warning);}
    Ok(handle)
}

use t3_contracts::server::OtlpSignal;

static GLOBAL_INITIALIZED:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);

pub fn is_initialized()->bool {GLOBAL_INITIALIZED.load(Ordering::Acquire)}

pub fn server_observability(config:&ResolvedConfig)->Result<t3_contracts::server::ServerObservability> {
    let active=is_initialized();
    let mut value=json!({"logsDirectoryPath":config.paths.logs().to_string_lossy(),"localTracingEnabled":active,
        "otlpTracesEnabled":active&&otlp::endpoint(&config.telemetry,OtlpSignal::Traces).is_some(),
        "otlpMetricsEnabled":active&&otlp::endpoint(&config.telemetry,OtlpSignal::Metrics).is_some(),
        "otlpLogsEnabled":active&&otlp::endpoint(&config.telemetry,OtlpSignal::Logs).is_some()});
    for (signal,prefix) in [(OtlpSignal::Traces,"otlpTraces"),(OtlpSignal::Metrics,"otlpMetrics"),(OtlpSignal::Logs,"otlpLogs")] {
        if let Some(endpoint)=otlp::endpoint(&config.telemetry,signal) {
            value[format!("{prefix}Url")]=json!(endpoint.url);
            if ["environment","settings","bootstrap"].contains(&endpoint.source.as_str()) {value[format!("{prefix}Source")]=json!(endpoint.source);}
            if let Some(variable)=&endpoint.variable {value[format!("{prefix}Variable")]=json!(variable);}
        }
    }
    Ok(serde_json::from_value(value)?)
}
