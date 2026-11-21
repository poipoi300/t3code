//! Bounded reads of attachment files and private snapshots for consumers requiring a pathname.
use crate::{Error, Result, attachments::FILE_MAX_BYTES, io_error, scoped::ScopedDir};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub(crate) const TEMP_SNAPSHOT_PREFIX: &str = "t3-attachment-";

pub struct PinnedFiles {
    directory: PathBuf,
    pinned: OnceLock<ScopedDir>,
}

impl PinnedFiles {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            pinned: OnceLock::new(),
        }
    }

    fn scope(&self) -> Result<&ScopedDir> {
        if let Some(scope) = self.pinned.get() {
            return Ok(scope);
        }
        // Resolve the configured parent (which can be a platform alias) but open the
        // directory entry itself without following links. Keep this handle across deliveries.
        let parent = self.directory.parent().ok_or(Error::AssetDenied)?;
        let parent =
            crate::paths::canonicalize(parent).map_err(|e| io_error("open-parent", parent, e))?;
        let root = parent.join(self.directory.file_name().ok_or(Error::AssetDenied)?);
        let opened = ScopedDir::open_unlinked(&root)?;
        let _ = self.pinned.set(opened);
        self.pinned.get().ok_or(Error::AssetDenied)
    }

    pub fn child_directories(&self) -> Result<Vec<PathBuf>> {
        let scope = self.scope()?;
        let (_, entries) = scope.read_dir(Path::new(""))?;
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io_error("read-directory", &self.directory, e))?;
            if entry
                .file_type()
                .map_err(|e| io_error("stat-entry", &self.directory, e))?
                .is_dir()
            {
                result.push(self.directory.join(entry.file_name()));
            }
        }
        Ok(result)
    }

    pub fn open(&self, path: &Path) -> Result<File> {
        let relative = path
            .strip_prefix(&self.directory)
            .map_err(|_| Error::AssetDenied)?;
        self.scope()?.open_file(relative)
    }

    pub fn read(&self, path: &Path, limit: u64) -> Result<Vec<u8>> {
        let file = self.open(path)?;
        read_handle(&file, path, limit)
    }
}

fn read_handle(file: &File, path: &Path, limit: u64) -> Result<Vec<u8>> {
    let size = file
        .metadata()
        .map_err(|e| io_error("stat-file", path, e))?
        .len();
    if size > limit {
        return Err(Error::AssetDenied);
    }
    let mut bytes = Vec::new();
    file.take(size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| io_error("read-file", path, e))?;
    if bytes.len() as u64 != size {
        return Err(Error::AssetDenied);
    }
    Ok(bytes)
}

fn private_scope(pinned: &PinnedFiles) -> Result<&ScopedDir> {
    let scope = pinned.scope()?;
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        let mode = scope
            .dir
            .dir_metadata()
            .map_err(|e| io_error("stat-reference-directory", &pinned.directory, e))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(Error::AssetDenied);
        }
    }
    Ok(scope)
}

