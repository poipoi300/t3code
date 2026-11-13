use crate::{Error, Rejection, Result, SharedClock};
use std::{future::Future, time::Duration};

#[derive(Debug, Clone, Copy)]
pub enum RetryMode {
    Registration,
    DesiredLink,
    Recovery,
}

pub struct RetryPolicy {
    pub mode: RetryMode,
    pub continue_registration: bool,
    started: i64,
    attempt: u32,
}
impl RetryPolicy {
    pub fn new(mode: RetryMode, now: i64) -> Self {
        Self {
            mode,
            continue_registration: false,
            started: now,
            attempt: 0,
        }
    }
    /// Jitter is injected in [0, 1]; production uses independent cryptographic randomness.
    pub fn next(&mut self, error: &Error, now: i64, jitter: f64) -> Option<Duration> {
        let Error::Remote {
            rejection: Rejection::Unavailable,
            retry_after_ms,
        } = error
        else {
            return None;
        };
        let nominal =
            1000_u64.saturating_mul(1_u64.checked_shl(self.attempt.min(30)).unwrap_or(u64::MAX));
        let jitter = if jitter.is_finite() {
            jitter.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let delay = ((nominal as f64) * (0.8 + 0.4 * jitter)).min(300_000.0) as u64;
        let delay = delay.max(retry_after_ms.unwrap_or(0));
        let delay = if matches!(self.mode, RetryMode::Recovery) {
            delay.max(120_000)
        } else {
            delay
        };
        if matches!(self.mode, RetryMode::Registration)
            && !self.continue_registration
            && now
                .saturating_sub(self.started)
                .saturating_add(delay as i64)
                >= 600_000
        {
            return None;
        }
        self.attempt = self.attempt.saturating_add(1);
        Some(Duration::from_millis(delay))
    }
}
pub async fn retry<T, F, Fut>(
    clock: SharedClock,
    mut policy: RetryPolicy,
    mut operation: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let mut random = [0; 4];
                getrandom::fill(&mut random).map_err(|_| Error::Proof)?;
                let jitter = f64::from(u32::from_le_bytes(random)) / f64::from(u32::MAX);
                let Some(delay) = policy.next(&error, clock.now_ms(), jitter) else {
                    return Err(error);
                };
                tokio::time::sleep(delay).await;
            }
        }
    }
}
#[derive(Debug)]
pub struct RecoveryCooldown {
    last_finished: Option<i64>,
    cooldown: i64,
}
impl Default for RecoveryCooldown {
    fn default() -> Self {
        Self {
            last_finished: None,
            cooldown: 120_000,
        }
    }
}
impl RecoveryCooldown {
    pub fn wait_ms(&mut self, now: i64) -> u64 {
        let Some(last) = self.last_finished else {
            return 0;
        };
        let elapsed = now.saturating_sub(last);
        self.cooldown = if elapsed >= self.cooldown.saturating_add(600_000) {
            120_000
        } else {
            (self.cooldown * 2).min(1_800_000)
        };
        self.cooldown.saturating_sub(elapsed).max(0) as u64
    }
    pub fn finished(&mut self, now: i64) {
        self.last_finished = Some(now);
    }
}
