//! Files read under one fixed root through the handle that was opened, for TypeScript's
//! `readContainedWorkflowFile` (`apps/server/src/orchestration-v2/workflowFileRead.ts`).
//!
//! The caller resolves the path ([`ContainedFiles::realpath`]), checks containment and the
//! extension, opens it ([`ContainedFiles::open`]), and compares the opened file's identity with
//! the path's own identity ([`ContainedFiles::lstat_identity`]) before reading. Opening walks
//! from the pinned root one component at a time without following links (`ScopedDir`), so a link
//! swapped into the leaf or into any ancestor after resolution is refused, and the contents
//! returned are those of the file whose identity was compared, not of whatever the path names
//! later. Type and size bounds stay with the caller, as in TypeScript: a directory or other
//! non-regular file opens and reports `regular: false`.
use crate::{paths, scoped::ScopedDir};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

/// A file's device and inode (Windows: volume serial number and file index), which a rename
/// within the volume keeps, as `stat`'s `dev`/`ino` do in TypeScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

pub struct FileStat {
    pub size: u64,
    pub regular: bool,
    pub identity: FileIdentity,
}

/// Reads confined to `root` (its real path, resolved on every open).
#[derive(Debug, Clone)]
pub struct ContainedFiles {
    root: PathBuf,
}

/// An opened file: every read and `stat` goes through this handle.
pub struct ContainedFile {
    file: File,
}

fn denied(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} is outside the contained root", path.display()),
    )
}

fn workspace_error(error: crate::Error) -> io::Error {
    match error {
        crate::Error::Io { source, .. } => source,
        error => io::Error::new(io::ErrorKind::PermissionDenied, error.to_string()),
    }
}

impl ContainedFiles {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The real path, refusing remote drives, links to network targets and device names
    /// (`paths::canonicalize`), so resolution never authenticates to another host.
    pub fn realpath(&self, path: &Path) -> io::Result<PathBuf> {
        paths::canonicalize(path)
    }

    /// Opens a resolved path under the root for reading, without following a link in the leaf
    /// or any ancestor (Unix: `O_NONBLOCK`, so a FIFO cannot block the open).
    pub fn open(&self, path: &Path) -> io::Result<ContainedFile> {
        let scope = ScopedDir::open(&self.root).map_err(workspace_error)?;
        let relative = path.strip_prefix(&scope.root).map_err(|_| denied(path))?;
        let name = relative.file_name().ok_or_else(|| denied(path))?;
        let dirs = scope
            .walk(relative.parent().unwrap_or(Path::new("")), false)
            .map_err(workspace_error)?;
        let parent = dirs.last().ok_or_else(|| denied(path))?;
        // Directory-capable options, so a directory opens and reports `regular: false` (Node opens
        // one with backup semantics on Windows); still no link is followed.
        let file = parent
            .open_with(name, &crate::scoped::options(true))?
            .into_std();
        let named = crate::scoped::final_path(&file, path).map_err(workspace_error)?;
        if !paths::within(&named, &scope.root) {
            return Err(denied(path));
        }
        Ok(ContainedFile { file })
    }

    /// The identity of the path itself, not of a link's target (`lstat`).
    pub fn lstat_identity(&self, path: &Path) -> io::Result<FileIdentity> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::symlink_metadata(path)?;
            Ok(FileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
            };
            // Opens a link or junction itself; attribute access only, any sharing.
            let file = std::fs::OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES)
                .share_mode(7)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)?;
            identity(&file)
        }
    }
}

