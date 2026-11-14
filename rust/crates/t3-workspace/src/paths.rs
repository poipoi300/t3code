use crate::{Error, Result, io_error};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    validate_host_path(path)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "unsafe path"))?;
    reject_network_links(path)?;
    // Dunce keeps the verbatim prefix when the plain spelling would exceed `MAX_PATH`; a
    // verbatim disk path is local and is returned in the plain spelling callers compare with.
    let real = without_verbatim_disk_prefix(dunce::canonicalize(path)?);
    if is_network_path(&real.to_string_lossy()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "network path",
        ));
    }
    Ok(real)
}

/// Two leading separators: UNC (`\\host\share`), verbatim UNC (`\\?\UNC\…`), volume GUID and
/// device (`\\.\…`) paths. A verbatim disk path (`\\?\C:\…`) is local.
pub fn is_network_path(value: &str) -> bool {
    value
        .as_bytes()
        .get(..2)
        .is_some_and(|p| p.iter().all(|b| *b == b'/' || *b == b'\\'))
        && verbatim_disk_rest(value).is_none()
}

/// `C:\…` from `\\?\C:\…` (either separator), else `None`.
fn verbatim_disk_rest(value: &str) -> Option<&str> {
    let rest = value
        .strip_prefix(r"\\?\")
        .or_else(|| value.strip_prefix("//?/"))?;
    let bytes = rest.as_bytes();
    let disk = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes.get(2).is_none_or(|b| matches!(b, b'\\' | b'/'));
    disk.then_some(rest)
}

fn without_verbatim_disk_prefix(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(verbatim_disk_rest) {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

pub fn expand_home(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        home.to_owned()
    } else if let Some(suffix) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        home.join(format!("./{}", suffix.trim_start_matches(['/', '\\'])))
    } else {
        PathBuf::from(value)
    }
}

pub fn validate_host_path(path: &Path) -> Result<()> {
    let text = path
        .to_str()
        .ok_or_else(|| Error::Invalid("path must be valid Unicode".into()))?;
    // A client never needs a verbatim spelling; it bypasses the checks below.
    if text.contains('\0') || is_network_path(text) || verbatim_disk_rest(text).is_some() {
        return Err(Error::OutsideRoot(text.into()));
    }
    #[cfg(windows)]
    if let Some(Component::Prefix(prefix)) = path.components().next()
        && let std::path::Prefix::Disk(letter) = prefix.kind()
        && winsafe::GetDriveType(Some(&format!("{}:\\", char::from(letter))))
            == winsafe::co::DRIVE::REMOTE
    {
        return Err(Error::OutsideRoot(text.into()));
    }
    for component in path.components() {
        if let Component::Normal(name) = component {
            let name = name
                .to_str()
                .ok_or_else(|| Error::Invalid("path must be valid Unicode".into()))?;
            if cfg!(windows) {
                let stem = name
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .trim_end()
                    .to_uppercase();
                if name.contains(':')
                    || name.ends_with(['.', ' '])
                    || matches!(
                        stem.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                    )
                    || ["COM", "LPT"].iter().any(|p| {
                        stem.strip_prefix(p).is_some_and(|tail| {
                            matches!(
                                tail,
                                "0" | "1"
                                    | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "¹"
                                    | "²"
                                    | "³"
                            )
                        })
                    })
                {
                    return Err(Error::OutsideRoot(text.into()));
                }
            }
        }
    }
    Ok(())
}
fn reject_network_links(path: &Path) -> io::Result<()> {
    fn inspect(path: &Path, hops: usize) -> io::Result<()> {
        validate_host_path(path).map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "network or invalid link target",
            )
        })?;
        if hops >= 40 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "too many path links",
            ));
        }
        let mut prefix = PathBuf::new();
        let components: Vec<_> = path.components().collect();
        for (index, component) in components.iter().enumerate() {
            prefix.push(component);
            if fs::symlink_metadata(&prefix).is_ok_and(|m| m.is_symlink()) {
                let target = fs::read_link(&prefix)?;
                if is_network_path(&target.to_string_lossy()) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "network link",
                    ));
                }
                let resolved = prefix.parent().unwrap_or(Path::new("")).join(target);
                let mut resolved = resolved;
                for suffix in &components[index + 1..] {
                    resolved.push(suffix);
                }
                return inspect(&resolved, hops + 1);
            }
        }
        Ok(())
    }
    inspect(path, 0)
}
pub fn relative_to(candidate: &Path, root: &Path) -> Result<PathBuf> {
    if !within(candidate, root) {
        return Err(Error::PathEscape(candidate.into()));
    }
    Ok(candidate
        .components()
        .skip(root.components().count())
        .collect())
}
pub(crate) fn bound_text(value: &str, max: usize, empty: bool, label: &str) -> Result<()> {
    if (!empty && value.is_empty()) || value.encode_utf16().count() > max || value.contains('\0') {
        return Err(Error::Invalid(format!("invalid {label}")));
    }
    Ok(())
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| io_error("current-directory", path, e))?
            .join(path)
    };
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}

pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

pub fn within(candidate: &Path, root: &Path) -> bool {
    let candidate: Vec<_> = candidate.components().collect();
    let root: Vec<_> = root.components().collect();
    candidate.len() >= root.len()
        && candidate.iter().zip(&root).all(|(a, b)| {
            if cfg!(windows) {
                a.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
            } else {
                a == b
            }
        })
}

pub fn resolve_relative(root: &Path, value: &str) -> Result<(PathBuf, String)> {
    let path = Path::new(value);
    validate_host_path(path)?;
    if value.contains('\0')
        || path.is_absolute()
        || path.has_root()
        || path.components().any(|c| matches!(c, Component::Prefix(_)))
    {
        return Err(Error::OutsideRoot(value.into()));
    }
    let root = absolute(root)?;
    let target = absolute(&root.join(path))?;
    if target == root || !within(&target, &root) {
        return Err(Error::OutsideRoot(value.into()));
    }
    let relative = target
        .components()
        .skip(root.components().count())
        .collect::<PathBuf>();
    Ok((target, display_path(&relative)))
}

pub async fn normalize_root(value: &str, home: &Path, create: bool) -> Result<PathBuf> {
    if value.trim().is_empty() {
        return Err(Error::Invalid("workspace root is empty".into()));
    }
    let expanded = expand_home(value.trim(), home);
    validate_host_path(&expanded)?;
    if expanded
        .components()
        .any(|c| matches!(c, Component::Prefix(_)))
        && !expanded.is_absolute()
    {
        return Err(Error::OutsideRoot(value.into()));
    }
    let path = absolute(&expanded)?;
    let owned = path.clone();
    crate::blocking(move || {
        reject_network_links(&owned).map_err(|e| io_error("inspect-workspace-root", &owned, e))?;
        if create {
            fs::create_dir_all(&owned).map_err(|e| io_error("create-workspace-root", &owned, e))?;
        }
        if owned.exists() {
            canonicalize(&owned).map_err(|e| io_error("realpath-workspace-root", &owned, e))?;
        }
        match fs::metadata(&owned) {
            Ok(info) if info.is_dir() => Ok(owned),
            Ok(_) => Err(Error::RootNotDirectory(owned)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(Error::RootMissing(owned)),
            Err(e) => Err(io_error("stat-workspace-root", &owned, e)),
        }
    })
    .await
}

/// Resolve missing tails without treating a dangling symlink as absence.
pub fn nearest_real(path: &Path) -> Result<(PathBuf, bool)> {
    let mut candidate = path;
    let mut tail = Vec::new();
    loop {
        match canonicalize(candidate) {
            Ok(mut real) => {
                for part in tail.iter().rev() {
                    real.push(part);
                }
                return Ok((real, tail.is_empty()));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(candidate) {
                    Ok(_) => return Err(io_error("realpath-target", candidate, e)),
                    Err(meta) if meta.kind() == io::ErrorKind::NotFound => {}
                    Err(meta) => return Err(io_error("stat", candidate, meta)),
                }
                let Some(name) = candidate.file_name() else {
                    return Err(io_error("realpath-target", candidate, e));
                };
                tail.push(name.to_owned());
                candidate = candidate
                    .parent()
                    .ok_or_else(|| io_error("realpath-target", candidate, e))?;
            }
            Err(e) => return Err(io_error("realpath-target", candidate, e)),
        }
    }
}

pub fn is_git_name(segment: &str) -> bool {
    let normalized: String=segment.chars().filter(|c|!matches!(*c,'\u{200c}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{206a}'..='\u{206f}'|'\u{feff}')).collect();
    let name = normalized
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches(['.', ' ']);
    matches!(name.to_uppercase().as_str(), ".GIT" | "GIT~1")
}

pub fn in_git_folder(path: &Path) -> Result<bool> {
    if path
        .components()
        .any(|s| is_git_name(&s.as_os_str().to_string_lossy()))
    {
        return Ok(true);
    }
    let mut folder = path.parent();
    while let Some(dir) = folder {
        let stat = |name: &str| -> Result<Option<fs::Metadata>> {
            let p = dir.join(name);
            match fs::symlink_metadata(&p) {
                Ok(m) => Ok(Some(m)),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    Ok(None)
                }
                Err(e) => Err(io_error("stat", p, e)),
            }
        };
        if stat("HEAD")?.is_some()
            && (stat("commondir")?.is_some()
                || (stat("objects")?.is_some() && stat("refs")?.is_some()))
        {
            return Ok(true);
        }
        if stat(".git")?.is_some_and(|m| m.is_file()) {
            return Ok(false);
        }
        folder = dir.parent();
    }
    Ok(false)
}

