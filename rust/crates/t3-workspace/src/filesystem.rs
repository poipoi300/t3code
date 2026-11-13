use crate::{
    Error, Result, blocking, io_error,
    paths::{self, InternalPaths, WorkspaceRoots},
};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString as Text, filesystem::*, project::*};

pub const READ_MAX_BYTES: u64 = 1024 * 1024;
pub const WRITE_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const METADATA_MAX_PATHS: usize = 1024;

#[derive(Clone)]
pub struct WorkspaceFileSystem {
    pub home: PathBuf,
    pub internal: InternalPaths,
    pub known: Arc<dyn WorkspaceRoots>,
    pub indexes: Arc<crate::search::SearchIndexes>,
}
impl WorkspaceFileSystem {
    pub async fn read_file(&self, input: ProjectReadFileInput) -> Result<ProjectReadFileResult> {
        paths::bound_text(&input.relative_path, 512, false, "read path")?;
        paths::bound_text(&input.cwd.0, 32768, false, "workspace root")?;
        let known = self.known.load().await?;
        known.assert_contains(Path::new(&input.cwd.0))?;
        let this = self.clone();
        blocking(move || this.read(&input, &known)).await
    }
    fn read(
        &self,
        input: &ProjectReadFileInput,
        known: &paths::KnownWorkspaces,
    ) -> Result<ProjectReadFileResult> {
        let cwd = Path::new(&input.cwd.0);
        let requested = &input.relative_path;
        let host = paths::expand_home(requested, &self.home);
        paths::validate_host_path(&host)?;
        let scoped = crate::scoped::ScopedDir::open_authorized(cwd, known)?;
        let missing = input.allow_missing == Some(Some(true));
        let relative = if host.has_root() {
            {
                let (real, _) = paths::nearest_real(&paths::absolute(&host)?)?;
                if self.internal.contains(&real) {
                    return Err(Error::OutsideRoot(requested.clone()));
                }
                paths::display_path(&paths::relative_to(&real, &scoped.root)?)
            }
        } else {
            requested.clone()
        };
        let resolved = scoped.resolve(&relative, missing)?;
        let real = scoped.root.join(&resolved);
        let relative_path = if host.has_root() {
            requested.clone()
        } else {
            paths::resolve_relative(cwd, requested)?.1
        };
        let absent = || ProjectReadFileResult {
            relative_path: relative_path.clone(),
            contents: String::new(),
            byte_length: 0,
            truncated: false,
            exists: Some(Some(false)),
        };
        if self.internal.contains(&real) {
            return Err(Error::OutsideRoot(requested.clone()));
        }
        #[cfg(test)]
        BEFORE_OPEN.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let mut file = match scoped.open_file(&resolved) {
            Ok(file) => file,
            Err(Error::Io { source, .. })
                if missing && source.kind() == io::ErrorKind::NotFound =>
            {
                return Ok(absent());
            }
            Err(error) => return Err(error),
        };
        if self
            .internal
            .contains(&crate::scoped::final_path(&file, &real)?)
        {
            return Err(Error::OutsideRoot(requested.clone()));
        }
        let info = file.metadata().map_err(|e| io_error("stat", &real, e))?;
        if !info.is_file() {
            return Err(Error::NotFile(real));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(info.len().min(READ_MAX_BYTES))
            .read_to_end(&mut bytes)
            .map_err(|e| io_error("read", &real, e))?;
        if bytes.contains(&0) {
            return Err(Error::Binary(real));
        }
        let truncated = info.len() > READ_MAX_BYTES;
        let mut end = bytes.len();
        if truncated {
            // Only an incomplete tail is dropped; malformed UTF-8 elsewhere uses replacement decoding.
            let mut offset = 0;
            while let Err(error) = std::str::from_utf8(&bytes[offset..]) {
                offset += error.valid_up_to();
                match error.error_len() {
                    Some(n) => offset += n,
                    None => {
                        end = offset;
                        break;
                    }
                }
            }
        }
        let decoded = String::from_utf8_lossy(&bytes[..end]);
        let contents = decoded
            .strip_prefix('\u{feff}')
            .unwrap_or(&decoded)
            .to_owned();
        Ok(ProjectReadFileResult {
            relative_path,
            contents,
            byte_length: info.len(),
            truncated,
            exists: None,
        })
    }
    pub async fn write_file(&self, input: ProjectWriteFileInput) -> Result<ProjectWriteFileResult> {
        paths::bound_text(&input.relative_path, 512, false, "write path")?;
        paths::bound_text(&input.cwd.0, 32768, false, "workspace root")?;
        if input.contents.len() > WRITE_MAX_BYTES {
            return Err(Error::Invalid("file exceeds write limit".into()));
        }
        let known = self.known.load().await?;
        known.assert_contains(Path::new(&input.cwd.0))?;
        let cwd = input.cwd.0.clone();
        let result = blocking(move || {
            let scoped =
                crate::scoped::ScopedDir::open_authorized(Path::new(&input.cwd.0), &known)?;
            let (named, relative) = paths::resolve_relative(&scoped.root, &input.relative_path)?;
            let resolved = scoped.resolve(&relative, true)?;
            let real = scoped.root.join(&resolved);
            if paths::in_git_folder(&named)? || paths::in_git_folder(&real)? {
                return Err(Error::PathEscape(real));
            }
            scoped.write_atomic(&resolved, input.contents.as_bytes())?;
            Ok(ProjectWriteFileResult {
                relative_path: relative,
            })
        })
        .await?;
        self.indexes.refresh(Path::new(&cwd)).await?;
        Ok(result)
    }
    pub async fn get_metadata(
        &self,
        input: FilesystemGetMetadataInput,
    ) -> Result<FilesystemGetMetadataResult> {
        if input.paths.len() > METADATA_MAX_PATHS {
            return Err(Error::Invalid("too many metadata paths".into()));
        }
        for path in &input.paths {
            paths::bound_text(&path.0, 32768, false, "metadata path")?;
        }
        let known = self.known.load().await?;
        let this = self.clone();
        blocking(move || {
            let mut cache = HashMap::new();
            let entries = input
                .paths
                .iter()
                .map(|p| {
                    cache
                        .entry(p.0.clone())
                        .or_insert_with(|| {
                            let expanded = paths::expand_home(&p.0, &this.home);
                            let real = paths::canonicalize(&expanded).ok()?;
                            if !known.contains(&real) {
                                return None;
                            }
                            this.metadata(&p.0, &known)
                        })
                        .clone()
                })
                .collect();
            Ok(FilesystemGetMetadataResult { entries })
        })
        .await
    }
    fn metadata(
        &self,
        requested: &str,
        known: &paths::KnownWorkspaces,
    ) -> Option<FilesystemEntryMetadata> {
        let p = paths::expand_home(requested, &self.home);
        if !p.is_absolute() || paths::is_network_path(&p.to_string_lossy()) {
            return None;
        }
        let canonical = paths::canonicalize(&p).ok()?;
        let parent = canonical.parent()?;
        let scope = crate::scoped::ScopedDir::open(parent).ok()?;
        let mut opened = scope.open_file(Path::new(canonical.file_name()?)).ok();
        let (info, real) = if let Some(file) = &opened {
            (
                cap_std::fs::Metadata::from_file(file).ok()?,
                crate::scoped::final_path(file, &canonical).ok()?,
            )
        } else {
            let directory = crate::scoped::ScopedDir::open_authorized(&canonical, known).ok()?;
            (directory.dir.dir_metadata().ok()?, directory.root)
        };
        if !known.contains(&real) || self.internal.contains(&real) {
            return None;
        }
        let (kind, byte_length) = if info.is_file() {
            (FilesystemEntryKind::File, Some(info.len()))
        } else if info.is_dir() {
            (FilesystemEntryKind::Directory, None)
        } else {
            (FilesystemEntryKind::Other, None)
        };
        let mime_type = if info.is_file() && info.len() > 0 && p.extension().is_none() {
            opened.take().and_then(|mut file| {
                if !file.metadata().ok()?.is_file() {
                    return None;
                }
                let mut bytes = [0; 512];
                let len = file.read(&mut bytes).ok()?;
                header_mime_type(&bytes[..len]).map(|s| Text(s.into()))
            })
        } else {
            None
        };
        Some(FilesystemEntryMetadata {
            kind,
            byte_length,
            mime_type,
        })
    }
}

pub(crate) fn open_read(path: &Path, no_follow: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | if no_follow { libc::O_NOFOLLOW } else { 0 });
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        if no_follow {
            options.custom_flags(0x00200000);
        }
    }
    options.open(path).map_err(|e| io_error("open", path, e))
}

