//! The T3 home the server owns: who holds it, and the pid its runtime file names.
//! Ports `DesktopHomeOwner.ts` (`isHomeOwnershipLocked`, `readLiveHomeOwner`, `homeOwnedMessage`).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use t3_config::{
    home::HomeLayout,
    runtime::{RuntimeState, read_live_runtime_state, read_runtime_state},
};

/// The layout of an explicit home (`--home-dir`/`T3CODE_HOME`): state below `userdata`.
pub fn layout(base_dir: &Path) -> HomeLayout {
    HomeLayout::derive(base_dir, false, true)
}

/// Whether another server holds the ownership lock (`isHomeOwnershipLocked`): only SQLITE_BUSY
/// counts. A missing file or any other error leaves the decision to the server.
pub fn is_locked(layout: &HomeLayout) -> bool {
    let Ok(connection) = Connection::open_with_flags(
        layout.ownership_database(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return false;
    };
    let result = connection
        .execute_batch("PRAGMA busy_timeout = 0")
        .and_then(|()| {
            connection.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                row.get::<_, i64>(0)
            })
        });
    matches!(
        result,
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::DatabaseBusy
    )
}

/// The pid the runtime file names, if the file is there and valid.
pub fn runtime_pid(layout: &HomeLayout) -> Option<i64> {
    read_runtime_state(&layout.runtime_state()).map(|state| state.pid)
}

/// The live server recorded as owner (`readLiveHomeOwner`). Only meaningful once the lock is known
/// to be held: a live pid alone may be reused.
pub fn live_owner(layout: &HomeLayout) -> Option<RuntimeState> {
    read_live_runtime_state(&layout.runtime_state())
}

/// `homeOwnedMessage`, word for word.
pub fn owned_message(owner: Option<&RuntimeState>) -> String {
    let Some(owner) = owner else {
        return "Another T3 Code server, such as the background service, is using this data directory, or an older background service is set up for it. Stop that server or try `t3 update`. If the older launcher cannot update, run `t3 service uninstall` and optionally reinstall with `t3 service install` from a current CLI. Use `npx t3@latest` if `t3` is missing or outdated, and pass `--base-dir` with the service's T3 home to each command. Then reopen T3 Code. You can also set a separate T3CODE_HOME and pair with the running server.".to_owned();
    };
    let managed = owner.service_managed == Some(true);
    let kind = if managed {
        "The T3 Code background service"
    } else {
        "Another T3 Code server (for example `t3 serve`, or one started over SSH)"
    };
    let version = owner
        .server_version
        .as_deref()
        .filter(|version| !version.is_empty())
        .map_or_else(String::new, |version| format!(", version {version}"));
    let stop = if managed {
        "Stop it with `t3 service uninstall`, passing `--base-dir` with this T3 home"
    } else {
        "If a T3 Code app on another machine started it over SSH, that app restarts it: remove or disconnect that SSH environment there, or quit that app. Then stop the server"
    };
    format!(
        "{kind} is using this T3 home (PID {}, port {}{version}). {stop}, and reopen T3 Code. You can also set a separate T3CODE_HOME and pair with the running server.",
        owner.pid, owner.port
    )
}
