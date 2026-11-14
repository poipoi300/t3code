//! A test switch: with `T3_UI_ENGINE_PANICS=fail` in the environment, a panic the engine caught
//! fails the test that caused it, with its message.
//!
//! NSH catches a panic in a binding, effect, timer or task ("a prop read or written after its
//! owner was disposed"), reports it as `log::error!(target: "js", ..)` from its frame loop, and
//! carries on natively; on the web the same panic kills the page. This logger panics on that
//! report, on the test's own thread and outside the engine's catch, so the headless call that
//! surfaced it fails. Without the variable nothing is installed. The app never sets it, and its
//! runners install their own logger first (`log::set_logger` succeeds once per process).

/// Called from [`crate::registries`], which every headless engine of the crate's tests is built
/// with.
pub(crate) fn install_from_environment() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            if std::env::var_os("T3_UI_ENGINE_PANICS").is_some_and(|value| value == "fail") {
                static LOGGER: FailOnEnginePanics = FailOnEnginePanics;
                if log::set_logger(&LOGGER).is_ok() {
                    log::set_max_level(log::LevelFilter::Error);
                }
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct FailOnEnginePanics;

#[cfg(not(target_arch = "wasm32"))]
impl log::Log for FailOnEnginePanics {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Error && metadata.target() == "js"
    }

    fn log(&self, record: &log::Record<'_>) {
        // While a test's own assertion unwinds, a second panic would abort the process.
        if self.enabled(record.metadata()) && !std::thread::panicking() {
            panic!("the engine caught a panic: {}", record.args());
        }
    }

    fn flush(&self) {}
}
