use crate::{CancellationToken, ExecuteOptions, Git, GitError};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const SHARED_WORKSPACE_RESTORE_MESSAGE: &str = "File restore requires an isolated worktree. This workspace may contain changes from another thread or a side chat. Rewind the conversation without restoring files instead, or delete side chats you no longer need.";

#[derive(Debug, Clone)]
pub struct RestoreSession {
    pub cwd: PathBuf,
    pub stopped: bool,
    pub supports_multiple_provider_threads: bool,
}
#[derive(Debug, Clone)]
pub struct RestoreParticipant {
    pub thread_id: String,
    pub deleted: bool,
    pub worktree_path: Option<PathBuf>,
    /// Required for a thread without a worktree; absence fails closed.
    pub project_root: Option<PathBuf>,
    pub scope_paths: Vec<PathBuf>,
    pub provider_sessions: Vec<RestoreSession>,
    pub side_chat: bool,
    pub active_run: bool,
    pub has_mutating_turn_items: bool,
}
#[derive(Debug, Clone)]
pub struct RestoreSafety {
    pub thread_id: String,
    pub worktree_path: Option<PathBuf>,
    pub scope_cwd: PathBuf,
    /// Include both active and archived threads and their current provider sessions.
    pub participants: Vec<RestoreParticipant>,
}
pub(crate) fn lexical_absolute(path: &Path) -> Result<PathBuf, std::io::Error> {
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component),
        }
    }
    Ok(normalized)
}
pub(crate) fn resolve(path: &Path) -> Result<PathBuf, std::io::Error> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for name in missing.into_iter().rev() {
                    resolved.push(name);
                }
                return lexical_absolute(&resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(
                    component @ (Component::Normal(_) | Component::ParentDir | Component::CurDir),
                ) = ancestor.components().next_back()
                else {
                    return Err(error);
                };
                missing.push(component.as_os_str().to_owned());
                let Some(parent) = ancestor.parent() else {
                    return Err(error);
                };
                ancestor = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

fn comparison_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    let path = resolve(path)?;
    #[cfg(windows)]
    let path = PathBuf::from(path.to_string_lossy().to_lowercase());
    Ok(path)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkKind {
    None,
    Symlink,
    #[cfg(windows)]
    MountPoint,
    #[cfg(windows)]
    OtherNameSurrogate,
}
impl LinkKind {
    fn is_mount(self) -> bool {
        #[cfg(windows)]
        {
            matches!(self, Self::MountPoint | Self::OtherNameSurrogate)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
    fn is_junction(self) -> bool {
        #[cfg(windows)]
        {
            self == Self::MountPoint
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

#[cfg(windows)]
fn classify_tag(tag: u32) -> LinkKind {
    match tag {
        0xa000000c => LinkKind::Symlink,
        0xa0000003 => LinkKind::MountPoint,
        tag if tag & 0x20000000 != 0 => LinkKind::OtherNameSurrogate,
        _ => LinkKind::None,
    }
}
#[cfg(windows)]
#[allow(unsafe_code)]
fn link_kind(path: &Path, metadata: &std::fs::Metadata) -> std::io::Result<LinkKind> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows_sys::Win32::System::{IO::DeviceIoControl, Ioctl::FSCTL_GET_REPARSE_POINT};
    if metadata.file_attributes() & 0x400 == 0 {
        return Ok(LinkKind::None);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .access_mode(0)
        .share_mode(7)
        .custom_flags(0x00200000 | 0x02000000)
        .open(path)?;
    let mut buffer = [0u8; 16 * 1024];
    let mut returned = 0;
    // SAFETY: the live File owns this synchronous handle; the output buffer and count are writable.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_GET_REPARSE_POINT,
            std::ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if returned < 8 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "short reparse header",
        ));
    }
    Ok(classify_tag(u32::from_le_bytes([
        buffer[0], buffer[1], buffer[2], buffer[3],
    ])))
}
#[cfg(not(windows))]
fn link_kind(_: &Path, metadata: &std::fs::Metadata) -> std::io::Result<LinkKind> {
    Ok(if metadata.file_type().is_symlink() {
        LinkKind::Symlink
    } else {
        LinkKind::None
    })
}

pub(crate) struct RestoreScope {
    pub root: PathBuf,
    pub directory: PathBuf,
    pub pathspec: String,
}
#[derive(Clone, Copy)]
enum PathOutput {
    Diff,
    CachedDiff,
    Index { include_skipped: bool },
    Clean,
}
struct PathRecords {
    kind: PathOutput,
    pending: Vec<u8>,
    paths: BTreeSet<PathBuf>,
    has_entries: bool,
    gitlink_leaves: BTreeMap<PathBuf, bool>,
    pending_gitlink: Option<bool>,
    error: Option<GitError>,
}
fn os_path(bytes: Vec<u8>) -> Result<PathBuf, GitError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes).into())
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| GitError::InvalidOutput("Git path is not UTF-8".into()))
    }
}
fn unquote_git_path(bytes: &[u8]) -> Result<Vec<u8>, GitError> {
    if !bytes.starts_with(b"\"") {
        return Ok(bytes.to_vec());
    }
    if bytes.len() < 2 || bytes.last() != Some(&b'"') {
        return Err(GitError::InvalidOutput("quoted Git path".into()));
    }
    let mut result = Vec::new();
    let mut index = 1;
    while index + 1 < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte != b'\\' {
            result.push(byte);
            continue;
        }
        let escaped = *bytes
            .get(index)
            .ok_or_else(|| GitError::InvalidOutput("Git path escape".into()))?;
        index += 1;
        result.push(match escaped {
            b'a' => 7,
            b'b' => 8,
            b't' => 9,
            b'n' => 10,
            b'v' => 11,
            b'f' => 12,
            b'r' => 13,
            b'\\' => b'\\',
            b'"' => b'"',
            b'0'..=b'3' => {
                let next = bytes
                    .get(index..index + 2)
                    .ok_or_else(|| GitError::InvalidOutput("Git path octal escape".into()))?;
                if !next.iter().all(|byte| matches!(byte, b'0'..=b'7')) {
                    return Err(GitError::InvalidOutput("Git path octal digits".into()));
                }
                index += 2;
                (escaped - b'0') * 64 + (next[0] - b'0') * 8 + next[1] - b'0'
            }
            _ => return Err(GitError::InvalidOutput("Git path escape code".into())),
        });
    }
    Ok(result)
}
impl PathRecords {
    fn feed(&mut self, chunk: &[u8]) {
        let delimiter = if matches!(self.kind, PathOutput::Clean) {
            b'\n'
        } else {
            0
        };
        for part in chunk.split_inclusive(|byte| *byte == delimiter) {
            let complete = part.last() == Some(&delimiter);
            self.pending.extend_from_slice(if complete {
                &part[..part.len() - 1]
            } else {
                part
            });
            if complete {
                let record = std::mem::take(&mut self.pending);
                if self.error.is_none()
                    && let Err(error) = self.record(&record)
                {
                    self.error = Some(error);
                }
            }
        }
    }
    fn record(&mut self, record: &[u8]) -> Result<(), GitError> {
        if record.is_empty() {
            return Ok(());
        }
        self.has_entries = true;
        let mut mode = None;
        let path = match self.kind {
            PathOutput::Diff | PathOutput::CachedDiff => {
                if let Some(skip) = self.pending_gitlink.take() {
                    mode = Some(skip);
                    record.to_vec()
                } else {
                    let fields: Vec<_> = record.split(|byte| *byte == b' ').collect();
                    if fields.len() != 5
                        || fields[0].len() != 7
                        || fields[0][0] != b':'
                        || fields[1].len() != 6
                    {
                        return Err(GitError::InvalidOutput("Git raw diff metadata".into()));
                    }
                    // Restore does not recurse into source gitlinks or removed index gitlinks.
                    self.pending_gitlink = Some(
                        &fields[0][1..] == b"160000"
                            || matches!(self.kind, PathOutput::CachedDiff)
                                && &fields[0][1..] == b"000000"
                                && fields[1] == b"160000",
                    );
                    return Ok(());
                }
            }
            PathOutput::Index { include_skipped } => {
                if record.len() < 3 || record[1] != b' ' {
                    return Err(GitError::InvalidOutput("Git index path record".into()));
                }
                let gitlink = record.get(2..8) == Some(b"160000".as_slice());
                if !gitlink
                    && !(include_skipped && record[0] == b'S')
                    && !record[0].is_ascii_lowercase()
                {
                    return Ok(());
                }
                let separator = record
                    .iter()
                    .position(|byte| *byte == b'\t')
                    .ok_or_else(|| GitError::InvalidOutput("Git index stage metadata".into()))?;
                mode = Some(gitlink);
                record[separator + 1..].to_vec()
            }
            PathOutput::Clean => {
                if record.starts_with(b"Would skip repository ") {
                    return Ok(());
                }
                let path = record
                    .strip_prefix(b"Would remove ")
                    .ok_or_else(|| GitError::InvalidOutput("Git clean dry-run record".into()))?;
                unquote_git_path(path)?
            }
        };
        let path = os_path(path)?;
        if let Some(mode) = mode {
            self.gitlink_leaves.insert(path.clone(), mode);
        }
        self.paths.insert(path);
        Ok(())
    }
}
impl Git {
    pub(crate) async fn restore_scope(&self) -> Result<RestoreScope, GitError> {
        let output = self
            .text(&[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--show-prefix",
            ])
            .await?;
        let (root, prefix) = output
            .trim_end_matches('\n')
            .split_once('\n')
            .unwrap_or((output.trim_end_matches('\n'), ""));
        let root = std::fs::canonicalize(root)?;
        let actual = resolve(self.cwd())?;
        if !actual.starts_with(&root) {
            return Err(GitError::UnsafeRestore(
                "Restore scope is outside Git's top-level.".into(),
            ));
        }
        let prefix = Path::new(prefix.trim_end_matches('/'));
        if prefix
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(GitError::InvalidOutput("Git scope prefix".into()));
        }
        Ok(RestoreScope {
            directory: root.join(prefix),
            pathspec: if prefix.as_os_str().is_empty() {
                ".".into()
            } else {
                format!(
                    ":(top,literal){}",
                    if cfg!(windows) {
                        prefix.to_string_lossy().replace('\\', "/")
                    } else {
                        prefix.to_string_lossy().into_owned()
                    }
                )
            },
            root,
        })
    }
    async fn path_records(
        &self,
        args: &[&str],
        kind: PathOutput,
    ) -> Result<(bool, BTreeSet<PathBuf>, BTreeMap<PathBuf, bool>), GitError> {
        let records = Arc::new(Mutex::new(PathRecords {
            kind,
            pending: Vec::new(),
            paths: BTreeSet::new(),
            has_entries: false,
            gitlink_leaves: BTreeMap::new(),
            pending_gitlink: None,
            error: None,
        }));
        let callback = records.clone();
        self.execute(
            args,
            ExecuteOptions {
                capture: true,
                max_output_bytes: 4096,
                env: {
                    let mut env = crate::env(&[("GIT_OPTIONAL_LOCKS", "0")]);
                    if matches!(kind, PathOutput::Clean) {
                        env.extend(crate::env(&[("LC_ALL", "C")]));
                    }
                    env
                },
                on_stdout_chunk: Some(Arc::new(move |chunk| {
                    if let Ok(mut records) = callback.lock() {
                        records.feed(chunk);
                    }
                })),
                ..Default::default()
            },
        )
        .await?;
        let mut records = records
            .lock()
            .map_err(|_| GitError::InvalidOutput("Git path collector poisoned".into()))?;
        if let Some(error) = records.error.take() {
            return Err(error);
        }
        if !records.pending.is_empty() || records.pending_gitlink.is_some() {
            return Err(GitError::InvalidOutput(
                "unterminated Git path record".into(),
            ));
        }
        Ok((
            records.has_entries,
            std::mem::take(&mut records.paths),
            std::mem::take(&mut records.gitlink_leaves),
        ))
    }
    pub(crate) async fn check_restore_writes(
        &self,
        scope: &RestoreScope,
        commit: &str,
        sparse: bool,
        selected: Option<&[String]>,
    ) -> Result<bool, GitError> {
        let common = [
            "--raw",
            "--no-abbrev",
            "--ita-visible-in-index",
            "-z",
            "--no-relative",
            "--no-renames",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
        ];
        let mut cached = vec!["-c", "core.fsmonitor=false", "diff", "--cached"];
        cached.extend(common);
        cached.extend(["--ignore-submodules=none", commit, "--", "."]);
        let mut working = vec!["-c", "core.fsmonitor=false", "diff"];
        working.extend(common);
        working.extend(["--ignore-submodules=all", commit, "--", "."]);
        let mut index_args = vec!["ls-files", "--cached", "--full-name", "--stage", "-v", "-z"];
        if sparse {
            index_args.push("--sparse");
        }
        index_args.extend(["--", "."]);
        let (index, staged, worktree) = tokio::join!(
            self.path_records(
                &index_args,
                PathOutput::Index {
                    include_skipped: !sparse || selected.is_some(),
                }
            ),
            self.path_records(&cached, PathOutput::CachedDiff),
            self.path_records(&working, PathOutput::Diff)
        );
        let (index_tracked, mut paths, mut gitlink_leaves) = index?;
        let (source_tracked, staged, staged_gitlink_leaves) = staged?;
        let (_, worktree, worktree_gitlink_leaves) = worktree?;
        paths.extend(staged);
        paths.extend(worktree);
        gitlink_leaves.extend(worktree_gitlink_leaves);
        gitlink_leaves.extend(staged_gitlink_leaves);
        let tracked = index_tracked || source_tracked;
        if let Some(selected) = selected {
            let selected: BTreeSet<_> = selected.iter().map(PathBuf::from).collect();
            paths.retain(|path| selected.contains(path));
        }
        self.check_touch_paths(
            scope,
            paths
                .into_iter()
                .map(|path| scope.root.join(path))
                .collect(),
            gitlink_leaves
                .into_iter()
                .filter(|(_, skip)| *skip)
                .map(|(path, _)| scope.root.join(path))
                .collect(),
            false,
        )
        .await?;
        Ok(tracked)
    }
    pub(crate) async fn clean_has_native_mount_guard(&self) -> Result<bool, GitError> {
        #[cfg(not(windows))]
        {
            Ok(true)
        }
        #[cfg(windows)]
        {
            let result = self
                .without_fsmonitor()
                .execute(
                    &["--version"],
                    ExecuteOptions {
                        allow_nonzero: true,
                        timeout: Some(std::time::Duration::from_secs(5)),
                        max_output_bytes: 4096,
                        ..Default::default()
                    },
                )
                .await;
            match result {
                Err(error @ GitError::Process(t3_process::ProcessError::Cancelled)) => Err(error),
                Ok(output)
                    if output.code == Some(0)
                        && !output.stdout_truncated
                        && !output.stdout_invalid_utf8 =>
                {
                    Ok(clean_mount_guard_version(&output.stdout_text()))
                }
                _ => Ok(false),
            }
        }
    }
    pub(crate) async fn check_clean_paths(
        &self,
        scope: &RestoreScope,
        native_mount_guard: bool,
    ) -> Result<(), GitError> {
        let (_, paths, _) = self
            .path_records(
                &[
                    "-c",
                    "core.quotePath=true",
                    "-c",
                    "color.ui=false",
                    "clean",
                    "-nd",
                    "--",
                    ".",
                ],
                PathOutput::Clean,
            )
            .await?;
        self.check_touch_paths(
            scope,
            paths
                .into_iter()
                .map(|path| scope.directory.join(path))
                .collect(),
            BTreeSet::new(),
            native_mount_guard,
        )
        .await
    }
    pub(crate) async fn recreate_restore_scope(
        &self,
        scope: &RestoreScope,
    ) -> Result<(), GitError> {
        let root = scope.root.clone();
        let directory = scope.directory.clone();
        let cancellation = self.cancellation.clone();
        tokio::task::spawn_blocking(move || {
            check_directories(
                &root,
                &directory,
                &mut BTreeMap::new(),
                &cancellation,
                false,
            )?;
            std::fs::create_dir_all(directory)?;
            Ok(())
        })
        .await
        .map_err(|_| GitError::InvalidOutput("restore scope recreation task failed".into()))?
    }
    async fn check_touch_paths(
        &self,
        scope: &RestoreScope,
        paths: Vec<PathBuf>,
        skip_leaves: BTreeSet<PathBuf>,
        native_clean_guard: bool,
    ) -> Result<(), GitError> {
        let root = scope.root.clone();
        let cancellation = self.cancellation.clone();
        tokio::task::spawn_blocking(move || {
            let mut checked = BTreeMap::new();
            for path in paths {
                let path = lexical_absolute(&path)?;
                if !path.starts_with(&root) {
                    return Err(GitError::UnsafeRestore(
                        "Git touch path is outside the top-level.".into(),
                    ));
                }
                if let Some(parent) = path.parent().filter(|path| path.starts_with(&root)) {
                    if !check_directories(&root, parent, &mut checked, &cancellation, true)? {
                        continue;
                    }
                }
                if !skip_leaves.contains(&path) {
                    if native_clean_guard {
                        check_clean_leaf(&path)?;
                    } else {
                        check_deleting_directory(&path, &cancellation)?;
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| GitError::InvalidOutput("restore path check task failed".into()))?
    }
}
#[cfg(windows)]
fn clean_mount_guard_version(output: &str) -> bool {
    // Git for Windows 2.23.0 added clean's mount-point guard: git-for-windows/git#2268.
    let Some(version) = output.trim().strip_prefix("git version ") else {
        return false;
    };
    let Some((core, build)) = version.split_once(".windows.") else {
        return false;
    };
    if !build.parse::<u32>().is_ok_and(|build| build > 0) {
        return false;
    }
    let mut parts = core.split('.');
    let version = (
        parts.next().and_then(|v| v.parse::<u32>().ok()),
        parts.next().and_then(|v| v.parse::<u32>().ok()),
        parts.next().and_then(|v| v.parse::<u32>().ok()),
    );
    matches!(version, (Some(major), Some(minor), Some(patch))
        if parts.next().is_none() && (major, minor, patch) >= (2, 23, 0))
}
fn check_directories(
    root: &Path,
    directory: &Path,
    checked: &mut BTreeMap<PathBuf, bool>,
    cancellation: &CancellationToken,
    replace_ancestors: bool,
) -> Result<bool, GitError> {
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| GitError::UnsafeRestore("Git path parent is outside the top-level.".into()))?;
    let mut directory = root.to_owned();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if cancellation.is_cancelled() {
            return Err(t3_process::ProcessError::Cancelled.into());
        }
        if let Some(component) = component {
            directory.push(component);
        }
        if let Some(real) = checked.get(&directory) {
            if !real {
                return Ok(false);
            }
            continue;
        }
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) => {
                let kind = link_kind(&directory, &metadata)?;
                if kind.is_mount() {
                    return Err(GitError::UnsafeRestore(format!(
                        "Refusing a junction, mount point or name surrogate ancestor at {}.",
                        directory.display()
                    )));
                }
                if kind == LinkKind::Symlink || !metadata.is_dir() {
                    if directory == root || !replace_ancestors {
                        return Err(GitError::UnsafeRestore(format!(
                            "Restore scope is not a real directory at {}.",
                            directory.display()
                        )));
                    }
                    checked.insert(directory, false);
                    return Ok(false);
                }
                checked.insert(directory.clone(), true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                checked.insert(directory, false);
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}
fn check_clean_leaf(path: &Path) -> Result<(), GitError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let kind = link_kind(path, &metadata)?;
    if kind.is_mount() && !kind.is_junction() {
        return Err(GitError::UnsafeRestore(format!(
            "Refusing an unsupported name surrogate at {}.",
            path.display()
        )));
    }
    Ok(())
}
fn check_deleting_directory(path: &Path, cancellation: &CancellationToken) -> Result<(), GitError> {
    let mut pending = vec![path.to_owned()];
    while let Some(path) = pending.pop() {
        if cancellation.is_cancelled() {
            return Err(t3_process::ProcessError::Cancelled.into());
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let kind = link_kind(&path, &metadata)?;
        if kind.is_mount() {
            return Err(GitError::UnsafeRestore(format!(
                "Refusing a junction, mount point or name surrogate at {}.",
                path.display()
            )));
        }
        if kind == LinkKind::None && metadata.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}
pub fn is_restore_isolated(safety: &RestoreSafety) -> Result<bool, GitError> {
    let Some(worktree) = &safety.worktree_path else {
        return Ok(false);
    };
    let cwd = comparison_path(&safety.scope_cwd)?;
    if cwd != comparison_path(worktree)? {
        return Ok(false);
    }
    for other in &safety.participants {
        if other.thread_id == safety.thread_id || other.deleted {
            continue;
        }
        if other.side_chat
            && !other.active_run
            && !other.has_mutating_turn_items
            && other.worktree_path == safety.worktree_path
        {
            continue;
        }
        let mut paths: Vec<_> = other
            .worktree_path
            .iter()
            .chain(other.scope_paths.iter())
            .collect();
        paths.extend(
            other
                .provider_sessions
                .iter()
                .filter(|session| !session.stopped && !session.supports_multiple_provider_threads)
                .map(|session| &session.cwd),
        );
        if other.worktree_path.is_none() {
            let Some(root) = &other.project_root else {
                return Ok(false);
            };
            paths.push(root);
        }
        for path in paths {
            let path = comparison_path(path)?;
            if cwd.starts_with(&path) || path.starts_with(&cwd) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
impl Git {
    pub async fn restore_checkpoint_isolated(
        &self,
        reference: &str,
        fallback_to_head: bool,
        safety: &RestoreSafety,
    ) -> Result<bool, GitError> {
        if comparison_path(self.cwd())? != comparison_path(&safety.scope_cwd)?
            || !is_restore_isolated(safety)?
        {
            return Err(GitError::UnsafeRestore(
                SHARED_WORKSPACE_RESTORE_MESSAGE.into(),
            ));
        }
        self.restore_checkpoint(reference, fallback_to_head).await
    }
}

#[cfg(all(test, unix))]
mod path_tests {
    use super::*;
    #[test]
    fn realpath_resolves_symlinks_before_parent_components_and_missing_suffixes()
    -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        std::fs::create_dir_all(root.join("deep/target"))?;
        std::os::unix::fs::symlink(root.join("deep/target"), root.join("link"))?;
        assert_eq!(
            resolve(&root.join("link/.."))?,
            std::fs::canonicalize(root.join("deep"))?
        );
        assert_eq!(
            resolve(&root.join("link/../missing"))?,
            std::fs::canonicalize(root.join("deep"))?.join("missing")
        );
        Ok(())
    }
}

#[cfg(test)]
mod touch_tests {
    use super::*;
    #[test]
    fn clean_paths_decode_git_quotes_and_keep_os_bytes() {
        assert_eq!(
            unquote_git_path(br#""a\n\t\"\\\303\251/""#).unwrap(),
            b"a\n\t\"\\\xc3\xa9/"
        );
        assert!(unquote_git_path(br#""bad\q""#).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn name_surrogate_tags_fail_closed_except_true_symlinks() {
        assert!(classify_tag(0xa000000c) == LinkKind::Symlink);
        assert!(classify_tag(0xa0000003).is_mount());
        for tag in [0xa0000019, 0xa000001d, 0xa0000027, 0x20000042] {
            assert!(classify_tag(tag).is_mount(), "name surrogate {tag:#x}");
        }
        for tag in [0x9000001a, 0x80000017, 0x8000001b, 0] {
            assert!(classify_tag(tag) == LinkKind::None);
        }
    }
    #[cfg(windows)]
    #[test]
    fn native_clean_guard_requires_a_supported_windows_release() {
        for version in [
            "git version 2.23.0.windows.1",
            "git version 2.52.0.windows.1\n",
        ] {
            assert!(clean_mount_guard_version(version), "{version}");
        }
        for version in [
            "git version 2.22.0.windows.1",
            "git version 2.23.0.windows.0",
            "git version 2.23.0.rc1.windows.1",
            "git version 2.52.0",
            "unknown",
        ] {
            assert!(!clean_mount_guard_version(version), "{version}");
        }
    }
}
