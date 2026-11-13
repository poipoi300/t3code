use crate::{Result, files};
use notify::{RecursiveMode, Watcher};
use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct WatchHandle {
    task: tokio::task::JoinHandle<()>,
}
impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) fn watch<F, Fut>(path: &Path, follow_symlink: bool, reload: F) -> Result<WatchHandle>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let path = path.to_owned();
    let parent = path.parent().unwrap_or(Path::new(".")).to_owned();
    std::fs::create_dir_all(&parent)
        .map_err(|e| files::io("prepare watch directory", &parent, e))?;
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })?;
    watcher.watch(&parent, RecursiveMode::NonRecursive)?;
    let mut link_target = if follow_symlink {
        files::target(&path).ok().filter(|p| *p != path)
    } else {
        None
    };
    let mut extra_dir = watch_target(&mut watcher, &parent, link_target.as_deref())?;
    let task = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if !relevant(event, &path, link_target.as_deref()) {
                continue;
            }
            let delay = tokio::time::sleep(Duration::from_millis(100));
            tokio::pin!(delay);
            loop {
                tokio::select! {
                    _ = &mut delay => break,
                    event = events.recv() => match event {
                        Some(event) => { if relevant(event, &path, link_target.as_deref()) { delay.as_mut().reset(tokio::time::Instant::now() + Duration::from_millis(100)); } },
                        None => return,
                    }
                }
            }
            if follow_symlink {
                let next = files::target(&path).ok().filter(|p| *p != path);
                if next != link_target {
                    if let Some(dir) = extra_dir.take() {
                        let _ = watcher.unwatch(&dir);
                    }
                    match watch_target(&mut watcher, &parent, next.as_deref()) {
                        Ok(dir) => extra_dir = dir,
                        Err(error) => {
                            tracing::warn!(%error, "Failed to watch settings symlink target")
                        }
                    }
                    link_target = next;
                }
            }
            if let Err(error) = reload().await {
                tracing::warn!(%error, "Configuration reload failed");
            }
        }
    });
    Ok(WatchHandle { task })
}
fn watch_target(
    watcher: &mut notify::RecommendedWatcher,
    parent: &Path,
    target: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let Some(directory) = target.and_then(Path::parent).filter(|p| *p != parent) else {
        return Ok(None);
    };
    std::fs::create_dir_all(directory)
        .map_err(|e| files::io("prepare target directory", directory, e))?;
    watcher.watch(directory, RecursiveMode::NonRecursive)?;
    Ok(Some(directory.to_owned()))
}
fn relevant(event: notify::Result<notify::Event>, path: &Path, target: Option<&Path>) -> bool {
    match event {
        Ok(e) => {
            !matches!(e.kind, notify::EventKind::Access(_))
                && e.paths
                    .iter()
                    .any(|p| same_path(p, path) || target.is_some_and(|t| same_path(p, t)))
        }
        Err(error) => {
            tracing::warn!(%error, "Configuration watch failed");
            false
        }
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if left.file_name() != right.file_name() {
        return false;
    }
    fn normalized(path: &Path) -> Option<PathBuf> {
        Some(
            std::fs::canonicalize(path.parent()?)
                .ok()?
                .join(path.file_name()?),
        )
    }
    normalized(left).is_some_and(|p| Some(p) == normalized(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_events_match_a_symlinked_home_even_after_file_removal() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        let link = temp.path().join("home");
        std::fs::create_dir(&real).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real, &link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        for filename in ["settings.json", "keybindings.json"] {
            let path = link.join(filename);
            let canonical = std::fs::canonicalize(&real).unwrap().join(filename);
            let event =
                notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::File))
                    .add_path(canonical);
            assert!(relevant(Ok(event), &path, None));
        }
    }
}
