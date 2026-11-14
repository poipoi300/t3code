//! A stand-in for the client core's composition root, until `t3-client` provides one.
//!
//! It validates pairing input with the client's own `resolve_pairing_target`, then plays the part
//! of a server: it writes wire-shaped stream items (decoded by `t3-contracts`) into the real
//! `ClientStore` through the client's reducers. Every screen reads real store state through the
//! real selectors; only the network is missing. Replacing it is one line in `runtime::host`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use futures::FutureExt;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::future::LocalBoxFuture;
use futures::stream::LocalBoxStream;
use nsh_core::prelude::timer;
use serde_json::{Value, json};
use t3_client::ClientStore;
use t3_client::connection::{
    AttemptStage, ConnectionFailure, ConnectionState, FailureKind, PairingInput, Phase,
    resolve_pairing_target,
};
use t3_client::platform::NetworkStatus;
use t3_client::types::{EnvironmentId, Generation, ThreadId, ThreadKey};
use t3_contracts::orchestration_v2::{
    OrchestrationV2ShellStreamItem, OrchestrationV2ThreadStreamItem,
};

use super::backend::{Backend, EnvironmentEntry, SendError};

mod fixtures;

pub use fixtures::{DEMO_PAIRING_URL, FIRST_THREAD};

use crate::runtime::tasks::watch;

#[derive(Clone)]
pub struct DemoBackend {
    inner: Rc<Inner>,
}

struct Inner {
    store: ClientStore,
    environments: RefCell<Vec<EnvironmentEntry>>,
    sessions: RefCell<BTreeMap<EnvironmentId, Generation>>,
    sequences: RefCell<BTreeMap<ThreadKey, u64>>,
    ordinals: RefCell<BTreeMap<ThreadKey, u64>>,
    loaded: RefCell<BTreeSet<ThreadKey>>,
    listeners: RefCell<Vec<mpsc::UnboundedSender<()>>>,
    clock: Cell<u64>,
    /// How long a reply streams per chunk. Zero in tests, where the stream finishes at once.
    chunk_delay: Duration,
}

impl DemoBackend {
    pub fn new(store: ClientStore) -> Self {
        Self::with_chunk_delay(store, Duration::from_millis(60))
    }

    pub fn with_chunk_delay(store: ClientStore, chunk_delay: Duration) -> Self {
        Self {
            inner: Rc::new(Inner {
                store,
                environments: RefCell::new(Vec::new()),
                sessions: RefCell::new(BTreeMap::new()),
                sequences: RefCell::new(BTreeMap::new()),
                ordinals: RefCell::new(BTreeMap::new()),
                loaded: RefCell::new(BTreeSet::new()),
                listeners: RefCell::new(Vec::new()),
                clock: Cell::new(0),
                chunk_delay,
            }),
        }
    }
}

impl Inner {
    fn notify(&self) {
        self.listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(()).is_ok());
    }

    fn set_state(&self, environment: &EnvironmentId, state: ConnectionState) {
        if let Some(entry) = self
            .environments
            .borrow_mut()
            .iter_mut()
            .find(|entry| &entry.id == environment)
        {
            entry.state = state;
        }
        self.notify();
    }

    fn now(&self) -> String {
        let seconds = self.clock.get() + 1;
        self.clock.set(seconds);
        fixtures::timestamp(seconds)
    }

    fn connect(&self, environment: &EnvironmentId) -> Result<(), ConnectionFailure> {
        let session = self.store.begin_session(environment.clone());
        self.sessions
            .borrow_mut()
            .insert(environment.clone(), session);
        let snapshot = shell_item(json!({"kind": "snapshot", "snapshot": fixtures::shell()}))?;
        let synchronized = shell_item(json!({"kind": "synchronized"}))?;
        self.store
            .apply_shell(environment, session, vec![snapshot, synchronized]);
        self.set_state(
            environment,
            ConnectionState {
                desired: true,
                network: NetworkStatus::Online,
                phase: Phase::Connected,
                generation: session,
                ..ConnectionState::default()
            },
        );
        Ok(())
    }

    fn next_sequence(&self, key: &ThreadKey) -> u64 {
        let mut sequences = self.sequences.borrow_mut();
        let sequence = sequences.entry(key.clone()).or_insert(100);
        *sequence += 1;
        *sequence
    }

    fn next_ordinal(&self, key: &ThreadKey) -> u64 {
        let mut ordinals = self.ordinals.borrow_mut();
        let ordinal = ordinals.entry(key.clone()).or_insert(0);
        *ordinal += 1;
        *ordinal
    }

    fn deliver(&self, key: &ThreadKey, items: Vec<Value>) -> Result<(), SendError> {
        let session = self
            .sessions
            .borrow()
            .get(&key.environment)
            .copied()
            .ok_or(SendError::Disconnected)?;
        let items = items
            .into_iter()
            .map(serde_json::from_value::<OrchestrationV2ThreadStreamItem>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SendError::Rejected(error.to_string()))?;
        self.store
            .apply_thread(key.clone(), session, items)
            .map(|_| ())
            .map_err(|error| SendError::Rejected(error.to_string()))
    }

    fn event(&self, key: &ThreadKey, item: Value) -> Value {
        json!({
            "kind": "event",
            "sequence": self.next_sequence(key),
            "event": {
                "type": "turn-item.updated",
                "id": format!("event-{}", self.next_sequence(key)),
                "threadId": key.thread.0,
                "occurredAt": self.now(),
                "payload": item,
            }
        })
    }

    fn load_thread(&self, key: &ThreadKey) -> Result<(), SendError> {
        if self.loaded.borrow().contains(key) {
            return Ok(());
        }
        let conversation = fixtures::conversation(&key.thread.0);
        let count = conversation.len() as u64;
        let projection = fixtures::projection(&key.thread.0, conversation);
        let snapshot = json!({
            "kind": "snapshot",
            "snapshotSequence": self.next_sequence(key),
            "projection": projection,
        });
        self.deliver(key, vec![snapshot, json!({"kind": "synchronized"})])?;
        self.ordinals.borrow_mut().insert(key.clone(), count);
        self.loaded.borrow_mut().insert(key.clone());
        Ok(())
    }
}

