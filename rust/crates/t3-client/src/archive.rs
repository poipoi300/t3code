//! Bounded, in-memory ZIP inspection; paths never reach a host filesystem.
use crate::platform::Platform;
use futures::FutureExt;
use std::collections::{BTreeMap, BTreeSet};

pub const ARCHIVE_MAX_BYTES: usize = 50 * 1024 * 1024;
pub const ARCHIVE_ENTRY_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const ARCHIVE_MAX_ENTRIES: usize = 2048;
pub const ARCHIVE_MAX_RATIO: u32 = 200;
pub const XML_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveLimits {
    pub compressed_bytes: usize,
    pub expanded_bytes: usize,
    pub entries: usize,
    pub name_bytes: usize,
    pub expansion_ratio: u32,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            compressed_bytes: 50 * 1024 * 1024,
            expanded_bytes: 50 * 1024 * 1024,
            entries: 2048,
            name_bytes: 1024,
            expansion_ratio: 200,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReadOptions {
    pub limits: ArchiveLimits,
    pub entry_bytes: usize,
    pub xml_bytes: usize,
    pub ratio_floor: usize,
    pub markup_limit: Option<usize>,
}
impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            limits: ArchiveLimits::default(),
            entry_bytes: ARCHIVE_MAX_BYTES,
            xml_bytes: XML_MAX_BYTES,
            ratio_floor: 0,
            markup_limit: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArchiveError {
    #[error("This is not a readable ZIP archive.")]
    Damaged,
    #[error("The expanded archive exceeds the preview size or compression ratio limit.")]
    Limit,
    #[error("This archive contains more than 2,048 entries.")]
    Entries,
    #[error("An archive path is too long to preview.")]
    Name,
    #[error("Archive path is unsafe or duplicated.")]
    UnsafePath,
    #[error("Unsupported ZIP compression or encryption.")]
    Unsupported,
    #[error("Overlapping ZIP entries.")]
    Overlap,
    #[error("ZIP checksum or size does not match its directory.")]
    Integrity,
}
type Result<T> = std::result::Result<T, ArchiveError>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub bytes: Vec<u8>,
    pub directory: bool,
    pub dos_date: u16,
    pub dos_time: u16,
}
#[derive(Debug)]
struct Record<'a> {
    name: &'a str,
    compressed: &'a [u8],
    expanded: usize,
    method: u16,
    crc: u32,
    date: u16,
    time: u16,
}
fn bytes_at(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    bytes
        .get(offset..offset.checked_add(length).ok_or(ArchiveError::Damaged)?)
        .ok_or(ArchiveError::Damaged)
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    let b = bytes_at(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    let b = bytes_at(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<usize> {
    let b = bytes_at(bytes, offset, 8)?;
    usize::try_from(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
    .map_err(|_| ArchiveError::Damaged)
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['/', '\\'])
        && !name.contains(['\\', ':', '\0'])
        && !name.split('/').any(|part| part == ".." || part == ".")
}
fn directory<'a>(bytes: &'a [u8], limits: &ArchiveLimits) -> Result<Vec<Record<'a>>> {
    if bytes.len() > limits.compressed_bytes.min(ARCHIVE_MAX_BYTES) {
        return Err(ArchiveError::Limit);
    }
    let end = (bytes.len().saturating_sub(65557)..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&i| u32_at(bytes, i) == Ok(0x06054b50))
        .ok_or(ArchiveError::Damaged)?;
    if end.checked_add(22 + u16_at(bytes, end + 20)? as usize) != Some(bytes.len()) {
        return Err(ArchiveError::Damaged);
    }
    let mut count = u16_at(bytes, end + 10)? as usize;
    let mut size = u32_at(bytes, end + 12)? as usize;
    let mut start = u32_at(bytes, end + 16)? as usize;
    let mut directory_end = end;
    if count == 0xffff || size == u32::MAX as usize || start == u32::MAX as usize {
        let locator = end.checked_sub(20).ok_or(ArchiveError::Damaged)?;
        let record = locator.checked_sub(56).ok_or(ArchiveError::Damaged)?;
        if u32_at(bytes, locator)? != 0x07064b50
            || u32_at(bytes, record)? != 0x06064b50
            || u64_at(bytes, record + 4)? != 44
            || u32_at(bytes, record + 16)? != 0
            || u32_at(bytes, record + 20)? != 0
            || u32_at(bytes, locator + 4)? != 0
            || u32_at(bytes, locator + 16)? != 1
            || u64_at(bytes, locator + 8)? != record
        {
            return Err(ArchiveError::Unsupported);
        }
        count = u64_at(bytes, record + 32)?;
        size = u64_at(bytes, record + 40)?;
        start = u64_at(bytes, record + 48)?;
        if u64_at(bytes, record + 24)? != count {
            return Err(ArchiveError::Damaged);
        }
        directory_end = record;
    } else if u16_at(bytes, end + 4)? != 0
        || u16_at(bytes, end + 6)? != 0
        || u16_at(bytes, end + 8)? as usize != count
    {
        return Err(ArchiveError::Unsupported);
    }
    if count > limits.entries.min(ARCHIVE_MAX_ENTRIES) {
        return Err(ArchiveError::Entries);
    }
    if start.checked_add(size) != Some(directory_end) {
        return Err(ArchiveError::Damaged);
    }
    let mut records = Vec::new();
    let mut ranges = Vec::new();
    let mut names = BTreeSet::new();
    let mut pos = start;
    let mut declared_total = 0usize;
    while pos < directory_end {
        if records.len() >= limits.entries.min(ARCHIVE_MAX_ENTRIES) {
            return Err(ArchiveError::Entries);
        }
        bytes_at(bytes, pos, 46)?;
        if u32_at(bytes, pos)? != 0x02014b50 {
            return Err(ArchiveError::Damaged);
        }
        let flags = u16_at(bytes, pos + 8)?;
        let method = u16_at(bytes, pos + 10)?;
        if flags & 0x2041 != 0 || ![0, 8].contains(&method) || u16_at(bytes, pos + 34)? != 0 {
            return Err(ArchiveError::Unsupported);
        }
        let compressed = u32_at(bytes, pos + 20)? as usize;
        let expanded = u32_at(bytes, pos + 24)? as usize;
        let name_length = u16_at(bytes, pos + 28)? as usize;
        if name_length > limits.name_bytes.min(1024) {
            return Err(ArchiveError::Name);
        }
        let name_raw = bytes_at(bytes, pos + 46, name_length)?;
        let name = std::str::from_utf8(name_raw).map_err(|_| ArchiveError::Name)?;
        if !safe_name(name) || !names.insert(name) {
            return Err(ArchiveError::UnsafePath);
        }
        let extra = u16_at(bytes, pos + 30)? as usize;
        let comment = u16_at(bytes, pos + 32)? as usize;
        let local = u32_at(bytes, pos + 42)? as usize;
        if u32_at(bytes, local)? != 0x04034b50
            || u16_at(bytes, local + 6)? != flags
            || u16_at(bytes, local + 8)? != method
        {
            return Err(ArchiveError::Damaged);
        }
        let local_name = u16_at(bytes, local + 26)? as usize;
        let local_extra = u16_at(bytes, local + 28)? as usize;
        if local_name != name_length || bytes_at(bytes, local + 30, local_name)? != name_raw {
            return Err(ArchiveError::Integrity);
        }
        let data_start = local
            .checked_add(30 + local_name + local_extra)
            .ok_or(ArchiveError::Damaged)?;
        let data_end = data_start
            .checked_add(compressed)
            .ok_or(ArchiveError::Damaged)?;
        let mut range_end = data_end;
        if flags & 8 == 0 {
            if u32_at(bytes, local + 14)? != u32_at(bytes, pos + 16)?
                || u32_at(bytes, local + 18)? as usize != compressed
                || u32_at(bytes, local + 22)? as usize != expanded
            {
                return Err(ArchiveError::Integrity);
            }
        } else {
            let descriptor = if u32_at(bytes, data_end)? == 0x08074b50 {
                data_end + 4
            } else {
                data_end
            };
            if u32_at(bytes, descriptor)? != u32_at(bytes, pos + 16)?
                || u32_at(bytes, descriptor + 4)? as usize != compressed
                || u32_at(bytes, descriptor + 8)? as usize != expanded
            {
                return Err(ArchiveError::Integrity);
            }
            range_end = descriptor + 12;
        }
        if range_end > start {
            return Err(ArchiveError::Overlap);
        }
        declared_total = declared_total
            .checked_add(expanded)
            .ok_or(ArchiveError::Limit)?;
        if declared_total > limits.expanded_bytes.min(ARCHIVE_MAX_BYTES) {
            return Err(ArchiveError::Limit);
        }
        ranges.push((local, range_end));
        records.push(Record {
            name,
            compressed: bytes_at(bytes, data_start, compressed)?,
            expanded,
            method,
            crc: u32_at(bytes, pos + 16)?,
            date: u16_at(bytes, pos + 14)?,
            time: u16_at(bytes, pos + 12)?,
        });
        pos = pos
            .checked_add(46 + name_length + extra + comment)
            .ok_or(ArchiveError::Damaged)?;
        if pos > directory_end {
            return Err(ArchiveError::Damaged);
        }
    }
    if records.len() != count {
        return Err(ArchiveError::Integrity);
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|p| p[0].1 > p[1].0) {
        return Err(ArchiveError::Overlap);
    }
    Ok(records)
}
pub fn check_zip_directory(bytes: &[u8], limits: &ArchiveLimits) -> Result<()> {
    directory(bytes, limits).map(|_| ())
}
struct Budget {
    work: usize,
    checksum: u32,
    remaining: usize,
    markup: usize,
    markup_limit: Option<usize>,
}
impl Budget {
    fn push(&mut self, output: &mut Vec<u8>, byte: u8, cap: usize) -> Result<()> {
        if output.len() >= cap || self.remaining == 0 {
            return Err(ArchiveError::Limit);
        }
        if byte == b'<' {
            if self.markup_limit.is_some_and(|limit| self.markup >= limit) {
                return Err(ArchiveError::Limit);
            }
            self.markup += 1;
        }
        self.remaining -= 1;
        self.work += 1;
        self.checksum ^= byte as u32;
        for _ in 0..8 {
            self.checksum =
                (self.checksum >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(self.checksum & 1));
        }
        output.push(byte);
        Ok(())
    }
}
/// Synchronous inspection for small inputs; UI callers use `read_zip` to yield CPU work.
pub fn bounded_zip(bytes: &[u8], options: &ReadOptions) -> Result<BTreeMap<String, ArchiveEntry>> {
    read_inner(None, bytes, options)
        .now_or_never()
        .ok_or(ArchiveError::Unsupported)?
}
pub async fn read_zip(
    platform: &dyn Platform,
    bytes: &[u8],
    options: &ReadOptions,
) -> Result<BTreeMap<String, ArchiveEntry>> {
    read_inner(Some(platform), bytes, options).await
}
async fn checkpoint(platform: Option<&dyn Platform>, work: &mut usize) {
    *work += 1;
    if *work >= 16 * 1024 {
        *work = 0;
        if let Some(platform) = platform {
            platform.yield_to_host().await;
        }
    }
}
async fn read_inner(
    platform: Option<&dyn Platform>,
    bytes: &[u8],
    options: &ReadOptions,
) -> Result<BTreeMap<String, ArchiveEntry>> {
    let records = directory(bytes, &options.limits)?;
    let ratio = bytes
        .len()
        .max(1)
        .saturating_mul(options.limits.expansion_ratio.min(ARCHIVE_MAX_RATIO) as usize)
        .max(options.ratio_floor);
    let mut budget = Budget {
        work: 0,
        checksum: !0,
        remaining: options
            .limits
            .expanded_bytes
            .min(ARCHIVE_MAX_BYTES)
            .min(ratio),
        markup: 0,
        markup_limit: options.markup_limit,
    };
    let mut files = BTreeMap::new();
    for record in records {
        budget.checksum = !0;
        if let Some(platform) = platform {
            platform.yield_to_host().await;
        }
        let cap =
            options
                .entry_bytes
                .min(ARCHIVE_MAX_BYTES)
                .min(if record.name.ends_with(".xml") {
                    options.xml_bytes
                } else {
                    ARCHIVE_MAX_BYTES
                });
        if record.expanded > cap || record.expanded > budget.remaining {
            return Err(ArchiveError::Limit);
        }
        // Directory sizes are untrusted; check each actual byte before growing output.
        let mut output = Vec::new();
        if record.method == 0 {
            for &byte in record.compressed {
                budget.push(&mut output, byte, cap)?;
                checkpoint(platform, &mut budget.work).await;
            }
        } else {
            inflate(record.compressed, &mut output, &mut budget, cap, platform).await?;
        }
        if output.len() != record.expanded || !budget.checksum != record.crc {
            return Err(ArchiveError::Integrity);
        }
        files.insert(
            record.name.to_owned(),
            ArchiveEntry {
                bytes: output,
                directory: record.name.ends_with('/'),
                dos_date: record.date,
                dos_time: record.time,
            },
        );
    }
    Ok(files)
}

struct Bits<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl Bits<'_> {
    fn read(&mut self, count: usize) -> Result<usize> {
        let mut value = 0;
        for shift in 0..count {
            let byte = *self
                .bytes
                .get(self.position / 8)
                .ok_or(ArchiveError::Damaged)?;
            value |= ((byte as usize >> (self.position % 8)) & 1) << shift;
            self.position += 1;
        }
        Ok(value)
    }
}
struct Huffman {
    counts: [usize; 16],
    symbols: Vec<usize>,
}
impl Huffman {
    fn new(lengths: &[usize]) -> Result<Self> {
        let mut counts = [0; 16];
        for &length in lengths {
            if length > 15 {
                return Err(ArchiveError::Damaged);
            }
            counts[length] += 1;
        }
        let mut left = 1isize;
        for count in counts.iter().skip(1) {
            left = left * 2 - *count as isize;
            if left < 0 {
                return Err(ArchiveError::Damaged);
            }
        }
        let mut symbols = Vec::new();
        for length in 1..=15 {
            for (symbol, &n) in lengths.iter().enumerate() {
                if n == length {
                    symbols.push(symbol);
                }
            }
        }
        Ok(Self { counts, symbols })
    }
    fn decode(&self, bits: &mut Bits<'_>) -> Result<usize> {
        let (mut code, mut first, mut index) = (0, 0, 0);
        for length in 1..=15 {
            code |= bits.read(1)?;
            let count = self.counts[length];
            if code >= first && code - first < count {
                return self
                    .symbols
                    .get(index + code - first)
                    .copied()
                    .ok_or(ArchiveError::Damaged);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(ArchiveError::Damaged)
    }
}
async fn inflate(
    bytes: &[u8],
    output: &mut Vec<u8>,
    budget: &mut Budget,
    cap: usize,
    platform: Option<&dyn Platform>,
) -> Result<()> {
    let mut bits = Bits { bytes, position: 0 };
    loop {
        let block_start = bits.position;
        let last = bits.read(1)? != 0;
        match bits.read(2)? {
            0 => {
                bits.position = bits.position.div_ceil(8) * 8;
                let length = bits.read(16)?;
                if length ^ bits.read(16)? != 0xffff {
                    return Err(ArchiveError::Damaged);
                }
                for _ in 0..length {
                    budget.push(output, bits.read(8)? as u8, cap)?;
                    checkpoint(platform, &mut budget.work).await;
                }
            }
            block @ (1 | 2) => {
                let (literals, distances) = if block == 1 {
                    let mut lens = [8; 288];
                    lens[144..256].fill(9);
                    lens[256..280].fill(7);
                    (Huffman::new(&lens)?, Huffman::new(&[5; 32])?)
                } else {
                    let literals = bits.read(5)? + 257;
                    let distances = bits.read(5)? + 1;
                    let count = bits.read(4)? + 4;
                    let order = [
                        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                    ];
                    let mut lens = [0; 19];
                    for &i in &order[..count] {
                        lens[i] = bits.read(3)?;
                    }
                    let codes = Huffman::new(&lens)?;
                    let mut lens = Vec::new();
                    while lens.len() < literals + distances {
                        let symbol = codes.decode(&mut bits)?;
                        let (value, count) = match symbol {
                            0..=15 => (symbol, 1),
                            16 => (
                                *lens.last().ok_or(ArchiveError::Damaged)?,
                                bits.read(2)? + 3,
                            ),
                            17 => (0, bits.read(3)? + 3),
                            18 => (0, bits.read(7)? + 11),
                            _ => return Err(ArchiveError::Damaged),
                        };
                        if lens.len() + count > literals + distances {
                            return Err(ArchiveError::Damaged);
                        }
                        lens.extend(std::iter::repeat_n(value, count));
                    }
                    if lens[256] == 0 {
                        return Err(ArchiveError::Damaged);
                    }
                    (
                        Huffman::new(&lens[..literals])?,
                        Huffman::new(&lens[literals..])?,
                    )
                };
                loop {
                    let symbol = literals.decode(&mut bits)?;
                    match symbol {
                        0..=255 => budget.push(output, symbol as u8, cap)?,
                        256 => break,
                        257..=285 => {
                            const BASE: [usize; 29] = [
                                3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43,
                                51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
                            ];
                            const EXTRA: [usize; 29] = [
                                0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4,
                                4, 4, 5, 5, 5, 5, 0,
                            ];
                            const DB: [usize; 30] = [
                                1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257,
                                385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289,
                                16385, 24577,
                            ];
                            const DE: [usize; 30] = [
                                0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9,
                                10, 10, 11, 11, 12, 12, 13, 13,
                            ];
                            let length = BASE[symbol - 257] + bits.read(EXTRA[symbol - 257])?;
                            let symbol = distances.decode(&mut bits)?;
                            if symbol >= DB.len() {
                                return Err(ArchiveError::Damaged);
                            }
                            let distance = DB[symbol] + bits.read(DE[symbol])?;
                            if distance > output.len() {
                                return Err(ArchiveError::Damaged);
                            }
                            for _ in 0..length {
                                let byte = output[output.len() - distance];
                                budget.push(output, byte, cap)?;
                            }
                        }
                        _ => return Err(ArchiveError::Damaged),
                    }
                    checkpoint(platform, &mut budget.work).await;
                }
            }
            _ => return Err(ArchiveError::Damaged),
        }
        budget.work = budget
            .work
            .saturating_add((bits.position - block_start).div_ceil(8));
        checkpoint(platform, &mut budget.work).await;
        if last {
            break;
        }
    }
    if bits.position.div_ceil(8) != bytes.len() {
        return Err(ArchiveError::Integrity);
    }
    Ok(())
}
#[cfg(test)]
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::platform::*;
    use futures::{future::LocalBoxFuture, stream::LocalBoxStream};
    #[derive(Default)]
    struct YieldPlatform {
        calls: std::cell::Cell<usize>,
    }
    fn unsupported<'a, T>() -> HostFuture<'a, T> {
        async { Err(PlatformError::Unsupported("unused test capability".into())) }.boxed_local()
    }
    impl Storage for YieldPlatform {
        fn get(&self, _: StorageArea, _: StorageKey) -> HostFuture<'_, Option<Vec<u8>>> {
            unsupported()
        }
        fn write(&self, _: StorageArea, _: Vec<StorageWrite>) -> HostFuture<'_, ()> {
            unsupported()
        }
        fn list(&self, _: StorageArea, _: String) -> HostFuture<'_, Vec<StorageKey>> {
            unsupported()
        }
    }
    impl Platform for YieldPlatform {
        fn now_epoch_ms(&self) -> u64 {
            0
        }
        fn monotonic_ms(&self) -> u64 {
            0
        }
        fn random_bytes(&self, _: usize) -> std::result::Result<Vec<u8>, PlatformError> {
            Err(PlatformError::Unsupported("random".into()))
        }
        fn sleep(&self, _: u64) -> LocalBoxFuture<'_, ()> {
            std::future::pending().boxed_local()
        }
        fn yield_to_host(&self) -> LocalBoxFuture<'_, ()> {
            self.calls.set(self.calls.get() + 1);
            let mut yielded = false;
            futures::future::poll_fn(move |cx| {
                if yielded {
                    std::task::Poll::Ready(())
                } else {
                    yielded = true;
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            })
            .boxed_local()
        }
        fn events(&self) -> LocalBoxStream<'static, PlatformEvent> {
            use futures::StreamExt;
            futures::stream::empty().boxed_local()
        }
        fn http(&self, _: HttpRequest) -> HostFuture<'_, HttpResponse> {
            unsupported()
        }
        fn websocket(&self, _: SocketRequest) -> HostFuture<'_, Box<dyn WebSocket>> {
            unsupported()
        }
        fn host(&self, _: HostAction) -> HostFuture<'_, HostReply> {
            unsupported()
        }
    }
    fn put16(bytes: &mut [u8], at: usize, n: usize) {
        bytes[at..at + 2].copy_from_slice(&(n as u16).to_le_bytes());
    }
    fn put32(bytes: &mut [u8], at: usize, n: u32) {
        bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    fn zip(entries: &[(&str, &[u8], usize, u16, u32)]) -> Vec<u8> {
        let mut bytes = vec![];
        let mut central = vec![];
        for &(name, data, expanded, method, crc) in entries {
            let offset = bytes.len();
            let mut local = vec![0; 30];
            put32(&mut local, 0, 0x04034b50);
            put16(&mut local, 4, 20);
            put16(&mut local, 8, method as usize);
            put32(&mut local, 14, crc);
            put32(&mut local, 18, data.len() as u32);
            put32(&mut local, 22, expanded as u32);
            put16(&mut local, 26, name.len());
            bytes.extend(local);
            bytes.extend(name.as_bytes());
            bytes.extend(data);
            let mut record = vec![0; 46];
            put32(&mut record, 0, 0x02014b50);
            put16(&mut record, 6, 20);
            put16(&mut record, 10, method as usize);
            put32(&mut record, 16, crc);
            put32(&mut record, 20, data.len() as u32);
            put32(&mut record, 24, expanded as u32);
            put16(&mut record, 28, name.len());
            put32(&mut record, 42, offset as u32);
            central.extend(record);
            central.extend(name.as_bytes());
        }
        let mut end = vec![0; 22];
        put32(&mut end, 0, 0x06054b50);
        put16(&mut end, 8, entries.len());
        put16(&mut end, 10, entries.len());
        put32(&mut end, 12, central.len() as u32);
        put32(&mut end, 16, bytes.len() as u32);
        bytes.extend(central);
        bytes.extend(end);
        bytes
    }
    fn stored(name: &str, data: &[u8]) -> Vec<u8> {
        zip(&[(name, data, data.len(), 0, crc32(data))])
    }
    fn dynamic_bomb() -> Vec<u8> {
        let mut bytes = vec![
            237, 193, 49, 1, 0, 0, 0, 194, 160, 108, 235, 95, 202, 26, 30, 64, 1,
        ];
        bytes.extend([0; 96]);
        bytes.extend([175, 6]);
        bytes
    }
    #[test]
    fn reads_stored_fixed_and_dynamic_deflate_with_crc() {
        let options = ReadOptions::default();
        assert_eq!(
            bounded_zip(&stored("a.txt", b"hello"), &options).unwrap()["a.txt"].bytes,
            b"hello"
        );
        let fixed = [203, 72, 205, 201, 201, 87, 40, 207, 47, 202, 73, 1, 0];
        let archive = zip(&[("a.txt", &fixed, 11, 8, crc32(b"hello world"))]);
        assert_eq!(
            bounded_zip(&archive, &options).unwrap()["a.txt"].bytes,
            b"hello world"
        );
        let expanded = vec![b'A'; 100_000];
        let compressed = dynamic_bomb();
        let archive = zip(&[("a", &compressed, expanded.len(), 8, crc32(&expanded))]);
        let options = ReadOptions {
            ratio_floor: 100_000,
            ..ReadOptions::default()
        };
        assert_eq!(
            bounded_zip(&archive, &options).unwrap()["a"].bytes,
            expanded
        );
        let uncompressed_deflate = [1, 3, 0, 252, 255, b'a', b'b', b'c'];
        assert_eq!(
            bounded_zip(
                &zip(&[("s", &uncompressed_deflate, 3, 8, crc32(b"abc"))]),
                &options
            )
            .unwrap()["s"]
                .bytes,
            b"abc"
        );
    }
    #[test]
    fn rejects_bombs_and_lies_without_preallocating_from_sizes() {
        let compressed = dynamic_bomb();
        let bomb = zip(&[("a", &compressed, 100_000, 8, 0)]);
        assert_eq!(
            bounded_zip(&bomb, &ReadOptions::default()),
            Err(ArchiveError::Limit)
        );
        let lying = zip(&[("a", &compressed, 1, 8, 0)]);
        let options = ReadOptions {
            entry_bytes: 1024,
            ..ReadOptions::default()
        };
        assert_eq!(bounded_zip(&lying, &options), Err(ArchiveError::Limit));
        let mut bytes = stored("a", b"hello");
        put32(&mut bytes, 22, u32::MAX);
        assert_eq!(
            bounded_zip(&bytes, &ReadOptions::default()),
            Err(ArchiveError::Integrity)
        );
        let mut bytes = stored("a", b"hello");
        bytes[31] = b'x';
        assert_eq!(
            bounded_zip(&bytes, &ReadOptions::default()),
            Err(ArchiveError::Integrity)
        );
    }
    #[test]
    fn rejects_overlapping_entries_and_traversal_names() {
        let mut archive = zip(&[
            ("a", b"a", 1, 0, crc32(b"a")),
            ("b", b"b", 1, 0, crc32(b"b")),
        ]);
        let central = 64;
        put32(&mut archive, 18, 33);
        put32(&mut archive, 22, 33);
        put32(&mut archive, central + 20, 33);
        put32(&mut archive, central + 24, 33);
        assert_eq!(
            bounded_zip(&archive, &ReadOptions::default()),
            Err(ArchiveError::Overlap)
        );
        for name in [
            "../a",
            "a/../../b",
            "/tmp/a",
            "C:/a",
            "a\\..\\b",
            "a/./b",
            "a\0b",
        ] {
            assert_eq!(
                bounded_zip(&stored(name, b"x"), &ReadOptions::default()),
                Err(ArchiveError::UnsafePath),
                "{name}"
            );
        }
        assert_eq!(
            bounded_zip(
                &zip(&[("a", b"", 0, 0, 0), ("a", b"", 0, 0, 0)]),
                &ReadOptions::default()
            ),
            Err(ArchiveError::UnsafePath)
        );
    }
    #[test]
    fn enforces_name_entry_xml_total_and_markup_limits() {
        let options = ReadOptions::default();
        assert!(bounded_zip(&stored(&"n".repeat(1024), b""), &options).is_ok());
        assert_eq!(
            bounded_zip(&stored(&"n".repeat(1025), b""), &options),
            Err(ArchiveError::Name)
        );
        let entries = (0..2048).map(|i| format!("{i}")).collect::<Vec<_>>();
        let rows = entries
            .iter()
            .map(|n| (n.as_str(), &b""[..], 0, 0, 0))
            .collect::<Vec<_>>();
        assert!(bounded_zip(&zip(&rows), &options).is_ok());
        let mut too_many = rows.clone();
        too_many.push(("last", b"", 0, 0, 0));
        assert_eq!(
            bounded_zip(&zip(&too_many), &options),
            Err(ArchiveError::Entries)
        );
        assert_eq!(
            bounded_zip(&stored("part.xml", &vec![0; XML_MAX_BYTES + 1]), &options),
            Err(ArchiveError::Limit)
        );
        let options = ReadOptions {
            markup_limit: Some(1),
            ..ReadOptions::default()
        };
        assert_eq!(
            bounded_zip(&stored("arbitrary.bin", b"<<"), &options),
            Err(ArchiveError::Limit)
        );
        let options = ReadOptions {
            entry_bytes: 3,
            ..ReadOptions::default()
        };
        assert_eq!(
            bounded_zip(&stored("a", b"1234"), &options),
            Err(ArchiveError::Limit)
        );
    }
    #[test]
    fn truncated_and_damaged_directories_never_panic() {
        let archive = stored("a", b"hello");
        for n in 0..archive.len() {
            assert!(bounded_zip(&archive[..n], &ReadOptions::default()).is_err());
        }
        for i in 0..archive.len() {
            let mut bytes = archive.clone();
            bytes[i] ^= 255;
            let _ = bounded_zip(&bytes, &ReadOptions::default());
        }
    }
    #[test]
    fn asynchronous_reader_yields_inside_decompression() {
        let platform = YieldPlatform::default();
        let compressed = dynamic_bomb();
        let content = vec![b'A'; 100_000];
        let archive = zip(&[("a", &compressed, 100_000, 8, crc32(&content))]);
        let options = ReadOptions {
            ratio_floor: 100_000,
            ..ReadOptions::default()
        };
        let mut read = Box::pin(read_zip(&platform, &archive, &options));
        let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(std::future::Future::poll(read.as_mut(), &mut cx).is_pending());
        assert_eq!(
            futures::executor::block_on(read).unwrap()["a"].bytes,
            content
        );
        assert!(platform.calls.get() > 5);
    }
    #[test]
    fn reads_data_descriptors_and_small_zip64_directories() {
        let mut archive = stored("a", b"abc");
        let central = 34;
        put16(&mut archive, 6, 8);
        put16(&mut archive, central + 8, 8);
        let mut descriptor = vec![0; 16];
        put32(&mut descriptor, 0, 0x08074b50);
        put32(&mut descriptor, 4, crc32(b"abc"));
        put32(&mut descriptor, 8, 3);
        put32(&mut descriptor, 12, 3);
        archive.splice(central..central, descriptor);
        let end = archive.len() - 22;
        put32(&mut archive, end + 16, (central + 16) as u32);
        assert_eq!(
            bounded_zip(&archive, &ReadOptions::default()).unwrap()["a"].bytes,
            b"abc"
        );
        let archive = stored("a", b"abc");
        let end = archive.len() - 22;
        let mut zip64 = archive[..end].to_vec();
        let mut record = vec![0; 56];
        put32(&mut record, 0, 0x06064b50);
        put32(&mut record, 4, 44);
        put32(&mut record, 24, 1);
        put32(&mut record, 32, 1);
        put32(&mut record, 40, (end - central) as u32);
        put32(&mut record, 48, central as u32);
        zip64.extend(record);
        let mut locator = vec![0; 20];
        put32(&mut locator, 0, 0x07064b50);
        put32(&mut locator, 8, end as u32);
        put32(&mut locator, 16, 1);
        zip64.extend(locator);
        let mut tail = archive[end..].to_vec();
        put16(&mut tail, 8, 0xffff);
        put16(&mut tail, 10, 0xffff);
        put32(&mut tail, 12, u32::MAX);
        put32(&mut tail, 16, u32::MAX);
        zip64.extend(tail);
        assert_eq!(
            bounded_zip(&zip64, &ReadOptions::default()).unwrap()["a"].bytes,
            b"abc"
        );
    }
    #[test]
    fn deflate_matches_a_zlib_generated_corpus() {
        let vectors = [
            (
                "010101fefe07192d435b7591afcff1153b638db9e717497db3eb25619fdf2165abf33d89d72779cd237bd5318fef51b51b83ed59c737a91d930b85017fff81058b139d29b747d96d039b35d16f0fb155fba34df9a75709bd732be5a15f1fe1a56b33fdc99767390de3bb95714f2f11f5dbc3ad998777695d534b45413f3f41454b535d69778799adc3dbf5112f4f7195bbe30d396797c9fd336ba5e11f5fa1e52b73bd0957a7f94da3fb55b10f6fd1359b036dd947b7299d138b0581ff7f01850b931da937c759ed831bb551ef8f31d57b23cd7927d7893df3ab6521df9f6125ebb37d4917e7b98d633b15f1cfaf91755b432d1907f7e9ddd3cbc5c1bfbfc1c5cbd3dde9f707",
                257usize,
                0u8,
            ),
            (
                "6397d4758e2e9db8fefc4751ebe4de9dcfc53d6b37bf564d9c7f5f3175f567dbceebea956795abaf1af6bf0fdc2addfc36f2b8f94ad9c9dcad8cf5ff1b59bb85e76a6e77bf99cb3cdbf4623effc6d0df8b7d7f2e0fe7dc5bacfd7461bcfcc3a5d9c67f4f4e4fb7e47dbc7b6aa1bfbee0d7db87d7ce6c2fcf8c0df67675b4b77774f50e8ecd2c6f9fb9f6f0edaf82fafe8553773fe6b54c9f7ef2af71f6d287f2f10b9f6a17efe50c5ffed377f1efd08dfcf9174d6733e7de74dfae3957b89bb5f17f3d632bf764d995e6c723df364b6f0d7cdf6f78b55af96ca5faf54edbcfab5315efcf4f547dbdb9d653fcf9cede646bd18fe7d74f2c8d76d69564fffef2eee5d3470feedf7ff0e8e9cb775f7e671ff5ff68fc8fa6ffd1fc3f5afe8d96ffa3f5df68fd3fdafe196dff8db67f47dbffa3fd9fd1fedf68ff77b4ff3f3afe313afe333afe353afe373afe393afe3b3afe3d3afe3f3aff313aff333aff353aff373aff393aff3b0ce6bf01",
                8192usize,
                0u8,
            ),
            (
                "edcf8166020014004049922493249964264992244992249324932449922449669224499299249949922499244992244992244992244932339324c9f61ff3ee0f0e4d6649f5cfb1d2e8872834bfd677a4075fe5f3de945ed12c1f07516446f70cefbc534ef45b5da504bf743d7ee1368e0d21fcbf01d40b21c5a8c9e70e64923776e2cb9a4b5679ce6b31cd27e63663a4ae7376eeb5ff6615e0368d844bc5be392e3ac5f7b0db667854c82462b144a67834d8dce1f7626771bc61ab5c89c60627b0bef5af5c7b6e4d3566b6cca726469b3f2bb3174d19ef1cf39248c75c5e63a4082fa8c0af1f11c2c66f0bfc9eee2b48a9aabfa39ca9f76ee8a1cf22a2c38785b64a9bee3f2bbe07d2aefe6a16127f46a5d8b35eca22a34ffbe564d06db75aedee60b2dc9fd0f0873ffce10f7ff8c31ffef0873ffce10f7ff8c31ffef0873ffce10f7ff8c31ffef0873ffce10f7ff8ff83ff1f",
                8192usize,
                0u8,
            ),
            (
                "05c101010000008220cdff9f03000000000000000000000000505555555555555555555555d56ddbb66ddbb66ddbb66ddbb66ddbb66ddbb6adaaaaaaaaaaaaaaaaaaaaaaaa00000000000000000000000040550f",
                256usize,
                1u8,
            ),
            (
                "95c101010000080220cdff9fdb0084951d1b0b2b3b361656766c2cacecd85858d9b1b107",
                1000usize,
                1u8,
            ),
        ];
        for (hex, size, kind) in vectors {
            let compressed = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>();
            let expected = (0..size)
                .map(|i| {
                    if kind == 0 {
                        ((i * i + 17 * i + 7) % 256) as u8
                    } else {
                        ((i / 50) % 4) as u8
                    }
                })
                .collect::<Vec<_>>();
            let archive = zip(&[("part", &compressed, size, 8, crc32(&expected))]);
            assert_eq!(
                bounded_zip(&archive, &ReadOptions::default()).unwrap()["part"].bytes,
                expected
            );
        }
    }
}
