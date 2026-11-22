//! The guest page's state: one share, one isolated thread projection, no owner services.

use std::{cell::RefCell, rc::Rc};

use futures::StreamExt;
use serde_json::Value;
use t3_contracts::{
    base_schemas::ThreadId,
    guest_share::{
        GUEST_MESSAGE_MAX_LENGTH, GUEST_NAME_MAX_LENGTH, GUEST_PENDING_MESSAGE_LIMIT,
        ThreadGuestMessageStatus,
    },
    guest_stream::GuestShareView,
    orchestration_v2::{
        OrchestrationV2CommandOutputFrame, OrchestrationV2ThreadStreamItem, OrchestrationV2TurnItem,
    },
    orchestration_v2_id_table::IdOrIndex,
};

use super::transport::{GuestError, GuestFailure, GuestStreamHandle, GuestTransport};
use crate::{
    ClientError,
    composition::Spawner,
    services::command_output::CommandOutputLease,
    state::{decoding, items, thread::ThreadState},
    types::Generation,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRole {
    /// Anyone who wrote into the thread (the owner or an approved guest).
    Person,
    Assistant,
    Command,
}

/// One readable row of the shared conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRow {
    pub id: String,
    pub role: TranscriptRole,
    pub text: String,
    pub exit_code: Option<i64>,
    pub running: bool,
}

/// Paging state for "Load earlier turns" on the guest page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestHistoryState {
    pub cursor: Option<String>,
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
}

