use std::{fs, io, path::Path};

use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::{Error, Result, migrations};

/// Copies a sibling `state.sqlite` via SQLite backup; the source is always read-only.
pub fn initialize_v2_database(destination: impl AsRef<Path>) -> Result<()> {
    initialize(destination.as_ref(), false)
}

pub(crate) fn initialize_supported_v2_database(destination: &Path) -> Result<()> {
    initialize(destination, true)
}

fn initialize(destination: &Path, require_supported: bool) -> Result<()> {
    let directory = import_directory(destination);
    let source = directory.join("state.sqlite");
    // Startup cleanup is best-effort, including when V2 already exists.
    cleanup_abandoned_copies(directory, |error| {
        eprintln!(
            "Could not remove abandoned V2 database copies in {}: {error}",
            directory.display()
        )
    });
    let import = || -> Result<()> {
        if destination.try_exists()? || !source.try_exists()? {
            return Ok(());
        }
        let temporary = tempfile::Builder::new()
            .prefix(&format!(".v2-import-{}-", std::process::id()))
            .rand_bytes(6)
            .tempdir_in(directory)?;
        let snapshot = temporary.path().join("snapshot.sqlite");
        let source = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if require_supported {
            let head = migrations::history(&source)?.last().map_or(0, |row| row.id);
            if head < migrations::MINIMUM_SUPPORTED_HEAD {
                return Err(Error::UnsupportedHead {
                    head,
                    minimum: migrations::MINIMUM_SUPPORTED_HEAD,
                });
            }
        }
        let mut copy = Connection::open(&snapshot)?;
        {
            let backup = Backup::new(&source, &mut copy)?;
            finish_backup(backup.step(-1)?)?;
        }
        copy.close().map_err(|(_, error)| error)?;
        source.close().map_err(|(_, error)| error)?;
        publish_snapshot(&snapshot, destination)?;
        Ok(())
    };
    import().map_err(|cause| Error::Import {
        source_path: source,
        destination_path: destination.to_owned(),
        cause: Box::new(cause),
    })
}

pub(crate) fn finish_backup(step: StepResult) -> Result<()> {
    match step {
        StepResult::Done => Ok(()),
        StepResult::Busy => {
            Err(rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(5), None).into())
        }
        StepResult::Locked => {
            Err(rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(6), None).into())
        }
        _ => Err(Error::IncompleteBackup),
    }
}

pub(crate) fn import_directory(destination: &Path) -> &Path {
    destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub(crate) fn cleanup_abandoned_copies(directory: &Path, warn: impl FnOnce(io::Error)) {
    if let Err(error) = remove_abandoned_copies(directory) {
        warn(error);
    }
}

pub(crate) fn publish_snapshot(snapshot: &Path, destination: &Path) -> Result<()> {
    match fs::hard_link(snapshot, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(pid).is_some()
}

fn remove_abandoned_copies(directory: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name
            .to_str()
            .and_then(|name| name.strip_prefix(".v2-import-"))
        else {
            continue;
        };
        let (pid, suffix) = match name.split_once('-') {
            Some((pid, suffix)) if !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) => {
                (Some(pid), suffix)
            }
            None => (None, name),
            _ => continue,
        };
        if suffix.len() != 6 || !suffix.bytes().all(|b| b.is_ascii_alphanumeric()) {
            continue;
        }
        if let Some(pid) = pid {
            // PIDs outside the platform range cannot refer to a live process.
            if pid.parse::<u32>().is_ok_and(process_alive) {
                continue;
            }
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}
