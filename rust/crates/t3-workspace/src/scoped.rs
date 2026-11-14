use crate::{Error, Result, io_error, paths};
use cap_fs_ext::{FollowSymlinks, MetadataExt, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{
    fs::File,
    io::Write,
    path::{Component, Path, PathBuf},
};

pub(crate) struct ScopedDir {
    pub root: PathBuf,
    pub dir: Dir,
}
pub(crate) fn identity(file: &File) -> Result<(String, String, u64)> {
    let info =
        cap_std::fs::Metadata::from_file(file).map_err(|e| io_error("stat-handle", "file", e))?;
    Ok((info.dev().to_string(), info.ino().to_string(), info.nlink()))
}
pub(crate) fn final_path(file: &File, expected: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let path =
            winx::file::get_file_path(file).map_err(|e| io_error("final-path", expected, e))?;
        let path = dunce::simplified(&path).to_owned();
        paths::validate_host_path(&path)?;
        Ok(path)
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let path = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .map_err(|e| io_error("final-path", expected, e))?;
        paths::validate_host_path(&path)?;
        Ok(path)
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStringExt;
        let path =
            rustix::fs::getpath(file).map_err(|e| io_error("final-path", expected, e.into()))?;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(
            path.into_bytes(),
        )))
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (file, expected);
        Err(Error::Invalid(
            "handle path validation is unavailable on this host".into(),
        ))
    }
}
pub(crate) fn options(directory: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(directory);
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        // Holding each ancestor without delete sharing prevents rename/junction swaps on Windows.
        if directory {
            options.share_mode(3);
        }
    }
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options
}
impl ScopedDir {
    pub fn open_authorized(root: &Path, known: &paths::KnownWorkspaces) -> Result<Self> {
        let scope = Self::open(root)?;
        if !known.contains(&scope.root) {
            return Err(Error::UnknownWorkspace(scope.root));
        }
        Ok(scope)
    }
    pub fn open_unlinked(root: &Path) -> Result<Self> {
        let named = paths::absolute(root)?;
        paths::validate_host_path(&named)?;
        let volume = named.ancestors().last().ok_or(Error::AssetDenied)?;
        let scope = Self::open(volume)?;
        let relative = paths::relative_to(&named, &scope.root)?;
        let dirs = scope.walk(&relative, false)?;
        let dir = dirs
            .last()
            .ok_or(Error::AssetDenied)?
            .try_clone()
            .map_err(|e| io_error("clone-root", &named, e))?;
        let file = dir.into_std_file();
        let root = final_path(&file, &named)?;
        Ok(Self {
            root,
            dir: Dir::from_std_file(file),
        })
    }
    pub fn open(root: &Path) -> Result<Self> {
        let root =
            paths::canonicalize(root).map_err(|e| io_error("realpath-workspace-root", root, e))?;
        let ambient = Dir::open_ambient_dir(&root, cap_std::ambient_authority())
            .map_err(|e| io_error("open-root", &root, e))?;
        let file = ambient
            .open_with(".", &options(true))
            .map_err(|e| io_error("pin-root", &root, e))?
            .into_std();
        if !paths::within(&final_path(&file, &root)?, &root) {
            return Err(Error::PathEscape(root));
        }
        Ok(Self {
            root,
            dir: Dir::from_std_file(file),
        })
    }
    pub fn resolve(&self, relative: &str, missing: bool) -> Result<PathBuf> {
        let (named, _) = paths::resolve_relative(&self.root, relative)?;
        let (real, exists) = paths::nearest_real(&named)?;
        if !exists && !missing {
            return Err(io_error(
                "realpath-target",
                named,
                std::io::ErrorKind::NotFound.into(),
            ));
        }
        let path = paths::relative_to(&real, &self.root)?;
        paths::validate_host_path(&path)?;
        Ok(path)
    }
    pub(crate) fn walk(&self, relative: &Path, create: bool) -> Result<Vec<Dir>> {
        let mut dirs = vec![
            self.dir
                .try_clone()
                .map_err(|e| io_error("clone-root", &self.root, e))?,
        ];
        for part in relative.components() {
            let Component::Normal(name) = part else {
                if part == Component::CurDir {
                    continue;
                }
                return Err(Error::PathEscape(relative.into()));
            };
            let current = dirs
                .last()
                .ok_or_else(|| Error::PathEscape(relative.into()))?;
            if create {
                match current.create_dir(name) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(io_error("make-directory", relative, e)),
                }
            }
            let file = current
                .open_with(name, &options(true))
                .map_err(|e| io_error("open-directory", relative, e))?;
            if !file
                .metadata()
                .map_err(|e| io_error("stat-directory", relative, e))?
                .is_dir()
            {
                return Err(Error::PathEscape(relative.into()));
            }
            let file = file.into_std();
            if !paths::within(&final_path(&file, &self.root.join(relative))?, &self.root) {
                return Err(Error::PathEscape(relative.into()));
            }
            dirs.push(Dir::from_std_file(file));
        }
        Ok(dirs)
    }
    pub fn open_file(&self, relative: &Path) -> Result<File> {
        let dirs = self.walk(relative.parent().unwrap_or(Path::new("")), false)?;
        let parent = dirs
            .last()
            .ok_or_else(|| Error::PathEscape(relative.into()))?;
        let name = relative
            .file_name()
            .ok_or_else(|| Error::PathEscape(relative.into()))?;
        let file = parent
            .open_with(name, &options(false))
            .map_err(|e| io_error("open", relative, e))?
            .into_std();
        self.check_file(&file, relative)?;
        Ok(file)
    }
    pub fn check_file(&self, file: &File, relative: &Path) -> Result<()> {
        if !file
            .metadata()
            .map_err(|e| io_error("stat", relative, e))?
            .is_file()
        {
            return Err(Error::NotFile(relative.into()));
        }
        let final_name = final_path(file, &self.root.join(relative))?;
        if !paths::within(&final_name, &self.root) || identity(file)?.2 != 1 {
            return Err(Error::PathEscape(relative.into()));
        }
        Ok(())
    }
    pub fn read_dir(&self, relative: &Path) -> Result<(Vec<Dir>, cap_std::fs::ReadDir)> {
        let dirs = self.walk(relative, false)?;
        let iter = dirs
            .last()
            .ok_or_else(|| Error::PathEscape(relative.into()))?
            .entries()
            .map_err(|e| io_error("read-directory", relative, e))?;
        Ok((dirs, iter))
    }
    pub fn write_atomic(&self, relative: &Path, contents: &[u8]) -> Result<()> {
        let dirs = self.walk(relative.parent().unwrap_or(Path::new("")), true)?;
        let parent = dirs
            .last()
            .ok_or_else(|| Error::PathEscape(relative.into()))?;
        let name = relative
            .file_name()
            .ok_or_else(|| Error::PathEscape(relative.into()))?;
        let permissions = match parent.open_with(name, &options(false)) {
            Ok(file) => {
                let file = file.into_std();
                self.check_file(&file, relative)?;
                Some(
                    file.metadata()
                        .map_err(|e| io_error("stat", relative, e))?
                        .permissions(),
                )
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_error("open", relative, e)),
        };
        let staging = format!(".t3-write-{}.part", uuid::Uuid::new_v4());
        let mut options = options(false);
        options.read(false).write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = parent
            .open_with(&staging, &options)
            .map_err(|e| io_error("open-staging", relative, e))?
            .into_std();
        let result = (|| {
            self.check_file(&file, relative)?;
            file.write_all(contents)
                .map_err(|e| io_error("write-file", relative, e))?;
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)
                    .map_err(|e| io_error("set-permissions", relative, e))?;
            }
            file.sync_all()
                .map_err(|e| io_error("sync-file", relative, e))?;
            drop(file);
            parent
                .rename(&staging, parent, name)
                .map_err(|e| io_error("replace-file", relative, e))?;
            #[cfg(unix)]
            parent
                .try_clone()
                .map_err(|e| io_error("clone-directory", relative, e))?
                .into_std_file()
                .sync_all()
                .map_err(|e| io_error("sync-directory", relative, e))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = parent.remove_file(staging);
        }
        result
    }
    pub fn remove_file(&self, relative: &Path) -> Result<()> {
        let dirs = self.walk(relative.parent().unwrap_or(Path::new("")), false)?;
        let parent = dirs
            .last()
            .ok_or_else(|| Error::PathEscape(relative.into()))?;
        parent
            .remove_file(
                relative
                    .file_name()
                    .ok_or_else(|| Error::PathEscape(relative.into()))?,
            )
            .map_err(|e| io_error("remove-file", relative, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_swapped_ancestor_cannot_redirect_read_write_list_or_delete() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(root.join("dir")).expect("directory");
        std::fs::create_dir(&outside).expect("outside");
        std::fs::write(root.join("dir/file"), "inside").expect("file");
        std::fs::write(outside.join("file"), "outside").expect("file");
        let scope = ScopedDir::open(&root).expect("scope");
        let relative = scope.resolve("dir/file", false).expect("validated target");
        std::fs::rename(root.join("dir"), root.join("old")).expect("rename");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, root.join("dir")).expect("swap");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("dir")).expect("swap");
        assert!(scope.open_file(&relative).is_err());
        assert!(scope.write_atomic(&relative, b"changed").is_err());
        assert!(scope.read_dir(Path::new("dir")).is_err());
        assert!(scope.remove_file(&relative).is_err());
        assert_eq!(
            std::fs::read_to_string(outside.join("file")).expect("outside"),
            "outside"
        );
    }
    #[test]
    fn a_swapped_final_file_is_not_followed_when_opening() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("root");
        std::fs::create_dir(&root).expect("root");
        let outside = temp.path().join("outside");
        std::fs::write(&outside, "outside").expect("file");
        std::fs::write(root.join("file"), "inside").expect("file");
        let scope = ScopedDir::open(&root).expect("scope");
        let relative = scope.resolve("file", false).expect("validated file");
        std::fs::remove_file(root.join("file")).expect("remove");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, root.join("file")).expect("swap");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("file")).expect("swap");
        assert!(scope.open_file(&relative).is_err());
        assert!(scope.write_atomic(&relative, b"changed").is_err());
        assert_eq!(
            std::fs::read_to_string(outside).expect("outside"),
            "outside"
        );
    }
}
