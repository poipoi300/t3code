//! A bounded reader for extension packages (`.vsix` files are ZIP archives).
//!
//! Open VSX packages come from third parties, so the directory is checked before
//! anything is inflated: entry count, total and per-entry size, compression ratio,
//! ZIP64 and path escapes. Pure and allocation-bounded; no I/O.

use std::collections::BTreeMap;

use miniz_oxide::inflate::{TINFLStatus, decompress_to_vec_with_limit};

use super::openvsx::{MAX_PACKAGE_PATH_LENGTH, normalize_package_path};

pub const MAX_ZIP_ENTRIES: usize = 5_000;
pub const MAX_UNCOMPRESSED_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_COMPRESSION_RATIO: u64 = 200;
pub const MAX_THEME_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PackageError(pub String);

fn error(message: &str) -> PackageError {
    PackageError(message.to_owned())
}

fn u16_at(bytes: &[u8], at: usize) -> Option<usize> {
    bytes
        .get(at..at + 2)
        .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;

struct Eocd {
    offset: usize,
    directory_offset: usize,
    directory_end: usize,
    comment_length: usize,
}

fn find_eocd(bytes: &[u8]) -> Result<Eocd, PackageError> {
    let minimum = bytes.len().saturating_sub(65_557);
    let mut offset = bytes
        .len()
        .checked_sub(22)
        .ok_or_else(|| error("That extension package has no ZIP directory."))?;
    loop {
        if offset < minimum {
            return Err(error("That extension package has no ZIP directory."));
        }
        let comment = u16_at(bytes, offset + 20);
        if u32_at(bytes, offset) == Some(EOCD_SIGNATURE)
            && comment.is_some_and(|c| offset + 22 + c == bytes.len())
        {
            break;
        }
        match offset.checked_sub(1) {
            Some(next) => offset = next,
            None => return Err(error("That extension package has no ZIP directory.")),
        }
    }
    let invalid = || error("That extension package has an invalid ZIP directory.");
    let directory_size = u32_at(bytes, offset + 12).ok_or_else(invalid)? as usize;
    let directory_offset = u32_at(bytes, offset + 16).ok_or_else(invalid)? as usize;
    let directory_end = directory_offset + directory_size;
    if directory_end != offset || directory_end > bytes.len() {
        return Err(invalid());
    }
    Ok(Eocd {
        offset,
        directory_offset,
        directory_end,
        comment_length: u16_at(bytes, offset + 20).ok_or_else(invalid)?,
    })
}

/// One central-directory record.
#[derive(Debug, Clone)]
struct Record {
    name: String,
    method: u16,
    compressed: u32,
    uncompressed: u32,
    local_offset: usize,
}

/// `inspectZipDirectory`: validates the central directory and returns the archive with
/// any trailing comment removed, because a comment can imitate an end-of-directory record.
pub fn inspect_zip_directory(bytes: &[u8]) -> Result<Vec<u8>, PackageError> {
    let eocd = find_eocd(bytes)?;
    read_records(bytes, &eocd)?;
    if eocd.comment_length == 0 {
        return Ok(bytes.to_vec());
    }
    let mut stripped = bytes[..eocd.offset + 22].to_vec();
    stripped[eocd.offset + 20] = 0;
    stripped[eocd.offset + 21] = 0;
    Ok(stripped)
}

fn read_records(bytes: &[u8], eocd: &Eocd) -> Result<Vec<Record>, PackageError> {
    let invalid = || error("That extension package has an invalid ZIP directory.");
    let mut records = Vec::new();
    let mut total: u64 = 0;
    let mut offset = eocd.directory_offset;
    while offset < eocd.directory_end {
        if offset + 46 > eocd.directory_end || u32_at(bytes, offset) != Some(CENTRAL_SIGNATURE) {
            return Err(invalid());
        }
        if records.len() + 1 > MAX_ZIP_ENTRIES {
            return Err(error("That extension package has too many files."));
        }
        let compressed = u32_at(bytes, offset + 20).ok_or_else(invalid)?;
        let uncompressed = u32_at(bytes, offset + 24).ok_or_else(invalid)?;
        if compressed == 0xffff_ffff || uncompressed == 0xffff_ffff {
            return Err(error(
                "That extension package has unsupported ZIP64 metadata.",
            ));
        }
        total += u64::from(uncompressed);
        if total > MAX_UNCOMPRESSED_BYTES {
            return Err(error(
                "That extension package expands beyond the safe import limit.",
            ));
        }
        if uncompressed > 0
            && (compressed == 0
                || f64::from(uncompressed) / f64::from(compressed) > MAX_COMPRESSION_RATIO as f64)
        {
            return Err(error(
                "That extension package has an unsafe compression ratio.",
            ));
        }
        let name_length = u16_at(bytes, offset + 28).ok_or_else(invalid)?;
        let extra_length = u16_at(bytes, offset + 30).ok_or_else(invalid)?;
        let comment_length = u16_at(bytes, offset + 32).ok_or_else(invalid)?;
        let name_bytes = bytes
            .get(offset + 46..offset + 46 + name_length)
            .ok_or_else(invalid)?;
        records.push(Record {
            name: String::from_utf8_lossy(name_bytes).into_owned(),
            method: u16_at(bytes, offset + 10).ok_or_else(invalid)? as u16,
            compressed,
            uncompressed,
            local_offset: u32_at(bytes, offset + 42).ok_or_else(invalid)? as usize,
        });
        offset += 46 + name_length + extra_length + comment_length;
    }
    if offset != eocd.directory_end {
        return Err(invalid());
    }
    Ok(records)
}

/// JSZip's path sanitiser: drops `.` segments and interior empty ones, applies `..`.
fn sanitize(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    let mut result: Vec<&str> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if *part == "." || (part.is_empty() && index != 0 && index != parts.len() - 1) {
            continue;
        }
        if *part == ".." {
            result.pop();
        } else {
            result.push(part);
        }
    }
    result.join("/")
}

