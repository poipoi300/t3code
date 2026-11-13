use crate::{
    ClientError,
    platform::Platform,
    services::resources::{Resource, ResourcePolicy},
    state::shell::ShellState,
    types::{EnvironmentId, Generation, QueryState, ThreadId, ThreadKey},
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_contracts::orchestration_v2::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyRead {
    Pending,
    Failed,
    Settled,
}
pub fn thread_family_read<T>(state: &QueryState<T>) -> FamilyRead {
    if state.loading {
        FamilyRead::Pending
    } else if state.error.is_some() {
        FamilyRead::Failed
    } else if state.value.is_some() {
        FamilyRead::Settled
    } else {
        FamilyRead::Pending
    }
}
pub type FamilyRows = Rc<RefCell<Vec<Rc<OrchestrationV2ThreadFamily>>>>;
enum RowSource {
    Rows(FamilyRows),
    Query {
        generation: u64,
        resource: Resource<OrchestrationV2ThreadFamily>,
    },
}
struct MountedRows {
    rows: RowSource,
    count: usize,
    id: u64,
}
#[derive(Default)]
struct MountState {
    rows: BTreeMap<(EnvironmentId, String), MountedRows>,
    revision: u64,
    merged: BTreeMap<EnvironmentId, MergedFamily>,
    next: u64,
}
struct MergedFamily {
    source: Rc<OrchestrationV2ShellSnapshot>,
    removed: BTreeMap<ThreadId, u64>,
    reads: Vec<Rc<OrchestrationV2ThreadFamily>>,
    result: Rc<OrchestrationV2ShellSnapshot>,
}
#[derive(Clone, Default)]
pub struct FamilyMounts(Rc<RefCell<MountState>>);
pub struct FamilyMount {
    owner: Weak<RefCell<MountState>>,
    key: (EnvironmentId, String),
    released: bool,
    id: u64,
    observer: Option<crate::services::resources::ResourceSubscription<OrchestrationV2ThreadFamily>>,
}
impl FamilyMount {
    pub fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        self.observer.take();
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let mut state = owner.borrow_mut();
        if let Some(entry) = state
            .rows
            .get_mut(&self.key)
            .filter(|entry| entry.id == self.id)
        {
            if entry.count > 1 {
                entry.count -= 1;
            } else {
                state.rows.remove(&self.key);
            }
            state.revision += 1;
        }
    }
}
impl Drop for FamilyMount {
    fn drop(&mut self) {
        self.release();
    }
}
impl FamilyMounts {
    pub fn revision(&self) -> u64 {
        self.0.borrow().revision
    }
    pub fn mount_rows(
        &self,
        environment: EnvironmentId,
        key: String,
        rows: FamilyRows,
    ) -> FamilyMount {
        self.mount_source(environment, key, RowSource::Rows(rows))
    }
    fn mount_source(
        &self,
        environment: EnvironmentId,
        key: String,
        rows: RowSource,
    ) -> FamilyMount {
        let key = (environment, key);
        let mut state = self.0.borrow_mut();
        state.next += 1;
        let next = state.next;
        let entry = state
            .rows
            .entry(key.clone())
            .or_insert_with(|| MountedRows {
                rows,
                count: 0,
                id: next,
            });
        entry.count += 1;
        let id = entry.id;
        state.revision += 1;
        FamilyMount {
            owner: Rc::downgrade(&self.0),
            key,
            released: false,
            id,
            observer: None,
        }
    }
    pub fn merge(
        &self,
        environment: &EnvironmentId,
        shell: &ShellState,
    ) -> Option<Rc<OrchestrationV2ShellSnapshot>> {
        let snapshot = shell.snapshot.as_ref()?;
        let cut = snapshot
            .older_settled
            .as_ref()
            .map_or(0, |cut| cut.as_of_sequence);
        let reads: Vec<_> = self
            .0
            .borrow()
            .rows
            .iter()
            .filter(|((id, _), _)| id == environment)
            .flat_map(|(_, entry)| match &entry.rows {
                RowSource::Rows(rows) => rows.borrow().clone(),
                RowSource::Query {
                    generation,
                    resource,
                } if *generation == shell.generation => {
                    resource.snapshot().value.into_iter().collect()
                }
                _ => vec![],
            })
            .filter(|read| read.snapshot_sequence >= cut)
            .collect();
        if let Some(previous) = self.0.borrow().merged.get(environment)
            && Rc::ptr_eq(&previous.source, snapshot)
            && previous.removed == shell.removed_at
            && previous.reads.len() == reads.len()
            && previous
                .reads
                .iter()
                .zip(&reads)
                .all(|(a, b)| Rc::ptr_eq(a, b))
        {
            return Some(previous.result.clone());
        }
        let mut present: std::collections::BTreeSet<_> =
            snapshot.threads.iter().map(|t| &t.id).collect();
        let mut extras = vec![];
        for read in &reads {
            for thread in &read.threads {
                if !present.contains(&thread.id)
                    && !shell
                        .removed_at
                        .get(&thread.id)
                        .is_some_and(|at| *at > read.snapshot_sequence)
                {
                    present.insert(&thread.id);
                    extras.push(thread.clone());
                }
            }
        }
        let result = if extras.is_empty() {
            snapshot.clone()
        } else {
            let mut merged = (**snapshot).clone();
            merged.threads.extend(extras);
            Rc::new(merged)
        };
        self.0.borrow_mut().merged.insert(
            environment.clone(),
            MergedFamily {
                source: snapshot.clone(),
                removed: shell.removed_at.clone(),
                reads,
                result: result.clone(),
            },
        );
        Some(result)
    }
    pub fn remove_environment(&self, environment: &EnvironmentId) {
        let mut state = self.0.borrow_mut();
        state.rows.retain(|(id, _), _| id != environment);
        state.merged.remove(environment);
        state.revision += 1;
    }
}

