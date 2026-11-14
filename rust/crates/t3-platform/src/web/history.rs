//! The app's location on the web: the browser's own location and history, as TanStack's
//! browser history uses `__TSR_index` and `__TSR_key` in history.state, including entries
//! written by TypeScript. Route logic stays in `t3_client::navigation`.
use super::js;
use futures::{StreamExt, channel::mpsc, channel::oneshot, stream::LocalBoxStream};
use std::{cell::RefCell, rc::Rc};
use t3_client::{
    navigation::{AppUrlRequest, AppUrlSnapshot, CloseModal, RoutePolicy},
    platform::PlatformError,
};
use wasm_bindgen::JsValue;
use web_sys::Window;

const INDEX_KEY: &str = "__TSR_index";
const ENTRY_KEY: &str = "__TSR_key";

fn random_key() -> Result<String, PlatformError> {
    let encoded: String = js_sys::Number::from(js_sys::Math::random() + 1.0)
        .to_string_with_radix(36)
        .map_err(history_error)?
        .into();
    Ok(encoded.chars().skip(7).collect())
}

fn keyed_state(index: u64, key: &str) -> Result<js_sys::Object, PlatformError> {
    js::object(&[
        (INDEX_KEY, JsValue::from_f64(index as f64)),
        (ENTRY_KEY, JsValue::from_str(key)),
        ("key", JsValue::from_str(key)),
    ])
}

#[derive(Default)]
struct State {
    /// The furthest entry this tab knows is ahead of the first (pushed or traversed to).
    forward_limit: u64,
    policy: RoutePolicy,
    listeners: Vec<mpsc::UnboundedSender<Result<AppUrlSnapshot, PlatformError>>>,
    traversal: Option<oneshot::Sender<()>>,
}

pub(crate) struct BrowserHistory {
    window: Window,
    state: Rc<RefCell<State>>,
}

fn history_error(_: JsValue) -> PlatformError {
    PlatformError::Unsupported("browser history".into())
}

fn index(window: &Window) -> u64 {
    window
        .history()
        .ok()
        .and_then(|history| history.state().ok())
        .and_then(|state| js::get(&state, INDEX_KEY).as_f64())
        .filter(|index| *index >= 0.0)
        .map_or(0, |index| index as u64)
}

fn path(window: &Window) -> String {
    let location = window.location();
    format!(
        "{}{}{}",
        location.pathname().unwrap_or_else(|_| "/".into()),
        location.search().unwrap_or_default(),
        location.hash().unwrap_or_default()
    )
}

fn snapshot(window: &Window, state: &State) -> AppUrlSnapshot {
    let index = index(window);
    AppUrlSnapshot {
        path: path(window),
        history_index: index,
        entry_key: window
            .history()
            .ok()
            .and_then(|history| history.state().ok())
            .and_then(|state| js::get(&state, ENTRY_KEY).as_string()),
        can_go_back: index > 0,
        can_go_forward: index < state.forward_limit,
        policy: state.policy,
    }
}

fn notify(window: &Window, state: &mut State) {
    let current = snapshot(window, state);
    state
        .listeners
        .retain(|listener| listener.unbounded_send(Ok(current.clone())).is_ok());
}

impl BrowserHistory {
    /// Listens for back and forward; the returned listener must live as long as the history.
    pub(crate) fn new(window: Window) -> Result<(Self, js::Listener), PlatformError> {
        let history = window.history().map_err(history_error)?;
        let previous = history.state().map_err(history_error)?;
        let present_key = js::get(&previous, ENTRY_KEY)
            .as_string()
            .filter(|key| !key.is_empty());
        if present_key.is_none() || js::get(&previous, INDEX_KEY).is_undefined() {
            let key = match present_key.or_else(|| {
                js::get(&previous, "key")
                    .as_string()
                    .filter(|key| !key.is_empty())
            }) {
                Some(key) => key,
                None => random_key()?,
            };
            let entry = js_sys::Object::new();
            if previous.is_object() {
                let _ = js_sys::Object::assign(&entry, &js_sys::Object::from(previous));
            }
            let _ = js_sys::Object::assign(&entry, &keyed_state(index(&window), &key)?);
            history.replace_state(&entry, "").map_err(history_error)?;
        }
        let state = Rc::new(RefCell::new(State {
            forward_limit: index(&window),
            ..State::default()
        }));
        let observed = window.clone();
        let shared = state.clone();
        let listener = js::listen(&window, "popstate", move |_| {
            let mut state = shared.borrow_mut();
            let reached = index(&observed);
            state.forward_limit = state.forward_limit.max(reached);
            notify(&observed, &mut state);
            if let Some(traversal) = state.traversal.take() {
                let _ = traversal.send(());
            }
        })?;
        Ok((Self { window, state }, listener))
    }

    pub(crate) fn launch_url(&self) -> Option<String> {
        self.window.location().href().ok()
    }

    pub(crate) fn changes(&self) -> LocalBoxStream<'static, Result<AppUrlSnapshot, PlatformError>> {
        let (sender, receiver) = mpsc::unbounded();
        let mut state = self.state.borrow_mut();
        let _ = sender.unbounded_send(Ok(snapshot(&self.window, &state)));
        state.listeners.push(sender);
        receiver.boxed_local()
    }

    pub(crate) fn set_policy(&self, policy: RoutePolicy) {
        let mut state = self.state.borrow_mut();
        if state.policy != policy {
            state.policy = policy;
            notify(&self.window, &mut state);
        }
    }

    fn write(&self, href: &str, push: bool) -> Result<(), PlatformError> {
        let history = self.window.history().map_err(history_error)?;
        let current = index(&self.window);
        let next = if push { current + 1 } else { current };
        let entry = keyed_state(next, &random_key()?)?;
        if push {
            history.push_state_with_url(&entry, "", Some(href))
        } else {
            history.replace_state_with_url(&entry, "", Some(href))
        }
        .map_err(history_error)?;
        let mut state = self.state.borrow_mut();
        if push {
            // A push discards the entries that were ahead.
            state.forward_limit = next;
        }
        notify(&self.window, &mut state);
        Ok(())
    }

    /// Traverses and settles when the browser reports the new entry; out of range does nothing.
    async fn go(&self, delta: i64) -> Result<(), PlatformError> {
        let target = index(&self.window) as i64 + delta;
        let limit = self.state.borrow().forward_limit as i64;
        if delta == 0 || !(0..=limit).contains(&target) {
            return Ok(());
        }
        let (sender, receiver) = oneshot::channel();
        self.state.borrow_mut().traversal = Some(sender);
        let delta =
            i32::try_from(delta).map_err(|_| PlatformError::InvalidData("history step".into()))?;
        self.window
            .history()
            .map_err(history_error)?
            .go_with_delta(delta)
            .map_err(history_error)?;
        receiver.await.map_err(|_| PlatformError::Cancelled)
    }

    pub(crate) async fn apply(&self, request: AppUrlRequest) -> Result<(), PlatformError> {
        match request {
            AppUrlRequest::Push(href) | AppUrlRequest::CloseModal(CloseModal::Push(href)) => {
                self.write(&href, true)
            }
            AppUrlRequest::Replace(href) => self.write(&href, false),
            AppUrlRequest::Go(delta) => self.go(delta).await,
            AppUrlRequest::CloseModal(CloseModal::GoThenPush { delta, href }) => {
                self.go(delta).await?;
                self.write(&href, true)
            }
        }
    }
}
