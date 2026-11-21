use crate::{
    Error, Result, blocking, io_error, paths,
    search::{self, IndexVariant, SearchIndexes, entry},
};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::{base_schemas::TrimmedNonEmptyString as Text, filesystem::*, project::*};

#[derive(Clone)]
pub struct WorkspaceEntries {
    pub home: PathBuf,
    pub indexes: Arc<SearchIndexes>,
    pub known: Arc<dyn paths::WorkspaceRoots>,
}
impl WorkspaceEntries {
    pub async fn browse(&self, input: FilesystemBrowseInput) -> Result<FilesystemBrowseResult> {
        self.browse_with_storage(input, None).await
    }
    pub async fn browse_with_storage(
        &self,
        input: FilesystemBrowseInput,
        storage: Option<PathBuf>,
    ) -> Result<FilesystemBrowseResult> {
        paths::bound_text(&input.partial_path.0, 32768, false, "browse path")?;
        let known = self.known.load().await?;
        let home = self.home.clone();
        blocking(move || {
            let storage = storage.as_deref().map(paths::AttachmentStorage::new);
            let private = |path: &Path| storage.as_ref().is_some_and(|s| s.contains(path));
            let partial = &input.partial_path.0;
            paths::validate_host_path(Path::new(partial))?;
            let explicit = [".", ".."].contains(&partial.as_str())
                || ["./", "../", ".\\", "..\\"]
                    .iter()
                    .any(|p| partial.starts_with(p));
            let resolved = if explicit {
                let cwd = input.cwd.flatten().ok_or_else(|| {
                    Error::Invalid("a current project is required for relative browsing".into())
                })?;
                paths::absolute(&paths::expand_home(&cwd.0, &home).join(partial))?
            } else {
                paths::absolute(&paths::expand_home(partial, &home))?
            };
            let dot_prefix = partial.ends_with("/.") || partial.ends_with("\\.");
            let separator = dot_prefix || partial.ends_with(['/', '\\']) || partial == "~";
            let parent = if separator {
                resolved.as_path()
            } else {
                resolved
                    .parent()
                    .ok_or_else(|| Error::Invalid("missing parent".into()))?
            };
            let prefix = if dot_prefix {
                ".".into()
            } else if separator {
                String::new()
            } else {
                resolved
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase()
            };
            let hidden = separator || prefix.starts_with('.');
            let canonical_parent =
                paths::canonicalize(parent).map_err(|e| io_error("realpath-target", parent, e))?;
            if private(parent) || private(&canonical_parent) {
                return Err(Error::AssetDenied);
            }
            known.assert_contains(&canonical_parent)?;
            let scoped = crate::scoped::ScopedDir::open_authorized(&canonical_parent, &known)?;
            let (_held, children) = match scoped.read_dir(Path::new("")) {
                Ok(entries) => entries,
                Err(Error::Io { source: e, .. })
                    if e.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    return Ok(FilesystemBrowseResult {
                        parent_path: Text(parent.to_string_lossy().into()),
                        entries: vec![],
                    });
                }
                Err(e) => return Err(e),
            };
            let mut entries = Vec::new();
            for child in children {
                let child = child.map_err(|e| io_error("read-directory", parent, e))?;
                let file_name = child.file_name();
                let Some(name) = file_name.to_str().map(str::to_owned) else {
                    continue;
                };
                if child
                    .file_type()
                    .map_err(|e| io_error("stat", parent.join(child.file_name()), e))?
                    .is_dir()
                    && name.to_lowercase().starts_with(&prefix)
                    && (hidden || !name.starts_with('.'))
                    && !private(&canonical_parent.join(child.file_name()))
                {
                    entries.push(FilesystemBrowseEntry {
                        name: Text(name),
                        full_path: Text(parent.join(child.file_name()).to_string_lossy().into()),
                    });
                }
            }
            entries.sort_by(|a, b| crate::locale_cmp(&a.name.0, &b.name.0));
            Ok(FilesystemBrowseResult {
                parent_path: Text(parent.to_string_lossy().into()),
                entries,
            })
        })
        .await
    }
    pub async fn list(&self, input: ProjectListEntriesInput) -> Result<ProjectListEntriesResult> {
        paths::bound_text(&input.cwd.0, 32768, false, "workspace root")?;
        let cwd = paths::normalize_root(&input.cwd.0, &self.home, false).await?;
        let known = self.known.load().await?;
        known.assert_contains(&cwd)?;
        let Some(directory) = input.directory_path.flatten() else {
            return self
                .indexes
                .get_authorized(&cwd, IndexVariant::Paths, &known)
                .await?
                .list()
                .await;
        };
        let cwd_copy = cwd.clone();
        let relative = directory.0;
        paths::bound_text(&relative, 512, true, "directory path")?;
        let relative_copy = relative.clone();
        let (mut entries, nested_roots, through_symlink) =
            blocking(move || list_directory(&cwd_copy, &relative_copy, &known)).await?;
        let mut git_root = String::new();
        let mut prefix = String::new();
        for nested in nested_roots {
            if check_ignore(&cwd.join(&git_root), &[format!("{nested}/.git")], &prefix)
                .await
                .is_some_and(|o| o.code == Some(128))
            {
                git_root = nested;
                prefix = format!("{git_root}/");
            }
        }
        let mut ignored = HashSet::new();
        let directory_git_path = relative.strip_prefix(&prefix).unwrap_or(&relative);
        if entries.len() > 1000
            && !directory_git_path.is_empty()
            && !through_symlink
            && !directory_git_path.starts_with(':')
            && !directory_git_path.contains(['*', '?', '[', '\\'])
            && check_ignore(
                &cwd.join(&git_root),
                std::slice::from_ref(&relative),
                &prefix,
            )
            .await
            .is_some_and(|out| out.code == Some(0))
        {
            for item in &mut entries {
                item.ignored = Some(Some(true));
            }
            return Ok(ProjectListEntriesResult {
                entries,
                truncated: false,
            });
        }
        for chunk in entries.chunks(1000) {
            let paths: Vec<_> = chunk.iter().map(|e| e.path.0.clone()).collect();
            let Some(result) = check_ignore(&cwd.join(&git_root), &paths, &prefix).await else {
                break;
            };
            if !matches!(result.code, Some(0 | 1)) {
                break;
            }
            for path in result.stdout_text().split('\0').filter(|s| !s.is_empty()) {
                ignored.insert(format!("{prefix}{path}"));
            }
        }
        for item in &mut entries {
            if ignored.contains(&item.path.0) {
                item.ignored = Some(Some(true));
            }
        }
        Ok(ProjectListEntriesResult {
            entries,
            truncated: false,
        })
    }
    pub async fn search(
        &self,
        input: ProjectSearchEntriesInput,
    ) -> Result<ProjectSearchEntriesResult> {
        paths::bound_text(&input.query.0, 256, true, "entry query")?;
        if !(1..=200).contains(&input.limit) {
            return Err(Error::Invalid("entry search limit must be 1..=200".into()));
        }
        paths::bound_text(&input.cwd.0, 32768, false, "workspace root")?;
        let cwd = paths::normalize_root(&input.cwd.0, &self.home, false).await?;
        let known = self.known.load().await?;
        known.assert_contains(&cwd)?;
        let query = input
            .query
            .0
            .trim()
            .trim_start_matches(['@', '.', '/'])
            .to_lowercase();
        let kind = input.kind.flatten();
        let images = input.image_only.flatten().unwrap_or(false);
        let mut result = self
            .indexes
            .get_authorized(&cwd, IndexVariant::Paths, &known)
            .await?
            .search(query, input.limit as usize, kind.clone(), images)
            .await?;
        if kind == Some(ProjectEntryKind::Directory) || images {
            return Ok(result);
        }
        let relative = input.query.0.trim().replace('\\', "/");
        if !relative.contains('/')
            || relative.ends_with('/')
            || Path::new(&relative).has_root()
            || Path::new(&relative)
                .components()
                .any(|c| matches!(c, std::path::Component::Prefix(_)))
            || relative.as_bytes().get(1) == Some(&b':')
            || relative.split('/').any(|p| p == "..")
            || result.entries.iter().any(|e| {
                e.kind == ProjectEntryKind::File
                    && (e.path.0 == relative || e.path.0.ends_with(&format!("/{relative}")))
            })
        {
            return Ok(result);
        }
        let Ok(listed) = search::git(
            &cwd,
            &["worktree", "list", "--porcelain"],
            None,
            8 * 1024 * 1024,
        )
        .await
        else {
            return Ok(result);
        };
        if listed.code != Some(0) {
            return Ok(result);
        }
        let mut extras = Vec::new();
        let real_cwd = paths::canonicalize(&cwd).unwrap_or(cwd.clone());
        for record in listed.stdout_text().split("\n\n") {
            if record.lines().any(|l| l.starts_with("prunable")) {
                continue;
            }
            let Some(root) = record.lines().find_map(|l| l.strip_prefix("worktree ")) else {
                continue;
            };
            let root = Path::new(root.trim_end_matches('\r'));
            if root == real_cwd || !paths::within(root, &real_cwd) {
                continue;
            }
            let candidate = root.join(&relative);
            if !paths::within(&candidate, &real_cwd) {
                continue;
            }
            let Ok(scope) = crate::scoped::ScopedDir::open_authorized(&real_cwd, &known) else {
                continue;
            };
            let Ok(relative_file) = paths::relative_to(&candidate, &real_cwd) else {
                continue;
            };
            if scope.open_file(&relative_file).is_ok() {
                let relative = candidate
                    .components()
                    .skip(real_cwd.components().count())
                    .collect::<PathBuf>();
                extras.push(entry(
                    paths::display_path(&relative),
                    ProjectEntryKind::File,
                ));
            }
        }
        extras.append(&mut result.entries);
        result.truncated |= extras.len() > input.limit as usize;
        extras.truncate(input.limit as usize);
        result.entries = extras;
        Ok(result)
    }
    pub async fn search_contents(
        &self,
        input: ProjectSearchContentsInput,
    ) -> Result<ProjectSearchContentsResult> {
        paths::bound_text(&input.query, 256, false, "content query")?;
        if !(1..=500).contains(&input.limit) {
            return Err(Error::Invalid(
                "content search limit must be 1..=500".into(),
            ));
        }
        paths::bound_text(&input.cwd.0, 32768, false, "workspace root")?;
        let cwd = paths::normalize_root(&input.cwd.0, &self.home, false).await?;
        let known = self.known.load().await?;
        known.assert_contains(&cwd)?;
        self.indexes
            .get_authorized(&cwd, IndexVariant::Content, &known)
            .await?
            .search_contents(input)
            .await
    }
    pub async fn list_ignored_files(
        &self,
        input: ProjectListIgnoredFilesInput,
    ) -> Result<ProjectListIgnoredFilesResult> {
        let query = input.query.as_ref().map(|q| q.0.trim()).unwrap_or("");
        paths::bound_text(query, 256, true, "ignored-file query")?;
        let query = query.to_lowercase();
        paths::bound_text(&input.cwd.0, 32768, false, "workspace root")?;
        let cwd = paths::normalize_root(&input.cwd.0, &self.home, false).await?;
        let known = self.known.load().await?;
        let scope = crate::scoped::ScopedDir::open_authorized(&cwd, &known)?;
        let cwd = scope.root.clone();
        let Ok(out) = search::git(
            &cwd,
            &[
                "-c",
                "core.fsmonitor=false",
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "--no-empty-directory",
                "-z",
            ],
            None,
            4 * 1024 * 1024,
        )
        .await
        else {
            return Ok(ProjectListIgnoredFilesResult {
                files: vec![],
                truncated: false,
            });
        };
        if out.code != Some(0) {
            return Ok(ProjectListIgnoredFilesResult {
                files: vec![],
                truncated: false,
            });
        }
        let mut paths: Vec<_> = out
            .stdout_text()
            .split('\0')
            .filter(|s| !s.is_empty() && !s.ends_with('/') && s.to_lowercase().contains(&query))
            .map(str::to_owned)
            .collect();
        paths.sort();
        let truncated = paths.len() > 200;
        let files = paths
            .into_iter()
            .take(200)
            .filter_map(|path| {
                let relative = scope.resolve(&path, false).ok()?;
                let file = scope.open_file(&relative).ok()?;
                let info = file.metadata().ok()?;
                info.is_file().then_some(ProjectIgnoredFile {
                    path: Text(path),
                    byte_length: info.len(),
                })
            })
            .collect();
        Ok(ProjectListIgnoredFilesResult { files, truncated })
    }
}
async fn check_ignore(
    cwd: &Path,
    paths: &[String],
    prefix: &str,
) -> Option<t3_process::ProcessOutput> {
    let mut input = paths
        .iter()
        .map(|p| p.strip_prefix(prefix).unwrap_or(p))
        .collect::<Vec<_>>()
        .join("\0")
        .into_bytes();
    input.push(0);
    search::git(
        cwd,
        &[
            "-c",
            "core.fsmonitor=false",
            "check-ignore",
            "-z",
            "--stdin",
        ],
        Some(input),
        16 * 1024 * 1024,
    )
    .await
    .ok()
}
fn list_directory(
    cwd: &Path,
    relative: &str,
    known: &paths::KnownWorkspaces,
) -> Result<(Vec<ProjectEntry>, Vec<String>, bool)> {
    let (target, relative) = if relative.is_empty() {
        (cwd.to_owned(), String::new())
    } else {
        paths::resolve_relative(cwd, relative)?
    };
    let root = paths::canonicalize(cwd).map_err(|e| io_error("realpath-workspace-root", cwd, e))?;
    let directory =
        paths::canonicalize(&target).map_err(|e| io_error("realpath-target", &target, e))?;
    let has_git = |p: &Path| {
        p.components()
            .any(|s| paths::is_git_name(&s.as_os_str().to_string_lossy()))
    };
    if !paths::within(&directory, &root)
        || has_git(&directory)
        || relative.split('/').any(paths::is_git_name)
    {
        return Err(Error::PathEscape(directory));
    }
    let mut visited = vec![root.clone()];
    let mut current = cwd.to_owned();
    let mut nested_roots = Vec::new();
    let mut through_symlink = false;
    for segment in relative.split('/').filter(|s| !s.is_empty()) {
        current.push(segment);
        through_symlink |= fs::symlink_metadata(&current).is_ok_and(|info| info.is_symlink())
            || is_reparse(&current);
        visited.push(
            paths::canonicalize(&current).map_err(|e| io_error("realpath-target", &current, e))?,
        );
        if fs::symlink_metadata(current.join(".git")).is_ok() {
            nested_roots.push(paths::display_path(
                current
                    .strip_prefix(cwd)
                    .map_err(|_| Error::PathEscape(current.clone()))?,
            ));
        }
    }
    let mut entries = Vec::new();
    let scope = crate::scoped::ScopedDir::open_authorized(&root, known)?;
    let real_relative = paths::relative_to(&directory, &root)?;
    let (_held, children) = scope.read_dir(&real_relative)?;
    for child in children {
        let child = child.map_err(|e| io_error("read-directory", &directory, e))?;
        let file_name = child.file_name();
        let Some(name) = file_name.to_str().map(str::to_owned) else {
            continue;
        };
        if paths::is_git_name(&name) {
            continue;
        }
        let ty = child
            .file_type()
            .map_err(|e| io_error("stat", directory.join(child.file_name()), e))?;
        let kind = if ty.is_symlink() || is_reparse(&directory.join(child.file_name())) {
            let Ok(real) = paths::canonicalize(directory.join(child.file_name())) else {
                continue;
            };
            if !paths::within(&real, &root) || has_git(&real) {
                continue;
            }
            match fs::metadata(&real) {
                Ok(m) if m.is_file() => ProjectEntryKind::File,
                Ok(m)
                    if m.is_dir()
                        && !visited
                            .iter()
                            .any(|ancestor| paths::within(ancestor, &real)) =>
                {
                    ProjectEntryKind::Directory
                }
                _ => continue,
            }
        } else if ty.is_dir() {
            ProjectEntryKind::Directory
        } else if ty.is_file() {
            ProjectEntryKind::File
        } else {
            continue;
        };
        entries.push(entry(
            if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            },
            kind,
        ));
    }
    entries.sort_by(|a, b| crate::locale_cmp(&a.path.0, &b.path.0));
    Ok((entries, nested_roots, through_symlink))
}
fn is_reparse(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}
