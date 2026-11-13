use crate::metrics::Metrics;
use std::{collections::BTreeMap, future::Future, sync::LazyLock, time::Instant};
use tracing::Instrument;

static AGGREGATES: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../fixtures/rpc-aggregates.json"))
        .expect("reviewed RPC aggregate labels")
});
static METRICS: LazyLock<Metrics> =
    LazyLock::new(|| Metrics::new(opentelemetry::global::meter("t3code-server")));
tokio::task_local! {static TRACING_DISABLED:bool;}

pub fn aggregate(method: &str) -> &str {
    AGGREGATES
        .get(method)
        .map(String::as_str)
        .unwrap_or("unknown")
}
pub(crate) fn tracing_suppressed() -> bool {
    TRACING_DISABLED
        .try_with(|disabled| *disabled)
        .unwrap_or(false)
}
pub fn tracing_disabled(method: &str) -> bool {
    matches!(
        method,
        "server.getTraceDiagnostics"
            | "server.getProcessDiagnostics"
            | "server.getProcessResourceHistory"
            | "server.signalProcess"
    )
}
pub fn request_span(method: &'static str) -> tracing::Span {
    if tracing_disabled(method) {
        return tracing::Span::none();
    }
    tracing::info_span!("ws.rpc",otel.name=%format!("ws.rpc.{method}"),"rpc.transport"="websocket","rpc.system"="effect-rpc","rpc.method"=method,"rpc.aggregate"=aggregate(method))
}
pub async fn poll<T>(method: &str, future: impl Future<Output = T>, span: tracing::Span) -> T {
    if tracing_disabled(method) {
        TRACING_DISABLED.scope(true, future).await
    } else {
        future.instrument(span).await
    }
}

#[derive(Clone, Copy)]
pub enum Outcome {
    Success,
    Failure,
    Interrupt,
}
impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Interrupt => "interrupt",
        }
    }
}
pub struct RequestMeasurement {
    method: &'static str,
    started: Instant,
    completed: bool,
}
impl RequestMeasurement {
    pub fn new(method: &'static str) -> Self {
        Self {
            method,
            started: Instant::now(),
            completed: false,
        }
    }
    pub fn complete(&mut self, outcome: Outcome) {
        if std::mem::replace(&mut self.completed, true) || !crate::logging::is_initialized() {
            return;
        }
        let mut attributes = [("method".into(), self.method.into())]
            .into_iter()
            .collect();
        METRICS.duration(
            "t3_rpc_request_duration",
            &attributes,
            self.started.elapsed().as_secs_f64(),
        );
        attributes.insert("outcome".into(), outcome.label().into());
        METRICS.increment("t3_rpc_requests_total", &attributes, 1);
    }
}
impl Drop for RequestMeasurement {
    fn drop(&mut self) {
        self.complete(Outcome::Interrupt);
    }
}
