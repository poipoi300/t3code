use crate::{Result, attachments::hex, io_error, paths};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};
use t3_contracts::{
    assets::NativeAppIconReference, provider_runtime::ToolActivityNativeAppReference,
};
use tokio::{sync::Mutex, time::Instant};

type Cache = HashMap<String, (Option<PathBuf>, Instant)>;
pub struct NativeAppIcons {
    pub home: PathBuf,
    pub provider_status_cache_dir: PathBuf,
    cache: Mutex<Cache>,
    slots: tokio::sync::Semaphore,
}
impl NativeAppIcons {
    pub fn new(home: PathBuf, provider_status_cache_dir: PathBuf) -> Self {
        Self {
            home,
            provider_status_cache_dir,
            cache: Default::default(),
            slots: tokio::sync::Semaphore::new(2),
        }
    }
    pub async fn resolve(&self, app: &NativeAppIconReference) -> Option<PathBuf> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        if let NativeAppIconReference::Native(ToolActivityNativeAppReference::DisplayName {
            display_name,
        }) = app
            && display_name.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
        {
            return None;
        }
        let key = serde_json::to_string(app).ok()?;
        {
            let cache = self.cache.lock().await;
            if let Some((path, expires)) = cache.get(&key)
                && Instant::now() < *expires
                && path.as_ref().is_none_or(|p| p.is_file())
            {
                return path.clone();
            }
        }
        let _slot = self.slots.acquire().await.ok()?;
        let found = self.resolve_uncached(app).await.ok()?;
        let mut cache = self.cache.lock().await;
        if cache.len() >= 256
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(
            key,
            (found.clone(), Instant::now() + Duration::from_secs(3600)),
        );
        found
    }
    async fn resolve_uncached(&self, app: &NativeAppIconReference) -> Result<Option<PathBuf>> {
        let app_path = match app {
            NativeAppIconReference::Path(reference) => {
                let path = paths::expand_home(&reference.path.0, &self.home);
                if !path.is_absolute() || !path.to_string_lossy().ends_with(".app") {
                    return Ok(None);
                }
                path
            }
            NativeAppIconReference::Native(reference) => {
                let query = match reference {
                    ToolActivityNativeAppReference::AppId { app_id } => app_id_query(app_id)?,
                    ToolActivityNativeAppReference::DisplayName { display_name } => {
                        let escaped = display_name
                            .chars()
                            .map(|c| {
                                if ['\\', '\'', '*', '?'].contains(&c) {
                                    format!("\\{c}")
                                } else {
                                    c.to_string()
                                }
                            })
                            .collect::<String>();
                        format!(
                            "kMDItemContentType == 'com.apple.application-bundle' && kMDItemDisplayName == '{escaped}'"
                        )
                    }
                };
                let output = command("/usr/bin/mdfind", &[&query]).await?;
                let candidates: Vec<_> = output
                    .lines()
                    .map(str::trim)
                    .filter(|s| s.ends_with(".app"))
                    .map(PathBuf::from)
                    .collect();
                let matching: Vec<_> = candidates
                    .iter()
                    .filter(|p| match reference {
                        ToolActivityNativeAppReference::AppId { .. } => true,
                        ToolActivityNativeAppReference::DisplayName { display_name } => {
                            p.file_stem().is_some_and(|s| {
                                s.to_string_lossy().to_lowercase() == display_name.to_lowercase()
                            })
                        }
                    })
                    .collect();
                let candidates = if matching.is_empty() {
                    candidates.iter().collect::<Vec<_>>()
                } else {
                    matching
                };
                let mut best: Option<(PathBuf, String)> = None;
                for path in candidates {
                    let last = command(
                        "/usr/bin/mdls",
                        &[
                            "-raw",
                            "-name",
                            "kMDItemLastUsedDate",
                            &path.to_string_lossy(),
                        ],
                    )
                    .await
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                    if best.as_ref().is_none_or(|(_, previous)| last > *previous) {
                        best = Some((path.clone(), last));
                    }
                }
                let Some((path, _)) = best else {
                    return Ok(None);
                };
                path
            }
        };
        let app =
            paths::canonicalize(&app_path).map_err(|e| io_error("realpath-app", app_path, e))?;
        let plist = app.join("Contents/Info.plist");
        let resources = app.join("Contents/Resources");
        let mut icon_name = plist_value(&plist, "CFBundleIconFile").await;
        if icon_name.is_empty() {
            icon_name = plist_value(&plist, "CFBundleIconName").await;
        }
        if !icon_name.is_empty()
            && Path::new(&icon_name)
                .file_name()
                .is_none_or(|s| s != icon_name.as_str())
        {
            return Ok(None);
        }
        let mut choices = Vec::new();
        if !icon_name.is_empty() {
            choices.push(
                resources.join(if Path::new(&icon_name).extension().is_some() {
                    icon_name
                } else {
                    format!("{icon_name}.icns")
                }),
            );
        }
        choices.push(resources.join("AppIcon.icns"));
        let mut dir = tokio::fs::read_dir(&resources)
            .await
            .map_err(|e| io_error("read-directory", &resources, e))?;
        let mut names = Vec::new();
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| io_error("read-directory", &resources, e))?
        {
            names.push(entry.file_name());
        }
        names.sort();
        if let Some(name) = names
            .into_iter()
            .find(|s| s.to_string_lossy().to_lowercase().ends_with(".icns"))
        {
            choices.push(resources.join(name));
        }
        let Some(source) = choices.into_iter().find(|p| p.is_file()) else {
            return Ok(None);
        };
        let source =
            paths::canonicalize(&source).map_err(|e| io_error("realpath-icon", source, e))?;
        if !paths::within(&source, &resources) {
            return Ok(None);
        }
        let mut version = plist_value(&plist, "CFBundleVersion").await;
        if version.is_empty() {
            version = plist_value(&plist, "CFBundleShortVersionString").await;
        }
        let key = hex(&Sha256::digest(format!(
            "{}\0{version}\0{}",
            app.display(),
            source.display()
        )));
        let directory = self.provider_status_cache_dir.join("native-app-icons");
        let output = directory.join(format!("{key}.png"));
        if output.is_file() {
            return Ok(Some(output));
        }
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|e| io_error("make-directory", &directory, e))?;
        let pending = directory.join(format!(
            ".{key}-{}-{}.png",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let _cleanup = PendingIcon(pending.clone());
        command(
            "/usr/bin/sips",
            &[
                "-z",
                "64",
                "64",
                "-s",
                "format",
                "png",
                &source.to_string_lossy(),
                "--out",
                &pending.to_string_lossy(),
            ],
        )
        .await?;
        tokio::fs::rename(&pending, &output)
            .await
            .map_err(|e| io_error("rename-icon", &output, e))?;
        Ok(output.is_file().then_some(output))
    }
}
async fn command(program: &str, args: &[&str]) -> Result<String> {
    let mut options = t3_process::ProcessOptions::new(program).args(args.iter().copied());
    options.timeout = Some(Duration::from_secs(5));
    options.max_output_bytes = 4 * 1024 * 1024;
    Ok(t3_process::run(options).await?.stdout_text())
}
async fn plist_value(plist: &Path, key: &str) -> String {
    command(
        "/usr/bin/plutil",
        &["-extract", key, "raw", "-o", "-", &plist.to_string_lossy()],
    )
    .await
    .unwrap_or_default()
    .trim()
    .into()
}
struct PendingIcon(PathBuf);
impl Drop for PendingIcon {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn app_id_query(app_id: &str) -> Result<String> {
    if app_id.is_empty()
        || app_id.len() > 512
        || !app_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(crate::Error::Invalid("invalid app id".into()));
    }
    Ok(format!("kMDItemCFBundleIdentifier == '{app_id}'"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spotlight_app_ids_require_the_contract_pattern_and_length() {
        for app in ["bad' || kMDItemContentType == '*'", "", &"x".repeat(513)] {
            assert!(app_id_query(app).is_err(), "{app}");
        }
        assert!(app_id_query("com.example.App-1_2").is_ok());
    }
}
