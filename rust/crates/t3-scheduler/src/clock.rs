use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use std::time::Duration;
use tokio::time::Instant;

#[async_trait]
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Utc>;
    async fn sleep(&self, duration: Duration);
}

/// Production clock follows wall-clock corrections; timer waits remain monotonic.
#[derive(Default)]
pub struct SystemClock;
#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        let now = Utc::now();
        DateTime::from_timestamp_millis(now.timestamp_millis()).unwrap_or(now)
    }
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Wall-clock origin plus Tokio's monotonic clock, including its paused test clock.
pub struct TokioClock {
    origin: DateTime<Utc>,
    instant: Instant,
}

impl TokioClock {
    pub fn new(origin: DateTime<Utc>) -> Self {
        Self {
            origin,
            instant: Instant::now(),
        }
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new(Utc::now())
    }
}

#[async_trait]
impl Clock for TokioClock {
    fn now(&self) -> DateTime<Utc> {
        TimeDelta::from_std(self.instant.elapsed())
            .ok()
            .and_then(|elapsed| self.origin.checked_add_signed(elapsed))
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    }
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}
