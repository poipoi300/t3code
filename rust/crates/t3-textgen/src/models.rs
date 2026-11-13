use crate::{CancellationToken, speech::SpeechError};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const DEFAULT_SPEECH_MODEL_ID: &str = "handy-computer/parakeet-unified-en-0.6b-gguf";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechModel {
    pub id: String,
    pub revision: String,
    pub name: String,
    pub description: String,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub license: String,
    pub languages: Vec<String>,
    pub benchmark: serde_json::Value,
    pub recommended: bool,
    pub supports_streaming: bool,
    pub supports_language_detection: bool,
    pub supports_translation: bool,
}
pub fn catalog() -> Vec<SpeechModel> {
    #[derive(Deserialize)]
    struct Catalog {
        models: Vec<SpeechModel>,
    }
    serde_json::from_str::<Catalog>(include_str!("../fixtures/catalog.json"))
        .expect("catalog fixture")
        .models
}
pub fn silero() -> SpeechModel {
    SpeechModel {
        id: "handy-computer/silero-vad-v6.2-gguf".into(),
        revision: "97bbd75179f535e4708bccdf61e9278b8b60dfc7".into(),
        name: "Silero VAD".into(),
        description: String::new(),
        filename: "silero-vad-v6.2-F32.gguf".into(),
        size: 1_241_056,
        sha256: "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6".into(),
        license: "MIT".into(),
        languages: Vec::new(),
        benchmark: serde_json::Value::Null,
        recommended: false,
        supports_streaming: true,
        supports_language_detection: false,
        supports_translation: false,
    }
}

fn identity(model: &SpeechModel, path: &Path) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() != model.size {
        return None;
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    same_file::Handle::from_path(path).ok()?.hash(&mut hash);
    Some(format!(
        "rust:{}:{}:{:?}:{:x}",
        model.sha256,
        metadata.len(),
        metadata.modified().ok()?,
        hash.finish()
    ))
}
fn digest_matches(model: &SpeechModel, path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let Ok(n) = file.read(&mut buffer) else {
            return false;
        };
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    format!("{:x}", hash.finalize()) == model.sha256
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}
pub trait DownloadProgress: Send + Sync {
    fn progress(&self, _bytes: u64) {}
    fn verifying(&self) {}
}
impl DownloadProgress for () {}

