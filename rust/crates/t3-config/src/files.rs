use crate::{Error, Result};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        operation,
        path: path.to_owned(),
        source,
    }
}

pub(crate) fn read(path: &Path) -> Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            Ok(Some(
                text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io("read", path, error)),
    }
}

pub(crate) fn target(path: &Path) -> Result<PathBuf> {
    let mut current = path.to_owned();
    for _ in 0..40 {
        match fs::read_link(&current) {
            Ok(link) => {
                current = if link.is_absolute() {
                    link
                } else {
                    current.parent().unwrap_or(Path::new(".")).join(link)
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
                ) =>
            {
                return Ok(current);
            }
            #[cfg(windows)]
            Err(error) if error.raw_os_error() == Some(4390) => return Ok(current),
            Err(error) => return Err(io("resolve symlink", &current, error)),
        }
    }
    Err(crate::invalid(
        path.display().to_string(),
        "too many symbolic links",
    ))
}

pub(crate) async fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let path = target(path)?;
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| io("create directory", parent, e))?;
    let temp = tempfile::Builder::new()
        .prefix(&format!(
            "{}.",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
        .tempdir_in(parent)
        .map_err(|e| io("create temporary directory", parent, e))?;
    let source = temp.path().join("contents.tmp");
    fs::write(&source, contents).map_err(|e| io("write", &source, e))?;
    let before = file_version(&path);
    let first = match fs::rename(&source, &path) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if cfg!(windows) && windows_lock(&first) && before.as_ref().is_ok_and(|v| !v.readonly) {
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if file_version(&path).ok() != before.as_ref().ok().cloned() {
                break;
            }
            match fs::rename(&source, &path) {
                Ok(()) => return Ok(()),
                Err(e) if windows_lock(&e) => (),
                Err(e) => return Err(io("rename", &path, e)),
            }
        }
    }
    Err(io("rename", &path, first))
}

#[derive(Clone, PartialEq)]
struct FileVersion {
    identity: Option<file_id::FileId>,
    len: u64,
    modified: Option<std::time::SystemTime>,
    readonly: bool,
}
fn file_version(path: &Path) -> std::io::Result<FileVersion> {
    match fs::metadata(path) {
        Ok(m) => Ok(FileVersion {
            identity: Some(file_id::get_file_id(path)?),
            len: m.len(),
            modified: m.modified().ok(),
            readonly: m.permissions().readonly(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileVersion {
            identity: None,
            len: 0,
            modified: None,
            readonly: false,
        }),
        Err(e) => Err(e),
    }
}
fn windows_lock(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

pub(crate) async fn write_json(
    path: &Path,
    value: &impl serde::Serialize,
    pretty: bool,
) -> Result<()> {
    let mut bytes = if pretty {
        serde_json::to_vec_pretty(value)?
    } else {
        serde_json::to_vec(value)?
    };
    bytes.push(b'\n');
    atomic_write(path, &bytes).await
}

pub(crate) fn publish_if_absent(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| io("create directory", parent, e))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| io("create temporary file", parent, e))?;
    temp.write_all(contents).map_err(|e| io("write", path, e))?;
    match fs::hard_link(temp.path(), path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(io("publish", path, e)),
    }
}

pub(crate) fn lenient_json(raw: &str) -> Result<Value> {
    let chars: Vec<char> = raw.chars().collect();
    let mut stripped = String::new();
    let (mut i, mut quoted, mut escape) = (0, false, false);
    while i < chars.len() {
        let c = chars[i];
        if quoted {
            stripped.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
            stripped.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            stripped.push(' ');
            i += 2;
            while i < chars.len() && !matches!(chars[i], '\n' | '\r') {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            if let Some(end) = chars[i + 2..].windows(2).position(|w| w == ['*', '/']) {
                stripped.push(' ');
                i += end + 4;
                continue;
            }
            stripped.push(c);
        } else {
            stripped.push(c);
        }
        i += 1;
    }
    let chars: Vec<char> = stripped.chars().collect();
    let mut clean = String::new();
    (quoted, escape) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if !quoted
            && c == ','
            && chars[i + 1..]
                .iter()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| matches!(c, '}' | ']'))
        {
            continue;
        }
        clean.push(c);
        if quoted {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        }
    }
    Ok(serde_json::from_str(&clean)?)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[tokio::test(start_paused = true)]
    async fn rename_retry_stops_when_an_external_writer_changes_the_target() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        fs::write(&path, b"original").unwrap();
        let _locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let save = atomic_write(&path, b"replacement");
        tokio::pin!(save);
        tokio::select! { biased; r = &mut save => panic!("rename was not blocked: {r:?}"), _ = async {} => () }
        fs::write(&path, b"external change").unwrap();
        assert!(save.await.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"external change");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
