use crate::{
    Error,
    safety::{self, MAX_FILE_BYTES, MAX_LINE_BYTES, ScanBudget},
};
use rusqlite::{Connection, OpenFlags, limits::Limit};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    ops::Deref,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) struct Snapshot {
    connection: Connection,
    pub(crate) fingerprint: String,
    pub(crate) main_fingerprint: String,
    _directory: tempfile::TempDir,
}
impl Deref for Snapshot {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}

fn side(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}
fn stamp(path: &Path) -> std::io::Result<Option<(u64, std::time::SystemTime)>> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => Ok(Some((m.len(), m.modified()?))),
        Ok(_) => Err(safety::exceeded()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
pub(crate) fn connect(path: &Path) -> Result<Snapshot, Error> {
    connect_bounded(path, &ScanBudget::default())
}
pub(crate) fn connect_bounded(path: &Path, budget: &ScanBudget) -> Result<Snapshot, Error> {
    let directory = tempfile::tempdir()?;
    let copy = directory.path().join("source.db");
    let source_paths = [path.to_path_buf(), side(path, "-wal")];
    let before = source_paths
        .iter()
        .map(|p| stamp(p))
        .collect::<Result<Vec<_>, _>>()?;
    if before[0].is_none() || stamp(&side(path, "-journal"))?.is_some_and(|(len, _)| len > 0) {
        return Err(safety::exceeded().into());
    }
    let total = before.iter().flatten().try_fold(0u64, |sum, (len, _)| {
        sum.checked_add(*len).ok_or_else(safety::exceeded)
    })?;
    if total > MAX_FILE_BYTES {
        return Err(safety::exceeded().into());
    }
    let mut hash = Sha256::new();
    let mut main_fingerprint = String::new();
    for (index, source) in source_paths.iter().enumerate() {
        let Some((length, _)) = before[index] else {
            hash.update([0]);
            continue;
        };
        hash.update([1]);
        hash.update(length.to_le_bytes());
        let mut input = safety::open_regular(source)?.take(length);
        let destination = if index == 0 {
            copy.clone()
        } else {
            side(&copy, "-wal")
        };
        let mut output = fs::File::create(destination)?;
        let mut buffer = [0; 64 * 1024];
        let mut copied = 0;
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            budget.charge(n as u64)?;
            hash.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
            copied += n as u64;
        }
        if index == 0 {
            main_fingerprint = format!(
                "{:?}:{:x}",
                file_id::get_file_id(path).ok(),
                hash.clone().finalize()
            );
        }
        if copied != length {
            return Err(safety::exceeded().into());
        }
    }
    let after = source_paths
        .iter()
        .map(|p| stamp(p))
        .collect::<Result<Vec<_>, _>>()?;
    if before != after || stamp(&side(path, "-journal"))?.is_some_and(|(len, _)| len > 0) {
        return Err(safety::exceeded().into());
    }
    // Recovery, locks and WAL/SHM files belong exclusively to the private copy.
    let connection = Connection::open_with_flags(
        &copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::ZERO)?;
    connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_LINE_BYTES as i32)?;
    connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA cache_size=-2048; PRAGMA temp_store=MEMORY;")?;
    let deadline = (Instant::now() + Duration::from_secs(5)).min(budget.deadline());
    let mut steps = 0u64;
    connection.progress_handler(
        1000,
        Some(move || {
            steps += 1000;
            steps > 10_000_000 || Instant::now() >= deadline
        }),
    )?;
    Ok(Snapshot {
        connection,
        fingerprint: format!("{:x}", hash.finalize()),
        main_fingerprint,
        _directory: directory,
    })
}
