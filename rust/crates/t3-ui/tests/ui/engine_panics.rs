//! Fails a test when the engine caught a panic while it ran. NSH catches panics in bindings,
//! effects, timers and tasks and only logs them ("a prop read or written after its owner was
//! disposed"), so a test passed while the app it drove was broken. Hold the guard for the test:
//! `let _panics = engine_panics::guard();` before starting the engine.

use std::cell::RefCell;
use std::sync::Once;

thread_local! {
    /// The panics on this test's thread since its guard was taken (`None`: no guard).
    static CAUGHT: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

#[must_use = "the guard checks when it is dropped, at the end of the test"]
pub struct Guard(());

pub fn guard() -> Guard {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // `try_with`: a panic while the thread's locals are torn down has nowhere to go.
            let _ = CAUGHT.try_with(|caught| {
                if let Ok(mut caught) = caught.try_borrow_mut()
                    && let Some(caught) = caught.as_mut()
                {
                    caught.push(info.to_string());
                }
            });
            previous(info);
        }));
    });
    CAUGHT.with(|caught| *caught.borrow_mut() = Some(Vec::new()));
    Guard(())
}

impl Drop for Guard {
    fn drop(&mut self) {
        let caught = CAUGHT
            .with(|caught| caught.borrow_mut().take())
            .unwrap_or_default();
        // A failing assertion is a panic too; the test already fails with its own message.
        if !std::thread::panicking() && !caught.is_empty() {
            panic!(
                "the engine caught {} panic(s) while the test ran:\n{}",
                caught.len(),
                caught.join("\n")
            );
        }
    }
}