/// Plain command output text with SGR escapes removed for simple display.
pub fn plain_command_output(text: &str) -> String {
    t3_shared::terminal_output::terminal_output_plain_text(text)
}

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

    pub fn history(&self) -> GuestHistoryState {
        let Some(thread) = &self.thread else {
            return GuestHistoryState::default();
        };
        let has_more = thread.history.has_more && thread.history.cursor.is_some();
        GuestHistoryState {
            cursor: has_more.then(|| thread.history.cursor.clone()).flatten(),
            has_more,
            loading: thread.history.loading,
            error: thread.history.error.clone(),
        }
    }

    pub fn has_more_history(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| thread.history.has_more && thread.history.cursor.is_some())
    }

    pub fn history_cursor(&self) -> Option<&str> {
        self.thread
            .as_ref()
            .filter(|thread| thread.history.has_more)
            .and_then(|thread| thread.history.cursor.as_deref())
    }

    /// The conversation as the guest reads it, in thread order, including command rows whose
    /// output loads on expand.
    pub fn transcript(&self) -> Vec<TranscriptRow> {
        let Some(projection) = self
            .thread
            .as_ref()
            .and_then(|thread| thread.projection.as_ref())
        else {
            return Vec::new();
        };
        let row_of = |item: &OrchestrationV2TurnItem| -> Option<TranscriptRow> {
            let IdOrIndex::Id(id) = items::id(item) else {
                return None;
            };
            let id = id.0.clone();
            match item {
                OrchestrationV2TurnItem::UserMessage(v) => Some(TranscriptRow {
                    id,
                    role: TranscriptRole::Person,
                    text: v.text.clone(),
                    exit_code: None,
                    running: false,
                }),
                OrchestrationV2TurnItem::AssistantMessage(v) => Some(TranscriptRow {
                    id,
                    role: TranscriptRole::Assistant,
                    text: v.text.clone(),
                    exit_code: None,
                    running: v.streaming,
                }),
                OrchestrationV2TurnItem::CommandExecution(v) => Some(TranscriptRow {
                    id,
                    role: TranscriptRole::Command,
                    text: v.input.clone(),
                    exit_code: v.exit_code.flatten(),
                    running: items::active(item),
                }),
                _ => None,
            }
        };
        if !projection.visible_turn_items.is_empty() {
            projection
                .visible_turn_items
                .iter()
                .filter_map(|row| row_of(&row.item))
                .collect()
        } else {
            let mut rows: Vec<(u64, TranscriptRow)> = projection
                .turn_items
                .iter()
                .filter_map(|item| row_of(item).map(|row| (items::ordinal(item), row)))
                .collect();
            rows.sort_by_key(|(ordinal, _)| *ordinal);
            rows.into_iter().map(|(_, row)| row).collect()
        }
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

const MAX_BACKOFF_MS: u64 = 15_000;
const BACKOFF_MS: [u64; 5] = [1_000, 2_000, 4_000, 8_000, MAX_BACKOFF_MS];

/// Exponential backoff with equal jitter in `[base / 2, base]`, respecting `Retry-After` up to
/// [`MAX_BACKOFF_MS`].
pub fn reconnect_delay_ms(attempt: usize, random: f64, retry_after_ms: Option<u64>) -> u64 {
    let base = BACKOFF_MS[attempt.min(BACKOFF_MS.len() - 1)];
    let sample = if random.is_nan() {
        0.5
    } else {
        random.clamp(0.0, 1.0)
    };
    let backoff = ((base as f64 / 2.0) * (1.0 + sample)).round() as u64;
    retry_after_ms
        .map_or(backoff, |retry_after| backoff.max(retry_after))
        .min(MAX_BACKOFF_MS)
}

fn history_error_text(error: &GuestError) -> String {
    match error {
        GuestError::Rejected { message, .. } => message.clone(),
        GuestError::Network(_) | GuestError::RetryAfter { .. } => {
            "Could not reach the server.".into()
        }
    }
}

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

    /// Checks the browser's cookie first, as the TS page does: a browser that already joined goes
    /// straight back in, even from a spent invite, and the invite is dropped. Without a usable
    /// cookie an unspent invite asks for a name; a failed check does not hide the form.
    pub async fn start(&self) {
        match self.transport.session(&self.share).await {
            Ok(()) => {
                *self.credential.borrow_mut() = None;
                self.update(|state| state.phase = GuestPhase::Connecting);
            }
            Err(_) if self.credential.borrow().is_some() => {
                self.update(|state| state.phase = GuestPhase::NeedsName { error: None });
            }
            Err(GuestError::Rejected { reason, .. }) => {
                self.update(|state| state.phase = GuestPhase::Ended(end_for(reason)));
            }
            Err(GuestError::Network(_) | GuestError::RetryAfter { .. }) => {
                self.update(|state| state.phase = GuestPhase::Reconnecting);
            }
        }
    }

    /// `start`, then hold the stream when the page can already read: a transient failure of the
    /// check goes on to the reconnect loop, which retries with backoff.
    pub async fn resume(&self) {
        self.start().await;
        if self.snapshot(|state| {
            matches!(
                state.phase,
                GuestPhase::Connecting | GuestPhase::Reconnecting
            )
        }) {
            self.run().await;
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
                // Only the server's explicit verdicts end the page; anything else (a proxy's bodiless
                // 413, an unknown reason) keeps the invite and the form for another try.
                if !matches!(
                    reason,
                    GuestFailure::Used | GuestFailure::Inactive | GuestFailure::Invalid
                ) {
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
            Err(GuestError::Network(_) | GuestError::RetryAfter { .. }) => {
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
            let mut retry_after_ms = None;
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
                Err(error) => {
                    retry_after_ms = error.retry_after_ms();
                }
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
                            Ok(None) => break,
                            Err(GuestError::Network(_)) => break,
                            Err(GuestError::RetryAfter { millis, .. }) => {
                                retry_after_ms = Some(millis);
                                break;
                            }
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
            let wait = reconnect_delay_ms(attempt, self.transport.random(), retry_after_ms);
            attempt += 1;
            self.transport.sleep(wait).await;
        }
    }

    /// Load the next page of older turns at the current history cursor, ignoring stale answers
    /// if a reconnect snapshot moved the cursor while the request was in flight.
    pub async fn load_earlier(&self) {
        let request = {
            let mut state = self.state.borrow_mut();
            let Some(thread) = state.thread.as_mut() else {
                return;
            };
            thread.history_request()
        };
        let Some(request) = request else {
            return;
        };
        (self.listener)(&self.state.borrow());
        match self.transport.history(&self.share, &request.cursor).await {
            Ok(page) => {
                self.update(|state| {
                    let Some(thread) = state.thread.as_mut() else {
                        return;
                    };
                    let _ = thread.apply_history(&request, page);
                });
            }
            Err(error) => {
                let message = history_error_text(&error);
                self.update(|state| {
                    let Some(thread) = state.thread.as_mut() else {
                        return;
                    };
                    if thread.session == request.session
                        && thread.generation == request.generation
                        && thread.history.cursor.as_deref() == Some(request.cursor.as_str())
                    {
                        thread.history.loading = false;
                        thread.history.error = Some(message);
                    }
                });
            }
        }
    }

    /// Stream one expanded command row's output through `/api/guest/command-output`, releasing
    /// the connection as soon as the command settles or the returned lease is dropped.
    pub fn command_output(&self, item_id: &str, spawn: &Spawner) -> CommandOutputLease {
        let transport = self.transport.clone();
        let share = self.share.clone();
        let item_id = item_id.to_owned();
        enum State {
            Connect,
            Open(Box<dyn GuestStreamHandle>),
            Done,
        }
        let frames = futures::stream::unfold(State::Connect, move |state| {
            let transport = transport.clone();
            let share = share.clone();
            let item_id = item_id.clone();
            async move {
                match state {
                    State::Done => None,
                    State::Connect => match transport.command_output(&share, &item_id).await {
                        Ok(stream) => Some((None, State::Open(stream))),
                        Err(_) => Some((
                            Some(Err(ClientError::Rejected {
                                message: "Output is no longer available.".into(),
                                detail: None,
                            })),
                            State::Done,
                        )),
                    },
                    State::Open(mut stream) => loop {
                        match stream.next().await {
                            Ok(Some(raw)) => {
                                let Ok(frame) = serde_json::from_value::<
                                    OrchestrationV2CommandOutputFrame,
                                >(raw) else {
                                    continue;
                                };
                                let next = if frame.running {
                                    State::Open(stream)
                                } else {
                                    State::Done
                                };
                                return Some((Some(Ok(frame)), next));
                            }
                            Ok(None) | Err(_) => {
                                return Some((
                                    Some(Err(ClientError::Rejected {
                                        message: "Output is no longer available.".into(),
                                        detail: None,
                                    })),
                                    State::Done,
                                ));
                            }
                        }
                    },
                }
            }
        })
        .filter_map(futures::future::ready);
        CommandOutputLease::from_frames(Box::pin(frames), spawn)
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
            Err(GuestError::Network(_) | GuestError::RetryAfter { .. }) => {
                Err(SubmitRefusal::Network)
            }
        }
    }
}