#[derive(Debug, Clone)]
pub struct InternalPaths {
    pub state_dir: PathBuf,
    pub secrets_dir: PathBuf,
}
impl InternalPaths {
    pub fn contains(&self, path: &Path) -> bool {
        let real = |p: &Path| canonicalize(p).unwrap_or_else(|_| p.to_owned());
        let file = real(path);
        within(&file, &real(&self.secrets_dir))
            || (file.parent() == Some(real(&self.state_dir).as_path())
                && file
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().contains(".sqlite")))
            || within(&file, Path::new("/dev/fd"))
    }
}

/// Authoritative roots supplied by orchestration after resolving managed/thread worktrees.
#[derive(Debug, Clone, Default)]
pub struct KnownWorkspaces {
    roots: Vec<PathBuf>,
}
pub type WorkspaceRootsFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<KnownWorkspaces>> + Send + 'a>>;
pub trait WorkspaceRoots: Send + Sync {
    fn load(&self) -> WorkspaceRootsFuture<'_>;
}
impl<F, Fut> WorkspaceRoots for F
where
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<KnownWorkspaces>> + Send + 'static,
{
    fn load(&self) -> WorkspaceRootsFuture<'_> {
        Box::pin(self())
    }
}
impl KnownWorkspaces {
    pub async fn from_project_snapshot(
        projects: &[t3_contracts::project::Project],
        managed_directories: Vec<PathBuf>,
        thread_worktrees: &[(String, PathBuf)],
    ) -> Result<Self> {
        let mut roots = managed_directories;
        roots.extend(
            projects
                .iter()
                .filter(|project| project.deleted_at.is_none())
                .map(|project| PathBuf::from(&project.workspace_root.0)),
        );
        let mut known = Self::new(roots)?;
        let mut listed = std::collections::HashMap::<String, Vec<PathBuf>>::new();
        for (project_id, worktree) in thread_worktrees {
            if !worktree.is_absolute() {
                continue;
            }
            let Ok(real) = canonicalize(worktree) else {
                continue;
            };
            if known.contains(&real) {
                continue;
            }
            let Some(project) = projects
                .iter()
                .find(|p| p.id.0 == *project_id && p.deleted_at.is_none())
            else {
                continue;
            };
            if !listed.contains_key(project_id) {
                let mut paths = Vec::new();
                if let Ok(output) = crate::search::git(
                    Path::new(&project.workspace_root.0),
                    &["worktree", "list", "--porcelain"],
                    None,
                    8 * 1024 * 1024,
                )
                .await
                    && output.code == Some(0)
                {
                    for record in output.stdout_text().split("\n\n") {
                        if record.lines().any(|line| line.starts_with("prunable")) {
                            continue;
                        }
                        if let Some(path) = record
                            .lines()
                            .find_map(|line| line.strip_prefix("worktree "))
                            && let Ok(path) = canonicalize(path.trim_end_matches('\r'))
                        {
                            paths.push(path);
                        }
                    }
                }
                listed.insert(project_id.clone(), paths);
            }
            if listed
                .get(project_id)
                .is_some_and(|paths| paths.contains(&real))
                && real.parent().is_some()
            {
                known.roots.push(real);
            }
        }
        Ok(known)
    }
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Result<Self> {
        let mut resolved = Vec::new();
        for root in roots {
            let root = absolute(&root)?;
            let root = match canonicalize(&root) {
                Ok(real) => real,
                Err(e) if e.kind() == io::ErrorKind::NotFound => root,
                Err(_) => continue,
            };
            if root.parent().is_some() {
                resolved.push(root);
            }
        }
        Ok(Self { roots: resolved })
    }
    pub fn contains(&self, real: &Path) -> bool {
        self.roots.iter().any(|r| within(real, r))
    }
    pub fn assert_contains(&self, cwd: &Path) -> Result<()> {
        let real = canonicalize(absolute(cwd)?).map_err(|_| Error::UnknownWorkspace(cwd.into()))?;
        if self.contains(&real) {
            Ok(())
        } else {
            Err(Error::UnknownWorkspace(cwd.into()))
        }
    }
}
