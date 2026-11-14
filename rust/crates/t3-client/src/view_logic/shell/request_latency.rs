//! Requests the server has not acknowledged in time (`rpc/requestLatencyState.ts`).
//!
//! The host reports each request sent and acknowledged, and calls [`RequestLatency::tick`] at
//! [`RequestLatency::next_deadline`] instead of a timer per request; the slow list is what the
//! "server is slow" indicator shows.
use std::collections::BTreeMap;

pub const SLOW_RPC_ACK_THRESHOLD_MS: u64 = 15_000;
/// Updates wait for package installs; VCS refreshes may wait for background work and network.
pub const LONG_RUNNING_RPC_ACK_THRESHOLD_MS: u64 = 120_000;
pub const MAX_TRACKED_RPC_ACK_REQUESTS: usize = 256;

/// Background reports nobody waits on, and a sign-in wait that lasts as long as the user takes.
const UNTRACKED: &[&str] = &[
    "server.getUsageSummary",
    "server.reportClientActivity",
    "sourceControl.signInWait",
];
const LONG_RUNNING: &[&str] = &[
    "server.updateProvider",
    "server.refreshProviders",
    "server.updateServer",
    "vcs.refreshStatus",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlowRequest {
    pub request_id: String,
    pub started_at: String,
    pub started_at_ms: u64,
    pub tag: String,
    pub threshold_ms: u64,
}

#[derive(Debug, Clone)]
pub struct RequestLatency {
    threshold_ms: u64,
    /// Pending requests in send order (`order` is the insertion counter, as a JS `Map` keeps).
    pending: BTreeMap<String, (u64, SlowRequest)>,
    order: u64,
    slow: Vec<SlowRequest>,
}

impl Default for RequestLatency {
    fn default() -> Self {
        Self::new(SLOW_RPC_ACK_THRESHOLD_MS)
    }
}

impl RequestLatency {
    pub fn new(threshold_ms: u64) -> Self {
        Self {
            threshold_ms,
            pending: BTreeMap::new(),
            order: 0,
            slow: Vec::new(),
        }
    }

    pub fn tracks(method: &str) -> bool {
        !method.contains("subscribe")
            && !method.starts_with("pullRequests.")
            && !UNTRACKED.contains(&method)
    }

    fn threshold(&self, method: &str) -> u64 {
        if LONG_RUNNING.contains(&method) {
            self.threshold_ms.max(LONG_RUNNING_RPC_ACK_THRESHOLD_MS)
        } else {
            self.threshold_ms
        }
    }

    pub fn sent(&mut self, request_id: &str, method: &str, tag: &str, now_epoch_ms: u64) {
        if !Self::tracks(method) {
            return;
        }
        self.pending.remove(request_id);
        while self.pending.len() >= MAX_TRACKED_RPC_ACK_REQUESTS {
            let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, (order, _))| *order)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.pending.remove(&oldest);
        }
        self.order += 1;
        self.pending.insert(
            request_id.to_owned(),
            (
                self.order,
                SlowRequest {
                    request_id: request_id.to_owned(),
                    started_at: crate::time::iso_from_epoch_ms(now_epoch_ms),
                    started_at_ms: now_epoch_ms,
                    tag: tag.to_owned(),
                    threshold_ms: self.threshold(method),
                },
            ),
        );
    }

    /// Returns whether the slow list changed.
    pub fn acknowledged(&mut self, request_id: &str) -> bool {
        self.pending.remove(request_id);
        let before = self.slow.len();
        self.slow.retain(|request| request.request_id != request_id);
        self.slow.len() != before
    }

    /// Moves every request past its threshold to the slow list, in the order their thresholds
    /// passed; returns whether the slow list changed.
    pub fn tick(&mut self, now_epoch_ms: u64) -> bool {
        let mut due: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, (_, request))| {
                request.started_at_ms.saturating_add(request.threshold_ms) <= now_epoch_ms
            })
            .map(|(id, (order, request))| {
                (
                    request.started_at_ms + request.threshold_ms,
                    *order,
                    id.clone(),
                )
            })
            .collect();
        due.sort();
        for (_, _, id) in &due {
            if let Some((_, request)) = self.pending.remove(id) {
                self.slow.push(request);
            }
        }
        if self.slow.len() > MAX_TRACKED_RPC_ACK_REQUESTS {
            self.slow
                .drain(..self.slow.len() - MAX_TRACKED_RPC_ACK_REQUESTS);
        }
        !due.is_empty()
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.pending
            .values()
            .map(|(_, request)| request.started_at_ms + request.threshold_ms)
            .min()
    }

    pub fn slow(&self) -> &[SlowRequest] {
        &self.slow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_requests_appear_after_their_threshold_and_clear_on_acknowledgement() {
        let mut latency = RequestLatency::default();
        latency.sent("1", "orchestration.dispatchCommand", "dispatch", 1_000);
        latency.sent("2", "server.updateProvider", "update", 1_000);
        latency.sent("3", "orchestration.subscribeShell", "shell", 1_000);
        latency.sent("4", "pullRequests.list", "list", 1_000);
        latency.sent("5", "server.getUsageSummary", "usage", 1_000);
        assert_eq!(latency.next_deadline(), Some(16_000));
        assert!(!latency.tick(15_999));
        assert!(latency.tick(16_000));
        assert_eq!(latency.slow().len(), 1);
        assert_eq!(latency.slow()[0].started_at, "1970-01-01T00:00:01.000Z");
        assert_eq!(
            latency.next_deadline(),
            Some(121_000),
            "long-running threshold"
        );
        assert!(latency.acknowledged("1"));
        assert!(latency.slow().is_empty());
        assert!(!latency.acknowledged("2"), "pending, not slow");
        assert!(!latency.tick(200_000));
    }

    #[test]
    fn pending_and_slow_lists_are_bounded_oldest_first() {
        let mut latency = RequestLatency::new(10);
        for index in 0..=MAX_TRACKED_RPC_ACK_REQUESTS {
            latency.sent(&index.to_string(), "m", "m", 0);
        }
        latency.tick(10);
        assert_eq!(latency.slow().len(), MAX_TRACKED_RPC_ACK_REQUESTS);
        assert_eq!(
            latency.slow()[0].request_id,
            "1",
            "the oldest pending was evicted"
        );
        // Re-sending an id restarts it.
        latency.sent("x", "m", "m", 100);
        latency.sent("x", "m", "m", 105);
        assert_eq!(latency.next_deadline(), Some(115));
    }
}