fn identity(file: &File) -> io::Result<FileIdentity> {
    use cap_fs_ext::MetadataExt;
    // Built from the handle, so the volume serial number and file index are present.
    let metadata = cap_std::fs::Metadata::from_file(file)?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

impl ContainedFile {
    pub fn stat(&self) -> io::Result<FileStat> {
        let metadata = self.file.metadata()?;
        Ok(FileStat {
            size: metadata.len(),
            regular: metadata.is_file(),
            identity: identity(&self.file)?,
        })
    }

    /// Up to `length` bytes from `offset`; fewer only at the end of the file.
    pub fn read_at(&self, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let mut buffer = vec![0; length];
        let mut filled = 0;
        while filled < length {
            let position = offset + filled as u64;
            #[cfg(unix)]
            let read =
                std::os::unix::fs::FileExt::read_at(&self.file, &mut buffer[filled..], position);
            #[cfg(windows)]
            let read = std::os::windows::fs::FileExt::seek_read(
                &self.file,
                &mut buffer[filled..],
                position,
            );
            match read {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        buffer.truncate(filled);
        Ok(buffer)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        outside: PathBuf,
        files: ContainedFiles,
    }
    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("projects");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(root.join("session")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("session/flow.js"), "inside").unwrap();
        std::fs::write(outside.join("flow.js"), "outside").unwrap();
        let files = ContainedFiles::new(&root);
        let root = files.realpath(&root).unwrap();
        Fixture {
            _temp: temp,
            root,
            outside,
            files,
        }
    }
    fn file_link(target: &Path, link: &Path) {
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }
    fn dir_link(target: &Path, link: &Path) {
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(target, link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn reads_through_the_opened_handle_with_its_identity() {
        let f = fixture();
        let path = f.files.realpath(&f.root.join("session/flow.js")).unwrap();
        let opened = f.files.open(&path).unwrap();
        let stat = opened.stat().unwrap();
        assert!(stat.regular);
        assert_eq!(stat.size, 6);
        assert_eq!(f.files.lstat_identity(&path).unwrap(), stat.identity);
        assert_eq!(opened.read_at(0, 64).unwrap(), b"inside");
        assert_eq!(opened.read_at(2, 2).unwrap(), b"si");
    }

    #[test]
    fn a_leaf_replaced_by_a_link_to_an_outside_file_after_resolution_is_refused() {
        let f = fixture();
        let path = f.files.realpath(&f.root.join("session/flow.js")).unwrap();
        let recorded = f.files.lstat_identity(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        file_link(&f.outside.join("flow.js"), &path);
        assert!(f.files.open(&path).is_err());
        // The path's own identity is the link's: neither the recorded file's nor the target's.
        let now = f.files.lstat_identity(&path).unwrap();
        assert_ne!(now, recorded);
        let target = ContainedFiles::new(&f.outside)
            .open(&paths::canonicalize(f.outside.join("flow.js")).unwrap())
            .unwrap()
            .stat()
            .unwrap()
            .identity;
        assert_ne!(now, target, "lstat_identity followed the link");
        let outside = f.files.open(&f.outside.join("flow.js"));
        assert!(outside.is_err(), "a path outside the root opens");
    }

    /// A hard link is the same file as its outside target, so no identity check can tell; a
    /// regular file with more than one link is refused, as `ScopedDir::check_file` does.
    #[test]
    fn a_hard_linked_file_is_refused() {
        let f = fixture();
        let hard = f.root.join("session/hard.js");
        std::fs::hard_link(f.outside.join("flow.js"), &hard).unwrap();
        let path = f.files.realpath(&hard).unwrap();
        assert!(f.files.open(&path).is_err(), "a hard link to an outside file opened");
    }

    /// Resolution itself is confined: a path not lexically inside the root is refused before any
    /// filesystem call, the same way whether it exists, is missing or is a directory.
    #[test]
    fn paths_outside_the_root_are_refused_alike_before_resolution() {
        let f = fixture();
        std::fs::create_dir(f.outside.join("dir.js")).unwrap();
        let mut kinds = Vec::new();
        for path in [
            f.outside.join("flow.js"),
            f.outside.join("absent.js"),
            f.outside.join("dir.js"),
            f.root.join("session").join("..").join("..").join("outside").join("flow.js"),
        ] {
            let error = f.files.realpath(&path).expect_err("outside the root resolved");
            kinds.push((error.kind(), error.to_string()));
        }
        assert!(kinds.windows(2).all(|pair| pair[0] == pair[1]), "{kinds:?}");
        assert!(f.files.realpath(&f.root).is_ok());
    }

    /// While the file is open, Windows refuses to replace it (the handle denies delete sharing);
    /// elsewhere the replacement lands and the path's identity no longer matches the handle's,
    /// which the caller reports as `changed-during-read`.
    #[test]
    fn a_file_replaced_after_opening_is_detected_or_refused() {
        let f = fixture();
        let path = f.files.realpath(&f.root.join("session/flow.js")).unwrap();
        let opened = f.files.open(&path).unwrap();
        let identity = opened.stat().unwrap().identity;
        let replacement = f.root.join("session/new.js");
        std::fs::write(&replacement, "replaced").unwrap();
        let replaced = std::fs::rename(&replacement, &path);
        if cfg!(windows) {
            assert!(replaced.is_err(), "an open file was replaced");
            assert_eq!(f.files.lstat_identity(&path).unwrap(), identity);
        } else {
            replaced.unwrap();
            assert_ne!(f.files.lstat_identity(&path).unwrap(), identity);
        }
    }

    #[test]
    fn a_link_in_any_ancestor_is_refused() {
        let f = fixture();
        let path = f.files.realpath(&f.root.join("session/flow.js")).unwrap();
        std::fs::rename(f.root.join("session"), f.root.join("moved")).unwrap();
        dir_link(&f.outside, &f.root.join("session"));
        assert!(f.files.open(&path).is_err());
        // A link deeper in the chain is refused the same way.
        let nested = f.root.join("moved/inner");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("deep.js"), "deep").unwrap();
        let deep = f.files.realpath(&nested.join("deep.js")).unwrap();
        std::fs::rename(&nested, f.root.join("moved/old-inner")).unwrap();
        dir_link(&f.outside, &nested);
        assert!(f.files.open(&deep).is_err());
    }

    /// A junction (no privilege needed to create one) swapped into an ancestor is refused too.
    #[cfg(windows)]
    #[test]
    fn a_junction_in_an_ancestor_is_refused() {
        let f = fixture();
        let path = f.files.realpath(&f.root.join("session/flow.js")).unwrap();
        std::fs::rename(f.root.join("session"), f.root.join("moved")).unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(f.root.join("session"))
            .arg(&f.outside)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(f.files.open(&path).is_err());
    }

    #[test]
    fn identity_survives_a_rename_within_the_root_as_dev_and_ino_do() {
        let f = fixture();
        let path = f.files.realpath(&f.root.join("session/flow.js")).unwrap();
        let before = f.files.open(&path).unwrap().stat().unwrap().identity;
        let renamed = f.root.join("session/renamed.js");
        std::fs::rename(&path, &renamed).unwrap();
        let after = f.files.open(&renamed).unwrap().stat().unwrap().identity;
        assert_eq!(before, after);
        // A new file at the old name is a different file.
        std::fs::write(&path, "replacement").unwrap();
        assert_ne!(f.files.lstat_identity(&path).unwrap(), before);
    }

    #[test]
    fn a_directory_opens_as_not_regular_and_a_missing_file_fails() {
        let f = fixture();
        std::fs::create_dir(f.root.join("session/dir.js")).unwrap();
        let path = f.files.realpath(&f.root.join("session/dir.js")).unwrap();
        assert!(!f.files.open(&path).unwrap().stat().unwrap().regular);
        assert!(f.files.realpath(&f.root.join("session/absent.js")).is_err());
    }
}
