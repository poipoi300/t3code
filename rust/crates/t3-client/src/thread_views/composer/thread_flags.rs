//! Per-thread composer flags the web keeps in memory for the app's lifetime: the armed fresh
//! session (`apps/web/src/components/chat/freshSessionArms.ts`) and "keep full history" for the
//! next send (ChatView.tsx `fullHistoryThreadKeys`). Neither is persisted, as on the web.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use futures::stream::LocalBoxStream;

use super::send::Changes;
use crate::types::ThreadKey;

#[derive(Default)]
struct State {
    arms: BTreeMap<ThreadKey, u64>,
    next_arm: u64,
    full_history: BTreeSet<ThreadKey>,
    resuming: BTreeSet<ThreadKey>,
}

#[derive(Default)]
pub struct ComposerThreadFlags {
    state: RefCell<State>,
    changes: Changes,
}

/// `isClearComposerCommand` (packages/shared/src/composerTrigger.ts): `/clear` as the whole message.
pub fn is_clear_composer_command(text: &str) -> bool {
    let text = crate::view_logic::JsTrim::js_trim(text);
    text.get(..6).is_some_and(|word| word.eq_ignore_ascii_case("/clear"))
        && text[6..].chars().all(crate::view_logic::is_js_whitespace)
}

impl ComposerThreadFlags {
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.changes.subscribe()
    }

    /// Arms the thread's next message to start a fresh provider session; returns the arm.
    pub fn arm_fresh_session(&self, key: &ThreadKey) -> u64 {
        let arm = {
            let mut state = self.state.borrow_mut();
            state.next_arm += 1;
            let arm = state.next_arm;
            state.arms.insert(key.clone(), arm);
            arm
        };
        self.changes.notify();
        arm
    }

    /// With `arm`, clears only that arm, so a send never consumes one made while it was in flight.
    pub fn disarm_fresh_session(&self, key: &ThreadKey, arm: Option<u64>) {
        let removed = {
            let mut state = self.state.borrow_mut();
            match (state.arms.get(key), arm) {
                (Some(current), Some(arm)) if *current != arm => false,
                (Some(_), _) => state.arms.remove(key).is_some(),
                (None, _) => false,
            }
        };
        if removed {
            self.changes.notify();
        }
    }

    pub fn fresh_session_arm(&self, key: &ThreadKey) -> Option<u64> {
        self.state.borrow().arms.get(key).copied()
    }

    /// `useComposerResume`'s per-route lock; false when a resume is already running.
    pub(crate) fn begin_resume(&self, key: &ThreadKey) -> bool {
        let began = self.state.borrow_mut().resuming.insert(key.clone());
        if began {
            self.changes.notify();
        }
        began
    }

    pub(crate) fn end_resume(&self, key: &ThreadKey) {
        if self.state.borrow_mut().resuming.remove(key) {
            self.changes.notify();
        }
    }

    pub fn is_resuming(&self, key: &ThreadKey) -> bool {
        self.state.borrow().resuming.contains(key)
    }

    pub fn keep_full_history(&self, key: &ThreadKey) -> bool {
        self.state.borrow().full_history.contains(key)
    }

    pub fn set_keep_full_history(&self, key: &ThreadKey, keep: bool) {
        let changed = {
            let mut state = self.state.borrow_mut();
            if keep {
                state.full_history.insert(key.clone())
            } else {
                state.full_history.remove(key)
            }
        };
        if changed {
            self.changes.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EnvironmentId, ThreadId};

    fn key() -> ThreadKey {
        ThreadKey {
            environment: EnvironmentId("env".into()),
            thread: ThreadId("thread".into()),
        }
    }

    #[test]
    fn a_send_clears_only_the_arm_it_read() {
        let flags = ComposerThreadFlags::default();
        let read = flags.arm_fresh_session(&key());
        let rearmed = flags.arm_fresh_session(&key());
        flags.disarm_fresh_session(&key(), Some(read));
        assert_eq!(flags.fresh_session_arm(&key()), Some(rearmed));
        flags.disarm_fresh_session(&key(), Some(rearmed));
        assert_eq!(flags.fresh_session_arm(&key()), None);
    }

    #[test]
    fn clear_is_the_whole_message_in_any_case() {
        assert!(is_clear_composer_command(" /CLEAR \n"));
        assert!(!is_clear_composer_command("/clear now"));
        assert!(!is_clear_composer_command("/clearer"));
    }
}