pub struct AttachmentDelivery {
    sources: PinnedFiles,
    durable: Option<PinnedFiles>,
    // Provider session snapshots live outside the directory accepting uploads.
    snapshots: OnceLock<(PinnedFiles, tempfile::TempDir)>,
}
impl AttachmentDelivery {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            sources: PinnedFiles::new(directory),
            durable: None,
            snapshots: OnceLock::new(),
        }
    }
    /// Question answers persist path references across server restarts. The host chooses
    /// a private state directory separate from the attachment sources.
    pub fn with_reference_directory(directory: PathBuf, references: PathBuf) -> Self {
        Self {
            durable: Some(PinnedFiles::new(references)),
            ..Self::new(directory)
        }
    }

    fn snapshots(&self) -> Result<&PinnedFiles> {
        if let Some(pinned) = &self.durable {
            if pinned.pinned.get().is_none() {
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
                match builder.create(&pinned.directory) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => {
                        return Err(io_error("create-reference-directory", &pinned.directory, e));
                    }
                }
            }
            private_scope(pinned)?;
            return Ok(pinned);
        }
        if self.snapshots.get().is_none() {
            let mut builder = tempfile::Builder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                builder.permissions(std::fs::Permissions::from_mode(0o700));
            }
            let dir = builder
                .prefix(TEMP_SNAPSHOT_PREFIX)
                .tempdir()
                .map_err(|e| io_error("create-reference-directory", std::env::temp_dir(), e))?;
            let pinned = PinnedFiles::new(dir.path().into());
            pinned.scope()?;
            let _ = self.snapshots.set((pinned, dir));
        }
        self.snapshots
            .get()
            .map(|(pinned, _)| pinned)
            .ok_or(Error::AssetDenied)
    }
    pub fn reference_directory(&self) -> Result<PathBuf> {
        Ok(self.snapshots()?.directory.clone())
    }

    /// A path-only consumer receives its own copy of the bytes read from the checked handle.
    pub fn reference(&self, original: &Path) -> Result<PathBuf> {
        let file = self.sources.open(original)?;
        let bytes = read_handle(&file, original, FILE_MAX_BYTES)?;
        let snapshots = self.snapshots()?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let leaf = original.file_name().ok_or(Error::AssetDenied)?;
        let relative = Path::new(leaf).join(format!("{digest}-{}", leaf.to_string_lossy()));
        let scope = snapshots.scope()?;
        if scope.open_file(&relative).is_err() {
            scope.write_atomic(&relative, &bytes)?;
        }
        // Rollback can run while a cancelled request is copying. In either order it
        // must leave no snapshot once the claimed source has been removed.
        let identity = crate::scoped::identity(&file)?;
        if !self
            .sources
            .open(original)
            .is_ok_and(|current| crate::scoped::identity(&current).is_ok_and(|now| now == identity))
        {
            let _ = scope.remove_file(&relative);
            let _ = scope.dir.remove_dir(leaf);
            return Err(Error::AssetDenied);
        }
        Ok(snapshots.directory.join(relative))
    }

    pub fn copy_claim(&self, source: &Path, to: &Path) -> Result<()> {
        use std::io::Write;
        let bytes = self.sources.read(source, FILE_MAX_BYTES)?;
        self.write_claim(to, |file| file.write_all(&bytes))
    }

    pub fn write_claim(
        &self,
        path: &Path,
        write: impl FnOnce(&mut File) -> std::io::Result<()>,
    ) -> Result<()> {
        let scope = self.sources.scope()?;
        let name = self.source_leaf(path)?;
        let mut options = crate::scoped::options(false);
        options.read(false).write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = scope
            .dir
            .open_with(name, &options)
            .map_err(|e| io_error("create-claim", path, e))?
            .into_std();
        let result = scope.check_file(&file, name).and_then(|()| {
            write(&mut file)
                .and_then(|()| file.sync_all())
                .map_err(|e| io_error("write-claim", path, e))
        });
        if result.is_err() {
            drop(file);
            let _ = scope.remove_file(name);
        }
        result
    }

    fn source_leaf<'a>(&self, path: &'a Path) -> Result<&'a Path> {
        if path.parent() != Some(&self.sources.directory) {
            return Err(Error::AssetDenied);
        }
        Ok(Path::new(path.file_name().ok_or(Error::AssetDenied)?))
    }

    pub fn remove_claim(&self, path: &Path) {
        let Ok(leaf) = self.source_leaf(path) else {
            return;
        };
        let Ok(scope) = self.sources.scope() else {
            return;
        };
        match scope.remove_file(leaf) {
            Ok(()) => (),
            Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return,
        }
        let cache = self
            .durable
            .as_ref()
            .or_else(|| self.snapshots.get().map(|(dir, _)| dir));
        if let Some(cache) = cache.and_then(|dir| private_scope(dir).ok()) {
            remove_group(cache, leaf);
        }
    }

    pub(crate) fn sweep_references(&self) {
        let Some(durable) = &self.durable else { return };
        let (Ok(cache), Ok(sources)) = (private_scope(durable), self.sources.scope()) else {
            return;
        };
        let Ok((_, groups)) = cache.read_dir(Path::new("")) else {
            return;
        };
        for group in groups.flatten() {
            let name = group.file_name();
            if sources
                .dir
                .symlink_metadata(&name)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                remove_group(cache, Path::new(&name));
            }
        }
    }
    pub fn read_reference(&self, path: &Path, limit: u64) -> Result<Vec<u8>> {
        self.snapshots()?.read(path, limit)
    }
}

fn remove_group(cache: &ScopedDir, group: &Path) {
    let Ok((held, entries)) = cache.read_dir(group) else {
        return;
    };
    let Some(parent) = held.last() else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name
            .to_str()
            .and_then(|s| s.split_once('-'))
            .is_some_and(|(digest, leaf)| {
                digest.len() == 64
                    && digest.bytes().all(|b| b.is_ascii_hexdigit())
                    && (Some(leaf) == group.to_str() || leaf.strip_suffix(".txt") == group.to_str())
            })
        {
            let _ = parent.remove_file(name);
        }
    }
    drop(held);
    let _ = cache.dir.remove_dir(group);
}
