//! What the window shows for the supervisor's events: the O08 startup states.

use t3_client::view_logic::shell::startup_recovery::{
    StartupEvent, StartupFailure, StartupRecovery,
};

use crate::SupervisorEvent;

/// The state the shell publishes (`LocalBackendStatus` in t3-ui, minus its window chrome).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendView {
    /// First launch, before the server answers.
    Starting,
    /// The server exited; restart `attempt` is scheduled and no failure is reported yet.
    Restarting { attempt: u32 },
    /// Answering: nothing is shown.
    Ready,
    /// The recovery window reports this.
    Failed(StartupFailure),
    /// Another server owns the T3 home; nothing is retried. Carries `homeOwnedMessage`.
    HomeOwned(String),
}

/// Folds supervisor events into views with `StartupRecovery`'s rules: the first failure of a run
/// of failures stays, `Ready` clears it, and a failure never covers a main window that is already
/// visible after the server had started.
#[derive(Debug, Default)]
pub struct ViewTracker {
    recovery: StartupRecovery,
    started: bool,
}

impl ViewTracker {
    /// The new view, or `None` when the event changes nothing the user sees. `main_visible` is
    /// whether the main window is on screen now.
    pub fn apply(&mut self, event: &SupervisorEvent, main_visible: bool) -> Option<BackendView> {
        match event {
            SupervisorEvent::Starting => Some(BackendView::Starting),
            SupervisorEvent::Spawned { .. } | SupervisorEvent::Stopped => None,
            SupervisorEvent::Ready { .. } => {
                self.started = true;
                self.recovery.on_event(StartupEvent::Ready, main_visible);
                Some(BackendView::Ready)
            }
            SupervisorEvent::Restarting { attempt, .. } => {
                // A reported failure keeps its window while restarts continue behind it, and a
                // server that dies mid-session never covers the live main window.
                (self.recovery.failure().is_none() && !(self.started && main_visible))
                    .then_some(BackendView::Restarting { attempt: *attempt })
            }
            SupervisorEvent::Failing(failure) => {
                let effects = self
                    .recovery
                    .on_event(StartupEvent::Failing(failure.clone()), main_visible);
                effects
                    .show_window
                    .then(|| BackendView::Failed(failure.clone()))
            }
            SupervisorEvent::HomeOwned { message } => Some(BackendView::HomeOwned(message.clone())),
        }
    }

    /// macOS `activate`: the failure to show again, if one is recorded.
    pub fn on_activate(&self, main_visible: bool) -> Option<BackendView> {
        self.recovery
            .on_activate(main_visible)
            .then(|| self.recovery.failure().cloned().map(BackendView::Failed))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn restarting(attempt: u32) -> SupervisorEvent {
        SupervisorEvent::Restarting {
            attempt,
            delay: Duration::ZERO,
            reason: "code=3".into(),
        }
    }

    fn failing(reason: &str) -> SupervisorEvent {
        SupervisorEvent::Failing(StartupFailure::new(reason, false, "out"))
    }

    #[test]
    fn starting_then_restarting_then_the_first_reported_failure() {
        let mut tracker = ViewTracker::default();
        assert_eq!(
            tracker.apply(&SupervisorEvent::Starting, false),
            Some(BackendView::Starting)
        );
        assert_eq!(
            tracker.apply(&restarting(1), false),
            Some(BackendView::Restarting { attempt: 1 })
        );
        let first = tracker.apply(&failing("first"), false);
        assert!(matches!(first, Some(BackendView::Failed(f)) if f.reason == "first"));
        // The window keeps the first failure and is not replaced by status lines or later ones.
        assert_eq!(tracker.apply(&restarting(4), false), None);
        assert_eq!(tracker.apply(&failing("second"), false), None);
        assert_eq!(
            tracker.on_activate(false),
            Some(BackendView::Failed(StartupFailure::new(
                "first", false, "out"
            )))
        );
    }

    #[test]
    fn ready_clears_the_failure() {
        let mut tracker = ViewTracker::default();
        tracker.apply(&failing("first"), false);
        assert_eq!(
            tracker.apply(&SupervisorEvent::Ready { pid: 1 }, true),
            Some(BackendView::Ready)
        );
        assert_eq!(tracker.on_activate(false), None);
        assert_eq!(tracker.apply(&restarting(1), true), None);
    }

    #[test]
    fn a_failure_after_a_start_never_covers_the_visible_main_window() {
        let mut tracker = ViewTracker::default();
        tracker.apply(&SupervisorEvent::Ready { pid: 1 }, true);
        assert_eq!(tracker.apply(&failing("died"), true), None);
        assert_eq!(tracker.apply(&restarting(1), true), None);
    }

    #[test]
    fn an_owned_state_directory_is_shown_as_a_failure() {
        let mut tracker = ViewTracker::default();
        let event = SupervisorEvent::HomeOwned {
            message: "owned by 42".into(),
        };
        assert_eq!(
            tracker.apply(&event, true),
            Some(BackendView::HomeOwned("owned by 42".into()))
        );
    }
}