fn shell_item(value: Value) -> Result<OrchestrationV2ShellStreamItem, ConnectionFailure> {
    serde_json::from_value(value)
        .map_err(|error| ConnectionFailure::new(FailureKind::Transport, error.to_string()))
}

impl Backend for DemoBackend {
    fn pair(
        &self,
        input: PairingInput,
    ) -> LocalBoxFuture<'static, Result<EnvironmentId, ConnectionFailure>> {
        let inner = self.inner.clone();
        async move {
            let target = resolve_pairing_target(&input)?;
            let label = url_host(&target.http_base);
            let environment = EnvironmentId(format!("demo-{label}"));
            let known = inner
                .environments
                .borrow()
                .iter()
                .any(|entry| entry.id == environment);
            if !known {
                inner.environments.borrow_mut().push(EnvironmentEntry {
                    id: environment.clone(),
                    label,
                    state: ConnectionState::default(),
                    damaged: false,
                    // The demo saves no route to reach it.
                    routes: Vec::new(),
                });
            }
            inner.set_state(
                &environment,
                ConnectionState {
                    desired: true,
                    network: NetworkStatus::Online,
                    phase: Phase::Connecting,
                    stage: Some(AttemptStage::Opening),
                    attempt: 1,
                    ..ConnectionState::default()
                },
            );
            if !inner.chunk_delay.is_zero() {
                timer(Duration::from_millis(400)).await;
            }
            inner.connect(&environment)?;
            Ok(environment)
        }
        .boxed_local()
    }

    fn environments(&self) -> Vec<EnvironmentEntry> {
        self.inner.environments.borrow().clone()
    }

    fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.inner.listeners.borrow_mut().push(sender);
        receiver.boxed_local()
    }

    fn open_thread(&self, key: &ThreadKey) {
        let _ = self.inner.load_thread(key);
    }

    fn close_thread(&self, _key: &ThreadKey) {}

    fn send_message(
        &self,
        key: &ThreadKey,
        text: String,
    ) -> LocalBoxFuture<'static, Result<(), SendError>> {
        let inner = self.inner.clone();
        let key = key.clone();
        async move {
            if text.trim().is_empty() {
                return Err(SendError::Rejected("Enter a message.".into()));
            }
            inner.load_thread(&key)?;
            let user = fixtures::user_message(
                &key.thread.0,
                inner.next_ordinal(&key),
                &text,
                &inner.now(),
            );
            let event = inner.event(&key, user);
            inner.deliver(&key, vec![event])?;
            let ordinal = inner.next_ordinal(&key);
            let reply = fixtures::reply_to(&text);
            let chunks = fixtures::chunks(&reply);
            let streaming = inner.chunk_delay;
            let task = {
                let inner = inner.clone();
                let key = key.clone();
                async move {
                    let mut text = String::new();
                    for chunk in &chunks {
                        text.push_str(chunk);
                        let item = fixtures::assistant_message(
                            &key.thread.0,
                            ordinal,
                            &text,
                            true,
                            &inner.now(),
                        );
                        let event = inner.event(&key, item);
                        inner.deliver(&key, vec![event])?;
                        if !streaming.is_zero() {
                            timer(streaming).await;
                        }
                    }
                    let item = fixtures::assistant_message(
                        &key.thread.0,
                        ordinal,
                        &text,
                        false,
                        &inner.now(),
                    );
                    let event = inner.event(&key, item);
                    inner.deliver(&key, vec![event])
                }
            };
            if streaming.is_zero() {
                task.await
            } else {
                watch(async move {
                    let _ = task.await;
                });
                Ok(())
            }
        }
        .boxed_local()
    }
}

