//! Port of projectFilesQueryState.ts `useProjectFileQuery` for read-only previews: one shared read
//! per (environment, workspace, path), re-read on a new session, on refresh and on each workspace
//! mutation. Contents survive a failed refresh; a folder error overrides them.
use super::{CANNOT_READ_HOST_FILES, NOT_CONNECTED, ProjectFiles, problem, tree::Problem};
use crate::{
    platform::Platform,
    rpc::{RpcCallError, RpcEndpoint},
    types::EnvironmentId,
};
use futures::{FutureExt, StreamExt, channel::mpsc, future::AbortHandle, stream::LocalBoxStream};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use t3_contracts::{
    base_schemas::TrimmedNonEmptyString,
    project::{
        ProjectFileFailure, ProjectReadFileError, ProjectReadFileInput, ProjectReadFileResult,
    },
    rpc::WsProjectsReadFileRpc,
};

pub const WORKSPACE_QUERY_FAILED: &str = "Workspace query failed.";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileReadView {
    pub data: Option<Rc<ProjectReadFileResult>>,
    /// The line the panel shows when there is no data: access, a missing file or the failure.
    pub error: Option<Problem>,
    /// `'{path}' does not exist.`: an `allowMissing` read of a file that is not there.
    pub missing: bool,
    pub read_error: Option<Rc<ProjectReadFileError>>,
    pub pending: bool,
    pub is_not_file: bool,
    pub is_file: bool,
}

#[derive(Default)]
pub(crate) struct ReadState {
    reads: RefCell<BTreeMap<(EnvironmentId, String, String), Weak<ReadShared>>>,
}

struct ReadShared {
    state: RefCell<Settled>,
    listeners: RefCell<Vec<mpsc::UnboundedSender<()>>>,
    kick: mpsc::UnboundedSender<()>,
    task: Cell<Option<AbortHandle>>,
    handled_mutation: RefCell<Option<String>>,
    reads: Cell<u64>,
}
#[derive(Default)]
struct Settled {
    data: Option<Rc<ProjectReadFileResult>>,
    error: Option<Problem>,
    read_error: Option<Rc<ProjectReadFileError>>,
    waiting: bool,
    access_pending: bool,
    access_error: Option<Problem>,
}
impl Drop for ReadShared {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl ReadShared {
    fn notify(&self) {
        self.listeners
            .borrow_mut()
            .retain(|listener| listener.unbounded_send(()).is_ok());
    }
}

/// A screen's hold on one file read.
#[derive(Clone)]
pub struct FileRead {
    shared: Rc<ReadShared>,
}

impl FileRead {
    pub fn view(&self) -> FileReadView {
        let state = self.shared.state.borrow();
        if state.access_pending {
            return FileReadView {
                pending: true,
                ..FileReadView::default()
            };
        }
        if let Some(error) = &state.access_error {
            return FileReadView {
                error: Some(error.clone()),
                ..FileReadView::default()
            };
        }
        let missing = state
            .data
            .as_ref()
            .is_some_and(|d| d.exists.flatten() == Some(false));
        let data = if missing { None } else { state.data.clone() };
        let is_not_file = state
            .read_error
            .as_ref()
            .is_some_and(|e| e.failure.clone().flatten() == Some(ProjectFileFailure::PathNotFile));
        let binary = state
            .read_error
            .as_ref()
            .is_some_and(|e| e.failure.clone().flatten() == Some(ProjectFileFailure::BinaryFile));
        FileReadView {
            is_file: !is_not_file && (data.is_some() || binary),
            data,
            error: (!missing).then(|| state.error.clone()).flatten(),
            missing,
            read_error: state.read_error.clone(),
            pending: state.waiting,
            is_not_file,
        }
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        let (sender, receiver) = mpsc::unbounded();
        self.shared.listeners.borrow_mut().push(sender);
        receiver.boxed_local()
    }
    pub fn refresh(&self) {
        let _ = self.shared.kick.unbounded_send(());
    }
    /// The panel mounts the read under the workspace mutation id, so a new one re-reads it.
    pub fn observe_mutation(&self, mutation: Option<&str>) {
        let Some(mutation) = mutation else {
            return;
        };
        let mut handled = self.shared.handled_mutation.borrow_mut();
        if handled.as_deref() == Some(mutation) {
            return;
        }
        let first = handled.is_none();
        *handled = Some(mutation.into());
        if !first {
            self.refresh();
        }
    }
    /// Reads sent, for tests that count work.
    pub fn reads(&self) -> u64 {
        self.shared.reads.get()
    }
}

impl<P: Platform + Clone + 'static> ProjectFiles<P> {
    /// `projects.readFile` for `relative_path` (`allowMissing` for the project file `t3.json`).
    pub fn read_file(
        &self,
        environment: &EnvironmentId,
        cwd: &str,
        relative_path: &str,
    ) -> FileRead {
        let key = (
            environment.clone(),
            cwd.to_owned(),
            relative_path.to_owned(),
        );
        if let Some(shared) = self
            .state
            .reads
            .reads
            .borrow()
            .get(&key)
            .and_then(Weak::upgrade)
        {
            return FileRead { shared };
        }
        let (kick, kicks) = mpsc::unbounded();
        let shared = Rc::new(ReadShared {
            state: RefCell::new(Settled {
                waiting: true,
                ..Settled::default()
            }),
            listeners: RefCell::default(),
            kick,
            task: Cell::new(None),
            handled_mutation: RefCell::default(),
            reads: Cell::new(0),
        });
        let task = self.runtime.spawn(
            drive_read(
                self.clone(),
                Rc::downgrade(&shared),
                environment.clone(),
                cwd.to_owned(),
                relative_path.to_owned(),
                kicks,
            )
            .boxed_local(),
        );
        shared.task.set(Some(task));
        let mut reads = self.state.reads.reads.borrow_mut();
        reads.retain(|_, read| read.strong_count() > 0);
        reads.insert(key, Rc::downgrade(&shared));
        FileRead { shared }
    }
}

