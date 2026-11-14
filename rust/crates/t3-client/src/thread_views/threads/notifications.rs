//! When a thread notifies (`ThreadNotificationCoordinator.logic.ts`) and what a screen reader
//! announces for threads in the background (`backgroundThreadAnnouncement.ts`).
//!
//! Time is injected: [`NotificationGate::observe`] returns what to notify now, and the host calls
//! [`NotificationGate::tick`] at [`NotificationGate::next_deadline`] instead of a timer per thread.
use super::activity_announcement::run_outcome_text;
use std::collections::{BTreeMap, BTreeSet};
use t3_contracts::{
    base_schemas::RunId, orchestration_v2::OrchestrationV2RunStatus,
    orchestration_v2_id_table::IdOrIndex,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Input,
    Completion,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationObservation {
    /// Status and request key, or status and run without a request, while it needs attention.
    pub attention: Option<String>,
    /// The pending request's id, when it has one; `None` whenever `attention` is.
    pub request_id: Option<String>,
    /// The thread is idle and its latest run completed.
    pub settled: bool,
    /// When the latest run completed, if it did.
    pub completed_at: Option<i64>,
    /// The latest run answers a background-work progress wake, like a Monitor line.
    pub background_wake: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObserveOptions {
    pub archived: bool,
    pub quiet: bool,
}

/// Matches the push relay's completion confirmation: a final wake can start right after a
/// progress wake.
pub const RELEASED_COMPLETION_GRACE_MS: u64 = 5_000;

#[derive(Debug, Clone)]
pub struct NotificationGate<K> {
    /// The newest completion already notified, or deliberately skipped.
    handled: BTreeMap<K, i64>,
    /// Wake completions waiting out the grace: completion time and due tick (none while paused).
    releases: BTreeMap<K, (i64, Option<u64>)>,
    /// Attention keys with a request id seen since the thread last stopped needing attention.
    attended: BTreeMap<K, BTreeSet<String>>,
}

impl<K> Default for NotificationGate<K> {
    fn default() -> Self {
        Self {
            handled: BTreeMap::new(),
            releases: BTreeMap::new(),
            attended: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Clone> NotificationGate<K> {
    fn release(&mut self, key: &K, at: i64, now: u64) {
        self.releases
            .insert(key.clone(), (at, Some(now + RELEASED_COMPLETION_GRACE_MS)));
    }
    fn skip(&mut self, key: &K, at: Option<i64>) {
        if let Some(at) = at
            && self.handled.get(key).is_none_or(|handled| at > *handled)
        {
            self.handled.insert(key.clone(), at);
        }
    }
    fn attend(
        &mut self,
        key: &K,
        prior: Option<&NotificationObservation>,
        next: &NotificationObservation,
    ) -> bool {
        let Some(attention) = &next.attention else {
            self.attended.remove(key);
            return false;
        };
        if next.request_id.is_none() {
            return prior.is_none_or(|prior| prior.attention.as_ref() != Some(attention));
        }
        self.attended
            .entry(key.clone())
            .or_default()
            .insert(attention.clone())
    }

    /// Feeds one thread's new observation; returns the notification to show now, if any.
    pub fn observe(
        &mut self,
        key: &K,
        prior: Option<&NotificationObservation>,
        next: &NotificationObservation,
        options: ObserveOptions,
        now: u64,
    ) -> Option<NotificationKind> {
        // The grace survives updates that keep the wake's completion; anything else cancels it.
        if let Some(&(at, due)) = self.releases.get(key)
            && Some(at) == next.completed_at
            && !options.archived
            && !options.quiet
        {
            if due.is_none() {
                self.release(key, at, now);
            }
            return None;
        }
        self.releases.remove(key);
        // The first sighting, archived threads, and stopped or silent runs never notify
        // completion.
        if prior.is_none() || options.archived || options.quiet {
            self.skip(key, next.completed_at);
        }
        let fresh = self.attend(key, prior, next);
        if prior.is_none() || options.archived {
            return None;
        }
        if fresh {
            return Some(NotificationKind::Input);
        }
        let at = next.completed_at?;
        if !next.settled || self.handled.get(key).is_some_and(|handled| at <= *handled) {
            return None;
        }
        if next.background_wake {
            self.release(key, at, now);
            return None;
        }
        self.handled.insert(key.clone(), at);
        Some(NotificationKind::Completion)
    }

    /// Completions whose grace ended by `now`, to notify.
    pub fn tick(&mut self, now: u64) -> Vec<K> {
        let due: Vec<_> = self
            .releases
            .iter()
            .filter(|(_, (_, due))| due.is_some_and(|due| due <= now))
            .map(|(key, (at, _))| (key.clone(), *at))
            .collect();
        for (key, at) in &due {
            self.releases.remove(key);
            self.handled.insert(key.clone(), *at);
        }
        due.into_iter().map(|(key, _)| key).collect()
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.releases.values().filter_map(|(_, due)| *due).min()
    }

    /// Stops grace timers while the snapshot is stale; the next observation restarts or drops
    /// them.
    pub fn pause(&mut self) {
        for (_, due) in self.releases.values_mut() {
            *due = None;
        }
    }

    /// Forgets threads that left the snapshot.
    pub fn retain(&mut self, keys: &BTreeSet<K>) {
        self.handled.retain(|key, _| keys.contains(key));
        self.releases.retain(|key, _| keys.contains(key));
        self.attended.retain(|key, _| keys.contains(key));
    }

    pub fn clear(&mut self) {
        self.handled.clear();
        self.releases.clear();
        self.attended.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundRequest {
    Approval,
    Input,
}

impl BackgroundRequest {
    /// `THREAD_STATUS_TEXT` in `threadActivityAnnouncement.ts`.
    fn text(self) -> &'static str {
        match self {
            Self::Approval => "Needs approval",
            Self::Input => "Needs your input",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundThread {
    pub key: String,
    pub title: String,
    pub request: Option<BackgroundRequest>,
    /// `None` for a subagent's request, which the parent's shell only knows by kind.
    pub request_id: Option<String>,
    pub run_id: Option<IdOrIndex<RunId>>,
    pub run_status: Option<OrchestrationV2RunStatus>,
    /// The run doing work now, which can be older than a queued run cancelled while it ran.
    pub active_run_id: Option<IdOrIndex<RunId>>,
    /// A snoozed thread stays hidden until its timer, so it stays quiet too.
    pub snoozed: bool,
}

/// The request and its id come from one source, so a subagent's request never takes the
/// parent's id.
pub fn resolve_background_request(
    has_pending_approvals: bool,
    has_pending_user_input: bool,
    pending_subagent_request: Option<crate::services::models::PendingSubagentRequest>,
    own_request_id: Option<&str>,
) -> (Option<BackgroundRequest>, Option<String>) {
    use crate::services::models::PendingSubagentRequest as Subagent;
    let own = own_request_id.map(str::to_owned);
    if has_pending_approvals {
        (Some(BackgroundRequest::Approval), own)
    } else if pending_subagent_request == Some(Subagent::Approval) {
        (Some(BackgroundRequest::Approval), None)
    } else if has_pending_user_input {
        (Some(BackgroundRequest::Input), own)
    } else if pending_subagent_request == Some(Subagent::UserInput) {
        (Some(BackgroundRequest::Input), None)
    } else {
        (None, None)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundMemoryEntry {
    request: Option<BackgroundRequest>,
    request_id: Option<String>,
    /// The last run whose ending is known, so each run's ending is announced once.
    settled_run_id: Option<IdOrIndex<RunId>>,
}

pub type BackgroundMemory = BTreeMap<String, BackgroundMemoryEntry>;

/// Only live environments supply threads. First sightings establish a silent baseline, so
/// opening the app or reconnecting does not read out every thread.
pub fn next_background_announcement(
    previous: &BackgroundMemory,
    threads: &[BackgroundThread],
    open_thread: Option<&str>,
) -> (BackgroundMemory, Option<String>) {
    let mut memory = BackgroundMemory::new();
    let mut announcements = Vec::new();
    for thread in threads {
        let before = previous.get(&thread.key);
        let outcome = run_outcome_text(thread.run_status.as_ref());
        let settled =
            outcome.is_some() || thread.run_status == Some(OrchestrationV2RunStatus::RolledBack);
        memory.insert(
            thread.key.clone(),
            BackgroundMemoryEntry {
                request: thread.request,
                request_id: thread.request_id.clone(),
                settled_run_id: if settled {
                    thread.run_id.clone()
                } else {
                    before.and_then(|before| before.settled_run_id.clone())
                },
            },
        );
        let Some(before) = before else {
            continue;
        };
        if Some(thread.key.as_str()) == open_thread || thread.snoozed {
            continue;
        }
        if let Some(request) = thread.request
            && (Some(request) != before.request || thread.request_id != before.request_id)
        {
            announcements.push(format!("{}: {}", request.text(), thread.title));
        }
        // While another run still works, the latest run's ending (a queued message cancelled
        // before it started) is not the thread finishing.
        let another_run_working = thread
            .active_run_id
            .as_ref()
            .is_some_and(|active| Some(active) != thread.run_id.as_ref());
        if let Some(outcome) = outcome
            && thread.run_id != before.settled_run_id
            && !another_run_working
        {
            announcements.push(format!("{outcome}: {}", thread.title));
        }
    }
    let announcement = (!announcements.is_empty()).then(|| announcements.join(". "));
    (memory, announcement)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working() -> NotificationObservation {
        NotificationObservation::default()
    }
    fn completed(at: i64) -> NotificationObservation {
        NotificationObservation {
            settled: true,
            completed_at: Some(at),
            ..Default::default()
        }
    }
    fn waking() -> NotificationObservation {
        NotificationObservation {
            background_wake: true,
            ..working()
        }
    }
    fn woke(at: i64) -> NotificationObservation {
        NotificationObservation {
            background_wake: true,
            ..completed(at)
        }
    }
    fn attention(key: &str, request: Option<&str>) -> NotificationObservation {
        NotificationObservation {
            attention: Some(key.into()),
            request_id: request.map(str::to_owned),
            ..working()
        }
    }
    const VISIBLE: ObserveOptions = ObserveOptions {
        archived: false,
        quiet: false,
    };

    /// The TypeScript test harness: one thread, the previous observation as `prior`.
    #[derive(Default)]
    struct Harness {
        gate: NotificationGate<&'static str>,
        prior: Option<NotificationObservation>,
        now: u64,
        notified: Vec<NotificationKind>,
    }
    impl Harness {
        fn observe(&mut self, next: NotificationObservation) {
            self.observe_with(next, VISIBLE);
        }
        fn observe_with(&mut self, next: NotificationObservation, options: ObserveOptions) {
            if let Some(kind) =
                self.gate
                    .observe(&"thread", self.prior.as_ref(), &next, options, self.now)
            {
                self.notified.push(kind);
            }
            self.prior = Some(next);
        }
        fn advance(&mut self, milliseconds: u64) {
            self.now += milliseconds;
            for _ in self.gate.tick(self.now) {
                self.notified.push(NotificationKind::Completion);
            }
        }
    }
    use NotificationKind::{Completion, Input};

    #[test]
    fn completions_notify_once_each_and_newer_ones_notify_again() {
        let mut h = Harness::default();
        h.observe(working());
        h.observe(completed(1));
        assert_eq!(h.notified, [Completion]);
        h.observe(completed(1));
        h.observe(working());
        h.observe(completed(1));
        assert_eq!(h.notified.len(), 1);
        h.observe(working());
        h.observe(completed(2));
        h.observe(completed(1));
        assert_eq!(h.notified, [Completion, Completion]);
    }

    #[test]
    fn attention_notifies_once_per_stretch_and_the_completion_still_follows() {
        let mut h = Harness::default();
        h.observe(working());
        let a = attention("run-1:approval:a", Some("a"));
        h.observe(a.clone());
        assert_eq!(h.notified, [Input]);
        h.observe(a.clone());
        assert_eq!(h.notified.len(), 1);
        h.observe(attention("run-1:approval:b", Some("b")));
        h.observe(a.clone());
        assert_eq!(h.notified.len(), 2);
        h.observe(working());
        h.observe(a.clone());
        assert_eq!(h.notified.len(), 3);
        let subagent = attention("run-1:approval:", None);
        h.observe(subagent.clone());
        h.observe(a);
        h.observe(subagent);
        assert_eq!(h.notified.len(), 5);
        h.observe(working());
        h.observe(completed(1));
        assert_eq!(h.notified, [Input, Input, Input, Input, Input, Completion]);
    }

    #[test]
    fn a_completion_is_not_replayed_after_attention_resolves() {
        let mut h = Harness::default();
        h.observe(working());
        h.observe(completed(1));
        h.observe(attention("run-1:limited", None));
        h.observe(completed(1));
        assert_eq!(h.notified, [Completion, Input]);
    }

    #[test]
    fn existing_archived_and_stopped_completions_never_notify() {
        let mut h = Harness::default();
        h.observe(completed(1));
        h.observe(working());
        h.observe_with(
            completed(2),
            ObserveOptions {
                quiet: true,
                ..VISIBLE
            },
        );
        h.observe(completed(2));
        h.observe(working());
        h.observe_with(
            completed(3),
            ObserveOptions {
                archived: true,
                ..VISIBLE
            },
        );
        h.observe(completed(3));
        assert!(h.notified.is_empty());
        h.observe(working());
        h.observe(completed(4));
        assert_eq!(h.notified, [Completion]);
    }

    #[test]
    fn removal_or_clearing_forgets_handled_completions_and_pending_wakes() {
        for clear in [false, true] {
            let mut h = Harness::default();
            h.observe(completed(1));
            if clear {
                h.gate.clear();
            } else {
                h.gate.retain(&BTreeSet::new());
            }
            assert_eq!(
                h.gate.observe(&"thread", None, &working(), VISIBLE, 0),
                None
            );
            assert_eq!(
                h.gate
                    .observe(&"thread", Some(&working()), &completed(1), VISIBLE, 0),
                Some(Completion)
            );
            let mut h = Harness::default();
            h.observe(waking());
            h.observe(woke(1));
            if clear {
                h.gate.clear();
            } else {
                h.gate.retain(&BTreeSet::new());
            }
            h.advance(RELEASED_COMPLETION_GRACE_MS);
            assert!(h.notified.is_empty());
        }
    }

    #[test]
    fn a_wake_completion_waits_out_the_grace_and_a_following_run_replaces_it() {
        let mut h = Harness::default();
        h.observe(working());
        h.observe(completed(1));
        h.observe(waking());
        h.observe(woke(2));
        assert_eq!(h.notified.len(), 1);
        h.observe(working());
        h.advance(RELEASED_COMPLETION_GRACE_MS);
        assert_eq!(h.notified.len(), 1);
        h.observe(completed(3));
        h.advance(RELEASED_COMPLETION_GRACE_MS);
        assert_eq!(h.notified, [Completion, Completion]);
    }

    #[test]
    fn a_wake_completion_notifies_once_after_the_grace_even_across_a_pause() {
        let mut h = Harness::default();
        h.observe(waking());
        h.observe(woke(1));
        h.gate.pause();
        assert_eq!(h.gate.next_deadline(), None);
        h.advance(RELEASED_COMPLETION_GRACE_MS);
        assert!(h.notified.is_empty());
        h.observe(woke(1));
        h.advance(RELEASED_COMPLETION_GRACE_MS - 1);
        h.observe(woke(1));
        assert!(h.notified.is_empty());
        h.advance(1);
        assert_eq!(h.notified, [Completion]);
        h.observe(woke(1));
        h.advance(RELEASED_COMPLETION_GRACE_MS);
        assert_eq!(h.notified.len(), 1);
    }

    fn run(id: &str) -> Option<IdOrIndex<RunId>> {
        Some(IdOrIndex::Id(RunId(id.into())))
    }
    fn thread(
        request: Option<BackgroundRequest>,
        run_id: &str,
        status: Option<OrchestrationV2RunStatus>,
    ) -> BackgroundThread {
        BackgroundThread {
            key: "env:t".into(),
            title: "Fix bug".into(),
            request,
            request_id: request.map(|_| "r1".into()),
            run_id: run(run_id),
            run_status: status,
            active_run_id: None,
            snoozed: false,
        }
    }

    #[test]
    fn background_threads_baseline_silently_then_announce_requests_and_endings_once() {
        use OrchestrationV2RunStatus::*;
        let (memory, said) = next_background_announcement(
            &BTreeMap::new(),
            &[thread(None, "1", Some(Running))],
            None,
        );
        assert_eq!(said, None, "first sighting");
        let (memory, said) = next_background_announcement(
            &memory,
            &[thread(
                Some(BackgroundRequest::Approval),
                "1",
                Some(Running),
            )],
            None,
        );
        assert_eq!(said.as_deref(), Some("Needs approval: Fix bug"));
        let (memory, said) =
            next_background_announcement(&memory, &[thread(None, "1", Some(Completed))], None);
        assert_eq!(said.as_deref(), Some("Done: Fix bug"));
        let (memory, said) =
            next_background_announcement(&memory, &[thread(None, "1", Some(Completed))], None);
        assert_eq!(said, None, "each ending once");
        // The open thread and a snoozed thread stay quiet.
        let (memory, said) = next_background_announcement(
            &memory,
            &[thread(None, "2", Some(Failed))],
            Some("env:t"),
        );
        assert_eq!(said, None);
        let mut cancelled = thread(None, "3", Some(Cancelled));
        cancelled.active_run_id = run("4");
        let (_, said) = next_background_announcement(&memory, &[cancelled], None);
        assert_eq!(said, None, "another run still works");
    }

    #[test]
    fn a_subagent_request_never_takes_the_parents_id() {
        use crate::services::models::PendingSubagentRequest as Subagent;
        assert_eq!(
            resolve_background_request(false, false, Some(Subagent::Approval), Some("own")),
            (Some(BackgroundRequest::Approval), None)
        );
        assert_eq!(
            resolve_background_request(true, false, Some(Subagent::Approval), Some("own")),
            (Some(BackgroundRequest::Approval), Some("own".into()))
        );
        assert_eq!(
            resolve_background_request(false, true, None, Some("own")),
            (Some(BackgroundRequest::Input), Some("own".into()))
        );
        assert_eq!(
            resolve_background_request(false, false, None, Some("own")),
            (None, None)
        );
    }
}
