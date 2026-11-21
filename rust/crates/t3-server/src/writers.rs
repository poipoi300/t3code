//! Admission for the host's writers in a folder (t3-git H-2). A checkpoint file restore holds its
//! folder and thread: terminals, setup scripts, file writes and Git actions in an overlapping
//! folder, or for that thread, wait until the restore returns. Those already admitted when the
//! hold is taken are named, and the restore refuses rather than racing them.
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};
use t3_workspace::restore::RestoreFolder;

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
    folders: Vec<RestoreFolder>,
}

impl Claim {
    fn conflicts(&self, other: &Claim) -> bool {
        matches!((&self.thread, &other.thread), (Some(a), Some(b)) if a == b)
            || self
                .folders
                .iter()
                .any(|a| other.folders.iter().any(|b| a.overlaps(b)))
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
        self.enter_folders(name, thread, &folder.into_iter().collect::<Vec<_>>())
            .await
    }

    pub async fn enter_folders(
        self: &Arc<Self>,
        name: impl Into<String>,
        thread: Option<&str>,
        folders: &[&Path],
    ) -> Admission {
        let name = name.into();
        let mut waiting = None;
        loop {
            let claim = Claim {
                thread: thread.map(str::to_owned),
                folders: folders.iter().map(|p| RestoreFolder::resolve(p)).collect(),
            };
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
            folders: vec![RestoreFolder::resolve(folder)],
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
    RestoreFolder::resolve(a).overlaps(&RestoreFolder::resolve(b))
}

/// File and Git RPCs still write when the terminal feature is disabled.
#[cfg(all(feature = "orchestration", not(feature = "terminal")))]
pub(crate) struct RpcWriters(pub Arc<FolderGate>);

#[cfg(all(feature = "orchestration", not(feature = "terminal")))]
#[async_trait::async_trait]
impl t3_orchestration::api::ThreadWriters for RpcWriters {
    async fn hold(
        &self,
        thread: t3_orchestration::api::ThreadId,
        cwd: std::path::PathBuf,
    ) -> t3_orchestration::error::Result<t3_orchestration::api::WriterHold> {
        let (guard, running) = self.0.hold(&thread.0, &cwd);
        Ok(t3_orchestration::api::WriterHold {
            guard: Box::new(guard),
            running,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn assert_same_claim(a: &Path, b: &Path) {
        for (writer, restore) in [(a, b), (b, a)] {
            let gate = Arc::new(FolderGate::default());
            let first = gate.enter("writer", None, Some(writer)).await;
            let (hold, running) = gate.hold("other-thread", restore);
            assert_eq!(running, ["writer"], "{writer:?} vs {restore:?}");
            drop(first);
            let next = gate.enter("next", None, Some(writer));
            tokio::pin!(next);
            tokio::select! {
                biased;
                _ = &mut next => panic!("writer entered a held folder"),
                _ = std::future::ready(()) => {}
            }
            drop(hold);
            next.await;
        }
    }

    #[tokio::test]
    async fn folder_claims_resolve_alternate_spellings_and_missing_tails() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("Workspace");
        std::fs::create_dir(&real).unwrap();
        std::fs::create_dir(root.path().join("sibling")).unwrap();
        let missing = real.join("future");
        assert_same_claim(&missing, &root.path().join("sibling/../Workspace/future")).await;
        #[cfg(unix)]
        {
            let link = root.path().join("linked");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_same_claim(&missing, &link.join("future")).await;
        }
        if root.path().join("workspace").is_dir() {
            assert_same_claim(&missing, &root.path().join("workspace/future")).await;
        }
    }
}
