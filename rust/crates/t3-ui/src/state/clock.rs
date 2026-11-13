//! A shared wall clock for relative labels ("5m"). It ticks twice a minute; nothing repaints
//! between ticks, and a label whose text did not change stays as it is.

use std::time::Duration;

use nsh_core::prelude::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Now(i64);

/// Starts the clock under the current owner (`App`).
pub fn start() {
    let now = global::<Now>();
    now.set(Now(read()));
    set_interval(Duration::from_secs(30), move || now.set(Now(read())));
}

fn read() -> i64 {
    nsh_core::time::unix_ms() as i64
}

/// Epoch milliseconds; reading it subscribes to the next tick.
pub fn now_ms() -> i64 {
    try_global::<Now>().map_or(0, |now| now.get().0)
}
