//! The app's location on desktop and mobile, where there is no browser history: an in-memory
//! stack with back and forward, as the TypeScript router's memory history keeps it. Route
//! logic stays in `t3_client::navigation`; this only holds and changes the address.
use futures::{StreamExt, channel::mpsc, stream::LocalBoxStream};
use t3_client::{
    navigation::{AppUrlRequest, AppUrlSnapshot, CloseModal, RoutePolicy},
    platform::PlatformError,
};

type Listener = mpsc::UnboundedSender<Result<AppUrlSnapshot, PlatformError>>;

pub(crate) struct MemoryHistory {
    entries: Vec<String>,
    entry_keys: Vec<u64>,
    next_entry_key: u64,
    index: usize,
    policy: RoutePolicy,
    listeners: Vec<Listener>,
}

impl MemoryHistory {
    /// Starts at `launch` (an app path such as `/settings`), or `/`.
    pub(crate) fn new(launch: Option<String>) -> Self {
        Self {
            entries: vec![launch.unwrap_or_else(|| "/".into())],
            entry_keys: vec![0],
            next_entry_key: 1,
            index: 0,
            policy: RoutePolicy::default(),
            listeners: Vec::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> AppUrlSnapshot {
        AppUrlSnapshot {
            path: self.entries[self.index].clone(),
            history_index: self.index as u64,
            entry_key: Some(self.entry_keys[self.index].to_string()),
            can_go_back: self.index > 0,
            can_go_forward: self.index + 1 < self.entries.len(),
            policy: self.policy,
        }
    }

    /// The current location first, then every change, until the stream is dropped.
    pub(crate) fn changes(
        &mut self,
    ) -> LocalBoxStream<'static, Result<AppUrlSnapshot, PlatformError>> {
        let (sender, receiver) = mpsc::unbounded();
        let _ = sender.unbounded_send(Ok(self.snapshot()));
        self.listeners.push(sender);
        receiver.boxed_local()
    }

    fn notify(&mut self) {
        let snapshot = self.snapshot();
        self.listeners
            .retain(|listener| listener.unbounded_send(Ok(snapshot.clone())).is_ok());
    }

    pub(crate) fn set_policy(&mut self, policy: RoutePolicy) {
        if self.policy != policy {
            self.policy = policy;
            self.notify();
        }
    }

    /// A new entry after the current one; forward entries are discarded.
    pub(crate) fn push(&mut self, href: String) {
        let key = self.allocate_key();
        self.entries.truncate(self.index + 1);
        self.entry_keys.truncate(self.index + 1);
        self.entries.push(href);
        self.entry_keys.push(key);
        self.index += 1;
        self.notify();
    }

    fn allocate_key(&mut self) -> u64 {
        let key = self.next_entry_key;
        self.next_entry_key = self
            .next_entry_key
            .checked_add(1)
            .expect("history entry id exhausted");
        key
    }

    fn go(&mut self, delta: i64) {
        // Out of range does nothing, as `history.go` does.
        let target = self.index as i64 + delta;
        if delta != 0 && (0..self.entries.len() as i64).contains(&target) {
            self.index = target as usize;
            self.notify();
        }
    }

    pub(crate) fn apply(&mut self, request: AppUrlRequest) {
        match request {
            AppUrlRequest::Push(href) | AppUrlRequest::CloseModal(CloseModal::Push(href)) => {
                self.push(href)
            }
            AppUrlRequest::Replace(href) => {
                let key = self.allocate_key();
                self.entries[self.index] = href;
                self.entry_keys[self.index] = key;
                self.notify();
            }
            AppUrlRequest::Go(delta) => self.go(delta),
            AppUrlRequest::CloseModal(CloseModal::GoThenPush { delta, href }) => {
                self.go(delta);
                self.push(href);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use futures::FutureExt;

    fn path(history: &MemoryHistory) -> (String, bool, bool) {
        let snapshot = history.snapshot();
        (snapshot.path, snapshot.can_go_back, snapshot.can_go_forward)
    }

    #[test]
    fn back_forward_and_a_push_that_prunes_forward_entries() {
        let mut history = MemoryHistory::new(None);
        let mut changes = history.changes();
        assert_eq!(
            changes
                .next()
                .now_or_never()
                .flatten()
                .unwrap()
                .unwrap()
                .path,
            "/"
        );
        history.apply(AppUrlRequest::Push("/settings".into()));
        history.apply(AppUrlRequest::Push("/usage".into()));
        history.apply(AppUrlRequest::Go(-2));
        assert_eq!(path(&history), ("/".into(), false, true));
        history.apply(AppUrlRequest::Go(1));
        assert_eq!(path(&history), ("/settings".into(), true, true));
        let settings = history.snapshot();
        history.apply(AppUrlRequest::Go(-1));
        history.apply(AppUrlRequest::Go(1));
        assert_eq!(history.snapshot().entry_key, settings.entry_key);
        history.apply(AppUrlRequest::Go(5));
        assert_eq!(
            path(&history),
            ("/settings".into(), true, true),
            "out of range does nothing"
        );
        history.apply(AppUrlRequest::Go(-1));
        history.apply(AppUrlRequest::Push("/archived".into()));
        assert_eq!(history.snapshot().history_index, settings.history_index);
        assert_ne!(history.snapshot().entry_key, settings.entry_key);
        assert_eq!(
            path(&history),
            ("/archived".into(), true, false),
            "forward entries are gone"
        );
        let pushed_key=history.snapshot().entry_key;
        history.apply(AppUrlRequest::Replace("/archived?x=1".into()));
        assert_eq!(history.snapshot().history_index, 1);
        assert_ne!(history.snapshot().entry_key,pushed_key);
        let seen: Vec<_> = std::iter::from_fn(|| changes.next().now_or_never().flatten())
            .map(|snapshot| snapshot.unwrap().path)
            .collect();
        assert_eq!(
            seen,
            [
                "/settings",
                "/usage",
                "/",
                "/settings",
                "/",
                "/settings",
                "/",
                "/archived",
                "/archived?x=1"
            ]
        );
    }

    #[test]
    fn closing_a_modal_goes_back_then_pushes_and_a_launch_link_is_the_first_entry() {
        let mut history = MemoryHistory::new(Some("/e/env/t/thread".into()));
        history.apply(AppUrlRequest::Push("/settings".into()));
        history.apply(AppUrlRequest::Push("/settings/keybindings".into()));
        history.apply(AppUrlRequest::CloseModal(CloseModal::GoThenPush {
            delta: -2,
            href: "/".into(),
        }));
        assert_eq!(path(&history), ("/".into(), true, false));
        history.apply(AppUrlRequest::Go(-1));
        assert_eq!(history.snapshot().path, "/e/env/t/thread");
        let policy = RoutePolicy {
            secrets_enabled: true,
            ..RoutePolicy::default()
        };
        let mut changes = history.changes();
        let _ = changes.next().now_or_never();
        history.set_policy(policy);
        assert_eq!(
            changes
                .next()
                .now_or_never()
                .flatten()
                .unwrap()
                .unwrap()
                .policy,
            policy
        );
    }
}
