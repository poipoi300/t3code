use std::{
    collections::BTreeMap,
    future::Future,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub const MAX_READ_BYTES: usize = 256 * 1024;
pub const MAX_LOCATE_ATTEMPTS: usize = 40;

pub fn claude_task_output_root(
    environment: &BTreeMap<String, String>,
    cwd: &Path,
    platform: &str,
    uid: Option<u32>,
) -> Option<PathBuf> {
    let uid = uid?;
    let fallback = if platform == "darwin" {
        "/tmp"
    } else {
        ["TMPDIR", "TMP", "TEMP"]
            .into_iter()
            .find_map(|key| {
                environment
                    .get(key)
                    .filter(|v| !v.is_empty())
                    .map(String::as_str)
            })
            .unwrap_or("/tmp")
    };
    let base = environment
        .get("CLAUDE_CODE_TMPDIR")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback);
    let path = cwd.join(base).join(format!("claude-{uid}"));
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if component == std::path::Component::ParentDir {
            normalized.pop();
        } else if component != std::path::Component::CurDir {
            normalized.push(component);
        }
    }
    Some(normalized)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', ':'])
}

pub fn locate(root: &Path, session_id: &str, task_id: &str) -> Option<PathBuf> {
    if !safe_component(session_id) || !safe_component(task_id) {
        return None;
    }
    let mut projects = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|v| v.path())
        .collect::<Vec<_>>();
    projects.sort();
    projects
        .into_iter()
        .map(|project| {
            project
                .join(session_id)
                .join("tasks")
                .join(format!("{task_id}.output"))
        })
        .find(|file| file.is_file())
}

#[derive(Default)]
pub struct OutputCursor {
    offset: u64,
    partial: Vec<u8>,
}
impl OutputCursor {
    pub fn read(&mut self, path: &Path, final_read: bool) -> std::io::Result<String> {
        let mut file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        if size < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        let start = self.offset.max(size.saturating_sub(MAX_READ_BYTES as u64));
        let skipped = start > self.offset;
        let mut bytes = Vec::with_capacity((size - start) as usize);
        file.seek(SeekFrom::Start(start))?;
        file.take(MAX_READ_BYTES as u64).read_to_end(&mut bytes)?;
        self.offset = start + bytes.len() as u64;
        if skipped {
            self.partial.clear();
            let first = bytes
                .iter()
                .position(|b| b & 0xc0 != 0x80)
                .unwrap_or(bytes.len());
            bytes.drain(..first);
        }
        self.partial.extend(bytes);
        let mut output = if skipped {
            "\n".to_owned()
        } else {
            String::new()
        };
        let mut first = 0;
        loop {
            match std::str::from_utf8(&self.partial[first..]) {
                Ok(text) => {
                    output.push_str(text);
                    first = self.partial.len();
                    break;
                }
                Err(error) => {
                    let valid = first + error.valid_up_to();
                    output.push_str(&String::from_utf8_lossy(&self.partial[first..valid]));
                    first = valid;
                    match error.error_len() {
                        Some(length) => {
                            output.push('\u{fffd}');
                            first += length;
                        }
                        None => break,
                    }
                }
            }
        }
        self.partial.drain(..first);
        if final_read && !self.partial.is_empty() {
            output.push_str(&String::from_utf8_lossy(&self.partial));
            self.partial.clear();
        }
        Ok(output)
    }
}

pub async fn tail_claude_task_output<F, Fut>(
    root: PathBuf,
    session_id: String,
    task_id: String,
    stop: CancellationToken,
    mut on_chunk: F,
) where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut found = None;
    for attempt in 0..MAX_LOCATE_ATTEMPTS {
        found = locate(&root, &session_id, &task_id);
        if found.is_some() {
            break;
        }
        if attempt + 1 == MAX_LOCATE_ATTEMPTS {
            return;
        }
        tokio::select! { _ = stop.cancelled() => return,
        _ = tokio::time::sleep(Duration::from_millis(250)) => {} }
    }
    let Some(file) = found else {
        return;
    };
    let mut cursor = OutputCursor::default();
    loop {
        let final_read = stop.is_cancelled();
        if let Ok(chunk) = cursor.read(&file, final_read)
            && !chunk.is_empty()
        {
            on_chunk(chunk).await;
        }
        if final_read {
            return;
        }
        tokio::select! { _ = stop.cancelled() => {},
        _ = tokio::time::sleep(Duration::from_millis(250)) => {} }
    }
}
