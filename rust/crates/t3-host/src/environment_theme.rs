use crate::{Error, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use notify::Watcher;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use t3_contracts::server::{EnvironmentTheme, EnvironmentThemeFile};
use tokio::sync::{Mutex, watch};

pub const MAX_THEME_FILES: usize = 32;
pub const MAX_THEME_FILE_BYTES: u64 = 32 * 1024;
pub const MAX_THEME_TOTAL_BYTES: usize = 192 * 1024;
const RESERVED: &[&str] = &[
    "system",
    "light",
    "dark",
    "t3-chat",
    "grove",
    "ocean",
    "ember",
    "iris",
    "t3-chat-dark",
    "t3-grove",
    "t3-ocean",
    "t3-ember",
    "t3-iris",
    "t3-code",
];
pub fn read_theme_file_guarded(path: &Path, max_bytes: u64) -> Option<String> {
    let directory = Dir::open_ambient_dir(path.parent()?, cap_std::ambient_authority()).ok()?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = directory.open_with(path.file_name()?, &options).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return None;
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(metadata.len())
        .read_to_end(&mut bytes)
        .ok()?;
    Some(
        String::from_utf8_lossy(&bytes)
            .trim_start_matches('\u{feff}')
            .to_owned(),
    )
}
pub fn read_published_themes(directory: &Path) -> Vec<EnvironmentTheme> {
    let mut names: Vec<_> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    names.sort();
    let mut result = Vec::new();
    let mut examined = 0;
    let mut total = 0;
    for name in names {
        let Some(id) = name.to_str().and_then(|s| s.strip_suffix(".json")) else {
            continue;
        };
        if id.is_empty()
            || id.len() > 48
            || RESERVED.contains(&id)
            || !id
                .bytes()
                .enumerate()
                .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || i > 0 && b == b'-')
        {
            continue;
        }
        examined += 1;
        if examined > MAX_THEME_FILES {
            break;
        }
        let Some(raw) = read_theme_file_guarded(&directory.join(&name), MAX_THEME_FILE_BYTES)
        else {
            continue;
        };
        let Some(file) = decode_theme(&raw) else {
            continue;
        };
        total += raw.len();
        if total > MAX_THEME_TOTAL_BYTES {
            break;
        }
        result.push(EnvironmentTheme {
            id: id.into(),
            version: file.version,
            name: file.name,
            appearance: file.appearance,
            canvas: file.canvas,
            accent: file.accent,
            colors: file.colors,
            variants: file.variants,
        });
    }
    result
}
fn decode_theme(raw: &str) -> Option<EnvironmentThemeFile> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for key in ["version", "canvas", "accent", "colors", "variants"] {
        if value.get(key).is_some_and(serde_json::Value::is_null) {
            return None;
        }
    }
    if value.get("variants").is_some_and(|variants| {
        ["light", "dark"]
            .iter()
            .any(|key| variants.get(key).is_some_and(serde_json::Value::is_null))
    }) {
        return None;
    }
    let mut file: EnvironmentThemeFile = serde_json::from_value(value).ok()?;
    file.name.0 = file.name.0.trim().to_owned();
    if file.name.0.is_empty() || file.name.0.encode_utf16().count() > 48 {
        return None;
    }
    for color in [file.canvas.as_ref(), file.accent.as_ref()]
        .into_iter()
        .flatten()
    {
        let color = color.as_ref()?;
        let hex = color.strip_prefix('#')?;
        if !matches!(hex.len(), 3 | 6) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
    }
    fn colors(map: &mut t3_contracts::server::EnvironmentThemeColors) -> Option<()> {
        for (k, v) in map {
            if k.is_empty()
                || k.len() > 64
                || !k
                    .bytes()
                    .enumerate()
                    .all(|(i, b)| b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit())
            {
                return None;
            }
            v.0 = v.0.trim().to_owned();
            if v.0.is_empty() || v.0.encode_utf16().count() > 64 {
                return None;
            }
        }
        Some(())
    }
    if let Some(Some(map)) = &mut file.colors {
        colors(map)?;
    }
    if let Some(Some(variants)) = &mut file.variants {
        if let Some(Some(map)) = &mut variants.light {
            colors(map)?;
        }
        if let Some(Some(map)) = &mut variants.dark {
            colors(map)?;
        }
    }
    if !(file.canvas.as_ref().is_some_and(Option::is_some)
        && file.accent.as_ref().is_some_and(Option::is_some)
        || file
            .colors
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|m| !m.is_empty()))
    {
        return None;
    }
    Some(file)
}
pub struct EnvironmentThemes {
    directory: PathBuf,
    published: watch::Sender<Vec<EnvironmentTheme>>,
    gate: Mutex<()>,
}
pub struct ThemeWatch {
    _watcher: notify::RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for ThemeWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl EnvironmentThemes {
    pub fn new(directory: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            directory,
            published: watch::channel(Vec::new()).0,
            gate: Mutex::new(()),
        })
    }
    pub async fn current(&self) -> Vec<EnvironmentTheme> {
        let _guard = self.gate.lock().await;
        let themes = read_published_themes(&self.directory);
        self.published.send_if_modified(|current| {
            if *current == themes {
                return false;
            }
            *current = themes.clone();
            true
        });
        themes
    }
    pub async fn stream_changes(&self) -> watch::Receiver<Vec<EnvironmentTheme>> {
        let subscription = self.published.subscribe();
        self.current().await;
        subscription
    }
    pub async fn start(self: &Arc<Self>) -> Result<ThemeWatch> {
        fs::create_dir_all(&self.directory).map_err(|_| Error::Io)?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    let _ = tx.send(());
                }
            })
            .map_err(|_| Error::Service)?;
        watcher
            .watch(&self.directory, notify::RecursiveMode::NonRecursive)
            .map_err(|_| Error::Service)?;
        self.current().await;
        let service = self.clone();
        let task = tokio::spawn(async move {
            while rx.recv().await.is_some() {
                let delay = tokio::time::sleep(std::time::Duration::from_millis(100));
                tokio::pin!(delay);
                loop {
                    tokio::select! {_= &mut delay=>break,event=rx.recv()=>{if event.is_none(){return;}delay.as_mut().reset(tokio::time::Instant::now()+std::time::Duration::from_millis(100));}}
                }
                service.current().await;
            }
        });
        Ok(ThemeWatch {
            _watcher: watcher,
            task,
        })
    }
}
