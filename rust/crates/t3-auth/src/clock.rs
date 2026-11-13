use crate::{Error, Result};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use t3_contracts::wire::WireDateTime;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
    fn wait_until(&self, deadline_ms: i64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Epoch anchored to Tokio's monotonic clock; also works with paused test time.
pub struct TokioClock {
    epoch_ms: i64,
    start: tokio::time::Instant,
}
impl TokioClock {
    pub fn at(epoch_ms: i64) -> Arc<Self> {
        Arc::new(Self {
            epoch_ms,
            start: tokio::time::Instant::now(),
        })
    }
    pub fn system() -> Arc<Self> {
        Self::at(chrono::Utc::now().timestamp_millis())
    }
}

/// Production clock follows wall-clock corrections, like Effect's DateTime.now.
pub struct SystemClock;
impl SystemClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
    fn wait_until(&self, deadline_ms: i64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(Duration::from_millis(
            deadline_ms.saturating_sub(self.now_ms()).max(0) as u64,
        )))
    }
}
impl Clock for TokioClock {
    fn now_ms(&self) -> i64 {
        self.epoch_ms + self.start.elapsed().as_millis() as i64
    }
    fn wait_until(&self, deadline_ms: i64) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(Duration::from_millis(
            deadline_ms.saturating_sub(self.now_ms()).max(0) as u64,
        )))
    }
}

pub fn iso(ms: i64) -> Result<WireDateTime> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| WireDateTime(d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
        .ok_or_else(|| Error::Date(ms.to_string()))
}
pub fn epoch(value: &WireDateTime) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(&value.0)
        .map(|d| d.timestamp_millis())
        .map_err(|_| Error::Date(value.0.clone()))
}

pub(crate) fn expires_after(now: i64, ttl: i64) -> Result<i64> {
    now.checked_add(ttl)
        .ok_or_else(|| Error::Date("TTL overflow".into()))
}
