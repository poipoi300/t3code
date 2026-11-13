use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub(crate) fn open_regular(path: &Path) -> io::Result<File> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(exceeded());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT avoids following a replaced link.
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(exceeded());
    }
    Ok(file)
}

pub(crate) const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_SCAN_BYTES: u64 = 1024 * 1024 * 1024;
pub(crate) const MAX_SCAN_RETAINED_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_DEPTH: usize = 64;
pub(crate) const MAX_ENTRIES: usize = 20_000;
pub(crate) const MAX_RECORDS: usize = 20_000;
pub(crate) const MAX_RETAINED_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_STATE_BYTES: usize = 1024 * 1024;

pub(crate) fn exceeded() -> io::Error {
    io::Error::other("Usage input exceeds safety budget")
}

pub(crate) fn read_document(path: &Path) -> io::Result<serde_json::Value> {
    let file = open_regular(path)?;
    let length = file.metadata()?.len();
    if length > 64 * 1024 * 1024 {
        return Err(exceeded());
    }
    let mut bytes = Vec::new();
    file.take(length).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length || !json_shape_ok(&bytes, 2_000_000) {
        return Err(exceeded());
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid usage state document"))
}

pub(crate) struct ScanBudget {
    bytes: AtomicU64,
    retained: AtomicU64,
    deadline: Instant,
}
impl Default for ScanBudget {
    fn default() -> Self {
        Self::new(MAX_SCAN_BYTES, MAX_SCAN_RETAINED_BYTES)
    }
}
impl ScanBudget {
    pub(crate) fn new(bytes: u64, retained: u64) -> Self {
        Self {
            bytes: AtomicU64::new(bytes),
            retained: AtomicU64::new(retained),
            deadline: Instant::now() + Duration::from_secs(30),
        }
    }
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(crate) fn charge(&self, bytes: u64) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(exceeded());
        }
        self.bytes
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                n.checked_sub(bytes)
            })
            .map(|_| ())
            .map_err(|_| exceeded())
    }
    pub(crate) fn retain(&self, bytes: usize) -> io::Result<()> {
        self.retained
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                n.checked_sub(bytes as u64)
            })
            .map(|_| ())
            .map_err(|_| exceeded())
    }
}

pub(crate) struct BudgetReader<'a, R> {
    pub(crate) input: R,
    pub(crate) remaining: u64,
    pub(crate) budget: &'a ScanBudget,
}
impl<R: Read> Read for BudgetReader<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let limit = out.len().min(self.remaining.saturating_add(1) as usize);
        let n = self.input.read(&mut out[..limit])?;
        if n as u64 > self.remaining {
            return Err(exceeded());
        }
        self.budget.charge(n as u64)?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

pub(crate) fn json_depth_ok(bytes: &[u8]) -> bool {
    json_shape_ok(bytes, MAX_ENTRIES)
}
pub(crate) fn json_shape_ok(bytes: &[u8], max_nodes: usize) -> bool {
    let (mut depth, mut string, mut escaped) = (0usize, false, false);
    let mut nodes = 0;
    for &b in bytes {
        if string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                string = false;
            }
        } else if b == b'"' {
            string = true;
        } else if b == b'{' || b == b'[' {
            nodes += 1;
            depth += 1;
            if depth > MAX_DEPTH {
                return false;
            }
        } else if b == b',' || b == b':' {
            nodes += 1;
        } else if b == b'}' || b == b']' {
            depth = depth.saturating_sub(1);
        }
        if nodes > max_nodes {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::io::Write;

    #[test]
    fn a_scan_budget_is_shared_across_files_and_workers() {
        let budget = ScanBudget::new(10, MAX_SCAN_RETAINED_BYTES);
        std::thread::scope(|scope| {
            scope.spawn(|| budget.charge(6).unwrap()).join().unwrap();
            scope
                .spawn(|| assert!(budget.charge(5).is_err()))
                .join()
                .unwrap();
        });
        assert!(budget.charge(4).is_ok());
        assert!(budget.charge(1).is_err());
    }

    #[test]
    fn retained_records_share_a_scan_limit_across_transcripts() {
        use crate::{UsageProviderKind, reader::read_transcript_bounded};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        fs::write(&path, b"{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T00:00:00Z\",\"message\":{\"model\":\"fixture\",\"usage\":{\"input_tokens\":1}}}\n").unwrap();
        let first = read_transcript_bounded(
            &path,
            &UsageProviderKind::Claude,
            None,
            &ScanBudget::default(),
        )
        .unwrap();
        let retained = serde_json::to_vec(&first.records).unwrap().len();
        let budget = ScanBudget::new(MAX_SCAN_BYTES, retained as u64 + 16);
        assert!(
            read_transcript_bounded(&path, &UsageProviderKind::Claude, None, &budget).is_some()
        );
        assert!(
            read_transcript_bounded(&path, &UsageProviderKind::Claude, None, &budget).is_none()
        );
    }

    #[test]
    fn decompressed_size_is_bounded_independently_of_compressed_size() {
        let budget = ScanBudget::default();
        let mut reader = BudgetReader {
            input: std::io::repeat(b'x'),
            remaining: 32,
            budget: &budget,
        };
        let mut output = Vec::new();
        assert!(reader.read_to_end(&mut output).is_err());
        assert!(output.len() <= 32);
    }

    #[test]
    fn growing_files_stop_at_the_initial_snapshot_length() {
        struct GrowingFile {
            input: std::io::Take<File>,
            append: File,
        }
        impl Read for GrowingFile {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                let n = self.input.read(out)?;
                self.append.write_all(&out[..n])?;
                Ok(n)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("growing.jsonl");
        fs::write(&path, b"usage\n".repeat(1024)).unwrap();
        let input = open_regular(&path).unwrap();
        let size = input.metadata().unwrap().len();
        let append = OpenOptions::new().append(true).open(&path).unwrap();
        let budget = ScanBudget::default();
        let mut reader = BudgetReader {
            input: GrowingFile {
                input: input.take(size),
                append,
            },
            remaining: MAX_FILE_BYTES,
            budget: &budget,
        };
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert_eq!(output.len() as u64, size);
        assert_eq!(fs::metadata(&path).unwrap().len(), size * 2);
    }
}
