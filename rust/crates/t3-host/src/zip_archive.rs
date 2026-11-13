use crate::{Error, Result};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Seek},
    path::{Component, Path},
};
use zip::{CompressionMethod, ZipArchive};

#[derive(Debug, Clone, Copy)]
pub struct ArchiveLimits {
    pub compressed_bytes: u64,
    pub entries: usize,
    pub entry_bytes: u64,
    pub expanded_bytes: u64,
}
pub struct Archive<R: Read + Seek> {
    zip: ZipArchive<R>,
    limits: ArchiveLimits,
    total: u64,
    seen: BTreeSet<String>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
    pub mode: Option<u32>,
    pub bytes: Vec<u8>,
}
impl Archive<File> {
    pub fn open(path: &Path, limits: ArchiveLimits) -> Result<Self> {
        let file = File::open(path).map_err(|_| Error::Io)?;
        if file.metadata().map_err(|_| Error::Io)?.len() > limits.compressed_bytes {
            return Err(Error::Refused("archive exceeds compressed size limit"));
        }
        Self::new(file, limits)
    }
}
impl<R: Read + Seek> Archive<R> {
    pub fn new(mut reader: R, limits: ArchiveLimits) -> Result<Self> {
        let position = reader.stream_position().map_err(|_| Error::Io)?;
        let size = reader
            .seek(std::io::SeekFrom::End(0))
            .map_err(|_| Error::Io)?;
        if size > limits.compressed_bytes {
            return Err(Error::Refused("archive exceeds compressed size limit"));
        }
        reader
            .seek(std::io::SeekFrom::Start(position))
            .map_err(|_| Error::Io)?;
        let zip =
            ZipArchive::new(reader).map_err(|_| Error::Refused("archive could not be read"))?;
        if zip.has_duplicate_names() {
            return Err(Error::Refused("archive repeats a member"));
        }
        if zip.len() > limits.entries {
            return Err(Error::Refused("archive exceeds entry count limit"));
        }
        Ok(Self {
            zip,
            limits,
            total: 0,
            seen: BTreeSet::new(),
        })
    }
    pub fn entry_count(&self) -> usize {
        self.zip.len()
    }
    pub fn read_entry(&mut self, index: usize) -> Result<Entry> {
        let mut file = self
            .zip
            .by_index(index)
            .map_err(|_| Error::Refused("archive member could not be read"))?;
        let name = file
            .name()
            .map_err(|_| Error::Refused("archive member name could not be decoded"))?
            .into_owned();
        if !safe_name(&name)
            || file.enclosed_name().is_none()
            || file.encrypted()
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
            || file.is_symlink()
            || file
                .unix_mode()
                .is_some_and(|m| !matches!(m & 0o170000, 0 | 0o100000 | 0o040000))
        {
            return Err(Error::Refused("archive contains an unsafe member"));
        }
        if !self.seen.insert(name.clone()) {
            return Err(Error::Refused("archive repeats a member"));
        }
        let next = self
            .total
            .checked_add(file.size())
            .ok_or(Error::Refused("archive size overflow"))?;
        if file.size() > self.limits.entry_bytes || next > self.limits.expanded_bytes {
            return Err(Error::Refused("archive exceeds expanded size limit"));
        }
        let mut bytes = Vec::new();
        let size = file.size();
        let mode = file.unix_mode();
        let directory = file.is_dir();
        file.by_ref()
            .take(size.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Refused("archive member is corrupt"))?;
        if bytes.len() as u64 != size {
            return Err(Error::Refused("archive member size mismatch"));
        }
        self.total = next;
        Ok(Entry {
            name,
            directory,
            mode,
            bytes,
        })
    }
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', '\0', ':'])
        && !name.starts_with('/')
        && name.split('/').all(|p| p != "..")
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}