#[derive(Clone)]
pub struct ModelStore {
    directory: PathBuf,
    home: PathBuf,
    platform: String,
    client: reqwest::Client,
    corrupt: Arc<Mutex<HashMap<PathBuf, String>>>,
}
impl ModelStore {
    pub fn new(
        state_directory: &Path,
        home: PathBuf,
        platform: String,
    ) -> Result<Self, SpeechError> {
        let directory = state_directory.join("speech/models");
        fs::create_dir_all(&directory).map_err(|_| SpeechError::Storage)?;
        let partial=regex::Regex::new(r"(?i)\.gguf\.[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.part$").expect("partial filename");
        for entry in fs::read_dir(&directory)
            .map_err(|_| SpeechError::Storage)?
            .flatten()
        {
            if partial.is_match(&entry.file_name().to_string_lossy()) {
                let _ = fs::remove_file(entry.path());
            }
        }
        // Model downloads follow Hugging Face's CDN redirects; no credentials are attached.
        let client = reqwest::Client::builder()
            .build()
            .map_err(|_| SpeechError::Download)?;
        Ok(Self {
            directory,
            home,
            platform,
            client,
            corrupt: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    pub fn path(&self, model: &SpeechModel) -> Result<PathBuf, SpeechError> {
        if Path::new(&model.filename).components().count() != 1
            || matches!(model.filename.as_str(), "." | "..")
            || model.filename.contains(['/', '\\', ':'])
        {
            return Err(SpeechError::ModelNotFound);
        }
        Ok(self.directory.join(&model.filename))
    }
    pub fn is_ready(&self, model: &SpeechModel) -> bool {
        let Ok(path) = self.path(model) else {
            return false;
        };
        let Some(marker) = identity(model, &path) else {
            return false;
        };
        self.corrupt
            .lock()
            .is_ok_and(|corrupt| corrupt.get(&path) != Some(&marker))
    }
    pub fn installed_path(&self, model: &SpeechModel) -> Option<PathBuf> {
        let path = self.path(model).ok()?;
        self.verified_path(model, &path).then_some(path)
    }
    pub async fn verified_installed_path(&self, model: &SpeechModel) -> Option<PathBuf> {
        let store = self.clone();
        let model = model.clone();
        tokio::task::spawn_blocking(move || store.installed_path(&model))
            .await
            .ok()
            .flatten()
    }
    fn verified_path(&self, model: &SpeechModel, path: &Path) -> bool {
        let Some(marker) = identity(model, path) else {
            return false;
        };
        if self
            .corrupt
            .lock()
            .is_ok_and(|c| c.get(path) == Some(&marker))
        {
            return false;
        }
        if fs::read_to_string(sidecar(path, ".verified"))
            .ok()
            .as_deref()
            == Some(&marker)
        {
            return true;
        }
        let valid = digest_matches(model, path);
        if !valid {
            if let Ok(mut corrupt) = self.corrupt.lock() {
                corrupt.insert(path.to_owned(), marker);
            }
            return false;
        }
        if identity(model, path).as_ref() != Some(&marker) {
            return false;
        }
        let _ = fs::write(sidecar(path, ".verified"), marker);
        true
    }
    pub fn linked_from(&self, model: &SpeechModel) -> Option<PathBuf> {
        let path = self.path(model).ok()?;
        fs::read_link(&path).ok().or_else(|| {
            fs::read_to_string(sidecar(&path, ".source"))
                .ok()
                .map(PathBuf::from)
        })
    }
    pub fn remove(&self, model: &SpeechModel) -> Result<(), SpeechError> {
        let path = self.path(model)?;
        for file in [
            path.clone(),
            sidecar(&path, ".verified"),
            sidecar(&path, ".source"),
        ] {
            match fs::remove_file(file) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err(SpeechError::Storage),
            }
        }
        Ok(())
    }
    pub fn link(&self, models: &[SpeechModel], source: &Path) -> Result<Vec<String>, SpeechError> {
        let mut pending = vec![(source.to_owned(), 5)];
        let mut files = Vec::new();
        let mut visited = 0;
        while let Some((entry, depth)) = pending.pop() {
            if visited >= 5000 {
                break;
            }
            visited += 1;
            let Ok(metadata) = fs::metadata(&entry) else {
                continue;
            };
            if metadata.is_file() {
                files.push((entry, metadata.len()));
            } else if metadata.is_dir()
                && depth > 0
                && let Ok(entries) = fs::read_dir(entry)
            {
                for entry in entries.flatten() {
                    pending.push((entry.path(), depth - 1));
                }
            }
        }
        let mut linked = Vec::new();
        for (file, size) in files {
            for model in models {
                if model.size != size || linked.contains(&model.id) {
                    continue;
                }
                let path = self.path(model)?;
                if fs::canonicalize(&file).ok() == fs::canonicalize(&path).ok()
                    || self.verified_path(model, &path)
                {
                    continue;
                }
                let Some(marker) = identity(model, &file) else {
                    continue;
                };
                if !digest_matches(model, &file) {
                    continue;
                }
                let staged = sidecar(&path, &format!(".{}.part", uuid::Uuid::new_v4()));
                #[cfg(windows)]
                let symbolic = std::os::windows::fs::symlink_file(&file, &staged);
                #[cfg(unix)]
                let symbolic = std::os::unix::fs::symlink(&file, &staged);
                let hard = symbolic.is_err();
                if hard {
                    fs::hard_link(&file, &staged).map_err(|_| SpeechError::Storage)?;
                }
                let guard = PartialFile(staged.clone());
                if identity(model, &staged).as_ref() != Some(&marker) {
                    continue;
                }
                replace_file(&staged, &path)?;
                if hard {
                    let _ = fs::write(sidecar(&path, ".source"), file.to_string_lossy().as_bytes());
                } else {
                    let _ = fs::remove_file(sidecar(&path, ".source"));
                }
                let _ = fs::write(sidecar(&path, ".verified"), marker);
                drop(guard);
                linked.push(model.id.clone());
            }
        }
        Ok(linked)
    }
    fn known_copies(&self, model: &SpeechModel) -> Vec<PathBuf> {
        let mut paths = vec![
            self.home
                .join(".cache/huggingface/hub")
                .join(format!("models--{}", model.id.replacen('/', "--", 1)))
                .join("snapshots")
                .join(&model.revision)
                .join(&model.filename),
        ];
        let directories = match self.platform.as_str() {
            "darwin" => vec!["Library/Application Support"],
            "win32" => vec!["AppData/Roaming"],
            _ => vec![".local/share", ".config"],
        };
        paths.extend(directories.into_iter().map(|dir| {
            self.home
                .join(dir)
                .join("com.pais.handy/models")
                .join(&model.filename)
        }));
        paths
    }
    pub fn download_url(model: &SpeechModel) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            model.id, model.revision, model.filename
        )
    }
    pub async fn download(
        &self,
        model: &SpeechModel,
        progress: &dyn DownloadProgress,
        cancellation: CancellationToken,
    ) -> Result<PathBuf, SpeechError> {
        if let Some(path) = self.verified_installed_path(model).await {
            return Ok(path);
        }
        for copy in self.known_copies(model) {
            if self
                .link(std::slice::from_ref(model), &copy)
                .is_ok_and(|ids| !ids.is_empty())
            {
                return self.path(model);
            }
        }
        self.download_from(model, &Self::download_url(model), progress, cancellation)
            .await
    }
    /// The caller owns URL authorization; production calls `download`, which uses pinned catalog URLs.
    pub async fn download_from(
        &self,
        model: &SpeechModel,
        url: &str,
        progress: &dyn DownloadProgress,
        cancellation: CancellationToken,
    ) -> Result<PathBuf, SpeechError> {
        let path = self.path(model)?;
        let partial = sidecar(&path, &format!(".{}.part", uuid::Uuid::new_v4()));
        let _cleanup = PartialFile(partial.clone());
        let work = async {
            for attempt in 0..3 {
                let offset = fs::metadata(&partial).map_or(0, |m| m.len());
                if offset == model.size {
                    break;
                }
                match self
                    .fetch_into(model, url, &partial, offset, progress)
                    .await
                {
                    Ok(downloaded) if downloaded == model.size => break,
                    Ok(_) => (),
                    Err(SpeechError::Transport) if attempt < 2 => (),
                    Err(e) => return Err(e),
                }
            }
            progress.verifying();
            let model_for_hash = model.clone();
            let file_for_hash = partial.clone();
            let valid = tokio::task::spawn_blocking(move || {
                identity(&model_for_hash, &file_for_hash).is_some()
                    && digest_matches(&model_for_hash, &file_for_hash)
            })
            .await
            .map_err(|_| SpeechError::Storage)?;
            if !valid {
                return Err(SpeechError::Verification);
            }
            replace_file(&partial, &path)?;
            let _ = fs::remove_file(sidecar(&path, ".source"));
            if let Some(marker) = identity(model, &path) {
                let _ = fs::write(sidecar(&path, ".verified"), marker);
            }
            Ok(path)
        };
        tokio::select! {biased;_=cancellation.cancelled()=>Err(SpeechError::Cancelled),result=work=>result}
    }
    async fn fetch_into(
        &self,
        model: &SpeechModel,
        url: &str,
        partial: &Path,
        offset: u64,
        progress: &dyn DownloadProgress,
    ) -> Result<u64, SpeechError> {
        let mut request = self.client.get(url);
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let response = request.send().await.map_err(|_| SpeechError::Transport)?;
        let resumed = offset > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if !resumed && response.status() != reqwest::StatusCode::OK {
            return Err(SpeechError::Download);
        }
        if resumed
            && response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                != Some(&format!("bytes {offset}-{}/{}", model.size - 1, model.size))
        {
            return Err(SpeechError::InvalidRange);
        }
        let start = if resumed { offset } else { 0 };
        if response
            .content_length()
            .is_some_and(|length| length != model.size - start)
        {
            return Err(SpeechError::InvalidSize);
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(partial)
            .map_err(|_| SpeechError::Transport)?;
        let mut downloaded = start;
        progress.progress(start);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| SpeechError::Transport)?;
            downloaded = downloaded
                .checked_add(chunk.len() as u64)
                .ok_or(SpeechError::InvalidSize)?;
            if downloaded > model.size {
                return Err(SpeechError::InvalidSize);
            }
            file.write_all(&chunk).map_err(|_| SpeechError::Transport)?;
            progress.progress(downloaded);
        }
        Ok(downloaded)
    }
}
struct PartialFile(PathBuf);
impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn replace_file(source: &Path, dest: &Path) -> Result<(), SpeechError> {
    // MoveFileEx replacement is provided by tempfile's cross-platform persist implementation.
    let staged = tempfile::NamedTempFile::from_parts(
        File::open(source).map_err(|_| SpeechError::Storage)?,
        tempfile::TempPath::try_from_path(source.to_path_buf())
            .map_err(|_| SpeechError::Storage)?,
    );
    staged.persist(dest).map_err(|_| SpeechError::Storage)?;
    Ok(())
}
