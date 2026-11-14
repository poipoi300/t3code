//! Picking files for the composer (`HostAction::PickResources`) and reading what was picked
//! (`ReadResource`), as the TypeScript desktop does with its renderer's `<input type="file">`.
//! The dialog is a seam: desktop builds use the OS dialog through `rfd`, tests a fake.
use futures::future::BoxFuture;
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf};
use t3_client::platform::{HostReply, HostResource, PlatformError};

/// Asks the user for files; `None` when they cancel.
pub trait FileDialog: Send + Sync + 'static {
    fn pick_files(&self, multiple: bool) -> BoxFuture<'static, Option<Vec<PathBuf>>>;
}

/// The OS file dialog: Windows' `IFileOpenDialog`, macOS' `NSOpenPanel` (on the main thread, as
/// AppKit requires), the XDG desktop portal on Linux.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub struct SystemFileDialog;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl FileDialog for SystemFileDialog {
    fn pick_files(&self, multiple: bool) -> BoxFuture<'static, Option<Vec<PathBuf>>> {
        use futures::FutureExt;
        let dialog = rfd::AsyncFileDialog::new();
        async move {
            let picked = if multiple {
                dialog.pick_files().await
            } else {
                dialog.pick_file().await.map(|file| vec![file])
            };
            picked.map(|files| {
                files
                    .into_iter()
                    .map(|file| file.path().to_owned())
                    .collect()
            })
        }
        .boxed()
    }
}

/// What this run picked, by the handle the composer holds.
#[derive(Default)]
pub(crate) struct PickedFiles(RefCell<BTreeMap<String, PathBuf>>);

impl PickedFiles {
    pub(crate) fn contains(&self, handle: &str) -> bool {
        self.0.borrow().contains_key(handle)
    }
}

/// The type the composer files an attachment under; the server checks the bytes again.
fn mime_of(path: &std::path::Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        _ => "application/octet-stream",
    }
}

/// Opens the dialog; a cancel is an empty selection, not an error.
pub(crate) async fn pick(
    dialog: &dyn FileDialog,
    picked: &PickedFiles,
    random: impl Fn(usize) -> Result<Vec<u8>, PlatformError>,
    multiple: bool,
) -> Result<HostReply, PlatformError> {
    let Some(paths) = dialog.pick_files(multiple).await else {
        return Ok(HostReply::Resources(Vec::new()));
    };
    let mut resources = Vec::with_capacity(paths.len());
    for path in paths {
        let handle: String = random(16)?
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let handle = format!("picked-{handle}");
        resources.push(HostResource {
            handle: handle.clone(),
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            mime: mime_of(&path).to_owned(),
        });
        picked.0.borrow_mut().insert(handle, path);
    }
    Ok(HostReply::Resources(resources))
}

/// The picked file's bytes, refused over `max_bytes` before reading them.
pub(crate) fn read(
    picked: &PickedFiles,
    handle: &str,
    max_bytes: usize,
) -> Result<std::path::PathBuf, PlatformError> {
    let path = picked
        .0
        .borrow()
        .get(handle)
        .cloned()
        .ok_or_else(|| PlatformError::InvalidData("unknown file handle".into()))?;
    let length = std::fs::metadata(&path).map_err(unreadable)?.len();
    if length > max_bytes as u64 {
        return Err(PlatformError::TooLarge);
    }
    Ok(path)
}

/// Reads at most `max_bytes`; a file that grew past it since the check is refused too.
pub(crate) fn read_bytes(
    path: &std::path::Path,
    max_bytes: usize,
) -> Result<Vec<u8>, PlatformError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(unreadable)?
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() > max_bytes {
        return Err(PlatformError::TooLarge);
    }
    Ok(bytes)
}

fn unreadable(error: std::io::Error) -> PlatformError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            PlatformError::Denied("the file cannot be read".into())
        }
        _ => PlatformError::Storage("the file cannot be read".into()),
    }
}
