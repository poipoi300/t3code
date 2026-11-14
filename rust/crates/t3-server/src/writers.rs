//! Admission for the host's writers in a folder (t3-git H-2). A checkpoint file restore holds its
//! folder and thread: terminals, setup scripts, file writes and Git actions in an overlapping
//! folder, or for that thread, wait until the restore returns. Those already admitted when the
//! hold is taken are named, and the restore refuses rather than racing them.
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub struct FolderGate {
    state: Mutex<State>,
    released: tokio::sync::Notify,
    waiting: tokio::sync::watch::Sender<usize>,
}

#[derive(Default)]
struct State {
    next: u64,
    holds: BTreeMap<u64, Claim>,
    admitted: BTreeMap<u64, (String, Claim)>,
}

struct Claim {
    thread: Option<String>,
    folder: Option<PathBuf>,
}

impl Claim {
    fn conflicts(&self, other: &Claim) -> bool {
        matches!((&self.thread, &other.thread), (Some(a), Some(b)) if a == b)
            || matches!((&self.folder, &other.folder), (Some(a), Some(b)) if overlaps(a, b))
    }
}

/// An admitted writer; released when dropped.
pub struct Admission {
    gate: Arc<FolderGate>,
    id: u64,
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.gate.lock().admitted.remove(&self.id);
    }
}

/// A restore's hold; new conflicting writers wait until it drops.
pub struct Hold {
    gate: Arc<FolderGate>,
    id: u64,
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.gate.lock().holds.remove(&self.id);
        self.gate.released.notify_waiters();
    }
}

struct Waiting<'a>(&'a FolderGate);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.waiting.send_modify(|count| *count -= 1);
    }
}

impl FolderGate {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Admits a writer named as a user would see it (such as "terminal `dev`"), for a thread
    /// and/or a folder, waiting while a conflicting restore holds them.
    pub async fn enter(
        self: &Arc<Self>,
        name: impl Into<String>,
        thread: Option<&str>,
        folder: Option<&Path>,
    ) -> Admission {
        let claim = Claim {
            thread: thread.map(str::to_owned),
            folder: folder.map(plain),
        };
        let name = name.into();
        let mut waiting = None;
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            {
                let mut state = self.lock();
                if !state.holds.values().any(|hold| hold.conflicts(&claim)) {
                    state.next += 1;
                    let id = state.next;
                    state.admitted.insert(id, (name, claim));
                    return Admission {
                        gate: self.clone(),
                        id,
                    };
                }
            }
            if waiting.is_none() {
                self.waiting.send_modify(|count| *count += 1);
                waiting = Some(Waiting(self));
            }
            released.await;
        }
    }

    /// Holds `thread` and `folder` against new writers and names the writers already admitted.
    pub fn hold(self: &Arc<Self>, thread: &str, folder: &Path) -> (Hold, Vec<String>) {
        let claim = Claim {
            thread: Some(thread.to_owned()),
            folder: Some(plain(folder)),
        };
        let mut state = self.lock();
        let running = state
            .admitted
            .values()
            .filter(|(_, admitted)| admitted.conflicts(&claim))
            .map(|(name, _)| name.clone())
            .collect();
        state.next += 1;
        let id = state.next;
        state.holds.insert(id, claim);
        (
            Hold {
                gate: self.clone(),
                id,
            },
            running,
        )
    }

    /// How many writers wait on a hold; changes as they start and stop waiting.
    pub fn waiting(&self) -> tokio::sync::watch::Receiver<usize> {
        self.waiting.subscribe()
    }
}

/// Admits a Git action or file write in `folder` through the environment's gate (`domains`),
/// for the call's duration.
pub async fn admit(
    services: &crate::services::ServiceRegistry,
    name: &str,
    folder: &str,
) -> Option<Admission> {
    let gate = services.domain::<FolderGate>()?;
    Some(gate.enter(name, None, Some(Path::new(folder))).await)
}

/// One folder contains the other: a writer in a parent folder can write into the child.
pub fn overlaps(a: &Path, b: &Path) -> bool {
    let (a, b) = (plain(a), plain(b));
    within(&a, &b) || within(&b, &a)
}

fn within(candidate: &Path, root: &Path) -> bool {
    let parts = |path: &Path| -> Vec<String> {
        path.components()
            .filter(|part| !matches!(part, Component::CurDir))
            .map(|part| {
                let text = part.as_os_str().to_string_lossy();
                if cfg!(windows) {
                    text.to_lowercase()
                } else {
                    text.into_owned()
                }
            })
            .collect()
    };
    let (candidate, root) = (parts(candidate), parts(root));
    candidate.len() >= root.len() && candidate.iter().zip(&root).all(|(a, b)| a == b)
}

/// Without the drive verbatim prefix a canonical scope carries, with `/` as `\` on Windows.
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let text = if cfg!(windows) {
        text.replace('/', "\\")
    } else {
        text.into_owned()
    };
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => rest.into(),
        _ => text.into(),
    }
}
