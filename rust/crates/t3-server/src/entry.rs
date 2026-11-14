//! The `t3` binary's entry. Its future, including the server's start-up and shutdown, runs on a
//! thread with a stated stack instead of the OS main thread, which has 1 MiB on Windows (the MSVC
//! default) and 8 MiB on Linux and macOS. Budget: README, "Start-up and shutdown".

/// The entry thread's stack, as on Linux and macOS. Tokio's workers keep their default.
pub const STACK_SIZE: usize = 8 * 1024 * 1024;

/// Runs `main`'s future on a multi-threaded runtime driven from a `STACK_SIZE` thread, and
/// returns its output; a panic in it resumes on the caller.
pub fn run<M, F>(main: M) -> std::io::Result<F::Output>
where
    M: FnOnce() -> F + Send + 'static,
    F: std::future::Future,
    F::Output: Send + 'static,
{
    let entry = std::thread::Builder::new()
        .name("t3-main".into())
        .stack_size(STACK_SIZE)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map(|runtime| runtime.block_on(main()))
        })?;
    match entry.join() {
        Ok(output) => output,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    /// Recurses with a 64 KiB frame; 48 levels need about 3 MiB, more than the 1 MiB main thread
    /// of Windows or the 2 MiB a default spawned thread gets, and well within `STACK_SIZE`.
    fn deep(depth: usize) -> u8 {
        let frame = std::hint::black_box([depth as u8; 64 * 1024]);
        if depth == 0 {
            frame[0]
        } else {
            deep(depth - 1).wrapping_add(frame[frame.len() - 1])
        }
    }

    #[test]
    fn the_entry_future_runs_on_the_sized_thread_with_a_runtime() {
        let (name, flavor, _) = super::run(|| async {
            (
                std::thread::current().name().map(str::to_owned),
                tokio::runtime::Handle::current().runtime_flavor(),
                deep(48),
            )
        })
        .unwrap();
        assert_eq!(name.as_deref(), Some("t3-main"));
        assert_eq!(flavor, tokio::runtime::RuntimeFlavor::MultiThread);
    }

    #[test]
    fn a_panic_in_the_entry_future_reaches_the_caller() {
        let caught = std::panic::catch_unwind(|| {
            super::run(|| async {
                if std::hint::black_box(true) {
                    panic!("entry failed");
                }
            })
            .unwrap();
        });
        assert!(caught.is_err());
    }
}
