//! Engine panics as test failures.
//!
//! NSH catches a panic in a binding, effect, timer or task ("a prop read or written after its
//! owner was disposed"), reports it as `log::error!(target: "js", ..)` from its frame loop, and
//! carries on natively; on the web the same panic kills the page. The logger here panics on that
//! report, on the engine's thread and outside the engine's catch, so the headless call that
//! surfaced it fails, with the engine's message printed above the test's own.
//!
//! - `T3_UI_ENGINE_PANICS=fail` in the environment turns it on for every engine in the process
//!   (read by [`crate::registries`], which every headless engine of the crate's tests is built
//!   with).
//! - [`strict`], called in a test fixture's root component, turns it on for that engine alone.
//!
//! The app never does either. Its runners install their own logger first, and
//! `log::set_logger` succeeds once per process.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_arch = "wasm32"))]
static EVERY_ENGINE: AtomicBool = AtomicBool::new(false);

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// This engine's thread fails on its caught panics ([`strict`]).
    static THIS_ENGINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// For tests: the engine this is called on (from a fixture's root component, which runs on the
/// engine's thread) fails on any panic it catches, whatever the environment says.
pub fn strict() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        install();
        THIS_ENGINE.with(|strict| strict.set(true));
    }
}

/// Called from [`crate::registries`].
pub(crate) fn install_from_environment() {
    #[cfg(not(target_arch = "wasm32"))]
    if std::env::var_os("T3_UI_ENGINE_PANICS").is_some_and(|value| value == "fail") {
        EVERY_ENGINE.store(true, Ordering::Relaxed);
        install();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        static LOGGER: FailOnEnginePanics = FailOnEnginePanics;
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Error);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
struct FailOnEnginePanics;

#[cfg(not(target_arch = "wasm32"))]
impl log::Log for FailOnEnginePanics {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Error && metadata.target() == "js"
    }

    fn log(&self, record: &log::Record<'_>) {
        let on = EVERY_ENGINE.load(Ordering::Relaxed) || THIS_ENGINE.with(std::cell::Cell::get);
        // While a panic already unwinds, a second one would abort the process.
        if on && self.enabled(record.metadata()) && !std::thread::panicking() {
            panic!("the engine caught a panic: {}", record.args());
        }
    }

    fn flush(&self) {}
}
