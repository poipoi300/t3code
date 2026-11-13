//! Visit tracking emits effects; the caller schedules the returned deadline.
use crate::types::ThreadKey;
use std::collections::BTreeMap;
pub const VISIT_DISPATCH_THROTTLE_MS: u64 = 10_000;
pub const THREAD_OPEN_DEDUPE_MS: u64 = 600_000;
pub struct VisitedShell<'a> {
    pub updated_at: Option<i64>,
    pub last_visited_at: Option<Option<i64>>,
    pub completed_at: Option<i64>,
    pub woke_at: Option<i64>,
    pub timestamp: &'a str,
    pub wake_timestamp: Option<&'a str>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisitEffect {
    Local { visited_at: String },
    Dispatch { visited_at: String, opened: bool },
}
#[derive(Debug, Clone)]
struct Pending {
    at: u64,
    millis: i64,
    timestamp: String,
}
#[derive(Debug, Clone)]
pub struct VisitTracker {
    open_pending: bool,
    last_dispatched: Option<i64>,
    last_dispatch_at: u64,
    pending: Option<Pending>,
}
impl VisitTracker {
    pub fn new(open_pending: bool) -> Self {
        Self {
            open_pending,
            last_dispatched: None,
            last_dispatch_at: 0,
            pending: None,
        }
    }
    pub fn deadline(&self) -> Option<u64> {
        self.pending.as_ref().map(|p| p.at)
    }
    fn dispatch(&mut self, millis: i64, timestamp: String, opened: bool, now: u64) -> VisitEffect {
        self.pending = None;
        self.last_dispatched = Some(millis);
        self.last_dispatch_at = now;
        VisitEffect::Dispatch {
            visited_at: timestamp,
            opened,
        }
    }
    pub fn observe(&mut self, shell: Option<VisitedShell<'_>>, now: u64) -> Option<VisitEffect> {
        self.pending = None;
        let shell = shell?;
        let updated = shell.updated_at?;
        let wake_is_latest = shell.woke_at.is_some_and(|w| w > updated);
        let (millis, timestamp) = if wake_is_latest {
            (shell.woke_at?, shell.wake_timestamp?)
        } else {
            (updated, shell.timestamp)
        };
        let Some(visited) = shell.last_visited_at else {
            return Some(VisitEffect::Local {
                visited_at: timestamp.into(),
            });
        };
        if self.open_pending {
            self.open_pending = false;
            return Some(self.dispatch(millis, timestamp.into(), true, now));
        }
        if visited.is_some_and(|v| v >= millis) {
            if self.last_dispatched.is_none_or(|v| v < millis) {
                self.last_dispatched = Some(millis);
            }
            return None;
        }
        if self.last_dispatched.is_some_and(|v| v >= millis) {
            return None;
        }
        let unseen_completion = shell
            .completed_at
            .is_some_and(|c| visited.is_none_or(|v| c > v));
        if unseen_completion
            || wake_is_latest
            || now.saturating_sub(self.last_dispatch_at) >= VISIT_DISPATCH_THROTTLE_MS
        {
            return Some(self.dispatch(millis, timestamp.into(), false, now));
        }
        self.pending = Some(Pending {
            at: self
                .last_dispatch_at
                .saturating_add(VISIT_DISPATCH_THROTTLE_MS),
            millis,
            timestamp: timestamp.into(),
        });
        None
    }
    pub fn tick(&mut self, now: u64) -> Option<VisitEffect> {
        if !self.pending.as_ref().is_some_and(|p| p.at <= now) {
            return None;
        }
        let pending = self.pending.take()?;
        Some(self.dispatch(pending.millis, pending.timestamp, false, now))
    }
    pub fn unmount(&mut self) {
        self.pending = None;
    }
}
#[derive(Default)]
pub struct ThreadOpens {
    sent: BTreeMap<ThreadKey, u64>,
    pub local: BTreeMap<ThreadKey, u64>,
    pub revision: u64,
}
impl ThreadOpens {
    pub fn claim(&mut self, key: ThreadKey, now: u64) -> bool {
        self.local.insert(key.clone(), now);
        self.revision += 1;
        if self
            .sent
            .get(&key)
            .is_some_and(|last| now.saturating_sub(*last) < THREAD_OPEN_DEDUPE_MS)
        {
            return false;
        }
        self.sent.insert(key, now);
        true
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn opens_are_deduplicated_but_recents_follow_each_open() {
        use crate::types::{EnvironmentId, ThreadId};
        let key = ThreadKey {
            environment: EnvironmentId("e".into()),
            thread: ThreadId("t".into()),
        };
        let mut opens = ThreadOpens::default();
        assert!(opens.claim(key.clone(), 0));
        assert!(!opens.claim(key.clone(), 1));
        assert_eq!(opens.local[&key], 1);
        assert_eq!(opens.revision, 2);
        assert!(opens.claim(key.clone(), THREAD_OPEN_DEDUPE_MS));
        let other = ThreadKey {
            environment: EnvironmentId("other".into()),
            ..key
        };
        assert!(opens.claim(other, 1));
    }
    fn shell(at: i64, visited: Option<i64>) -> VisitedShell<'static> {
        VisitedShell {
            updated_at: Some(at),
            last_visited_at: Some(visited),
            completed_at: None,
            woke_at: None,
            timestamp: match at {
                10 => "10",
                11 => "11",
                12 => "12",
                20 => "20",
                30 => "30",
                _ => "other",
            },
            wake_timestamp: None,
        }
    }
    #[test]
    fn open_and_throttle() {
        let mut t = VisitTracker::new(true);
        assert_eq!(
            t.observe(Some(shell(10, Some(0))), 0),
            Some(VisitEffect::Dispatch {
                visited_at: "10".into(),
                opened: true
            })
        );
        assert!(t.observe(Some(shell(11, Some(0))), 1000).is_none());
        assert!(t.observe(Some(shell(12, Some(0))), 1000).is_none());
        assert_eq!(t.deadline(), Some(10_000));
        assert!(t.tick(9999).is_none());
        assert_eq!(
            t.tick(10_000),
            Some(VisitEffect::Dispatch {
                visited_at: "12".into(),
                opened: false
            })
        );
    }
    #[test]
    fn completion_and_wake_immediate() {
        for wake in [false, true] {
            let mut t = VisitTracker::new(true);
            t.observe(Some(shell(10, Some(0))), 0);
            let mut s = shell(20, Some(0));
            if wake {
                s.woke_at = Some(25);
                s.wake_timestamp = Some("25");
            } else {
                s.completed_at = Some(20);
            }
            assert_eq!(
                t.observe(Some(s), 1),
                Some(VisitEffect::Dispatch {
                    visited_at: if wake { "25" } else { "20" }.into(),
                    opened: false
                })
            );
        }
    }
    #[test]
    fn mark_unread_stays_sticky() {
        let mut t = VisitTracker::new(true);
        t.observe(Some(shell(10, Some(0))), 0);
        t.observe(Some(shell(10, Some(10))), 1);
        t.observe(Some(shell(10, Some(5))), 2);
        assert!(t.tick(10_000).is_none());
        assert!(t.observe(Some(shell(30, Some(5))), 10_000).is_some());
    }
    #[test]
    fn reopened_read_snapshot_rewind_and_retired_wake() {
        let mut t = VisitTracker::new(false);
        t.observe(Some(shell(10, Some(10))), 0);
        t.observe(Some(shell(10, Some(8))), 1);
        t.observe(Some(shell(5, Some(5))), 2);
        assert!(t.observe(Some(shell(10, Some(8))), 3).is_none());
        let mut t = VisitTracker::new(true);
        let mut s = shell(10, Some(0));
        s.woke_at = Some(15);
        s.wake_timestamp = Some("15");
        t.observe(Some(s), 0);
        assert!(t.observe(Some(shell(10, Some(8))), 100).is_none());
    }
    #[test]
    fn local_missing_shell_and_unmount() {
        let mut t = VisitTracker::new(true);
        assert!(t.observe(None, 0).is_none());
        let mut s = shell(10, None);
        s.last_visited_at = None;
        assert_eq!(
            t.observe(Some(s), 1),
            Some(VisitEffect::Local {
                visited_at: "10".into()
            })
        );
        assert_eq!(
            t.observe(Some(shell(10, None)), 2),
            Some(VisitEffect::Dispatch {
                visited_at: "10".into(),
                opened: true
            })
        );
        t.observe(Some(shell(11, None)), 3);
        t.unmount();
        assert!(t.tick(10_002).is_none());
    }
}
