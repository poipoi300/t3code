//! Native telemetry services; not a wasm dependency.

pub mod analytics;
pub mod desktop;
pub mod diagnostics;
pub mod history;
pub mod host;
pub mod logging;
pub mod metrics;
pub mod model;
pub mod observability;
pub mod otlp;
pub mod privacy;
pub mod product;
pub mod provider_usage;
pub mod resources;
pub mod rpc;

mod collector;
mod http;

use t3_contracts::wire::{JsNumber, WireDateTime};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("telemetry I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("telemetry data is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid telemetry configuration: {0}")]
    Configuration(String),
    #[error("telemetry worker stopped")]
    WorkerStopped,
    #[error("telemetry export failed")]
    Export,
    #[error("tracing subscriber is already installed")]
    SubscriberAlreadyInstalled,
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> u64;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        unix_millis()
    }
}
pub struct TokioClock {
    origin: tokio::time::Instant,
    epoch_ms: u64,
}
impl TokioClock {
    pub fn new(epoch_ms: u64) -> Self {
        Self {
            origin: tokio::time::Instant::now(),
            epoch_ms,
        }
    }
}
impl Clock for TokioClock {
    fn now_ms(&self) -> u64 {
        self.epoch_ms
            .saturating_add(self.origin.elapsed().as_millis() as u64)
    }
}

pub trait TelemetryNumber {
    fn from_number(value: JsNumber) -> Self;
    fn value(&self) -> f64;
}
impl TelemetryNumber for JsNumber {
    fn from_number(value: JsNumber) -> Self {
        value
    }
    fn value(&self) -> f64 {
        self.value()
    }
}
impl TelemetryNumber for t3_contracts::settings::WireNumber {
    fn from_number(value: JsNumber) -> Self {
        Self::Finite(value)
    }
    fn value(&self) -> f64 {
        match self {
            Self::Finite(value) => value.value(),
            Self::NonFinite(_) => 0.0,
        }
    }
}
impl TelemetryNumber for t3_contracts::auth::EncodedNumber {
    fn from_number(value: JsNumber) -> Self {
        Self::Finite(value)
    }
    fn value(&self) -> f64 {
        match self {
            Self::Finite(value) => value.value(),
            Self::NonFinite(_) => 0.0,
        }
    }
}
pub(crate) fn number<T: TelemetryNumber>(value: f64) -> T {
    T::from_number(JsNumber::from_f64(value).unwrap_or_else(|| 0.into()))
}

pub fn datetime(millis: u64) -> WireDateTime {
    let date = chrono::DateTime::from_timestamp_millis(millis.min(i64::MAX as u64) as i64)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH);
    WireDateTime(date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