pub fn header_mime_type(bytes: &[u8]) -> Option<&'static str> {
    for (magic, mime) in [
        (b"\x89PNG\r\n\x1a\n".as_slice(), "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"PK\x03\x04", "application/zip"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"%PDF-", "application/pdf"),
        (b"SQLite format 3\0", "application/vnd.sqlite3"),
    ] {
        if bytes.starts_with(magic) {
            return Some(mime);
        }
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    if !bytes.starts_with(b"#!") {
        return None;
    }
    let header = String::from_utf8_lossy(bytes);
    let line = header.lines().next()?;
    for (pattern, mime) in [
        (r"\bpython[\d.]*\b", "text/x-python"),
        (r"\b(?:node|nodejs)\b", "text/javascript"),
        (r"\bruby\b", "text/x-ruby"),
        (r"\b(?:sh|bash|zsh|fish)\b", "text/x-shellscript"),
    ] {
        if regex::Regex::new(pattern).ok()?.is_match(line) {
            return Some(mime);
        }
    }
    None
}

#[cfg(test)]
thread_local! { static BEFORE_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default(); }
#[cfg(test)]
mod race_tests {
    use super::*;
    use std::fs;
    #[test]
    fn a_swapped_workspace_alias_cannot_replace_the_authorized_root() {
        let tmp = tempfile::tempdir().expect("temp");
        let root = tmp.path().join("project");
        let outside = tmp.path().join("outside");
        let alias = tmp.path().join("alias");
        fs::create_dir(&root).expect("root");
        fs::create_dir(&outside).expect("outside");
        fs::write(root.join("file.txt"), "inside").expect("file");
        fs::write(outside.join("file.txt"), "outside").expect("file");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&root, &alias).expect("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, &alias).expect("alias");
        let known = paths::KnownWorkspaces::new([root.clone()]).expect("known");
        known
            .assert_contains(&alias)
            .expect("authorized before swap");
        #[cfg(windows)]
        fs::remove_dir(&alias).expect("remove alias");
        #[cfg(unix)]
        fs::remove_file(&alias).expect("remove alias");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, &alias).expect("swap");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &alias).expect("swap");
        let service = WorkspaceFileSystem {
            home: root.clone(),
            internal: InternalPaths {
                state_dir: root.join("state"),
                secrets_dir: root.join("secrets"),
            },
            known: {
                let known = known.clone();
                Arc::new(move || {
                    let known = known.clone();
                    async move { Ok(known) }
                })
            },
            indexes: Arc::new(crate::search::SearchIndexes::new(InternalPaths {
                state_dir: root.join("state"),
                secrets_dir: root.join("secrets"),
            })),
        };
        let result = service.read(
            &ProjectReadFileInput {
                cwd: Text(alias.to_string_lossy().into()),
                relative_path: "file.txt".into(),
                allow_missing: None,
            },
            &known,
        );
        assert!(result.is_err(), "swapped root served outside content");
    }

    #[test]
    fn swapped_directory_cannot_redirect_a_validated_read() {
        let tmp = tempfile::tempdir().expect("temp");
        let root = tmp.path().join("project");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(root.join("dir")).expect("dir");
        fs::create_dir(&outside).expect("outside");
        fs::write(root.join("dir/file.txt"), "inside").expect("file");
        fs::write(outside.join("file.txt"), "outside").expect("file");
        let service = WorkspaceFileSystem {
            home: root.clone(),
            internal: InternalPaths {
                state_dir: root.join("state"),
                secrets_dir: root.join("secrets"),
            },
            known: {
                let root = root.clone();
                Arc::new(move || {
                    let root = root.clone();
                    async move { crate::paths::KnownWorkspaces::new([root]) }
                })
            },
            indexes: Arc::new(crate::search::SearchIndexes::new(
                crate::paths::InternalPaths {
                    state_dir: root.join("state"),
                    secrets_dir: root.join("secrets"),
                },
            )),
        };
        let swap_root = root.clone();
        BEFORE_OPEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(swap_root.join("dir"), swap_root.join("old")).expect("rename");
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir(&outside, swap_root.join("dir")).expect("swap");
                #[cfg(unix)]
                std::os::unix::fs::symlink(&outside, swap_root.join("dir")).expect("swap");
            }))
        });
        let result = service.read(
            &ProjectReadFileInput {
                cwd: Text(root.to_string_lossy().into()),
                relative_path: "dir/file.txt".into(),
                allow_missing: None,
            },
            &paths::KnownWorkspaces::new([root.clone()]).expect("known"),
        );
        assert!(
            result.is_err() || result.expect("read").contents == "inside",
            "read escaped after check"
        );
    }
}