fn url_host(base: &str) -> String {
    let rest = base.split("://").nth(1).unwrap_or(base);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_owned()
}

pub fn demo_thread(environment: &EnvironmentId) -> ThreadKey {
    ThreadKey {
        environment: environment.clone(),
        thread: ThreadId(fixtures::FIRST_THREAD.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use t3_client::types::SyncStatus;

    fn backend() -> (ClientStore, DemoBackend) {
        let store = ClientStore::default();
        let backend = DemoBackend::with_chunk_delay(store.clone(), Duration::ZERO);
        (store, backend)
    }

    #[test]
    fn invalid_pairing_input_is_refused_with_the_clients_message_and_registers_nothing() {
        let (store, backend) = backend();
        let error = block_on(backend.pair(PairingInput::default())).expect_err("empty input");
        assert_eq!(error.detail, "Enter a backend URL.");
        let error = block_on(backend.pair(PairingInput {
            pairing_url: Some("https://example.test/pair".into()),
            ..PairingInput::default()
        }))
        .expect_err("no token");
        assert_eq!(error.detail, "Pairing URL is missing its token.");
        assert!(backend.environments().is_empty());
        assert!(store.snapshot().environments.is_empty());
    }

    #[test]
    fn pairing_installs_a_live_shell_through_the_clients_reducer() {
        let (store, backend) = backend();
        let environment = block_on(backend.pair(PairingInput {
            pairing_url: Some(DEMO_PAIRING_URL.into()),
            ..PairingInput::default()
        }))
        .expect("pairs");
        let entries = backend.environments();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].state.phase, Phase::Connected);
        let snapshot = store.snapshot();
        let shell = t3_client::state::selectors::shell(&snapshot, &environment).expect("shell");
        assert_eq!(shell.status, SyncStatus::Live);
        let threads = t3_client::state::selectors::sidebar_threads(shell, None);
        assert!(threads.len() >= 6, "{} threads", threads.len());
    }

    #[test]
    fn a_sent_message_and_its_streamed_reply_reach_the_timeline() {
        let (store, backend) = backend();
        let environment = block_on(backend.pair(PairingInput {
            pairing_url: Some(DEMO_PAIRING_URL.into()),
            ..PairingInput::default()
        }))
        .expect("pairs");
        let key = demo_thread(&environment);
        backend.open_thread(&key);
        let before = store
            .shared_thread(&key)
            .expect("thread loaded")
            .visible_turn_items
            .len();
        assert!(before > 0);
        block_on(backend.send_message(&key, "Ship it on every platform".into())).expect("sent");
        let projection = store.shared_thread(&key).expect("thread");
        assert_eq!(projection.visible_turn_items.len(), before + 2);
        let input = t3_client::view_logic::session::TimelineEntriesInput {
            visible_turn_items: projection.visible_turn_items.to_vec(),
            ..Default::default()
        };
        let entries =
            t3_client::view_logic::session::derive_timeline_entries_from_visible_turn_items(&input);
        let texts: Vec<String> = entries
            .iter()
            .filter_map(|entry| match &entry.content {
                t3_client::view_logic::session::TimelineContent::Message { message, .. } => {
                    Some(message.text.clone())
                }
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "Ship it on every platform"));
        let reply = texts.last().expect("reply");
        assert_eq!(reply, &fixtures::reply_to("Ship it on every platform"));
    }

    #[test]
    fn sending_to_an_unpaired_environment_is_disconnected_not_dropped_silently() {
        let (_store, backend) = backend();
        let key = demo_thread(&EnvironmentId("nowhere".into()));
        assert_eq!(
            block_on(backend.send_message(&key, "hello".into())),
            Err(SendError::Disconnected)
        );
        assert_eq!(
            block_on(backend.send_message(&key, "   ".into())),
            Err(SendError::Rejected("Enter a message.".into()))
        );
    }
}