#[derive(Default)]
pub struct ThreadFamilies {
    reads: BTreeMap<(ThreadKey, u64), Resource<OrchestrationV2ThreadFamily>>,
    pub mounts: FamilyMounts,
}
impl ThreadFamilies {
    pub fn query(
        &mut self,
        key: ThreadKey,
        generation: u64,
        session: Generation,
    ) -> Option<Resource<OrchestrationV2ThreadFamily>> {
        if generation == 0 {
            return None;
        }
        self.reads.retain(|(old_key, old_generation), resource| {
            let keep = old_key.environment != key.environment || *old_generation == generation;
            if !keep {
                resource.cancel();
            }
            keep
        });
        let resource = self.reads.entry((key, generation)).or_insert_with(|| {
            Resource::new(
                ResourcePolicy {
                    stale_ms: u64::MAX,
                    idle_ttl_ms: 60_000,
                    refresh_ms: None,
                    completed_ttl_ms: None,
                },
                session,
            )
        });
        resource.replace_session(session);
        Some(resource.clone())
    }
    pub fn mount(
        &mut self,
        key: ThreadKey,
        generation: u64,
        session: Generation,
        now: u64,
    ) -> Option<(FamilyMount, Resource<OrchestrationV2ThreadFamily>)> {
        let resource = self.query(key.clone(), generation, session)?;
        if resource.snapshot().error.is_some() {
            resource.invalidate();
        }
        let mount_key = format!("family:{}", key.thread.0);
        if let Some(entry) = self
            .mounts
            .0
            .borrow_mut()
            .rows
            .get_mut(&(key.environment.clone(), mount_key.clone()))
            && matches!(&entry.rows,RowSource::Query {generation: old,..} if *old!=generation)
        {
            entry.rows = RowSource::Query {
                generation,
                resource: resource.clone(),
            };
        }
        let mut mount = self.mounts.mount_source(
            key.environment,
            mount_key,
            RowSource::Query {
                generation,
                resource: resource.clone(),
            },
        );
        mount.observer = Some(resource.observe(now));
        Some((mount, resource))
    }
    pub fn collect(&mut self, now: u64) {
        self.reads.retain(|_, resource| !resource.expired(now));
    }
    pub fn refresh(&self, key: &ThreadKey, generation: u64) {
        if let Some(resource) = self.reads.get(&(key.clone(), generation)) {
            resource.invalidate();
        }
    }
    pub fn remove_environment(&mut self, environment: &EnvironmentId) {
        self.reads.retain(|(key, _), resource| {
            if &key.environment == environment {
                resource.cancel();
                false
            } else {
                true
            }
        });
        self.mounts.remove_environment(environment);
    }
}
pub async fn read_family<
    P: Platform + ?Sized,
    F: std::future::Future<Output = Result<OrchestrationV2ThreadFamily, ClientError>>,
    C: std::future::Future<Output = Result<bool, ClientError>>,
>(
    resource: &Resource<OrchestrationV2ThreadFamily>,
    platform: &P,
    thread: ThreadId,
    config: C,
    read: impl FnOnce(ThreadId) -> F,
) -> Result<(), ClientError> {
    resource
        .run(platform, |_| async move {
            if config.await? {
                read(thread).await
            } else {
                Ok(OrchestrationV2ThreadFamily {
                    snapshot_sequence: 0,
                    threads: vec![],
                })
            }
        })
        .await
}
