//! The guest page's state: one share, one isolated thread projection, no owner services.

use std::{cell::RefCell, rc::Rc};

use serde_json::Value;
use t3_contracts::{
    base_schemas::ThreadId,
    guest_share::{
        GUEST_MESSAGE_MAX_LENGTH, GUEST_NAME_MAX_LENGTH, GUEST_PENDING_MESSAGE_LIMIT,
        ThreadGuestMessageStatus,
    },
    guest_stream::GuestShareView,
    orchestration_v2::OrchestrationV2ThreadStreamItem,
    orchestration_v2_id_table::IdOrIndex,
};

use super::transport::{GuestError, GuestFailure, GuestTransport};
use crate::{
    state::{decoding, thread::ThreadState},
    types::Generation,
};

/// An invite credential. It lives only in memory, is never printed, and is dropped once it has
/// been redeemed or has failed for good.
pub struct GuestCredential(String);

impl GuestCredential {
    pub fn new(credential: impl Into<String>) -> Self {
        Self(credential.into())
    }
}

impl std::fmt::Debug for GuestCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GuestCredential(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestEnd {
    /// No usable link, or a link for a different browser.
    Invalid,
    /// Revoked, replaced, expired, archived or deleted.
    Inactive,
    /// The one-use invite was already redeemed elsewhere.
    Used,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestPhase {
    Checking,
    NeedsName {
        error: Option<String>,
    },
    Joining,
    Connecting,
    Live,
    /// The stream dropped; the transcript stays on screen while the page reconnects.
    Reconnecting,
    Ended(GuestEnd),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitRefusal {
    Empty,
    TooLong,
    TooManyPending,
    NotLive,
    Server(GuestFailure, String),
    Network,
}

pub struct GuestSession {
    pub phase: GuestPhase,
    pub view: Option<GuestShareView>,
    thread: Option<ThreadState>,
    generation: u64,
}

impl GuestSession {
    fn new() -> Self {
        Self {
            phase: GuestPhase::Checking,
            view: None,
            thread: None,
            generation: 0,
        }
    }

    pub fn thread_id(&self) -> Option<&ThreadId> {
        self.thread.as_ref().map(|thread| &thread.id)
    }

    pub fn thread(&self) -> Option<&ThreadState> {
        self.thread.as_ref()
    }

    pub fn pending_count(&self) -> usize {
        self.view.as_ref().map_or(0, |view| {
            view.messages
                .iter()
                .filter(|message| message.status == ThreadGuestMessageStatus::Pending)
                .count()
        })
    }

    /// One decoded stream frame. A protocol violation ends the session as invalid.
    fn apply_frame(&mut self, frame: Value) -> bool {
        match frame.get("type").and_then(Value::as_str) {
            Some("view") => {
                let Some(view) = frame
                    .get("view")
                    .and_then(|view| serde_json::from_value::<GuestShareView>(view.clone()).ok())
                else {
                    return self.violation();
                };
                self.view = Some(view);
                true
            }
            Some("thread") => {
                let Some(item) = frame
                    .get("item")
                    .and_then(|item| decoding::thread_stream(item.clone()).ok())
                else {
                    return self.violation();
                };
                self.apply_thread_item(item)
            }
            Some("ended") => {
                self.phase = GuestPhase::Ended(GuestEnd::Inactive);
                self.thread = None;
                self.view = None;
                true
            }
            _ => self.violation(),
        }
    }

    fn violation(&mut self) -> bool {
        self.phase = GuestPhase::Ended(GuestEnd::Invalid);
        self.thread = None;
        self.view = None;
        true
    }

    fn apply_thread_item(&mut self, item: OrchestrationV2ThreadStreamItem) -> bool {
        let session = Generation(self.generation);
        if let OrchestrationV2ThreadStreamItem::Snapshot(snapshot) = &item {
            let IdOrIndex::Id(id) = &snapshot.projection.thread.id else {
                return self.violation();
            };
            if self.thread.as_ref().is_some_and(|thread| &thread.id != id) {
                // One share is one thread: a snapshot of another thread is never rendered.
                return self.violation();
            }
            if self.thread.is_none() {
                self.thread = Some(ThreadState::new(id.clone()));
            }
            if let Some(thread) = &mut self.thread {
                thread.begin_session(session);
            }
        }
        let Some(thread) = &mut self.thread else {
            return true;
        };
        if thread.apply(session, item).is_err() {
            return self.violation();
        }
        // The stream proves liveness once the first snapshot or event has applied.
        if matches!(
            self.phase,
            GuestPhase::Connecting | GuestPhase::Reconnecting
        ) && thread.projection.is_some()
        {
            self.phase = GuestPhase::Live;
        }
        true
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.encode_utf16().count() <= GUEST_NAME_MAX_LENGTH
        && !name.chars().any(|ch| {
            ch.is_control()
                || matches!(ch,
                    '\u{061c}' | '\u{200e}' | '\u{200f}'
                    | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
        })
}

fn end_for(reason: GuestFailure) -> GuestEnd {
    match reason {
        GuestFailure::Used => GuestEnd::Used,
        GuestFailure::Inactive => GuestEnd::Inactive,
        _ => GuestEnd::Invalid,
    }
}

type Listener = Rc<dyn Fn(&GuestSession)>;

/// Drives one guest page over a transport. Cloneable handle; the UI listens for changes.
#[derive(Clone)]
pub struct GuestClient {
    transport: Rc<dyn GuestTransport>,
    share: Rc<str>,
    credential: Rc<RefCell<Option<GuestCredential>>>,
    state: Rc<RefCell<GuestSession>>,
    listener: Listener,
}

const BACKOFF_MS: [u64; 5] = [1_000, 2_000, 4_000, 8_000, 15_000];

impl GuestClient {
    pub fn new(
        transport: Rc<dyn GuestTransport>,
        share: &str,
        credential: Option<GuestCredential>,
        listener: impl Fn(&GuestSession) + 'static,
    ) -> Self {
        Self {
            transport,
            share: share.into(),
            credential: Rc::new(RefCell::new(credential)),
            state: Rc::new(RefCell::new(GuestSession::new())),
            listener: Rc::new(listener),
        }
    }

    pub fn snapshot<R>(&self, read: impl FnOnce(&GuestSession) -> R) -> R {
        read(&self.state.borrow())
    }

    fn update(&self, change: impl FnOnce(&mut GuestSession)) {
        change(&mut self.state.borrow_mut());
        (self.listener)(&self.state.borrow());
    }

    /// With an unspent invite the page asks for a name; otherwise it checks the browser's cookie.
    pub async fn start(&self) {
        if self.credential.borrow().is_some() {
            self.update(|state| state.phase = GuestPhase::NeedsName { error: None });
            return;
        }
        match self.transport.session(&self.share).await {
            Ok(()) => self.update(|state| state.phase = GuestPhase::Connecting),
            Err(GuestError::Rejected { reason, .. }) => {
                self.update(|state| state.phase = GuestPhase::Ended(end_for(reason)));
            }
            Err(GuestError::Network(_)) => {
                self.update(|state| state.phase = GuestPhase::Reconnecting);
            }
        }
    }

    /// Redeem the invite under `name`. A network failure keeps the invite for another try.
    pub async fn join(&self, name: &str) {
        let name = t3_contracts::wire::trim_js(name).to_owned();
        if !valid_name(&name) {
            self.update(|state| {
                state.phase = GuestPhase::NeedsName {
                    error: Some("Enter a name of up to 40 characters.".into()),
                };
            });
            return;
        }
        let Some(credential) = self.credential.borrow_mut().take() else {
            return;
        };
        self.update(|state| state.phase = GuestPhase::Joining);
        match self.transport.redeem(&credential.0, &name).await {
            Ok(()) => self.update(|state| state.phase = GuestPhase::Connecting),
            Err(GuestError::Rejected {
                reason, message, ..
            }) => {
                if matches!(reason, GuestFailure::Busy | GuestFailure::Rejected) {
                    *self.credential.borrow_mut() = Some(credential);
                    self.update(|state| {
                        state.phase = GuestPhase::NeedsName {
                            error: Some(message),
                        };
                    });
                } else {
                    self.update(|state| state.phase = GuestPhase::Ended(end_for(reason)));
                }
            }
            Err(GuestError::Network(_)) => {
                *self.credential.borrow_mut() = Some(credential);
                self.update(|state| {
                    state.phase = GuestPhase::NeedsName {
                        error: Some("Could not reach the owner's server. Try again.".into()),
                    };
                });
            }
        }
    }

    /// Hold the live stream until the share ends, reconnecting after network failures.
    pub async fn run(&self) {
        let mut attempt = 0usize;
        loop {
            if matches!(self.state.borrow().phase, GuestPhase::Ended(_)) {
                return;
            }
            match self.transport.open_stream(&self.share).await {
                Err(GuestError::Rejected { reason, .. }) => {
                    self.update(|state| {
                        state.generation += 1;
                        state.phase = GuestPhase::Ended(end_for(reason));
                        state.thread = None;
                        state.view = None;
                    });
                    return;
                }
                Err(GuestError::Network(_)) => {}
                Ok(mut stream) => {
                    self.update(|state| state.generation += 1);
                    loop {
                        match stream.next().await {
                            Ok(Some(frame)) => {
                                attempt = 0;
                                self.update(|state| {
                                    state.apply_frame(frame);
                                });
                                if matches!(self.state.borrow().phase, GuestPhase::Ended(_)) {
                                    return;
                                }
                            }
                            Ok(None) | Err(GuestError::Network(_)) => break,
                            Err(GuestError::Rejected { reason, .. }) => {
                                self.update(|state| {
                                    state.phase = GuestPhase::Ended(end_for(reason));
                                });
                                return;
                            }
                        }
                    }
                }
            }
            self.update(|state| {
                if let Some(thread) = &mut state.thread {
                    thread.disconnect();
                }
                state.phase = GuestPhase::Reconnecting;
            });
            let wait = BACKOFF_MS[attempt.min(BACKOFF_MS.len() - 1)];
            attempt += 1;
            self.transport.sleep(wait).await;
        }
    }

    /// Send one message to the owner's approval queue.
    pub async fn submit(&self, text: &str) -> Result<(), SubmitRefusal> {
        let text = t3_contracts::wire::trim_js(text).to_owned();
        {
            let state = self.state.borrow();
            if !matches!(state.phase, GuestPhase::Live) {
                return Err(SubmitRefusal::NotLive);
            }
            if text.is_empty() {
                return Err(SubmitRefusal::Empty);
            }
            if text.encode_utf16().count() > GUEST_MESSAGE_MAX_LENGTH {
                return Err(SubmitRefusal::TooLong);
            }
            if state.pending_count() >= GUEST_PENDING_MESSAGE_LIMIT {
                return Err(SubmitRefusal::TooManyPending);
            }
        }
        match self.transport.submit(&self.share, &text).await {
            Ok(()) => Ok(()),
            Err(GuestError::Rejected {
                reason, message, ..
            }) => {
                if matches!(reason, GuestFailure::Invalid | GuestFailure::Inactive) {
                    self.update(|state| state.phase = GuestPhase::Ended(end_for(reason)));
                }
                Err(SubmitRefusal::Server(reason, message))
            }
            Err(GuestError::Network(_)) => Err(SubmitRefusal::Network),
        }
    }
}