/// A checked archive whose entries can be read by sanitised path.
#[derive(Debug)]
pub struct ZipArchive {
    bytes: Vec<u8>,
    entries: BTreeMap<String, Record>,
}

impl ZipArchive {
    /// `inspectZipDirectory` + `JSZip.loadAsync` + `inspectZip`.
    pub fn open(bytes: &[u8]) -> Result<Self, PackageError> {
        let bytes = inspect_zip_directory(bytes)?;
        let eocd = find_eocd(&bytes)?;
        let records = read_records(&bytes, &eocd)
            .map_err(|_| error("That Open VSX extension package could not be opened."))?;
        let mut entries = BTreeMap::new();
        for record in records {
            let safe = sanitize(&record.name);
            if safe != record.name {
                if record.name.len() > MAX_PACKAGE_PATH_LENGTH {
                    return Err(error("Theme path is not a safe relative package path."));
                }
                normalize_package_path(&record.name, "extension/")
                    .map_err(|e| PackageError(e.0))?;
            }
            entries.insert(safe, record);
        }
        if entries.len() > MAX_ZIP_ENTRIES {
            return Err(error("That extension package has too many files."));
        }
        Ok(Self { bytes, entries })
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// `readZipText`: the entry as UTF-8 text, refusing anything over 256 KiB.
    pub fn read_text(&self, path: &str, description: &str) -> Result<String, PackageError> {
        let record = self.entries.get(path).ok_or_else(|| {
            PackageError(format!(
                "{description} is missing from the extension package."
            ))
        })?;
        if record.uncompressed as usize > MAX_THEME_BYTES {
            return Err(PackageError(format!("{description} is too large.")));
        }
        let unreadable =
            || PackageError("That Open VSX extension package could not be opened.".into());
        let header = record.local_offset;
        if u32_at(&self.bytes, header) != Some(LOCAL_SIGNATURE) {
            return Err(unreadable());
        }
        let name_length = u16_at(&self.bytes, header + 26).ok_or_else(unreadable)?;
        let extra_length = u16_at(&self.bytes, header + 28).ok_or_else(unreadable)?;
        let start = header + 30 + name_length + extra_length;
        let data = self
            .bytes
            .get(start..start + record.compressed as usize)
            .ok_or_else(unreadable)?;
        let raw = match record.method {
            0 => data.to_vec(),
            8 => decompress_to_vec_with_limit(data, MAX_THEME_BYTES).map_err(|failure| {
                if failure.status == TINFLStatus::HasMoreOutput {
                    PackageError(format!("{description} is too large."))
                } else {
                    unreadable()
                }
            })?,
            _ => return Err(unreadable()),
        };
        if raw.len() > MAX_THEME_BYTES {
            return Err(PackageError(format!("{description} is too large.")));
        }
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }
}
