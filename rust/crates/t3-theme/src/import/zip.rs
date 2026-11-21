//! A bounded reader for extension packages (`.vsix` files are ZIP archives).
//!
//! Open VSX packages come from third parties, so the directory is checked before
//! anything is inflated: entry count, total and per-entry size, compression ratio,
//! ZIP64 and path escapes. Pure and allocation-bounded; no I/O.

use std::collections::BTreeMap;

use miniz_oxide::inflate::{TINFLStatus, decompress_to_vec_with_limit};

use super::openvsx::MAX_PACKAGE_PATH_LENGTH;

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

fn invalid() -> PackageError {
    error("That extension package has an invalid ZIP directory.")
}

fn slice(bytes: &[u8], at: usize, length: usize) -> Result<&[u8], PackageError> {
    bytes
        .get(at..at.checked_add(length).ok_or_else(invalid)?)
        .ok_or_else(invalid)
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], PackageError> {
    let (head, tail) = bytes.split_at_checked(length).ok_or_else(invalid)?;
    *bytes = tail;
    Ok(head)
}

fn u16_at(bytes: &[u8], at: usize) -> Result<usize, PackageError> {
    Ok(usize::from(u16::from_le_bytes(
        slice(bytes, at, 2)?.try_into().map_err(|_| invalid())?,
    )))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, PackageError> {
    Ok(u32::from_le_bytes(
        slice(bytes, at, 4)?.try_into().map_err(|_| invalid())?,
    ))
}

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;

struct Directory<'a> {
    local: &'a [u8],
    central: &'a [u8],
    count: usize,
    eocd_offset: usize,
}

fn directory(bytes: &[u8]) -> Result<Directory<'_>, PackageError> {
    let start = bytes.len().saturating_sub(22 + usize::from(u16::MAX));
    let tail = bytes.get(start..).ok_or_else(invalid)?;
    for (index, header) in tail.windows(22).enumerate().rev() {
        if u32_at(header, 0)? != EOCD_SIGNATURE {
            continue;
        }
        let offset = start.checked_add(index).ok_or_else(invalid)?;
        let mut comment = bytes.get(offset..).ok_or_else(invalid)?;
        take(&mut comment, 22)?;
        if comment.len() != u16_at(header, 20)? {
            continue;
        }
        let count = u16_at(header, 10)?;
        if count > MAX_ZIP_ENTRIES {
            return Err(error("That extension package has too many files."));
        }
        if u16_at(header, 4)? != 0 || u16_at(header, 6)? != 0 || u16_at(header, 8)? != count {
            return Err(invalid());
        }
        let before = bytes.get(..offset).ok_or_else(invalid)?;
        let (local, central) = before
            .split_at_checked(u32_at(header, 16)? as usize)
            .ok_or_else(invalid)?;
        if central.len() != u32_at(header, 12)? as usize || count > central.len() / 46 {
            return Err(invalid());
        }
        return Ok(Directory {
            local,
            central,
            count,
            eocd_offset: offset,
        });
    }
    Err(error("That extension package has no ZIP directory."))
}

#[derive(Debug, Clone)]
struct Record {
    method: usize,
    uncompressed: u32,
    data: std::ops::Range<usize>,
}

fn extra_fields(mut bytes: &[u8]) -> Result<(), PackageError> {
    while !bytes.is_empty() {
        let header = take(&mut bytes, 4)?;
        take(&mut bytes, u16_at(header, 2)?)?;
    }
    Ok(())
}

fn records(directory: &Directory<'_>) -> Result<BTreeMap<String, Record>, PackageError> {
    let mut entries = BTreeMap::new();
    let mut central = directory.central;
    let mut total = 0u64;
    for _ in 0..directory.count {
        let header = take(&mut central, 46)?;
        if u32_at(header, 0)? != CENTRAL_SIGNATURE || u16_at(header, 34)? != 0 {
            return Err(invalid());
        }
        let compressed = u32_at(header, 20)?;
        let uncompressed = u32_at(header, 24)?;
        if compressed == u32::MAX || uncompressed == u32::MAX {
            return Err(error(
                "That extension package has unsupported ZIP64 metadata.",
            ));
        }
        total = total
            .checked_add(u64::from(uncompressed))
            .ok_or_else(invalid)?;
        if total > MAX_UNCOMPRESSED_BYTES {
            return Err(error(
                "That extension package expands beyond the safe import limit.",
            ));
        }
        if u64::from(uncompressed) > u64::from(compressed) * MAX_COMPRESSION_RATIO {
            return Err(error(
                "That extension package has an unsafe compression ratio.",
            ));
        }
        let name = take(&mut central, u16_at(header, 28)?)?;
        extra_fields(take(&mut central, u16_at(header, 30)?)?)?;
        take(&mut central, u16_at(header, 32)?)?;
        let path = std::str::from_utf8(name).map_err(|_| invalid())?;
        if path.is_empty()
            || path.encode_utf16().count() > MAX_PACKAGE_PATH_LENGTH
            || path.starts_with('/')
            || path.contains(['\\', '\0'])
            || path.as_bytes().get(1) == Some(&b':')
        {
            return Err(error("Theme path is not a safe relative package path."));
        }
        if path.split('/').any(|segment| segment == "..") {
            return Err(error("Theme path escapes the extension package."));
        }
        let mut local = directory
            .local
            .get(u32_at(header, 42)? as usize..)
            .ok_or_else(invalid)?;
        let local_header = take(&mut local, 30)?;
        let method = u16_at(header, 10)?;
        if u32_at(local_header, 0)? != LOCAL_SIGNATURE
            || u16_at(local_header, 8)? != method
            || take(&mut local, u16_at(local_header, 26)?)? != name
        {
            return Err(invalid());
        }
        extra_fields(take(&mut local, u16_at(local_header, 28)?)?)?;
        let start = directory
            .local
            .len()
            .checked_sub(local.len())
            .ok_or_else(invalid)?;
        take(&mut local, compressed as usize)?;
        let end = directory
            .local
            .len()
            .checked_sub(local.len())
            .ok_or_else(invalid)?;
        if entries
            .insert(
                path.to_owned(),
                Record {
                    method,
                    uncompressed,
                    data: start..end,
                },
            )
            .is_some()
        {
            return Err(invalid());
        }
    }
    if !central.is_empty() {
        return Err(invalid());
    }
    Ok(entries)
}

/// Validate the directory and local ranges, then remove the untrusted ZIP comment.
pub fn inspect_zip_directory(bytes: &[u8]) -> Result<Vec<u8>, PackageError> {
    let directory = directory(bytes)?;
    records(&directory)?;
    let mut stripped = bytes
        .get(..directory.eocd_offset)
        .ok_or_else(invalid)?
        .to_vec();
    stripped.extend_from_slice(slice(bytes, directory.eocd_offset, 20)?);
    stripped.extend_from_slice(&[0, 0]);
    Ok(stripped)
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
        let directory = directory(bytes)?;
        let entries = records(&directory)?;
        Ok(Self {
            bytes: directory.local.to_vec(),
            entries,
        })
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
        let data = self.bytes.get(record.data.clone()).ok_or_else(unreadable)?;
        let raw = match record.method {
            0 if data.len() > MAX_THEME_BYTES => {
                return Err(PackageError(format!("{description} is too large.")));
            }
            0 if data.len() == record.uncompressed as usize => data.to_vec(),
            0 => return Err(unreadable()),
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
        if raw.len() != record.uncompressed as usize {
            return Err(unreadable());
        }
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }
}