async fn drive_read<P: Platform + Clone + 'static>(
    files: ProjectFiles<P>,
    weak: Weak<ReadShared>,
    environment: EnvironmentId,
    cwd: String,
    relative_path: String,
    mut kicks: mpsc::UnboundedReceiver<()>,
) {
    let mut changes = files.changes(&environment).fuse();
    let mut session = None;
    let mut read_needed = true;
    loop {
        let access = files.read_access(&environment);
        let endpoint = files.endpoint(&environment);
        let current = files
            .runtime
            .store()
            .snapshot()
            .environments
            .get(&environment)
            .map(|e| e.session.0);
        if current != session && endpoint.is_some() {
            session = current;
            read_needed = true;
        }
        {
            let Some(shared) = weak.upgrade() else { return };
            let mut state = shared.state.borrow_mut();
            state.access_pending = access.pending;
            state.access_error = (!access.allowed && !access.pending).then(|| {
                access
                    .error
                    .as_deref()
                    .map_or_else(|| Problem::client(CANNOT_READ_HOST_FILES), Problem::client)
            });
            drop(state);
            shared.notify();
        }
        if access.allowed && read_needed {
            read_needed = false;
            if let Some(shared) = weak.upgrade() {
                shared.state.borrow_mut().waiting = true;
                shared.reads.set(shared.reads.get() + 1);
                shared.notify();
            }
            let outcome = match endpoint {
                None => Err((Problem::client(NOT_CONNECTED), None)),
                Some(endpoint) => {
                    let input = ProjectReadFileInput {
                        cwd: TrimmedNonEmptyString(cwd.clone()),
                        relative_path: relative_path.clone(),
                        allow_missing: (relative_path == super::preview::T3_PROJECT_FILE_NAME)
                            .then_some(Some(true)),
                    };
                    match endpoint.command::<WsProjectsReadFileRpc>(input).await {
                        Ok(result) => Ok(result),
                        Err(RpcCallError::Rejected(errors)) => {
                            let typed = errors.iter().find_map(|e| {
                                serde_json::from_value::<ProjectReadFileError>(e.clone()).ok()
                            });
                            let problem = typed.as_ref().map_or_else(
                                || Problem::client(WORKSPACE_QUERY_FAILED),
                                |e| Problem::server(e.message.0.clone()),
                            );
                            Err((problem, typed))
                        }
                        Err(error) => Err((
                            problem(&error.into_client_error(), WORKSPACE_QUERY_FAILED),
                            None,
                        )),
                    }
                }
            };
            let Some(shared) = weak.upgrade() else { return };
            let mut state = shared.state.borrow_mut();
            state.waiting = false;
            match outcome {
                Ok(result) => {
                    state.data = Some(Rc::new(result));
                    state.error = None;
                    state.read_error = None;
                }
                Err((problem, typed)) => {
                    state.error = Some(problem);
                    state.read_error = typed.map(Rc::new);
                }
            }
            drop(state);
            shared.notify();
        }
        futures::select! {
            kick = kicks.next() => match kick {
                None => return,
                Some(()) => read_needed = true,
            },
            change = changes.next().fuse() => if change.is_none() { return },
        }
    }
}
