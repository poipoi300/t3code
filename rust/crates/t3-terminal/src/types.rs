//! Terminal wire payloads are owned by t3-contracts.
pub use t3_contracts::terminal::*;
pub type Environment = TerminalEnvironment;
#[derive(Clone, Debug)]
pub(crate) struct EventIdentity {
    pub thread_id: String,
    pub terminal_id: String,
    pub sequence: Option<Option<u64>>,
}
pub(crate) trait EventExt {
    fn identity(&self) -> EventIdentity;
    fn attached(&self) -> TerminalAttachStreamEvent;
}
impl EventExt for TerminalEvent {
    fn identity(&self) -> EventIdentity {
        match self {
            TerminalEvent::TerminalStartedEvent(e) => EventIdentity {
                thread_id: e.thread_id.clone(),
                terminal_id: e.terminal_id.clone(),
                sequence: e.sequence,
            },
            TerminalEvent::TerminalOutputEvent(e) => EventIdentity {
                thread_id: e.thread_id.clone(),
                terminal_id: e.terminal_id.clone(),
                sequence: e.sequence,
            },
            TerminalEvent::TerminalExitedEvent(e) => EventIdentity {
                thread_id: e.thread_id.clone(),
                terminal_id: e.terminal_id.clone(),
                sequence: e.sequence,
            },
            TerminalEvent::TerminalClosedEvent(e) => EventIdentity {
                thread_id: e.thread_id.clone(),
                terminal_id: e.terminal_id.clone(),
                sequence: e.sequence,
            },
            TerminalEvent::TerminalErrorEvent(e) => EventIdentity {
                thread_id: e.thread_id.clone(),
                terminal_id: e.terminal_id.clone(),
                sequence: e.sequence,
            },
            TerminalEvent::TerminalClearedEvent(e) => EventIdentity {
                thread_id: e.thread_id.clone(),
                terminal_id: e.terminal_id.clone(),
                sequence: e.sequence,
            },
            TerminalEvent::TerminalRestartedEvent(e) => EventIdentity {
                thread_id: e.thread_id.clone(),
                terminal_id: e.terminal_id.clone(),
                sequence: e.sequence,
            },
            TerminalEvent::TerminalActivityEvent(e) => EventIdentity {
                thread_id: e.thread_id.clone(),
                terminal_id: e.terminal_id.clone(),
                sequence: e.sequence,
            },
        }
    }
    fn attached(&self) -> TerminalAttachStreamEvent {
        match self {
            TerminalEvent::TerminalStartedEvent(e) => {
                TerminalAttachStreamEvent::TerminalAttachSnapshotEvent(
                    TerminalAttachSnapshotEvent {
                        r#type: TerminalAttachSnapshotEventType::Snapshot,
                        snapshot: e.snapshot.clone(),
                    },
                )
            }
            TerminalEvent::TerminalOutputEvent(e) => {
                TerminalAttachStreamEvent::TerminalOutputEvent(e.clone())
            }
            TerminalEvent::TerminalExitedEvent(e) => {
                TerminalAttachStreamEvent::TerminalExitedEvent(e.clone())
            }
            TerminalEvent::TerminalClosedEvent(e) => {
                TerminalAttachStreamEvent::TerminalClosedEvent(e.clone())
            }
            TerminalEvent::TerminalErrorEvent(e) => {
                TerminalAttachStreamEvent::TerminalErrorEvent(e.clone())
            }
            TerminalEvent::TerminalClearedEvent(e) => {
                TerminalAttachStreamEvent::TerminalClearedEvent(e.clone())
            }
            TerminalEvent::TerminalRestartedEvent(e) => {
                TerminalAttachStreamEvent::TerminalRestartedEvent(e.clone())
            }
            TerminalEvent::TerminalActivityEvent(e) => {
                TerminalAttachStreamEvent::TerminalActivityEvent(e.clone())
            }
        }
    }
}
