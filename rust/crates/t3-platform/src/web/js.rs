//! Small JS interop helpers: callback-backed futures that unregister themselves on drop.
use futures::channel::oneshot;
use js_sys::{Object, Reflect};
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
use t3_client::platform::PlatformError;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::EventTarget;

pub(crate) fn object(entries: &[(&str, JsValue)]) -> Result<Object, PlatformError> {
    let object = Object::new();
    for (key, value) in entries {
        Reflect::set(&object, &JsValue::from_str(key), value)
            .map_err(|_| PlatformError::InvalidData("JS object".into()))?;
    }
    Ok(object)
}

pub(crate) fn get(target: &JsValue, key: &str) -> JsValue {
    Reflect::get(target, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

/// Error names only: messages from browsers can echo URLs that carry tickets.
pub(crate) fn error_name(value: &JsValue) -> String {
    get(value, "name")
        .as_string()
        .unwrap_or_else(|| "Error".into())
}

pub(crate) type Listener = (
    EventTarget,
    &'static str,
    Closure<dyn FnMut(web_sys::Event)>,
);

/// An event name and the value its first firing resolves to.
pub(crate) type Outcome<T> = (&'static str, Box<dyn Fn(&web_sys::Event) -> T>);

/// Resolves with the first listed event to fire; listeners are removed when it resolves or drops.
pub(crate) struct FirstEvent<T> {
    receiver: oneshot::Receiver<T>,
    listeners: Vec<Listener>,
}
impl<T: 'static> FirstEvent<T> {
    pub(crate) fn new(
        target: &EventTarget,
        outcomes: Vec<Outcome<T>>,
    ) -> Result<Self, PlatformError> {
        let (sender, receiver) = oneshot::channel();
        let sender = Rc::new(RefCell::new(Some(sender)));
        let mut listeners = Vec::new();
        for (name, outcome) in outcomes {
            let sender = sender.clone();
            let callback =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    if let Some(sender) = sender.borrow_mut().take() {
                        let _ = sender.send(outcome(&event));
                    }
                });
            target
                .add_event_listener_with_callback(name, callback.as_ref().unchecked_ref())
                .map_err(|_| PlatformError::Unsupported("event listener".into()))?;
            listeners.push((target.clone(), name, callback));
        }
        Ok(Self {
            receiver,
            listeners,
        })
    }
}
impl<T> Future for FirstEvent<T> {
    type Output = Result<T, PlatformError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.receiver)
            .poll(cx)
            .map(|result| result.map_err(|_| PlatformError::Closed))
    }
}
impl<T> Drop for FirstEvent<T> {
    fn drop(&mut self) {
        remove_listeners(&mut self.listeners);
    }
}

pub(crate) fn remove_listeners(listeners: &mut Vec<Listener>) {
    for (target, name, callback) in listeners.drain(..) {
        let _ = target.remove_event_listener_with_callback(name, callback.as_ref().unchecked_ref());
    }
}

/// A persistent listener owned by the platform; removed with [`remove_listeners`].
pub(crate) fn listen(
    target: &EventTarget,
    name: &'static str,
    handler: impl FnMut(web_sys::Event) + 'static,
) -> Result<Listener, PlatformError> {
    let callback = Closure::<dyn FnMut(web_sys::Event)>::new(handler);
    target
        .add_event_listener_with_callback(name, callback.as_ref().unchecked_ref())
        .map_err(|_| PlatformError::Unsupported("event listener".into()))?;
    Ok((target.clone(), name, callback))
}

pub(crate) type Listeners = Vec<Listener>;
